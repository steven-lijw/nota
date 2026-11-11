//! Database property values: how they are drawn, and the popover used to edit them.

use crate::store::Store;
use crate::text_input::{InputEvent, TextInput};
use crate::theme::{theme, Theme};
use crate::ui::*;
use chrono::Datelike;
use gpui::{
    div, prelude::*, px, AnyElement, Context, ElementId, Entity, EventEmitter, FontWeight, MouseButton, Pixels,
    Point, SharedString, Subscription, Window,
};
use nota_core::query::{add_months, date_to_string, format_date, month_grid, month_name, parse_date, today};
use nota_core::{format_number, Page, PropKind, PropValue, Property};

pub fn eid(prefix: &str, key: &str) -> ElementId {
    ElementId::Name(format!("{prefix}-{key}").into())
}

pub fn prop_icon(kind: PropKind) -> &'static str {
    match kind {
        PropKind::Title => "title",
        PropKind::Text => "text",
        PropKind::Number => "hash",
        PropKind::Select => "select",
        PropKind::MultiSelect => "list",
        PropKind::Status => "status",
        PropKind::Date => "calendar",
        PropKind::Checkbox => "checkbox",
        PropKind::Url => "link",
    }
}

/// Draw a property value the way Notion shows it in cells and on cards.
/// Returns `None` when there is nothing to show.
pub fn prop_value(row: &Page, prop: &Property, t: &Theme) -> Option<AnyElement> {
    if prop.kind == PropKind::Title {
        return Some(
            div()
                .font_weight(FontWeight::MEDIUM)
                .whitespace_nowrap()
                .overflow_hidden()
                .text_ellipsis()
                .child(SharedString::from(row.display_title().to_string()))
                .into_any_element(),
        );
    }
    let value = row.props.get(&prop.name);
    if prop.kind == PropKind::Checkbox {
        return Some(checkbox(value.is_some_and(|v| v.as_bool()), t).into_any_element());
    }
    let value = value.filter(|v| !v.is_empty())?;
    let text = |s: String| {
        div()
            .whitespace_nowrap()
            .overflow_hidden()
            .text_ellipsis()
            .child(SharedString::from(s))
    };
    Some(match prop.kind {
        PropKind::Select => pill(value.as_text(), prop.option_color(&value.as_text()), t).into_any_element(),
        PropKind::Status => status_pill(value.as_text(), prop.option_color(&value.as_text()), t).into_any_element(),
        PropKind::MultiSelect => {
            let mut row = div().flex().items_center().gap(px(4.)).overflow_hidden();
            for name in value.as_list() {
                let color = prop.option_color(&name).to_string();
                row = row.child(pill(name, &color, t));
            }
            row.into_any_element()
        }
        PropKind::Date => text(format_date(&value.as_text())).into_any_element(),
        PropKind::Number => text(match value {
            PropValue::Number(n) => format_number(*n),
            v => v.as_text(),
        })
        .into_any_element(),
        PropKind::Url => text(value.as_text())
            .border_b_1()
            .border_color(t.border_strong)
            .into_any_element(),
        _ => text(value.as_text().replace('\n', " ")).into_any_element(),
    })
}

pub enum CellEvent {
    Close,
}

/// The popover that edits one property of one row.
pub struct CellEditor {
    store: Entity<Store>,
    db_id: String,
    row_id: String,
    prop: String,
    kind: PropKind,
    pub input: Entity<TextInput>,
    pub pos: Point<Pixels>,
    month: (i32, u32),
    _subs: Vec<Subscription>,
}

impl EventEmitter<CellEvent> for CellEditor {}

impl CellEditor {
    pub fn new(
        store: Entity<Store>,
        db_id: String,
        row_id: String,
        prop: String,
        pos: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let (kind, initial) = {
            let s = store.read(cx);
            let kind = s
                .page(&db_id)
                .and_then(|p| p.database.as_ref())
                .and_then(|d| d.prop(&prop))
                .map(|p| p.kind)
                .unwrap_or(PropKind::Text);
            let row = s.page(&row_id);
            let initial = match kind {
                PropKind::Title => row.map(|r| r.title.clone()).unwrap_or_default(),
                k if k.has_options() => String::new(),
                _ => row.and_then(|r| r.props.get(&prop)).map(|v| v.as_text()).unwrap_or_default(),
            };
            (kind, initial)
        };
        let month = parse_date(&initial).map(|d| (d.year(), d.month())).unwrap_or_else(|| {
            let d = today();
            (d.year(), d.month())
        });
        let input = cx.new(|cx| {
            let mut i = TextInput::new(window, cx);
            i.placeholder = match kind {
                PropKind::Date => "YYYY-MM-DD".into(),
                k if k.has_options() => "Search for an option…".into(),
                PropKind::Title => "Untitled".into(),
                _ => "Empty".into(),
            };
            i.multiline = kind == PropKind::Text;
            i.set_text(initial, cx);
            i.select_all_text(cx);
            i
        });
        input.read(cx).focus(window);
        let subs = vec![cx.subscribe(&input, |this: &mut Self, _, ev: &InputEvent, cx| match ev {
            InputEvent::Submit => {
                this.submit(cx);
            }
            InputEvent::Cancel => cx.emit(CellEvent::Close),
            InputEvent::Changed => {
                if matches!(this.kind, PropKind::Title | PropKind::Text | PropKind::Url) {
                    this.commit(cx);
                }
                cx.notify();
            }
            _ => {}
        })];
        CellEditor { store, db_id, row_id, prop, kind, input, pos, month, _subs: subs }
    }

    fn property(&self, cx: &Context<Self>) -> Option<Property> {
        self.store.read(cx).page(&self.db_id)?.database.as_ref()?.prop(&self.prop).cloned()
    }

    fn selected(&self, cx: &Context<Self>) -> Vec<String> {
        self.store
            .read(cx)
            .page(&self.row_id)
            .and_then(|r| r.props.get(&self.prop))
            .map(|v| v.as_list())
            .unwrap_or_default()
    }

    /// Write the text field back to the row (text-like kinds only).
    pub fn commit(&mut self, cx: &mut Context<Self>) {
        let text = self.input.read(cx).text().to_string();
        let (row, prop) = (self.row_id.clone(), self.prop.clone());
        match self.kind {
            PropKind::Title => self.store.update(cx, |s, cx| {
                if s.page(&row).is_some_and(|p| p.title != text) {
                    s.update_page(&row, cx, |p| p.title = text);
                }
            }),
            PropKind::Text | PropKind::Url => {
                let v = (!text.is_empty()).then_some(PropValue::Text(text));
                self.store.update(cx, |s, cx| {
                    if s.page(&row).map(|p| p.props.get(&prop).cloned()) != Some(v.clone()) {
                        s.set_prop(&row, &prop, v, cx)
                    }
                });
            }
            PropKind::Number => {
                let v = text.trim().replace(',', "").parse::<f64>().ok().map(PropValue::Number);
                if v.is_some() || text.trim().is_empty() {
                    self.store.update(cx, |s, cx| s.set_prop(&row, &prop, v, cx));
                }
            }
            PropKind::Date => {
                let t = text.trim();
                if t.is_empty() {
                    self.store.update(cx, |s, cx| s.set_prop(&row, &prop, None, cx));
                } else if let Some(d) = parse_date(t) {
                    self.store
                        .update(cx, |s, cx| s.set_prop(&row, &prop, Some(PropValue::Text(date_to_string(d))), cx));
                }
            }
            _ => {}
        }
    }

    fn submit(&mut self, cx: &mut Context<Self>) {
        if self.kind.has_options() {
            let q = self.input.read(cx).text().trim().to_string();
            if !q.is_empty() {
                let existing = self
                    .property(cx)
                    .and_then(|p| p.options.iter().find(|o| o.name.to_lowercase().contains(&q.to_lowercase())).cloned());
                let name = existing.map(|o| o.name).unwrap_or(q);
                self.pick(&name, cx);
                return;
            }
        } else {
            self.commit(cx);
        }
        cx.emit(CellEvent::Close);
    }

    fn pick(&mut self, name: &str, cx: &mut Context<Self>) {
        let (db, row, prop, kind) = (self.db_id.clone(), self.row_id.clone(), self.prop.clone(), self.kind);
        let name = name.to_string();
        let mut selected = self.selected(cx);
        self.store.update(cx, |s, cx| {
            let known = s
                .page(&db)
                .and_then(|p| p.database.as_ref())
                .and_then(|d| d.prop(&prop))
                .is_some_and(|p| p.options.iter().any(|o| o.name == name));
            if !known {
                s.update_page(&db, cx, |p| {
                    if let Some(pr) = p.database.as_mut().and_then(|d| d.prop_mut(&prop)) {
                        pr.ensure_option(&name);
                    }
                });
            }
            if kind == PropKind::MultiSelect {
                if selected.contains(&name) {
                    selected.retain(|n| n != &name);
                } else {
                    selected.push(name);
                }
                s.set_prop(&row, &prop, Some(PropValue::List(selected)), cx);
            } else if selected.first() == Some(&name) && kind == PropKind::Select {
                s.set_prop(&row, &prop, None, cx);
            } else {
                s.set_prop(&row, &prop, Some(PropValue::Text(name)), cx);
            }
        });
        self.input.update(cx, |i, cx| i.set_text("", cx));
        if kind != PropKind::MultiSelect {
            cx.emit(CellEvent::Close);
        }
        cx.notify();
    }

    fn set_date(&mut self, date: Option<chrono::NaiveDate>, cx: &mut Context<Self>) {
        let (row, prop) = (self.row_id.clone(), self.prop.clone());
        let v = date.map(|d| PropValue::Text(date_to_string(d)));
        self.store.update(cx, |s, cx| s.set_prop(&row, &prop, v, cx));
        cx.emit(CellEvent::Close);
    }
}

/// A compact month picker. `on_pick` receives the clicked day.
pub fn mini_calendar<V: 'static>(
    month: (i32, u32),
    selected: Option<chrono::NaiveDate>,
    t: &Theme,
    cx: &Context<V>,
    on_month: impl Fn(&mut V, (i32, u32), &mut Context<V>) + 'static + Clone,
    on_pick: impl Fn(&mut V, chrono::NaiveDate, &mut Context<V>) + 'static + Clone,
) -> AnyElement {
    let (y, m) = month;
    let now = today();
    let nav = |id: &'static str, name: &'static str, delta: i32, cx: &Context<V>| {
        let on_month = on_month.clone();
        icon_button(id, name, t).on_click(cx.listener(move |this, _, _, cx| {
            on_month(this, add_months(y, m, delta), cx);
            cx.notify();
        }))
    };
    let mut col = div().flex().flex_col().px(px(10.)).pb(px(6.)).child(
        div()
            .flex()
            .items_center()
            .h(px(32.))
            .child(
                div()
                    .flex_1()
                    .pl(px(4.))
                    .font_weight(FontWeight::MEDIUM)
                    .child(format!("{} {}", month_name(m), y)),
            )
            .child(nav("mc-prev", "chevron-left", -1, cx))
            .child(nav("mc-next", "chevron-right", 1, cx)),
    );
    let mut head = div().flex();
    for d in ["Su", "Mo", "Tu", "We", "Th", "Fr", "Sa"] {
        head = head.child(
            div()
                .w(px(32.))
                .h(px(24.))
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(12.))
                .text_color(t.text_faint)
                .child(d),
        );
    }
    col = col.child(head);
    for (w, week) in month_grid(y, m).chunks(7).enumerate() {
        let mut row = div().flex();
        for (i, day) in week.iter().enumerate() {
            let day = *day;
            let on_pick = on_pick.clone();
            let hover = t.hover;
            let mut cell = div()
                .id(("mc-day", w * 7 + i))
                .w(px(32.))
                .h(px(32.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(4.))
                .cursor_pointer()
                .text_size(px(14.))
                .on_click(cx.listener(move |this, _, _, cx| on_pick(this, day, cx)))
                .child(day.day().to_string());
            if Some(day) == selected {
                cell = cell.bg(t.accent).text_color(gpui::white());
            } else {
                cell = cell.hover(move |s| s.bg(hover));
                if day == now {
                    cell = cell.text_color(t.danger).font_weight(FontWeight::SEMIBOLD);
                } else if day.month() != m {
                    cell = cell.text_color(t.text_faint);
                }
            }
            row = row.child(cell);
        }
        col = col.child(row);
    }
    col.into_any_element()
}

impl Render for CellEditor {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let field = div()
            .mx(px(8.))
            .my(px(6.))
            .px(px(8.))
            .py(px(5.))
            .rounded(px(6.))
            .bg(t.hover)
            .border_1()
            .border_color(t.border)
            .child(self.input.clone());
        let mut surface = menu_surface(&t).w(px(280.)).on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation());
        match self.kind {
            PropKind::Date => {
                let selected = parse_date(self.input.read(cx).text());
                surface = surface
                    .child(field)
                    .child(mini_calendar(
                        self.month,
                        selected,
                        &t,
                        cx,
                        |this: &mut Self, m, _| this.month = m,
                        |this: &mut Self, day, cx| this.set_date(Some(day), cx),
                    ))
                    .child(menu_separator(&t))
                    .child(
                        menu_item("cell-clear", Some("x"), "Clear", &t)
                            .on_click(cx.listener(|this, _, _, cx| this.set_date(None, cx))),
                    );
            }
            k if k.has_options() => {
                let prop = self.property(cx);
                let selected = self.selected(cx);
                let q = self.input.read(cx).text().trim().to_lowercase();
                surface = surface.child(field).child(menu_label("Select an option or create one", &t));
                let mut exact = false;
                if let Some(prop) = &prop {
                    for (i, o) in prop.options.iter().enumerate() {
                        if o.name.to_lowercase() == q {
                            exact = true;
                        }
                        if !q.is_empty() && !o.name.to_lowercase().contains(&q) {
                            continue;
                        }
                        let name = o.name.clone();
                        let chip = if k == PropKind::Status {
                            status_pill(o.name.clone(), &o.color, &t)
                        } else {
                            pill(o.name.clone(), &o.color, &t)
                        };
                        let hover = t.hover;
                        let mut row = div()
                            .id(("cell-opt", i))
                            .flex()
                            .items_center()
                            .h(px(30.))
                            .mx(px(4.))
                            .px(px(8.))
                            .rounded(px(6.))
                            .cursor_pointer()
                            .hover(move |s| s.bg(hover))
                            .on_click(cx.listener(move |this, _, _, cx| this.pick(&name, cx)))
                            .child(div().flex_1().flex().child(chip));
                        if selected.contains(&o.name) {
                            row = row.child(icon("check", 14., t.text_muted));
                        }
                        surface = surface.child(row);
                    }
                }
                if !q.is_empty() && !exact {
                    let name = self.input.read(cx).text().trim().to_string();
                    let hover = t.hover;
                    surface = surface.child(
                        div()
                            .id("cell-create")
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .h(px(30.))
                            .mx(px(4.))
                            .px(px(8.))
                            .rounded(px(6.))
                            .cursor_pointer()
                            .hover(move |s| s.bg(hover))
                            .child(div().text_color(t.text_muted).child("Create"))
                            .child(pill(name.clone(), "default", &t))
                            .on_click(cx.listener(move |this, _, _, cx| this.pick(&name, cx))),
                    );
                }
                surface = surface.pb(px(6.));
            }
            _ => {
                surface = surface.child(field);
            }
        }
        surface
    }
}
