//! LSP snippet text (`InsertTextFormat::SNIPPET`): tab stops `$1`, `${1}`,
//! placeholders `${1:text}`, choices `${1|a,b|}` and the final stop `$0`.
//!
//! The editor has no tab stop session, so a snippet is inserted as plain text
//! and only places the caret: on the first numbered stop (its placeholder
//! selected), else on `$0`, else at the end.

use std::ops::Range;

/// A snippet with its markers removed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ExpandedSnippet {
    /// The text to insert.
    pub text: String,
    /// Where the caret goes, as a byte range of `text` (the placeholder of
    /// the first stop, empty for a bare stop).
    pub caret: Range<usize>,
}

pub(crate) fn expand_snippet(snippet: &str) -> ExpandedSnippet {
    let mut text = String::with_capacity(snippet.len());
    let mut stops: Vec<(usize, Range<usize>)> = Vec::new();
    expand_into(snippet, &mut text, &mut stops);
    let caret = stops
        .iter()
        .filter(|(number, _)| *number > 0)
        .min_by_key(|(number, _)| *number)
        .or_else(|| stops.iter().find(|(number, _)| *number == 0))
        .map(|(_, range)| range.clone())
        .unwrap_or(text.len()..text.len());
    ExpandedSnippet { text, caret }
}

fn expand_into(snippet: &str, text: &mut String, stops: &mut Vec<(usize, Range<usize>)>) {
    let mut chars = snippet.char_indices().peekable();
    while let Some((ix, c)) = chars.next() {
        match c {
            '\\' => match chars.peek() {
                Some(&(_, escaped @ ('$' | '}' | '\\'))) => {
                    text.push(escaped);
                    chars.next();
                }
                _ => text.push('\\'),
            },
            '$' => {
                let rest = &snippet[ix + 1..];
                if let Some((number, len)) = leading_number(rest) {
                    // `$1`
                    stops.push((number, text.len()..text.len()));
                    skip(&mut chars, len);
                } else if let Some(body) = rest.strip_prefix('{')
                    && let Some((number, len)) = leading_number(body)
                    && let Some(end) = closing_brace(&body[len..])
                {
                    // `${1}`, `${1:placeholder}` or `${1|one,two|}`
                    let inner = &body[len..len + end];
                    let start = text.len();
                    if let Some(placeholder) = inner.strip_prefix(':') {
                        expand_into(placeholder, text, stops);
                    } else if let Some(choices) = inner
                        .strip_prefix('|')
                        .and_then(|choices| choices.strip_suffix('|'))
                    {
                        text.push_str(choices.split(',').next().unwrap_or_default());
                    }
                    stops.push((number, start..text.len()));
                    // `{`, the number, the inner part and `}`.
                    skip(&mut chars, 1 + len + end + 1);
                } else {
                    text.push('$');
                }
            }
            _ => text.push(c),
        }
    }
}

/// The tab stop number at the start of `text` and its length in bytes.
fn leading_number(text: &str) -> Option<(usize, usize)> {
    let len = text.bytes().take_while(u8::is_ascii_digit).count();
    Some((text[..len].parse().ok()?, len))
}

/// The byte offset of the `}` that closes a `${` whose body is `text`,
/// skipping nested `${...}` and escaped characters.
fn closing_brace(text: &str) -> Option<usize> {
    let mut depth = 0usize;
    let mut chars = text.char_indices().peekable();
    while let Some((ix, c)) = chars.next() {
        match c {
            '\\' => {
                chars.next();
            }
            '$' if matches!(chars.peek(), Some((_, '{'))) => {
                depth += 1;
                chars.next();
            }
            '}' if depth == 0 => return Some(ix),
            '}' => depth -= 1,
            _ => {}
        }
    }
    None
}

fn skip(chars: &mut std::iter::Peekable<std::str::CharIndices<'_>>, bytes: usize) {
    let Some(&(start, _)) = chars.peek() else {
        return;
    };
    while chars.peek().is_some_and(|(ix, _)| *ix < start + bytes) {
        chars.next();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn expand(snippet: &str) -> (String, Range<usize>) {
        let expanded = expand_snippet(snippet);
        (expanded.text, expanded.caret)
    }

    #[test]
    fn test_final_stop_places_the_caret() {
        assert_eq!(expand("count($0)"), ("count()".to_string(), 6..6));
    }

    #[test]
    fn test_first_numbered_stop_wins_over_the_final_one() {
        assert_eq!(
            expand("SELECT $2 FROM $1 WHERE $0"),
            ("SELECT  FROM  WHERE ".to_string(), 13..13)
        );
    }

    #[test]
    fn test_placeholder_is_selected() {
        assert_eq!(
            expand("SELECT * FROM ${1:table} LIMIT ${2:10}"),
            ("SELECT * FROM table LIMIT 10".to_string(), 14..19)
        );
        assert_eq!(
            expand("x = ${1|true,false|}"),
            ("x = true".to_string(), 4..8)
        );
    }

    #[test]
    fn test_nested_placeholder_and_escapes() {
        assert_eq!(
            expand(r"${1:a ${2:b}} costs \$5 \} \\"),
            (r"a b costs $5 } \".to_string(), 0..3)
        );
    }

    #[gpui::test]
    fn test_insert_completion_places_the_caret_on_the_first_stop(cx: &mut gpui::TestAppContext) {
        use crate::input::EditorState;
        use gpui::{AppContext as _, px, size};

        cx.update(crate::init);
        let mut editor = None;
        let window = cx.open_window(size(px(400.), px(100.)), |window, cx| {
            editor = Some(cx.new(|cx| EditorState::new(window, cx).default_value("SELECT co")));
            gpui::EmptyView
        });
        let editor = editor.unwrap();
        window
            .update(cx, |_, window, cx| {
                editor.update(cx, |state, cx| {
                    state.set_cursor_to(9);
                    let item = lsp_types::CompletionItem {
                        label: "count".to_string(),
                        text_edit: Some(lsp_types::CompletionTextEdit::Edit(
                            lsp_types::TextEdit::new(
                                lsp_types::Range::new(
                                    lsp_types::Position::new(0, 7),
                                    lsp_types::Position::new(0, 9),
                                ),
                                "count(${1:expr})$0".to_string(),
                            ),
                        )),
                        insert_text_format: Some(lsp_types::InsertTextFormat::SNIPPET),
                        ..Default::default()
                    };
                    state.insert_completion(&item, 7..9, window, cx);
                    assert_eq!(state.value(), "SELECT count(expr)");
                    assert_eq!(
                        state.selected_range(),
                        13..17,
                        "the placeholder is selected"
                    );

                    // Plain text is inserted as it is.
                    let item = lsp_types::CompletionItem {
                        label: "$1".to_string(),
                        insert_text: Some(" $1".to_string()),
                        ..Default::default()
                    };
                    state.set_cursor_to(18);
                    state.insert_completion(&item, 18..18, window, cx);
                    assert_eq!(state.value(), "SELECT count(expr) $1");
                });
            })
            .unwrap();
    }

    #[test]
    fn test_text_without_stops_puts_the_caret_at_the_end() {
        assert_eq!(
            expand("$TM_SELECTED_TEXT and $"),
            ("$TM_SELECTED_TEXT and $".to_string(), 23..23)
        );
        assert_eq!(expand("é$1"), ("é".to_string(), 2..2));
    }
}
