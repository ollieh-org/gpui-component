use std::rc::Rc;

use gpui::{
    App, DefiniteLength, Entity, IntoElement, RenderOnce, SharedString, StyleRefinement, Styled,
    Window, prelude::FluentBuilder as _,
};

use super::{Input, TextareaState};
use crate::native_menu::NativeMenu;
use crate::{RoleOverride, StyledExt as _};

/// A styled ordinary multi-line text field.
#[derive(IntoElement)]
pub struct Textarea {
    state: Entity<TextareaState>,
    style: StyleRefinement,
    height: Option<DefiniteLength>,
    appearance: bool,
    bordered: bool,
    disabled: bool,
    readonly: bool,
    tab_index: isize,
    role: RoleOverride,
    aria_label: Option<SharedString>,

    /// An optional context menu builder to allow a custom context menu.
    ///
    /// If set, this overrides the built-in context menu.
    context_menu_builder: Option<Rc<dyn Fn(NativeMenu, &mut Window, &mut App) -> NativeMenu>>,
}

impl Textarea {
    pub fn new(state: &Entity<TextareaState>) -> Self {
        Self {
            state: state.clone(),
            style: StyleRefinement::default(),
            height: None,
            appearance: true,
            bordered: true,
            disabled: false,
            readonly: false,
            tab_index: 0,
            role: RoleOverride::default(),
            aria_label: None,
            context_menu_builder: None,
        }
    }

    pub fn h(mut self, height: impl Into<DefiniteLength>) -> Self {
        self.height = Some(height.into());
        self
    }

    pub fn appearance(mut self, appearance: bool) -> Self {
        self.appearance = appearance;
        self
    }

    pub fn bordered(mut self, bordered: bool) -> Self {
        self.bordered = bordered;
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Set the textarea to read-only, default is `false`.
    ///
    /// Unlike [`Self::disabled`], a read-only textarea keeps the normal appearance
    /// and still can be focused, selected and copied, it only rejects the changes
    /// made by the user.
    pub fn readonly(mut self, readonly: bool) -> Self {
        self.readonly = readonly;
        self
    }

    pub fn tab_index(mut self, index: isize) -> Self {
        self.tab_index = index;
        self
    }

    pub fn role(mut self, role: impl Into<RoleOverride>) -> Self {
        self.role = role.into();
        self
    }

    pub fn aria_label(mut self, label: impl Into<SharedString>) -> Self {
        self.aria_label = Some(label.into());
        self
    }

    /// Replace the built-in context menu shown on right-click.
    ///
    /// The closure receives an empty menu and returns the one to show, so it
    /// decides entirely what appears — the default items are not added.
    pub fn context_menu(
        mut self,
        f: impl Fn(NativeMenu, &mut Window, &mut App) -> NativeMenu + 'static,
    ) -> Self {
        self.context_menu_builder = Some(Rc::new(f));
        self
    }
}

impl Styled for Textarea {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl RenderOnce for Textarea {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        Input::from_state(self.state.clone())
            .appearance(self.appearance)
            .bordered(self.bordered)
            .disabled(self.disabled)
            .readonly(self.readonly)
            .tab_index(self.tab_index)
            .role(self.role)
            .when_some(self.height, |this, height| this.h(height))
            .when_some(self.aria_label, |this, label| this.aria_label(label))
            .when_some(self.context_menu_builder, |this, build| {
                this.context_menu(move |menu, window, cx| build(menu, window, cx))
            })
            .refine_style(&self.style)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ElementExt as _, input::TextareaState};
    use gpui::{Context, Render, TestAppContext, div, prelude::*, px};
    use std::sync::{Arc, Mutex};

    struct Composer {
        state: Entity<TextareaState>,
        height: Arc<Mutex<f32>>,
    }
    impl Render for Composer {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let measured = self.height.clone();
            div()
                .size_full()
                .flex()
                .flex_col()
                .child(div().flex_1().min_h(px(0.)))
                .child(
                    div().flex_none().flex().flex_col().child(
                        div()
                            .w_full()
                            .flex_none()
                            .min_h(px(58.))
                            .px_2()
                            .py_2()
                            .flex()
                            .items_start()
                            .child(div().size(px(32.)).flex_none())
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.))
                                    .flex()
                                    .flex_col()
                                    .px(px(2.))
                                    .py_1()
                                    .child(
                                        Textarea::new(&self.state)
                                            .appearance(false)
                                            .bordered(false)
                                            .w_full(),
                                    ),
                            )
                            .on_prepaint(move |bounds, _, _| {
                                *measured.lock().unwrap() = bounds.size.height.as_f32()
                            }),
                    ),
                )
        }
    }
    #[gpui::test]
    fn auto_grow_composer_grows_and_shrinks_with_content(cx: &mut TestAppContext) {
        cx.update(crate::init);
        let height = Arc::new(Mutex::new(0.));
        let observed = height.clone();
        let (root, cx) = cx.add_window_view(|window, cx| Composer {
            state: cx.new(|cx| TextareaState::new(window, cx).auto_grow(1, 8)),
            height,
        });
        cx.run_until_parked();
        let before = *observed.lock().unwrap();
        cx.update(|window, cx| {
            root.update(cx, |root, cx| {
                root.state.update(cx, |state, cx| {
                    state.set_value("one\ntwo\nthree", window, cx)
                })
            })
        });
        cx.run_until_parked();
        let after = *observed.lock().unwrap();
        assert!(after > before, "composer did not grow: {before} -> {after}");
        for content in [
            "wrapped words ".repeat(500),
            "line\n".repeat(12),
            "line\n".repeat(24),
        ] {
            cx.update(|window, cx| {
                root.update(cx, |root, cx| {
                    root.state
                        .update(cx, |state, cx| state.set_value(content, window, cx))
                })
            });
            cx.run_until_parked();
            assert!(*observed.lock().unwrap() > after);
        }
        let capped = *observed.lock().unwrap();
        cx.update(|window, cx| {
            root.update(cx, |root, cx| {
                root.state.update(cx, |state, cx| {
                    state.set_value("line\n".repeat(100), window, cx)
                })
            })
        });
        cx.run_until_parked();
        assert_eq!(*observed.lock().unwrap(), capped);
        cx.update(|window, cx| {
            root.update(cx, |root, cx| {
                root.state
                    .update(cx, |state, cx| state.set_value("one", window, cx))
            })
        });
        cx.run_until_parked();
        assert_eq!(*observed.lock().unwrap(), before);
    }
}
