use std::ops::Range;

/// A serialized source range that should be displayed and edited as one token.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InlineTokenSpec {
    pub range: Range<usize>,
    pub label: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct InlineToken {
    pub range: Range<usize>,
    pub source: String,
}

pub(crate) fn tokenize(text: &str, specs: Vec<InlineTokenSpec>) -> (String, Vec<InlineToken>) {
    let mut display = String::new();
    let mut tokens = Vec::new();
    let mut offset = 0;
    for spec in specs {
        if spec.range.start < offset
            || spec.range.is_empty()
            || spec.label.is_empty()
            || spec.label.contains(['\n', '\r'])
            || text.get(spec.range.clone()).is_none()
        {
            continue;
        }
        display.push_str(&text[offset..spec.range.start]);
        let start = display.len();
        display.push_str(&spec.label);
        tokens.push(InlineToken {
            range: start..display.len(),
            source: text[spec.range.clone()].into(),
        });
        offset = spec.range.end;
    }
    display.push_str(&text[offset..]);
    (display, tokens)
}

pub(crate) fn serialize(text: &str, tokens: &[InlineToken], range: Range<usize>) -> String {
    let mut result = String::new();
    let mut offset = range.start;
    for token in tokens
        .iter()
        .filter(|token| token.range.start >= range.start && token.range.end <= range.end)
    {
        result.push_str(&text[offset..token.range.start]);
        result.push_str(&token.source);
        offset = token.range.end;
    }
    result.push_str(&text[offset..range.end]);
    result
}

pub(crate) fn snap(tokens: &[InlineToken], offset: usize, forward: Option<bool>) -> usize {
    for token in tokens {
        if token.range.start < offset && offset < token.range.end {
            return if forward.unwrap_or(offset - token.range.start >= token.range.end - offset) {
                token.range.end
            } else {
                token.range.start
            };
        }
    }
    offset
}

pub(crate) fn expand(tokens: &[InlineToken], range: Range<usize>) -> Range<usize> {
    if range.is_empty() {
        let offset = snap(tokens, range.start, None);
        offset..offset
    } else {
        snap(tokens, range.start, Some(false))..snap(tokens, range.end, Some(true))
    }
}

pub(crate) fn edited(
    tokens: &[InlineToken],
    range: &Range<usize>,
    inserted_len: usize,
    inserted: Vec<InlineToken>,
) -> Vec<InlineToken> {
    let mut result = tokens
        .iter()
        .filter_map(|token| {
            let mut token = token.clone();
            if token.range.end <= range.start {
                Some(token)
            } else if token.range.start >= range.end {
                token.range = (token.range.start - range.len() + inserted_len)
                    ..(token.range.end - range.len() + inserted_len);
                Some(token)
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    result.extend(inserted.into_iter().map(|mut token| {
        token.range = (token.range.start + range.start)..(token.range.end + range.start);
        token
    }));
    result.sort_by_key(|token| token.range.start);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    fn sample() -> (String, Vec<InlineToken>) {
        tokenize(
            "hi [person](target)!",
            vec![InlineTokenSpec {
                range: 3..19,
                label: "@Zoë".into(),
            }],
        )
    }
    #[test]
    fn serialization_preserves_source_and_unicode_display() {
        let (text, tokens) = sample();
        assert_eq!(text, "hi @Zoë!");
        assert_eq!(
            serialize(&text, &tokens, 0..text.len()),
            "hi [person](target)!"
        );
        assert_eq!(
            serialize(&text, &tokens, tokens[0].range.clone()),
            "[person](target)"
        );
    }
    #[test]
    fn selection_and_cursor_cannot_split_tokens() {
        let (_, tokens) = sample();
        assert_eq!(expand(&tokens, 4..5), 3..8);
        assert_eq!(snap(&tokens, 7, Some(false)), 3);
        assert_eq!(snap(&tokens, 4, Some(true)), 8);
        assert_eq!(expand(&tokens, 5..5), 3..3);
    }
    #[test]
    fn edits_shift_tokens_and_remove_replaced_tokens() {
        let (_, tokens) = sample();
        assert_eq!(edited(&tokens, &(0..0), 2, vec![])[0].range, 5..10);
        assert!(edited(&tokens, &(3..8), 0, vec![]).is_empty());
        assert_eq!(edited(&tokens, &(8..8), 1, vec![]), tokens);
    }
}
