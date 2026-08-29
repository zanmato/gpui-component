---
title: Editor
description: 支持语法高亮、行号、折叠和文本装饰的源代码编辑器。
---

# Editor

`Editor` 用于编辑源代码。单行输入请使用 [Input](./input.md)，普通多行文本请使用 [Textarea](./textarea.md)。

## 导入

```rust
use gpui_component::input::{Editor, EditorState, TabSize};
```

## 基础用法

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

使用 `.language()` 指定语法高亮语言。应用需要启用对应的 Cargo feature，例如 `tree-sitter-rust` 或 `tree-sitter-markdown`；也可以使用 `tree-sitter-languages` 包含全部内置语法。

## 编辑器选项

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

## 文本装饰

```rust
let decorations = editor.update(cx, |state, cx| {
    state.create_decorations_collection(initial_decorations, cx)
});
```

需要装饰存在多久，就应将返回的 `TextDecorationCollection` 保留多久；文本修改后，其 range 会自动跟随内容变化。

## 语言功能（LSP）

Editor 通过 provider trait 支持 Language Server Protocol。在
`EditorState::lsp_mut()` 上安装 provider，对应的交互即会生效；未安装的槽位保持无效状态：

```rust
editor.update(cx, |state, cx| {
    let lsp = state.lsp_mut();
    lsp.set_document_uri("file:///src/main.go".parse().unwrap());
    lsp.completion_provider = Some(my_completions);
    lsp.hover_provider = Some(my_hover);
    lsp.formatting_provider = Some(my_formatter);
    // …definition、references、rename、signature help、document
    // symbols、document highlights、inlay hints、code actions、
    // semantic tokens、document colors、on-type formatting。
});
```

每个 provider 都是一个与其 LSP 请求同构的 trait——`CompletionProvider`、
`HoverProvider`、支持 prepare 的 `RenameProvider`、覆盖整篇文档与选区的
`FormattingProvider`、`InlayHintProvider` 等。字节偏移与 UTF-16 的
`lsp_types::Position` 之间通过 `RopeExt` 转换，符合协议规定的编码。异步响应带版本保护：针对已被编辑文档的过期响应会被丢弃。

内置按键：输入时自动补全、悬停显示 hover，F12 跳转定义，Shift-F12
列出引用，Cmd-F12 跳转实现，Cmd-Shift-O 打开文档大纲，F2 重命名，
Shift-Alt-F 格式化，Cmd-. 打开 code actions，Cmd-Shift-Space 切换
signature help。Workspace edit——rename、code actions、
`workspace/applyEdit`——通过 `apply_workspace_edit` 原子应用：一步撤销，光标位置保持不变。

完整的客户端实现——进程启动、JSON-RPC 帧解析、能力协商、文档同步——见
[`editor_lsp` 示例](https://github.com/longbridge/gpui-component/tree/main/crates/story/examples/editor_lsp)，它将全部
provider 接到真实的 gopls 上，是接入自有语言服务器的参考实现。

## 值与事件

```rust
let source = editor.read(cx).value();

editor.update(cx, |state, cx| {
    state.set_value(new_source, window, cx);
});
```

`EditorState` 会发出 `InputEvent::Change`、`Focus` 和 `Blur` 等事件。

## 字体

Editor 默认使用主题中的等宽字体 —— `mono_font_family` 和 `mono_font_size`，行高为字号的
1.5 倍。这只是默认值：在 Editor 上设置的文本样式会覆盖它，gutter 和行高都跟随字号变化。

```rust
Editor::new(&editor).text_sm()

Editor::new(&editor)
    .font_family("JetBrains Mono")
    .text_size(px(15.))
```

这些就是所有元素都有的 [`Styled`](https://docs.rs/gpui/latest/gpui/trait.Styled.html)
方法，`font_weight`、`line_height` 用法相同。

## 外观

```rust
Editor::new(&editor)
    .h(px(480.))
    .bordered(true)
    .disabled(false)
    .readonly(false)
    .aria_label("Rust 源代码")
```

预览文件但不允许修改时使用 `readonly`。与 `disabled` 不同，只读编辑器保持正常外观，仍然可以聚焦、选中、复制和搜索，只是拒绝用户对内容的修改。`set_value` 等程序调用不受影响。

```rust
Editor::new(&editor).readonly(true)
```

Editor 聚焦时不会应用单行 Input 的焦点边框效果。gutter、当前行背景和滚动条会作为同一个编辑器表面对齐绘制。

前后缀、密码显示切换和清除按钮只属于单行 Input。Editor 的工具栏和操作按钮应组合在组件外部。
