//! The LSP snippet grammar and the editing session that walks its
//! tabstops.
//!
//! <https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#snippet_syntax>

use gpui::Context;
use std::ops::Range;

use crate::input::{EditorMode, InputBaseState};

/// A snippet source parsed into plain text plus tabstop ranges.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParsedSnippet {
    /// The text with all snippet markers removed.
    pub(crate) text: String,
    /// One byte range in `text` per tabstop, in visit order: `$1..$n`,
    /// then `$0` (appended at the end of the text when the source has no
    /// explicit `$0`). Only the first occurrence of a repeated index is
    /// kept.
    pub(crate) stops: Vec<Range<usize>>,
}

/// Parse the LSP snippet grammar subset: `$n`, `${n}`, `${n:placeholder}`
/// (placeholders may nest), `${n|choice,…|}` (first choice wins), `\`
/// escapes, and variables (`$name`, `${name}`, `${name:default}`) which
/// resolve to their default or nothing.
pub(crate) fn parse_snippet(source: &str) -> ParsedSnippet {
    let mut parser = Parser {
        source: source.as_bytes(),
        pos: 0,
        text: String::new(),
        stops: vec![],
    };
    parser.parse_body(None);

    let mut stops: Vec<(u32, Range<usize>)> = vec![];
    for (index, range) in parser.stops {
        if !stops.iter().any(|(existing, _)| *existing == index) {
            stops.push((index, range));
        }
    }
    // Visit order is 1..n then 0; a missing $0 means "end of the text".
    stops.sort_by_key(|(index, _)| if *index == 0 { u32::MAX } else { *index });
    if !stops.iter().any(|(index, _)| *index == 0) {
        stops.push((0, parser.text.len()..parser.text.len()));
    }

    ParsedSnippet {
        text: parser.text,
        stops: stops.into_iter().map(|(_, range)| range).collect(),
    }
}

struct Parser<'a> {
    source: &'a [u8],
    pos: usize,
    text: String,
    stops: Vec<(u32, Range<usize>)>,
}

impl Parser<'_> {
    /// Parse until end of input, or until `stop_at` (used for the `}` of a
    /// placeholder). Returns whether `stop_at` was consumed.
    fn parse_body(&mut self, stop_at: Option<u8>) -> bool {
        while self.pos < self.source.len() {
            let byte = self.source[self.pos];
            match byte {
                b'\\' => {
                    self.pos += 1;
                    if let Some(&escaped) = self.source.get(self.pos) {
                        if !matches!(escaped, b'$' | b'\\' | b'}') {
                            self.text.push('\\');
                        }
                        self.push_source_char();
                    } else {
                        self.text.push('\\');
                    }
                }
                b'$' => {
                    self.pos += 1;
                    self.parse_dollar();
                }
                _ if Some(byte) == stop_at => {
                    self.pos += 1;
                    return true;
                }
                _ => self.push_source_char(),
            }
        }
        false
    }

    fn parse_dollar(&mut self) {
        match self.source.get(self.pos) {
            Some(b'{') => {
                self.pos += 1;
                self.parse_braced();
            }
            Some(byte) if byte.is_ascii_digit() => {
                let index = self.take_number();
                let at = self.text.len();
                self.stops.push((index, at..at));
            }
            Some(byte) if byte.is_ascii_alphabetic() || *byte == b'_' => {
                // A bare variable; no values are provided, resolve to
                // nothing.
                self.take_identifier();
            }
            _ => self.text.push('$'),
        }
    }

    fn parse_braced(&mut self) {
        match self.source.get(self.pos) {
            Some(byte) if byte.is_ascii_digit() => {
                let index = self.take_number();
                let start = self.text.len();
                match self.source.get(self.pos) {
                    Some(b':') => {
                        // `${n:placeholder}` — the placeholder may nest.
                        self.pos += 1;
                        self.parse_body(Some(b'}'));
                    }
                    Some(b'|') => {
                        // `${n|first,…|}` — the first choice becomes the
                        // placeholder text.
                        self.pos += 1;
                        self.parse_first_choice();
                    }
                    Some(b'}') => {
                        self.pos += 1;
                    }
                    _ => {}
                }
                self.stops.push((index, start..self.text.len()));
            }
            Some(byte) if byte.is_ascii_alphabetic() || *byte == b'_' => {
                self.take_identifier();
                match self.source.get(self.pos) {
                    Some(b':') => {
                        // `${name:default}` — keep the default.
                        self.pos += 1;
                        self.parse_body(Some(b'}'));
                    }
                    Some(b'}') => {
                        self.pos += 1;
                    }
                    _ => {}
                }
            }
            _ => {
                self.text.push_str("${");
            }
        }
    }

    fn parse_first_choice(&mut self) {
        let mut keep = true;
        while self.pos < self.source.len() {
            match self.source[self.pos] {
                b'\\' => {
                    self.pos += 1;
                    if self.pos < self.source.len() {
                        if keep {
                            self.push_source_char();
                        } else {
                            self.advance_char();
                        }
                    }
                }
                b',' => {
                    keep = false;
                    self.pos += 1;
                }
                b'|' => {
                    self.pos += 1;
                    if self.source.get(self.pos) == Some(&b'}') {
                        self.pos += 1;
                    }
                    return;
                }
                _ if keep => self.push_source_char(),
                _ => self.advance_char(),
            }
        }
    }

    fn take_number(&mut self) -> u32 {
        let start = self.pos;
        while self
            .source
            .get(self.pos)
            .is_some_and(|byte| byte.is_ascii_digit())
        {
            self.pos += 1;
        }
        std::str::from_utf8(&self.source[start..self.pos])
            .ok()
            .and_then(|digits| digits.parse().ok())
            .unwrap_or(0)
    }

    fn take_identifier(&mut self) {
        while self
            .source
            .get(self.pos)
            .is_some_and(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
        {
            self.pos += 1;
        }
    }

    /// Copy the (possibly multi-byte) character at `pos` to the output.
    fn push_source_char(&mut self) {
        let start = self.pos;
        self.advance_char();
        self.text
            .push_str(std::str::from_utf8(&self.source[start..self.pos]).unwrap_or_default());
    }

    fn advance_char(&mut self) {
        self.pos += 1;
        while self
            .source
            .get(self.pos)
            .is_some_and(|byte| (*byte & 0b1100_0000) == 0b1000_0000)
        {
            self.pos += 1;
        }
    }
}

/// A live snippet editing session: the inserted snippet's tabstops as
/// absolute document ranges, walked with Tab / Shift-Tab.
#[derive(Debug, Clone)]
pub(crate) struct SnippetSession {
    /// Absolute byte ranges of the stops, in visit order (`$0` last).
    stops: Vec<Range<usize>>,
    active: usize,
}

impl SnippetSession {
    pub(crate) fn new(stops: Vec<Range<usize>>) -> Self {
        Self { stops, active: 0 }
    }

    fn active_range(&self) -> &Range<usize> {
        &self.stops[self.active]
    }

    /// Adjust the stop ranges for an edit of `old_range` replaced by
    /// `new_len` bytes. Returns `false` when the session can no longer
    /// track its stops (the edit happened outside the active stop) and
    /// must end.
    fn adjust_for_edit(&mut self, old_range: &Range<usize>, new_len: usize) -> bool {
        let active = self.active_range();
        if old_range.start < active.start || old_range.start > active.end {
            return false;
        }

        let delta = new_len as isize - old_range.len() as isize;
        for stop in &mut self.stops {
            if stop.end <= old_range.start && *stop != *old_range {
                // Entirely before the edit.
            } else if old_range.start >= stop.start && old_range.end <= stop.end {
                // Inside this stop: it grows or shrinks.
                stop.end = (stop.end as isize + delta) as usize;
            } else if stop.start >= old_range.end {
                // Entirely after the edit: shift.
                stop.start = (stop.start as isize + delta) as usize;
                stop.end = (stop.end as isize + delta) as usize;
            } else {
                // Partial overlap: the stop no longer means anything.
                return false;
            }
        }
        true
    }
}

impl InputBaseState<EditorMode> {
    /// Begin a snippet session over freshly inserted text. `stops` are
    /// absolute document ranges; the first is selected immediately. A
    /// single stop (just `$0`) only moves the cursor.
    pub(crate) fn start_snippet_session(
        &mut self,
        stops: Vec<Range<usize>>,
        cx: &mut Context<Self>,
    ) {
        let Some(first) = stops.first().cloned() else {
            return;
        };
        self.selected_range = first.into();
        self.extras.snippet = (stops.len() > 1).then(|| SnippetSession::new(stops));
        cx.notify();
    }

    /// Move to the next (or previous) tabstop. Returns `false` when no
    /// session is active, letting Tab fall through to indentation.
    pub(crate) fn snippet_tab(&mut self, forward: bool, cx: &mut Context<Self>) -> bool {
        let Some(session) = &mut self.extras.snippet else {
            return false;
        };

        if forward {
            session.active += 1;
        } else if session.active > 0 {
            session.active -= 1;
        } else {
            // Shift-Tab at the first stop: stay, but keep the key.
            return true;
        }

        let range = session.stops[session.active].clone();
        let ended = forward && session.active + 1 == session.stops.len();
        self.selected_range = range.into();
        if ended {
            self.extras.snippet = None;
        }
        cx.notify();
        true
    }

    /// End the active snippet session. Returns whether one was active.
    pub(crate) fn end_snippet_session(&mut self) -> bool {
        self.extras.snippet.take().is_some()
    }

    /// Keep the session's ranges anchored through an edit, ending it when
    /// the edit leaves the active stop.
    pub(crate) fn adjust_snippet_session(&mut self, old_range: &Range<usize>, new_len: usize) {
        if let Some(session) = &mut self.extras.snippet
            && !session.adjust_for_edit(old_range, new_len)
        {
            self.extras.snippet = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_support::build_editor;
    use super::*;
    use crate::input::{IndentInline, Undo};
    use gpui::TestAppContext;
    use lsp_types::{CompletionItem, CompletionTextEdit, InsertTextFormat, Position, TextEdit};

    fn parsed(source: &str) -> (String, Vec<Range<usize>>) {
        let snippet = parse_snippet(source);
        (snippet.text, snippet.stops)
    }

    #[test]
    fn test_parse_snippet() {
        // Plain text passes through, $0 defaults to the end.
        assert_eq!(parsed("hello"), ("hello".into(), vec![5..5]));

        // Numbered stops in visit order, $0 last.
        let (text, stops) = parsed("if $2 { $1 }$0");
        assert_eq!(text, "if  {  }");
        assert_eq!(stops, vec![6..6, 3..3, 8..8]);

        // Placeholders keep their text; nesting flattens.
        let (text, stops) = parsed("f(${1:outer ${2:inner}})");
        assert_eq!(text, "f(outer inner)");
        assert_eq!(stops, vec![2..13, 8..13, 14..14]);

        // Choices collapse to the first option.
        let (text, stops) = parsed("${1|red,green,blue|}");
        assert_eq!(text, "red");
        assert_eq!(stops, vec![0..3, 3..3]);

        // Escapes: \$ is a literal dollar, \\ a backslash; unknown escapes
        // keep the backslash.
        assert_eq!(parsed(r"\$1 \\ \n").0, r"$1 \ \n");

        // Variables resolve to their default or nothing.
        assert_eq!(parsed("$TM_FILENAME-${x:fallback}").0, "-fallback");

        // A repeated index keeps its first occurrence.
        let (_, stops) = parsed("$1 and $1");
        assert_eq!(stops.len(), 2);
        assert_eq!(stops[0], 0..0);
    }

    #[gpui::test]
    fn snippet_completions_walk_their_tabstops(cx: &mut TestAppContext) {
        let (editor, mut cx) = build_editor(cx);

        cx.update(|window, cx| {
            editor.update(cx, |editor, cx| {
                editor.set_value("fmt.Printf", window, cx);
                editor.selected_range = (10..10).into();
            });
        });

        let item = CompletionItem {
            label: "Printf".into(),
            insert_text_format: Some(InsertTextFormat::SNIPPET),
            text_edit: Some(CompletionTextEdit::Edit(TextEdit::new(
                lsp_types::Range::new(Position::new(0, 4), Position::new(0, 10)),
                "Printf(${1:format}, ${2:args})".into(),
            ))),
            ..Default::default()
        };

        cx.update(|window, cx| {
            editor.update(cx, |editor, cx| {
                editor.insert_completion(&item, 4..10, window, cx);
                // Markers are stripped and the first placeholder is
                // selected.
                assert_eq!(editor.text().to_string(), "fmt.Printf(format, args)");
                assert_eq!(
                    (editor.selected_range.start, editor.selected_range.end),
                    (11, 17)
                );
            });
        });

        // Typing replaces the selected placeholder and the later stops
        // shift with the edit.
        cx.update(|window, cx| {
            editor.update(cx, |editor, cx| {
                editor.replace("\"%d\"", window, cx);
                assert_eq!(editor.text().to_string(), "fmt.Printf(\"%d\", args)");
            });
        });

        // Tab jumps to the second placeholder, shifted with the edit.
        cx.update(|window, cx| {
            editor.update(cx, |editor, cx| {
                editor.indent_inline(&IndentInline, window, cx);
                assert_eq!(
                    (editor.selected_range.start, editor.selected_range.end),
                    (17, 21)
                );
            });
        });

        // Tab again lands on the implicit $0 at the end and the session
        // ends.
        cx.update(|window, cx| {
            editor.update(cx, |editor, cx| {
                editor.indent_inline(&IndentInline, window, cx);
                assert_eq!(
                    (editor.selected_range.start, editor.selected_range.end),
                    (22, 22)
                );
                assert!(editor.extras.snippet.is_none());
            });
        });

        cx.update(|window, cx| {
            editor.update(cx, |editor, cx| {
                editor.undo(&Undo, window, cx);
            });
        });
    }

    #[gpui::test]
    fn escape_ends_the_snippet_session(cx: &mut TestAppContext) {
        let (editor, mut cx) = build_editor(cx);

        cx.update(|window, cx| {
            editor.update(cx, |editor, cx| {
                editor.set_value("x", window, cx);
                editor.selected_range = (1..1).into();
            });
        });

        let item = CompletionItem {
            label: "pair".into(),
            insert_text_format: Some(InsertTextFormat::SNIPPET),
            insert_text: Some("(${1:a}, ${2:b})".into()),
            ..Default::default()
        };
        cx.update(|window, cx| {
            editor.update(cx, |editor, cx| {
                editor.insert_completion(&item, 1..1, window, cx);
                assert!(editor.extras.snippet.is_some());
                editor.escape(&crate::input::Escape, window, cx);
                assert!(editor.extras.snippet.is_none());
            });
        });
    }
}
