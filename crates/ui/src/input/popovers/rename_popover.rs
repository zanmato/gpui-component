use gpui::{
    App, AppContext as _, Context, Empty, Entity, InteractiveElement as _, IntoElement,
    ParentElement, Pixels, Point, Render, Styled, Subscription, WeakEntity, Window, deferred, px,
};
use gpui_base::input::RenamePrompt;

use crate::{
    input::{self, EditorState, Input, InputEvent, InputState, popovers::editor_popover},
    v_flex,
};

const WIDTH: Pixels = px(280.);

/// The rename prompt: a small single-line input over the editor,
/// prefilled with the symbol being renamed. Unlike the menus it takes
/// focus; Enter confirms and Escape hands focus back.
pub struct RenamePopover {
    editor: WeakEntity<EditorState>,
    input: Entity<InputState>,
    open: bool,

    _subscriptions: Vec<Subscription>,
}

impl RenamePopover {
    /// NOTE: This element should not be created from EditorState::new,
    /// unless that will stack overflow.
    pub(crate) fn new(
        editor: Entity<EditorState>,
        window: &mut Window,
        cx: &mut App,
    ) -> Entity<Self> {
        cx.new(|cx| {
            let input = cx.new(|cx| InputState::new(window, cx));
            let _subscriptions = vec![cx.subscribe_in(
                &input,
                window,
                |this: &mut Self, _, event: &InputEvent, window, cx| {
                    if let InputEvent::PressEnter { .. } = event {
                        this.commit(window, cx);
                    }
                },
            )];

            Self {
                editor: editor.downgrade(),
                input,
                open: false,
                _subscriptions,
            }
        })
    }

    pub(crate) fn show(
        &mut self,
        prompt: &RenamePrompt,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open = true;
        self.input.update(cx, |input, cx| {
            input.set_value(prompt.placeholder.clone(), window, cx);
            input.select_all(window, cx);
            input.focus(window, cx);
        });
        cx.notify();
    }

    pub(crate) fn hide(&mut self, cx: &mut Context<Self>) {
        self.open = false;
        cx.notify();
    }

    fn commit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let new_name = self.input.read(cx).value().to_string();
        self.open = false;
        let editor = self.editor.clone();
        cx.spawn_in(window, async move |_, cx| {
            editor.update_in(cx, |editor, window, cx| {
                editor.commit_rename(&new_name, window, cx);
                editor.focus(window, cx);
            })
        })
        .detach();
        cx.notify();
    }

    fn cancel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open = false;
        let editor = self.editor.clone();
        cx.spawn_in(window, async move |_, cx| {
            editor.update_in(cx, |editor, window, cx| {
                editor.cancel_rename(cx);
                editor.focus(window, cx);
            })
        })
        .detach();
        cx.notify();
    }

    fn origin(&self, cx: &App) -> Option<Point<Pixels>> {
        let editor = self.editor.upgrade()?;
        let editor = editor.read(cx);
        let (cursor_bounds, line_height) = editor.cursor_layout()?;
        let scroll_origin = editor.scroll_offset();

        Some(
            scroll_origin + cursor_bounds.origin - editor.input_bounds().origin
                + Point::new(-px(4.), line_height + px(4.)),
        )
    }
}

impl Render for RenamePopover {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.open {
            return Empty.into_any_element();
        }
        let Some(pos) = self.origin(cx) else {
            return Empty.into_any_element();
        };

        deferred(
            editor_popover("rename-popover", cx)
                .absolute()
                .left(pos.x)
                .top(pos.y)
                .w(WIDTH)
                .on_action(cx.listener(|this, _: &input::Escape, window, cx| {
                    this.cancel(window, cx);
                }))
                .child(
                    v_flex().p_1().child(
                        Input::new(&self.input)
                            .text_size(px(12.))
                            .w(WIDTH - px(16.)),
                    ),
                )
                .on_mouse_down_out(cx.listener(|this, _, window, cx| {
                    this.cancel(window, cx);
                })),
        )
        .into_any_element()
    }
}
