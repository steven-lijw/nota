//! Data model: blocks, pages, databases.

use crate::rich_text::RichText;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_BLOCK_ID: AtomicU64 = AtomicU64::new(1);

pub fn next_block_id() -> u64 {
    NEXT_BLOCK_ID.fetch_add(1, Ordering::Relaxed)
}

pub fn new_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

pub fn now() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

#[derive(Clone, Debug, PartialEq)]
pub enum BlockKind {
    Paragraph,
    Heading(u8),
    Bullet,
    Numbered,
    Todo { checked: bool },
    Toggle { open: bool },
    Quote,
    Callout { icon: String },
    Divider,
    Code { lang: String },
    /// Link to a sub-page; `text` caches the page title.
    PageLink { page: String },
}

impl BlockKind {
    /// Whether the block holds editable text.
    pub fn has_text(&self) -> bool {
        !matches!(self, BlockKind::Divider | BlockKind::PageLink { .. })
    }

    pub fn is_list(&self) -> bool {
        matches!(
            self,
            BlockKind::Bullet | BlockKind::Numbered | BlockKind::Todo { .. } | BlockKind::Toggle { .. }
        )
    }

    pub fn label(&self) -> &'static str {
        match self {
            BlockKind::Paragraph => "Text",
            BlockKind::Heading(1) => "Heading 1",
            BlockKind::Heading(2) => "Heading 2",
            BlockKind::Heading(_) => "Heading 3",
            BlockKind::Bullet => "Bulleted list",
            BlockKind::Numbered => "Numbered list",
            BlockKind::Todo { .. } => "To-do list",
            BlockKind::Toggle { .. } => "Toggle list",
            BlockKind::Quote => "Quote",
            BlockKind::Callout { .. } => "Callout",
            BlockKind::Divider => "Divider",
            BlockKind::Code { .. } => "Code",
            BlockKind::PageLink { .. } => "Page",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Block {
    /// Runtime-only identity (not persisted).
    pub id: u64,
    pub kind: BlockKind,
    pub text: RichText,
    pub indent: u8,
}

impl Block {
    pub fn new(kind: BlockKind, text: impl Into<RichText>) -> Self {
        Block {
            id: next_block_id(),
            kind,
            text: text.into(),
            indent: 0,
        }
    }

    pub fn paragraph(text: impl Into<RichText>) -> Self {
        Block::new(BlockKind::Paragraph, text)
    }

    pub fn with_indent(mut self, indent: u8) -> Self {
        self.indent = indent;
        self
    }

    /// Clone with a fresh runtime id.
    pub fn duplicate(&self) -> Self {
        let mut b = self.clone();
        b.id = next_block_id();
        b
    }
}

/// Clamp indents so that no block is more than one level deeper than its predecessor.
pub fn normalize_indents(blocks: &mut [Block]) {
    let mut prev: i32 = -1;
    for b in blocks.iter_mut() {
        let max = (prev + 1).max(0) as u8;
        if b.indent > max {
            b.indent = max;
        }
        prev = b.indent as i32;
    }
}

/// End (exclusive) of the subtree rooted at `ix`: following blocks with a deeper indent.
pub fn subtree_end(blocks: &[Block], ix: usize) -> usize {
    let base = blocks[ix].indent;
    let mut end = ix + 1;
    while end < blocks.len() && blocks[end].indent > base {
        end += 1;
    }
    end
}

/// Grow a block range so it ends after the subtree of every block in it.
pub fn expand_to_subtrees(blocks: &[Block], range: std::ops::Range<usize>) -> std::ops::Range<usize> {
    let mut end = range.end.min(blocks.len());
    for ix in range.start..end.min(blocks.len()) {
        end = end.max(subtree_end(blocks, ix));
    }
    range.start..end
}

/// The indents a block range may take when dropped into `gap` (an insert
/// position in the unmodified `blocks`): at least the indent of the block
/// that will follow it (so it never adopts the next block's parent's
/// children) and at most one deeper than the block above.
pub fn drop_indent_range(blocks: &[Block], moving: std::ops::Range<usize>, gap: usize) -> (u8, u8) {
    let prev = (0..gap).rev().find(|i| !moving.contains(i));
    let next = (gap..blocks.len()).find(|i| !moving.contains(i));
    let max = match prev {
        None => 0,
        Some(p) => {
            let closed = matches!(blocks[p].kind, BlockKind::Toggle { open: false });
            // a collapsed toggle hides its children: drop next to it instead
            blocks[p].indent + if closed { 0 } else { 1 }
        }
    };
    let min = next.map_or(0, |n| blocks[n].indent).min(max);
    (min, max)
}

/// Move `range` so it starts at `gap` (an index into the unmodified vector)
/// with its first block at `indent`; the rest keep their relative depth.
/// Returns where the blocks ended up, or `None` if the move is a no-op or
/// would put the range inside itself.
pub fn move_blocks(
    blocks: &mut Vec<Block>,
    range: std::ops::Range<usize>,
    gap: usize,
    indent: u8,
) -> Option<std::ops::Range<usize>> {
    if range.is_empty() || range.end > blocks.len() || gap > blocks.len() {
        return None;
    }
    if gap > range.start && gap < range.end {
        return None;
    }
    let base = blocks[range.start..range.end].iter().map(|b| b.indent).min().unwrap_or(0);
    let first = blocks[range.start].indent;
    let unchanged_place = gap == range.start || gap == range.end;
    if unchanged_place && first == indent {
        return None;
    }
    let len = range.len();
    let mut moved: Vec<Block> = blocks.drain(range.clone()).collect();
    // Shift the whole range so its first block lands on `indent`.
    let shift = indent as i32 - first as i32;
    for b in &mut moved {
        let d = (b.indent as i32 - base as i32) + (base as i32 + shift);
        b.indent = d.max(0) as u8;
    }
    let at = if gap >= range.end { gap - len } else { gap };
    blocks.splice(at..at, moved);
    normalize_indents(blocks);
    Some(at..at + len)
}

/// For each block, whether it is visible (not inside a collapsed toggle).
pub fn visibility(blocks: &[Block]) -> Vec<bool> {
    let mut out = Vec::with_capacity(blocks.len());
    let mut hide_below: Option<u8> = None;
    for b in blocks {
        if let Some(h) = hide_below {
            if b.indent > h {
                out.push(false);
                continue;
            }
            hide_below = None;
        }
        out.push(true);
        if let BlockKind::Toggle { open: false } = b.kind {
            hide_below = Some(b.indent);
        }
    }
    out
}

/// The ordinal shown for a numbered-list block at `ix`.
pub fn list_number(blocks: &[Block], ix: usize) -> usize {
    let indent = blocks[ix].indent;
    let mut n = 1;
    let mut i = ix;
    while i > 0 {
        i -= 1;
        let b = &blocks[i];
        if b.indent > indent {
            continue;
        }
        if b.indent < indent || b.kind != BlockKind::Numbered {
            break;
        }
        n += 1;
    }
    n
}

// ---------------------------------------------------------------------------
// Pages & databases
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PageKind {
    #[default]
    Page,
    Database,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PropValue {
    Bool(bool),
    Number(f64),
    Text(String),
    List(Vec<String>),
}

impl PropValue {
    pub fn as_text(&self) -> String {
        match self {
            PropValue::Bool(b) => b.to_string(),
            PropValue::Number(n) => format_number(*n),
            PropValue::Text(s) => s.clone(),
            PropValue::List(l) => l.join(", "),
        }
    }

    pub fn as_list(&self) -> Vec<String> {
        match self {
            PropValue::List(l) => l.clone(),
            PropValue::Text(s) if !s.is_empty() => vec![s.clone()],
            _ => vec![],
        }
    }

    pub fn as_bool(&self) -> bool {
        match self {
            PropValue::Bool(b) => *b,
            PropValue::Text(s) => s == "true",
            _ => false,
        }
    }

    pub fn as_number(&self) -> Option<f64> {
        match self {
            PropValue::Number(n) => Some(*n),
            PropValue::Text(s) => s.trim().parse().ok(),
            _ => None,
        }
    }

    pub fn is_empty(&self) -> bool {
        match self {
            PropValue::Bool(b) => !*b,
            PropValue::Number(_) => false,
            PropValue::Text(s) => s.is_empty(),
            PropValue::List(l) => l.is_empty(),
        }
    }
}

pub fn format_number(n: f64) -> String {
    if n.fract() == 0.0 && n.abs() < 1e15 {
        format!("{}", n as i64)
    } else {
        format!("{}", n)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PropKind {
    Title,
    Text,
    Number,
    Select,
    MultiSelect,
    Status,
    Date,
    Checkbox,
    Url,
}

impl PropKind {
    pub const ALL: [PropKind; 8] = [
        PropKind::Text,
        PropKind::Number,
        PropKind::Select,
        PropKind::MultiSelect,
        PropKind::Status,
        PropKind::Date,
        PropKind::Checkbox,
        PropKind::Url,
    ];

    pub fn label(&self) -> &'static str {
        match self {
            PropKind::Title => "Title",
            PropKind::Text => "Text",
            PropKind::Number => "Number",
            PropKind::Select => "Select",
            PropKind::MultiSelect => "Multi-select",
            PropKind::Status => "Status",
            PropKind::Date => "Date",
            PropKind::Checkbox => "Checkbox",
            PropKind::Url => "URL",
        }
    }

    pub fn has_options(&self) -> bool {
        matches!(self, PropKind::Select | PropKind::MultiSelect | PropKind::Status)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SelectOption {
    pub name: String,
    #[serde(default = "default_color")]
    pub color: String,
}

fn default_color() -> String {
    "default".into()
}

pub const OPTION_COLORS: [&str; 10] = [
    "default", "gray", "brown", "orange", "yellow", "green", "blue", "purple", "pink", "red",
];

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Property {
    pub name: String,
    #[serde(rename = "type")]
    pub kind: PropKind,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<SelectOption>,
}

impl Property {
    pub fn new(name: impl Into<String>, kind: PropKind) -> Self {
        let mut p = Property {
            name: name.into(),
            kind,
            options: vec![],
        };
        if kind == PropKind::Status {
            p.options = default_status_options();
        }
        p
    }

    pub fn option_color(&self, name: &str) -> &str {
        self.options
            .iter()
            .find(|o| o.name == name)
            .map(|o| o.color.as_str())
            .unwrap_or("default")
    }

    /// Make sure an option exists, picking the next unused colour.
    pub fn ensure_option(&mut self, name: &str) {
        if name.is_empty() || self.options.iter().any(|o| o.name == name) {
            return;
        }
        let color = OPTION_COLORS[(self.options.len() + 1) % OPTION_COLORS.len()];
        self.options.push(SelectOption {
            name: name.to_string(),
            color: color.to_string(),
        });
    }
}

pub fn default_status_options() -> Vec<SelectOption> {
    vec![
        SelectOption {
            name: "Not started".into(),
            color: "gray".into(),
        },
        SelectOption {
            name: "In progress".into(),
            color: "blue".into(),
        },
        SelectOption {
            name: "Done".into(),
            color: "green".into(),
        },
    ]
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ViewKind {
    Table,
    Board,
    List,
    Gallery,
    Calendar,
}

impl ViewKind {
    pub const ALL: [ViewKind; 5] = [
        ViewKind::Table,
        ViewKind::Board,
        ViewKind::List,
        ViewKind::Gallery,
        ViewKind::Calendar,
    ];

    pub fn label(&self) -> &'static str {
        match self {
            ViewKind::Table => "Table",
            ViewKind::Board => "Board",
            ViewKind::List => "List",
            ViewKind::Gallery => "Gallery",
            ViewKind::Calendar => "Calendar",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Sort {
    pub prop: String,
    #[serde(default)]
    pub desc: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FilterOp {
    Contains,
    NotContains,
    Is,
    IsNot,
    IsEmpty,
    IsNotEmpty,
    Gt,
    Lt,
    Checked,
    Unchecked,
}

impl FilterOp {
    pub fn label(&self) -> &'static str {
        match self {
            FilterOp::Contains => "Contains",
            FilterOp::NotContains => "Does not contain",
            FilterOp::Is => "Is",
            FilterOp::IsNot => "Is not",
            FilterOp::IsEmpty => "Is empty",
            FilterOp::IsNotEmpty => "Is not empty",
            FilterOp::Gt => "Is after / greater than",
            FilterOp::Lt => "Is before / less than",
            FilterOp::Checked => "Is checked",
            FilterOp::Unchecked => "Is not checked",
        }
    }

    pub fn needs_value(&self) -> bool {
        !matches!(
            self,
            FilterOp::IsEmpty | FilterOp::IsNotEmpty | FilterOp::Checked | FilterOp::Unchecked
        )
    }

    pub fn for_kind(kind: PropKind) -> &'static [FilterOp] {
        match kind {
            PropKind::Title | PropKind::Text | PropKind::Url => &[
                FilterOp::Contains,
                FilterOp::NotContains,
                FilterOp::Is,
                FilterOp::IsNot,
                FilterOp::IsEmpty,
                FilterOp::IsNotEmpty,
            ],
            PropKind::Number | PropKind::Date => &[
                FilterOp::Is,
                FilterOp::IsNot,
                FilterOp::Gt,
                FilterOp::Lt,
                FilterOp::IsEmpty,
                FilterOp::IsNotEmpty,
            ],
            PropKind::Select | PropKind::Status => &[
                FilterOp::Is,
                FilterOp::IsNot,
                FilterOp::IsEmpty,
                FilterOp::IsNotEmpty,
            ],
            PropKind::MultiSelect => &[
                FilterOp::Contains,
                FilterOp::NotContains,
                FilterOp::IsEmpty,
                FilterOp::IsNotEmpty,
            ],
            PropKind::Checkbox => &[FilterOp::Checked, FilterOp::Unchecked],
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Filter {
    pub prop: String,
    pub op: FilterOp,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub value: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct View {
    pub name: String,
    #[serde(rename = "type")]
    pub kind: ViewKind,
    /// Board: property to group by.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_by: Option<String>,
    /// Calendar: date property.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date_prop: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sorts: Vec<Sort>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub filters: Vec<Filter>,
    /// Properties hidden in this view.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hidden: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub widths: BTreeMap<String, f32>,
}

impl View {
    pub fn new(name: impl Into<String>, kind: ViewKind) -> Self {
        View {
            name: name.into(),
            kind,
            group_by: None,
            date_prop: None,
            sorts: vec![],
            filters: vec![],
            hidden: vec![],
            widths: BTreeMap::new(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Database {
    pub schema: Vec<Property>,
    pub views: Vec<View>,
}

impl Database {
    pub fn new_default() -> Self {
        Database {
            schema: vec![
                Property::new("Name", PropKind::Title),
                Property {
                    name: "Tags".into(),
                    kind: PropKind::MultiSelect,
                    options: vec![],
                },
            ],
            views: vec![View::new("Table", ViewKind::Table)],
        }
    }

    pub fn prop(&self, name: &str) -> Option<&Property> {
        self.schema.iter().find(|p| p.name == name)
    }

    pub fn prop_mut(&mut self, name: &str) -> Option<&mut Property> {
        self.schema.iter_mut().find(|p| p.name == name)
    }

    pub fn title_prop_name(&self) -> &str {
        self.schema
            .iter()
            .find(|p| p.kind == PropKind::Title)
            .map(|p| p.name.as_str())
            .unwrap_or("Name")
    }

    /// First property usable for board grouping.
    pub fn first_of(&self, kinds: &[PropKind]) -> Option<&Property> {
        self.schema.iter().find(|p| kinds.contains(&p.kind))
    }

    pub fn unique_prop_name(&self, base: &str) -> String {
        if self.prop(base).is_none() {
            return base.to_string();
        }
        let mut n = 1;
        loop {
            let name = format!("{} {}", base, n);
            if self.prop(&name).is_none() {
                return name;
            }
            n += 1;
        }
    }
}

#[derive(Clone, Debug)]
pub struct Page {
    pub id: String,
    pub title: String,
    pub icon: Option<String>,
    pub parent: Option<String>,
    pub kind: PageKind,
    pub created: String,
    pub updated: String,
    pub favorite: bool,
    pub trashed: bool,
    pub order: f64,
    pub props: BTreeMap<String, PropValue>,
    pub database: Option<Database>,
    pub blocks: Vec<Block>,
    /// Path relative to the vault root, once the page has been written or loaded.
    pub file: Option<std::path::PathBuf>,
}

impl Page {
    pub fn new(kind: PageKind, parent: Option<String>) -> Self {
        let now = now();
        Page {
            id: new_id(),
            title: String::new(),
            icon: None,
            parent,
            kind,
            created: now.clone(),
            updated: now,
            favorite: false,
            trashed: false,
            order: 0.0,
            props: BTreeMap::new(),
            database: if kind == PageKind::Database {
                Some(Database::new_default())
            } else {
                None
            },
            blocks: vec![Block::paragraph("")],
            file: None,
        }
    }

    pub fn display_title(&self) -> &str {
        if self.title.is_empty() {
            if self.kind == PageKind::Database {
                "Untitled"
            } else {
                "New page"
            }
        } else {
            &self.title
        }
    }

    pub fn is_database(&self) -> bool {
        self.kind == PageKind::Database
    }

    /// Plain text of the page body, for search and previews.
    pub fn plain_text(&self) -> String {
        let mut s = String::new();
        for b in &self.blocks {
            if !b.text.is_empty() {
                s.push_str(&b.text.text);
                s.push('\n');
            }
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbering_and_visibility() {
        let blocks = vec![
            Block::new(BlockKind::Numbered, "a"),
            Block::new(BlockKind::Bullet, "child").with_indent(1),
            Block::new(BlockKind::Numbered, "b"),
            Block::new(BlockKind::Toggle { open: false }, "t"),
            Block::paragraph("hidden").with_indent(1),
            Block::new(BlockKind::Numbered, "c"),
        ];
        assert_eq!(list_number(&blocks, 0), 1);
        assert_eq!(list_number(&blocks, 2), 2);
        assert_eq!(list_number(&blocks, 5), 1);
        assert_eq!(
            visibility(&blocks),
            vec![true, true, true, true, false, true]
        );
        assert_eq!(subtree_end(&blocks, 0), 2);
        assert_eq!(subtree_end(&blocks, 3), 5);
    }

    fn shape(blocks: &[Block]) -> Vec<(String, u8)> {
        blocks.iter().map(|b| (b.text.text.clone(), b.indent)).collect()
    }

    fn sample() -> Vec<Block> {
        vec![
            Block::paragraph("a"),
            Block::new(BlockKind::Bullet, "b"),
            Block::new(BlockKind::Bullet, "b1").with_indent(1),
            Block::new(BlockKind::Bullet, "b2").with_indent(1),
            Block::paragraph("c"),
        ]
    }

    #[test]
    fn move_down_and_up() {
        let mut v = sample();
        // move "a" to the very end
        let r = move_blocks(&mut v, 0..1, 5, 0).unwrap();
        assert_eq!(r, 4..5);
        assert_eq!(shape(&v)[4], ("a".into(), 0));
        // move "c" (now at 3) to the top
        let r = move_blocks(&mut v, 3..4, 0, 0).unwrap();
        assert_eq!(r, 0..1);
        assert_eq!(v[0].text.text, "c");
    }

    #[test]
    fn move_subtree_keeps_children() {
        let mut v = sample();
        let range = expand_to_subtrees(&v, 1..2);
        assert_eq!(range, 1..4);
        move_blocks(&mut v, range, 0, 0).unwrap();
        assert_eq!(
            shape(&v),
            vec![("b".into(), 0), ("b1".into(), 1), ("b2".into(), 1), ("a".into(), 0), ("c".into(), 0)]
        );
    }

    #[test]
    fn nest_by_indent_and_noops() {
        let mut v = sample();
        // same place, same indent: nothing happens
        assert!(move_blocks(&mut v, 4..5, 4, 0).is_none());
        assert!(move_blocks(&mut v, 4..5, 5, 0).is_none());
        // into itself
        assert!(move_blocks(&mut v, 1..4, 2, 0).is_none());
        // same place, deeper: becomes a child of b2's parent level
        let r = move_blocks(&mut v, 4..5, 4, 1).unwrap();
        assert_eq!(r, 4..5);
        assert_eq!(v[4].indent, 1);
    }

    #[test]
    fn drop_indent_limits() {
        let v = sample();
        // between b and b1: must become b's first child
        assert_eq!(drop_indent_range(&v, 4..5, 2), (1, 1));
        // after b2, before c: anywhere from top level to under b2
        assert_eq!(drop_indent_range(&v, 0..1, 4), (0, 2));
        // top of the page
        assert_eq!(drop_indent_range(&v, 4..5, 0), (0, 0));
        // a collapsed toggle does not take children
        let t = vec![Block::new(BlockKind::Toggle { open: false }, "t"), Block::paragraph("x")];
        assert_eq!(drop_indent_range(&t, 1..2, 1), (0, 0));
    }

    #[test]
    fn move_selection_with_mixed_depths() {
        let mut v = sample();
        // b1, b2 (indent 1) moved to the top become top-level
        move_blocks(&mut v, 2..4, 0, 0).unwrap();
        assert_eq!(shape(&v)[..2], [("b1".into(), 0), ("b2".into(), 0)]);
    }
}
