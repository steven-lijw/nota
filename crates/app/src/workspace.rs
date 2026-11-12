//! The window: sidebar, top bar, the open page, the AI panel, quick find
//! and the small global menus.

use crate::actions::*;
use crate::ai::{AiEvent, AiPanel};
use crate::page_view::{PageEvent, PageView};
use crate::props::eid;
use crate::store::{Store, StoreEvent};
use crate::text_input::{InputEvent, TextInput};
use crate::theme::{theme, Theme, AI_PANEL_W, SIDEBAR_W, TOPBAR_H};
use crate::ui::*;
use crate::update::{current_version, UpdateStatus, Updater};
use gpui::{
    deferred, div, prelude::*, px, AnyElement, ClipboardItem, Context, Corner, DragMoveEvent, Entity, FontWeight,
    MouseButton, MouseDownEvent, PathPromptOptions, Pixels, Point, SharedString, Subscription, Window,
};
use nota_core::markdown::{from_markdown, to_markdown};
use nota_core::query::relative_time;
use nota_core::{Anchor, Block, BlockKind, Page, PageKind, Vault};

const MAC: bool = cfg!(target_os = "macos");

#[derive(Clone, PartialEq)]
enum Menu {
    Page(String),
    Trash,
    Settings,
    More,
}

/// Where a page dragged in the sidebar would go, relative to the row under the pointer.
#[derive(Clone, Copy, PartialEq)]
enum Zone {
    Before,
    Inside,
    After,
}

#[derive(Clone)]
struct DragPage {
    id: String,
    title: String,
}

struct DragLabel(SharedString);

impl Render for DragLabel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        div()
            .px(px(8.))
            .py(px(4.))
            .rounded(px(6.))
            .bg(t.panel_bg)
            .shadow(card_shadow(&t))
            .text_size(px(14.))
            .text_color(t.text)
            .font_family(crate::theme::ui_font())
            .opacity(0.9)
            .child(self.0.clone())
    }
}

struct Search {
    input: Entity<TextInput>,
    selected: usize,
    _sub: Subscription,
}

pub struct Workspace {
    focus: gpui::FocusHandle,
    store: Entity<Store>,
    page: Option<Entity<PageView>>,
    current: Option<String>,
    back: Vec<String>,
    forward: Vec<String>,
    ai: Entity<AiPanel>,
    search: Option<Search>,
    menu: Option<(Menu, Point<Pixels>)>,
    side_drop: Option<(String, Zone)>,
    /// Recently shown pages, kept alive so going back restores scroll and undo.
    recent: Vec<(String, Entity<PageView>)>,
    updater: Entity<Updater>,
    _page_sub: Option<Subscription>,
    /// Follows the open editor's selection for the AI panel.
    _editor_obs: Option<Subscription>,
    _subs: Vec<Subscription>,
}

impl Workspace {
    pub fn new(store: Entity<Store>, updater: Entity<Updater>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let ai = cx.new(|cx| AiPanel::new(store.clone(), window, cx));
        let subs = vec![
            cx.observe(&store, |_, _, cx| cx.notify()),
            cx.observe(&updater, |_, _, cx| cx.notify()),
            cx.subscribe_in(&store, window, |this: &mut Self, _, ev: &StoreEvent, window, cx| {
                if !matches!(ev, StoreEvent::Reloaded) {
                    return;
                }
                let gone = this.current.as_ref().is_none_or(|id| this.store.read(cx).page(id).is_none());
                if gone {
                    this.back.clear();
                    this.forward.clear();
                    let first = this.first_page(cx);
                    this.show(first, window, cx);
                }
            }),
            cx.subscribe_in(&ai, window, |this: &mut Self, _, ev: &AiEvent, window, cx| match ev {
                AiEvent::Insert(md) => {
                    let blocks = from_markdown(md);
                    if let Some(editor) = this.page.as_ref().and_then(|p| p.read(cx).editor.clone()) {
                        editor.update(cx, |e, cx| e.append_blocks(blocks, cx));
                    }
                }
                AiEvent::InsertBelow(md, target) | AiEvent::Replace(md, target) => {
                    let replace = matches!(ev, AiEvent::Replace(..));
                    if let Some(editor) = this.page.as_ref().and_then(|p| p.read(cx).editor.clone()) {
                        let ok = editor.update(cx, |e, cx| e.apply_ai(md, target, replace, window, cx));
                        if !ok {
                            this.store.update(cx, |s, cx| {
                                s.last_error = Some("That part of the page has changed — the answer was not inserted.".into());
                                cx.notify();
                            });
                        }
                    }
                }
                AiEvent::Close => this.toggle_ai(window, cx),
            }),
        ];
        let mut ws = Workspace {
            focus: cx.focus_handle(),
            store,
            page: None,
            current: None,
            back: Vec::new(),
            forward: Vec::new(),
            ai,
            search: None,
            menu: None,
            side_drop: None,
            recent: Vec::new(),
            updater,
            _page_sub: None,
            _editor_obs: None,
            _subs: subs,
        };
        let start = {
            let s = ws.store.read(cx);
            s.state.last_page.clone().filter(|id| s.page(id).is_some() && !s.vault.is_trashed(id))
        }
        .or_else(|| ws.first_page(cx));
        ws.show(start, window, cx);
        ws
    }

    fn first_page(&self, cx: &Context<Self>) -> Option<String> {
        self.store.read(cx).vault.children(None).first().map(|p| p.id.clone())
    }

    // --- navigation ----------------------------------------------------------

    fn show(&mut self, id: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        self.menu = None;
        self.search = None;
        self._page_sub = None;
        self.page = None;
        self.current = id.clone();
        window.focus(&self.focus);
        if let Some(id) = id.clone() {
            let store = self.store.clone();
            let cached = self.recent.iter().position(|(pid, _)| *pid == id).map(|i| self.recent.remove(i).1);
            let view = match cached {
                Some(view) => {
                    view.update(cx, |v, cx| v.refresh(cx));
                    view
                }
                None => cx.new(|cx| PageView::new(store, id.clone(), window, cx)),
            };
            self.recent.push((id.clone(), view.clone()));
            if self.recent.len() > 8 {
                self.recent.remove(0);
            }
            self._page_sub = Some(cx.subscribe_in(&view, window, |this: &mut Self, _, ev: &PageEvent, window, cx| {
                match ev {
                    PageEvent::Open(id) => this.open(id.clone(), window, cx),
                    PageEvent::AskAi => this.ask_ai(window, cx),
                }
            }));
            self.page = Some(view);
            self.store.update(cx, |s, _| {
                let ancestors: Vec<String> = s.vault.ancestors(&id).iter().map(|p| p.id.clone()).collect();
                for a in ancestors {
                    if !s.state.expanded.contains(&a) {
                        s.state.expanded.push(a);
                    }
                }
                s.state.last_page = Some(id);
                s.save_state();
            });
        }
        self.ai.update(cx, |ai, cx| ai.set_context(id, cx));
        self._editor_obs = None;
        if let Some(editor) = self.page.as_ref().and_then(|p| p.read(cx).editor.clone()) {
            let ai = self.ai.clone();
            self._editor_obs = Some(cx.observe(&editor, move |_, editor, cx| {
                let sel = editor.read(cx).ai_selection();
                ai.update(cx, |ai, cx| ai.set_selection(sel, cx));
            }));
        }
        cx.notify();
    }

    pub fn open(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.current.as_ref() == Some(&id) {
            self.search = None;
            self.menu = None;
            cx.notify();
            return;
        }
        if self.store.read(cx).page(&id).is_none() {
            return;
        }
        if let Some(cur) = self.current.take() {
            self.back.push(cur);
        }
        self.forward.clear();
        self.show(Some(id), window, cx);
    }

    fn go_back(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        while let Some(id) = self.back.pop() {
            if self.store.read(cx).page(&id).is_some() {
                if let Some(cur) = self.current.take() {
                    self.forward.push(cur);
                }
                self.show(Some(id), window, cx);
                return;
            }
        }
    }

    fn go_forward(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        while let Some(id) = self.forward.pop() {
            if self.store.read(cx).page(&id).is_some() {
                if let Some(cur) = self.current.take() {
                    self.back.push(cur);
                }
                self.show(Some(id), window, cx);
                return;
            }
        }
    }

    // --- commands ------------------------------------------------------------

    fn new_page(&mut self, parent: Option<String>, kind: PageKind, window: &mut Window, cx: &mut Context<Self>) {
        let id = self.store.update(cx, |s, cx| {
            let id = s.create_page(kind, parent.clone(), cx);
            if let Some(parent) = parent {
                if s.page(&parent).is_some_and(|p| !p.is_database()) {
                    let link = Block::new(BlockKind::PageLink { page: id.clone() }, "");
                    s.update_page(&parent, cx, |p| {
                        let tail_empty =
                            p.blocks.last().is_some_and(|b| b.kind == BlockKind::Paragraph && b.text.is_empty());
                        let at = if tail_empty { p.blocks.len() - 1 } else { p.blocks.len() };
                        p.blocks.insert(at, link);
                    });
                    cx.emit(StoreEvent::BlocksChanged(parent.clone()));
                }
                s.set_expanded(&parent, true, cx);
            }
            id
        });
        self.open(id, window, cx);
    }

    fn trash(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.menu = None;
        self.store.update(cx, |s, cx| s.trash(id, cx));
        let affected = self.current.as_ref().is_some_and(|c| self.store.read(cx).vault.is_trashed(c));
        if affected {
            let parent = self
                .store
                .read(cx)
                .page(id)
                .and_then(|p| p.parent.clone())
                .filter(|p| !self.store.read(cx).vault.is_trashed(p));
            let next = parent.or_else(|| self.first_page(cx));
            self.back.retain(|b| b != id);
            self.show(next, window, cx);
        }
        cx.notify();
    }

    fn toggle_favorite(&mut self, id: &str, cx: &mut Context<Self>) {
        self.menu = None;
        self.store.update(cx, |s, cx| s.update_page(id, cx, |p| p.favorite = !p.favorite));
    }

    fn duplicate(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.menu = None;
        if let Some(new) = self.store.update(cx, |s, cx| s.duplicate_page(id, cx)) {
            self.open(new, window, cx);
        }
    }

    fn reveal(&mut self, id: &str, cx: &mut Context<Self>) {
        self.menu = None;
        self.store.update(cx, |s, _| s.flush());
        if let Some(path) = self.store.read(cx).vault.abs_path(id) {
            cx.reveal_path(&path);
        }
        cx.notify();
    }

    fn toggle_theme(&mut self, cx: &mut Context<Self>) {
        let dark = self.store.update(cx, |s, _| {
            s.state.dark = !s.state.dark;
            s.save_state();
            s.state.dark
        });
        cx.set_global(Theme::get(dark));
        cx.refresh_windows();
    }

    fn toggle_sidebar(&mut self, cx: &mut Context<Self>) {
        self.store.update(cx, |s, cx| {
            s.state.sidebar_collapsed = !s.state.sidebar_collapsed;
            s.save_state();
            cx.notify();
        });
    }

    /// Open the AI panel (if needed) and put the caret in its prompt.
    fn ask_ai(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.store.read(cx).state.ai_open {
            self.toggle_ai(window, cx);
        } else {
            self.ai.read(cx).focus_input(window, cx);
        }
    }

    fn toggle_ai(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let open = self.store.update(cx, |s, cx| {
            s.state.ai_open = !s.state.ai_open;
            s.save_state();
            cx.notify();
            s.state.ai_open
        });
        if open {
            self.ai.read(cx).focus_input(window, cx);
        }
    }

    fn open_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.search.take().is_some() {
            cx.notify();
            return;
        }
        self.menu = None;
        let input = cx.new(|cx| {
            let mut i = TextInput::new(window, cx);
            i.placeholder = "Search pages…".into();
            i.font_size = px(18.);
            i.line_height = px(26.);
            i
        });
        input.read(cx).focus(window);
        let sub = cx.subscribe_in(&input, window, |this: &mut Self, _, ev: &InputEvent, window, cx| {
            let hits = this.search_hits(cx);
            let Some(search) = &mut this.search else { return };
            match ev {
                InputEvent::Changed => search.selected = 0,
                InputEvent::MoveDown => search.selected = (search.selected + 1).min(hits.len().saturating_sub(1)),
                InputEvent::MoveUp => search.selected = search.selected.saturating_sub(1),
                InputEvent::Cancel => {
                    this.search = None;
                    window.focus(&this.focus);
                }
                InputEvent::Submit => {
                    if let Some(id) = hits.get(search.selected).map(|h| h.0.clone()) {
                        this.open(id, window, cx);
                    }
                    this.search = None;
                }
                _ => {}
            }
            cx.notify();
        });
        self.search = Some(Search { input, selected: 0, _sub: sub });
        cx.notify();
    }

    /// (page id, snippet)
    fn search_hits(&self, cx: &Context<Self>) -> Vec<(String, String)> {
        let Some(search) = &self.search else { return vec![] };
        let q = search.input.read(cx).text().to_string();
        self.store.read(cx).vault.search(&q).into_iter().take(30).map(|h| (h.id, h.snippet)).collect()
    }

    fn switch_vault(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.menu = None;
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Open vault".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else { return };
            let Some(path) = paths.into_iter().next() else { return };
            this.update_in(cx, |this, window, cx| match Vault::open(&path) {
                Ok(vault) => {
                    crate::save_vault_path(&path);
                    this.current = None;
                    this.page = None;
                    this.recent.clear();
                    let dark = this.store.update(cx, |s, cx| {
                        s.switch_vault(vault, cx);
                        s.state.dark
                    });
                    cx.set_global(Theme::get(dark));
                    let start = {
                        let s = this.store.read(cx);
                        s.state.last_page.clone().filter(|id| s.page(id).is_some())
                    }
                    .or_else(|| this.first_page(cx));
                    this.back.clear();
                    this.forward.clear();
                    this.show(start, window, cx);
                    cx.refresh_windows();
                }
                Err(err) => {
                    this.store.update(cx, |s, cx| {
                        s.last_error = Some(format!("Could not open that folder: {err:#}"));
                        cx.notify();
                    });
                }
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn open_menu(&mut self, menu: Menu, pos: Point<Pixels>, cx: &mut Context<Self>) {
        self.menu = Some((menu, pos));
        cx.notify();
    }

    /// Move a page in the tree: under `new_parent`, at `anchor` among its siblings.
    /// The sub-page link moves from the old parent's body to the new one's.
    fn move_page(&mut self, id: &str, new_parent: Option<String>, anchor: Anchor, cx: &mut Context<Self>) {
        self.store.update(cx, |s, cx| {
            if let Some(parent) = &new_parent {
                let bad = parent == id
                    || s.vault.descendants(id).contains(parent)
                    || s.page(parent).is_none_or(|p| p.is_database());
                if bad {
                    return;
                }
            }
            if matches!(&anchor, Anchor::Before(a) | Anchor::After(a) if a == id) {
                return;
            }
            let old_parent = s.page(id).and_then(|p| p.parent.clone());
            let reparent = old_parent != new_parent;
            if reparent {
                s.update_page(id, cx, |p| p.props.clear());
                if let Some(old) = old_parent.filter(|o| s.page(o).is_some_and(|p| !p.is_database())) {
                    let target = id.to_string();
                    s.update_page(&old, cx, |p| {
                        p.blocks.retain(|b| !matches!(&b.kind, BlockKind::PageLink { page } if *page == target));
                        if p.blocks.is_empty() {
                            p.blocks.push(Block::paragraph(""));
                        }
                    });
                    cx.emit(StoreEvent::BlocksChanged(old));
                }
                if let Some(new) = &new_parent {
                    let link = Block::new(BlockKind::PageLink { page: id.to_string() }, "");
                    s.update_page(new, cx, |p| {
                        let tail_empty =
                            p.blocks.last().is_some_and(|b| b.kind == BlockKind::Paragraph && b.text.is_empty());
                        let at = if tail_empty { p.blocks.len() - 1 } else { p.blocks.len() };
                        p.blocks.insert(at, link);
                    });
                    cx.emit(StoreEvent::BlocksChanged(new.clone()));
                }
            }
            s.place_page(id, new_parent.clone(), anchor, cx);
            if let Some(parent) = &new_parent {
                s.set_expanded(parent, true, cx);
            }
        });
    }

    fn drop_on_row(&mut self, dragged: &str, cx: &mut Context<Self>) {
        let Some((target, zone)) = self.side_drop.take() else {
            cx.notify();
            return;
        };
        let parent = self.store.read(cx).page(&target).and_then(|p| p.parent.clone());
        match zone {
            Zone::Inside => self.move_page(dragged, Some(target), Anchor::End, cx),
            Zone::Before => self.move_page(dragged, parent, Anchor::Before(target), cx),
            Zone::After => self.move_page(dragged, parent, Anchor::After(target), cx),
        }
        cx.notify();
    }

    // --- sidebar -------------------------------------------------------------

    fn side_item(&self, id: &'static str, icon_name: &str, label: &'static str, t: &Theme) -> gpui::Stateful<gpui::Div> {
        let hover = t.hover;
        div()
            .id(id)
            .flex()
            .flex_none()
            .items_center()
            .gap(px(8.))
            .h(px(30.))
            .mx(px(6.))
            .px(px(8.))
            .rounded(px(6.))
            .text_color(t.sidebar_text)
            .font_weight(FontWeight::MEDIUM)
            .cursor_pointer()
            .hover(move |s| s.bg(hover))
            .child(icon(icon_name, 18., t.sidebar_text))
            .child(div().flex_1().child(label))
    }

    fn nav_row(&self, section: &str, page: &Page, depth: usize, s: &Store, t: &Theme, cx: &Context<Self>) -> AnyElement {
        let key = format!("{section}-{}", page.id);
        let group: SharedString = format!("nav-{key}").into();
        let id = page.id.clone();
        let selected = self.current.as_deref() == Some(page.id.as_str());
        let can_nest = !page.is_database();
        let expanded = can_nest && section == "tree" && s.is_expanded(&page.id);
        let hover = t.hover;
        let active = t.active;
        let small = |eid_: gpui::ElementId, name: &str| {
            div()
                .id(eid_)
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .size(px(20.))
                .rounded(px(4.))
                .hover(move |s| s.bg(active))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(icon(name, 14., t.text_faint))
        };
        let (i1, i2, i3, i4, i5, i6) = (id.clone(), id.clone(), id.clone(), id.clone(), id.clone(), id.clone());
        let mut row = div()
            .id(eid("nav", &key))
            .group(group.clone())
            .flex()
            .flex_none()
            .items_center()
            .gap(px(2.))
            .h(px(30.))
            .mx(px(6.))
            .pl(px(4. + depth as f32 * 14.))
            .pr(px(4.))
            .rounded(px(6.))
            .cursor_pointer()
            .font_weight(FontWeight::MEDIUM)
            .text_color(if selected { t.text } else { t.sidebar_text })
            .hover(move |s| s.bg(hover))
            .on_click(cx.listener(move |this, _, window, cx| this.open(i1.clone(), window, cx)))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, ev: &MouseDownEvent, _, cx| this.open_menu(Menu::Page(i2.clone()), ev.position, cx)),
            )
            .on_drag(DragPage { id: id.clone(), title: page.display_title().to_string() }, |d: &DragPage, _, _, cx| {
                let label: SharedString = d.title.clone().into();
                cx.new(|_| DragLabel(label))
            });
        if selected {
            row = row.bg(t.active);
        }
        if section == "tree" {
            let target = i3.clone();
            row = row
                .relative()
                .on_drag_move(cx.listener(move |this, ev: &DragMoveEvent<DragPage>, _, cx| {
                    let pos = ev.event.position;
                    let mine = this.side_drop.as_ref().is_some_and(|(t, _)| *t == target);
                    if !ev.bounds.contains(&pos) {
                        if mine {
                            this.side_drop = None;
                            cx.notify();
                        }
                        return;
                    }
                    if ev.drag(cx).id == target {
                        this.side_drop = None;
                        cx.notify();
                        return;
                    }
                    let frac = (pos.y - ev.bounds.top()) / ev.bounds.size.height;
                    let zone = if can_nest && (0.28..=0.72).contains(&frac) {
                        Zone::Inside
                    } else if frac < 0.5 {
                        Zone::Before
                    } else {
                        Zone::After
                    };
                    let next = Some((target.clone(), zone));
                    if this.side_drop != next {
                        this.side_drop = next;
                        cx.notify();
                    }
                }))
                .on_drop(cx.listener(move |this, d: &DragPage, _, cx| this.drop_on_row(&d.id, cx)));
            if let Some((_, zone)) = self.side_drop.as_ref().filter(|(t, _)| *t == page.id) {
                let line = || div().absolute().left(px(4. + depth as f32 * 14.)).right(px(4.)).h(px(2.)).bg(t.accent);
                row = match zone {
                    Zone::Inside => row.bg(t.accent_bg),
                    Zone::Before => row.child(line().top(px(-1.))),
                    Zone::After => row.child(line().bottom(px(-1.))),
                };
            }
        }
        row = row.child(if section == "tree" && can_nest {
            small(eid("nav-exp", &key), if expanded { "chevron-down" } else { "chevron-right" })
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.store.update(cx, |s, cx| {
                        let open = s.is_expanded(&i4);
                        s.set_expanded(&i4, !open, cx)
                    })
                }))
                .into_any_element()
        } else {
            div().size(px(20.)).flex_none().into_any_element()
        });
        row = row.child(page_icon(page.icon.as_deref(), page.is_database(), 15., t)).child(
            div()
                .flex_1()
                .min_w_0()
                .pl(px(4.))
                .whitespace_nowrap()
                .overflow_hidden()
                .text_ellipsis()
                .child(SharedString::from(page.display_title().to_string())),
        );
        let mut actions = div().flex().flex_none().items_center().opacity(0.).group_hover(group, |s| s.opacity(1.)).child(
            small(eid("nav-more", &key), "more").on_click(cx.listener(move |this, ev: &gpui::ClickEvent, _, cx| {
                cx.stop_propagation();
                this.open_menu(Menu::Page(i5.clone()), ev.position(), cx)
            })),
        );
        if can_nest {
            actions = actions.child(small(eid("nav-add", &key), "plus").on_click(cx.listener(
                move |this, _, window, cx| {
                    cx.stop_propagation();
                    this.new_page(Some(i6.clone()), PageKind::Page, window, cx)
                },
            )));
        }
        row.child(actions).into_any_element()
    }

    fn tree(&self, parent: Option<&str>, depth: usize, s: &Store, t: &Theme, cx: &Context<Self>, out: &mut Vec<AnyElement>) {
        if depth > 12 {
            return;
        }
        let children = s.vault.sidebar_children(parent);
        if children.is_empty() && depth > 0 {
            out.push(
                div()
                    .flex_none()
                    .h(px(28.))
                    .flex()
                    .items_center()
                    .pl(px(32. + depth as f32 * 14.))
                    .text_color(t.text_faint)
                    .child("No pages inside")
                    .into_any_element(),
            );
        }
        for page in children {
            out.push(self.nav_row("tree", page, depth, s, t, cx));
            if !page.is_database() && s.is_expanded(&page.id) {
                self.tree(Some(&page.id), depth + 1, s, t, cx, out);
            }
        }
    }

    fn section_label(&self, label: &'static str, t: &Theme) -> gpui::Div {
        div()
            .flex()
            .flex_none()
            .items_center()
            .h(px(28.))
            .mt(px(14.))
            .mx(px(6.))
            .px(px(8.))
            .text_size(px(12.))
            .font_weight(FontWeight::MEDIUM)
            .text_color(t.text_faint)
            .child(div().flex_1().child(label))
    }

    fn render_sidebar(&self, s: &Store, t: &Theme, cx: &Context<Self>) -> AnyElement {
        let vault_name = s
            .vault
            .root
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "Vault".into());
        let initial: String = vault_name.chars().next().map(|c| c.to_uppercase().collect()).unwrap_or_default();
        let hover = t.hover;
        let header = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(8.))
            .h(px(32.))
            .mx(px(6.))
            .mt(px(if MAC { 40. } else { 8. }))
            .mb(px(4.))
            .pl(px(6.))
            .pr(px(2.))
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_center()
                    .size(px(22.))
                    .rounded(px(5.))
                    .bg(t.active)
                    .text_size(px(13.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(t.text_muted)
                    .child(SharedString::from(initial)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(t.text)
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .text_ellipsis()
                    .child(SharedString::from(vault_name)),
            )
            .child(
                icon_button("side-collapse", "chevrons-left", t)
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_sidebar(cx))),
            )
            .child(icon_button("side-new", "edit", t).on_click(cx.listener(|this, _, window, cx| {
                this.new_page(None, PageKind::Page, window, cx)
            })));

        let ai_open = s.state.ai_open;
        let mut col = div()
            .flex()
            .flex_col()
            .flex_none()
            .w(px(SIDEBAR_W))
            .h_full()
            .bg(t.sidebar_bg)
            .border_r_1()
            .border_color(t.border)
            .text_size(px(14.))
            .child(header)
            .child(
                self.side_item("side-search", "search", "Search", t)
                    .on_click(cx.listener(|this, _, window, cx| this.open_search(window, cx)))
                    .child(div().text_size(px(12.)).text_color(t.text_faint).child(if MAC { "⌘K" } else { "Ctrl K" })),
            )
            .child({
                let item = self
                    .side_item("side-ai", "sparkles", "Nota AI", t)
                    .on_click(cx.listener(|this, _, window, cx| this.toggle_ai(window, cx)));
                if ai_open {
                    item.bg(t.active).text_color(t.text)
                } else {
                    item
                }
            })
            .child(
                self.side_item("side-newpage", "plus", "New page", t)
                    .on_click(cx.listener(|this, _, window, cx| this.new_page(None, PageKind::Page, window, cx))),
            );

        let mut scroll = div().id("side-scroll").flex().flex_col().flex_1().min_h_0().overflow_y_scroll().pb(px(8.));
        let favorites = s.vault.favorites();
        if !favorites.is_empty() {
            scroll = scroll.child(self.section_label("Favorites", t));
            for page in favorites {
                scroll = scroll.child(self.nav_row("fav", page, 0, s, t, cx));
            }
        }
        let accent_bg = t.accent_bg;
        scroll = scroll.child(
            self.section_label("Private", t)
                .id("side-private")
                .group("side-private")
                .rounded(px(6.))
                .drag_over::<DragPage>(move |s, _, _, _| s.bg(accent_bg))
                .on_drop(cx.listener(|this, d: &DragPage, _, cx| this.move_page(&d.id, None, Anchor::Start, cx)))
                .child(
                    div()
                        .id("side-private-add")
                        .flex()
                        .items_center()
                        .justify_center()
                        .size(px(20.))
                        .rounded(px(4.))
                        .cursor_pointer()
                        .opacity(0.)
                        .group_hover("side-private", |s| s.opacity(1.))
                        .hover(move |s| s.bg(hover))
                        .on_click(cx.listener(|this, _, window, cx| this.new_page(None, PageKind::Page, window, cx)))
                        .child(icon("plus", 14., t.text_faint)),
                ),
        );
        let mut rows = Vec::new();
        self.tree(None, 0, s, t, cx, &mut rows);
        for r in rows {
            scroll = scroll.child(r);
        }
        scroll = scroll
            .child(div().flex_none().h(px(14.)))
            .child(self.side_item("side-newdb", "database", "New database", t).on_click(cx.listener(
                |this, _, window, cx| this.new_page(None, PageKind::Database, window, cx),
            )))
            .child(self.side_item("side-settings", "settings", "Settings", t).on_click(cx.listener(
                |this, ev: &gpui::ClickEvent, _, cx| this.open_menu(Menu::Settings, ev.position(), cx),
            )))
            .child(self.side_item("side-trash", "trash", "Trash", t).on_click(cx.listener(
                |this, ev: &gpui::ClickEvent, _, cx| this.open_menu(Menu::Trash, ev.position(), cx),
            )));
        col = col.child(scroll);
        if let Some(bar) = self.render_update_bar(t, cx) {
            col = col.child(bar);
        }
        col.into_any_element()
    }

    /// "Restart to update" at the bottom of the sidebar once an update is downloaded.
    fn render_update_bar(&self, t: &Theme, cx: &Context<Self>) -> Option<AnyElement> {
        let UpdateStatus::Ready { version, .. } = &self.updater.read(cx).status else {
            return None;
        };
        let hover = t.accent_hover;
        Some(
            div()
                .id("update-ready")
                .flex()
                .flex_none()
                .items_center()
                .gap(px(8.))
                .m(px(8.))
                .px(px(10.))
                .py(px(8.))
                .rounded(px(8.))
                .bg(t.accent)
                .text_color(gpui::white())
                .text_size(px(13.))
                .font_weight(FontWeight::MEDIUM)
                .cursor_pointer()
                .hover(move |s| s.bg(hover))
                .on_click(cx.listener(|this, _, _, cx| this.install_update(cx)))
                .child(icon("refresh", 15., gpui::white()))
                .child(div().flex_1().child("Restart to update"))
                .child(div().text_size(px(12.)).opacity(0.85).child(SharedString::from(format!("v{version}"))))
                .into_any_element(),
        )
    }

    fn install_update(&mut self, cx: &mut Context<Self>) {
        self.store.update(cx, |s, _| {
            s.flush();
            s.save_state();
        });
        let result = self.updater.update(cx, |u, cx| u.install(cx));
        if let Err(msg) = result {
            self.store.update(cx, |s, cx| {
                s.last_error = Some(msg);
                cx.notify();
            });
        }
    }

    fn check_for_updates(&mut self, cx: &mut Context<Self>) {
        self.updater.update(cx, |u, cx| u.check(true, cx));
    }

    // --- top bar ---------------------------------------------------------------

    fn render_topbar(&self, s: &Store, t: &Theme, cx: &Context<Self>) -> AnyElement {
        let collapsed = s.state.sidebar_collapsed;
        let mut bar = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(2.))
            .h(px(TOPBAR_H))
            .pl(px(if collapsed && MAC { 78. } else { 10. }))
            .pr(px(10.))
            .text_size(px(14.));
        if collapsed {
            bar = bar.child(
                icon_button("top-sidebar", "chevrons-right", t).on_click(cx.listener(|this, _, _, cx| this.toggle_sidebar(cx))),
            );
        }
        let nav = |id: &'static str, name: &'static str, enabled: bool| {
            let b = icon_button(id, name, t);
            if enabled {
                b
            } else {
                b.opacity(0.35)
            }
        };
        bar = bar
            .child(nav("top-back", "chevron-left", !self.back.is_empty()).on_click(cx.listener(|this, _, window, cx| this.go_back(window, cx))))
            .child(
                nav("top-fwd", "chevron-right", !self.forward.is_empty())
                    .on_click(cx.listener(|this, _, window, cx| this.go_forward(window, cx))),
            );

        let mut crumbs = div().flex().flex_1().min_w_0().items_center().gap(px(2.)).pl(px(4.)).overflow_hidden();
        if let Some(page) = self.current.as_ref().and_then(|id| s.page(id)) {
            let mut chain: Vec<&Page> = s.vault.ancestors(&page.id);
            chain.push(page);
            let n = chain.len();
            for (i, p) in chain.into_iter().enumerate() {
                if i > 0 {
                    crumbs = crumbs.child(div().flex_none().text_color(t.text_faint).child("/"));
                }
                let hover = t.hover;
                let id = p.id.clone();
                crumbs = crumbs.child(
                    div()
                        .id(("crumb", i))
                        .flex()
                        .items_center()
                        .gap(px(4.))
                        .h(px(26.))
                        .px(px(6.))
                        .max_w(px(240.))
                        .rounded(px(6.))
                        .cursor_pointer()
                        .when(i + 1 < n, |d| d.flex_shrink())
                        .hover(move |s| s.bg(hover))
                        .on_click(cx.listener(move |this, _, window, cx| this.open(id.clone(), window, cx)))
                        .child(page_icon(p.icon.as_deref(), p.is_database(), 14., t))
                        .child(
                            div()
                                .min_w_0()
                                .whitespace_nowrap()
                                .overflow_hidden()
                                .text_ellipsis()
                                .child(SharedString::from(p.display_title().to_string())),
                        ),
                );
            }
        }
        bar = bar.child(crumbs);

        if let Some(page) = self.current.as_ref().and_then(|id| s.page(id)) {
            let id = page.id.clone();
            let edited = relative_time(&page.updated);
            if !edited.is_empty() {
                bar = bar.child(
                    div()
                        .flex_none()
                        .px(px(8.))
                        .text_color(t.text_faint)
                        .whitespace_nowrap()
                        .child(SharedString::from(format!("Edited {edited}"))),
                );
            }
            let star = div()
                .id("top-star")
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .size(px(26.))
                .rounded(px(6.))
                .cursor_pointer()
                .hover({
                    let hover = t.hover;
                    move |s| s.bg(hover)
                })
                .on_click(cx.listener(move |this, _, _, cx| this.toggle_favorite(&id, cx)))
                .child(if page.favorite {
                    icon("star-filled", 16., gpui::rgb(0xf6c050).into())
                } else {
                    icon("star", 16., t.text_muted)
                });
            bar = bar.child(star);
        }
        let ai_btn = icon_button("top-ai", "sparkles", t).on_click(cx.listener(|this, _, window, cx| this.toggle_ai(window, cx)));
        bar = bar
            .child(if s.state.ai_open { ai_btn.bg(t.active) } else { ai_btn })
            .child(icon_button("top-more", "more", t).on_click(cx.listener(|this, ev: &gpui::ClickEvent, _, cx| {
                this.open_menu(Menu::More, ev.position(), cx)
            })));
        bar.into_any_element()
    }

    // --- menus -------------------------------------------------------------------

    fn page_menu_items(&self, id: &str, s: &Store, t: &Theme, cx: &Context<Self>, mut menu: gpui::Div) -> gpui::Div {
        let Some(page) = s.page(id) else { return menu };
        let (a, b, c, d, e, f) =
            (id.to_string(), id.to_string(), id.to_string(), id.to_string(), id.to_string(), id.to_string());
        menu = menu
            .child(
                menu_item(
                    "pm-fav",
                    Some(if page.favorite { "star-filled" } else { "star" }),
                    if page.favorite { "Remove from Favorites" } else { "Add to Favorites" },
                    t,
                )
                .on_click(cx.listener(move |this, _, _, cx| this.toggle_favorite(&a, cx))),
            )
            .child(
                menu_item("pm-dup", Some("copy"), "Duplicate", t)
                    .on_click(cx.listener(move |this, _, window, cx| this.duplicate(&b, window, cx))),
            );
        if !page.is_database() {
            menu = menu
                .child(menu_item("pm-sub", Some("plus"), "New sub-page", t).on_click(cx.listener(
                    move |this, _, window, cx| this.new_page(Some(c.clone()), PageKind::Page, window, cx),
                )))
                .child(menu_item("pm-md", Some("text"), "Copy as Markdown", t).on_click(cx.listener(
                    move |this, _, _, cx| {
                        this.menu = None;
                        if let Some(p) = this.store.read(cx).page(&f) {
                            let md = format!("# {}\n\n{}", p.display_title(), to_markdown(&p.blocks));
                            cx.write_to_clipboard(ClipboardItem::new_string(md));
                        }
                        cx.notify();
                    },
                )));
        }
        menu.child(
            menu_item("pm-reveal", Some("folder"), if MAC { "Show in Finder" } else { "Show file" }, t)
                .on_click(cx.listener(move |this, _, _, cx| this.reveal(&d, cx))),
        )
        .child(menu_separator(t))
        .child(
            menu_item("pm-trash", Some("trash"), "Move to Trash", t)
                .on_click(cx.listener(move |this, _, window, cx| this.trash(&e, window, cx))),
        )
    }

    fn render_menu(&self, s: &Store, t: &Theme, cx: &Context<Self>) -> Option<AnyElement> {
        let (menu, pos) = self.menu.as_ref()?;
        let mut surface = menu_surface(t).w(px(250.));
        let mut corner = Corner::TopLeft;
        match menu {
            Menu::Page(id) => surface = self.page_menu_items(id, s, t, cx, surface),
            Menu::More => {
                corner = Corner::TopRight;
                if let Some(id) = &self.current {
                    surface = self.page_menu_items(id, s, t, cx, surface).child(menu_separator(t));
                }
                surface = surface.child(
                    menu_item("mm-theme", Some(if t.dark { "sun" } else { "moon" }), if t.dark { "Light mode" } else { "Dark mode" }, t)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.menu = None;
                            this.toggle_theme(cx)
                        })),
                );
            }
            Menu::Settings => {
                let root = s.vault.root.clone();
                surface = surface
                    .w(px(290.))
                    .child(menu_label("Vault", t))
                    .child(
                        div()
                            .px(px(12.))
                            .pb(px(6.))
                            .text_size(px(12.))
                            .text_color(t.text_muted)
                            .child(SharedString::from(s.vault.root.display().to_string())),
                    )
                    .child(menu_item("st-open", Some("folder"), "Open vault folder", t).on_click(cx.listener(
                        move |this, _, _, cx| {
                            this.menu = None;
                            cx.open_url(&format!("file://{}", root.display()));
                            cx.notify();
                        },
                    )))
                    .child(
                        menu_item("st-switch", Some("arrow-up-right"), "Switch vault…", t)
                            .on_click(cx.listener(|this, _, window, cx| this.switch_vault(window, cx))),
                    )
                    .child(menu_item("st-reload", Some("refresh"), "Reload from disk", t).on_click(cx.listener(
                        |this, _, _, cx| {
                            this.menu = None;
                            this.store.update(cx, |s, cx| s.reload(cx));
                        },
                    )))
                    .child(menu_separator(t))
                    .child(
                        menu_item("st-theme", Some(if t.dark { "sun" } else { "moon" }), if t.dark { "Light mode" } else { "Dark mode" }, t)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.menu = None;
                                this.toggle_theme(cx)
                            })),
                    )
                    .child(menu_separator(t))
                    .child(menu_label(SharedString::from(format!("Nota {}", current_version())), t));
                let u = self.updater.read(cx);
                let note = match &u.status {
                    UpdateStatus::Checking => Some("Checking for updates…".to_string()),
                    UpdateStatus::Downloading(v) => Some(format!("Downloading {v}…")),
                    UpdateStatus::UpToDate if u.manual => Some("You're up to date.".to_string()),
                    UpdateStatus::Failed(e) if u.manual => Some(e.clone()),
                    _ => None,
                };
                surface = match &u.status {
                    UpdateStatus::Ready { version, .. } => surface.child(
                        menu_item("st-install", Some("refresh"), format!("Restart to update to {version}"), t)
                            .on_click(cx.listener(|this, _, _, cx| this.install_update(cx))),
                    ),
                    _ => surface.child(
                        menu_item("st-update", Some("refresh"), "Check for updates", t)
                            .on_click(cx.listener(|this, _, _, cx| this.check_for_updates(cx))),
                    ),
                };
                if let Some(note) = note {
                    surface = surface.child(
                        div().px(px(12.)).pb(px(6.)).text_size(px(12.)).text_color(t.text_faint).child(SharedString::from(note)),
                    );
                }
            }
            Menu::Trash => {
                surface = surface.w(px(340.)).child(menu_label("Trash", t));
                let trashed = s.vault.trashed_roots();
                if trashed.is_empty() {
                    surface = surface.child(div().px(px(12.)).py(px(10.)).text_color(t.text_faint).child("Trash is empty"));
                }
                let mut list = div().id("trash-list").flex().flex_col().max_h(px(360.)).overflow_y_scroll();
                for (i, p) in trashed.into_iter().enumerate() {
                    let (a, b, c) = (p.id.clone(), p.id.clone(), p.id.clone());
                    list = list.child(
                        menu_item(("trash", i), None, p.display_title().to_string(), t)
                            .on_click(cx.listener(move |this, _, window, cx| this.open(a.clone(), window, cx)))
                            .child(icon_button(("trash-restore", i), "undo", t).on_click(cx.listener(
                                move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.store.update(cx, |s, cx| s.restore(&b, cx))
                                },
                            )))
                            .child(icon_button(("trash-del", i), "trash", t).on_click(cx.listener(
                                move |this, _, window, cx| {
                                    cx.stop_propagation();
                                    this.store.update(cx, |s, cx| s.delete_forever(&c, cx));
                                    let gone = this.current.as_ref().is_some_and(|id| this.store.read(cx).page(id).is_none());
                                    if gone {
                                        let first = this.first_page(cx);
                                        this.show(first, window, cx);
                                        this.menu = Some((Menu::Trash, Point::default()));
                                    }
                                },
                            ))),
                    );
                }
                surface = surface.child(list);
                corner = Corner::BottomLeft;
            }
        }
        let pos = if *menu == Menu::Trash && *pos == Point::default() { gpui::point(px(250.), px(400.)) } else { *pos };
        Some(popover(
            pos,
            corner,
            surface,
            cx.listener(|this, _, _, cx| {
                this.menu = None;
                cx.notify();
            }),
        ))
    }

    fn render_search(&self, s: &Store, t: &Theme, cx: &Context<Self>) -> Option<AnyElement> {
        let search = self.search.as_ref()?;
        let hits = self.search_hits(cx);
        let query_empty = search.input.read(cx).text().trim().is_empty();
        let mut list = div().id("search-list").flex().flex_col().max_h(px(420.)).overflow_y_scroll().py(px(6.));
        if hits.is_empty() {
            list = list.child(div().px(px(16.)).py(px(18.)).text_color(t.text_faint).child("No results"));
        } else if query_empty {
            list = list.child(menu_label("Recently edited", t));
        }
        for (i, (id, snippet)) in hits.iter().enumerate() {
            let Some(page) = s.page(id) else { continue };
            let hover = t.hover;
            let pid = id.clone();
            let trail: Vec<String> = s.vault.ancestors(id).iter().map(|p| p.display_title().to_string()).collect();
            let sub = if !snippet.is_empty() { snippet.clone() } else { trail.join(" / ") };
            let mut row = div()
                .id(("hit", i))
                .flex()
                .flex_none()
                .items_center()
                .gap(px(10.))
                .min_h(px(36.))
                .mx(px(6.))
                .px(px(10.))
                .rounded(px(6.))
                .cursor_pointer()
                .hover(move |s| s.bg(hover))
                .on_click(cx.listener(move |this, _, window, cx| this.open(pid.clone(), window, cx)))
                .child(page_icon(page.icon.as_deref(), page.is_database(), 16., t))
                .child(
                    div()
                        .flex_none()
                        .max_w(px(300.))
                        .font_weight(FontWeight::MEDIUM)
                        .whitespace_nowrap()
                        .overflow_hidden()
                        .text_ellipsis()
                        .child(SharedString::from(page.display_title().to_string())),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(px(12.))
                        .text_color(t.text_faint)
                        .whitespace_nowrap()
                        .overflow_hidden()
                        .text_ellipsis()
                        .child(SharedString::from(sub)),
                );
            if i == search.selected {
                row = row.bg(t.hover);
            }
            list = list.child(row);
        }
        let panel = div()
            .occlude()
            .flex()
            .flex_col()
            .w(px(640.))
            .mt(px(110.))
            .rounded(px(12.))
            .bg(t.panel_bg)
            .border_1()
            .border_color(t.border)
            .shadow(popover_shadow(t))
            .text_size(px(14.))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .h(px(52.))
                    .px(px(16.))
                    .border_b_1()
                    .border_color(t.border)
                    .child(icon("search", 20., t.text_faint))
                    .child(div().flex_1().child(search.input.clone())),
            )
            .child(list)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(14.))
                    .h(px(32.))
                    .px(px(16.))
                    .border_t_1()
                    .border_color(t.border)
                    .text_size(px(12.))
                    .text_color(t.text_faint)
                    .child("↑↓ Select")
                    .child("↵ Open")
                    .child("Esc Close"),
            );
        Some(
            deferred(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .flex()
                    .justify_center()
                    .items_start()
                    .bg(t.overlay)
                    .occlude()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, window, cx| {
                            this.search = None;
                            window.focus(&this.focus);
                            cx.notify();
                        }),
                    )
                    .child(panel),
            )
            .with_priority(2)
            .into_any_element(),
        )
    }
}

impl Render for Workspace {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        if !cx.has_active_drag() && self.side_drop.is_some() {
            self.side_drop = None;
        }
        let store = self.store.clone();
        let cx: &Context<Self> = cx;
        let s = store.read(cx);

        let mut main = div().flex().flex_col().flex_1().min_w_0().h_full().bg(t.bg).child(self.render_topbar(s, &t, cx));
        main = main.child(match &self.page {
            Some(page) => div().flex_1().min_h_0().child(page.clone()).into_any_element(),
            None => div()
                .flex_1()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(12.))
                .text_color(t.text_faint)
                .child("No page open")
                .child(primary_button("empty-new", "New page", &t).on_click(cx.listener(|this, _, window, cx| {
                    this.new_page(None, PageKind::Page, window, cx)
                })))
                .into_any_element(),
        });

        let mut root = div()
            .id("workspace")
            .track_focus(&self.focus)
            .relative()
            .flex()
            .size_full()
            .bg(t.bg)
            .text_color(t.text)
            .font_family(crate::theme::ui_font())
            .text_size(px(14.))
            .line_height(px(20.))
            .on_action(cx.listener(|this, _: &NewPage, window, cx| this.new_page(None, PageKind::Page, window, cx)))
            .on_action(cx.listener(|this, _: &ToggleSidebar, _, cx| this.toggle_sidebar(cx)))
            .on_action(cx.listener(|this, _: &ToggleAi, window, cx| this.toggle_ai(window, cx)))
            .on_action(cx.listener(|this, _: &QuickFind, window, cx| this.open_search(window, cx)))
            .on_action(cx.listener(|this, _: &ToggleTheme, _, cx| this.toggle_theme(cx)))
            .on_action(cx.listener(|this, _: &CheckForUpdates, _, cx| {
                this.check_for_updates(cx);
                this.menu = Some((Menu::Settings, gpui::point(px(250.), px(420.))));
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &GoBack, window, cx| this.go_back(window, cx)))
            .on_action(cx.listener(|this, _: &GoForward, window, cx| this.go_forward(window, cx)));
        if !s.state.sidebar_collapsed {
            root = root.child(self.render_sidebar(s, &t, cx));
        }
        root = root.child(main);
        if s.state.ai_open {
            root = root.child(
                div()
                    .flex_none()
                    .w(px(AI_PANEL_W))
                    .h_full()
                    .border_l_1()
                    .border_color(t.border)
                    .child(self.ai.clone()),
            );
        }
        if let Some(err) = &s.last_error {
            root = root.child(
                div()
                    .id("error-toast")
                    .absolute()
                    .bottom(px(16.))
                    .left(px(SIDEBAR_W + 16.))
                    .px(px(12.))
                    .py(px(8.))
                    .rounded(px(8.))
                    .bg(t.danger)
                    .text_color(gpui::white())
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.store.update(cx, |s, cx| {
                            s.last_error = None;
                            cx.notify();
                        })
                    }))
                    .child(SharedString::from(err.clone())),
            );
        }
        if let Some(menu) = self.render_menu(s, &t, cx) {
            root = root.child(menu);
        }
        if let Some(search) = self.render_search(s, &t, cx) {
            root = root.child(search);
        }
        root
    }
}
