use anyhow::Result;
use gpui::{App, Context, SharedString, Task, Window};
use lsp_types::{PrepareRenameResponse, WorkspaceEdit};
use ropey::Rope;
use std::ops::Range;

use crate::input::{EditorMode, InputBaseState, Rename, RopeExt};

/// Rename provider: validate the symbol under the cursor and rename every
/// occurrence through a workspace edit.
///
/// https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#textDocument_rename
pub trait RenameProvider {
    /// Validate that the offset is renameable and report the range and
    /// placeholder to prefill.
    ///
    /// textDocument/prepareRename
    ///
    /// https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#textDocument_prepareRename
    fn prepare_rename(
        &self,
        text: &Rope,
        offset: usize,
        window: &mut Window,
        cx: &mut App,
    ) -> Task<Result<Option<PrepareRenameResponse>>>;

    /// Compute the workspace edit renaming the symbol at the offset.
    ///
    /// textDocument/rename
    ///
    /// https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#textDocument_rename
    fn rename(
        &self,
        text: &Rope,
        offset: usize,
        new_name: &str,
        window: &mut Window,
        cx: &mut App,
    ) -> Task<Result<Option<WorkspaceEdit>>>;

    /// Whether the server implements prepareRename. When it does not, the
    /// word under the cursor stands in for the validated range.
    fn supports_prepare(&self) -> bool {
        true
    }
}

/// The rename prompt the editor is showing, mirrored by the popover
/// renderer through a revision check.
#[derive(Clone, Debug, Default)]
pub struct RenamePromptState {
    /// The active prompt, `None` when no rename is in progress.
    pub prompt: Option<RenamePrompt>,
    revision: u64,
}

/// One pending rename: the validated symbol range and the name to prefill.
#[derive(Clone, Debug)]
pub struct RenamePrompt {
    pub symbol_range: Range<usize>,
    pub placeholder: SharedString,
}

impl RenamePromptState {
    /// Bumped whenever the content changes. See
    /// [`super::CompletionMenuState::revision`].
    pub fn revision(&self) -> u64 {
        self.revision
    }

    fn set(&mut self, prompt: Option<RenamePrompt>) {
        self.prompt = prompt;
        self.revision = self.revision.wrapping_add(1);
    }
}

impl InputBaseState<EditorMode> {
    /// The rename prompt state.
    #[doc(hidden)]
    pub fn rename_prompt_state(&self) -> &RenamePromptState {
        &self.extras.rename_prompt
    }

    pub(crate) fn on_action_rename(
        &mut self,
        _: &Rename,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(provider) = self.extras.lsp.rename_provider.clone() else {
            return;
        };

        let offset = self.cursor();
        if !provider.supports_prepare() {
            // No server-side validation: the word under the cursor is the
            // best available guess.
            let Some(range) = self.text.word_range(offset) else {
                return;
            };
            self.open_rename_prompt(range, cx);
            return;
        }

        let version = self.document_version;
        let task = provider.prepare_rename(&self.text, offset, window, cx);
        self.extras.lsp._rename_task = cx.spawn_in(window, async move |editor, cx| {
            let Ok(response) = task.await else {
                return;
            };
            editor
                .update(cx, |editor, cx| {
                    if editor.document_version != version {
                        return;
                    }
                    let range = match response {
                        // The server rejected the position.
                        None => return,
                        Some(PrepareRenameResponse::Range(range))
                        | Some(PrepareRenameResponse::RangeWithPlaceholder { range, .. }) => {
                            editor.text.position_to_offset(&range.start)
                                ..editor.text.position_to_offset(&range.end)
                        }
                        Some(PrepareRenameResponse::DefaultBehavior { .. }) => {
                            match editor.text.word_range(editor.cursor()) {
                                Some(range) => range,
                                None => return,
                            }
                        }
                    };
                    editor.open_rename_prompt(range, cx);
                })
                .ok();
        });
    }

    fn open_rename_prompt(&mut self, symbol_range: Range<usize>, cx: &mut Context<Self>) {
        let placeholder = SharedString::from(self.text.slice(symbol_range.clone()).to_string());
        self.extras.rename_prompt.set(Some(RenamePrompt {
            symbol_range,
            placeholder,
        }));
        cx.notify();
    }

    /// Ask the provider for the workspace edit and apply it. Called by the
    /// rename popover on confirm.
    pub fn commit_rename(&mut self, new_name: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(prompt) = self.extras.rename_prompt.prompt.clone() else {
            return;
        };
        self.extras.rename_prompt.set(None);
        cx.notify();

        let Some(provider) = self.extras.lsp.rename_provider.clone() else {
            return;
        };
        if new_name.is_empty()
            || new_name.as_bytes()
                == self
                    .text
                    .slice(prompt.symbol_range.clone())
                    .to_string()
                    .as_bytes()
        {
            return;
        }

        let version = self.document_version;
        let task = provider.rename(&self.text, prompt.symbol_range.start, new_name, window, cx);
        self.extras.lsp._rename_task = cx.spawn_in(window, async move |editor, cx| {
            let Ok(Some(edit)) = task.await else {
                return;
            };
            editor
                .update_in(cx, |editor, window, cx| {
                    if editor.document_version != version {
                        return;
                    }
                    editor.apply_workspace_edit(&edit, window, cx);
                })
                .ok();
        });
    }

    /// Close the rename prompt without renaming.
    pub fn cancel_rename(&mut self, cx: &mut Context<Self>) {
        if self.extras.rename_prompt.prompt.is_some() {
            self.extras.rename_prompt.set(None);
            cx.notify();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_support::build_editor;
    use super::*;
    use crate::input::Undo;
    use gpui::TestAppContext;
    use lsp_types::Position;
    use std::cell::Cell;
    use std::rc::Rc;

    struct GreetRename {
        reject: Cell<bool>,
    }

    impl RenameProvider for GreetRename {
        fn prepare_rename(
            &self,
            _: &Rope,
            _: usize,
            _: &mut Window,
            _: &mut App,
        ) -> Task<Result<Option<PrepareRenameResponse>>> {
            if self.reject.get() {
                return Task::ready(Ok(None));
            }
            Task::ready(Ok(Some(PrepareRenameResponse::RangeWithPlaceholder {
                range: lsp_types::Range::new(Position::new(0, 0), Position::new(0, 5)),
                placeholder: "greet".into(),
            })))
        }

        fn rename(
            &self,
            _: &Rope,
            _: usize,
            new_name: &str,
            _: &mut Window,
            _: &mut App,
        ) -> Task<Result<Option<WorkspaceEdit>>> {
            // Rename all three occurrences of "greet"; ranges are given
            // out of order on purpose.
            let edit = |line: u32, start: u32| {
                lsp_types::TextEdit::new(
                    lsp_types::Range::new(
                        Position::new(line, start),
                        Position::new(line, start + 5),
                    ),
                    new_name.to_string(),
                )
            };
            let mut changes = std::collections::HashMap::new();
            changes.insert(
                "file:///doc.go".parse().unwrap(),
                vec![edit(2, 0), edit(0, 0), edit(1, 3)],
            );
            Task::ready(Ok(Some(WorkspaceEdit {
                changes: Some(changes),
                ..Default::default()
            })))
        }
    }

    #[gpui::test]
    fn rename_prepares_commits_atomically_and_rejects(cx: &mut TestAppContext) {
        let (editor, mut cx) = build_editor(cx);
        let provider = Rc::new(GreetRename {
            reject: Cell::new(false),
        });

        cx.update(|window, cx| {
            editor.update(cx, |editor, cx| {
                editor.set_value("greet one\nfn greet()\ngreet me", window, cx);
                editor
                    .extras
                    .lsp
                    .set_document_uri("file:///doc.go".parse().unwrap());
                editor.extras.lsp.rename_provider = Some(provider.clone());
            });
        });

        // Prepare fills the prompt with the validated range and name.
        cx.update(|window, cx| {
            editor.update(cx, |editor, cx| {
                editor.on_action_rename(&Rename, window, cx)
            });
        });
        cx.run_until_parked();
        cx.update(|_, cx| {
            let prompt = editor.read(cx).rename_prompt_state().prompt.clone();
            let prompt = prompt.expect("prompt is open");
            assert_eq!(prompt.symbol_range, 0..5);
            assert_eq!(prompt.placeholder.as_ref(), "greet");
        });

        // Committing applies every occurrence as one atomic edit…
        cx.update(|window, cx| {
            editor.update(cx, |editor, cx| editor.commit_rename("salute", window, cx));
        });
        cx.run_until_parked();
        cx.update(|_, cx| {
            assert_eq!(
                editor.read(cx).text().to_string(),
                "salute one\nfn salute()\nsalute me"
            );
            assert!(editor.read(cx).rename_prompt_state().prompt.is_none());
        });

        // …and undoes as one step.
        cx.update(|window, cx| {
            editor.update(cx, |editor, cx| {
                editor.undo(&Undo, window, cx);
                assert_eq!(editor.text().to_string(), "greet one\nfn greet()\ngreet me");
            });
        });

        // A rejected prepare opens nothing.
        provider.reject.set(true);
        cx.update(|window, cx| {
            editor.update(cx, |editor, cx| {
                editor.on_action_rename(&Rename, window, cx)
            });
        });
        cx.run_until_parked();
        cx.update(|_, cx| {
            assert!(editor.read(cx).rename_prompt_state().prompt.is_none());
        });
    }
}
