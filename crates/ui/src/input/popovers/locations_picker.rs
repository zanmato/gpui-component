use std::rc::Rc;

use gpui::{
    Action, AnyElement, App, AppContext, Context, DismissEvent, Empty, Entity, EventEmitter,
    Half as _, InteractiveElement as _, IntoElement, ParentElement, Pixels, Point, Render,
    RenderOnce, SharedString, Styled, Subscription, WeakEntity, Window, deferred, div,
    prelude::FluentBuilder, px, relative,
};
pub(crate) use gpui_base::input::PickerLocation;

const MAX_MENU_WIDTH: Pixels = px(480.);
const MAX_MENU_HEIGHT: Pixels = px(320.);

use crate::{
    ActiveTheme, IndexPath, Selectable, actions,
    input::{self, EditorState, popovers::editor_popover},
    label::Label,
    list::{List, ListDelegate, ListEvent, ListState},
};

struct PickerDelegate {
    picker: Entity<LocationsPicker>,
    items: Vec<Rc<PickerLocation>>,
    selected_ix: usize,
}

impl PickerDelegate {
    fn set_items(&mut self, items: Vec<PickerLocation>) {
        self.items = items.into_iter().map(Rc::new).collect();
        self.selected_ix = 0;
    }

    fn selected_item(&self) -> Option<&Rc<PickerLocation>> {
        self.items.get(self.selected_ix)
    }
}

#[derive(IntoElement)]
struct PickerItem {
    ix: usize,
    item: Rc<PickerLocation>,
    children: Vec<AnyElement>,
    selected: bool,
}

impl PickerItem {
    fn new(ix: usize, item: Rc<PickerLocation>) -> Self {
        Self {
            ix,
            item,
            children: vec![],
            selected: false,
        }
    }
}

impl Selectable for PickerItem {
    fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    fn is_selected(&self) -> bool {
        self.selected
    }
}

impl ParentElement for PickerItem {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl RenderOnce for PickerItem {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let foreign = self.item.uri.is_some();

        div()
            .id(self.ix)
            .p_1()
            .text_xs()
            .line_height(relative(1.))
            .rounded(cx.theme().radius.half())
            .overflow_hidden()
            .text_ellipsis()
            .whitespace_nowrap()
            .when(foreign, |this| this.text_color(cx.theme().muted_foreground))
            .hover(|this| this.bg(cx.theme().accent.opacity(0.8)))
            .when(self.selected, |this| {
                this.bg(cx.theme().tokens.accent)
                    .text_color(cx.theme().accent_foreground)
            })
            .child(self.item.preview.clone())
            .children(self.children)
    }
}

impl EventEmitter<DismissEvent> for PickerDelegate {}

impl ListDelegate for PickerDelegate {
    type Item = PickerItem;

    fn items_count(&self, _: usize, _: &gpui::App) -> usize {
        self.items.len()
    }

    fn render_item(
        &mut self,
        ix: crate::IndexPath,
        _: &mut Window,
        _: &mut Context<ListState<Self>>,
    ) -> Option<Self::Item> {
        let item = self.items.get(ix.row)?;
        Some(PickerItem::new(ix.row, item.clone()))
    }

    fn set_selected_index(
        &mut self,
        ix: Option<crate::IndexPath>,
        _: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) {
        self.selected_ix = ix.map(|i| i.row).unwrap_or(0);
        cx.notify();
    }

    fn confirm(&mut self, _: bool, window: &mut Window, cx: &mut Context<ListState<Self>>) {
        let Some(item) = self.selected_item() else {
            return;
        };

        self.picker.update(cx, |this, cx| {
            this.select_item(&item, window, cx);
        });
    }
}

/// The locations picker: a jump list fed by find-all-references and the
/// other multi-location features.
pub struct LocationsPicker {
    state: WeakEntity<EditorState>,
    list: Entity<ListState<PickerDelegate>>,
    title: SharedString,
    open: bool,

    _subscriptions: Vec<Subscription>,
}

impl LocationsPicker {
    /// NOTE: This element should not be created from EditorState::new,
    /// unless that will stack overflow.
    pub(crate) fn new(
        state: Entity<EditorState>,
        window: &mut Window,
        cx: &mut App,
    ) -> Entity<Self> {
        cx.new(|cx| {
            let view = cx.entity();
            let delegate = PickerDelegate {
                picker: view,
                items: vec![],
                selected_ix: 0,
            };

            let list = cx.new(|cx| ListState::new(delegate, window, cx));

            let _subscriptions =
                vec![
                    cx.subscribe(&list, |this: &mut Self, _, ev: &ListEvent, cx| {
                        if let ListEvent::Confirm(_) = ev {
                            this.hide(cx);
                        }
                        cx.notify();
                    }),
                ];

            Self {
                state: state.downgrade(),
                list,
                title: SharedString::default(),
                open: false,
                _subscriptions,
            }
        })
    }

    fn select_item(&mut self, item: &PickerLocation, window: &mut Window, cx: &mut Context<Self>) {
        let state = self.state.clone();
        let item = item.clone();

        cx.spawn_in(window, async move |_, cx| {
            state.update_in(cx, |state, window, cx| {
                state.confirm_picker_location(&item, window, cx);
            })
        })
        .detach();

        self.hide(cx);
    }

    pub(crate) fn handle_action(
        &mut self,
        action: Box<dyn Action>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.open {
            return false;
        }

        cx.propagate();
        if input::Enter::is_primary(&*action) {
            self.on_action_enter(window, cx);
        } else if action.partial_eq(&input::Escape) {
            self.hide(cx);
        } else if action.partial_eq(&input::MoveUp) {
            self.list.update(cx, |this, cx| {
                this.on_action_select_prev(&actions::SelectUp, window, cx)
            });
        } else if action.partial_eq(&input::MoveDown) {
            self.list.update(cx, |this, cx| {
                this.on_action_select_next(&actions::SelectDown, window, cx)
            });
        } else {
            return false;
        }

        true
    }

    fn on_action_enter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(item) = self.list.read(cx).delegate().selected_item().cloned() else {
            return;
        };
        self.select_item(&item, window, cx);
    }

    pub(crate) fn hide(&mut self, cx: &mut Context<Self>) {
        self.open = false;
        let state = self.state.clone();
        cx.spawn(async move |_, cx| {
            let _ = state.update(cx, |state, cx| state.dismiss_locations_picker(cx));
        })
        .detach();
        cx.notify();
    }

    pub(crate) fn show(
        &mut self,
        title: SharedString,
        items: impl Into<Vec<PickerLocation>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.title = title;
        self.open = true;
        self.list.update(cx, |this, cx| {
            this.delegate_mut().set_items(items.into());
            this.set_selected_index(Some(IndexPath::new(0)), window, cx);
        });

        cx.notify();
    }

    fn origin(&self, cx: &App) -> Option<Point<Pixels>> {
        let state = self.state.upgrade()?;
        let state = state.read(cx);
        let (cursor_bounds, line_height) = state.cursor_layout()?;
        let scroll_origin = state.scroll_offset();

        Some(
            scroll_origin + cursor_bounds.origin - state.input_bounds().origin
                + Point::new(-px(4.), line_height + px(4.)),
        )
    }
}

impl Render for LocationsPicker {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.open {
            return Empty.into_any_element();
        }

        if self.list.read(cx).delegate().items.is_empty() {
            self.open = false;
            return Empty.into_any_element();
        }

        let Some(pos) = self.origin(cx) else {
            return Empty.into_any_element();
        };

        let count = self.list.read(cx).delegate().items.len();

        deferred(
            editor_popover("locations-picker", cx)
                .absolute()
                .left(pos.x)
                .top(pos.y)
                .max_w(MAX_MENU_WIDTH)
                .min_w(px(240.))
                .child(
                    div().px_1().pb_1().child(
                        Label::new(SharedString::from(format!("{} ({count})", self.title)))
                            .text_xs()
                            .text_color(cx.theme().muted_foreground),
                    ),
                )
                .child(List::new(&self.list).max_h(MAX_MENU_HEIGHT))
                .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                    this.hide(cx);
                })),
        )
        .into_any_element()
    }
}
