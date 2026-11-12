//! The shared, observable model: the vault plus debounced autosave.

use gpui::{Context, EventEmitter, Task};
use nota_core::{Anchor, AppState, Page, PageKind, PropValue, Vault};
use std::collections::{HashMap, HashSet};
use std::time::Duration;

pub enum StoreEvent {
    /// The vault was re-read from disk (e.g. after the AI edited files).
    Reloaded,
    /// A page's body was changed by something other than its editor.
    BlocksChanged(String),
}

pub struct Store {
    pub vault: Vault,
    pub state: AppState,
    dirty: HashSet<String>,
    save_task: Option<Task<()>>,
    pub last_error: Option<String>,
    /// Which view is showing for each database (per session).
    pub active_views: HashMap<String, usize>,
}

impl EventEmitter<StoreEvent> for Store {}

impl Store {
    pub fn new(vault: Vault, cx: &mut Context<Self>) -> Self {
        let state = vault.load_state();
        cx.on_app_quit(|this: &mut Self, _cx| {
            this.flush();
            this.save_state();
            async {}
        })
        .detach();
        Store {
            vault,
            state,
            dirty: HashSet::new(),
            save_task: None,
            last_error: None,
            active_views: HashMap::new(),
        }
    }

    pub fn page(&self, id: &str) -> Option<&Page> {
        self.vault.page(id)
    }

    /// Mark a page as changed: bumps its timestamp and schedules a save.
    pub fn mark_dirty(&mut self, id: &str, cx: &mut Context<Self>) {
        self.vault.touch(id);
        self.dirty.insert(id.to_string());
        self.schedule_save(cx);
        cx.notify();
    }

    fn schedule_save(&mut self, cx: &mut Context<Self>) {
        self.save_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(500))
                .await;
            this.update(cx, |this, cx| {
                if this.flush() {
                    this.reload(cx);
                }
            })
            .ok();
        }));
    }

    /// Write every pending change to disk now. Returns true when a page was
    /// changed on disk by another program meanwhile: that page is left as
    /// the other program wrote it and the caller should reload.
    pub fn flush(&mut self) -> bool {
        let ids: Vec<String> = self.dirty.drain().collect();
        let mut changed = Vec::new();
        for id in ids {
            if self.vault.page(&id).is_none() {
                continue;
            }
            if let Err(err) = self.vault.save_page(&id) {
                match err.downcast_ref::<nota_core::ChangedOnDisk>() {
                    Some(c) => changed.push(c.0.clone()),
                    None => self.last_error = Some(format!("Could not save: {err:#}")),
                }
            }
        }
        if !changed.is_empty() {
            self.last_error = Some(format!(
                "{} changed on disk (by Nota AI or another app) — showing that version; your last few edits to it were not kept.",
                changed.iter().map(|t| format!("“{t}”")).collect::<Vec<_>>().join(", ")
            ));
        }
        !changed.is_empty()
    }

    pub fn update_page(&mut self, id: &str, cx: &mut Context<Self>, f: impl FnOnce(&mut Page)) {
        if let Some(page) = self.vault.page_mut(id) {
            f(page);
            self.mark_dirty(id, cx);
        }
    }

    pub fn create_page(&mut self, kind: PageKind, parent: Option<String>, cx: &mut Context<Self>) -> String {
        let id = self.vault.create_page(kind, parent);
        self.mark_dirty(&id, cx);
        id
    }

    pub fn trash(&mut self, id: &str, cx: &mut Context<Self>) {
        self.update_page(id, cx, |p| p.trashed = true);
    }

    pub fn restore(&mut self, id: &str, cx: &mut Context<Self>) {
        // If the old parent is gone or itself trashed, restore to the top level.
        let orphan = self
            .vault
            .page(id)
            .and_then(|p| p.parent.clone())
            .is_some_and(|parent| self.vault.page(&parent).is_none() || self.vault.is_trashed(&parent));
        self.update_page(id, cx, |p| {
            p.trashed = false;
            if orphan {
                p.parent = None;
            }
        });
    }

    pub fn delete_forever(&mut self, id: &str, cx: &mut Context<Self>) {
        for d in self.vault.descendants(id) {
            self.dirty.remove(&d);
        }
        self.dirty.remove(id);
        if let Err(err) = self.vault.delete_page(id) {
            self.last_error = Some(format!("Could not delete: {err:#}"));
        }
        cx.notify();
    }

    pub fn mark_many(&mut self, ids: &[String], cx: &mut Context<Self>) {
        for id in ids {
            self.dirty.insert(id.clone());
        }
        self.schedule_save(cx);
        cx.notify();
    }

    /// Move a page among its (new) siblings; saves every page whose order changed.
    pub fn place_page(&mut self, id: &str, parent: Option<String>, anchor: Anchor, cx: &mut Context<Self>) {
        let touched = self.vault.place_page(id, parent, anchor);
        self.vault.touch(id);
        self.mark_many(&touched, cx);
    }

    /// Flush, then re-read the whole vault from disk.
    pub fn reload(&mut self, cx: &mut Context<Self>) {
        self.flush();
        if let Err(err) = self.vault.load() {
            self.last_error = Some(format!("Could not reload the vault: {err:#}"));
        }
        cx.emit(StoreEvent::Reloaded);
        cx.notify();
    }

    pub fn save_state(&self) {
        self.vault.save_state(&self.state);
    }

    pub fn is_expanded(&self, id: &str) -> bool {
        self.state.expanded.iter().any(|e| e == id)
    }

    pub fn set_expanded(&mut self, id: &str, expanded: bool, cx: &mut Context<Self>) {
        self.state.expanded.retain(|e| e != id);
        if expanded {
            self.state.expanded.push(id.to_string());
        }
        self.save_state();
        cx.notify();
    }

    /// Set (or clear, with `None`) a database property on a row.
    pub fn set_prop(&mut self, row: &str, prop: &str, value: Option<PropValue>, cx: &mut Context<Self>) {
        let prop = prop.to_string();
        self.update_page(row, cx, move |p| match value {
            Some(v) if !(v.is_empty() && !matches!(v, PropValue::Bool(_))) => {
                p.props.insert(prop, v);
            }
            _ => {
                p.props.remove(&prop);
            }
        });
    }

    /// Deep-copy a page (with its sub-pages / rows). Returns the id of the copy.
    pub fn duplicate_page(&mut self, id: &str, cx: &mut Context<Self>) -> Option<String> {
        fn copy(store: &mut Store, id: &str, parent: Option<String>, top: bool, cx: &mut Context<Store>) -> Option<String> {
            let src = store.vault.page(id)?.clone();
            if src.trashed && !top {
                return None;
            }
            let new_id = store.vault.create_page(src.kind, parent);
            let children: Vec<String> = store.vault.children(Some(id)).iter().map(|p| p.id.clone()).collect();
            let mut mapping = HashMap::new();
            for child in children {
                if let Some(c) = copy(store, &child, Some(new_id.clone()), false, cx) {
                    mapping.insert(child, c);
                }
            }
            let page = store.vault.page_mut(&new_id)?;
            page.title = if top && !src.title.is_empty() { format!("{} (copy)", src.title) } else { src.title.clone() };
            page.icon = src.icon.clone();
            page.props = src.props.clone();
            page.database = src.database.clone();
            page.blocks = src.blocks.iter().map(|b| {
                let mut b = b.duplicate();
                if let nota_core::BlockKind::PageLink { page } = &mut b.kind {
                    if let Some(n) = mapping.get(page) {
                        *page = n.clone();
                    }
                }
                b
            }).collect();
            if top {
                page.order = src.order + 0.5;
            } else {
                page.order = src.order;
            }
            store.mark_dirty(&new_id, cx);
            Some(new_id)
        }
        let parent = self.vault.page(id)?.parent.clone();
        copy(self, id, parent, true, cx)
    }

    /// Point the app at a different vault folder.
    pub fn switch_vault(&mut self, vault: Vault, cx: &mut Context<Self>) {
        self.flush();
        self.save_state();
        self.state = vault.load_state();
        self.vault = vault;
        self.active_views.clear();
        cx.emit(StoreEvent::Reloaded);
        cx.notify();
    }
}
