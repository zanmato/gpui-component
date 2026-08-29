use anyhow::Result;
use gpui::{App, Context, Task, Window};
use lsp_types::{FormattingOptions, TextEdit};
use ropey::Rope;
use std::ops::Range;

use crate::input::{EditorMode, Format, InputBaseState};

/// Formatting provider: reformat the whole document or a range of it.
///
/// https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#textDocument_formatting
pub trait FormattingProvider {
    /// Compute the edits reformatting the whole document.
    ///
    /// textDocument/formatting
    ///
    /// https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#textDocument_formatting
    fn format(
        &self,
        text: &Rope,
        options: FormattingOptions,
        window: &mut Window,
        cx: &mut App,
    ) -> Task<Result<Option<Vec<TextEdit>>>>;

    /// Compute the edits reformatting the given byte range.
    ///
    /// textDocument/rangeFormatting
    ///
    /// https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#textDocument_rangeFormatting
    fn range_format(
        &self,
        text: &Rope,
        range: Range<usize>,
        options: FormattingOptions,
        window: &mut Window,
        cx: &mut App,
    ) -> Task<Result<Option<Vec<TextEdit>>>>;

    /// Whether the server formats whole documents.
    fn supports_format(&self) -> bool {
        true
    }

    /// Whether the server formats ranges. When it does not, the whole
    /// document is formatted even with a selection present.
    fn supports_range_format(&self) -> bool {
        true
    }
}

impl InputBaseState<EditorMode> {
    pub(crate) fn on_action_format(
        &mut self,
        _: &Format,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(provider) = self.extras.lsp.formatting_provider.clone() else {
            return;
        };

        let tab = self.mode.tab_size();
        let options = FormattingOptions {
            tab_size: tab.tab_size as u32,
            insert_spaces: !tab.hard_tabs,
            ..Default::default()
        };

        let selection = self.selected_range;
        let selected = selection.start.min(selection.end)..selection.start.max(selection.end);
        let task = if !selected.is_empty() && provider.supports_range_format() {
            provider.range_format(&self.text, selected, options, window, cx)
        } else if provider.supports_format() {
            provider.format(&self.text, options, window, cx)
        } else {
            return;
        };

        let version = self.document_version;
        self.extras.lsp._format_task = cx.spawn_in(window, async move |editor, cx| {
            let Ok(Some(edits)) = task.await else {
                return;
            };
            editor
                .update_in(cx, |editor, window, cx| {
                    if editor.document_version != version {
                        return;
                    }
                    editor.apply_text_edits(&edits, window, cx);
                })
                .ok();
        });
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_support::build_editor;
    use super::*;
    use gpui::TestAppContext;
    use lsp_types::Position;
    use std::cell::RefCell;
    use std::rc::Rc;

    #[derive(Default)]
    struct RecordingFormatter {
        options_seen: RefCell<Option<FormattingOptions>>,
        range_seen: RefCell<Option<Range<usize>>>,
        range_supported: bool,
    }

    impl FormattingProvider for RecordingFormatter {
        fn format(
            &self,
            _: &Rope,
            options: FormattingOptions,
            _: &mut Window,
            _: &mut App,
        ) -> Task<Result<Option<Vec<TextEdit>>>> {
            *self.options_seen.borrow_mut() = Some(options);
            *self.range_seen.borrow_mut() = None;
            // gofmt-style: fix the indentation of both inner lines.
            let edit = |line: u32, end: u32, new_text: &str| {
                TextEdit::new(
                    lsp_types::Range::new(Position::new(line, 0), Position::new(line, end)),
                    new_text.to_string(),
                )
            };
            Task::ready(Ok(Some(vec![edit(1, 4, "\t"), edit(2, 0, "\t")])))
        }

        fn range_format(
            &self,
            _: &Rope,
            range: Range<usize>,
            options: FormattingOptions,
            _: &mut Window,
            _: &mut App,
        ) -> Task<Result<Option<Vec<TextEdit>>>> {
            *self.options_seen.borrow_mut() = Some(options);
            *self.range_seen.borrow_mut() = Some(range);
            Task::ready(Ok(Some(vec![])))
        }

        fn supports_range_format(&self) -> bool {
            self.range_supported
        }
    }

    #[gpui::test]
    fn format_carries_tab_options_and_keeps_the_cursor(cx: &mut TestAppContext) {
        let (editor, mut cx) = build_editor(cx);
        let provider = Rc::new(RecordingFormatter {
            range_supported: false,
            ..Default::default()
        });

        cx.update(|window, cx| {
            editor.update(cx, |editor, cx| {
                editor.set_value("func main() {\n    a()\nb()\n}", window, cx);
                editor.extras.lsp.formatting_provider = Some(provider.clone());
                // Cursor on the "b" of the third line.
                let offset = "func main() {\n    a()\n".len();
                editor.selected_range = (offset..offset).into();
                editor.on_action_format(&Format, window, cx);
            });
        });
        cx.run_until_parked();

        let options = provider.options_seen.borrow().clone().unwrap();
        assert_eq!(options.tab_size, 2);
        assert!(options.insert_spaces);

        cx.update(|_, cx| {
            let editor = editor.read(cx);
            assert_eq!(editor.text().to_string(), "func main() {\n\ta()\n\tb()\n}");
            // The cursor stayed on "b" even though text before it shrank
            // and grew.
            assert_eq!(editor.cursor(), "func main() {\n\ta()\n\t".len());
        });
    }

    #[gpui::test]
    fn selection_dispatches_to_range_format_when_supported(cx: &mut TestAppContext) {
        let (editor, mut cx) = build_editor(cx);
        let provider = Rc::new(RecordingFormatter {
            range_supported: true,
            ..Default::default()
        });

        cx.update(|window, cx| {
            editor.update(cx, |editor, cx| {
                editor.set_value("aaa bbb ccc", window, cx);
                editor.extras.lsp.formatting_provider = Some(provider.clone());

                // A selection goes to range formatting, reversed or not.
                editor.selected_range = (8..4).into();
                editor.on_action_format(&Format, window, cx);
                assert_eq!(*provider.range_seen.borrow(), Some(4..8));

                // No selection falls back to the whole document.
                editor.selected_range = (4..4).into();
                editor.on_action_format(&Format, window, cx);
                assert_eq!(*provider.range_seen.borrow(), None);
            });
        });
    }

    struct StaleFormatter;

    impl FormattingProvider for StaleFormatter {
        fn format(
            &self,
            _: &Rope,
            _: FormattingOptions,
            _: &mut Window,
            _: &mut App,
        ) -> Task<Result<Option<Vec<TextEdit>>>> {
            Task::ready(Ok(Some(vec![TextEdit::new(
                lsp_types::Range::new(Position::new(0, 0), Position::new(0, 3)),
                "XXX".to_string(),
            )])))
        }

        fn range_format(
            &self,
            _: &Rope,
            _: Range<usize>,
            _: FormattingOptions,
            _: &mut Window,
            _: &mut App,
        ) -> Task<Result<Option<Vec<TextEdit>>>> {
            Task::ready(Ok(None))
        }
    }

    #[gpui::test]
    fn stale_format_responses_are_discarded(cx: &mut TestAppContext) {
        let (editor, mut cx) = build_editor(cx);

        cx.update(|window, cx| {
            editor.update(cx, |editor, cx| {
                editor.set_value("aaa", window, cx);
                editor.extras.lsp.formatting_provider = Some(Rc::new(StaleFormatter));
                editor.on_action_format(&Format, window, cx);
                // The document changes before the response resolves.
                editor.set_value("aaa bbb", window, cx);
            });
        });
        cx.run_until_parked();

        cx.update(|_, cx| {
            assert_eq!(editor.read(cx).text().to_string(), "aaa bbb");
        });
    }
}
