use anyhow::Result;
use gpui::{App, Context, SharedString, Task, Window};
use lsp_types::{DocumentSymbol, DocumentSymbolResponse, SymbolKind};
use ropey::Rope;

use crate::input::{EditorMode, InputBaseState, RopeExt, ToggleDocumentSymbols};

use super::PickerLocation;

/// Document symbol provider: the document's outline.
///
/// https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#textDocument_documentSymbol
pub trait DocumentSymbolProvider {
    /// Fetches the document's symbols, either the flat or the
    /// hierarchical shape.
    ///
    /// textDocument/documentSymbol
    ///
    /// https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#textDocument_documentSymbol
    fn document_symbols(
        &self,
        text: &Rope,
        window: &mut Window,
        cx: &mut App,
    ) -> Task<Result<Option<DocumentSymbolResponse>>>;
}

fn symbol_kind_label(kind: SymbolKind) -> &'static str {
    match kind {
        SymbolKind::FILE => "file",
        SymbolKind::MODULE => "module",
        SymbolKind::NAMESPACE => "namespace",
        SymbolKind::PACKAGE => "package",
        SymbolKind::CLASS => "class",
        SymbolKind::METHOD => "method",
        SymbolKind::PROPERTY => "property",
        SymbolKind::FIELD => "field",
        SymbolKind::CONSTRUCTOR => "constructor",
        SymbolKind::ENUM => "enum",
        SymbolKind::INTERFACE => "interface",
        SymbolKind::FUNCTION => "function",
        SymbolKind::VARIABLE => "variable",
        SymbolKind::CONSTANT => "constant",
        SymbolKind::STRING => "string",
        SymbolKind::NUMBER => "number",
        SymbolKind::BOOLEAN => "boolean",
        SymbolKind::ARRAY => "array",
        SymbolKind::OBJECT => "object",
        SymbolKind::KEY => "key",
        SymbolKind::NULL => "null",
        SymbolKind::ENUM_MEMBER => "enum member",
        SymbolKind::STRUCT => "struct",
        SymbolKind::EVENT => "event",
        SymbolKind::OPERATOR => "operator",
        SymbolKind::TYPE_PARAMETER => "type parameter",
        _ => "symbol",
    }
}

impl InputBaseState<EditorMode> {
    pub(crate) fn on_action_toggle_document_symbols(
        &mut self,
        _: &ToggleDocumentSymbols,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.extras.locations_picker.open {
            self.dismiss_locations_picker(cx);
            return;
        }
        let Some(provider) = self.extras.lsp.document_symbol_provider.clone() else {
            return;
        };

        let version = self.document_version;
        let task = provider.document_symbols(&self.text, window, cx);
        self.extras.lsp._document_symbols_task = cx.spawn_in(window, async move |editor, cx| {
            let Ok(Some(response)) = task.await else {
                return;
            };
            editor
                .update(cx, |editor, cx| {
                    if editor.document_version != version {
                        return;
                    }
                    let items = editor.outline_items(&response);
                    editor.present_picker_locations("Symbols", items, cx);
                })
                .ok();
        });
    }

    /// Flatten a symbols response into picker rows. The hierarchical shape
    /// keeps document order and shows nesting as indentation; both shapes
    /// jump to the symbol's selection range (its name), not its full body.
    fn outline_items(&self, response: &DocumentSymbolResponse) -> Vec<PickerLocation> {
        let mut items = vec![];
        match response {
            DocumentSymbolResponse::Flat(symbols) => {
                for symbol in symbols {
                    items.push(self.outline_item(
                        &symbol.name,
                        symbol.kind,
                        symbol.location.range,
                        0,
                    ));
                }
            }
            DocumentSymbolResponse::Nested(symbols) => {
                self.collect_nested(symbols, 0, &mut items);
            }
        }
        items
    }

    fn collect_nested(
        &self,
        symbols: &[DocumentSymbol],
        depth: usize,
        items: &mut Vec<PickerLocation>,
    ) {
        for symbol in symbols {
            items.push(self.outline_item(&symbol.name, symbol.kind, symbol.selection_range, depth));
            if let Some(children) = &symbol.children {
                self.collect_nested(children, depth + 1, items);
            }
        }
    }

    fn outline_item(
        &self,
        name: &str,
        kind: SymbolKind,
        range: lsp_types::Range,
        depth: usize,
    ) -> PickerLocation {
        let start = self.text.position_to_offset(&range.start);
        let end = self.text.position_to_offset(&range.end);
        PickerLocation {
            uri: None,
            range,
            offset_range: Some(start..end),
            preview: SharedString::from(format!(
                "{}{name}  ·  {}",
                "  ".repeat(depth),
                symbol_kind_label(kind),
            )),
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

    struct NestedSymbols;

    impl DocumentSymbolProvider for NestedSymbols {
        fn document_symbols(
            &self,
            _: &Rope,
            _: &mut Window,
            _: &mut App,
        ) -> Task<Result<Option<DocumentSymbolResponse>>> {
            let symbol = |name: &str, kind, line, children| {
                #[allow(deprecated)]
                DocumentSymbol {
                    name: name.to_string(),
                    detail: None,
                    kind,
                    tags: None,
                    deprecated: None,
                    range: lsp_types::Range::new(Position::new(line, 0), Position::new(line, 20)),
                    selection_range: lsp_types::Range::new(
                        Position::new(line, 5),
                        Position::new(line, 12),
                    ),
                    children,
                }
            };
            Task::ready(Ok(Some(DocumentSymbolResponse::Nested(vec![
                symbol(
                    "Greeter",
                    SymbolKind::STRUCT,
                    0,
                    Some(vec![symbol("Greet", SymbolKind::METHOD, 1, None)]),
                ),
                symbol("main", SymbolKind::FUNCTION, 2, None),
            ]))))
        }
    }

    #[gpui::test]
    fn nested_symbols_flatten_into_an_indented_outline(cx: &mut TestAppContext) {
        let (editor, mut cx) = build_editor(cx);

        cx.update(|window, cx| {
            editor.update(cx, |editor, cx| {
                editor.set_value(
                    "type Greeter struct {}\nfunc (g Greeter) Greet() {}\nfunc main() {}",
                    window,
                    cx,
                );
                editor.extras.lsp.document_symbol_provider = Some(Rc::new(NestedSymbols));
            });
        });

        cx.update(|window, cx| {
            editor.update(cx, |editor, cx| {
                editor.on_action_toggle_document_symbols(&ToggleDocumentSymbols, window, cx);
            });
        });
        cx.run_until_parked();

        cx.update(|window, cx| {
            editor.update(cx, |editor, cx| {
                let picker = editor.locations_picker_state().clone();
                assert!(picker.open);
                // Depth-first document order, children indented under
                // their parent.
                assert_eq!(picker.items.len(), 3);
                assert_eq!(picker.items[0].preview.as_ref(), "Greeter  ·  struct");
                assert_eq!(picker.items[1].preview.as_ref(), "  Greet  ·  method");
                assert_eq!(picker.items[2].preview.as_ref(), "main  ·  function");

                // Confirming jumps to the symbol's selection range, not its
                // whole body.
                let item = picker.items[1].clone();
                editor.confirm_picker_location(&item, window, cx);
                let line_start = "type Greeter struct {}\n".len();
                assert_eq!(editor.selected_range.start, line_start + 5);
            });
        });
    }
}
