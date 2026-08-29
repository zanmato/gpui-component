use anyhow::Result;
use gpui::{App, Context, SharedString, Task, Window};
use lsp_types::{Location, LocationLink};
use ropey::Rope;

use crate::input::{
    EditorMode, GoToDeclaration, GoToImplementation, GoToTypeDefinition, InputBaseState,
};

/// Type definition provider, the sibling of
/// [`super::DefinitionProvider`] for `textDocument/typeDefinition`.
///
/// https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#textDocument_typeDefinition
pub trait TypeDefinitionProvider {
    /// textDocument/typeDefinition
    fn type_definitions(
        &self,
        text: &Rope,
        offset: usize,
        window: &mut Window,
        cx: &mut App,
    ) -> Task<Result<Vec<LocationLink>>>;
}

/// Implementation provider for `textDocument/implementation`.
///
/// https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#textDocument_implementation
pub trait ImplementationProvider {
    /// textDocument/implementation
    fn implementations(
        &self,
        text: &Rope,
        offset: usize,
        window: &mut Window,
        cx: &mut App,
    ) -> Task<Result<Vec<LocationLink>>>;
}

/// Declaration provider for `textDocument/declaration`.
///
/// https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#textDocument_declaration
pub trait DeclarationProvider {
    /// textDocument/declaration
    fn declarations(
        &self,
        text: &Rope,
        offset: usize,
        window: &mut Window,
        cx: &mut App,
    ) -> Task<Result<Vec<LocationLink>>>;
}

impl InputBaseState<EditorMode> {
    pub(crate) fn on_action_go_to_type_definition(
        &mut self,
        _: &GoToTypeDefinition,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(provider) = self.extras.lsp.type_definition_provider.clone() else {
            return;
        };
        let task = provider.type_definitions(&self.text, self.cursor(), window, cx);
        self.navigate_when_resolved("Type Definitions", task, window, cx);
    }

    pub(crate) fn on_action_go_to_implementation(
        &mut self,
        _: &GoToImplementation,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(provider) = self.extras.lsp.implementation_provider.clone() else {
            return;
        };
        let task = provider.implementations(&self.text, self.cursor(), window, cx);
        self.navigate_when_resolved("Implementations", task, window, cx);
    }

    pub(crate) fn on_action_go_to_declaration(
        &mut self,
        _: &GoToDeclaration,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(provider) = self.extras.lsp.declaration_provider.clone() else {
            return;
        };
        let task = provider.declarations(&self.text, self.cursor(), window, cx);
        self.navigate_when_resolved("Declarations", task, window, cx);
    }

    /// Fetch definitions for the cursor and navigate. Backs the
    /// keyboard-invoked GoToDefinition action; Cmd-click keeps its own
    /// hover-cache path.
    pub(crate) fn go_to_definition_at_cursor(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(provider) = self.extras.lsp.definition_provider.clone() else {
            return false;
        };
        let task = provider.definitions(&self.text, self.cursor(), window, cx);
        self.navigate_when_resolved("Definitions", task, window, cx);
        true
    }

    /// Await a goto response and act on it: one target jumps directly,
    /// several open the locations picker.
    fn navigate_when_resolved(
        &mut self,
        title: &'static str,
        task: Task<Result<Vec<LocationLink>>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let version = self.document_version;
        self.extras.lsp._goto_task = cx.spawn_in(window, async move |editor, cx| {
            let Ok(links) = task.await else {
                return;
            };
            editor
                .update_in(cx, |editor, window, cx| {
                    if editor.document_version != version {
                        return;
                    }
                    editor.navigate_to_links(title, &links, window, cx);
                })
                .ok();
        });
    }

    pub(crate) fn navigate_to_links(
        &mut self,
        title: impl Into<SharedString>,
        links: &[LocationLink],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match links {
            [] => {}
            [only] => self.go_to_definition(only, window, cx),
            _ => {
                let locations: Vec<Location> = links
                    .iter()
                    .map(|link| Location::new(link.target_uri.clone(), link.target_selection_range))
                    .collect();
                self.present_locations(title, &locations, cx);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_support::build_editor;
    use super::*;
    use gpui::TestAppContext;
    use lsp_types::Position;
    use std::rc::Rc;

    struct CannedLinks(Vec<LocationLink>);

    impl ImplementationProvider for CannedLinks {
        fn implementations(
            &self,
            _: &Rope,
            _: usize,
            _: &mut Window,
            _: &mut App,
        ) -> Task<Result<Vec<LocationLink>>> {
            Task::ready(Ok(self.0.clone()))
        }
    }

    fn link(uri: &str, line: u32) -> LocationLink {
        let range = lsp_types::Range::new(Position::new(line, 0), Position::new(line, 4));
        LocationLink {
            origin_selection_range: None,
            target_uri: uri.parse().unwrap(),
            target_range: range,
            target_selection_range: range,
        }
    }

    #[gpui::test]
    fn goto_jumps_directly_or_opens_the_picker(cx: &mut TestAppContext) {
        let (editor, mut cx) = build_editor(cx);

        // A single result jumps straight to it.
        cx.update(|window, cx| {
            editor.update(cx, |editor, cx| {
                editor.set_value("one\ntwo\nthree", window, cx);
                editor
                    .extras
                    .lsp
                    .set_document_uri("file:///doc.go".parse().unwrap());
                editor.extras.lsp.implementation_provider =
                    Some(Rc::new(CannedLinks(vec![link("file:///doc.go", 2)])));
            });
        });
        cx.update(|window, cx| {
            editor.update(cx, |editor, cx| {
                editor.on_action_go_to_implementation(&GoToImplementation, window, cx);
            });
        });
        cx.run_until_parked();
        cx.update(|_, cx| {
            let editor = editor.read(cx);
            assert_eq!(editor.selected_range.start, "one\ntwo\n".len());
            assert!(!editor.locations_picker_state().open);
        });

        // Several results open the picker instead of guessing.
        cx.update(|window, cx| {
            editor.update(cx, |editor, cx| {
                editor.extras.lsp.implementation_provider = Some(Rc::new(CannedLinks(vec![
                    link("file:///doc.go", 0),
                    link("file:///other.go", 1),
                ])));
                editor.on_action_go_to_implementation(&GoToImplementation, window, cx);
            });
        });
        cx.run_until_parked();
        cx.update(|_, cx| {
            let picker = editor.read(cx).locations_picker_state();
            assert!(picker.open);
            assert_eq!(picker.title.as_ref(), "Implementations");
            assert_eq!(picker.items.len(), 2);
        });
    }
}
