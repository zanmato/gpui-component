use anyhow::Result;
use gpui::{App, Context, Task, Window};
use lsp_types::{SignatureHelp, SignatureHelpContext, SignatureHelpTriggerKind};
use ropey::Rope;
use std::ops::Range;

use crate::input::{EditorMode, InputBaseState, ToggleSignatureHelp};

/// Signature help provider.
///
/// https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#textDocument_signatureHelp
pub trait SignatureHelpProvider {
    /// Fetches signature help for the given byte offset.
    ///
    /// textDocument/signatureHelp
    ///
    /// https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#textDocument_signatureHelp
    fn signature_help(
        &self,
        text: &Rope,
        offset: usize,
        context: SignatureHelpContext,
        window: &mut Window,
        cx: &mut App,
    ) -> Task<Result<Option<SignatureHelp>>>;

    /// The characters that trigger signature help when typed, typically
    /// `(` and `,`.
    fn trigger_characters(&self) -> Vec<String> {
        vec![]
    }

    /// Extra characters that re-trigger while help is already showing.
    fn retrigger_characters(&self) -> Vec<String> {
        vec![]
    }
}

/// The signature help the editor is currently showing, mirrored by the
/// popover renderer through a revision check.
#[derive(Clone, Debug, Default)]
pub struct SignatureHelpState {
    /// The active help, `None` when the popover is hidden.
    pub help: Option<SignatureHelp>,
    revision: u64,
}

impl SignatureHelpState {
    /// Bumped whenever the content changes. See
    /// [`super::CompletionMenuState::revision`].
    pub fn revision(&self) -> u64 {
        self.revision
    }

    fn set(&mut self, help: Option<SignatureHelp>) {
        self.help = help;
        self.revision = self.revision.wrapping_add(1);
    }
}

impl InputBaseState<EditorMode> {
    /// The signature help currently showing.
    #[doc(hidden)]
    pub fn signature_help_state(&self) -> &SignatureHelpState {
        &self.extras.signature_help
    }

    pub(crate) fn on_action_toggle_signature_help(
        &mut self,
        _: &ToggleSignatureHelp,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.extras.signature_help.help.is_some() {
            self.dismiss_signature_help(cx);
            return;
        }
        self.request_signature_help(
            SignatureHelpContext {
                trigger_kind: SignatureHelpTriggerKind::INVOKED,
                trigger_character: None,
                is_retrigger: false,
                active_signature_help: None,
            },
            window,
            cx,
        );
    }

    /// Called for freshly typed text: opens help on a trigger character and
    /// keeps already-open help current while typing inside the call.
    pub(crate) fn handle_signature_help_trigger(
        &mut self,
        _range: &Range<usize>,
        new_text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(provider) = self.extras.lsp.signature_help_provider.clone() else {
            return;
        };

        let active = self.extras.signature_help.help.clone();
        let typed = new_text.chars().last().map(|c| c.to_string());
        let triggered = typed.as_ref().is_some_and(|c| {
            provider.trigger_characters().contains(c)
                || (active.is_some() && provider.retrigger_characters().contains(c))
        });
        if !triggered && active.is_none() {
            return;
        }

        self.request_signature_help(
            SignatureHelpContext {
                trigger_kind: if triggered {
                    SignatureHelpTriggerKind::TRIGGER_CHARACTER
                } else {
                    SignatureHelpTriggerKind::CONTENT_CHANGE
                },
                trigger_character: triggered.then_some(typed).flatten(),
                is_retrigger: active.is_some(),
                active_signature_help: active,
            },
            window,
            cx,
        );
    }

    fn request_signature_help(
        &mut self,
        context: SignatureHelpContext,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(provider) = self.extras.lsp.signature_help_provider.clone() else {
            return;
        };

        let offset = self.cursor();
        let task = provider.signature_help(&self.text, offset, context, window, cx);
        let version = self.document_version;
        self.extras.lsp._signature_help_task = cx.spawn_in(window, async move |editor, cx| {
            let Ok(help) = task.await else {
                return;
            };
            editor
                .update(cx, |editor, cx| {
                    if editor.document_version != version {
                        return;
                    }
                    editor.extras.signature_help.set(help);
                    cx.notify();
                })
                .ok();
        });
    }

    /// Hide the signature help popover. Returns whether it was showing.
    pub fn dismiss_signature_help(&mut self, cx: &mut Context<Self>) -> bool {
        if self.extras.signature_help.help.is_none() {
            return false;
        }
        self.extras.signature_help.set(None);
        // Drop an in-flight request so it cannot re-open the popover.
        self.extras.lsp._signature_help_task = Task::ready(());
        cx.notify();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_support::build_editor;
    use super::*;
    use gpui::{EntityInputHandler, TestAppContext};
    use lsp_types::{ParameterInformation, SignatureInformation};
    use std::cell::RefCell;
    use std::rc::Rc;

    #[derive(Default)]
    struct RecordingProvider {
        requests: Rc<RefCell<Vec<SignatureHelpContext>>>,
        respond_with_none: std::cell::Cell<bool>,
    }

    impl SignatureHelpProvider for RecordingProvider {
        fn signature_help(
            &self,
            _: &Rope,
            _: usize,
            context: SignatureHelpContext,
            _: &mut Window,
            _: &mut App,
        ) -> Task<Result<Option<SignatureHelp>>> {
            self.requests.borrow_mut().push(context.clone());
            if self.respond_with_none.get() {
                return Task::ready(Ok(None));
            }
            let active_parameter = if context.is_retrigger {
                Some(1)
            } else {
                Some(0)
            };
            Task::ready(Ok(Some(SignatureHelp {
                signatures: vec![SignatureInformation {
                    label: "fn add(a: i32, b: i32)".into(),
                    documentation: None,
                    parameters: Some(vec![
                        ParameterInformation {
                            label: lsp_types::ParameterLabel::Simple("a: i32".into()),
                            documentation: None,
                        },
                        ParameterInformation {
                            label: lsp_types::ParameterLabel::Simple("b: i32".into()),
                            documentation: None,
                        },
                    ]),
                    active_parameter,
                }],
                active_signature: Some(0),
                active_parameter,
            })))
        }

        fn trigger_characters(&self) -> Vec<String> {
            vec!["(".into()]
        }

        fn retrigger_characters(&self) -> Vec<String> {
            vec![",".into()]
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
    fn typing_a_trigger_character_opens_and_retriggers_signature_help(cx: &mut TestAppContext) {
        let (editor, mut cx) = build_editor(cx);
        let provider = Rc::new(RecordingProvider::default());
        let requests = provider.requests.clone();

        cx.update(|window, cx| {
            editor.update(cx, |editor, cx| {
                editor.set_value("add", window, cx);
                editor.selected_range = (3..3).into();
                editor.extras.lsp.signature_help_provider = Some(provider.clone());
            });
        });

        // A non-trigger character with no open help does nothing.
        type_text(&editor, "x", &mut cx);
        cx.run_until_parked();
        assert!(requests.borrow().is_empty());

        // The trigger character opens help.
        type_text(&editor, "(", &mut cx);
        cx.run_until_parked();
        {
            let requests = requests.borrow();
            assert_eq!(requests.len(), 1);
            assert_eq!(requests[0].trigger_character.as_deref(), Some("("));
            assert!(!requests[0].is_retrigger);
        }
        cx.update(|_, cx| {
            let state = editor.read(cx).signature_help_state();
            assert!(state.help.is_some());
        });

        // Typing while open re-requests with the active help attached.
        type_text(&editor, "1", &mut cx);
        cx.run_until_parked();
        {
            let requests = requests.borrow();
            assert_eq!(requests.len(), 2);
            assert!(requests[1].is_retrigger);
            assert!(requests[1].active_signature_help.is_some());
        }

        // The server answering `null` dismisses the popover.
        provider.respond_with_none.set(true);
        type_text(&editor, "2", &mut cx);
        cx.run_until_parked();
        cx.update(|_, cx| {
            assert!(editor.read(cx).signature_help_state().help.is_none());
        });
    }

    #[gpui::test]
    fn escape_dismisses_signature_help(cx: &mut TestAppContext) {
        let (editor, mut cx) = build_editor(cx);
        cx.update(|window, cx| {
            editor.update(cx, |editor, cx| {
                editor.set_value("add", window, cx);
                editor.selected_range = (3..3).into();
                editor.extras.lsp.signature_help_provider =
                    Some(Rc::new(RecordingProvider::default()));
            });
        });
        type_text(&editor, "(", &mut cx);
        cx.run_until_parked();

        cx.update(|window, cx| {
            editor.update(cx, |editor, cx| {
                assert!(editor.signature_help_state().help.is_some());
                editor.escape(&crate::input::Escape, window, cx);
                assert!(editor.signature_help_state().help.is_none());
            });
        });
    }
}
