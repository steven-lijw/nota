//! Database pages: view tabs, the table / board / list / gallery / calendar
//! layouts, and the menus for views, properties, filters and sorts.

use crate::props::{eid, prop_icon, prop_value, CellEditor, CellEvent};
use crate::store::Store;
use crate::text_input::{InputEvent, TextInput};
use crate::theme::{theme, ui_font, Theme};
use crate::ui::*;
use chrono::Datelike;
use gpui::{
    canvas, div, prelude::*, px, AnyElement, Bounds, Context, Corner, DragMoveEvent, Entity, EventEmitter,
    FontWeight, Hsla, MouseButton, MouseDownEvent, Pixels, Point, SharedString, Subscription, Window,
};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use nota_core::query::{add_months, date_to_string, group, month_grid, month_name, parse_date, query, today};
use nota_core::{
    default_status_options, Anchor, Database, Filter, FilterOp, Page, PageKind, PropKind, PropValue, Property,
    Sort, View, ViewKind,
};

pub enum DbEvent {
    /// Open the row as a full page.
    Open(String),
    /// Show the row in the side peek.
    Peek(String),
}

#[derive(Clone, PartialEq)]
enum Popup {
    ViewMenu(usize),
    AddView,
    PropMenu(String),
    AddProp,
    Filter,
    Sort,
    Props,
    RowMenu(String),
}

#[derive(Clone)]
pub struct DragRow {
    id: String,
    title: String,
    icon: Option<String>,
    /// Width of the card being dragged; the preview keeps that size so it
    /// stays exactly under the pointer where the card was grabbed.
    width: Option<f32>,
    /// Board: the option of the column the card came from.
    from: Option<String>,
}

/// What follows the pointer while a row is dragged.
struct DragCard {
    title: SharedString,
    icon: Option<String>,
    width: Option<f32>,
    /// Where the pointer grabbed the source element.
    grab: Point<Pixels>,
}

impl Render for DragCard {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let card = div()
            .flex()
            .items_center()
            .gap(px(6.))
            .px(px(10.))
            .py(px(8.))
            .rounded(px(8.))
            .bg(t.card_bg)
            .shadow(popover_shadow(&t))
            .text_size(px(14.))
            .text_color(t.text)
            .font_family(ui_font())
            .font_weight(FontWeight::MEDIUM)
            .whitespace_nowrap()
            .overflow_hidden()
            .opacity(0.92)
            .child(page_icon(self.icon.as_deref(), false, 14., &t))
            .child(self.title.clone());
        match self.width {
            Some(w) => div().w(px(w)).child(card),
            // Unknown size (list rows, calendar chips): float a chip at the pointer.
            None => div()
                .pl((self.grab.x - px(14.)).max(px(0.)))
                .pt((self.grab.y - px(16.)).max(px(0.)))
                .child(card.max_w(px(260.))),
        }
    }
}

/// Dragging the right edge of a table column header.
#[derive(Clone)]
struct ResizeCol(String);

struct NoPreview;

impl Render for NoPreview {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

/// Board drop indicator: before card `index` of column `group`.
#[derive(Clone, Copy, PartialEq)]
struct BoardDrop {
    group: usize,
    index: usize,
}

fn view_icon(kind: ViewKind) -> &'static str {
    match kind {
        ViewKind::Table => "table",
        ViewKind::Board => "board",
        ViewKind::List => "list",
        ViewKind::Gallery => "gallery",
        ViewKind::Calendar => "calendar",
    }
}

fn alpha(mut c: Hsla, a: f32) -> Hsla {
    c.a = a;
    c
}

fn col_width(view: &View, prop: &Property) -> f32 {
    view.widths.get(&prop.name).copied().unwrap_or(match prop.kind {
        PropKind::Title => 300.,
        PropKind::Checkbox => 110.,
        PropKind::Number => 130.,
        _ => 190.,
    })
}

pub struct DatabaseView {
    store: Entity<Store>,
    db_id: String,
    popup: Option<(Popup, Point<Pixels>)>,
    input: Entity<TextInput>,
    editing_filter: Option<usize>,
    cell: Option<Entity<CellEditor>>,
    month: (i32, u32),
    /// Window bounds of board cards, by row id.
    card_bounds: Rc<RefCell<HashMap<String, Bounds<Pixels>>>>,
    board_drop: Option<BoardDrop>,
    /// Table / list: insert position among the shown rows.
    row_drop: Option<usize>,
    /// (column, pointer x and width when the resize started)
    col_resize: Option<(String, Pixels, f32)>,
    dragging: Option<String>,
    _subs: Vec<Subscription>,
    _cell_sub: Option<Subscription>,
}

impl EventEmitter<DbEvent> for DatabaseView {}

impl DatabaseView {
    pub fn new(store: Entity<Store>, db_id: String, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| TextInput::new(window, cx));
        let subs = vec![
            cx.observe(&store, |_, _, cx| cx.notify()),
            cx.subscribe(&input, |this: &mut Self, _, ev: &InputEvent, cx| this.on_input(ev, cx)),
        ];
        let d = today();
        DatabaseView {
            store,
            db_id,
            popup: None,
            input,
            editing_filter: None,
            cell: None,
            month: (d.year(), d.month()),
            card_bounds: Rc::new(RefCell::new(HashMap::new())),
            board_drop: None,
            row_drop: None,
            col_resize: None,
            dragging: None,
            _subs: subs,
            _cell_sub: None,
        }
    }

    // --- state helpers -----------------------------------------------------

    fn view_ix(&self, cx: &Context<Self>) -> usize {
        let s = self.store.read(cx);
        let n = s.page(&self.db_id).and_then(|p| p.database.as_ref()).map_or(1, |d| d.views.len());
        s.active_views.get(&self.db_id).copied().unwrap_or(0).min(n.saturating_sub(1))
    }

    fn set_view(&mut self, ix: usize, cx: &mut Context<Self>) {
        let id = self.db_id.clone();
        self.store.update(cx, |s, cx| {
            s.active_views.insert(id, ix);
            cx.notify();
        });
    }

    fn update_db(&self, cx: &mut Context<Self>, f: impl FnOnce(&mut Database)) {
        let id = self.db_id.clone();
        self.store.update(cx, |s, cx| {
            s.update_page(&id, cx, |p| {
                if let Some(db) = p.database.as_mut() {
                    f(db)
                }
            })
        });
    }

    fn update_view(&self, cx: &mut Context<Self>, f: impl FnOnce(&mut View)) {
        let ix = self.view_ix(cx);
        self.update_db(cx, |db| {
            if let Some(v) = db.views.get_mut(ix) {
                f(v)
            }
        });
    }

    fn open_popup(&mut self, popup: Popup, pos: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        self.close_popup(cx);
        self.editing_filter = None;
        let text = match &popup {
            Popup::ViewMenu(ix) => {
                let s = self.store.read(cx);
                s.page(&self.db_id)
                    .and_then(|p| p.database.as_ref())
                    .and_then(|d| d.views.get(*ix))
                    .map(|v| v.name.clone())
            }
            Popup::PropMenu(name) => Some(name.clone()),
            _ => None,
        };
        if let Some(text) = text {
            self.input.update(cx, |i, cx| {
                i.placeholder = "Name".into();
                i.set_text(text, cx);
                i.select_all_text(cx);
            });
            self.input.read(cx).focus(window);
        }
        self.popup = Some((popup, pos));
        cx.notify();
    }

    fn close_popup(&mut self, cx: &mut Context<Self>) {
        if let Some((Popup::PropMenu(old), _)) = self.popup.take() {
            let new = self.input.read(cx).text().trim().to_string();
            if !new.is_empty() && new != old {
                let id = self.db_id.clone();
                self.store.update(cx, |s, cx| {
                    let touched = s.vault.rename_property(&id, &old, &new);
                    s.mark_many(&touched, cx);
                });
            }
        }
        self.editing_filter = None;
        cx.notify();
    }

    fn on_input(&mut self, ev: &InputEvent, cx: &mut Context<Self>) {
        match ev {
            InputEvent::Changed => {
                let text = self.input.read(cx).text().to_string();
                match self.popup.as_ref().map(|p| p.0.clone()) {
                    Some(Popup::ViewMenu(ix)) if !text.trim().is_empty() => self.update_db(cx, |db| {
                        if let Some(v) = db.views.get_mut(ix) {
                            v.name = text.trim().to_string();
                        }
                    }),
                    Some(Popup::Filter) => {
                        if let Some(i) = self.editing_filter {
                            self.update_view(cx, |v| {
                                if let Some(f) = v.filters.get_mut(i) {
                                    f.value = text;
                                }
                            });
                        }
                    }
                    _ => {}
                }
            }
            InputEvent::Submit | InputEvent::Cancel => self.close_popup(cx),
            _ => {}
        }
    }

    // --- rows and cells ----------------------------------------------------

    fn add_row(&mut self, preset: Option<(String, PropValue)>, cx: &mut Context<Self>) -> String {
        let db = self.db_id.clone();
        self.store.update(cx, |s, cx| {
            let id = s.create_page(PageKind::Page, Some(db), cx);
            if let Some((k, v)) = preset {
                s.set_prop(&id, &k, Some(v), cx);
            }
            id
        })
    }

    fn add_row_and_name(
        &mut self,
        preset: Option<(String, PropValue)>,
        pos: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let id = self.add_row(preset, cx);
        let title = {
            let s = self.store.read(cx);
            s.page(&self.db_id).and_then(|p| p.database.as_ref()).map(|d| d.title_prop_name().to_string())
        };
        if let Some(title) = title {
            self.edit_cell(id, title, pos, window, cx);
        }
    }

    fn edit_cell(&mut self, row: String, prop: String, pos: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        self.close_cell(cx);
        self.close_popup(cx);
        let kind = {
            let s = self.store.read(cx);
            s.page(&self.db_id).and_then(|p| p.database.as_ref()).and_then(|d| d.prop(&prop)).map(|p| p.kind)
        };
        if kind == Some(PropKind::Checkbox) {
            self.store.update(cx, |s, cx| {
                let on = s.page(&row).and_then(|p| p.props.get(&prop)).is_some_and(|v| v.as_bool());
                s.set_prop(&row, &prop, Some(PropValue::Bool(!on)), cx);
            });
            return;
        }
        let (store, db) = (self.store.clone(), self.db_id.clone());
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

    fn set_prop_kind(&mut self, name: &str, kind: PropKind, cx: &mut Context<Self>) {
        let (db_id, name) = (self.db_id.clone(), name.to_string());
        self.store.update(cx, |s, cx| {
            let mut seen: Vec<String> = Vec::new();
            for row in s.vault.rows(&db_id) {
                if let Some(v) = row.props.get(&name) {
                    for item in v.as_list() {
                        if !seen.contains(&item) {
                            seen.push(item);
                        }
                    }
                }
            }
            s.update_page(&db_id, cx, |p| {
                let Some(prop) = p.database.as_mut().and_then(|d| d.prop_mut(&name)) else {
                    return;
                };
                if prop.kind == PropKind::Title {
                    return;
                }
                prop.kind = kind;
                if kind.has_options() {
                    if kind == PropKind::Status && prop.options.is_empty() {
                        prop.options = default_status_options();
                    }
                    for v in seen.iter().take(40) {
                        prop.ensure_option(v);
                    }
                }
            });
        });
        cx.notify();
    }

    fn add_view(&mut self, kind: ViewKind, cx: &mut Context<Self>) {
        let mut ix = 0;
        self.update_db(cx, |db| {
            let mut name = kind.label().to_string();
            let mut n = 1;
            while db.views.iter().any(|v| v.name == name) {
                n += 1;
                name = format!("{} {}", kind.label(), n);
            }
            db.views.push(View::new(name, kind));
            ix = db.views.len() - 1;
        });
        self.set_view(ix, cx);
        self.popup = None;
    }

    // --- chrome --------------------------------------------------------------

    fn render_tabs(&self, db: &Database, active: usize, t: &Theme, cx: &Context<Self>) -> AnyElement {
        let mut tabs = div().id("view-tabs").flex().items_center().gap(px(2.)).h(px(38.)).flex_1().min_w_0().overflow_x_scroll();
        for (i, v) in db.views.iter().enumerate() {
            let hover = t.hover;
            let is_active = i == active;
            let mut tab = div()
                .id(("view-tab", i))
                .relative()
                .flex()
                .flex_none()
                .items_center()
                .gap(px(6.))
                .h(px(28.))
                .px(px(8.))
                .rounded(px(6.))
                .cursor_pointer()
                .text_size(px(14.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(if is_active { t.text } else { t.text_faint })
                .hover(move |s| s.bg(hover))
                .on_click(cx.listener(move |this, ev: &gpui::ClickEvent, window, cx| {
                    if this.view_ix(cx) == i {
                        this.open_popup(Popup::ViewMenu(i), ev.position(), window, cx);
                    } else {
                        this.set_view(i, cx);
                    }
                }))
                .child(icon(view_icon(v.kind), 16., if is_active { t.text } else { t.text_faint }))
                .child(SharedString::from(v.name.clone()));
            if is_active {
                tab = tab.child(div().absolute().left(px(8.)).right(px(8.)).bottom(px(-5.)).h(px(2.)).bg(t.text));
            }
            tabs = tabs.child(tab);
        }
        tabs = tabs.child(icon_button("view-add", "plus", t).on_click(cx.listener(
            |this, ev: &gpui::ClickEvent, window, cx| this.open_popup(Popup::AddView, ev.position(), window, cx),
        )));

        let view = &db.views[active];
        let accent = t.accent;
        let tool = |id: &'static str, label: String, on: bool| {
            let b = text_button(id, label, t);
            if on {
                b.text_color(accent)
            } else {
                b
            }
        };
        let n_f = view.filters.len();
        let n_s = view.sorts.len();
        div()
            .flex()
            .items_center()
            .h(px(38.))
            .border_b_1()
            .border_color(t.border)
            .child(tabs)
            .child(div().w(px(8.)).flex_none())
            .child(
                tool("db-filter", if n_f > 0 { format!("Filter · {n_f}") } else { "Filter".into() }, n_f > 0).on_click(
                    cx.listener(|this, ev: &gpui::ClickEvent, window, cx| {
                        this.open_popup(Popup::Filter, ev.position(), window, cx)
                    }),
                ),
            )
            .child(
                tool("db-sort", if n_s > 0 { format!("Sort · {n_s}") } else { "Sort".into() }, n_s > 0).on_click(
                    cx.listener(|this, ev: &gpui::ClickEvent, window, cx| {
                        this.open_popup(Popup::Sort, ev.position(), window, cx)
                    }),
                ),
            )
            .child(icon_button("db-props", "settings", t).on_click(cx.listener(
                |this, ev: &gpui::ClickEvent, window, cx| this.open_popup(Popup::Props, ev.position(), window, cx),
            )))
            .child(div().w(px(6.)))
            .child(primary_button("db-new", "New", t).on_click(cx.listener(
                |this, _, _, cx| {
                    let id = this.add_row(None, cx);
                    cx.emit(DbEvent::Peek(id));
                },
            )))
            .into_any_element()
    }

    // --- table ---------------------------------------------------------------

    fn render_table(&self, db: &Database, view: &View, rows: &[&Page], t: &Theme, cx: &Context<Self>) -> AnyElement {
        let cols: Vec<&Property> = db.schema.iter().filter(|p| !view.hidden.contains(&p.name)).collect();
        let total: f32 = cols.iter().map(|p| col_width(view, p)).sum::<f32>() + 46.;
        let hover = t.hover;

        let mut header = div().flex().h(px(34.)).border_b_1().border_color(t.border);
        for (i, p) in cols.iter().enumerate() {
            let name = p.name.clone();
            let (rname, w) = (p.name.clone(), col_width(view, p));
            let accent = t.accent;
            let resizing = self.col_resize.as_ref().is_some_and(|r| r.0 == p.name);
            let grip = div()
                .id(("th-resize", i))
                .absolute()
                .top_0()
                .bottom_0()
                .right(px(-3.))
                .w(px(6.))
                .cursor_col_resize()
                .when(resizing, |d| d.bg(Hsla { a: 0.5, ..accent }))
                .hover(move |s| s.bg(Hsla { a: 0.35, ..accent }))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                        cx.stop_propagation();
                        this.col_resize = Some((rname.clone(), ev.position.x, w));
                    }),
                )
                .on_drag(ResizeCol(p.name.clone()), |_, _, _, cx| cx.new(|_| NoPreview));
            header = header.child(
                div()
                    .id(("th", i))
                    .relative()
                    .child(grip)
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(6.))
                    .w(px(col_width(view, p)))
                    .px(px(8.))
                    .text_color(t.text_muted)
                    .cursor_pointer()
                    .hover(move |s| s.bg(hover))
                    .on_click(cx.listener(move |this, ev: &gpui::ClickEvent, window, cx| {
                        this.open_popup(Popup::PropMenu(name.clone()), ev.position(), window, cx)
                    }))
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
            );
        }
        header = header.child(
            div()
                .id("th-add")
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .w(px(46.))
                .cursor_pointer()
                .hover(move |s| s.bg(hover))
                .on_click(cx.listener(|this, ev: &gpui::ClickEvent, window, cx| {
                    this.open_popup(Popup::AddProp, ev.position(), window, cx)
                }))
                .child(icon("plus", 16., t.text_faint)),
        );

        let manual = view.sorts.is_empty();
        let ids: Vec<String> = rows.iter().map(|r| r.id.clone()).collect();
        let mut body = div()
            .flex()
            .flex_col()
            .w(px(total))
            .on_drag_move(cx.listener(|this, ev: &DragMoveEvent<ResizeCol>, _, cx| {
                let Some((name, start_x, start_w)) = this.col_resize.clone() else { return };
                let w = (start_w + (ev.event.position.x - start_x) / px(1.)).clamp(70., 900.).round();
                if ev.drag(cx).0 == name {
                    this.update_view(cx, |v| {
                        v.widths.insert(name, w);
                    });
                }
            }))
            .on_drag_move(cx.listener({
                let ids = ids.clone();
                move |this, ev: &DragMoveEvent<DragRow>, _, cx| this.row_drag_moved(ev, &ids, manual, cx)
            }))
            .on_drop(cx.listener({
                let ids = ids.clone();
                move |this, drag: &DragRow, _, cx| this.drop_row(drag, &ids, cx)
            }))
            .child(header);
        let n_rows = rows.len();
        for (ri, row) in rows.iter().enumerate() {
            let group: SharedString = format!("tr-{}", row.id).into();
            let rid = row.id.clone();
            let bounds = self.card_bounds.clone();
            let bid = row.id.clone();
            let mut tr = self
                .drag_source(div().id(eid("tr", &row.id)), row, None, None)
                .group(group.clone())
                .relative()
                .flex()
                .min_h(px(34.))
                .border_b_1()
                .border_color(t.border)
                .child(
                    canvas(move |b, _, _| {
                        bounds.borrow_mut().insert(bid, b);
                    }, |_, _, _, _| {})
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full(),
                )
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(move |this, ev: &MouseDownEvent, window, cx| {
                        this.open_popup(Popup::RowMenu(rid.clone()), ev.position, window, cx)
                    }),
                );
            for (i, p) in cols.iter().enumerate() {
                let (rid, pname) = (row.id.clone(), p.name.clone());
                let mut td = div()
                    .id(eid("td", &format!("{}-{}", row.id, i)))
                    .relative()
                    .flex()
                    .flex_none()
                    .items_center()
                    .w(px(col_width(view, p)))
                    .px(px(8.))
                    .overflow_hidden()
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, ev: &gpui::ClickEvent, window, cx| {
                        this.edit_cell(rid.clone(), pname.clone(), ev.position(), window, cx)
                    }));
                if i + 1 < cols.len() {
                    td = td.border_r_1().border_color(t.border);
                }
                if p.kind == PropKind::Title {
                    let rid = row.id.clone();
                    td = td
                        .gap(px(4.))
                        .child(page_icon(row.icon.as_deref(), false, 15., t))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(if row.title.is_empty() { t.text_faint } else { t.text })
                                .whitespace_nowrap()
                                .overflow_hidden()
                                .text_ellipsis()
                                .child(SharedString::from(row.display_title().to_string())),
                        )
                        .child(
                            div()
                                .id(eid("open", &row.id))
                                .absolute()
                                .right(px(6.))
                                .top(px(5.))
                                .flex()
                                .items_center()
                                .gap(px(3.))
                                .h(px(24.))
                                .px(px(6.))
                                .rounded(px(4.))
                                .bg(t.panel_bg)
                                .border_1()
                                .border_color(t.border)
                                .text_size(px(12.))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(t.text_muted)
                                .opacity(0.)
                                .group_hover(group.clone(), |s| s.opacity(1.))
                                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                .on_click(cx.listener(move |_, _, _, cx| {
                                    cx.stop_propagation();
                                    cx.emit(DbEvent::Open(rid.clone()))
                                }))
                                .child(icon("panel-right", 13., t.text_muted))
                                .child("OPEN"),
                        );
                } else if let Some(v) = prop_value(row, p, t) {
                    td = td.child(v);
                }
                tr = tr.child(td);
            }
            if let Some(at) = self.row_drop {
                let line = div().absolute().left_0().right_0().h(px(2.)).bg(t.accent);
                if at == ri {
                    tr = tr.child(line.top(px(-1.)));
                } else if at == n_rows && ri + 1 == n_rows {
                    tr = tr.child(line.bottom(px(-1.)));
                }
            }
            body = body.child(tr);
        }
        body = body
            .child(
                div()
                    .id("tr-new")
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .h(px(34.))
                    .px(px(8.))
                    .border_b_1()
                    .border_color(t.border)
                    .text_color(t.text_faint)
                    .cursor_pointer()
                    .hover(move |s| s.bg(hover))
                    .on_click(cx.listener(|this, ev: &gpui::ClickEvent, window, cx| {
                        this.add_row_and_name(None, ev.position(), window, cx)
                    }))
                    .child(icon("plus", 16., t.text_faint))
                    .child("New page"),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .h(px(34.))
                    .px(px(8.))
                    .text_size(px(12.))
                    .text_color(t.text_faint)
                    .child("COUNT")
                    .child(div().text_size(px(14.)).text_color(t.text_muted).child(rows.len().to_string())),
            );
        div().id("table-scroll").w_full().overflow_x_scroll().child(body).into_any_element()
    }

    // --- cards (board, gallery, calendar) -------------------------------------

    fn card_props(&self, db: &Database, view: &View, row: &Page, skip: Option<&str>, t: &Theme) -> Vec<AnyElement> {
        db.schema
            .iter()
            .filter(|p| p.kind != PropKind::Title && !view.hidden.contains(&p.name) && Some(p.name.as_str()) != skip)
            .filter(|p| p.kind != PropKind::Checkbox || row.props.get(&p.name).is_some_and(|v| v.as_bool()))
            .filter_map(|p| prop_value(row, p, t))
            .map(|v| div().flex().text_size(px(12.)).text_color(t.text_muted).child(v).into_any_element())
            .collect()
    }

    fn card_title(&self, row: &Page, t: &Theme) -> gpui::Div {
        div()
            .flex()
            .items_center()
            .gap(px(4.))
            .child(page_icon(row.icon.as_deref(), false, 14., t))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(if row.title.is_empty() { t.text_faint } else { t.text })
                    .child(SharedString::from(row.display_title().to_string())),
            )
    }

    /// Make `el` drag the given row (dimmed while it is being dragged).
    fn drag_source(
        &self,
        el: gpui::Stateful<gpui::Div>,
        row: &Page,
        width: Option<f32>,
        from: Option<String>,
    ) -> gpui::Stateful<gpui::Div> {
        let dragged = self.dragging.as_deref() == Some(row.id.as_str());
        el.when(dragged, |d| d.opacity(0.4)).on_drag(
            DragRow {
                id: row.id.clone(),
                title: row.display_title().to_string(),
                icon: row.icon.clone(),
                width,
                from,
            },
            |drag: &DragRow, grab, _, cx| {
                let (title, icon, width) = (drag.title.clone().into(), drag.icon.clone(), drag.width);
                cx.new(|_| DragCard { title, icon, width, grab })
            },
        )
    }

    /// A card (or list row) that opens on click and can be dragged.
    fn draggable_card(
        &self,
        row: &Page,
        width: Option<f32>,
        from: Option<String>,
        t: &Theme,
        cx: &Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let rid = row.id.clone();
        let rid2 = row.id.clone();
        let hover = t.hover;
        let el = div()
            .id(eid("card", &row.id))
            .cursor_pointer()
            .hover(move |s| s.bg(hover))
            .on_click(cx.listener(move |_, _, _, cx| cx.emit(DbEvent::Peek(rid.clone()))))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, ev: &MouseDownEvent, window, cx| {
                    this.open_popup(Popup::RowMenu(rid2.clone()), ev.position, window, cx)
                }),
            );
        self.drag_source(el, row, width, from)
    }

    /// A row is dragged over a table or list: work out the insert position.
    fn row_drag_moved(&mut self, ev: &DragMoveEvent<DragRow>, ids: &[String], manual: bool, cx: &mut Context<Self>) {
        let pos = ev.event.position;
        let dragged = ev.drag(cx).id.clone();
        if self.dragging.as_ref() != Some(&dragged) {
            self.dragging = Some(dragged.clone());
            cx.notify();
        }
        let at = if manual && ev.bounds.contains(&pos) {
            let bounds = self.card_bounds.borrow();
            Some(
                ids.iter()
                    .position(|id| bounds.get(id).is_some_and(|b| pos.y < b.center().y))
                    .unwrap_or(ids.len()),
            )
        } else {
            None
        };
        // dropping right next to itself changes nothing
        let at = at.filter(|&i| {
            let me = ids.iter().position(|id| *id == dragged);
            me.is_none_or(|m| i != m && i != m + 1)
        });
        if self.row_drop != at {
            self.row_drop = at;
            cx.notify();
        }
    }

    fn drop_row(&mut self, drag: &DragRow, ids: &[String], cx: &mut Context<Self>) {
        self.dragging = None;
        let Some(at) = self.row_drop.take() else {
            cx.notify();
            return;
        };
        let anchor = match ids.get(at) {
            Some(next) => Anchor::Before(next.clone()),
            None => match ids.last() {
                Some(last) => Anchor::After(last.clone()),
                None => return,
            },
        };
        let (row, db) = (drag.id.clone(), self.db_id.clone());
        self.store.update(cx, |s, cx| s.place_page(&row, Some(db), anchor, cx));
        cx.notify();
    }

    fn render_board(&self, db: &Database, view: &View, rows: &[&Page], t: &Theme, cx: &Context<Self>) -> AnyElement {
        let prop = view
            .group_by
            .as_ref()
            .and_then(|n| db.prop(n))
            .filter(|p| p.kind.has_options())
            .or_else(|| db.first_of(&[PropKind::Status, PropKind::Select, PropKind::MultiSelect]));
        let Some(prop) = prop else {
            return self.empty_state(
                "Board view groups pages by a Status or Select property.",
                "Add a Status property",
                t,
                cx.listener(|this, _: &gpui::ClickEvent, _, cx| {
                    this.update_db(cx, |db| {
                        let name = db.unique_prop_name("Status");
                        db.schema.push(Property::new(name, PropKind::Status));
                    })
                }),
            );
        };
        let manual = view.sorts.is_empty();
        let mut board = div().flex().items_start().gap(px(8.)).pt(px(10.)).pb(px(24.));
        for (gi, g) in group(prop, rows).into_iter().enumerate() {
            if g.option.is_none() && g.rows.is_empty() {
                continue;
            }
            let color = g.option.as_ref().map(|o| o.color.clone()).unwrap_or_else(|| "default".into());
            let (tag_bg, _) = t.tag(&color);
            let col_bg = alpha(tag_bg, if t.dark { 0.22 } else { 0.38 });
            let option = g.option.as_ref().map(|o| o.name.clone());
            let value = g.option.as_ref().map(|o| {
                if prop.kind == PropKind::MultiSelect {
                    PropValue::List(vec![o.name.clone()])
                } else {
                    PropValue::Text(o.name.clone())
                }
            });
            let head = match &g.option {
                Some(o) if prop.kind == PropKind::Status => status_pill(o.name.clone(), &o.color, t).into_any_element(),
                Some(o) => pill(o.name.clone(), &o.color, t).into_any_element(),
                None => div().text_color(t.text_muted).child(format!("No {}", prop.name)).into_any_element(),
            };
            // cards of this column, without the one being dragged
            let ids: Vec<String> = g.rows.iter().map(|r| r.id.clone()).collect();
            let (pname, kind, to) = (prop.name.clone(), prop.kind, option.clone());
            let over = alpha(t.accent, 0.10);
            let drop_here = self.board_drop.filter(|d| d.group == gi && manual);
            let mut col = div()
                .id(("board-col", gi))
                .flex()
                .flex_col()
                .flex_none()
                .gap(px(6.))
                .w(px(264.))
                .p(px(6.))
                .rounded(px(10.))
                .bg(col_bg)
                .drag_over::<DragRow>(move |s, _, _, _| s.bg(over))
                .on_drag_move(cx.listener({
                    let ids = ids.clone();
                    move |this, ev: &DragMoveEvent<DragRow>, _, cx| {
                        let pos = ev.event.position;
                        let dragged = ev.drag(cx).id.clone();
                        if this.dragging.as_ref() != Some(&dragged) {
                            this.dragging = Some(dragged.clone());
                            cx.notify();
                        }
                        if !ev.bounds.contains(&pos) {
                            if this.board_drop.is_some_and(|d| d.group == gi) {
                                this.board_drop = None;
                                cx.notify();
                            }
                            return;
                        }
                        let bounds = this.card_bounds.borrow();
                        let others: Vec<&String> = ids.iter().filter(|id| **id != dragged).collect();
                        let index = others
                            .iter()
                            .position(|id| bounds.get(*id).is_some_and(|b| pos.y < b.center().y))
                            .unwrap_or(others.len());
                        drop(bounds);
                        let d = Some(BoardDrop { group: gi, index });
                        if this.board_drop != d {
                            this.board_drop = d;
                            cx.notify();
                        }
                    }
                }))
                .on_drop(cx.listener({
                    let ids = ids.clone();
                    move |this, drag: &DragRow, _, cx| {
                        let target = this.board_drop.take().filter(|d| d.group == gi);
                        this.dragging = None;
                        let others: Vec<&String> = ids.iter().filter(|id| **id != drag.id).collect();
                        let anchor = target.filter(|_| manual).and_then(|d| match others.get(d.index) {
                            Some(next) => Some(Anchor::Before((*next).clone())),
                            None => others.last().map(|last| Anchor::After((*last).clone())),
                        });
                        let (row, db) = (drag.id.clone(), this.db_id.clone());
                        let (from, to, pname) = (drag.from.clone(), to.clone(), pname.clone());
                        this.store.update(cx, |s, cx| {
                            if from != to {
                                let v = moved_value(s.page(&row), &pname, kind, from.as_deref(), to.as_deref());
                                s.set_prop(&row, &pname, v, cx);
                            }
                            if let Some(anchor) = anchor {
                                s.place_page(&row, Some(db), anchor, cx);
                            }
                        });
                        cx.notify();
                    }
                }))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .h(px(28.))
                        .px(px(4.))
                        .child(head)
                        .child(div().text_color(t.text_faint).child(g.rows.len().to_string())),
                );
            let line = |top: bool| {
                let l = div().absolute().left(px(2.)).right(px(2.)).h(px(3.)).rounded(px(1.5)).bg(t.accent);
                if top { l.top(px(-5.)) } else { l.bottom(px(-5.)) }
            };
            let mut shown = 0;
            let n_others = g.rows.iter().filter(|r| self.dragging.as_deref() != Some(r.id.as_str())).count();
            for row in &g.rows {
                let bounds = self.card_bounds.clone();
                let rid = row.id.clone();
                let is_dragged = self.dragging.as_deref() == Some(row.id.as_str());
                let mut card = self
                    .draggable_card(row, Some(252.), option.clone(), t, cx)
                    .relative()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .px(px(10.))
                    .py(px(8.))
                    .rounded(px(8.))
                    .bg(t.card_bg)
                    .shadow(card_shadow(t))
                    .child(
                        canvas(move |b, _, _| {
                            bounds.borrow_mut().insert(rid, b);
                        }, |_, _, _, _| {})
                        .absolute()
                        .top_0()
                        .left_0()
                        .size_full(),
                    )
                    .child(self.card_title(row, t));
                for p in self.card_props(db, view, row, Some(&prop.name), t) {
                    card = card.child(p);
                }
                if let Some(d) = drop_here {
                    if !is_dragged {
                        if d.index == shown {
                            card = card.child(line(true));
                        } else if d.index == n_others && shown + 1 == n_others {
                            card = card.child(line(false));
                        }
                    }
                }
                if !is_dragged {
                    shown += 1;
                }
                col = col.child(card);
            }
            let pname = prop.name.clone();
            let hover = t.hover;
            col = col.child(
                div()
                    .id(("board-new", gi))
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .h(px(30.))
                    .px(px(6.))
                    .rounded(px(6.))
                    .text_color(t.text_faint)
                    .cursor_pointer()
                    .hover(move |s| s.bg(hover))
                    .on_click(cx.listener(move |this, ev: &gpui::ClickEvent, window, cx| {
                        let preset = value.clone().map(|v| (pname.clone(), v));
                        this.add_row_and_name(preset, ev.position(), window, cx)
                    }))
                    .child(icon("plus", 16., t.text_faint))
                    .child("New page"),
            );
            board = board.child(col);
        }
        div().id("board-scroll").w_full().overflow_x_scroll().child(board).into_any_element()
    }

    fn render_list(&self, db: &Database, view: &View, rows: &[&Page], t: &Theme, cx: &Context<Self>) -> AnyElement {
        let manual = view.sorts.is_empty();
        let ids: Vec<String> = rows.iter().map(|r| r.id.clone()).collect();
        let mut list = div()
            .flex()
            .flex_col()
            .pt(px(6.))
            .on_drag_move(cx.listener({
                let ids = ids.clone();
                move |this, ev: &DragMoveEvent<DragRow>, _, cx| this.row_drag_moved(ev, &ids, manual, cx)
            }))
            .on_drop(cx.listener({
                let ids = ids.clone();
                move |this, drag: &DragRow, _, cx| this.drop_row(drag, &ids, cx)
            }));
        let n_rows = rows.len();
        for (ri, row) in rows.iter().enumerate() {
            let mut right = div().flex().flex_none().items_center().gap(px(10.)).text_size(px(12.)).text_color(t.text_muted);
            for p in self.card_props(db, view, row, None, t) {
                right = right.child(p);
            }
            let bounds = self.card_bounds.clone();
            let bid = row.id.clone();
            let mut line = None;
            if let Some(at) = self.row_drop {
                let l = div().absolute().left_0().right_0().h(px(2.)).bg(t.accent);
                if at == ri {
                    line = Some(l.top(px(-1.)));
                } else if at == n_rows && ri + 1 == n_rows {
                    line = Some(l.bottom(px(-1.)));
                }
            }
            list = list.child(
                self.draggable_card(row, None, None, t, cx)
                    .relative()
                    .child(
                        canvas(move |b, _, _| {
                            bounds.borrow_mut().insert(bid, b);
                        }, |_, _, _, _| {})
                        .absolute()
                        .top_0()
                        .left_0()
                        .size_full(),
                    )
                    .children(line)
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .h(px(32.))
                    .px(px(6.))
                    .rounded(px(6.))
                    .child(page_icon(row.icon.as_deref(), false, 15., t))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(if row.title.is_empty() { t.text_faint } else { t.text })
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(SharedString::from(row.display_title().to_string())),
                    )
                    .child(right),
            );
        }
        let hover = t.hover;
        list.child(
            div()
                .id("list-new")
                .flex()
                .items_center()
                .gap(px(6.))
                .h(px(32.))
                .px(px(6.))
                .rounded(px(6.))
                .text_color(t.text_faint)
                .cursor_pointer()
                .hover(move |s| s.bg(hover))
                .on_click(cx.listener(|this, ev: &gpui::ClickEvent, window, cx| {
                    this.add_row_and_name(None, ev.position(), window, cx)
                }))
                .child(icon("plus", 16., t.text_faint))
                .child("New page"),
        )
        .into_any_element()
    }

    fn render_gallery(&self, db: &Database, view: &View, rows: &[&Page], t: &Theme, cx: &Context<Self>) -> AnyElement {
        let mut grid = div().flex().flex_wrap().gap(px(16.)).pt(px(14.)).pb(px(24.));
        for row in rows {
            let preview: String = row.plain_text().chars().take(260).collect();
            let mut meta = div().flex().flex_col().gap(px(6.)).px(px(10.)).py(px(8.)).child(self.card_title(row, t));
            for p in self.card_props(db, view, row, None, t) {
                meta = meta.child(p);
            }
            grid = grid.child(
                self.draggable_card(row, Some(250.), None, t, cx)
                    .flex()
                    .flex_col()
                    .w(px(250.))
                    .rounded(px(10.))
                    .bg(t.card_bg)
                    .shadow(card_shadow(t))
                    .overflow_hidden()
                    .child(
                        div()
                            .h(px(130.))
                            .px(px(14.))
                            .py(px(12.))
                            .bg(t.sidebar_bg)
                            .border_b_1()
                            .border_color(t.border)
                            .overflow_hidden()
                            .text_size(px(11.))
                            .line_height(px(16.))
                            .text_color(t.text_muted)
                            .child(SharedString::from(preview)),
                    )
                    .child(meta),
            );
        }
        let hover = t.hover;
        grid.child(
            div()
                .id("gallery-new")
                .flex()
                .items_center()
                .justify_center()
                .gap(px(6.))
                .w(px(250.))
                .h(px(170.))
                .rounded(px(10.))
                .border_1()
                .border_color(t.border)
                .text_color(t.text_faint)
                .cursor_pointer()
                .hover(move |s| s.bg(hover))
                .on_click(cx.listener(|this, _, _, cx| {
                    let id = this.add_row(None, cx);
                    cx.emit(DbEvent::Peek(id));
                }))
                .child(icon("plus", 16., t.text_faint))
                .child("New page"),
        )
        .into_any_element()
    }

    fn render_calendar(&self, db: &Database, view: &View, rows: &[&Page], t: &Theme, cx: &Context<Self>) -> AnyElement {
        let prop = view
            .date_prop
            .as_ref()
            .and_then(|n| db.prop(n))
            .filter(|p| p.kind == PropKind::Date)
            .or_else(|| db.first_of(&[PropKind::Date]));
        let Some(prop) = prop else {
            return self.empty_state(
                "Calendar view needs a Date property.",
                "Add a Date property",
                t,
                cx.listener(|this, _: &gpui::ClickEvent, _, cx| {
                    this.update_db(cx, |db| {
                        let name = db.unique_prop_name("Date");
                        db.schema.push(Property::new(name, PropKind::Date));
                    })
                }),
            );
        };
        let (y, m) = self.month;
        let now = today();
        let nav = div()
            .flex()
            .items_center()
            .h(px(42.))
            .child(
                div()
                    .flex_1()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(format!("{} {}", month_name(m), y)),
            )
            .child(icon_button("cal-prev", "chevron-left", t).on_click(cx.listener(move |this, _, _, cx| {
                this.month = add_months(y, m, -1);
                cx.notify();
            })))
            .child(text_button("cal-today", "Today", t).on_click(cx.listener(|this, _, _, cx| {
                let d = today();
                this.month = (d.year(), d.month());
                cx.notify();
            })))
            .child(icon_button("cal-next", "chevron-right", t).on_click(cx.listener(move |this, _, _, cx| {
                this.month = add_months(y, m, 1);
                cx.notify();
            })));
        let mut head = div().flex();
        for d in ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"] {
            head = head.child(
                div()
                    .flex_1()
                    .flex()
                    .justify_center()
                    .pb(px(6.))
                    .text_size(px(12.))
                    .text_color(t.text_faint)
                    .child(d),
            );
        }
        let mut grid = div().flex().flex_col().border_t_1().border_l_1().border_color(t.border);
        for (w, week) in month_grid(y, m).chunks(7).enumerate() {
            let mut line = div().flex();
            for (i, day) in week.iter().enumerate() {
                let day = *day;
                let key = date_to_string(day);
                let group: SharedString = format!("cal-{key}").into();
                let pname = prop.name.clone();
                let pname2 = prop.name.clone();
                let (k1, k2) = (key.clone(), key.clone());
                let over = alpha(t.accent, 0.10);
                let weekend = i == 0 || i == 6;
                let mut number = div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .h(px(22.))
                    .min_w(px(22.))
                    .px(px(4.))
                    .rounded(px(11.))
                    .text_size(px(13.))
                    .child(if day.day() == 1 {
                        format!("{} {}", &month_name(day.month())[..3], day.day())
                    } else {
                        day.day().to_string()
                    });
                if day == now {
                    number = number.bg(t.danger).text_color(gpui::white());
                } else if day.month() != m {
                    number = number.text_color(t.text_faint);
                }
                let hover = t.hover;
                let mut cell = div()
                    .id(("cal-cell", w * 7 + i))
                    .group(group.clone())
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(3.))
                    .min_h(px(118.))
                    .p(px(4.))
                    .border_r_1()
                    .border_b_1()
                    .border_color(t.border)
                    .drag_over::<DragRow>(move |s, _, _, _| s.bg(over))
                    .on_drop(cx.listener(move |this, drag: &DragRow, _, cx| {
                        let (row, name, v) = (drag.id.clone(), pname.clone(), PropValue::Text(k1.clone()));
                        this.store.update(cx, |s, cx| s.set_prop(&row, &name, Some(v), cx));
                    }))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .child(
                                div()
                                    .id(("cal-add", w * 7 + i))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .size(px(22.))
                                    .rounded(px(4.))
                                    .cursor_pointer()
                                    .opacity(0.)
                                    .group_hover(group, |s| s.opacity(1.))
                                    .hover(move |s| s.bg(hover))
                                    .on_click(cx.listener(move |this, ev: &gpui::ClickEvent, window, cx| {
                                        let preset = Some((pname2.clone(), PropValue::Text(k2.clone())));
                                        this.add_row_and_name(preset, ev.position(), window, cx)
                                    }))
                                    .child(icon("plus", 14., t.text_faint)),
                            )
                            .child(div().flex_1())
                            .child(number),
                    );
                if weekend {
                    cell = cell.bg(alpha(t.sidebar_bg, if t.dark { 0.5 } else { 0.7 }));
                }
                for row in rows {
                    let on_day = row.props.get(&prop.name).and_then(|v| parse_date(&v.as_text())) == Some(day);
                    if !on_day {
                        continue;
                    }
                    let mut card = self
                        .draggable_card(row, None, None, t, cx)
                        .flex()
                        .flex_col()
                        .gap(px(3.))
                        .px(px(6.))
                        .py(px(4.))
                        .rounded(px(5.))
                        .bg(t.card_bg)
                        .shadow(card_shadow(t))
                        .text_size(px(12.))
                        .child(
                            div()
                                .font_weight(FontWeight::MEDIUM)
                                .whitespace_nowrap()
                                .overflow_hidden()
                                .text_ellipsis()
                                .child(SharedString::from(format!(
                                    "{}{}",
                                    row.icon.as_ref().map(|i| format!("{i} ")).unwrap_or_default(),
                                    row.display_title()
                                ))),
                        );
                    for p in self.card_props(db, view, row, Some(&prop.name), t).into_iter().take(2) {
                        card = card.child(div().overflow_hidden().child(p));
                    }
                    cell = cell.child(card);
                }
                line = line.child(cell);
            }
            grid = grid.child(line);
        }
        div().flex().flex_col().pb(px(24.)).child(nav).child(head).child(grid).into_any_element()
    }

    fn empty_state(
        &self,
        message: &'static str,
        action: &'static str,
        t: &Theme,
        on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut gpui::App) + 'static,
    ) -> AnyElement {
        div()
            .flex()
            .flex_col()
            .items_center()
            .gap(px(12.))
            .py(px(48.))
            .text_color(t.text_muted)
            .child(message)
            .child(primary_button("db-empty-action", action, t).on_click(on_click))
            .into_any_element()
    }

    // --- menus -----------------------------------------------------------------

    fn name_field(&self, t: &Theme) -> gpui::Div {
        div()
            .mx(px(8.))
            .my(px(6.))
            .px(px(8.))
            .py(px(4.))
            .rounded(px(6.))
            .bg(t.hover)
            .border_1()
            .border_color(t.border)
            .child(self.input.clone())
    }

    fn render_popup(&self, db: &Database, active: usize, t: &Theme, cx: &Context<Self>) -> Option<AnyElement> {
        let (popup, pos) = self.popup.as_ref()?;
        let view = &db.views[active];
        let check = |on: bool| if on { Some(icon("check", 14., t.text_muted)) } else { None };
        let mut menu = menu_surface(t).w(px(250.));
        match popup {
            Popup::ViewMenu(ix) => {
                let ix = *ix;
                let v = db.views.get(ix)?;
                menu = menu.child(self.name_field(t)).child(menu_label("Layout", t));
                for (i, kind) in ViewKind::ALL.into_iter().enumerate() {
                    let mut item = menu_item(("vm-kind", i), Some(view_icon(kind)), kind.label(), t).on_click(
                        cx.listener(move |this, _, _, cx| {
                            this.update_db(cx, |db| {
                                if let Some(v) = db.views.get_mut(ix) {
                                    v.kind = kind;
                                }
                            })
                        }),
                    );
                    if let Some(c) = check(v.kind == kind) {
                        item = item.child(c);
                    }
                    menu = menu.child(item);
                }
                let cycle = |current: Option<&str>, kinds: &[PropKind]| -> Option<String> {
                    let names: Vec<&str> =
                        db.schema.iter().filter(|p| kinds.contains(&p.kind)).map(|p| p.name.as_str()).collect();
                    let at = current.and_then(|c| names.iter().position(|n| *n == c));
                    let next = at.map_or(0, |i| (i + 1) % names.len().max(1));
                    names.get(next).map(|s| s.to_string())
                };
                if v.kind == ViewKind::Board {
                    let kinds = [PropKind::Status, PropKind::Select, PropKind::MultiSelect];
                    let cur = v.group_by.clone().or_else(|| db.first_of(&kinds).map(|p| p.name.clone()));
                    let next = cycle(cur.as_deref(), &kinds);
                    let label = format!("Group by: {}", cur.unwrap_or_else(|| "—".into()));
                    menu = menu.child(menu_separator(t)).child(
                        menu_item("vm-group", Some("board"), label, t).on_click(cx.listener(move |this, _, _, cx| {
                            let next = next.clone();
                            this.update_db(cx, |db| {
                                if let Some(v) = db.views.get_mut(ix) {
                                    v.group_by = next;
                                }
                            })
                        })),
                    );
                }
                if v.kind == ViewKind::Calendar {
                    let cur = v.date_prop.clone().or_else(|| db.first_of(&[PropKind::Date]).map(|p| p.name.clone()));
                    let next = cycle(cur.as_deref(), &[PropKind::Date]);
                    let label = format!("Show by: {}", cur.unwrap_or_else(|| "—".into()));
                    menu = menu.child(menu_separator(t)).child(
                        menu_item("vm-date", Some("calendar"), label, t).on_click(cx.listener(move |this, _, _, cx| {
                            let next = next.clone();
                            this.update_db(cx, |db| {
                                if let Some(v) = db.views.get_mut(ix) {
                                    v.date_prop = next;
                                }
                            })
                        })),
                    );
                }
                menu = menu.child(menu_separator(t)).child(
                    menu_item("vm-dup", Some("copy"), "Duplicate view", t).on_click(cx.listener(move |this, _, _, cx| {
                        let mut new_ix = 0;
                        this.update_db(cx, |db| {
                            if let Some(v) = db.views.get(ix).cloned() {
                                let mut v = v;
                                v.name = format!("{} (copy)", v.name);
                                db.views.insert(ix + 1, v);
                                new_ix = ix + 1;
                            }
                        });
                        this.popup = None;
                        this.set_view(new_ix, cx);
                    })),
                );
                if db.views.len() > 1 {
                    menu = menu.child(
                        menu_item("vm-del", Some("trash"), "Delete view", t).on_click(cx.listener(
                            move |this, _, _, cx| {
                                this.popup = None;
                                this.update_db(cx, |db| {
                                    if ix < db.views.len() && db.views.len() > 1 {
                                        db.views.remove(ix);
                                    }
                                });
                                this.set_view(ix.saturating_sub(1), cx);
                            },
                        )),
                    );
                }
            }
            Popup::AddView => {
                menu = menu.child(menu_label("New view", t));
                for (i, kind) in ViewKind::ALL.into_iter().enumerate() {
                    menu = menu.child(
                        menu_item(("av", i), Some(view_icon(kind)), kind.label(), t)
                            .on_click(cx.listener(move |this, _, _, cx| this.add_view(kind, cx))),
                    );
                }
            }
            Popup::PropMenu(name) => {
                let prop = db.prop(name)?;
                menu = menu.child(self.name_field(t));
                if prop.kind != PropKind::Title {
                    menu = menu.child(menu_label("Type", t));
                    for (i, kind) in PropKind::ALL.into_iter().enumerate() {
                        let n = name.clone();
                        let mut item = menu_item(("pm-kind", i), Some(prop_icon(kind)), kind.label(), t)
                            .on_click(cx.listener(move |this, _, _, cx| this.set_prop_kind(&n, kind, cx)));
                        if let Some(c) = check(prop.kind == kind) {
                            item = item.child(c);
                        }
                        menu = menu.child(item);
                    }
                }
                menu = menu.child(menu_separator(t));
                for (id, icon_name, label, desc) in
                    [("pm-asc", "arrow-up", "Sort ascending", false), ("pm-desc", "arrow-down", "Sort descending", true)]
                {
                    let n = name.clone();
                    menu = menu.child(menu_item(id, Some(icon_name), label, t).on_click(cx.listener(
                        move |this, _, _, cx| {
                            let n = n.clone();
                            this.update_view(cx, |v| v.sorts = vec![Sort { prop: n, desc }]);
                            this.close_popup(cx);
                        },
                    )));
                }
                let n = name.clone();
                let kind = prop.kind;
                menu = menu.child(menu_item("pm-filter", Some("filter"), "Filter", t).on_click(cx.listener(
                    move |this, ev: &gpui::ClickEvent, window, cx| {
                        let n = n.clone();
                        this.add_filter(n, kind, ev.position(), window, cx);
                    },
                )));
                if prop.kind != PropKind::Title {
                    let n = name.clone();
                    menu = menu.child(menu_item("pm-hide", Some("eye-off"), "Hide in view", t).on_click(cx.listener(
                        move |this, _, _, cx| {
                            let n = n.clone();
                            this.update_view(cx, |v| v.hidden.push(n));
                            this.close_popup(cx);
                        },
                    )));
                    let n = name.clone();
                    menu = menu.child(menu_item("pm-del", Some("trash"), "Delete property", t).on_click(cx.listener(
                        move |this, _, _, cx| {
                            this.popup = None;
                            let id = this.db_id.clone();
                            this.store.update(cx, |s, cx| {
                                let touched = s.vault.delete_property(&id, &n);
                                s.mark_many(&touched, cx);
                            });
                            cx.notify();
                        },
                    )));
                }
            }
            Popup::AddProp => {
                menu = menu.child(menu_label("New property", t));
                for (i, kind) in PropKind::ALL.into_iter().enumerate() {
                    menu = menu.child(menu_item(("ap", i), Some(prop_icon(kind)), kind.label(), t).on_click(cx.listener(
                        move |this, ev: &gpui::ClickEvent, window, cx| {
                            let mut name = String::new();
                            this.update_db(cx, |db| {
                                name = db.unique_prop_name(kind.label());
                                db.schema.push(Property::new(name.clone(), kind));
                            });
                            this.popup = None;
                            this.open_popup(Popup::PropMenu(name), ev.position(), window, cx);
                        },
                    )));
                }
            }
            Popup::Filter => {
                menu = menu.w(px(340.));
                if view.filters.is_empty() {
                    menu = menu.child(menu_label("Add a filter", t));
                } else {
                    menu = menu.child(menu_label("Filters", t));
                }
                for (i, f) in view.filters.iter().enumerate() {
                    let kind = db.prop(&f.prop).map(|p| p.kind).unwrap_or(PropKind::Text);
                    let hover = t.hover;
                    let value: AnyElement = if !f.op.needs_value() {
                        div().flex_1().into_any_element()
                    } else if self.editing_filter == Some(i) {
                        div()
                            .flex_1()
                            .min_w_0()
                            .px(px(6.))
                            .py(px(2.))
                            .rounded(px(4.))
                            .bg(t.hover)
                            .border_1()
                            .border_color(t.accent)
                            .child(self.input.clone())
                            .into_any_element()
                    } else {
                        let fv = f.value.clone();
                        div()
                            .id(("f-val", i))
                            .flex_1()
                            .min_w_0()
                            .px(px(6.))
                            .py(px(2.))
                            .rounded(px(4.))
                            .border_1()
                            .border_color(t.border)
                            .cursor_text()
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .text_color(if f.value.is_empty() { t.placeholder } else { t.text })
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.editing_filter = Some(i);
                                let fv = fv.clone();
                                this.input.update(cx, |inp, cx| {
                                    inp.placeholder = "Value".into();
                                    inp.set_text(fv, cx)
                                });
                                this.input.read(cx).focus(window);
                                cx.notify();
                            }))
                            .child(SharedString::from(if f.value.is_empty() { "Value".to_string() } else { f.value.clone() }))
                            .into_any_element()
                    };
                    menu = menu.child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .min_h(px(32.))
                            .px(px(10.))
                            .child(
                                div()
                                    .flex_none()
                                    .max_w(px(90.))
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(SharedString::from(f.prop.clone())),
                            )
                            .child(
                                div()
                                    .id(("f-op", i))
                                    .flex_none()
                                    .px(px(6.))
                                    .py(px(2.))
                                    .rounded(px(4.))
                                    .text_color(t.text_muted)
                                    .cursor_pointer()
                                    .hover(move |s| s.bg(hover))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.update_view(cx, |v| {
                                            if let Some(f) = v.filters.get_mut(i) {
                                                let ops = FilterOp::for_kind(kind);
                                                let at = ops.iter().position(|o| *o == f.op).unwrap_or(0);
                                                f.op = ops[(at + 1) % ops.len()];
                                            }
                                        })
                                    }))
                                    .child(short_op(f.op)),
                            )
                            .child(value)
                            .child(icon_button(("f-del", i), "x", t).on_click(cx.listener(move |this, _, _, cx| {
                                this.editing_filter = None;
                                this.update_view(cx, |v| {
                                    if i < v.filters.len() {
                                        v.filters.remove(i);
                                    }
                                })
                            }))),
                    );
                }
                if !view.filters.is_empty() {
                    menu = menu.child(menu_separator(t)).child(menu_label("Add a filter", t));
                }
                for (i, p) in db.schema.iter().enumerate() {
                    let (n, kind) = (p.name.clone(), p.kind);
                    menu = menu.child(menu_item(("f-add", i), Some(prop_icon(p.kind)), p.name.clone(), t).on_click(
                        cx.listener(move |this, ev: &gpui::ClickEvent, window, cx| {
                            this.add_filter(n.clone(), kind, ev.position(), window, cx)
                        }),
                    ));
                }
            }
            Popup::Sort => {
                menu = menu.w(px(300.));
                menu = menu.child(menu_label(if view.sorts.is_empty() { "Sort by" } else { "Sorts" }, t));
                for (i, s) in view.sorts.iter().enumerate() {
                    let hover = t.hover;
                    menu = menu.child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .h(px(32.))
                            .px(px(10.))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(SharedString::from(s.prop.clone())),
                            )
                            .child(
                                div()
                                    .id(("s-dir", i))
                                    .flex()
                                    .items_center()
                                    .gap(px(4.))
                                    .px(px(6.))
                                    .py(px(2.))
                                    .rounded(px(4.))
                                    .text_color(t.text_muted)
                                    .cursor_pointer()
                                    .hover(move |s| s.bg(hover))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.update_view(cx, |v| {
                                            if let Some(s) = v.sorts.get_mut(i) {
                                                s.desc = !s.desc;
                                            }
                                        })
                                    }))
                                    .child(icon(if s.desc { "arrow-down" } else { "arrow-up" }, 14., t.text_muted))
                                    .child(if s.desc { "Descending" } else { "Ascending" }),
                            )
                            .child(icon_button(("s-del", i), "x", t).on_click(cx.listener(move |this, _, _, cx| {
                                this.update_view(cx, |v| {
                                    if i < v.sorts.len() {
                                        v.sorts.remove(i);
                                    }
                                })
                            }))),
                    );
                }
                if !view.sorts.is_empty() {
                    menu = menu.child(menu_separator(t)).child(menu_label("Add a sort", t));
                }
                for (i, p) in db.schema.iter().enumerate() {
                    if view.sorts.iter().any(|s| s.prop == p.name) {
                        continue;
                    }
                    let n = p.name.clone();
                    menu = menu.child(menu_item(("s-add", i), Some(prop_icon(p.kind)), p.name.clone(), t).on_click(
                        cx.listener(move |this, _, _, cx| {
                            let n = n.clone();
                            this.update_view(cx, |v| v.sorts.push(Sort { prop: n, desc: false }))
                        }),
                    ));
                }
            }
            Popup::Props => {
                menu = menu.child(menu_label("Properties shown in this view", t));
                for (i, p) in db.schema.iter().enumerate() {
                    let hidden = view.hidden.contains(&p.name);
                    let n = p.name.clone();
                    let is_title = p.kind == PropKind::Title;
                    let mut item = menu_item(("pv", i), Some(prop_icon(p.kind)), p.name.clone(), t);
                    if !is_title {
                        item = item
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let n = n.clone();
                                this.update_view(cx, |v| {
                                    if v.hidden.contains(&n) {
                                        v.hidden.retain(|h| h != &n);
                                    } else {
                                        v.hidden.push(n);
                                    }
                                })
                            }))
                            .child(icon(if hidden { "eye-off" } else { "eye" }, 15., if hidden { t.text_faint } else { t.text_muted }));
                    }
                    menu = menu.child(item);
                }
                menu = menu.child(menu_separator(t)).child(menu_item("pv-add", Some("plus"), "New property", t).on_click(
                    cx.listener(|this, ev: &gpui::ClickEvent, window, cx| {
                        this.open_popup(Popup::AddProp, ev.position(), window, cx)
                    }),
                ));
            }
            Popup::RowMenu(id) => {
                let (a, b, c) = (id.clone(), id.clone(), id.clone());
                menu = menu
                    .child(menu_item("rm-open", Some("arrow-up-right"), "Open", t).on_click(cx.listener(
                        move |this, _, _, cx| {
                            this.popup = None;
                            cx.emit(DbEvent::Open(a.clone()))
                        },
                    )))
                    .child(menu_item("rm-dup", Some("copy"), "Duplicate", t).on_click(cx.listener(
                        move |this, _, _, cx| {
                            this.popup = None;
                            this.store.update(cx, |s, cx| {
                                s.duplicate_page(&b, cx);
                            });
                            cx.notify();
                        },
                    )))
                    .child(menu_item("rm-del", Some("trash"), "Delete", t).on_click(cx.listener(
                        move |this, _, _, cx| {
                            this.popup = None;
                            this.store.update(cx, |s, cx| s.trash(&c, cx));
                            cx.notify();
                        },
                    )));
            }
        }
        Some(popover(
            *pos,
            Corner::TopLeft,
            menu,
            cx.listener(|this, _, _, cx| this.close_popup(cx)),
        ))
    }

    fn add_filter(&mut self, prop: String, kind: PropKind, pos: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let op = FilterOp::for_kind(kind)[0];
        let mut ix = 0;
        self.update_view(cx, |v| {
            v.filters.push(Filter { prop, op, value: String::new() });
            ix = v.filters.len() - 1;
        });
        let pos = self.popup.as_ref().filter(|p| p.0 == Popup::Filter).map(|p| p.1).unwrap_or(pos);
        self.close_popup(cx);
        self.popup = Some((Popup::Filter, pos));
        if op.needs_value() {
            self.editing_filter = Some(ix);
            self.input.update(cx, |i, cx| {
                i.placeholder = "Value".into();
                i.set_text("", cx)
            });
            self.input.read(cx).focus(window);
        }
        cx.notify();
    }
}

/// The value a board card gets when moved from column `from` to column `to`.
/// Multi-select keeps its other tags: only the source column's tag is swapped.
fn moved_value(row: Option<&Page>, prop: &str, kind: PropKind, from: Option<&str>, to: Option<&str>) -> Option<PropValue> {
    if kind != PropKind::MultiSelect {
        return to.map(|t| PropValue::Text(t.to_string()));
    }
    let mut list = row.and_then(|r| r.props.get(prop)).map(|v| v.as_list()).unwrap_or_default();
    if let Some(from) = from {
        list.retain(|v| v != from);
    }
    if let Some(to) = to {
        if !list.iter().any(|v| v == to) {
            list.insert(0, to.to_string());
        }
    }
    (!list.is_empty()).then_some(PropValue::List(list))
}

fn short_op(op: FilterOp) -> &'static str {
    match op {
        FilterOp::Contains => "contains",
        FilterOp::NotContains => "doesn't contain",
        FilterOp::Is => "is",
        FilterOp::IsNot => "is not",
        FilterOp::IsEmpty => "is empty",
        FilterOp::IsNotEmpty => "is not empty",
        FilterOp::Gt => ">",
        FilterOp::Lt => "<",
        FilterOp::Checked => "is checked",
        FilterOp::Unchecked => "is unchecked",
    }
}

impl Render for DatabaseView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        if !cx.has_active_drag() {
            self.dragging = None;
            self.board_drop = None;
            self.row_drop = None;
            self.col_resize = None;
        }
        let active = self.view_ix(cx);
        let store = self.store.clone();
        let cx: &Context<Self> = cx;
        let s = store.read(cx);
        let Some(db) = s.page(&self.db_id).and_then(|p| p.database.as_ref()) else {
            return div().into_any_element();
        };
        let Some(view) = db.views.get(active) else {
            return div().into_any_element();
        };
        let rows = query(db, view, s.vault.rows(&self.db_id));
        let body = match view.kind {
            ViewKind::Table => self.render_table(db, view, &rows, &t, cx),
            ViewKind::Board => self.render_board(db, view, &rows, &t, cx),
            ViewKind::List => self.render_list(db, view, &rows, &t, cx),
            ViewKind::Gallery => self.render_gallery(db, view, &rows, &t, cx),
            ViewKind::Calendar => self.render_calendar(db, view, &rows, &t, cx),
        };
        let mut root = div()
            .flex()
            .flex_col()
            .w_full()
            .text_size(px(14.))
            .line_height(px(20.))
            .text_color(t.text)
            .child(self.render_tabs(db, active, &t, cx))
            .child(body);
        if let Some(p) = self.render_popup(db, active, &t, cx) {
            root = root.child(p);
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
