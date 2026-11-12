//! One open page: icon, title, row properties and either the block editor
//! or (for databases) the database views.

use crate::database::{DatabaseView, DbEvent};
use crate::editor::{Editor, EditorEvent};
use crate::editor_render::GUTTER;
use crate::props::{prop_icon, prop_value, CellEditor, CellEvent};
use crate::store::{Store, StoreEvent};
use crate::text_input::{InputEvent, TextInput};
use crate::theme::{theme, Theme, PAGE_MAX_W};
use crate::ui::*;
use gpui::{
    div, prelude::*, px, AnyElement, Context, Corner, Entity, EventEmitter, FontWeight, Pixels, Point, ScrollHandle,
    SharedString, Subscription, Window,
};
use nota_core::{PropKind, PropValue};

pub enum PageEvent {
    Open(String),
    AskAi,
}

pub struct PageView {
    store: Entity<Store>,
    pub page_id: String,
    title: Entity<TextInput>,
    pub editor: Option<Entity<Editor>>,
    db: Option<Entity<DatabaseView>>,
    scroll: ScrollHandle,
    icon_picker: Option<Point<Pixels>>,
    cell: Option<Entity<CellEditor>>,
    _cell_sub: Option<Subscription>,
    _subs: Vec<Subscription>,
}

impl EventEmitter<PageEvent> for PageView {}

impl PageView {
    pub fn new(store: Entity<Store>, page_id: String, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let (is_db, title_text) = {
            let s = store.read(cx);
            let p = s.page(&page_id);
            (p.is_some_and(|p| p.is_database()), p.map(|p| p.title.clone()).unwrap_or_default())
        };
        let scroll = ScrollHandle::new();
        let title = cx.new(|cx| {
            let mut i = TextInput::new(window, cx);
            i.font_size = px(40.);
            i.line_height = px(50.);
            i.weight = FontWeight::BOLD;
            i.placeholder = if is_db { "Untitled".into() } else { "New page".into() };
            i.set_text(title_text.clone(), cx);
            i
        });
        let mut subs = vec![
            cx.observe(&store, |_, _, cx| cx.notify()),
            cx.subscribe_in(&title, window, |this: &mut Self, _, ev: &InputEvent, window, cx| match ev {
                InputEvent::Changed => {
                    let text = this.title.read(cx).text().to_string();
                    let id = this.page_id.clone();
                    this.store.update(cx, |s, cx| {
                        if s.page(&id).is_some_and(|p| p.title != text) {
                            s.update_page(&id, cx, |p| p.title = text)
                        }
                    });
                }
                InputEvent::Submit | InputEvent::MoveDown => {
                    if let Some(editor) = &this.editor {
                        editor.update(cx, |e, cx| e.focus_start(window, cx));
                    }
                }
                _ => {}
            }),
            cx.subscribe(&store, |this: &mut Self, _, ev: &StoreEvent, cx| {
                if let StoreEvent::BlocksChanged(id) = ev {
                    if *id == this.page_id {
                        if let Some(editor) = &this.editor {
                            editor.update(cx, |e, cx| e.reload_blocks(cx));
                        }
                    }
                    return;
                }
                let text = this.store.read(cx).page(&this.page_id).map(|p| p.title.clone()).unwrap_or_default();
                this.title.update(cx, |t, cx| {
                    if t.text() != text {
                        t.set_text(text, cx)
                    }
                });
                if let Some(editor) = &this.editor {
                    editor.update(cx, |e, cx| e.reload_blocks(cx));
                }
            }),
        ];
        let mut editor = None;
        let mut db = None;
        if is_db {
            let view = cx.new(|cx| DatabaseView::new(store.clone(), page_id.clone(), window, cx));
            subs.push(cx.subscribe(&view, |_, _, ev: &DbEvent, cx| {
                let DbEvent::Open(id) = ev;
                cx.emit(PageEvent::Open(id.clone()))
            }));
            db = Some(view);
        } else {
            let ed = cx.new(|cx| Editor::new(store.clone(), page_id.clone(), scroll.clone(), window, cx));
            subs.push(cx.subscribe_in(&ed, window, |this: &mut Self, ed, ev: &EditorEvent, window, cx| match ev {
                EditorEvent::Changed => {
                    let blocks = ed.read(cx).blocks.clone();
                    let id = this.page_id.clone();
                    this.store.update(cx, |s, cx| s.update_page(&id, cx, |p| p.blocks = blocks));
                }
                EditorEvent::OpenPage(id) => cx.emit(PageEvent::Open(id.clone())),
                EditorEvent::FocusTitle => {
                    this.title.update(cx, |t, cx| t.move_to_end(cx));
                    this.title.read(cx).focus(window);
                }
                EditorEvent::AskAi => cx.emit(PageEvent::AskAi),
            }));
            editor = Some(ed);
        }
        if title_text.is_empty() {
            title.read(cx).focus(window);
        }
        PageView {
            store,
            page_id,
            title,
            editor,
            db,
            scroll,
            icon_picker: None,
            cell: None,
            _cell_sub: None,
            _subs: subs,
        }
    }

    /// Called when a kept-alive view is shown again: pick up anything that
    /// changed in the store while it was hidden.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let (title, blocks) = {
            let s = self.store.read(cx);
            let p = s.page(&self.page_id);
            (p.map(|p| p.title.clone()).unwrap_or_default(), p.map(|p| p.blocks.clone()))
        };
        self.title.update(cx, |t, cx| {
            if t.text() != title {
                t.set_text(title, cx)
            }
        });
        if let (Some(editor), Some(blocks)) = (&self.editor, blocks) {
            let stale = editor.read(cx).blocks != blocks && !blocks.is_empty();
            if stale {
                editor.update(cx, |e, cx| e.reload_blocks(cx));
            }
        }
        self.icon_picker = None;
        cx.notify();
    }

    fn set_icon(&mut self, icon: Option<String>, cx: &mut Context<Self>) {
        let id = self.page_id.clone();
        self.store.update(cx, |s, cx| s.update_page(&id, cx, |p| p.icon = icon));
        self.icon_picker = None;
        cx.notify();
    }

    fn edit_prop(&mut self, db: String, prop: String, kind: PropKind, pos: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        self.close_cell(cx);
        let row = self.page_id.clone();
        if kind == PropKind::Checkbox {
            self.store.update(cx, |s, cx| {
                let on = s.page(&row).and_then(|p| p.props.get(&prop)).is_some_and(|v| v.as_bool());
                s.set_prop(&row, &prop, Some(PropValue::Bool(!on)), cx);
            });
            return;
        }
        let store = self.store.clone();
        let cell = cx.new(|cx| CellEditor::new(store, db, row, prop, pos, window, cx));
        self._cell_sub = Some(cx.subscribe(&cell, |this: &mut Self, _, _: &CellEvent, cx| this.close_cell(cx)));
        self.cell = Some(cell);
        cx.notify();
    }

    fn close_cell(&mut self, cx: &mut Context<Self>) {
        if let Some(cell) = self.cell.take() {
            cell.update(cx, |c, cx| c.commit(cx));
            self._cell_sub = None;
            cx.notify();
        }
    }

    fn render_icon_picker(&self, pos: Point<Pixels>, t: &Theme, cx: &Context<Self>) -> AnyElement {
        let mut grid = div().flex().flex_wrap().w(px(9. * 34.)).px(px(8.)).pb(px(6.));
        for (i, e) in EMOJIS.iter().enumerate() {
            let hover = t.hover;
            grid = grid.child(
                div()
                    .id(("emoji", i))
                    .flex()
                    .items_center()
                    .justify_center()
                    .size(px(34.))
                    .rounded(px(6.))
                    .text_size(px(22.))
                    .cursor_pointer()
                    .hover(move |s| s.bg(hover))
                    .on_click(cx.listener(move |this, _, _, cx| this.set_icon(Some(e.to_string()), cx)))
                    .child(*e),
            );
        }
        let menu = menu_surface(t)
            .child(
                div()
                    .flex()
                    .items_center()
                    .child(div().flex_1().child(menu_label("Emoji", t)))
                    .child(
                        text_button("emoji-remove", "Remove", t)
                            .mr(px(6.))
                            .on_click(cx.listener(|this, _, _, cx| this.set_icon(None, cx))),
                    ),
            )
            .child(grid);
        popover(
            pos,
            Corner::TopLeft,
            menu,
            cx.listener(|this, _, _, cx| {
                this.icon_picker = None;
                cx.notify();
            }),
        )
    }
}

impl Render for PageView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let store = self.store.clone();
        let cx: &Context<Self> = cx;
        let s = store.read(cx);
        let Some(page) = s.page(&self.page_id) else {
            return div().size_full().into_any_element();
        };
        let is_db = page.is_database();
        let hover = t.hover;

        // A file Nota could not read is shown as it is, read-only.
        if let Some(err) = &page.load_error {
            let file = page.file.as_ref().map(|f| f.display().to_string()).unwrap_or_default();
            return div()
                .id("page-scroll")
                .size_full()
                .overflow_y_scroll()
                .child(
                    div().flex().justify_center().w_full().child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(14.))
                            .w_full()
                            .max_w(px(PAGE_MAX_W))
                            .pt(px(48.))
                            .pb(px(80.))
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(px(4.))
                                    .p(px(12.))
                                    .rounded(px(8.))
                                    .bg(t.callout_bg)
                                    .text_size(px(14.))
                                    .child(div().font_weight(FontWeight::SEMIBOLD).child(format!("Nota couldn't read {file}")))
                                    .child(div().text_color(t.text_muted).child(SharedString::from(err.clone())))
                                    .child(div().text_color(t.text_muted).child(
                                        "It is shown read-only and won't be changed. Fix the file in a text editor, then use Settings → Reload from disk.",
                                    )),
                            )
                            .child(crate::blocks_view::static_blocks(&page.blocks, &t, 16.)),
                    ),
                )
                .into_any_element();
        }

        // --- icon + title ---------------------------------------------------
        let mut head = div().group("page-head").flex().flex_col();
        match page.icon.as_deref().filter(|i| !i.is_empty()) {
            Some(emoji) => {
                head = head.child(
                    div().flex().child(
                        div()
                            .id("page-icon")
                            .flex()
                            .items_center()
                            .justify_center()
                            .size(px(if is_db { 44. } else { 78. }))
                            .ml(px(-4.))
                            .mb(px(6.))
                            .rounded(px(6.))
                            .text_size(px(if is_db { 32. } else { 62. }))
                            .line_height(px(if is_db { 40. } else { 74. }))
                            .cursor_pointer()
                            .hover(move |s| s.bg(hover))
                            .on_click(cx.listener(|this, ev: &gpui::ClickEvent, _, cx| {
                                this.icon_picker = Some(ev.position());
                                cx.notify();
                            }))
                            .child(SharedString::from(emoji.to_string())),
                    ),
                );
            }
            None => {
                head = head.child(
                    div().flex().h(px(30.)).child(
                        div()
                            .id("add-icon")
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .h(px(26.))
                            .px(px(6.))
                            .ml(px(-6.))
                            .rounded(px(6.))
                            .text_size(px(14.))
                            .text_color(t.text_faint)
                            .cursor_pointer()
                            .opacity(0.)
                            .group_hover("page-head", |s| s.opacity(1.))
                            .hover(move |s| s.bg(hover))
                            .on_click(cx.listener(|this, ev: &gpui::ClickEvent, _, cx| {
                                this.icon_picker = Some(ev.position());
                                cx.notify();
                            }))
                            .child(icon("smile", 16., t.text_faint))
                            .child("Add icon"),
                    ),
                );
            }
        }
        head = head.child(div().pb(px(4.)).child(self.title.clone()));

        // --- properties of a database row -------------------------------------
        let mut props_panel = None;
        if let Some(dbp) = s.vault.parent_database(&self.page_id) {
            if let Some(db) = &dbp.database {
                let mut panel = div().flex().flex_col().pt(px(6.)).pb(px(10.)).text_size(px(14.));
                for (i, p) in db.schema.iter().enumerate() {
                    if p.kind == PropKind::Title {
                        continue;
                    }
                    let (db_id, name, kind) = (dbp.id.clone(), p.name.clone(), p.kind);
                    let value = prop_value(page, p, &t)
                        .unwrap_or_else(|| div().text_color(t.text_faint).child("Empty").into_any_element());
                    panel = panel.child(
                        div()
                            .flex()
                            .items_center()
                            .min_h(px(34.))
                            .child(
                                div()
                                    .flex()
                                    .flex_none()
                                    .items_center()
                                    .gap(px(6.))
                                    .w(px(160.))
                                    .text_color(t.text_muted)
                                    .child(icon(prop_icon(p.kind), 16., t.text_faint))
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .whitespace_nowrap()
                                            .overflow_hidden()
                                            .text_ellipsis()
                                            .child(SharedString::from(p.name.clone())),
                                    ),
                            )
                            .child(
                                div()
                                    .id(("page-prop", i))
                                    .flex()
                                    .flex_1()
                                    .min_w_0()
                                    .items_center()
                                    .min_h(px(30.))
                                    .px(px(6.))
                                    .rounded(px(6.))
                                    .cursor_pointer()
                                    .overflow_hidden()
                                    .hover(move |s| s.bg(hover))
                                    .on_click(cx.listener(move |this, ev: &gpui::ClickEvent, window, cx| {
                                        this.edit_prop(db_id.clone(), name.clone(), kind, ev.position(), window, cx)
                                    }))
                                    .child(value),
                            ),
                    );
                }
                props_panel = Some(panel.child(div().h(px(1.)).mt(px(8.)).bg(t.border)));
            }
        }

        let mut column = div().flex().flex_col().w_full();
        if s.vault.is_trashed(&self.page_id) {
            let id = self.page_id.clone();
            column = column.child(
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .gap(px(12.))
                    .h(px(44.))
                    .bg(t.danger)
                    .text_color(gpui::white())
                    .text_size(px(14.))
                    .child("This page is in Trash.")
                    .child(
                        div()
                            .id("restore-banner")
                            .px(px(10.))
                            .py(px(3.))
                            .rounded(px(6.))
                            .border_1()
                            .border_color(gpui::white())
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let id = id.clone();
                                this.store.update(cx, |s, cx| s.restore(&id, cx))
                            }))
                            .child("Restore page"),
                    ),
            );
        }
        if is_db {
            let mut inner = div().flex().flex_col().w_full().px(px(96.)).pt(px(36.)).pb(px(60.)).child(head);
            if let Some(db) = &self.db {
                inner = inner.child(db.clone());
            }
            column = column.child(inner);
        } else {
            let mut inner = div()
                .flex()
                .flex_col()
                .w_full()
                .max_w(px(PAGE_MAX_W + 2. * GUTTER))
                .pt(px(if page.icon.is_some() { 64. } else { 76. }));
            let mut top = div().flex().flex_col().px(px(GUTTER)).child(head);
            if let Some(panel) = props_panel {
                top = top.child(panel);
            }
            inner = inner.child(top);
            if let Some(editor) = &self.editor {
                inner = inner.child(div().flex().flex_col().w_full().pt(px(4.)).child(editor.clone()));
            }
            // The empty margins beside the text start a block selection rectangle.
            let margin = |id: &'static str| {
                div().id(id).flex_1().min_w_0().on_mouse_down(
                    gpui::MouseButton::Left,
                    cx.listener(|this, ev: &gpui::MouseDownEvent, window, cx| {
                        if let Some(editor) = &this.editor {
                            editor.update(cx, |e, cx| e.begin_marquee(ev.position, false, window, cx));
                        }
                    }),
                )
            };
            column = column.child(
                div()
                    .flex()
                    .w_full()
                    .child(margin("page-margin-left"))
                    .child(inner.min_w_0())
                    .child(margin("page-margin-right")),
            );
        }

        let mut root = div()
            .id("page-scroll")
            .size_full()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .child(column);
        if let Some(pos) = self.icon_picker {
            root = root.child(self.render_icon_picker(pos, &t, cx));
        }
        if let Some(cell) = &self.cell {
            root = root.child(popover(
                cell.read(cx).pos,
                Corner::TopLeft,
                cell.clone(),
                cx.listener(|this, _, _, cx| this.close_cell(cx)),
            ));
        }
        root.into_any_element()
    }
}
