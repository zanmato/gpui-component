use gpui::{Context, Window};
use lsp_types::{DocumentChangeOperation, DocumentChanges, OneOf, TextEdit, WorkspaceEdit};
use ropey::Rope;
use std::ops::Range;

use crate::input::undo_manager::EditIntent;
use crate::input::{EditorMode, InputBaseState, RopeExt};

/// A [`TextEdit`] resolved against the current document: a byte range and its
/// replacement text.
type ResolvedEdit = (Range<usize>, String);

/// Resolve `edits` to byte ranges against `text`, sorted ascending by start.
///
/// Returns `None` when two edits overlap: the LSP specification requires the
/// ranges of a single response to be non-overlapping, and applying an
/// overlapping batch would corrupt the document.
fn resolve_edits(text: &Rope, edits: &[TextEdit]) -> Option<Vec<ResolvedEdit>> {
    let mut resolved: Vec<ResolvedEdit> = edits
        .iter()
        .map(|edit| {
            let start = text.position_to_offset(&edit.range.start);
            let end = text.position_to_offset(&edit.range.end).max(start);
            (start..end, edit.new_text.clone())
        })
        .collect();
    resolved.sort_by_key(|(range, _)| (range.start, range.end));

    for pair in resolved.windows(2) {
        if pair[0].0.end > pair[1].0.start {
            return None;
        }
    }

    Some(resolved)
}

/// Merge non-overlapping sorted edits into one replacement covering
/// `first.start..last.end`, so a whole batch lands as a single edit: one
/// history entry, one undo step, and no transient intermediate documents.
fn merge_edits(text: &Rope, resolved: &[ResolvedEdit]) -> (Range<usize>, String) {
    let covering = resolved[0].0.start..resolved[resolved.len() - 1].0.end;
    let mut replacement = String::new();
    let mut last_end = covering.start;
    for (range, new_text) in resolved {
        replacement.push_str(&text.slice(last_end..range.start).to_string());
        replacement.push_str(new_text);
        last_end = range.end;
    }
    (covering, replacement)
}

/// Map a byte offset of the old document to the corresponding offset after
/// applying `resolved` (sorted, non-overlapping). An offset inside a replaced
/// range maps to the end of that edit's new text.
fn map_offset_through_edits(offset: usize, resolved: &[ResolvedEdit]) -> usize {
    let mut delta = 0isize;
    for (range, new_text) in resolved {
        if range.end <= offset {
            delta += new_text.len() as isize - range.len() as isize;
        } else if range.start < offset {
            return (range.start as isize + delta) as usize + new_text.len();
        } else {
            break;
        }
    }
    (offset as isize + delta).max(0) as usize
}

impl InputBaseState<EditorMode> {
    /// Apply a batch of [`TextEdit`]s to the current document as one atomic
    /// edit: ranges are resolved against the document *before* any edit is
    /// applied, the batch lands as a single undo step, and the cursor is
    /// mapped through the edits instead of jumping to the changed text.
    ///
    /// Returns `false` without touching the document when the edits overlap.
    pub fn apply_text_edits(
        &mut self,
        edits: &[TextEdit],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if edits.is_empty() {
            return true;
        }

        let Some(resolved) = resolve_edits(&self.text, edits) else {
            return false;
        };

        let cursor = self.cursor();
        let (covering, replacement) = merge_edits(&self.text, &resolved);
        let range_utf16 = self.range_to_utf16(&covering);
        self.undo_manager.pending_intent = Some(EditIntent::Atomic);
        self.replace_text_in_range_silent(Some(range_utf16), &replacement, window, cx);

        let cursor = map_offset_through_edits(cursor, &resolved).min(self.text.len());
        self.selected_range = (cursor..cursor).into();
        true
    }

    /// Apply the parts of a [`WorkspaceEdit`] that target the current
    /// document, following the `workspace/applyEdit` request.
    ///
    /// The current document is identified by [`super::Lsp::document_uri`];
    /// when no URI is configured every text edit is treated as targeting this
    /// editor, since it hosts the only document. Edits addressed to other
    /// documents and file operations (create/rename/delete) are skipped — a
    /// host that needs them handles the workspace edit itself before handing
    /// the remainder to the editor.
    ///
    /// Returns `false` when the current document's edits could not be applied
    /// (overlapping ranges); suitable as the `applied` field of the
    /// `workspace/applyEdit` response.
    ///
    /// https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#workspace_applyEdit
    pub fn apply_workspace_edit(
        &mut self,
        workspace_edit: &WorkspaceEdit,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let document_uri = self.extras.lsp.document_uri.clone();
        let matches_document =
            |uri: &lsp_types::Uri| document_uri.as_ref().is_none_or(|doc| doc == uri);

        let mut edits: Vec<TextEdit> = vec![];
        if let Some(changes) = &workspace_edit.changes {
            for (uri, text_edits) in changes {
                if matches_document(uri) {
                    edits.extend(text_edits.iter().cloned());
                }
            }
        }
        if let Some(document_changes) = &workspace_edit.document_changes {
            let document_edits: Vec<_> = match document_changes {
                DocumentChanges::Edits(document_edits) => document_edits.iter().collect(),
                DocumentChanges::Operations(operations) => operations
                    .iter()
                    .filter_map(|operation| match operation {
                        DocumentChangeOperation::Edit(edit) => Some(edit),
                        DocumentChangeOperation::Op(_) => None,
                    })
                    .collect(),
            };
            for document_edit in document_edits {
                if !matches_document(&document_edit.text_document.uri) {
                    continue;
                }
                edits.extend(document_edit.edits.iter().map(|edit| match edit {
                    OneOf::Left(text_edit) => text_edit.clone(),
                    OneOf::Right(annotated) => annotated.text_edit.clone(),
                }));
            }
        }

        self.apply_text_edits(&edits, window, cx)
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_support::build_editor;
    use super::*;
    use crate::input::Undo;
    use gpui::TestAppContext;
    use lsp_types::{Position, TextDocumentEdit, Uri};

    fn edit(start: (u32, u32), end: (u32, u32), new_text: &str) -> TextEdit {
        TextEdit::new(
            lsp_types::Range::new(Position::new(start.0, start.1), Position::new(end.0, end.1)),
            new_text.to_string(),
        )
    }

    #[test]
    fn test_resolve_edits_sorts_and_rejects_overlaps() {
        let text = Rope::from("aaa bbb ccc");
        let sorted = resolve_edits(
            &text,
            &[edit((0, 8), (0, 11), "YYY"), edit((0, 0), (0, 3), "X")],
        )
        .unwrap();
        assert_eq!(sorted, vec![(0..3, "X".into()), (8..11, "YYY".into())]);

        assert!(
            resolve_edits(
                &text,
                &[edit((0, 0), (0, 5), "X"), edit((0, 4), (0, 7), "Y")]
            )
            .is_none()
        );
    }

    #[test]
    fn test_merge_edits_builds_one_covering_replacement() {
        let text = Rope::from("aaa bbb ccc");
        let resolved = resolve_edits(
            &text,
            &[edit((0, 8), (0, 11), "YYY"), edit((0, 0), (0, 3), "X")],
        )
        .unwrap();
        let (covering, replacement) = merge_edits(&text, &resolved);
        assert_eq!(covering, 0..11);
        assert_eq!(replacement, "X bbb YYY");
    }

    #[test]
    fn test_map_offset_through_edits() {
        // "aaa bbb ccc" with aaa -> X (shrinks 2) and ccc -> YYYY (grows 1).
        let resolved = vec![(0..3usize, "X".to_string()), (8..11, "YYYY".to_string())];
        // Before all edits.
        assert_eq!(map_offset_through_edits(0, &resolved), 0);
        // Inside the first edit: end of its replacement.
        assert_eq!(map_offset_through_edits(2, &resolved), 1);
        // Between the edits: shifted by the first delta only.
        assert_eq!(map_offset_through_edits(5, &resolved), 3);
        // After all edits.
        assert_eq!(map_offset_through_edits(11, &resolved), 10);
    }

    #[gpui::test]
    fn workspace_edit_applies_atomically_and_undoes_in_one_step(cx: &mut TestAppContext) {
        let (editor, mut cx) = build_editor(cx);

        cx.update(|window, cx| {
            editor.update(cx, |editor, cx| {
                editor.set_value("aaa bbb ccc", window, cx);
                // Cursor inside "bbb", between the two edits.
                editor.selected_range = (5..5).into();
            });
        });

        let workspace_edit = WorkspaceEdit {
            changes: Some(
                [(
                    "file:///doc.txt".parse::<Uri>().unwrap(),
                    vec![edit((0, 8), (0, 11), "YYY"), edit((0, 0), (0, 3), "X")],
                )]
                .into(),
            ),
            ..Default::default()
        };
        cx.update(|window, cx| {
            editor.update(cx, |editor, cx| {
                assert!(editor.apply_workspace_edit(&workspace_edit, window, cx));
                assert_eq!(editor.text().to_string(), "X bbb YYY");
                // "aaa" shrank by two bytes, so the cursor inside "bbb" moved
                // with its word instead of jumping to the changed text.
                assert_eq!(editor.cursor(), 3);
            });
        });

        cx.update(|window, cx| {
            editor.update(cx, |editor, cx| {
                editor.undo(&Undo, window, cx);
                assert_eq!(editor.text().to_string(), "aaa bbb ccc");
            });
        });
    }

    #[gpui::test]
    fn workspace_edit_skips_other_documents(cx: &mut TestAppContext) {
        let (editor, mut cx) = build_editor(cx);

        cx.update(|window, cx| {
            editor.update(cx, |editor, cx| {
                editor.set_value("aaa", window, cx);
                editor
                    .extras
                    .lsp
                    .set_document_uri("file:///mine.txt".parse().unwrap());
            });
        });

        let foreign_uri: Uri = "file:///other.txt".parse().unwrap();
        let workspace_edit = WorkspaceEdit {
            document_changes: Some(DocumentChanges::Edits(vec![TextDocumentEdit {
                text_document: lsp_types::OptionalVersionedTextDocumentIdentifier {
                    uri: foreign_uri,
                    version: None,
                },
                edits: vec![OneOf::Left(edit((0, 0), (0, 3), "XXX"))],
            }])),
            ..Default::default()
        };
        cx.update(|window, cx| {
            editor.update(cx, |editor, cx| {
                assert!(editor.apply_workspace_edit(&workspace_edit, window, cx));
                assert_eq!(editor.text().to_string(), "aaa");
            });
        });
    }

    #[gpui::test]
    fn overlapping_edits_leave_the_document_untouched(cx: &mut TestAppContext) {
        let (editor, mut cx) = build_editor(cx);

        cx.update(|window, cx| {
            editor.update(cx, |editor, cx| {
                editor.set_value("aaa bbb", window, cx);
            });
        });

        cx.update(|window, cx| {
            editor.update(cx, |editor, cx| {
                let edits = [edit((0, 0), (0, 5), "X"), edit((0, 4), (0, 7), "Y")];
                assert!(!editor.apply_text_edits(&edits, window, cx));
                assert_eq!(editor.text().to_string(), "aaa bbb");
            });
        });
    }
}
