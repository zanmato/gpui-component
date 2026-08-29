use anyhow::Result;
use gpui::{App, Context, Task, Window};
use lsp_types::{FormattingOptions, TextEdit};
use ropey::Rope;
use std::ops::Range;

use crate::input::{EditorMode, InputBaseState};

/// On-type formatting provider: reformat around the cursor as trigger
/// characters are typed, e.g. re-indenting a block when `}` closes it.
///
/// https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#textDocument_onTypeFormatting
pub trait OnTypeFormattingProvider {
    /// Compute the edits for the character just typed. `offset` is the
    /// cursor position after the insert.
    ///
    /// textDocument/onTypeFormatting
    ///
    /// https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#textDocument_onTypeFormatting
    fn on_type_format(
        &self,
        text: &Rope,
        offset: usize,
        ch: &str,
        options: FormattingOptions,
        window: &mut Window,
        cx: &mut App,
    ) -> Task<Result<Option<Vec<TextEdit>>>>;

    /// The characters that trigger a formatting request when typed,
    /// from the server's `firstTriggerCharacter` and
    /// `moreTriggerCharacter`.
    fn trigger_characters(&self) -> Vec<String> {
        vec![]
    }
}

impl InputBaseState<EditorMode> {
    /// Called for freshly typed text: requests formatting when the last
    /// typed character is one of the provider's triggers. The response is
    /// applied through the silent edit path, which never re-enters this
    /// hook.
    pub(crate) fn handle_on_type_formatting_trigger(
        &mut self,
        _range: &Range<usize>,
        new_text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(provider) = self.extras.lsp.on_type_formatting_provider.clone() else {
            return;
        };
        let Some(typed) = new_text.chars().last().map(|c| c.to_string()) else {
            return;
        };
        if !provider.trigger_characters().contains(&typed) {
            return;
        }

        let tab = self.mode.tab_size();
        let options = FormattingOptions {
            tab_size: tab.tab_size as u32,
            insert_spaces: !tab.hard_tabs,
            ..Default::default()
        };

        let offset = self.cursor();
        let version = self.document_version;
        let task = provider.on_type_format(&self.text, offset, &typed, options, window, cx);
        self.extras.lsp._on_type_format_task = cx.spawn_in(window, async move |editor, cx| {
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
    use gpui::{EntityInputHandler, TestAppContext};
    use lsp_types::Position;
    use std::cell::RefCell;
    use std::rc::Rc;

    /// Fixes the indentation of the line the cursor is on whenever `}` is
    /// typed, recording every request it sees.
    #[derive(Default)]
    struct BraceFormatter {
        requests: RefCell<Vec<(usize, String)>>,
        respond_with_edit: std::cell::Cell<bool>,
    }

    impl OnTypeFormattingProvider for BraceFormatter {
        fn on_type_format(
            &self,
            text: &Rope,
            offset: usize,
            ch: &str,
            _options: FormattingOptions,
            _: &mut Window,
            _: &mut App,
        ) -> Task<Result<Option<Vec<TextEdit>>>> {
            self.requests.borrow_mut().push((offset, ch.to_string()));
            if !self.respond_with_edit.get() {
                return Task::ready(Ok(None));
            }
            // Strip the indentation in front of the typed brace.
            let line = crate::input::RopeExt::offset_to_position(text, offset).line;
            Task::ready(Ok(Some(vec![TextEdit::new(
                lsp_types::Range::new(Position::new(line, 0), Position::new(line, 4)),
                String::new(),
            )])))
        }

        fn trigger_characters(&self) -> Vec<String> {
            vec!["}".into()]
        }
    }

    fn type_text(
        editor: &gpui::Entity<InputBaseState<EditorMode>>,
        text: &str,
        cx: &mut gpui::VisualTestContext,
    ) {
        let editor = editor.clone();
        let text = text.to_string();
        cx.update(|window, cx| {
            editor.update(cx, |editor, cx| {
                let cursor = editor.cursor();
                let range = editor.range_to_utf16(&(cursor..cursor));
                editor.replace_text_in_range(Some(range), &text, window, cx);
            });
        });
    }

    #[gpui::test]
    fn trigger_character_requests_and_applies_without_retriggering(cx: &mut TestAppContext) {
        let (editor, mut cx) = build_editor(cx);
        let provider = Rc::new(BraceFormatter::default());
        provider.respond_with_edit.set(true);

        cx.update(|window, cx| {
            editor.update(cx, |editor, cx| {
                editor.set_value("func main() {\n    ", window, cx);
                editor.selected_range = (18..18).into();
                editor.extras.lsp.on_type_formatting_provider = Some(provider.clone());
            });
        });

        // A non-trigger character stays silent.
        type_text(&editor, "x", &mut cx);
        cx.run_until_parked();
        assert!(provider.requests.borrow().is_empty());

        // The trigger character requests formatting at the post-insert
        // position and the edit lands.
        type_text(&editor, "}", &mut cx);
        cx.run_until_parked();
        {
            let requests = provider.requests.borrow();
            assert_eq!(requests.len(), 1);
            assert_eq!(requests[0], ("func main() {\n    x}".len(), "}".into()));
        }
        cx.update(|_, cx| {
            assert_eq!(
                editor.read(cx).text().to_string(),
                "func main() {\nx}",
                "the response stripped the indentation"
            );
        });

        // Applying the response was itself an edit, but it must not have
        // issued another request.
        assert_eq!(provider.requests.borrow().len(), 1);
    }

    #[gpui::test]
    fn stale_on_type_responses_are_discarded(cx: &mut TestAppContext) {
        let (editor, mut cx) = build_editor(cx);
        let provider = Rc::new(BraceFormatter::default());
        provider.respond_with_edit.set(true);

        cx.update(|window, cx| {
            editor.update(cx, |editor, cx| {
                editor.set_value("    ", window, cx);
                editor.selected_range = (4..4).into();
                editor.extras.lsp.on_type_formatting_provider = Some(provider.clone());
            });
        });

        cx.update(|window, cx| {
            editor.update(cx, |editor, cx| {
                let cursor = editor.cursor();
                let range = editor.range_to_utf16(&(cursor..cursor));
                editor.replace_text_in_range(Some(range), "}", window, cx);
                // The document changes again before the response resolves.
                editor.set_value("    }\nmore", window, cx);
            });
        });
        cx.run_until_parked();

        cx.update(|_, cx| {
            assert_eq!(editor.read(cx).text().to_string(), "    }\nmore");
        });
    }
}
