use anyhow::Result;
use gpui::{App, Context, Task, Window};
use instant::Duration;
use lsp_types::{SignatureHelp, SignatureHelpContext, SignatureHelpTriggerKind};
use ropey::Rope;
use std::ops::Range;

use crate::input::{
    EditorMode, InputBaseState, SignatureHelpNext, SignatureHelpPrevious, ToggleSignatureHelp,
};

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
    /// The overload the user cycled to, overriding the server's choice.
    active_signature: Option<usize>,
    revision: u64,
}

impl SignatureHelpState {
    /// Bumped whenever the content changes. See
    /// [`super::CompletionMenuState::revision`].
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// The index of the signature to show: the overload the user cycled to,
    /// else the server's active signature.
    pub fn active_signature(&self) -> Option<usize> {
        let help = self.help.as_ref()?;
        if help.signatures.is_empty() {
            return None;
        }
        let index = self
            .active_signature
            .unwrap_or(help.active_signature.unwrap_or(0) as usize);
        Some(index.min(help.signatures.len() - 1))
    }

    /// The index of the parameter to emphasize in the active signature.
    pub fn active_parameter(&self) -> Option<usize> {
        let help = self.help.as_ref()?;
        let signature = help.signatures.get(self.active_signature()?)?;
        signature
            .active_parameter
            .or(help.active_parameter)
            .map(|index| index as usize)
    }

    /// Whether there is more than one overload to cycle through.
    pub fn has_multiple_signatures(&self) -> bool {
        self.help
            .as_ref()
            .is_some_and(|help| help.signatures.len() > 1)
    }

    fn set(&mut self, help: Option<SignatureHelp>) {
        // Keep the overload the user picked while the same call stays open.
        let same_shape = match (&self.help, &help) {
            (Some(previous), Some(next)) => previous.signatures.len() == next.signatures.len(),
            _ => false,
        };
        if !same_shape {
            self.active_signature = None;
        }
        self.help = help;
        self.revision = self.revision.wrapping_add(1);
    }
}

/// How long a first request waits for the cursor to settle, so arrowing
/// through a call does not fire a request per keystroke.
const OPEN_DELAY: Duration = Duration::from_millis(150);

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

    pub(crate) fn on_action_signature_help_next(
        &mut self,
        _: &SignatureHelpNext,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.cycle_signature(1, cx);
    }

    pub(crate) fn on_action_signature_help_previous(
        &mut self,
        _: &SignatureHelpPrevious,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.cycle_signature(-1, cx);
    }

    /// Show the next (or previous) overload of the open signature help.
    pub fn cycle_signature(&mut self, delta: isize, cx: &mut Context<Self>) {
        let state = &mut self.extras.signature_help;
        let Some(current) = state.active_signature() else {
            return;
        };
        let count = state.help.as_ref().map_or(0, |help| help.signatures.len());
        if count < 2 {
            return;
        }
        state.active_signature =
            Some((current as isize + delta).rem_euclid(count as isize) as usize);
        state.revision = state.revision.wrapping_add(1);
        cx.notify();
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

    /// Called when the cursor moves without an edit, so entering a call opens
    /// help and leaving it closes help. A selection never shows help.
    pub(crate) fn refresh_signature_help(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.extras.lsp.signature_help_provider.is_none() {
            return;
        }
        if !self.active_selection().is_empty() {
            self.dismiss_signature_help(cx);
            return;
        }

        let active = self.extras.signature_help.help.clone();
        self.request_signature_help(
            SignatureHelpContext {
                trigger_kind: SignatureHelpTriggerKind::CONTENT_CHANGE,
                trigger_character: None,
                is_retrigger: active.is_some(),
                active_signature_help: active,
            },
            window,
            cx,
        );
    }

    /// Ask the provider for help at the cursor. A request that would open the
    /// popover first waits for the cursor to settle; a request that keeps it
    /// current goes out at once. The response is dropped when the document
    /// or the cursor moved on in the meantime.
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
        let version = self.document_version;
        let delay = self.extras.signature_help.help.is_none();
        self.extras.lsp._signature_help_task = cx.spawn_in(window, async move |editor, cx| {
            if delay {
                cx.background_executor().timer(OPEN_DELAY).await;
            }

            let Ok(task) = editor.update_in(cx, |editor, window, cx| {
                if editor.document_version != version || editor.cursor() != offset {
                    return None;
                }
                Some(provider.signature_help(&editor.text, offset, context, window, cx))
            }) else {
                return;
            };
            let Some(task) = task else {
                return;
            };
            let Ok(help) = task.await else {
                return;
            };

            editor
                .update(cx, |editor, cx| {
                    if editor.document_version != version || editor.cursor() != offset {
                        return;
                    }
                    let help = help.filter(|help| !help.signatures.is_empty());
                    editor.extras.signature_help.set(help);
                    cx.notify();
                })
                .ok();
        });
    }

    /// Hide the signature help popover. Returns whether it was showing.
    pub fn dismiss_signature_help(&mut self, cx: &mut Context<Self>) -> bool {
        // Drop an in-flight request so it cannot re-open the popover.
        self.extras.lsp._signature_help_task = Task::ready(());
        if self.extras.signature_help.help.is_none() {
            return false;
        }
        self.extras.signature_help.set(None);
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
        overloads: std::cell::Cell<usize>,
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
            let overloads = self.overloads.get().max(1);
            Task::ready(Ok(Some(SignatureHelp {
                signatures: vec![
                    SignatureInformation {
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
                    };
                    overloads
                ],
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

    /// Let a debounced first request go out and resolve.
    fn settle(cx: &mut gpui::VisualTestContext) {
        cx.executor().advance_clock(OPEN_DELAY);
        cx.run_until_parked();
    }

    fn move_cursor(
        editor: &gpui::Entity<InputBaseState<EditorMode>>,
        offset: usize,
        cx: &mut gpui::VisualTestContext,
    ) {
        let editor = editor.clone();
        cx.update(|window, cx| {
            editor.update(cx, |editor, cx| editor.move_to(offset, None, window, cx));
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
                editor.set_selected_range(3..3, window, cx);
                editor.extras.lsp.signature_help_provider = Some(provider.clone());
            });
        });

        // A non-trigger character with no open help does nothing.
        type_text(&editor, "x", &mut cx);
        cx.run_until_parked();
        assert!(requests.borrow().is_empty());

        // The trigger character opens help once the cursor settles.
        type_text(&editor, "(", &mut cx);
        cx.run_until_parked();
        assert!(requests.borrow().is_empty());
        settle(&mut cx);
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
                editor.set_selected_range(3..3, window, cx);
                editor.extras.lsp.signature_help_provider =
                    Some(Rc::new(RecordingProvider::default()));
            });
        });
        type_text(&editor, "(", &mut cx);
        settle(&mut cx);

        cx.update(|window, cx| {
            editor.update(cx, |editor, cx| {
                assert!(editor.signature_help_state().help.is_some());
                editor.escape(&crate::input::Escape, window, cx);
                assert!(editor.signature_help_state().help.is_none());
            });
        });
    }

    #[gpui::test]
    fn cursor_movement_opens_and_closes_signature_help(cx: &mut TestAppContext) {
        let (editor, mut cx) = build_editor(cx);
        let provider = Rc::new(RecordingProvider::default());
        let requests = provider.requests.clone();
        cx.update(|window, cx| {
            editor.update(cx, |editor, cx| {
                editor.set_value("add(1, 2)", window, cx);
                editor.extras.lsp.signature_help_provider = Some(provider.clone());
            });
        });

        // Arrowing into the call asks once the cursor settles, as a fresh
        // request rather than a retrigger.
        move_cursor(&editor, 4, &mut cx);
        cx.run_until_parked();
        assert!(requests.borrow().is_empty());
        settle(&mut cx);
        {
            let requests = requests.borrow();
            assert_eq!(requests.len(), 1);
            assert_eq!(
                requests[0].trigger_kind,
                SignatureHelpTriggerKind::CONTENT_CHANGE
            );
            assert!(!requests[0].is_retrigger);
        }
        cx.update(|_, cx| assert!(editor.read(cx).signature_help_state().help.is_some()));

        // Moving on while open re-asks at once with the active help attached.
        move_cursor(&editor, 7, &mut cx);
        cx.run_until_parked();
        {
            let requests = requests.borrow();
            assert_eq!(requests.len(), 2);
            assert!(requests[1].is_retrigger);
            assert!(requests[1].active_signature_help.is_some());
        }

        // Two quick moves only ask for the final position.
        move_cursor(&editor, 5, &mut cx);
        move_cursor(&editor, 6, &mut cx);
        cx.run_until_parked();
        assert_eq!(requests.borrow().len(), 3);

        // Leaving the call closes help when the server answers `null`.
        provider.respond_with_none.set(true);
        move_cursor(&editor, 0, &mut cx);
        cx.run_until_parked();
        cx.update(|_, cx| assert!(editor.read(cx).signature_help_state().help.is_none()));

        // A selection never shows help.
        provider.respond_with_none.set(false);
        let before = requests.borrow().len();
        cx.update(|window, cx| {
            editor.update(cx, |editor, cx| editor.set_selected_range(4..5, window, cx));
        });
        settle(&mut cx);
        assert_eq!(requests.borrow().len(), before);
    }

    #[gpui::test]
    fn cycling_overloads_keeps_the_choice_across_retriggers(cx: &mut TestAppContext) {
        let (editor, mut cx) = build_editor(cx);
        let provider = Rc::new(RecordingProvider::default());
        provider.overloads.set(3);
        cx.update(|window, cx| {
            editor.update(cx, |editor, cx| {
                editor.set_value("add", window, cx);
                editor.set_selected_range(3..3, window, cx);
                editor.extras.lsp.signature_help_provider = Some(provider.clone());
            });
        });
        type_text(&editor, "(", &mut cx);
        settle(&mut cx);

        cx.update(|window, cx| {
            editor.update(cx, |editor, cx| {
                let state = editor.signature_help_state();
                assert!(state.has_multiple_signatures());
                assert_eq!(state.active_signature(), Some(0));
                editor.on_action_signature_help_previous(&SignatureHelpPrevious, window, cx);
                assert_eq!(editor.signature_help_state().active_signature(), Some(2));
                editor.on_action_signature_help_next(&SignatureHelpNext, window, cx);
                assert_eq!(editor.signature_help_state().active_signature(), Some(0));
                editor.on_action_signature_help_next(&SignatureHelpNext, window, cx);
                assert_eq!(editor.signature_help_state().active_signature(), Some(1));
            });
        });

        // Typing inside the call keeps the chosen overload.
        type_text(&editor, "1", &mut cx);
        cx.run_until_parked();
        cx.update(|_, cx| {
            let state = editor.read(cx).signature_help_state();
            assert_eq!(state.active_signature(), Some(1));
            assert_eq!(state.active_parameter(), Some(1));
        });

        // Blur closes it.
        cx.update(|window, cx| {
            editor.update(cx, |editor, cx| editor.on_blur(window, cx));
        });
        cx.update(|_, cx| assert!(editor.read(cx).signature_help_state().help.is_none()));
    }
}
