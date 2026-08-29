---
title: Editor
description: Source-code editor with syntax highlighting, gutter, folding, and decorations.
---

# Editor

`Editor` is the styled source-code control. Use [`Input`](./input.md) for
single-line values and [`Textarea`](./textarea.md) for ordinary multi-line text.

## Import

```rust
use gpui_component::input::{Editor, EditorState, TabSize};
```

## Basic usage

```rust
let editor = cx.new(|cx| {
    EditorState::new(window, cx)
        .language("rust")
        .line_number(true)
        .folding(true)
        .tab_size(TabSize {
            tab_size: 4,
            hard_tabs: false,
        })
        .default_value("fn main() {\n    println!(\"Hello\");\n}")
});

Editor::new(&editor).h(px(320.))
```

The language set via `.language()` selects syntax highlighting. Enable the
matching Cargo feature, such as `tree-sitter-rust` or `tree-sitter-markdown`;
use `tree-sitter-languages` to bundle all built-in grammars.

## Editor options

```rust
let editor = cx.new(|cx| {
    EditorState::new(window, cx)
        .language("json")
        .line_number(true)
        .folding(true)
        .show_whitespaces(true)
        .default_value(source)
});
```

## Decorations

```rust
let decorations = editor.update(cx, |state, cx| {
    state.create_decorations_collection(initial_decorations, cx)
});
```

Keep the returned `TextDecorationCollection` alive while the decorations are
needed. Its ranges follow subsequent text edits.

## Language features (LSP)

The editor speaks the Language Server Protocol through provider traits.
Install providers on `EditorState::lsp_mut()` and the matching interactions
light up; every slot left empty simply stays inert:

```rust
editor.update(cx, |state, cx| {
    let lsp = state.lsp_mut();
    lsp.set_document_uri("file:///src/main.go".parse().unwrap());
    lsp.completion_provider = Some(my_completions);
    lsp.hover_provider = Some(my_hover);
    lsp.formatting_provider = Some(my_formatter);
    // …definition, references, rename, signature help, document
    // symbols, document highlights, inlay hints, code actions,
    // semantic tokens, document colors, on-type formatting.
});
```

Each provider is one trait with the shape of its LSP request —
`CompletionProvider`, `HoverProvider`, `RenameProvider` with prepare
support, `FormattingProvider` for document and range formatting,
`InlayHintProvider`, and so on. Positions convert between byte offsets and
UTF-16 `lsp_types::Position` via `RopeExt`, matching the protocol's
mandatory encoding. Async responses are version-guarded: a reply that
resolves against an already-edited document is dropped.

The built-in keybindings: completion as you type, hover on rest, F12 goes
to definition, Shift-F12 lists references, Cmd-F12 goes to implementation,
Cmd-Shift-O opens the document outline, F2 renames, Shift-Alt-F formats,
Cmd-. opens code actions, and Cmd-Shift-Space toggles signature help.
Workspace edits — rename, code actions, `workspace/applyEdit` — apply
atomically through `apply_workspace_edit`: one undo step, cursor preserved.

A complete client — process spawn, JSON-RPC framing, capability
negotiation, document sync — lives in the
[`editor_lsp` example](https://github.com/longbridge/gpui-component/tree/main/crates/story/examples/editor_lsp),
which wires every provider to a real gopls and is the reference for
connecting your own server.

## Value and events

```rust
let source = editor.read(cx).value();

editor.update(cx, |state, cx| {
    state.set_value(new_source, window, cx);
});

cx.subscribe(&editor, |this, state, event: &InputEvent, cx| {
    if matches!(event, InputEvent::Change) {
        this.source = state.read(cx).value();
        cx.notify();
    }
});
```

## Font

The editor paints its code in the theme's monospace font — `mono_font_family` at
`mono_font_size` — with rows 1.5 times the font size. That is only the default:
a text style set on the editor refines over it, and the gutter and row height
follow the size.

```rust
Editor::new(&editor).text_sm()

Editor::new(&editor)
    .font_family("JetBrains Mono")
    .text_size(px(15.))
```

These are the ordinary [`Styled`](https://docs.rs/gpui/latest/gpui/trait.Styled.html)
methods every element has, so `font_weight` and `line_height` work the same way.

## Appearance

```rust
Editor::new(&editor)
    .h(px(480.))
    .bordered(true)
    .disabled(false)
    .readonly(false)
    .aria_label("Rust source")
```

Use `readonly` to preview a file without allowing changes. Unlike `disabled`,
a read-only editor keeps the normal appearance and still can be focused,
selected, copied and searched, it only rejects the changes made by the user.
The programmatic APIs such as `set_value` keep working.

```rust
Editor::new(&editor).readonly(true)
```

Editor focus does not add the single-line Input focus-border treatment. The
gutter, current-line background, and scrollbars are painted as one aligned
editor surface.

Input-only adornments such as `prefix`, `suffix`, mask toggle, and clear button
are intentionally absent. Compose toolbars and actions around `Editor`.
