use gpui::Corners;
use std::{
    ops::Range,
    rc::Rc,
    sync::{Arc, Mutex},
};

use gpui::{
    App, AppContext as _, BorderStyle, Bounds, ClickEvent, CursorStyle, Edges, Element, ElementId,
    GlobalElementId, Half, HighlightStyle, Hitbox, HitboxBehavior, InspectorElementId, IntoElement,
    LayoutId, MouseButton, MouseClickEvent, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels,
    Point, SharedString, StyledText, TextLayout, Window, point, px, quad,
};

use crate::{
    ActiveTheme,
    global_state::UiGlobalState,
    input::Selection,
    text::TextViewMultiClickKind,
    text::node::LinkMark,
    text::selection::word_range_at,
    text::state::LineSpan,
    text::text_view::{LinkClickHandlerFn, handle_link_click},
};

/// A inline element used to render a inline text and support selectable.
///
/// All text in TextView (including the CodeBlock) used this for text rendering.
pub(super) struct Inline {
    id: ElementId,
    text: SharedString,
    source_text: SharedString,
    padding: CodePadding,
    links: Rc<Vec<(Range<usize>, LinkMark)>>,
    highlights: Vec<(Range<usize>, HighlightStyle)>,
    styled_text: StyledText,
    font_runs: Vec<(Range<usize>, SharedString)>,
    link_click_handler: Option<Arc<LinkClickHandlerFn>>,

    state: Arc<Mutex<InlineState>>,
}

/// The inline text state, used RefCell to keep the selection state.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct InlineState {
    hovered_index: Option<usize>,
    /// The text that actually rendering, matched with selection.
    pub(super) text: SharedString,
    pub(super) selection: Option<Selection>,
}

impl InlineState {
    /// Save actually rendered text for selected text to use.
    pub(crate) fn set_text(&mut self, text: SharedString) {
        self.text = text;
    }
}

impl Inline {
    pub(super) fn new(
        id: impl Into<ElementId>,
        state: Arc<Mutex<InlineState>>,
        links: Vec<(Range<usize>, LinkMark)>,
        highlights: Vec<(Range<usize>, HighlightStyle)>,
        link_click_handler: Option<Arc<LinkClickHandlerFn>>,
    ) -> Self {
        let text = state
            .lock()
            .map(|state| state.text.clone())
            .unwrap_or_default();

        Self {
            id: id.into(),
            links: Rc::new(links),
            highlights,
            text: text.clone(),
            source_text: text.clone(),
            padding: CodePadding::default(),
            styled_text: StyledText::new(text),
            font_runs: Vec::new(),
            link_click_handler,
            state,
        }
    }

    pub(super) fn font_runs(mut self, font_runs: Vec<(Range<usize>, SharedString)>) -> Self {
        if font_runs.is_empty() {
            return self;
        }
        let (text, padding) = CodePadding::new(&self.source_text, &font_runs);
        self.text = text;
        self.links = Rc::new(
            self.links
                .iter()
                .map(|(range, link)| (padding.display_range(range.clone()), link.clone()))
                .collect(),
        );
        self.highlights = self
            .highlights
            .into_iter()
            .map(|(range, style)| (padding.display_range(range), style))
            .collect();
        self.font_runs = font_runs
            .into_iter()
            .map(|(range, font)| (padding.display_range(range), font))
            .collect();
        self.padding = padding;
        self
    }

    /// Get link at given mouse position.
    fn link_for_position(
        layout: &TextLayout,
        links: &Vec<(Range<usize>, LinkMark)>,
        position: Point<Pixels>,
    ) -> Option<LinkMark> {
        let offset = layout.index_for_position(position).ok()?;
        for (range, link) in links.iter() {
            if range.contains(&offset) {
                return Some(link.clone());
            }
        }

        None
    }

    /// Paint selected bounds for debug.
    #[allow(unused)]
    fn paint_selected_bounds(&self, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut App) {
        window.paint_quad(gpui::PaintQuad {
            bounds,
            background: {
                let mut color = cx.theme().blue;
                color.alpha = 0.01;
                color.into()
            },
            corner_radii: Corners::default(),
            border_color: gpui::transparent_black().into(),
            border_style: BorderStyle::default(),
            border_widths: gpui::Edges::all(px(0.)),
            ..gpui::fill(bounds, gpui::transparent_black())
        });
    }

    fn layout_selections(
        &self,
        text_layout: &TextLayout,
        bounds: &Bounds<Pixels>,
        window: &mut Window,
        cx: &mut App,
    ) -> (bool, bool, Option<Selection>) {
        let Some(text_view_state) = UiGlobalState::global(cx).text_view_state() else {
            return (false, false, None);
        };

        let text_view_state = text_view_state.read(cx);
        let is_selectable = text_view_state.is_selectable();
        if !is_selectable {
            return (false, false, None);
        }

        if text_view_state.is_all_selected() {
            return (
                is_selectable,
                true,
                Some((0..self.source_text.len()).into()),
            );
        }

        if let Some(selection) = text_view_state.multi_click_selection() {
            return (
                is_selectable,
                true,
                selection_for_multi_click(
                    &self.text,
                    text_layout,
                    *bounds,
                    selection.pos,
                    selection.kind,
                )
                .map(|range| Selection::from(self.padding.source_range(range))),
            );
        }

        let Some((selection_start, selection_end)) = text_view_state.selection_points(cx) else {
            return (is_selectable, false, None);
        };
        let line_height = window.line_height();

        // Use for debug selection bounds
        // self.paint_selected_bounds(Bounds::from_corners(selection_start, selection_end), window, cx);

        // NOTE: the selection is computed purely from the geometric band
        // (`selection_start`..`selection_end`), NOT from what is currently
        // visible. Every glyph of a *painted* element is laid out (its
        // `position_for_index` is valid) even when it is scrolled out of, or
        // clipped by, an ancestor's viewport — the content mask only clips the
        // painted pixels. Because the copied text is derived from
        // `InlineState.selection`, gating the selection on `content_mask` here
        // used to drop scrolled-out-but-selected glyphs, so a selection taller
        // than the viewport (e.g. a long chat message, or a drag with
        // auto-scroll) copied only the portion that happened to be on screen.
        //
        // This does not resurrect the #2156 clipped-hit-testing behavior: a
        // selection can only START on visible text (window selection resolves
        // endpoints with hitbox hover testing against visible Inline bounds),
        // so the band's endpoints are always anchored to on-screen text.
        // Content that is merely `overflow_hidden`
        // (not scrolled) lies outside that band and is still excluded, while
        // the highlight quads painted for off-screen glyphs are clipped away by
        // GPUI's content mask as before.
        let mut selection: Option<Selection> = None;
        let mut offset = 0;
        let mut chars = self.text.chars().peekable();
        while let Some(c) = chars.next() {
            let Some(pos) = text_layout.position_for_index(offset) else {
                offset += c.len_utf8();
                continue;
            };

            let next_offset = offset + c.len_utf8();
            let mut char_width = line_height.half();
            if let Some(next_pos) = text_layout.position_for_index(next_offset) {
                if next_pos.y == pos.y {
                    char_width = next_pos.x - pos.x;
                }
            }

            if point_in_text_selection(pos, char_width, selection_start, selection_end, line_height)
            {
                if selection.is_none() {
                    selection = Some((offset..offset).into());
                }

                if let Some(selection) = selection.as_mut() {
                    selection.end = next_offset;
                }
            }

            offset = next_offset;
        }

        (
            true,
            true,
            selection.map(|selection| {
                Selection::from(self.padding.source_range(selection.start..selection.end))
            }),
        )
    }

    fn text_line_bounds(
        &self,
        text_layout: &TextLayout,
        line_height: Pixels,
        mask_bounds: Bounds<Pixels>,
    ) -> Vec<Bounds<Pixels>> {
        let mut line_bounds = Vec::new();
        let mut current_line_y = None;
        let mut current_bounds: Option<Bounds<Pixels>> = None;
        let mut offset = 0;

        for c in self.text.chars() {
            let next_offset = offset + c.len_utf8();
            let Some(pos) = text_layout.position_for_index(offset) else {
                offset = next_offset;
                continue;
            };

            let mut char_width = line_height.half();
            if let Some(next_pos) = text_layout.position_for_index(next_offset) {
                if next_pos.y == pos.y {
                    char_width = next_pos.x - pos.x;
                }
            }

            let bounds = Bounds::from_corners(pos, point(pos.x + char_width, pos.y + line_height))
                .intersect(&mask_bounds);
            if bounds.size.width > px(0.) && bounds.size.height > px(0.) {
                if current_line_y == Some(pos.y) {
                    if let Some(current) = current_bounds.as_mut() {
                        *current = current.union(&bounds);
                    }
                } else {
                    if let Some(current) = current_bounds.take() {
                        line_bounds.push(current);
                    }
                    current_line_y = Some(pos.y);
                    current_bounds = Some(bounds);
                }
            }

            offset = next_offset;
        }

        if let Some(current) = current_bounds {
            line_bounds.push(current);
        }

        line_bounds
    }

    /// Paint the selection background.
    fn paint_selection(
        selection: &Selection,
        text_layout: &TextLayout,
        bounds: &Bounds<Pixels>,
        window: &mut Window,
        color: gpui::Hsla,
        radius: Pixels,
    ) {
        let mut start = selection.start;
        let mut end = selection.end;
        if end < start {
            std::mem::swap(&mut start, &mut end);
        }
        let Some(start_position) = text_layout.position_for_index(start) else {
            return;
        };
        let Some(end_position) = text_layout.position_for_index(end) else {
            return;
        };

        let line_height = text_layout.line_height();
        if start_position.y == end_position.y {
            window.paint_quad(quad(
                Bounds::from_corners(
                    start_position,
                    point(end_position.x, end_position.y + line_height),
                ),
                radius,
                color,
                Edges::default(),
                gpui::transparent_black(),
                BorderStyle::default(),
            ));
        } else {
            window.paint_quad(quad(
                Bounds::from_corners(
                    start_position,
                    point(bounds.right(), start_position.y + line_height),
                ),
                radius,
                color,
                Edges::default(),
                gpui::transparent_black(),
                BorderStyle::default(),
            ));

            if end_position.y > start_position.y + line_height {
                window.paint_quad(quad(
                    Bounds::from_corners(
                        point(bounds.left(), start_position.y + line_height),
                        point(bounds.right(), end_position.y),
                    ),
                    radius,
                    color,
                    Edges::default(),
                    gpui::transparent_black(),
                    BorderStyle::default(),
                ));
            }

            window.paint_quad(quad(
                Bounds::from_corners(
                    point(bounds.left(), end_position.y),
                    point(end_position.x, end_position.y + line_height),
                ),
                radius,
                color,
                Edges::default(),
                gpui::transparent_black(),
                BorderStyle::default(),
            ));
        }
    }
}

impl IntoElement for Inline {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for Inline {
    type RequestLayoutState = ();
    type PrepaintState = Hitbox;

    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone())
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        global_element_id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let text_style = window.text_style();

        let hovered_index = self.state.lock().ok().and_then(|state| state.hovered_index);
        let hovered_link = self
            .links
            .iter()
            .find(|(range, _)| hovered_index.is_some_and(|index| range.contains(&index)));
        let hover_highlights = hovered_link.into_iter().map(|(range, _)| {
            (
                range.clone(),
                HighlightStyle {
                    underline: Some(gpui::UnderlineStyle {
                        thickness: px(1.),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
            )
        });
        let highlights = gpui::combine_highlights(self.highlights.clone(), hover_highlights);
        let mut runs = Vec::new();
        let mut ix = 0;
        for (range, mut highlight) in highlights {
            if self
                .links
                .iter()
                .any(|(link, _)| link.start <= range.start && link.end >= range.end)
                || self
                    .font_runs
                    .iter()
                    .any(|(code, _)| code.start <= range.start && code.end >= range.end)
            {
                // Link and inline-code backgrounds are painted with rounded corners below.
                highlight.background_color = None;
            }
            if ix < range.start {
                runs.push(text_style.clone().to_run(range.start - ix));
            }
            runs.push(text_style.clone().highlight(highlight).to_run(range.len()));
            ix = range.end;
        }
        if ix < self.text.len() {
            runs.push(text_style.to_run(self.text.len() - ix));
        }

        self.styled_text =
            StyledText::new(self.text.clone()).with_runs(apply_font_runs(runs, &self.font_runs));
        let (layout_id, _) =
            self.styled_text
                .request_layout(global_element_id, inspector_id, window, cx);

        (layout_id, ())
    }

    fn prepaint(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        self.styled_text
            .prepaint(id, inspector_id, bounds, &mut (), window, cx);

        // Report this element's laid-out extent so an ancestor TextView with
        // `max_lines` can snap its clip to a whole-line boundary. The state
        // stack only holds an entry during prepaint when that view set
        // `max_lines`, so this is a no-op otherwise.
        if let Some(text_view_state) = UiGlobalState::global(cx).text_view_state().cloned() {
            let state = text_view_state.read(cx);
            if state.max_lines.is_some()
                && let Ok(mut line_spans) = state.line_spans.lock()
            {
                line_spans.push(LineSpan {
                    top: bounds.top(),
                    bottom: bounds.bottom(),
                    line_height: window.line_height(),
                });
            }
        }

        let hitbox = window.insert_hitbox(bounds, HitboxBehavior::Normal);
        let mouse_position = window.mouse_position();
        let text_layout = self.styled_text.layout().clone();
        if let Some(link) = Self::link_for_position(&text_layout, &self.links, mouse_position) {
            if hitbox.is_hovered(window) {
                let hitbox = hitbox.clone();
                let layout = text_layout.clone();
                let links = self.links.clone();
                let url = link.url.clone();
                let view = crate::tooltip::Tooltip::new(link.url).build(window, cx);
                window.set_tooltip(gpui::AnyTooltip {
                    view,
                    mouse_position,
                    check_visible_and_update: Rc::new(move |_, window, _| {
                        hitbox.is_hovered(window)
                            && Self::link_for_position(&layout, &links, window.mouse_position())
                                .is_some_and(|link| link.url == url)
                    }),
                });
            }
        }
        hitbox
    }

    fn paint(
        &mut self,
        global_id: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let current_view = window.current_view();
        let hitbox = prepaint;
        let Ok(mut state) = self.state.lock() else {
            return;
        };

        let text_layout = self.styled_text.layout().clone();
        for (range, highlight) in &self.highlights {
            if let Some(color) = highlight.background_color
                && (self
                    .links
                    .iter()
                    .any(|(link, _)| link.start <= range.start && link.end >= range.end)
                    || self
                        .font_runs
                        .iter()
                        .any(|(code, _)| code.start <= range.start && code.end >= range.end))
            {
                Self::paint_selection(
                    &range.clone().into(),
                    &text_layout,
                    &bounds,
                    window,
                    color,
                    px(3.),
                );
            }
        }
        self.styled_text
            .paint(global_id, None, bounds, &mut (), &mut (), window, cx);

        // layout selections
        let (is_selectable, is_selection, selection) =
            self.layout_selections(&text_layout, &bounds, window, cx);

        state.selection = selection;

        if is_selection || is_selectable {
            window.set_cursor_style(CursorStyle::IBeam, &hitbox);
        }

        // link cursor pointer
        let mouse_position = window.mouse_position();
        if Self::link_for_position(&text_layout, &self.links, mouse_position).is_some() {
            window.set_cursor_style(CursorStyle::PointingHand, &hitbox);
        }

        // A link's context menu owns right clicks, not the surrounding message.
        if self.link_click_handler.is_some() {
            window.on_mouse_event({
                let hitbox = hitbox.clone();
                let layout = text_layout.clone();
                let links = self.links.clone();
                move |event: &MouseDownEvent, phase, window, cx| {
                    if phase.bubble()
                        && event.button == MouseButton::Right
                        && hitbox.is_hovered(window)
                        && Self::link_for_position(&layout, &links, event.position).is_some()
                    {
                        cx.stop_propagation();
                    }
                }
            });
        }

        if let Some(selection) = &state.selection {
            Self::paint_selection(
                &self
                    .padding
                    .display_range(selection.start..selection.end)
                    .into(),
                &text_layout,
                &bounds,
                window,
                cx.theme().selection,
                px(0.),
            );
        }

        if is_selectable {
            if let Some(text_view_state) = UiGlobalState::global(cx).text_view_state().cloned() {
                let text_bounds = self.text_line_bounds(
                    &text_layout,
                    text_layout.line_height(),
                    window.content_mask().bounds,
                );
                text_view_state.update(cx, |state, _| {
                    state.selection_adapter.register_inline(text_bounds);
                });
            }

            window.on_mouse_event({
                let hitbox = hitbox.clone();
                let text_layout = text_layout.clone();
                let inline_state = self.state.clone();
                let text = self.text.clone();
                let source_text = self.source_text.clone();
                let padding = self.padding.clone();
                let text_view_state = UiGlobalState::global(cx).text_view_state().cloned();
                move |event: &MouseDownEvent, phase, window, cx| {
                    if !phase.bubble()
                        || !hitbox.is_hovered(window)
                        || event.button != MouseButton::Left
                    {
                        return;
                    }

                    let kind = match event.click_count {
                        2 => TextViewMultiClickKind::Word,
                        3 => TextViewMultiClickKind::Paragraph,
                        _ => return,
                    };

                    let Some(range) = selection_for_multi_click(
                        &text,
                        &text_layout,
                        hitbox.bounds,
                        event.position,
                        kind,
                    ) else {
                        return;
                    };

                    let range = padding.source_range(range);
                    let selected_text = source_text[range.clone()].to_string();

                    // This renderer owns multi-click selection. Prevent the
                    // window selection layer from handling the same press.
                    gpui_base::GlobalState::suppress_text_selection(cx);

                    if let Ok(mut inline_state) = inline_state.lock() {
                        inline_state.selection = Some(range.into());
                    }
                    if let Some(text_view_state) = &text_view_state {
                        text_view_state.update(cx, |state, cx| {
                            state.set_multi_click_selection(
                                event.position,
                                kind,
                                selected_text,
                                cx,
                            );
                        });
                    }
                    cx.notify(current_view);
                }
            });
        }

        // mouse move, update hovered link
        window.on_mouse_event({
            let hitbox = hitbox.clone();
            let text_layout = text_layout.clone();
            let state = self.state.clone();
            move |event: &MouseMoveEvent, phase, window, cx| {
                if !phase.bubble() {
                    return;
                }

                let updated = if hitbox.is_hovered(window) {
                    text_layout.index_for_position(event.position).ok()
                } else {
                    None
                };
                if let Ok(mut state) = state.lock() {
                    if state.hovered_index != updated {
                        state.hovered_index = updated;
                        cx.notify(current_view);
                    }
                }
            }
        });

        {
            // click to open link
            window.on_mouse_event({
                let links = self.links.clone();
                let text_layout = text_layout.clone();
                let hitbox = hitbox.clone();
                let text_view_state = UiGlobalState::global(cx).text_view_state().cloned();
                let link_click_handler = self.link_click_handler.clone();

                move |event: &MouseUpEvent, phase, window, cx| {
                    if !phase.bubble() || !hitbox.is_hovered(window) {
                        return;
                    }
                    if event.button != MouseButton::Right
                        && text_view_state
                            .as_ref()
                            .is_some_and(|state| state.read(cx).has_selection(cx))
                    {
                        return;
                    }

                    if let Some(link) =
                        Self::link_for_position(&text_layout, &links, event.position)
                    {
                        gpui_base::TextSelection::end(window, cx);
                        cx.stop_propagation();
                        let click = ClickEvent::Mouse(MouseClickEvent {
                            down: MouseDownEvent {
                                button: event.button,
                                position: event.position,
                                modifiers: event.modifiers,
                                click_count: event.click_count,
                                first_mouse: false,
                            },
                            up: event.clone(),
                        });
                        handle_link_click(&link_click_handler, link.url, click, window, cx);
                    }
                }
            });
        }
    }
}

fn selection_for_multi_click(
    text: &str,
    text_layout: &TextLayout,
    bounds: Bounds<Pixels>,
    pos: Point<Pixels>,
    kind: TextViewMultiClickKind,
) -> Option<std::ops::Range<usize>> {
    if !bounds.contains(&pos) {
        return None;
    }

    let offset = text_layout.index_for_position(pos).ok()?;

    match kind {
        TextViewMultiClickKind::Word => word_range_at(text, offset),
        // Known limitation: a paragraph maps to a single Inline run here. When a
        // paragraph embeds an inline image it is split into multiple Inline runs,
        // so triple-click only selects the run on the clicked side of the image.
        TextViewMultiClickKind::Paragraph => (!text.is_empty()).then_some(0..text.len()),
    }
}

/// Check if a `pos` is within a `bounds`, considering multi-line selections.
fn point_in_text_selection(
    pos: Point<Pixels>,
    char_width: Pixels,
    selection_start: Point<Pixels>,
    selection_end: Point<Pixels>,
    line_height: Pixels,
) -> bool {
    let point_in_line = |point: Point<Pixels>| point.y >= pos.y && point.y < pos.y + line_height;
    let top = selection_start.y.min(selection_end.y);
    let bottom = selection_start.y.max(selection_end.y);
    let x = pos.x + char_width.half();

    // Out of the vertical bounds
    if pos.y + line_height <= top || pos.y > bottom {
        return false;
    }

    // Treat the selection as single-line when both drag points fall within the
    // same rendered line, even if their y coordinates differ inside that line.
    if point_in_line(selection_start) && point_in_line(selection_end) {
        let left = selection_start.x.min(selection_end.x);
        let right = selection_start.x.max(selection_end.x);
        return x >= left && x <= right;
    }

    let (top_point, bottom_point) = if selection_start.y < selection_end.y {
        (selection_start, selection_end)
    } else {
        (selection_end, selection_start)
    };
    let is_top_line = point_in_line(top_point);
    let is_bottom_line = point_in_line(bottom_point);

    if is_top_line {
        return x >= top_point.x;
    } else if is_bottom_line {
        return x <= bottom_point.x;
    } else {
        return true;
    }
}

#[cfg(test)]
mod tests {
    use super::point_in_text_selection;
    use gpui::{point, px};

    #[test]
    fn test_point_in_text_selection() {
        let line_height = px(20.);
        let char_width = px(10.);
        let start = point(px(50.), px(50.));
        let end = point(px(150.), px(150.));

        // First line but haft line height, true
        // | p --------|
        // | selection |
        // |-----------|
        assert!(point_in_text_selection(
            point(px(50.), px(40.)),
            char_width,
            start,
            end,
            line_height
        ));

        // First line in selection, true
        // | p --------|
        // | selection |
        // |-----------|
        assert!(point_in_text_selection(
            point(px(50.), px(50.)),
            char_width,
            start,
            end,
            line_height
        ));
        // First line, but left out of selection, false
        // p |-----------|
        //   | selection |
        //   |-----------|
        assert!(!point_in_text_selection(
            point(px(40.), px(50.)),
            char_width,
            start,
            end,
            line_height
        ));
        // First line but right out of selection, true
        // |-----------| p
        // | selection |
        // |-----------|
        assert!(point_in_text_selection(
            point(px(160.), px(50.)),
            char_width,
            start,
            end,
            line_height
        ));

        // Middle line in selection, true
        // |-----------|
        // |     p     |
        // |-----------|
        assert!(point_in_text_selection(
            point(px(100.), px(70.)),
            char_width,
            start,
            end,
            line_height
        ));
        // Middle line, but left out of selection, true
        //   |-----------|
        // p | selection |
        //   |-----------|
        assert!(point_in_text_selection(
            point(px(40.), px(70.)),
            char_width,
            start,
            end,
            line_height
        ));
        // Middle line, but right out of selection, true
        // |-----------|
        // | selection | p
        // |-----------|
        assert!(point_in_text_selection(
            point(px(160.), px(70.)),
            char_width,
            start,
            end,
            line_height
        ));

        // Last line in selection, true
        // |-----------|
        // | selection |
        // |------- p -|
        assert!(point_in_text_selection(
            point(px(100.), px(140.)),
            char_width,
            start,
            end,
            line_height
        ));
        // Last line, but left out of selection, true
        //
        //   |-----------|
        //   | selection |
        // p |-----------|
        assert!(point_in_text_selection(
            point(px(40.), px(140.)),
            char_width,
            start,
            end,
            line_height
        ));
        // Last line, but right out of selection, false
        // |-----------|
        // | selection |
        // |-----------| p
        assert!(!point_in_text_selection(
            point(px(160.), px(140.)),
            char_width,
            start,
            end,
            line_height
        ));

        // Out of vertical bounds (top), false
        //       p
        // |-----------|
        // | selection |
        // |-----------|
        assert!(!point_in_text_selection(
            point(px(100.), px(20.)),
            char_width,
            start,
            end,
            line_height
        ));
        // Out of vertical bounds (bottom), false
        // |-----------|
        // | selection |
        // |-----------|
        //       p
        assert!(!point_in_text_selection(
            point(px(100.), px(160.)),
            char_width,
            start,
            end,
            line_height
        ));
    }

    #[test]
    fn test_point_in_text_selection_reversed_drag_direction() {
        let line_height = px(20.);
        let char_width = px(10.);

        // Mouse down on lower line then drag upward to x=150.
        // Top line should follow current mouse x, bottom line should keep anchor x.
        let start = point(px(80.), px(150.));
        let end = point(px(150.), px(50.));

        // On top line, selection starts from top cursor x (150), so x=140 should be excluded.
        assert!(!point_in_text_selection(
            point(px(140.), px(50.)),
            char_width,
            start,
            end,
            line_height
        ));
        assert!(point_in_text_selection(
            point(px(150.), px(50.)),
            char_width,
            start,
            end,
            line_height
        ));

        // On bottom line, selection ends at anchor x (80), so x=90 should be excluded.
        assert!(point_in_text_selection(
            point(px(75.), px(140.)),
            char_width,
            start,
            end,
            line_height
        ));
        assert!(!point_in_text_selection(
            point(px(80.), px(140.)),
            char_width,
            start,
            end,
            line_height
        ));
    }

    #[test]
    fn test_point_in_text_selection_same_visual_line_with_different_y() {
        let line_height = px(20.);
        let char_width = px(10.);
        let start = point(px(100.), px(55.));
        let end = point(px(60.), px(58.));

        assert!(!point_in_text_selection(
            point(px(40.), px(50.)),
            char_width,
            start,
            end,
            line_height
        ));
        assert!(point_in_text_selection(
            point(px(70.), px(50.)),
            char_width,
            start,
            end,
            line_height
        ));
        assert!(!point_in_text_selection(
            point(px(110.), px(50.)),
            char_width,
            start,
            end,
            line_height
        ));
    }

    #[test]
    fn test_point_in_text_selection_same_visual_line_with_reversed_y() {
        let line_height = px(20.);
        let char_width = px(10.);
        let start = point(px(60.), px(58.));
        let end = point(px(100.), px(55.));

        assert!(!point_in_text_selection(
            point(px(40.), px(50.)),
            char_width,
            start,
            end,
            line_height
        ));
        assert!(point_in_text_selection(
            point(px(70.), px(50.)),
            char_width,
            start,
            end,
            line_height
        ));
        assert!(!point_in_text_selection(
            point(px(110.), px(50.)),
            char_width,
            start,
            end,
            line_height
        ));
    }
}

/// Display-only spacing. Selection offsets always map back to the original text.
#[derive(Clone, Default)]
pub(super) struct CodePadding {
    insertions: Vec<(usize, Range<usize>, bool)>,
}

impl CodePadding {
    pub(super) fn new(text: &str, fonts: &[(Range<usize>, SharedString)]) -> (SharedString, Self) {
        // Word joiners keep the small side bearings attached to the code.
        const PAD: &str = "\u{2060}\u{2005}\u{2060}";
        let mut boundaries: Vec<_> = fonts
            .iter()
            .filter(|(range, _)| !range.is_empty())
            .flat_map(|(range, _)| [(range.start, true), (range.end, false)])
            .collect();
        boundaries.sort_unstable();
        boundaries.dedup();
        let mut padded = String::with_capacity(text.len() + boundaries.len() * PAD.len());
        let mut mapping = Self::default();
        let mut previous = 0;
        for (boundary, opening) in boundaries {
            padded.push_str(&text[previous..boundary]);
            let start = padded.len();
            padded.push_str(PAD);
            mapping
                .insertions
                .push((boundary, start..padded.len(), opening));
            previous = boundary;
        }
        padded.push_str(&text[previous..]);
        (padded.into(), mapping)
    }

    fn display_offset(&self, offset: usize) -> usize {
        // Closing padding belongs to the preceding character; opening padding
        // belongs to the following one. Adjacent style ranges stay disjoint.
        offset
            + self
                .insertions
                .iter()
                .filter(|(source, _, opening)| *source < offset || (*source == offset && !opening))
                .map(|(_, range, _)| range.len())
                .sum::<usize>()
    }

    pub(super) fn display_range(&self, range: Range<usize>) -> Range<usize> {
        self.display_offset(range.start)..self.display_offset(range.end)
    }

    fn source_offset(&self, offset: usize) -> usize {
        offset
            - self
                .insertions
                .iter()
                .map(|(_, range, _)| offset.saturating_sub(range.start).min(range.len()))
                .sum::<usize>()
    }

    pub(super) fn source_range(&self, range: Range<usize>) -> Range<usize> {
        self.source_offset(range.start)..self.source_offset(range.end)
    }
}

/// Split shaped runs at font boundaries without changing UTF-8 byte offsets.
pub(super) fn apply_font_runs(
    runs: Vec<gpui::TextRun>,
    fonts: &[(Range<usize>, SharedString)],
) -> Vec<gpui::TextRun> {
    if fonts.is_empty() {
        return runs;
    }
    let mut result = Vec::new();
    let mut offset = 0;
    for run in runs {
        let end = offset + run.len;
        let mut boundaries = vec![offset, end];
        for (range, _) in fonts {
            if range.start > offset && range.start < end {
                boundaries.push(range.start);
            }
            if range.end > offset && range.end < end {
                boundaries.push(range.end);
            }
        }
        boundaries.sort_unstable();
        boundaries.dedup();
        for boundary in boundaries.windows(2) {
            let mut part = run.clone();
            part.len = boundary[1] - boundary[0];
            if let Some((_, family)) = fonts.iter().find(|(range, _)| range.contains(&boundary[0]))
            {
                part.font.family = family.clone();
                part.font.style = gpui::FontStyle::Normal;
            }
            result.push(part);
        }
        offset = end;
    }
    result
}

#[cfg(test)]
mod font_run_tests {
    use super::*;

    #[test]
    fn chip_padding_is_display_only_and_round_trips_unicode_selection() {
        let source = "é \u{2005}code 後";
        let code = 6..10;
        let (display, padding) = CodePadding::new(source, &[(code.clone(), "Mono".into())]);
        assert!(display.len() > source.len());
        assert_eq!(padding.source_range(0..display.len()), 0..source.len());
        let displayed_code = padding.display_range(code.clone());
        assert_eq!(padding.source_range(displayed_code.clone()), code);
        assert!(display[displayed_code].contains("code"));
        for offset in source
            .char_indices()
            .map(|(index, _)| index)
            .chain([source.len()])
        {
            assert_eq!(
                padding.source_offset(padding.display_offset(offset)),
                offset
            );
        }
        for (_, inserted, _) in &padding.insertions {
            assert!(padding.source_range(inserted.clone()).is_empty());
        }
        // An actual space in the code/source is never stripped as padding.
        assert_eq!(&source[padding.source_range(0..display.len())], source);
    }

    #[test]
    fn padding_keeps_adjacent_highlight_ranges_disjoint() {
        let (_, padding) = CodePadding::new("a code z", &[(2..6, "Mono".into())]);
        let before = padding.display_range(0..2);
        let code = padding.display_range(2..6);
        let after = padding.display_range(6..8);
        assert_eq!(before.end, code.start);
        assert_eq!(code.end, after.start);
        assert_eq!(before.len(), 2);
        assert_eq!(after.len(), 2);
        assert!(code.len() > 4);
    }

    #[test]
    fn code_font_splits_runs_and_preserves_utf8_offsets_and_surrounding_style() {
        let mut style = gpui::TextStyle::default();
        style.font_style = gpui::FontStyle::Italic;
        let text = "é code 後";
        let runs = apply_font_runs(
            vec![style.to_run(text.len())],
            &[(3..7, "Test Mono".into())],
        );
        assert_eq!(
            runs.iter().map(|run| run.len).collect::<Vec<_>>(),
            vec![3, 4, 4]
        );
        assert_eq!(runs[0].font.style, gpui::FontStyle::Italic);
        assert_eq!(runs[1].font.family.as_ref(), "Test Mono");
        assert_eq!(runs[1].font.style, gpui::FontStyle::Normal);
        assert_eq!(runs[2].font.style, gpui::FontStyle::Italic);
    }
}
