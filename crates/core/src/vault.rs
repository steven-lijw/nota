//! The vault: a folder of Markdown files, one per page.
//!
//! Every page (and every database row) is a `.md` file with YAML front matter:
//!
//! ```text
//! ---
//! id: 4f2a…            # stable identity
//! title: Weekly plan
//! icon: "📅"
//! parent: 9c1b…        # parent page or database id
//! props:               # database rows only, keyed by property name
//!   Status: In progress
//! ---
//! # Markdown body…
//! ```
//!
//! Databases are pages with `kind: database` plus `schema` and `views`.
//! File names are `<Title> <id8>.md` and are only cosmetic: identity and
//! hierarchy live in the front matter, so files can be moved into sub-folders.

use crate::markdown::{from_markdown, to_markdown};
use crate::model::*;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

fn is_false(b: &bool) -> bool {
    !*b
}

fn is_page(k: &PageKind) -> bool {
    *k == PageKind::Page
}

#[derive(Serialize, Deserialize, Default)]
struct FrontMatter {
    #[serde(default)]
    id: String,
    #[serde(default)]
    title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    icon: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    parent: Option<String>,
    #[serde(default, skip_serializing_if = "is_page")]
    kind: PageKind,
    #[serde(default)]
    created: String,
    #[serde(default)]
    updated: String,
    #[serde(default, skip_serializing_if = "is_false")]
    favorite: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    trashed: bool,
    #[serde(default)]
    order: f64,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    props: BTreeMap<String, PropValue>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    schema: Vec<Property>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    views: Vec<View>,
}

/// Split a file into (front matter yaml, body).
fn split_front_matter(content: &str) -> (Option<&str>, &str) {
    let content = content.strip_prefix('\u{feff}').unwrap_or(content);
    let Some(rest) = content
        .strip_prefix("---\n")
        .or_else(|| content.strip_prefix("---\r\n"))
    else {
        return (None, content);
    };
    let mut offset = 0;
    for line in rest.split_inclusive('\n') {
        if line.trim_end() == "---" {
            let yaml = &rest[..offset];
            let body = &rest[offset + line.len()..];
            return (Some(yaml), body);
        }
        offset += line.len();
    }
    (None, content)
}

pub fn page_to_file_content(page: &Page) -> String {
    let (schema, views) = match &page.database {
        Some(db) => (db.schema.clone(), db.views.clone()),
        None => (vec![], vec![]),
    };
    let fm = FrontMatter {
        id: page.id.clone(),
        title: page.title.clone(),
        icon: page.icon.clone(),
        parent: page.parent.clone(),
        kind: page.kind,
        created: page.created.clone(),
        updated: page.updated.clone(),
        favorite: page.favorite,
        trashed: page.trashed,
        order: page.order,
        props: page.props.clone(),
        schema,
        views,
    };
    let yaml = serde_yaml::to_string(&fm).unwrap_or_default();
    format!("---\n{}---\n\n{}", yaml, to_markdown(&page.blocks))
}

/// Parse a vault file. Files without front matter (dropped in from elsewhere)
/// are imported as new top-level pages.
pub fn page_from_file_content(content: &str, fallback_title: &str) -> Page {
    let (yaml, body) = split_front_matter(content);
    let fm: Option<FrontMatter> = yaml.and_then(|y| serde_yaml::from_str(y).ok());
    let has_id = fm.as_ref().is_some_and(|f| !f.id.is_empty());
    let fm = fm.unwrap_or_default();
    let mut blocks = from_markdown(body);

    let mut title = fm.title;
    if !has_id && title.is_empty() {
        // Use a leading H1 as the title, like most Markdown tools do.
        if blocks
            .first()
            .is_some_and(|b| b.kind == BlockKind::Heading(1) && !b.text.is_empty())
        {
            title = blocks.remove(0).text.text;
        } else {
            title = fallback_title.to_string();
        }
    }
    if blocks.is_empty() {
        blocks.push(Block::paragraph(""));
    }
    let now = now();
    let database = if fm.kind == PageKind::Database {
        let mut db = Database {
            schema: fm.schema,
            views: fm.views,
        };
        if !db.schema.iter().any(|p| p.kind == PropKind::Title) {
            db.schema.insert(0, Property::new("Name", PropKind::Title));
        }
        if db.views.is_empty() {
            db.views.push(View::new("Table", ViewKind::Table));
        }
        Some(db)
    } else {
        None
    };
    Page {
        id: if has_id { fm.id } else { new_id() },
        title,
        icon: fm.icon.filter(|i| !i.is_empty()),
        parent: fm.parent.filter(|p| !p.is_empty()),
        kind: fm.kind,
        created: if fm.created.is_empty() {
            now.clone()
        } else {
            fm.created
        },
        updated: if fm.updated.is_empty() { now } else { fm.updated },
        favorite: fm.favorite,
        trashed: fm.trashed,
        order: fm.order,
        props: fm.props,
        database,
        blocks,
        file: None,
    }
}

pub fn sanitize_file_name(title: &str) -> String {
    let mut s: String = title
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => ' ',
            c if c.is_control() => ' ',
            c => c,
        })
        .collect();
    s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    let s = s.trim_matches(|c: char| c == '.' || c.is_whitespace());
    let mut out = String::new();
    for c in s.chars() {
        if out.len() + c.len_utf8() > 80 {
            break;
        }
        out.push(c);
    }
    if out.is_empty() {
        "Untitled".to_string()
    } else {
        out
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct AppState {
    #[serde(default)]
    pub expanded: Vec<String>,
    #[serde(default)]
    pub last_page: Option<String>,
    #[serde(default)]
    pub dark: bool,
    #[serde(default)]
    pub ai_open: bool,
    #[serde(default)]
    pub sidebar_collapsed: bool,
    #[serde(default)]
    pub ai_allow_edits: bool,
}

/// Where to put a page among its siblings.
#[derive(Clone, Debug, PartialEq)]
pub enum Anchor {
    Start,
    End,
    Before(String),
    After(String),
}

#[derive(Clone, Debug)]
pub struct SearchHit {
    pub id: String,
    pub snippet: String,
}

pub struct Vault {
    pub root: PathBuf,
    pub pages: HashMap<String, Page>,
}

impl Vault {
    /// Open (or create) a vault. A brand new vault gets some starter content.
    pub fn open(root: impl Into<PathBuf>) -> Result<Vault> {
        let root: PathBuf = root.into();
        std::fs::create_dir_all(&root)
            .with_context(|| format!("cannot create vault folder {}", root.display()))?;
        let mut vault = Vault {
            root,
            pages: HashMap::new(),
        };
        vault.load()?;
        if vault.pages.is_empty() {
            crate::sample::seed(&mut vault)?;
        }
        Ok(vault)
    }

    /// (Re)load every page from disk.
    pub fn load(&mut self) -> Result<()> {
        let mut pages = HashMap::new();
        let root = self.root.clone();
        Self::scan_dir(&root, &root, &mut pages)?;
        self.pages = pages;
        Ok(())
    }

    fn scan_dir(root: &Path, dir: &Path, out: &mut HashMap<String, Page>) -> Result<()> {
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(_) => return Ok(()),
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with('.') {
                continue;
            }
            if path.is_dir() {
                Self::scan_dir(root, &path, out)?;
            } else if path.extension().is_some_and(|e| e == "md") {
                let Ok(content) = std::fs::read_to_string(&path) else {
                    continue;
                };
                let stem = path
                    .file_stem()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_default();
                let mut page = page_from_file_content(&content, &stem);
                page.file = path.strip_prefix(root).ok().map(|p| p.to_path_buf());
                if out.contains_key(&page.id) {
                    // duplicate id (a copied file): give the copy a fresh identity
                    page.id = new_id();
                }
                out.insert(page.id.clone(), page);
            }
        }
        Ok(())
    }

    pub fn state_dir(&self) -> PathBuf {
        self.root.join(".nota")
    }

    pub fn load_state(&self) -> AppState {
        std::fs::read_to_string(self.state_dir().join("state.json"))
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save_state(&self, state: &AppState) {
        let dir = self.state_dir();
        if std::fs::create_dir_all(&dir).is_ok() {
            if let Ok(json) = serde_json::to_string_pretty(state) {
                let _ = std::fs::write(dir.join("state.json"), json);
            }
        }
    }

    pub fn page(&self, id: &str) -> Option<&Page> {
        self.pages.get(id)
    }

    pub fn page_mut(&mut self, id: &str) -> Option<&mut Page> {
        self.pages.get_mut(id)
    }

    fn sort_pages(pages: &mut [&Page]) {
        pages.sort_by(|a, b| {
            a.order
                .partial_cmp(&b.order)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.created.cmp(&b.created))
                .then_with(|| a.id.cmp(&b.id))
        });
    }

    /// Live (non-trashed) children of `parent`, in sidebar order.
    pub fn children(&self, parent: Option<&str>) -> Vec<&Page> {
        let mut v: Vec<&Page> = self
            .pages
            .values()
            .filter(|p| !p.trashed)
            .filter(|p| match (&p.parent, parent) {
                (None, None) => true,
                (Some(a), Some(b)) => a == b,
                // orphans (parent no longer exists) show up at the top level
                (Some(a), None) => !self.pages.contains_key(a),
                _ => false,
            })
            .collect();
        Self::sort_pages(&mut v);
        v
    }

    /// Children shown in the sidebar tree: database rows are not listed there.
    pub fn sidebar_children(&self, parent: Option<&str>) -> Vec<&Page> {
        if let Some(pid) = parent {
            if self.pages.get(pid).is_some_and(|p| p.is_database()) {
                return vec![];
            }
        }
        self.children(parent)
    }

    pub fn rows(&self, db_id: &str) -> Vec<&Page> {
        self.children(Some(db_id))
    }

    pub fn favorites(&self) -> Vec<&Page> {
        let mut v: Vec<&Page> = self
            .pages
            .values()
            .filter(|p| p.favorite && !self.is_trashed(&p.id))
            .collect();
        v.sort_by(|a, b| a.title.to_lowercase().cmp(&b.title.to_lowercase()));
        v
    }

    /// Ancestors from the root down, excluding the page itself.
    pub fn ancestors(&self, id: &str) -> Vec<&Page> {
        let mut out = Vec::new();
        let mut cur = self.pages.get(id).and_then(|p| p.parent.clone());
        let mut guard = 0;
        while let Some(pid) = cur {
            guard += 1;
            if guard > 64 {
                break;
            }
            match self.pages.get(&pid) {
                Some(p) => {
                    out.push(p);
                    cur = p.parent.clone();
                }
                None => break,
            }
        }
        out.reverse();
        out
    }

    pub fn is_trashed(&self, id: &str) -> bool {
        match self.pages.get(id) {
            Some(p) => p.trashed || self.ancestors(id).iter().any(|a| a.trashed),
            None => false,
        }
    }

    /// Pages explicitly moved to the trash (their descendants go with them).
    pub fn trashed_roots(&self) -> Vec<&Page> {
        let mut v: Vec<&Page> = self.pages.values().filter(|p| p.trashed).collect();
        v.sort_by(|a, b| b.updated.cmp(&a.updated));
        v
    }

    pub fn descendants(&self, id: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut stack = vec![id.to_string()];
        while let Some(cur) = stack.pop() {
            for p in self.pages.values() {
                if p.parent.as_deref() == Some(cur.as_str()) {
                    out.push(p.id.clone());
                    stack.push(p.id.clone());
                }
            }
        }
        out
    }

    /// The database a page is a row of, if any.
    pub fn parent_database(&self, id: &str) -> Option<&Page> {
        let parent = self.pages.get(id)?.parent.as_ref()?;
        self.pages.get(parent).filter(|p| p.is_database())
    }

    pub fn create_page(&mut self, kind: PageKind, parent: Option<String>) -> String {
        let mut page = Page::new(kind, parent.clone());
        let max = self
            .pages
            .values()
            .filter(|p| p.parent == parent)
            .map(|p| p.order)
            .fold(0.0_f64, f64::max);
        page.order = max + 1.0;
        let id = page.id.clone();
        self.pages.insert(id.clone(), page);
        id
    }

    /// Put page `id` under `parent` at the position given by `anchor`
    /// (relative to its new siblings). Returns every page whose `parent` or
    /// `order` changed. Orders are spread out again only when there is no
    /// room left between the neighbours.
    pub fn place_page(&mut self, id: &str, parent: Option<String>, anchor: Anchor) -> Vec<String> {
        if !self.pages.contains_key(id) {
            return vec![];
        }
        let siblings: Vec<(String, f64)> = self
            .children(parent.as_deref())
            .into_iter()
            .filter(|p| p.id != id)
            .map(|p| (p.id.clone(), p.order))
            .collect();
        let pos = match &anchor {
            Anchor::Start => 0,
            Anchor::End => siblings.len(),
            Anchor::Before(b) => siblings.iter().position(|s| &s.0 == b).unwrap_or(siblings.len()),
            Anchor::After(a) => siblings.iter().position(|s| &s.0 == a).map_or(siblings.len(), |i| i + 1),
        };
        let prev = pos.checked_sub(1).map(|i| siblings[i].1);
        let next = siblings.get(pos).map(|s| s.1);
        let mut touched = vec![id.to_string()];
        let order = match (prev, next) {
            (None, None) => Some(1.0),
            (Some(p), None) => Some(p + 1.0),
            (None, Some(n)) => Some(n - 1.0),
            (Some(p), Some(n)) if n - p > 1e-6 => Some((p + n) / 2.0),
            _ => None,
        };
        let order = match order {
            Some(o) => o,
            None => {
                // no gap: renumber the siblings 1, 2, 3… with room for this page
                for (i, (sid, _)) in siblings.iter().enumerate() {
                    let o = (i + if i >= pos { 2 } else { 1 }) as f64;
                    if let Some(p) = self.pages.get_mut(sid) {
                        if p.order != o {
                            p.order = o;
                            touched.push(sid.clone());
                        }
                    }
                }
                (pos + 1) as f64
            }
        };
        if let Some(p) = self.pages.get_mut(id) {
            p.parent = parent;
            p.order = order;
        }
        touched
    }

    pub fn touch(&mut self, id: &str) {
        if let Some(p) = self.pages.get_mut(id) {
            p.updated = now();
        }
    }

    pub fn abs_path(&self, id: &str) -> Option<PathBuf> {
        self.pages
            .get(id)
            .and_then(|p| p.file.as_ref())
            .map(|f| self.root.join(f))
    }

    /// Write one page to disk (atomically), renaming the file if the title changed.
    pub fn save_page(&mut self, id: &str) -> Result<()> {
        let root = self.root.clone();
        let Some(page) = self.pages.get_mut(id) else {
            return Ok(());
        };
        let short = &page.id[..page.id.len().min(8)];
        let desired = format!("{} {}.md", sanitize_file_name(&page.title), short);
        let old_rel = page.file.clone();
        let dir_rel = old_rel
            .as_ref()
            .and_then(|f| f.parent().map(|p| p.to_path_buf()))
            .unwrap_or_default();
        let new_rel = dir_rel.join(&desired);
        let new_abs = root.join(&new_rel);
        if let Some(parent) = new_abs.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content = page_to_file_content(page);
        let tmp = new_abs.with_extension("md.tmp");
        std::fs::write(&tmp, content).with_context(|| format!("writing {}", tmp.display()))?;
        std::fs::rename(&tmp, &new_abs)?;
        if let Some(old_rel) = old_rel {
            if old_rel != new_rel {
                let _ = std::fs::remove_file(root.join(old_rel));
            }
        }
        page.file = Some(new_rel);
        Ok(())
    }

    pub fn save_all(&mut self) -> Result<()> {
        let ids: Vec<String> = self.pages.keys().cloned().collect();
        for id in ids {
            self.save_page(&id)?;
        }
        Ok(())
    }

    /// Permanently delete a page and everything under it.
    pub fn delete_page(&mut self, id: &str) -> Result<()> {
        let mut ids = self.descendants(id);
        ids.push(id.to_string());
        for id in ids {
            if let Some(page) = self.pages.remove(&id) {
                if let Some(file) = page.file {
                    let _ = std::fs::remove_file(self.root.join(file));
                }
            }
        }
        Ok(())
    }

    /// Case-insensitive search over titles and body text.
    pub fn search(&self, query: &str) -> Vec<SearchHit> {
        let q = query.trim().to_lowercase();
        let mut hits: Vec<(u8, String, SearchHit)> = Vec::new();
        for p in self.pages.values() {
            if self.is_trashed(&p.id) {
                continue;
            }
            let title_l = p.title.to_lowercase();
            if q.is_empty() {
                hits.push((
                    0,
                    p.updated.clone(),
                    SearchHit {
                        id: p.id.clone(),
                        snippet: String::new(),
                    },
                ));
                continue;
            }
            if title_l.contains(&q) {
                let rank = if title_l.starts_with(&q) { 0 } else { 1 };
                hits.push((
                    rank,
                    p.updated.clone(),
                    SearchHit {
                        id: p.id.clone(),
                        snippet: String::new(),
                    },
                ));
                continue;
            }
            for b in &p.blocks {
                let text = &b.text.text;
                let lower = text.to_lowercase();
                if let Some(pos) = lower.find(&q) {
                    // map back approximately; lowercase can change byte lengths
                    let mut start = pos.min(text.len());
                    while start > 0 && !text.is_char_boundary(start) {
                        start -= 1;
                    }
                    let mut from = start.saturating_sub(30);
                    while from > 0 && !text.is_char_boundary(from) {
                        from -= 1;
                    }
                    let mut to = (start + 90).min(text.len());
                    while to < text.len() && !text.is_char_boundary(to) {
                        to += 1;
                    }
                    let mut snippet = text[from..to].replace('\n', " ");
                    if from > 0 {
                        snippet.insert(0, '…');
                    }
                    if to < text.len() {
                        snippet.push('…');
                    }
                    hits.push((
                        2,
                        p.updated.clone(),
                        SearchHit {
                            id: p.id.clone(),
                            snippet,
                        },
                    ));
                    break;
                }
            }
        }
        hits.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| b.1.cmp(&a.1)));
        hits.into_iter().map(|h| h.2).take(50).collect()
    }

    /// Rename a database property everywhere it is referenced.
    pub fn rename_property(&mut self, db_id: &str, old: &str, new: &str) -> Vec<String> {
        let mut touched = vec![];
        if old == new || new.is_empty() {
            return touched;
        }
        if let Some(db) = self.pages.get_mut(db_id).and_then(|p| p.database.as_mut()) {
            if db.prop(new).is_some() {
                return touched;
            }
            for p in &mut db.schema {
                if p.name == old {
                    p.name = new.to_string();
                }
            }
            for v in &mut db.views {
                if v.group_by.as_deref() == Some(old) {
                    v.group_by = Some(new.to_string());
                }
                if v.date_prop.as_deref() == Some(old) {
                    v.date_prop = Some(new.to_string());
                }
                for s in &mut v.sorts {
                    if s.prop == old {
                        s.prop = new.to_string();
                    }
                }
                for f in &mut v.filters {
                    if f.prop == old {
                        f.prop = new.to_string();
                    }
                }
                for h in &mut v.hidden {
                    if h == old {
                        *h = new.to_string();
                    }
                }
                if let Some(w) = v.widths.remove(old) {
                    v.widths.insert(new.to_string(), w);
                }
            }
            touched.push(db_id.to_string());
        }
        for p in self.pages.values_mut() {
            if p.parent.as_deref() == Some(db_id) {
                if let Some(v) = p.props.remove(old) {
                    p.props.insert(new.to_string(), v);
                    touched.push(p.id.clone());
                }
            }
        }
        touched
    }

    /// Remove a property from a database and all of its rows.
    pub fn delete_property(&mut self, db_id: &str, name: &str) -> Vec<String> {
        let mut touched = vec![];
        if let Some(db) = self.pages.get_mut(db_id).and_then(|p| p.database.as_mut()) {
            db.schema.retain(|p| p.name != name || p.kind == PropKind::Title);
            for v in &mut db.views {
                if v.group_by.as_deref() == Some(name) {
                    v.group_by = None;
                }
                if v.date_prop.as_deref() == Some(name) {
                    v.date_prop = None;
                }
                v.sorts.retain(|s| s.prop != name);
                v.filters.retain(|f| f.prop != name);
                v.hidden.retain(|h| h != name);
                v.widths.remove(name);
            }
            touched.push(db_id.to_string());
        }
        for p in self.pages.values_mut() {
            if p.parent.as_deref() == Some(db_id) && p.props.remove(name).is_some() {
                touched.push(p.id.clone());
            }
        }
        touched
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rich_text::MarkKind;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("nota-test-{}-{}", name, new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn place_page_orders() {
        let mut vault = Vault { root: temp_dir("place"), pages: HashMap::new() };
        let a = vault.create_page(PageKind::Page, None);
        let b = vault.create_page(PageKind::Page, None);
        let c = vault.create_page(PageKind::Page, None);
        let ids = |v: &Vault| v.children(None).iter().map(|p| p.id.clone()).collect::<Vec<_>>();
        vault.place_page(&c, None, Anchor::Before(a.clone()));
        assert_eq!(ids(&vault), vec![c.clone(), a.clone(), b.clone()]);
        let touched = vault.place_page(&c, None, Anchor::After(a.clone()));
        assert_eq!(touched, vec![c.clone()]);
        assert_eq!(ids(&vault), vec![a.clone(), c.clone(), b.clone()]);
        // equal orders (e.g. imported files) force a renumber
        for p in vault.pages.values_mut() {
            p.order = 0.0;
        }
        let before = ids(&vault);
        let moved = before[2].clone();
        vault.place_page(&moved, None, Anchor::Before(before[0].clone()));
        assert_eq!(ids(&vault), vec![moved, before[0].clone(), before[1].clone()]);
        // into another parent
        vault.place_page(&b, Some(a.clone()), Anchor::End);
        assert_eq!(vault.children(Some(&a)).len(), 1);
    }

    #[test]
    fn front_matter_split() {
        let (y, b) = split_front_matter("---\nid: a\n---\n\nbody\n---\nmore");
        assert_eq!(y, Some("id: a\n"));
        assert_eq!(b, "\nbody\n---\nmore");
        let (y, b) = split_front_matter("no front matter");
        assert_eq!(y, None);
        assert_eq!(b, "no front matter");
    }

    #[test]
    fn save_and_reload() {
        let dir = temp_dir("save");
        let mut vault = Vault {
            root: dir.clone(),
            pages: HashMap::new(),
        };
        let id = vault.create_page(PageKind::Page, None);
        {
            let p = vault.page_mut(&id).unwrap();
            p.title = "Plan: Q4 / 2026?".into();
            p.icon = Some("📅".into());
            let mut t = crate::rich_text::RichText::plain("hello world");
            t.toggle(0..5, MarkKind::Bold);
            p.blocks = vec![Block::paragraph(t), Block::new(BlockKind::Todo { checked: true }, "x")];
        }
        let db = vault.create_page(PageKind::Database, None);
        vault.page_mut(&db).unwrap().title = "Tasks".into();
        let row = vault.create_page(PageKind::Page, Some(db.clone()));
        {
            let p = vault.page_mut(&row).unwrap();
            p.title = "Row".into();
            p.props.insert("Tags".into(), PropValue::List(vec!["a".into(), "b".into()]));
            p.props.insert("Done".into(), PropValue::Bool(true));
            p.props.insert("Score".into(), PropValue::Number(3.5));
            p.props.insert("Note".into(), PropValue::Text("123".into()));
            p.props.insert("Due".into(), PropValue::Text("2026-11-20".into()));
        }
        vault.save_all().unwrap();
        let file = vault.page(&id).unwrap().file.clone().unwrap();
        assert!(file.to_string_lossy().starts_with("Plan Q4 2026 "));

        let mut again = Vault {
            root: dir.clone(),
            pages: HashMap::new(),
        };
        again.load().unwrap();
        assert_eq!(again.pages.len(), 3);
        let p = again.page(&id).unwrap();
        assert_eq!(p.title, "Plan: Q4 / 2026?");
        assert_eq!(p.icon.as_deref(), Some("📅"));
        assert_eq!(p.blocks.len(), 2);
        assert!(p.blocks[0].text.range_has(0..5, MarkKind::Bold));
        let r = again.page(&row).unwrap();
        assert_eq!(r.props["Tags"], PropValue::List(vec!["a".into(), "b".into()]));
        assert_eq!(r.props["Done"], PropValue::Bool(true));
        assert_eq!(r.props["Score"], PropValue::Number(3.5));
        assert_eq!(r.props["Note"], PropValue::Text("123".into()));
        assert_eq!(r.props["Due"], PropValue::Text("2026-11-20".into()));
        assert_eq!(again.rows(&db).len(), 1);
        assert!(again.page(&db).unwrap().database.is_some());

        // renaming the page renames the file and removes the old one
        again.page_mut(&id).unwrap().title = "Renamed".into();
        again.save_page(&id).unwrap();
        assert!(!dir.join(&file).exists());
        assert!(again.abs_path(&id).unwrap().exists());

        // property rename propagates to rows
        again.rename_property(&db, "Tags", "Labels");
        assert!(again.page(&row).unwrap().props.contains_key("Labels"));

        again.delete_page(&db).unwrap();
        assert_eq!(again.pages.len(), 1);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn import_plain_markdown() {
        let page = page_from_file_content("# My note\n\nSome text\n", "file-name");
        assert_eq!(page.title, "My note");
        assert_eq!(page.blocks.len(), 1);
        let page = page_from_file_content("Just text\n", "file-name");
        assert_eq!(page.title, "file-name");
    }

    #[test]
    fn seeded_vault_roundtrips() {
        let dir = temp_dir("seed");
        let vault = Vault::open(&dir).unwrap();
        assert!(vault.pages.len() >= 5);
        let again = Vault::open(&dir).unwrap();
        assert_eq!(again.pages.len(), vault.pages.len());
        for (id, p) in &vault.pages {
            let q = again.page(id).unwrap();
            assert_eq!(p.title, q.title);
            assert_eq!(to_markdown(&p.blocks), to_markdown(&q.blocks));
            assert_eq!(p.props, q.props);
            assert_eq!(p.database, q.database);
        }
        std::fs::remove_dir_all(dir).ok();
    }
}
