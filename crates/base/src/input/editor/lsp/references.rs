use anyhow::Result;
use gpui::{App, Context, SharedString, Task, Window};
use lsp_types::Location;
use ropey::Rope;
use std::ops::Range;

use crate::input::{EditorMode, FindAllReferences, InputBaseState, RopeExt};

/// References provider: every location that mentions the symbol under the
/// cursor.
///
/// https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#textDocument_references
pub trait ReferencesProvider {
    /// Fetches the references for the symbol at the given byte offset.
    ///
    /// textDocument/references
    ///
    /// https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#textDocument_references
    fn references(
        &self,
        text: &Rope,
        offset: usize,
        include_declaration: bool,
        window: &mut Window,
        cx: &mut App,
    ) -> Task<Result<Vec<Location>>>;
}

/// One row of the locations picker.
#[derive(Clone, Debug)]
pub struct PickerLocation {
    /// The document the location is in; `None` for the current document.
    pub uri: Option<lsp_types::Uri>,
    /// The location's range as the server reported it.
    pub range: lsp_types::Range,
    /// The resolved byte range, when the location is in this document.
    pub offset_range: Option<Range<usize>>,
    /// A one-line preview: the trimmed source line for this document,
    /// or the URI for another one.
    pub preview: SharedString,
}

/// The generic locations picker: a list of places to jump to, fed by
/// find-all-references and reused by the other multi-location features.
/// Mirrored by the renderer through a revision check.
#[derive(Clone, Debug, Default)]
pub struct LocationsPickerState {
    pub open: bool,
    /// What the list shows, e.g. "References".
    pub title: SharedString,
    pub items: Vec<PickerLocation>,
    revision: u64,
}

impl LocationsPickerState {
    /// Bumped whenever the content changes. See
    /// [`super::CompletionMenuState::revision`].
    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub(super) fn bump(&mut self) {
        self.revision = self.revision.wrapping_add(1);
    }
}

impl InputBaseState<EditorMode> {
    /// The locations picker state.
    #[doc(hidden)]
    pub fn locations_picker_state(&self) -> &LocationsPickerState {
        &self.extras.locations_picker
    }

    /// Open the locations picker over the given locations.
    pub fn present_locations(
        &mut self,
        title: impl Into<SharedString>,
        locations: &[Location],
        cx: &mut Context<Self>,
    ) {
        let items = self.picker_locations(locations);
        let picker = &mut self.extras.locations_picker;
        picker.title = title.into();
        picker.items = items;
        picker.open = !picker.items.is_empty();
        picker.bump();
        cx.notify();
    }

    pub fn dismiss_locations_picker(&mut self, cx: &mut Context<Self>) {
        if self.extras.locations_picker.open {
            self.extras.locations_picker.open = false;
            self.extras.locations_picker.bump();
            cx.notify();
        }
    }

    /// Jump to one picker row: in-document locations move the cursor,
    /// foreign documents go through the configured
    /// [`super::ShowDocumentHandler`].
    pub fn confirm_picker_location(
        &mut self,
        item: &PickerLocation,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.dismiss_locations_picker(cx);
        let link = lsp_types::LocationLink {
            origin_selection_range: None,
            target_uri: match &item.uri {
                Some(uri) => uri.clone(),
                None => match self.extras.lsp.document_uri.clone() {
                    Some(uri) => uri,
                    None => {
                        // No URI anywhere: it can only be this document.
                        let start = self.text.position_to_offset(&item.range.start);
                        let end = self.text.position_to_offset(&item.range.end);
                        self.move_to(start, None, cx);
                        self.select_to(end, cx);
                        return;
                    }
                },
            },
            target_range: item.range,
            target_selection_range: item.range,
        };
        self.go_to_definition(&link, window, cx);
    }

    pub(crate) fn on_action_find_all_references(
        &mut self,
        _: &FindAllReferences,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(provider) = self.extras.lsp.references_provider.clone() else {
            return;
        };

        let offset = self.cursor();
        let version = self.document_version;
        let task = provider.references(&self.text, offset, true, window, cx);
        self.extras.lsp._references_task = cx.spawn_in(window, async move |editor, cx| {
            let Ok(locations) = task.await else {
                return;
            };
            editor
                .update_in(cx, |editor, window, cx| {
                    if editor.document_version != version {
                        return;
                    }
                    match locations.as_slice() {
                        [] => {}
                        [only] => {
                            // A single result needs no picker.
                            let item = editor
                                .picker_locations(std::slice::from_ref(only))
                                .remove(0);
                            editor.confirm_picker_location(&item, window, cx);
                        }
                        _ => editor.present_locations("References", &locations, cx),
                    }
                })
                .ok();
        });
    }

    /// Build picker rows: locations in this document get a byte range and a
    /// source-line preview, others show their URI.
    fn picker_locations(&self, locations: &[Location]) -> Vec<PickerLocation> {
        let document_uri = self.extras.lsp.document_uri.clone();
        locations
            .iter()
            .map(|location| {
                let in_this_document = match &document_uri {
                    Some(uri) => *uri == location.uri,
                    // Without a configured URI the editor hosts the only
                    // known document.
                    None => true,
                };
                if in_this_document {
                    let start = self.text.position_to_offset(&location.range.start);
                    let end = self.text.position_to_offset(&location.range.end);
                    let line = self
                        .text
                        .slice_line(location.range.start.line as usize)
                        .to_string();
                    PickerLocation {
                        uri: None,
                        range: location.range,
                        offset_range: Some(start..end),
                        preview: SharedString::from(format!(
                            "{}: {}",
                            location.range.start.line + 1,
                            line.trim()
                        )),
                    }
                } else {
                    PickerLocation {
                        uri: Some(location.uri.clone()),
                        range: location.range,
                        offset_range: None,
                        preview: SharedString::from(format!(
                            "{}:{}",
                            location.uri.as_str(),
                            location.range.start.line + 1
                        )),
                    }
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_support::build_editor;
    use super::*;
    use gpui::TestAppContext;
    use lsp_types::{Position, Uri};
    use std::cell::RefCell;
    use std::rc::Rc;

    struct TwoReferences {
        include_declaration_seen: Rc<RefCell<Option<bool>>>,
    }

    impl ReferencesProvider for TwoReferences {
        fn references(
            &self,
            _: &Rope,
            _: usize,
            include_declaration: bool,
            _: &mut Window,
            _: &mut App,
        ) -> Task<Result<Vec<Location>>> {
            *self.include_declaration_seen.borrow_mut() = Some(include_declaration);
            let uri: Uri = "file:///workspace/main.go".parse().unwrap();
            Task::ready(Ok(vec![
                Location::new(
                    uri.clone(),
                    lsp_types::Range::new(Position::new(0, 0), Position::new(0, 5)),
                ),
                Location::new(
                    uri,
                    lsp_types::Range::new(Position::new(1, 8), Position::new(1, 13)),
                ),
                Location::new(
                    "file:///workspace/other.go".parse().unwrap(),
                    lsp_types::Range::new(Position::new(3, 0), Position::new(3, 5)),
                ),
            ]))
        }
    }

    #[gpui::test]
    fn find_all_references_opens_the_picker_and_jumps(cx: &mut TestAppContext) {
        let (editor, mut cx) = build_editor(cx);
        let include_declaration_seen: Rc<RefCell<Option<bool>>> = Rc::default();

        cx.update(|window, cx| {
            editor.update(cx, |editor, cx| {
                editor.set_value("hello world\n        hello again", window, cx);
                editor
                    .extras
                    .lsp
                    .set_document_uri("file:///workspace/main.go".parse().unwrap());
                editor.extras.lsp.references_provider = Some(Rc::new(TwoReferences {
                    include_declaration_seen: include_declaration_seen.clone(),
                }));
            });
        });

        cx.update(|window, cx| {
            editor.update(cx, |editor, cx| {
                editor.on_action_find_all_references(&FindAllReferences, window, cx);
            });
        });
        cx.run_until_parked();

        assert_eq!(*include_declaration_seen.borrow(), Some(true));
        cx.update(|window, cx| {
            editor.update(cx, |editor, cx| {
                let picker = editor.locations_picker_state().clone();
                assert!(picker.open);
                assert_eq!(picker.items.len(), 3);
                // In-document rows carry byte ranges and line previews in
                // server order; foreign rows show their URI.
                assert_eq!(picker.items[0].offset_range, Some(0..5));
                assert_eq!(picker.items[1].preview.as_ref(), "2: hello again");
                assert!(picker.items[2].uri.is_some());

                // Confirming an in-document row moves the cursor there and
                // closes the picker.
                let item = picker.items[1].clone();
                editor.confirm_picker_location(&item, window, cx);
                assert_eq!(editor.selected_range.start, "hello world\n        ".len());
                assert!(!editor.locations_picker_state().open);
            });
        });
    }
}
