//! Rendering of the block editor: block rows, hover handles, the slash menu,
//! the block menu and the selection toolbar.

use crate::actions::*;
use crate::blocks_view::{block_text, bullet_glyph, marker_box, top_margin};
use crate::editor::{BlockMenu, DragBlock, Editor, EditorEvent, ResizeImage, Sel, INDENT_PX};
use crate::theme::PAGE_MAX_W;
use crate::text_view::{TextLayout, TextView};
use crate::theme::{theme, ui_font, Theme};
use crate::ui::*;
use gpui::{
    anchored, canvas, deferred, div, font, img, point, prelude::*, px, AnyElement, Context, Corner,
    DispatchPhase, DragMoveEvent, ElementInputHandler, ExternalPaths, FontWeight, Hsla, MouseButton,
    MouseMoveEvent, MouseUpEvent, SharedString, TextRun, Window,
};
use nota_core::{list_number, visibility, Block, BlockKind, MarkKind};
use std::collections::HashSet;

/// Rows extend this far into the page margins so the hover handles stay
/// inside the row's hit area.
pub const GUTTER: f32 = 50.0;
const INDENT: f32 = INDENT_PX;
const BASE: f32 = 16.0;

pub struct DragPreview {
    label: SharedString,
}

/// Nothing follows the pointer while an image is resized.
pub struct NoPreview;

impl Render for NoPreview {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

impl Render for DragPreview {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        div()
            .px(px(8.))
            .py(px(4.))
            .max_w(px(360.))
            .rounded(px(4.))
            .bg(t.panel_bg)
            .border_1()
            .border_color(t.border)
            .shadow(card_shadow(&t))
            .text_size(px(14.))
            .text_color(t.text)
            .font_family(ui_font())
            .whitespace_nowrap()
            .overflow_hidden()
            .text_ellipsis()
            .opacity(0.85)
            .child(self.label.clone())
    }
}

fn placeholder_for(kind: &BlockKind, focused: bool) -> Option<&'static str> {
    match kind {
        BlockKind::Paragraph => focused.then_some("Write, press '/' for commands…"),
        BlockKind::Heading(1) => Some("Heading 1"),
        BlockKind::Heading(2) => Some("Heading 2"),
        BlockKind::Heading(_) => Some("Heading 3"),
        BlockKind::Bullet | BlockKind::Numbered => Some("List"),
        BlockKind::Todo { .. } => Some("To-do"),
        BlockKind::Toggle { .. } => Some("Toggle"),
        BlockKind::Quote => Some("Empty quote"),
        BlockKind::Callout { .. } => Some("Type something…"),
        _ => None,
    }
}

impl Editor {
    fn scroll_caret_into_view(&mut self, layout: &TextLayout, cx: &mut Context<Self>) {
        if !self.scroll_pending {
            return;
        }
        self.scroll_pending = false;
        let caret = layout.caret_bounds(self.cursor());
        let viewport = self.scroll.bounds();
        if viewport.size.height <= px(0.) {
            return;
        }
        let margin = px(56.);
        let mut offset = self.scroll.offset();
        if caret.bottom() + margin > viewport.bottom() {
            offset.y -= caret.bottom() + margin - viewport.bottom();
        } else if caret.top() - margin < viewport.top() {
            offset.y += viewport.top() - (caret.top() - margin);
            if offset.y > px(0.) {
                offset.y = px(0.);
            }
        } else {
            return;
        }
        self.scroll.set_offset(offset);
        cx.notify();
    }

    /// Widest an image may be in this block's column.
    fn column_width(&self, ix: usize) -> f32 {
        let b = &self.blocks[ix];
        let indent = b.indent as f32 * INDENT;
        self.row_bounds
            .borrow()
            .get(&b.id)
            .map(|r| r.size.width / px(1.) - 2. * GUTTER - indent)
            .unwrap_or(PAGE_MAX_W - indent)
            .max(80.)
    }

    fn render_image(&self, ix: usize, src: &str, width: Option<u32>, scale: f32, t: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let id = self.blocks[ix].id;
        let path = self.store.read(cx).vault.resolve_src(&self.page_id, src);
        let Some(path) = path.filter(|p| p.is_file()) else {
            return div()
                .flex()
                .flex_1()
                .items_center()
                .gap(px(8.))
                .h(px(44.))
                .px(px(12.))
                .rounded(px(4.))
                .bg(t.hover)
                .text_size(px(14.))
                .text_color(t.text_faint)
                .child(icon("image", 16., t.text_faint))
                .child(SharedString::from(format!("Image not found: {src}")))
                .into_any_element();
        };
        let max = self.column_width(ix);
        let natural = self.image_size(&path).map(|(w, _)| w as f32 / scale.max(1.));
        let w = width.map(|w| w as f32).or(natural).unwrap_or(max).min(max).max(48.);
        let group: SharedString = format!("img-{id}").into();
        let stop = |_: &gpui::MouseDownEvent, _: &mut Window, cx: &mut gpui::App| cx.stop_propagation();
        let handle = |side: f32| {
            let mut h = div()
                .id(if side < 0. { ("img-l", id as usize) } else { ("img-r", id as usize) })
                .absolute()
                .top_0()
                .bottom_0()
                .w(px(16.))
                .flex()
                .items_center()
                .justify_center()
                .cursor_ew_resize()
                .opacity(0.)
                .group_hover(group.clone(), |s| s.opacity(1.))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, ev: &gpui::MouseDownEvent, _, cx| {
                        cx.stop_propagation();
                        this.start_resize(id, ev.position.x, w);
                    }),
                )
                .on_drag(ResizeImage { id, side }, |_, _, _, cx| cx.new(|_| NoPreview))
                .child(
                    div()
                        .w(px(6.))
                        .h(px(48.).min(px(w * 0.4)))
                        .rounded(px(3.))
                        .bg(Hsla { a: 0.6, ..gpui::black() })
                        .border_1()
                        .border_color(Hsla { a: 0.9, ..gpui::white() }),
                );
            h = if side < 0. { h.left(px(2.)) } else { h.right(px(2.)) };
            h
        };
        let faint = t.text_faint;
        div()
            .flex()
            .flex_1()
            .min_w_0()
            .justify_center()
            .py(px(4.))
            .child(
                div()
                    .id(("img", id as usize))
                    .group(group.clone())
                    .relative()
                    .w(px(w))
                    .on_mouse_down(MouseButton::Right, stop)
                    .child(
                        img(path)
                            .w(px(w))
                            .rounded(px(3.))
                            .with_fallback(move || {
                                div()
                                    .h(px(44.))
                                    .flex()
                                    .items_center()
                                    .text_color(faint)
                                    .child("This image could not be shown")
                                    .into_any_element()
                            }),
                    )
                    .child(handle(-1.))
                    .child(handle(1.)),
            )
            .into_any_element()
    }

    fn render_block(&self, ix: usize, last_visible: bool, focused: bool, scale: f32, t: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let block: &Block = &self.blocks[ix];
        let id = block.id;
        let bt = block_text(block, t, BASE);
        let lh = bt.line_height;
        let is_cursor_block = focused && self.block_sel.is_none() && self.sel.block == ix;
        let in_block_sel = self
            .block_sel_range()
            .is_some_and(|r| r.contains(&ix));
        let entity = cx.entity();
        let layouts = self.layouts.clone();
        let indent_px = block.indent as f32 * INDENT;
        let group: SharedString = format!("blk-{}", id).into();

        // --- the text element ------------------------------------------------
        let mut text_view = TextView::new(bt.text.clone(), bt.runs.clone(), bt.font_size, lh)
            .highlights(bt.highlights.clone())
            .on_layout({
                let entity = entity.clone();
                move |layout, _window, cx| {
                    layouts.borrow_mut().insert(id, layout.clone());
                    if is_cursor_block {
                        entity.update(cx, |this, cx| this.scroll_caret_into_view(&layout, cx));
                    }
                }
            });
        if is_cursor_block {
            let focus = self.focus.clone();
            let entity = entity.clone();
            text_view = text_view
                .selection(Some(self.sel.range.clone()), t.selection)
                .cursor(self.sel.range.is_empty().then(|| self.cursor()), t.text)
                .marked(self.marked.clone())
                .input(move |bounds, window, cx| {
                    window.handle_input(&focus, ElementInputHandler::new(bounds, entity), cx);
                });
        }
        if let Some(ph) = placeholder_for(&block.kind, is_cursor_block) {
            let mut f = font(ui_font());
            if matches!(block.kind, BlockKind::Heading(_)) {
                f.weight = FontWeight::SEMIBOLD;
            }
            text_view = text_view.placeholder(
                ph,
                TextRun {
                    len: 0,
                    font: f,
                    color: t.placeholder,
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                },
            );
        }
        let pad_y = 3.0;
        let text = div()
            .flex_1()
            .min_w_0()
            .py(px(pad_y))
            .px(px(2.))
            .cursor_text()
            .child(text_view);

        let stop = |_: &gpui::MouseDownEvent, _: &mut Window, cx: &mut gpui::App| cx.stop_propagation();

        // --- kind-specific content -------------------------------------------
        let mut handle_offset = 0.0;
        let content: AnyElement = match &block.kind {
            BlockKind::Bullet => div()
                .flex()
                .flex_1()
                .min_w_0()
                .child(
                    marker_box(lh)
                        .mt(px(pad_y))
                        .text_size(px(20.))
                        .line_height(lh)
                        .child(bullet_glyph(block.indent)),
                )
                .child(text)
                .into_any_element(),
            BlockKind::Numbered => div()
                .flex()
                .flex_1()
                .min_w_0()
                .child(
                    marker_box(lh)
                        .mt(px(pad_y))
                        .justify_end()
                        .pr(px(4.))
                        .child(format!("{}.", list_number(&self.blocks, ix))),
                )
                .child(text)
                .into_any_element(),
            BlockKind::Todo { checked } => div()
                .flex()
                .flex_1()
                .min_w_0()
                .child(
                    marker_box(lh).mt(px(pad_y)).child(
                        div()
                            .id(("todo", id as usize))
                            .cursor_pointer()
                            .on_mouse_down(MouseButton::Left, stop)
                            .on_click(cx.listener(move |this, _, _, cx| this.toggle_todo(ix, cx)))
                            .child(checkbox(*checked, t)),
                    ),
                )
                .child(text)
                .into_any_element(),
            BlockKind::Toggle { open } => {
                let hover = t.hover;
                div()
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .child(
                        marker_box(lh).mt(px(pad_y)).child(
                            div()
                                .id(("toggle", id as usize))
                                .flex()
                                .items_center()
                                .justify_center()
                                .size(px(22.))
                                .rounded(px(4.))
                                .cursor_pointer()
                                .hover(move |s| s.bg(hover))
                                .on_mouse_down(MouseButton::Left, stop)
                                .on_click(cx.listener(move |this, _, _, cx| this.toggle_open(ix, cx)))
                                .child(icon(
                                    if *open { "triangle-down" } else { "triangle-right" },
                                    13.,
                                    t.text,
                                )),
                        ),
                    )
                    .child(text)
                    .into_any_element()
            }
            BlockKind::Quote => div()
                .flex_1()
                .min_w_0()
                .my(px(2.))
                .border_l(px(3.))
                .border_color(t.text)
                .pl(px(12.))
                .child(text)
                .into_any_element(),
            BlockKind::Callout { icon: emoji } => {
                handle_offset = 12.0;
                let hover = t.hover;
                div()
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .gap(px(6.))
                    .py(px(12.))
                    .pl(px(12.))
                    .pr(px(16.))
                    .rounded(px(6.))
                    .bg(t.callout_bg)
                    .child(
                        div()
                            .id(("callout", id as usize))
                            .flex()
                            .flex_none()
                            .items_center()
                            .justify_center()
                            .size(px(26.))
                            .mt(px(pad_y - 1.))
                            .rounded(px(4.))
                            .text_size(px(18.))
                            .cursor_pointer()
                            .hover(move |s| s.bg(hover))
                            .on_mouse_down(MouseButton::Left, stop)
                            .on_click(cx.listener(move |this, _, _, cx| this.cycle_callout_icon(ix, cx)))
                            .child(SharedString::from(emoji.clone())),
                    )
                    .child(text)
                    .into_any_element()
            }
            BlockKind::Code { lang } => {
                handle_offset = if lang.is_empty() { 16.0 } else { 30.0 };
                let mut code = div()
                    .flex_1()
                    .min_w_0()
                    .rounded(px(6.))
                    .bg(t.codeblock_bg)
                    .px(px(16.))
                    .py(px(16.));
                if !lang.is_empty() {
                    code = code.pt(px(8.)).child(
                        div()
                            .text_size(px(12.))
                            .text_color(t.text_faint)
                            .pb(px(6.))
                            .child(SharedString::from(lang.clone())),
                    );
                }
                code.child(text).into_any_element()
            }
            BlockKind::Divider => div()
                .flex()
                .flex_1()
                .items_center()
                .h(px(13.))
                .child(div().w_full().h(px(1.)).bg(t.border_strong))
                .into_any_element(),
            BlockKind::PageLink { page } => {
                let store = self.store.read(cx);
                let target = store.page(page).filter(|_| !store.vault.is_trashed(page));
                let hover = t.hover;
                match target {
                    Some(p) => {
                        let page_id = page.clone();
                        div()
                            .id(("plink", id as usize))
                            .flex()
                            .flex_1()
                            .min_w_0()
                            .items_center()
                            .gap(px(4.))
                            .h(px(30.))
                            .px(px(2.))
                            .rounded(px(4.))
                            .cursor_pointer()
                            .hover(move |s| s.bg(hover))
                            .on_mouse_down(MouseButton::Left, stop)
                            .on_click(cx.listener(move |_, _, _, cx| {
                                cx.emit(EditorEvent::OpenPage(page_id.clone()));
                            }))
                            .child(page_icon(p.icon.as_deref(), p.is_database(), 16., t))
                            .child(
                                div()
                                    .font_weight(FontWeight::MEDIUM)
                                    .border_b_1()
                                    .border_color(t.border_strong)
                                    .line_height(px(22.))
                                    .whitespace_nowrap()
                                    .overflow_hidden()
                                    .child(SharedString::from(p.display_title().to_string())),
                            )
                            .into_any_element()
                    }
                    None => div()
                        .flex()
                        .flex_1()
                        .items_center()
                        .gap(px(6.))
                        .h(px(30.))
                        .px(px(2.))
                        .text_color(t.text_faint)
                        .child(icon("page-empty", 16., t.text_faint))
                        .child("Deleted page")
                        .into_any_element(),
                }
            }
            BlockKind::Image { src, width } => {
                let (src, width) = (src.clone(), *width);
                self.render_image(ix, &src, width, scale, t, cx)
            }
            _ => text.into_any_element(),
        };

        // --- hover handles -----------------------------------------------------
        let hover = t.hover;
        let label: String = block.text.text.chars().take(60).collect();
        let drag_count = {
            let r = self.moving_range(ix);
            if r.len() > 1 && self.block_sel_range().is_some_and(|s| s.contains(&ix)) { r.len() } else { 1 }
        };
        let handle_top = pad_y + handle_offset + ((lh / px(1.)) - 24.0) / 2.0;
        let handles = div()
            .absolute()
            .left(px(GUTTER + indent_px - 44.))
            .top(px(handle_top))
            .flex()
            .items_center()
            .opacity(0.)
            .group_hover(group.clone(), |s| s.opacity(1.))
            .child(
                div()
                    .id(("add", id as usize))
                    .flex()
                    .items_center()
                    .justify_center()
                    .w(px(22.))
                    .h(px(24.))
                    .rounded(px(4.))
                    .cursor_pointer()
                    .hover(move |s| s.bg(hover))
                    .on_mouse_down(MouseButton::Left, stop)
                    .on_click(cx.listener(move |this, _, window, cx| this.add_block_below(ix, window, cx)))
                    .child(icon("plus", 16., t.text_faint)),
            )
            .child(
                div()
                    .id(("grip", id as usize))
                    .flex()
                    .items_center()
                    .justify_center()
                    .w(px(18.))
                    .h(px(24.))
                    .rounded(px(4.))
                    .cursor_pointer()
                    .hover(move |s| s.bg(hover))
                    .on_mouse_down(MouseButton::Left, stop)
                    .on_click(cx.listener(move |this, ev: &gpui::ClickEvent, _, cx| {
                        this.block_sel = Some((ix, ix));
                        this.block_menu = Some(BlockMenu {
                            block: ix,
                            pos: ev.position(),
                        });
                        cx.notify();
                    }))
                    .on_drag(
                        DragBlock {
                            id,
                            count: drag_count,
                            label: if label.is_empty() {
                                block.kind.label().to_string()
                            } else {
                                label
                            },
                        },
                        |drag: &DragBlock, _, _, cx| {
                            let label: SharedString = if drag.count > 1 {
                                format!("{} blocks", drag.count).into()
                            } else {
                                drag.label.clone().into()
                            };
                            cx.new(|_| DragPreview { label })
                        },
                    )
                    .child(icon("grip", 14., t.text_faint)),
            );

        let accent = t.accent;
        let mt = top_margin(&block.kind, ix == 0, BASE);
        let rows = self.row_bounds.clone();
        let mut row = div()
            .id(("block", id as usize))
            .group(group.clone())
            .relative()
            .flex()
            .items_start()
            .w_full()
            .pl(px(GUTTER + indent_px))
            .pr(px(GUTTER))
            .mt(px(mt))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, ev, window, cx| this.mouse_down_on_block(ix, ev, window, cx)),
            )
            .child(
                canvas(move |bounds, _, _| {
                    rows.borrow_mut().insert(id, bounds);
                }, |_, _, _, _| {})
                .absolute()
                .top_0()
                .left_0()
                .size_full(),
            );
        if self.drag_range.as_ref().is_some_and(|r| r.contains(&ix)) {
            row = row.opacity(0.4);
        }
        if in_block_sel {
            row = row.child(
                div()
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .left(px(GUTTER + indent_px - 2.))
                    .right(px(GUTTER - 2.))
                    .rounded(px(3.))
                    .bg(t.block_selection),
            );
        }
        row = row.child(handles).child(content);
        if let Some(target) = self.drop_target {
            let line = |indent: u8| {
                div()
                    .absolute()
                    .left(px(GUTTER + indent as f32 * INDENT))
                    .right(px(GUTTER))
                    .h(px(3.))
                    .rounded(px(1.5))
                    .bg(accent)
            };
            if target.gap == ix {
                row = row.child(line(target.indent).top(px(-(mt / 2.0) - 2.)));
            } else if target.gap >= self.blocks.len() && last_visible {
                row = row.child(line(target.indent).bottom(px(-3.)));
            }
        }
        row.into_any_element()
    }

    fn render_marquee(&self, t: &Theme) -> Option<AnyElement> {
        let m = self.marquee.as_ref().filter(|m| m.active)?;
        let origin = self.marquee_origin()?;
        let (l, r) = (origin.x.min(m.current.x), origin.x.max(m.current.x));
        let (top, bottom) = (origin.y.min(m.current.y), origin.y.max(m.current.y));
        let tint = |a: f32| Hsla { a, ..t.accent };
        Some(
            deferred(
                anchored().position(point(l, top)).snap_to_window().child(
                    div()
                        .w(r - l)
                        .h(bottom - top)
                        .rounded(px(2.))
                        .bg(tint(0.10))
                        .border_1()
                        .border_color(tint(0.40)),
                ),
            )
            .with_priority(1)
            .into_any_element(),
        )
    }

    fn render_slash_menu(&self, t: &Theme, cx: &mut Context<Self>) -> Option<AnyElement> {
        let slash = self.slash.as_ref()?;
        let layout = self.layout_of(slash.block)?;
        let caret = layout.caret_bounds(self.cursor());
        let items = self.filtered_slash_items();
        let selected = slash.selected.min(items.len().saturating_sub(1));
        let mut menu = menu_surface(t).w(px(300.));
        menu = menu.child(menu_label("Basic blocks", t));
        if items.is_empty() {
            menu = menu.child(
                div()
                    .px(px(12.))
                    .py(px(6.))
                    .text_color(t.text_faint)
                    .child("No results"),
            );
        }
        for (i, item) in items.iter().enumerate() {
            let hover = t.hover;
            let glyph: AnyElement = if item.icon.starts_with('H') {
                div()
                    .text_size(px(12.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(t.text_muted)
                    .child(item.icon)
                    .into_any_element()
            } else {
                icon(item.icon, 16., t.text_muted).into_any_element()
            };
            let mut row = div()
                .id(("slash", i))
                .flex()
                .flex_none()
                .items_center()
                .gap(px(8.))
                .h(px(30.))
                .mx(px(4.))
                .px(px(8.))
                .rounded(px(6.))
                .cursor_pointer()
                .hover(move |s| s.bg(hover))
                .on_click(cx.listener(move |this, _, window, cx| this.apply_slash(i, window, cx)))
                .child(
                    div()
                        .flex()
                        .flex_none()
                        .items_center()
                        .justify_center()
                        .size(px(20.))
                        .child(glyph),
                )
                .child(div().flex_1().child(item.label))
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(t.text_faint)
                        .child(item.hint),
                );
            if i == selected {
                row = row.bg(t.hover);
            }
            menu = menu.child(row);
        }
        Some(popover(
            point(caret.left(), caret.bottom() + px(6.)),
            Corner::TopLeft,
            menu,
            cx.listener(|this, _, _, cx| {
                this.slash = None;
                cx.notify();
            }),
        ))
    }

    fn render_block_menu(&self, t: &Theme, cx: &mut Context<Self>) -> Option<AnyElement> {
        let menu_state = self.block_menu.as_ref()?;
        let ix = menu_state.block;
        if ix >= self.blocks.len() {
            return None;
        }
        let mut menu = menu_surface(t).w(px(230.));
        menu = menu
            .child(
                menu_item("bm-delete", Some("trash"), "Delete", t)
                    .on_click(cx.listener(move |this, _, _, cx| this.delete_block(ix, cx))),
            )
            .child(
                menu_item("bm-dup", Some("copy"), "Duplicate", t)
                    .on_click(cx.listener(move |this, _, _, cx| this.duplicate_block(ix, cx))),
            );
        if self.blocks[ix].kind.has_text() {
            menu = menu
                .child(menu_separator(t))
                .child(menu_label("Turn into", t));
            let kinds: Vec<(&str, &str, BlockKind)> = vec![
                ("text", "Text", BlockKind::Paragraph),
                ("H1", "Heading 1", BlockKind::Heading(1)),
                ("H2", "Heading 2", BlockKind::Heading(2)),
                ("H3", "Heading 3", BlockKind::Heading(3)),
                ("list", "Bulleted list", BlockKind::Bullet),
                ("numbered", "Numbered list", BlockKind::Numbered),
                ("checkbox", "To-do list", BlockKind::Todo { checked: false }),
                ("triangle-right", "Toggle list", BlockKind::Toggle { open: true }),
                ("quote", "Quote", BlockKind::Quote),
                ("callout", "Callout", BlockKind::Callout { icon: "💡".into() }),
                ("code", "Code", BlockKind::Code { lang: String::new() }),
            ];
            for (i, (icon_name, label, kind)) in kinds.into_iter().enumerate() {
                let item = if icon_name.starts_with('H') {
                    menu_item(("bm-turn", i), None, label, t)
                } else {
                    menu_item(("bm-turn", i), Some(icon_name), label, t)
                };
                menu = menu.child(item.on_click(cx.listener(move |this, _, _, cx| {
                    this.block_sel = None;
                    this.sel = Sel::caret(ix, 0);
                    this.turn_into(kind.clone(), cx);
                })));
            }
        }
        Some(popover(
            menu_state.pos,
            Corner::TopLeft,
            menu,
            cx.listener(|this, _, _, cx| {
                this.block_menu = None;
                cx.notify();
            }),
        ))
    }

    fn render_format_toolbar(&self, focused: bool, t: &Theme, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !focused
            || self.block_sel.is_some()
            || self.sel.range.is_empty()
            || self.selecting
            || self.slash.is_some()
            || self.marked.is_some()
            || matches!(self.cur().kind, BlockKind::Code { .. })
        {
            return None;
        }
        let layout = self.layout_of(self.sel.block)?;
        let rect = layout.rects_for_range(self.sel.range.clone()).into_iter().next()?;
        let mut bar = div()
            .flex()
            .items_center()
            .gap(px(1.))
            .p(px(3.))
            .bg(t.panel_bg)
            .border_1()
            .border_color(t.border)
            .rounded(px(8.))
            .shadow(popover_shadow(t));
        let buttons = [
            ("bold", MarkKind::Bold),
            ("italic", MarkKind::Italic),
            ("underline", MarkKind::Underline),
            ("strike", MarkKind::Strike),
            ("code", MarkKind::Code),
        ];
        for (i, (name, kind)) in buttons.into_iter().enumerate() {
            let active = self.selection_has(kind);
            let hover = t.hover;
            bar = bar.child(
                div()
                    .id(("fmt", i))
                    .flex()
                    .items_center()
                    .justify_center()
                    .size(px(28.))
                    .rounded(px(5.))
                    .cursor_pointer()
                    .hover(move |s| s.bg(hover))
                    .on_click(cx.listener(move |this, _, _, cx| this.toggle_mark(kind, cx)))
                    .child(icon(name, 16., if active { t.accent } else { t.text })),
            );
        }
        Some(
            deferred(
                anchored()
                    .position(point(rect.left(), rect.top() - px(6.)))
                    .anchor(Corner::BottomLeft)
                    .snap_to_window_with_margin(px(8.))
                    .child(
                        div()
                            .occlude()
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .child(bar),
                    ),
            )
            .with_priority(1)
            .into_any_element(),
        )
    }
}

impl Render for Editor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let focused = self.focus.is_focused(window);
        let scale = window.scale_factor();
        let vis = visibility(&self.blocks);

        // forget layouts of blocks that no longer exist
        {
            let ids: HashSet<u64> = self.blocks.iter().map(|b| b.id).collect();
            self.layouts.borrow_mut().retain(|id, _| ids.contains(id));
            // hidden rows (inside collapsed toggles) must not keep stale bounds
            let shown: HashSet<u64> = self.blocks.iter().zip(&vis).filter(|(_, v)| **v).map(|(b, _)| b.id).collect();
            self.row_bounds.borrow_mut().retain(|id, _| shown.contains(id));
        }
        self.clear_stale_drag(cx);

        let mut col = div()
            .id("editor")
            .key_context("Editor")
            .track_focus(&self.focus)
            .flex()
            .flex_col()
            .w_full()
            .text_size(px(BASE))
            .line_height(px(24.))
            .text_color(t.text)
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::delete_word_left))
            .on_action(cx.listener(Self::delete_to_line_start))
            .on_action(cx.listener(Self::enter))
            .on_action(cx.listener(Self::shift_enter))
            .on_action(cx.listener(Self::tab))
            .on_action(cx.listener(Self::shift_tab))
            .on_action(cx.listener(Self::escape))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::up))
            .on_action(cx.listener(Self::down))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::select_up))
            .on_action(cx.listener(Self::select_down))
            .on_action(cx.listener(Self::word_left))
            .on_action(cx.listener(Self::word_right))
            .on_action(cx.listener(Self::select_word_left))
            .on_action(cx.listener(Self::select_word_right))
            .on_action(cx.listener(Self::line_start))
            .on_action(cx.listener(Self::line_end))
            .on_action(cx.listener(Self::select_line_start))
            .on_action(cx.listener(Self::select_line_end))
            .on_action(cx.listener(Self::doc_start))
            .on_action(cx.listener(Self::doc_end))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::undo))
            .on_action(cx.listener(Self::redo))
            .on_action(cx.listener(Self::move_block_up))
            .on_action(cx.listener(Self::move_block_down))
            .on_action(cx.listener(Self::duplicate))
            .on_action(cx.listener(|this, _: &Bold, _, cx| this.toggle_mark(MarkKind::Bold, cx)))
            .on_action(cx.listener(|this, _: &Italic, _, cx| this.toggle_mark(MarkKind::Italic, cx)))
            .on_action(cx.listener(|this, _: &Underline, _, cx| this.toggle_mark(MarkKind::Underline, cx)))
            .on_action(cx.listener(|this, _: &Strike, _, cx| this.toggle_mark(MarkKind::Strike, cx)))
            .on_action(cx.listener(|this, _: &InlineCode, _, cx| this.toggle_mark(MarkKind::Code, cx)))
            .on_action(cx.listener(|this, _: &TurnText, _, cx| this.turn_into(BlockKind::Paragraph, cx)))
            .on_action(cx.listener(|this, _: &TurnH1, _, cx| this.turn_into(BlockKind::Heading(1), cx)))
            .on_action(cx.listener(|this, _: &TurnH2, _, cx| this.turn_into(BlockKind::Heading(2), cx)))
            .on_action(cx.listener(|this, _: &TurnH3, _, cx| this.turn_into(BlockKind::Heading(3), cx)))
            .on_action(cx.listener(|this, _: &TurnTodo, _, cx| {
                this.turn_into(BlockKind::Todo { checked: false }, cx)
            }))
            .on_action(cx.listener(|this, _: &TurnBullet, _, cx| this.turn_into(BlockKind::Bullet, cx)))
            .on_action(cx.listener(|this, _: &TurnNumbered, _, cx| this.turn_into(BlockKind::Numbered, cx)))
            .on_action(cx.listener(|this, _: &TurnToggle, _, cx| {
                this.turn_into(BlockKind::Toggle { open: true }, cx)
            }))
            .on_action(cx.listener(|this, _: &TurnCode, _, cx| {
                this.turn_into(BlockKind::Code { lang: String::new() }, cx)
            }))
            .on_drag_move(cx.listener(|this, ev: &DragMoveEvent<DragBlock>, _, cx| {
                let drag = ev.drag(cx).clone();
                let pos = ev.event.position;
                let inside = ev.bounds.contains(&pos) && this.scroll.bounds().contains(&pos);
                this.drag_moved(&drag, pos, inside, cx);
            }))
            .on_drop(cx.listener(|this, drag: &DragBlock, _, cx| this.drop_dragged(drag, cx)))
            .on_drag_move(cx.listener(|this, ev: &DragMoveEvent<ExternalPaths>, _, cx| {
                let pos = ev.event.position;
                let inside = ev.bounds.contains(&pos) && this.scroll.bounds().contains(&pos);
                this.files_dragged(pos, inside, cx);
            }))
            .on_drop(cx.listener(|this, paths: &ExternalPaths, window, cx| {
                this.drop_files(paths.paths(), window, cx)
            }))
            .on_drag_move(cx.listener(|this, ev: &DragMoveEvent<ResizeImage>, _, cx| {
                let drag = ev.drag(cx).clone();
                let max = this.index_of_block(drag.id).map_or(PAGE_MAX_W, |ix| this.column_width(ix));
                this.resize_moved(&drag, ev.event.position.x, max, cx);
            }));

        let last_vis = (0..self.blocks.len()).rev().find(|&i| vis[i]);
        for ix in 0..self.blocks.len() {
            if vis[ix] {
                col = col.child(self.render_block(ix, Some(ix) == last_vis, focused, scale, &t, cx));
            }
        }

        // empty space below the content: click to continue writing
        col = col.child(
            div()
                .id("editor-tail")
                .w_full()
                .h(px(180.))
                .cursor_text()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, ev: &gpui::MouseDownEvent, window, cx| {
                        cx.stop_propagation();
                        this.begin_marquee(ev.position, true, window, cx);
                    }),
                ),
        );

        // While selecting, follow the pointer anywhere in the window.
        if self.tracking_pointer() {
            let entity = cx.entity();
            col = col.child(
                canvas(|_, _, _| {}, move |_, _, window, _| {
                    let e = entity.clone();
                    window.on_mouse_event(move |ev: &MouseMoveEvent, phase, window, cx| {
                        if phase == DispatchPhase::Capture {
                            e.update(cx, |this, cx| this.on_mouse_move(ev, window, cx));
                        }
                    });
                    let e = entity.clone();
                    window.on_mouse_event(move |ev: &MouseUpEvent, phase, window, cx| {
                        if phase == DispatchPhase::Capture && ev.button == MouseButton::Left {
                            e.update(cx, |this, cx| this.on_mouse_up(ev, window, cx));
                        }
                    });
                })
                .absolute(),
            );
        }
        if let Some(rect) = self.render_marquee(&t) {
            col = col.child(rect);
        }

        if let Some(menu) = self.render_slash_menu(&t, cx) {
            col = col.child(menu);
        }
        if let Some(menu) = self.render_block_menu(&t, cx) {
            col = col.child(menu);
        }
        if let Some(bar) = self.render_format_toolbar(focused, &t, cx) {
            col = col.child(bar);
        }
        col
    }
}
