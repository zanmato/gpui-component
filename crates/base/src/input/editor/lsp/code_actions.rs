use anyhow::Result;
use gpui::{Action, App, Context, Entity, SharedString, Task, Window};
use lsp_types::CodeAction;
use serde::Deserialize;
use std::ops::Range;

use crate::input::{EditorMode, EditorState, InputBaseState, NativeMenu, ToggleCodeActions};

/// Performs one of the code actions the context menu lists, by its place
/// in that list.
#[derive(Action, Clone, PartialEq, Eq, Deserialize)]
#[action(namespace = input, no_json)]
pub struct PerformContextMenuCodeAction {
    pub index: usize,
}

pub trait CodeActionProvider {
    /// The id for this CodeAction.
    fn id(&self) -> SharedString;

    /// Fetches code actions for the specified range.
    ///
    /// textDocument/codeAction
    ///
    /// https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#textDocument_codeAction
    fn code_actions(
        &self,
        state: Entity<EditorState>,
        range: Range<usize>,
        window: &mut Window,
        cx: &mut App,
    ) -> Task<Result<Vec<CodeAction>>>;

    /// Performs the specified code action.
    fn perform_code_action(
        &self,
        state: Entity<EditorState>,
        action: CodeAction,
        push_to_history: bool,
        window: &mut Window,
        cx: &mut App,
    ) -> Task<Result<()>>;
}

#[derive(Clone, Debug)]
pub struct CodeActionItem {
    pub provider_id: SharedString,
    pub action: CodeAction,
}

impl InputBaseState<EditorMode> {
    pub(crate) fn on_action_toggle_code_actions(
        &mut self,
        _: &ToggleCodeActions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.handle_code_action_trigger(window, cx)
    }

    /// Show code actions for the cursor.
    pub(crate) fn handle_code_action_trigger(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let version = self.document_version;
        let fetch = self.fetch_code_actions(window, cx);
        self.extras.context_menu_task = cx.spawn_in(window, async move |editor, cx| {
            let code_actions = fetch.await;

            if code_actions.is_empty() {
                editor.update(cx, |editor, cx| {
                    editor.extras.context_menu_content.code_action.open = false;
                    editor.extras.context_menu_content.code_action.items.clear();
                    editor.extras.context_menu_content.code_action.bump();
                    cx.notify();
                })?;
                return Ok(());
            }
            editor
                .update_in(cx, |editor, window, cx| {
                    if !editor.focus_handle.is_focused(window) {
                        return;
                    }
                    if editor.document_version != version {
                        return;
                    }

                    editor.extras.context_menu_content.code_action.items = code_actions;
                    editor.extras.context_menu_content.code_action.open = !editor
                        .extras
                        .context_menu_content
                        .code_action
                        .items
                        .is_empty();
                    // The overlay rebuilds on a new revision, so a refresh
                    // while the menu is already open must bump it too.
                    editor.extras.context_menu_content.code_action.bump();

                    cx.notify();
                })
                .ok();

            Ok(())
        });
    }

    /// Asks every provider for the code actions at the selection. A provider
    /// that fails offers nothing, the others still answer.
    pub(crate) fn fetch_code_actions(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<Vec<CodeActionItem>> {
        let providers = self.extras.lsp.code_action_providers.clone();
        let range = self.selected_range();
        let state = cx.entity();
        // Providers read the state, which is borrowed until this update
        // returns, so they are asked from the spawned task.
        cx.spawn_in(window, async move |_, cx| {
            let mut provider_responses = vec![];
            _ = cx.update(|window, cx| {
                for provider in providers {
                    let task = provider.code_actions(state.clone(), range.clone(), window, cx);
                    provider_responses.push((provider.id(), task));
                }
            });

            let mut code_actions: Vec<CodeActionItem> = vec![];
            for (provider_id, provider_response) in provider_responses {
                if let Ok(responses) = provider_response.await {
                    code_actions.extend(responses.into_iter().map(|action| CodeActionItem {
                        provider_id: provider_id.clone(),
                        action,
                    }))
                }
            }
            code_actions
        })
    }

    /// Keeps the code actions found for a right click and lists them as the
    /// first entries of its menu.
    pub(crate) fn context_menu_code_action_entries(
        &mut self,
        items: Vec<CodeActionItem>,
    ) -> NativeMenu {
        let menu = items
            .iter()
            .enumerate()
            .fold(NativeMenu::new(), |menu, (index, item)| {
                menu.menu(
                    item.action.title.clone(),
                    Box::new(PerformContextMenuCodeAction { index }),
                )
            });
        self.extras.context_menu_code_actions = (self.document_version, items);
        menu
    }

    pub(crate) fn on_action_perform_context_menu_code_action(
        &mut self,
        action: &PerformContextMenuCodeAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (version, items) = std::mem::take(&mut self.extras.context_menu_code_actions);
        // Its edits address the text the action was found for.
        if version != self.document_version {
            return;
        }
        let Some(item) = items.into_iter().nth(action.index) else {
            return;
        };
        self.perform_code_action(&item, window, cx);
    }

    pub fn perform_code_action(
        &mut self,
        item: &CodeActionItem,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let providers = self.extras.lsp.code_action_providers.clone();
        let Some(provider) = providers
            .iter()
            .find(|provider| provider.id() == item.provider_id)
        else {
            return;
        };

        let state = cx.entity();
        let task = provider.perform_code_action(state, item.action.clone(), true, window, cx);

        cx.spawn_in(window, async move |_, _| {
            let _ = task.await;
        })
        .detach();
    }
}
