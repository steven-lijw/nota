//! The block editor: state, editing commands and IME integration.
//! Rendering lives in `editor_render.rs`.

use crate::actions::*;
use crate::edit::*;
use crate::store::Store;
use gpui::AppContext;
use crate::text_view::TextLayout;
use gpui::{
    point, px, App, Bounds, ClipboardItem, Context, Entity, EntityInputHandler, EventEmitter,
    FocusHandle, Focusable, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels,
    Point, ScrollHandle, Subscription, Task, UTF16Selection, Window,
};
use nota_core::markdown::{from_markdown, to_markdown};
use nota_core::{
    drop_indent_range, expand_to_subtrees, move_blocks, normalize_indents, subtree_end, visibility,
    Block, BlockKind, MarkKind, Marks, PageKind, RichText,
};
use std::time::Duration;
use std::cell::RefCell;
use std::collections::HashMap;
use std::ops::Range;
use std::rc::Rc;
use std::time::Instant;

pub enum EditorEvent {
    Changed,
    OpenPage(String),
    /// Arrow-up from the very first block.
    FocusTitle,
    /// "Ask AI" on the selection toolbar.
    AskAi,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Sel {
    pub block: usize,
    pub range: Range<usize>,
    pub reversed: bool,
}

impl Sel {
    pub fn caret(block: usize, offset: usize) -> Self {
        Sel {
            block,
            range: offset..offset,
            reversed: false,
        }
    }

    pub fn cursor(&self) -> usize {
        if self.reversed {
            self.range.start
        } else {
            self.range.end
        }
    }
}

struct Snapshot {
    blocks: Vec<Block>,
    sel: Sel,
}

#[derive(Clone, Copy, PartialEq)]
enum EditKind {
    Typing,
    Other,
}

#[derive(Clone)]
pub enum SlashAction {
    Turn(BlockKind),
    Page,
    Database,
    Image,
}

pub struct SlashItem {
    pub label: &'static str,
    pub hint: &'static str,
    pub icon: &'static str,
    pub keywords: &'static str,
    pub action: SlashAction,
}

pub fn slash_items() -> Vec<SlashItem> {
    use BlockKind::*;
    let item = |label, hint, icon, keywords, action| SlashItem {
        label,
        hint,
        icon,
        keywords,
        action,
    };
    vec![
        item("Text", "", "text", "text paragraph plain 文本 正文 wenben", SlashAction::Turn(Paragraph)),
        item("Heading 1", "#", "H1", "h1 heading title 标题 biaoti", SlashAction::Turn(Heading(1))),
        item("Heading 2", "##", "H2", "h2 heading subtitle 标题 biaoti", SlashAction::Turn(Heading(2))),
        item("Heading 3", "###", "H3", "h3 heading 标题 biaoti", SlashAction::Turn(Heading(3))),
        item("Bulleted list", "-", "list", "bullet list unordered 列表 无序 liebiao", SlashAction::Turn(Bullet)),
        item("Numbered list", "1.", "numbered", "numbered list ordered 列表 有序 liebiao", SlashAction::Turn(Numbered)),
        item("To-do list", "[]", "checkbox", "todo task checkbox 待办 任务 daiban", SlashAction::Turn(Todo { checked: false })),
        item("Toggle list", ">", "triangle-right", "toggle collapse 折叠 zhedie", SlashAction::Turn(Toggle { open: true })),
        item("Page", "", "page", "page subpage 页面 子页面 yemian", SlashAction::Page),
        item("Callout", "", "callout", "callout note tip 标注 提示 biaozhu", SlashAction::Turn(Callout { icon: "💡".into() })),
        item("Quote", "\"", "quote", "quote blockquote 引用 yinyong", SlashAction::Turn(Quote)),
        item("Divider", "---", "divider", "divider line hr 分割线 fengexian", SlashAction::Turn(Divider)),
        item("Code", "```", "code", "code snippet 代码 daima", SlashAction::Turn(Code { lang: String::new() })),
        item("Image", "", "image", "image picture photo screenshot 图片 截图 tupian", SlashAction::Image),
        item("Database", "", "database", "database table board 数据库 表格 看板 shujuku", SlashAction::Database),
    ]
}

pub struct SlashState {
    pub block: usize,
    /// Byte offset of the `/`.
    pub start: usize,
    pub selected: usize,
}

pub struct BlockMenu {
    pub block: usize,
    pub pos: Point<Pixels>,
}

#[derive(Clone)]
pub struct DragBlock {
    /// Runtime id of the block whose handle was grabbed.
    pub id: u64,
    pub label: String,
    /// How many blocks move (the block selection, if the grabbed block is in it).
    pub count: usize,
}

/// Where dragged blocks would land: before `gap` (an index into `blocks`,
/// `blocks.len()` for the end) with the first block at `indent`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DropTarget {
    pub gap: usize,
    pub indent: u8,
}

/// Rubber-band selection started in empty space.
pub struct Marquee {
    /// Start point in content coordinates (window point minus scroll offset).
    pub start: Point<Pixels>,
    /// Current pointer position in window coordinates.
    pub current: Point<Pixels>,
    /// Becomes true once the pointer moved far enough to count as a drag.
    pub active: bool,
    /// Started below the last block: a plain click there adds a line.
    pub from_tail: bool,
}

/// The part of the page a question to Nota AI was about.
#[derive(Clone, Debug, PartialEq)]
pub enum AiTarget {
    /// Selected text inside one block.
    Text { block: u64, range: Range<usize> },
    /// Whole selected blocks (first and last, by runtime id).
    Blocks { first: u64, last: u64 },
}

/// What the AI panel shows and sends for the current selection.
#[derive(Clone, Debug, PartialEq)]
pub struct AiSelection {
    pub markdown: String,
    pub preview: String,
    pub target: AiTarget,
}

/// Dragging one of an image's side handles.
#[derive(Clone)]
pub struct ResizeImage {
    pub id: u64,
    /// -1 for the left handle, 1 for the right one.
    pub side: f32,
}

/// Image file types that can be dropped or pasted into a page.
pub const IMAGE_EXTS: [&str; 9] = ["png", "jpg", "jpeg", "gif", "webp", "bmp", "svg", "tif", "tiff"];

pub fn is_image_path(p: &std::path::Path) -> bool {
    p.extension().is_some_and(|e| IMAGE_EXTS.contains(&e.to_string_lossy().to_lowercase().as_str()))
}

/// Languages offered for code blocks (stored as written here, lower-case).
pub const CODE_LANGS: [&str; 22] = [
    "", "bash", "c", "cpp", "csharp", "css", "go", "html", "java", "javascript", "json", "kotlin",
    "markdown", "php", "python", "ruby", "rust", "sql", "swift", "typescript", "yaml", "toml",
];

pub fn lang_label(lang: &str) -> &str {
    match lang {
        "" => "Plain text",
        "bash" | "sh" | "shell" | "zsh" => "Shell",
        "c" => "C",
        "cpp" | "c++" => "C++",
        "csharp" | "cs" => "C#",
        "css" => "CSS",
        "go" => "Go",
        "html" => "HTML",
        "java" => "Java",
        "javascript" | "js" => "JavaScript",
        "json" => "JSON",
        "kotlin" => "Kotlin",
        "markdown" | "md" => "Markdown",
        "php" => "PHP",
        "python" | "py" => "Python",
        "ruby" | "rb" => "Ruby",
        "rust" | "rs" => "Rust",
        "sql" => "SQL",
        "swift" => "Swift",
        "typescript" | "ts" => "TypeScript",
        "yaml" | "yml" => "YAML",
        "toml" => "TOML",
        other => other,
    }
}

/// The ⌘K link editor.
pub struct LinkEdit {
    pub block: usize,
    pub range: Range<usize>,
    pub input: Entity<crate::text_input::TextInput>,
    pub had_link: bool,
    _sub: Subscription,
}

/// Small pickers attached to one block.
#[derive(Clone, Copy, PartialEq)]
pub enum BlockPicker {
    Language,
    CalloutIcon,
}

pub const INDENT_PX: f32 = 24.0;
const GUTTER_PX: f32 = 50.0;

pub struct Editor {
    pub(crate) store: Entity<Store>,
    pub(crate) page_id: String,
    pub blocks: Vec<Block>,
    pub(crate) focus: FocusHandle,
    pub(crate) sel: Sel,
    /// Whole-block selection: (anchor, head), inclusive block indices.
    pub(crate) block_sel: Option<(usize, usize)>,
    pub(crate) marked: Option<Range<usize>>,
    pub(crate) layouts: Rc<RefCell<HashMap<u64, TextLayout>>>,
    pending_marks: Option<Marks>,
    pub(crate) slash: Option<SlashState>,
    pub(crate) block_menu: Option<BlockMenu>,
    pub(crate) selecting: bool,
    drag_anchor: Option<(usize, usize)>,
    goal_x: Option<Pixels>,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    last_edit: Option<(Instant, EditKind, usize)>,
    pub(crate) scroll: ScrollHandle,
    pub(crate) scroll_pending: bool,
    /// Window bounds of every rendered block row, by block id.
    pub(crate) row_bounds: Rc<RefCell<HashMap<u64, Bounds<Pixels>>>>,
    pub(crate) drop_target: Option<DropTarget>,
    /// Blocks being dragged (dimmed while the drag lasts).
    pub(crate) drag_range: Option<Range<usize>>,
    pub(crate) marquee: Option<Marquee>,
    pub(crate) link_edit: Option<LinkEdit>,
    /// (block, picker, where it opens)
    pub(crate) picker: Option<(usize, BlockPicker, Point<Pixels>)>,
    last_mouse: Option<Point<Pixels>>,
    /// Image natural sizes in pixels, by file.
    pub(crate) image_dims: RefCell<HashMap<std::path::PathBuf, Option<(u32, u32)>>>,
    /// (image block id, pointer x and width when the resize started)
    resize_start: Option<(u64, Pixels, f32)>,
    autoscrolling: bool,
    _autoscroll: Option<Task<()>>,
    _subs: Vec<Subscription>,
}

impl EventEmitter<EditorEvent> for Editor {}

impl Focusable for Editor {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Editor {
    pub fn new(
        store: Entity<Store>,
        page_id: String,
        scroll: ScrollHandle,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut blocks = store
            .read(cx)
            .page(&page_id)
            .map(|p| p.blocks.clone())
            .unwrap_or_default();
        if blocks.is_empty() {
            blocks.push(Block::paragraph(""));
        }
        let focus = cx.focus_handle();
        let subs = vec![
            cx.on_blur(&focus, window, |this: &mut Self, _, cx| {
                this.marked = None;
                this.selecting = false;
                cx.notify();
            }),
            cx.observe(&store, |_, _, cx| cx.notify()),
        ];
        let mut editor = Editor {
            store,
            page_id,
            blocks,
            focus,
            sel: Sel::caret(0, 0),
            block_sel: None,
            marked: None,
            layouts: Rc::new(RefCell::new(HashMap::new())),
            pending_marks: None,
            slash: None,
            block_menu: None,
            selecting: false,
            drag_anchor: None,
            goal_x: None,
            undo: Vec::new(),
            redo: Vec::new(),
            last_edit: None,
            scroll,
            scroll_pending: false,
            row_bounds: Rc::new(RefCell::new(HashMap::new())),
            drop_target: None,
            drag_range: None,
            marquee: None,
            link_edit: None,
            picker: None,
            last_mouse: None,
            image_dims: RefCell::new(HashMap::new()),
            resize_start: None,
            autoscrolling: false,
            _autoscroll: None,
            _subs: subs,
        };
        editor.fix_selection();
        editor
    }

    /// Replace the content (after the vault was reloaded from disk).
    pub fn reload_blocks(&mut self, cx: &mut Context<Self>) {
        let blocks = self
            .store
            .read(cx)
            .page(&self.page_id)
            .map(|p| p.blocks.clone())
            .unwrap_or_default();
        self.blocks = if blocks.is_empty() {
            vec![Block::paragraph("")]
        } else {
            blocks
        };
        self.undo.clear();
        self.redo.clear();
        self.block_sel = None;
        self.slash = None;
        self.block_menu = None;
        self.marked = None;
        self.fix_selection();
        cx.notify();
    }

    pub fn focus_start(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus);
        if let Some(ix) = self.first_text_block() {
            self.set_caret(ix, 0, cx);
        }
    }

    pub fn focus_end(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus);
        if let Some(ix) = self.last_text_block() {
            let len = self.blocks[ix].text.len();
            self.set_caret(ix, len, cx);
        }
    }

    pub fn is_focused(&self, window: &Window) -> bool {
        self.focus.is_focused(window)
    }

    /// Append blocks at the end (used by "insert into page" from the AI panel).
    pub fn append_blocks(&mut self, mut new_blocks: Vec<Block>, cx: &mut Context<Self>) {
        if new_blocks.is_empty() {
            return;
        }
        self.checkpoint(EditKind::Other);
        if self
            .blocks
            .last()
            .is_some_and(|b| b.kind == BlockKind::Paragraph && b.text.is_empty())
        {
            self.blocks.pop();
        }
        let at = self.blocks.len();
        self.blocks.append(&mut new_blocks);
        self.sel = Sel::caret(at.min(self.blocks.len() - 1), 0);
        self.fix_selection();
        self.scroll_pending = true;
        self.after_edit(cx);
    }

    // ----------------------------------------------------------------------
    // helpers
    // ----------------------------------------------------------------------

    pub(crate) fn cur(&self) -> &Block {
        &self.blocks[self.sel.block]
    }

    fn text(&self) -> &str {
        &self.blocks[self.sel.block].text.text
    }

    pub(crate) fn cursor(&self) -> usize {
        self.sel.cursor()
    }

    fn fix_selection(&mut self) {
        if self.blocks.is_empty() {
            self.blocks.push(Block::paragraph(""));
        }
        if self.sel.block >= self.blocks.len() {
            self.sel = Sel::caret(self.blocks.len() - 1, 0);
        }
        let len = self.blocks[self.sel.block].text.len();
        let text = &self.blocks[self.sel.block].text.text;
        self.sel.range = clamp(text, self.sel.range.start.min(len))..clamp(text, self.sel.range.end.min(len));
        if let Some((a, h)) = self.block_sel {
            let max = self.blocks.len() - 1;
            self.block_sel = Some((a.min(max), h.min(max)));
        }
    }

    pub(crate) fn block_sel_range(&self) -> Option<Range<usize>> {
        self.block_sel.map(|(a, h)| a.min(h)..a.max(h) + 1)
    }

    fn first_text_block(&self) -> Option<usize> {
        let vis = visibility(&self.blocks);
        (0..self.blocks.len()).find(|&i| vis[i] && self.blocks[i].kind.has_text())
    }

    fn last_text_block(&self) -> Option<usize> {
        let vis = visibility(&self.blocks);
        (0..self.blocks.len())
            .rev()
            .find(|&i| vis[i] && self.blocks[i].kind.has_text())
    }

    fn prev_visible(&self, ix: usize) -> Option<usize> {
        let vis = visibility(&self.blocks);
        (0..ix).rev().find(|&i| vis[i])
    }

    fn next_visible(&self, ix: usize) -> Option<usize> {
        let vis = visibility(&self.blocks);
        (ix + 1..self.blocks.len()).find(|&i| vis[i])
    }

    fn prev_text_block(&self, ix: usize) -> Option<usize> {
        let vis = visibility(&self.blocks);
        (0..ix)
            .rev()
            .find(|&i| vis[i] && self.blocks[i].kind.has_text())
    }

    fn next_text_block(&self, ix: usize) -> Option<usize> {
        let vis = visibility(&self.blocks);
        (ix + 1..self.blocks.len()).find(|&i| vis[i] && self.blocks[i].kind.has_text())
    }

    pub(crate) fn layout_of(&self, ix: usize) -> Option<TextLayout> {
        let id = self.blocks.get(ix)?.id;
        self.layouts.borrow().get(&id).cloned()
    }

    pub(crate) fn set_caret(&mut self, block: usize, offset: usize, cx: &mut Context<Self>) {
        let block = block.min(self.blocks.len() - 1);
        let offset = clamp(&self.blocks[block].text.text, offset);
        if self.sel.block != block || self.sel.cursor() != offset {
            self.pending_marks = None;
        }
        if self.sel.block != block {
            self.marked = None;
        }
        self.sel = Sel::caret(block, offset);
        self.block_sel = None;
        self.scroll_pending = true;
        cx.notify();
    }

    fn select_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        let offset = clamp(self.text(), offset);
        if self.sel.reversed {
            self.sel.range.start = offset;
        } else {
            self.sel.range.end = offset;
        }
        if self.sel.range.end < self.sel.range.start {
            self.sel.reversed = !self.sel.reversed;
            self.sel.range = self.sel.range.end..self.sel.range.start;
        }
        self.scroll_pending = true;
        cx.notify();
    }

    fn checkpoint(&mut self, kind: EditKind) {
        let now = Instant::now();
        let coalesce = kind == EditKind::Typing
            && self.last_edit.is_some_and(|(at, k, block)| {
                k == EditKind::Typing
                    && block == self.sel.block
                    && now.duration_since(at).as_millis() < 1200
            });
        if !coalesce {
            self.undo.push(Snapshot {
                blocks: self.blocks.clone(),
                sel: self.sel.clone(),
            });
            if self.undo.len() > 200 {
                self.undo.remove(0);
            }
        }
        self.redo.clear();
        self.last_edit = Some((now, kind, self.sel.block));
    }

    fn after_edit(&mut self, cx: &mut Context<Self>) {
        normalize_indents(&mut self.blocks);
        self.fix_selection();
        self.scroll_pending = true;
        cx.emit(EditorEvent::Changed);
        cx.notify();
    }

    fn close_popups(&mut self) {
        self.slash = None;
        self.block_menu = None;
        self.link_edit = None;
        self.picker = None;
    }

    // ----------------------------------------------------------------------
    // links and block pickers
    // ----------------------------------------------------------------------

    /// The link around `offset` in block `ix`: its range and URL.
    pub(crate) fn link_at(&self, ix: usize, offset: usize) -> Option<(Range<usize>, String)> {
        let text = &self.blocks.get(ix)?.text;
        // merge neighbouring runs that carry the same link
        let mut links: Vec<(Range<usize>, String)> = Vec::new();
        for (r, m) in text.segments() {
            let Some(url) = &m.link else { continue };
            match links.last_mut() {
                Some((lr, lu)) if lr.end == r.start && lu == url => lr.end = r.end,
                _ => links.push((r.clone(), url.clone())),
            }
        }
        links.into_iter().find(|(r, _)| !r.is_empty() && r.start <= offset && offset <= r.end)
    }

    pub(crate) fn insert_link(&mut self, _: &InsertLink, window: &mut Window, cx: &mut Context<Self>) {
        if self.block_sel.is_some() || !self.cur().kind.has_text() || self.cur().kind.is_raw() {
            cx.propagate();
            return;
        }
        let ix = self.sel.block;
        let (range, url) = if self.sel.range.is_empty() {
            match self.link_at(ix, self.cursor()) {
                Some(found) => found,
                None => {
                    cx.propagate(); // nothing to link: ⌘K searches
                    return;
                }
            }
        } else {
            let r = self.sel.range.clone();
            let url = self.link_at(ix, r.start).filter(|(lr, _)| lr.start <= r.start && r.end <= lr.end).map(|l| l.1);
            (r, url.unwrap_or_default())
        };
        self.open_link_editor(ix, range, url, window, cx);
    }

    pub(crate) fn open_link_editor(&mut self, ix: usize, range: Range<usize>, url: String, window: &mut Window, cx: &mut Context<Self>) {
        use crate::text_input::{InputEvent, TextInput};
        let had_link = !url.is_empty();
        let input = cx.new(|cx| {
            let mut i = TextInput::new(window, cx);
            i.placeholder = "Paste or type a link…".into();
            i.set_text(url, cx);
            i.select_all_text(cx);
            i
        });
        input.read(cx).focus(window);
        let sub = cx.subscribe_in(&input, window, |this: &mut Self, _, ev: &InputEvent, window, cx| match ev {
            InputEvent::Submit => this.apply_link(window, cx),
            InputEvent::Cancel => {
                this.link_edit = None;
                window.focus(&this.focus);
                cx.notify();
            }
            _ => {}
        });
        self.sel = Sel { block: ix, range: range.clone(), reversed: false };
        self.link_edit = Some(LinkEdit { block: ix, range, input, had_link, _sub: sub });
        cx.notify();
    }

    /// Apply the URL typed in the link editor (empty removes the link).
    pub(crate) fn apply_link(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(edit) = self.link_edit.take() else { return };
        let url = edit.input.read(cx).text().trim().to_string();
        if edit.block < self.blocks.len() {
            let len = self.blocks[edit.block].text.len();
            let r = edit.range.start.min(len)..edit.range.end.min(len);
            self.checkpoint(EditKind::Other);
            let url = (!url.is_empty()).then(|| {
                if url.contains("://") || url.starts_with("mailto:") { url } else { format!("https://{url}") }
            });
            self.blocks[edit.block].text.set_link(r.clone(), url);
            self.sel = Sel::caret(edit.block, r.end);
            self.after_edit(cx);
        }
        window.focus(&self.focus);
    }

    pub(crate) fn remove_link(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(edit) = &self.link_edit {
            edit.input.update(cx, |i, cx| i.set_text("", cx));
        }
        self.apply_link(window, cx);
    }

    pub(crate) fn set_code_lang(&mut self, ix: usize, lang: &str, cx: &mut Context<Self>) {
        self.picker = None;
        if let Some(BlockKind::Code { lang: l }) = self.blocks.get_mut(ix).map(|b| &mut b.kind) {
            if l != lang {
                *l = lang.to_string();
                self.checkpoint(EditKind::Other);
                self.after_edit(cx);
                return;
            }
        }
        cx.notify();
    }

    pub(crate) fn set_callout_icon(&mut self, ix: usize, emoji: &str, cx: &mut Context<Self>) {
        self.picker = None;
        if let Some(BlockKind::Callout { icon }) = self.blocks.get_mut(ix).map(|b| &mut b.kind) {
            *icon = emoji.to_string();
            self.checkpoint(EditKind::Other);
            self.after_edit(cx);
        } else {
            cx.notify();
        }
    }

    // ----------------------------------------------------------------------
    // text insertion
    // ----------------------------------------------------------------------

    fn insert_text(&mut self, range: Range<usize>, new_text: &str, cx: &mut Context<Self>) {
        if !self.cur().kind.has_text() {
            return;
        }
        self.checkpoint(EditKind::Typing);
        let ix = self.sel.block;
        let is_code = self.blocks[ix].kind.is_raw();
        let marks = if is_code {
            Some(Marks::default())
        } else if range.is_empty() {
            self.pending_marks.clone()
        } else {
            None
        };
        self.blocks[ix].text.replace(range.clone(), new_text, marks);
        let at = range.start + new_text.len();
        self.sel = Sel::caret(ix, at);
        self.marked = None;
        self.goal_x = None;

        if !is_code {
            self.apply_markdown_shortcut(new_text);
            self.apply_inline_shortcut(new_text);
        }
        self.update_slash_after_insert(new_text);
        self.after_edit(cx);
    }

    /// Notion-style shortcuts: `# `, `- `, `1. `, `[] `, `> `, `" `, ``` and `---`.
    fn apply_markdown_shortcut(&mut self, typed: &str) {
        let ix = self.sel.block;
        if self.blocks[ix].kind != BlockKind::Paragraph {
            return;
        }
        let caret = self.sel.cursor();
        let prefix = self.blocks[ix].text.text[..caret].to_string();
        let whole = self.blocks[ix].text.text.as_str();

        if typed.ends_with('`') && whole == "```" {
            self.blocks[ix].kind = BlockKind::Code { lang: String::new() };
            self.blocks[ix].text = RichText::plain("");
            self.sel = Sel::caret(ix, 0);
            self.last_edit = None;
            return;
        }
        if typed.ends_with('-') && whole == "---" {
            self.blocks[ix].kind = BlockKind::Divider;
            self.blocks[ix].text = RichText::default();
            let indent = self.blocks[ix].indent;
            self.blocks
                .insert(ix + 1, Block::paragraph("").with_indent(indent));
            self.sel = Sel::caret(ix + 1, 0);
            self.last_edit = None;
            return;
        }
        if typed != " " {
            return;
        }
        let kind = match prefix.as_str() {
            "# " => Some(BlockKind::Heading(1)),
            "## " => Some(BlockKind::Heading(2)),
            "### " => Some(BlockKind::Heading(3)),
            "- " | "* " | "+ " => Some(BlockKind::Bullet),
            "[] " | "[ ] " => Some(BlockKind::Todo { checked: false }),
            "[x] " => Some(BlockKind::Todo { checked: true }),
            "> " => Some(BlockKind::Toggle { open: true }),
            "\" " | "“ " | "| " => Some(BlockKind::Quote),
            p => {
                let digits = p.chars().take_while(|c| c.is_ascii_digit()).count();
                let rest = &p[digits..];
                if digits > 0 && digits <= 3 && (rest == ". " || rest == ") ") {
                    Some(BlockKind::Numbered)
                } else {
                    None
                }
            }
        };
        if let Some(kind) = kind {
            self.blocks[ix].kind = kind;
            self.blocks[ix].text.replace(0..caret, "", None);
            self.sel = Sel::caret(ix, 0);
            self.last_edit = None;
        }
    }

    /// Inline Markdown while typing: `**bold**`, `*italic*`, `~~strike~~`, `` `code` ``.
    fn apply_inline_shortcut(&mut self, typed: &str) {
        let ix = self.sel.block;
        if !self.blocks[ix].kind.has_text() || !self.sel.range.is_empty() {
            return;
        }
        let caret = self.sel.cursor();
        let rules = [
            ("**", MarkKind::Bold),
            ("~~", MarkKind::Strike),
            ("`", MarkKind::Code),
            ("*", MarkKind::Italic),
            ("_", MarkKind::Italic),
        ];
        for (delim, kind) in rules {
            let last = delim.chars().last().unwrap_or('*');
            if !typed.ends_with(last) {
                continue;
            }
            let text = self.blocks[ix].text.text[..caret].to_string();
            if !text.ends_with(delim) || caret < delim.len() * 2 + 1 {
                continue;
            }
            let inner_end = caret - delim.len();
            let before = &text[..inner_end];
            if delim.len() == 1 && before.ends_with(delim) {
                continue; // "**" being typed, not "*x*"
            }
            let Some(open) = before.rfind(delim) else { continue };
            let inner_start = open + delim.len();
            if inner_start >= inner_end {
                continue;
            }
            let inner = &text[inner_start..inner_end];
            if inner.starts_with(char::is_whitespace) || inner.ends_with(char::is_whitespace) {
                continue;
            }
            if delim.len() == 1 && kind != MarkKind::Code && open > 0 && text[..open].ends_with(delim) {
                continue; // part of a "**" pair
            }
            if delim == "_" && open > 0 && text[..open].chars().next_back().is_some_and(|c| c.is_alphanumeric()) {
                continue; // snake_case words
            }
            let rt = &mut self.blocks[ix].text;
            rt.replace(inner_end..caret, "", None);
            rt.replace(open..inner_start, "", None);
            let len = inner_end - inner_start;
            rt.set_mark(open..open + len, kind, true);
            self.sel = Sel::caret(ix, open + len);
            self.pending_marks = Some(Marks::default());
            self.last_edit = None;
            return;
        }
    }

    fn update_slash_after_insert(&mut self, typed: &str) {
        let ix = self.sel.block;
        let caret = self.sel.cursor();
        if let Some(slash) = &mut self.slash {
            let text = &self.blocks[ix].text.text;
            let valid = slash.block == ix
                && caret > slash.start
                && text.get(slash.start..slash.start + 1) == Some("/");
            if !valid {
                self.slash = None;
            } else {
                let query = text[slash.start + 1..caret].to_string();
                slash.selected = 0;
                if query.contains('\n')
                    || query.starts_with(' ')
                    || (query.chars().count() > 14 && self.filtered_slash_items().is_empty())
                {
                    self.slash = None;
                }
            }
            return;
        }
        if (typed == "/" || typed == "、" || typed == "／") && caret >= typed.len() {
            let start = caret - typed.len();
            let text = &self.blocks[ix].text.text;
            let before_ok = start == 0
                || text[..start]
                    .chars()
                    .next_back()
                    .is_some_and(|c| c.is_whitespace());
            if before_ok && typed == "/" {
                self.slash = Some(SlashState {
                    block: ix,
                    start,
                    selected: 0,
                });
            }
        }
    }

    pub(crate) fn slash_query(&self) -> String {
        let Some(slash) = &self.slash else {
            return String::new();
        };
        let text = &self.blocks[slash.block.min(self.blocks.len() - 1)].text.text;
        let caret = self.sel.cursor().min(text.len());
        text.get(slash.start + 1..caret).unwrap_or("").to_lowercase()
    }

    pub(crate) fn filtered_slash_items(&self) -> Vec<SlashItem> {
        let q = self.slash_query();
        slash_items()
            .into_iter()
            .filter(|it| {
                q.is_empty()
                    || it.label.to_lowercase().contains(&q)
                    || it.keywords.split(' ').any(|k| k.starts_with(&q) || k.contains(&q))
            })
            .collect()
    }

    pub(crate) fn apply_slash(&mut self, item_ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let items = self.filtered_slash_items();
        let Some(slash) = self.slash.take() else {
            return;
        };
        let Some(item) = items.get(item_ix) else {
            return;
        };
        let ix = slash.block.min(self.blocks.len() - 1);
        self.checkpoint(EditKind::Other);
        let caret = self.sel.cursor().min(self.blocks[ix].text.len());
        if slash.start < caret {
            self.blocks[ix].text.replace(slash.start..caret, "", None);
        }
        self.sel = Sel::caret(ix, slash.start.min(self.blocks[ix].text.len()));
        let empty = self.blocks[ix].text.is_empty();
        let indent = self.blocks[ix].indent;

        match item.action.clone() {
            SlashAction::Turn(kind) => {
                if kind == BlockKind::Divider {
                    let at = if empty {
                        self.blocks[ix].kind = BlockKind::Divider;
                        self.blocks[ix].text = RichText::default();
                        ix
                    } else {
                        self.blocks
                            .insert(ix + 1, Block::new(BlockKind::Divider, RichText::default()).with_indent(indent));
                        ix + 1
                    };
                    self.blocks
                        .insert(at + 1, Block::paragraph("").with_indent(indent));
                    self.sel = Sel::caret(at + 1, 0);
                } else if empty {
                    self.blocks[ix].kind = kind;
                } else {
                    let end = subtree_end(&self.blocks, ix);
                    self.blocks
                        .insert(end, Block::new(kind, "").with_indent(indent));
                    self.sel = Sel::caret(end, 0);
                }
                self.after_edit(cx);
            }
            SlashAction::Image => {
                self.after_edit(cx);
                let paths = cx.prompt_for_paths(gpui::PathPromptOptions {
                    files: true,
                    directories: false,
                    multiple: true,
                    prompt: Some("Insert".into()),
                });
                cx.spawn_in(window, async move |this, cx| {
                    let Ok(Ok(Some(paths))) = paths.await else { return };
                    this.update_in(cx, |this, window, cx| {
                        this.drop_target = None;
                        this.drop_files(&paths, window, cx)
                    })
                    .ok();
                })
                .detach();
            }
            SlashAction::Page | SlashAction::Database => {
                let kind = if matches!(item.action, SlashAction::Database) {
                    PageKind::Database
                } else {
                    PageKind::Page
                };
                let parent = self.page_id.clone();
                let new_id = self
                    .store
                    .update(cx, |store, cx| store.create_page(kind, Some(parent), cx));
                let link = Block::new(BlockKind::PageLink { page: new_id.clone() }, "").with_indent(indent);
                if empty {
                    self.blocks[ix] = link;
                    self.blocks
                        .insert(ix + 1, Block::paragraph("").with_indent(indent));
                    self.sel = Sel::caret(ix + 1, 0);
                } else {
                    let end = subtree_end(&self.blocks, ix);
                    self.blocks.insert(end, link);
                }
                self.after_edit(cx);
                cx.emit(EditorEvent::OpenPage(new_id));
            }
        }
        let _ = window;
    }

    // ----------------------------------------------------------------------
    // block-level operations
    // ----------------------------------------------------------------------

    fn delete_block_selection(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(range) = self.block_sel_range() else {
            return false;
        };
        // children go with their parent
        let range = expand_to_subtrees(&self.blocks, range);
        self.checkpoint(EditKind::Other);
        let indent = self.blocks[range.start].indent;
        self.blocks.drain(range.clone());
        self.block_sel = None;
        let at = range.start.min(self.blocks.len());
        // Land the caret on the previous text block's end, or on a fresh paragraph.
        if self.blocks.is_empty() {
            self.blocks.push(Block::paragraph(""));
            self.sel = Sel::caret(0, 0);
        } else if at < self.blocks.len() && self.blocks[at].kind.has_text() {
            self.sel = Sel::caret(at, 0);
        } else if at > 0 && self.blocks[at - 1].kind.has_text() {
            self.sel = Sel::caret(at - 1, self.blocks[at - 1].text.len());
        } else {
            self.blocks
                .insert(at, Block::paragraph("").with_indent(indent));
            self.sel = Sel::caret(at, 0);
        }
        self.after_edit(cx);
        true
    }

    pub(crate) fn turn_into(&mut self, kind: BlockKind, cx: &mut Context<Self>) {
        self.checkpoint(EditKind::Other);
        let targets: Vec<usize> = match self.block_sel_range() {
            Some(r) => r.collect(),
            None => vec![self.sel.block],
        };
        for ix in targets {
            let block = &mut self.blocks[ix];
            if !block.kind.has_text() {
                continue;
            }
            if kind.is_raw() {
                block.text.clear_marks();
            }
            if kind == BlockKind::Divider {
                continue;
            }
            block.kind = kind.clone();
        }
        self.close_popups();
        self.after_edit(cx);
    }

    pub(crate) fn delete_block(&mut self, ix: usize, cx: &mut Context<Self>) {
        self.block_sel = Some((ix, ix));
        self.delete_block_selection(cx);
        self.close_popups();
    }

    pub(crate) fn duplicate_block(&mut self, ix: usize, cx: &mut Context<Self>) {
        self.checkpoint(EditKind::Other);
        let end = subtree_end(&self.blocks, ix);
        let copies: Vec<Block> = self.blocks[ix..end].iter().map(|b| b.duplicate()).collect();
        let n = copies.len();
        for (i, b) in copies.into_iter().enumerate() {
            self.blocks.insert(end + i, b);
        }
        if self.blocks[end].kind.has_text() {
            self.sel = Sel::caret(end, self.blocks[end].text.len());
        }
        let _ = n;
        self.block_sel = None;
        self.close_popups();
        self.after_edit(cx);
    }

    // ----------------------------------------------------------------------
    // dragging blocks
    // ----------------------------------------------------------------------

    /// The blocks that move when the handle of block `ix` is dragged: the
    /// whole block selection if `ix` is part of it, else `ix` and its children.
    pub(crate) fn moving_range(&self, ix: usize) -> Range<usize> {
        match self.block_sel_range() {
            Some(r) if r.contains(&ix) => expand_to_subtrees(&self.blocks, r),
            _ => ix..subtree_end(&self.blocks, ix),
        }
    }

    fn index_of(&self, id: u64) -> Option<usize> {
        self.blocks.iter().position(|b| b.id == id)
    }

    pub(crate) fn index_of_block(&self, id: u64) -> Option<usize> {
        self.index_of(id)
    }

    /// Visible blocks that have been laid out, with their row bounds, top to bottom.
    fn visible_rows(&self) -> Vec<(usize, Bounds<Pixels>)> {
        let vis = visibility(&self.blocks);
        let rows = self.row_bounds.borrow();
        self.blocks
            .iter()
            .enumerate()
            .filter(|(i, _)| vis[*i])
            .filter_map(|(i, b)| rows.get(&b.id).map(|r| (i, *r)))
            .collect()
    }

    /// Left edge of a block's content (where its text or marker starts).
    fn content_left(&self, ix: usize, row: &Bounds<Pixels>) -> Pixels {
        row.left() + px(GUTTER_PX + self.blocks[ix].indent as f32 * INDENT_PX)
    }

    /// Work out where the dragged blocks would land for a pointer position.
    fn drop_target_at(&self, pos: Point<Pixels>, moving: &Range<usize>) -> Option<DropTarget> {
        let rows = self.visible_rows();
        if rows.is_empty() {
            return None;
        }
        let gap = rows
            .iter()
            .find(|(_, b)| pos.y < b.center().y)
            .map_or(self.blocks.len(), |(i, _)| *i);
        if gap > moving.start && gap < moving.end {
            return None;
        }
        let (min, max) = drop_indent_range(&self.blocks, moving.clone(), gap);
        // The block shown just above the gap decides the default depth.
        let above = rows.iter().rev().find(|(i, _)| *i < gap && !moving.contains(i));
        let wanted = match above {
            None => 0,
            Some((p, row)) => {
                let depth = self.blocks[*p].indent as i32;
                let offset = (pos.x - self.content_left(*p, row)) / px(1.);
                if offset > 48.0 {
                    depth + 1
                } else if offset < -30.0 {
                    depth - (((-offset - 30.0) / INDENT_PX).floor() as i32 + 1)
                } else {
                    depth
                }
            }
        };
        let indent = wanted.clamp(min as i32, max as i32) as u8;
        let same_place = gap == moving.start || gap == moving.end;
        if same_place && indent == self.blocks[moving.start].indent {
            return None;
        }
        Some(DropTarget { gap, indent })
    }

    pub(crate) fn drag_moved(&mut self, drag: &DragBlock, pos: Point<Pixels>, inside: bool, cx: &mut Context<Self>) {
        self.last_mouse = Some(pos);
        let Some(ix) = self.index_of(drag.id) else { return };
        let moving = self.moving_range(ix);
        let target = if inside { self.drop_target_at(pos, &moving) } else { None };
        if self.drag_range.as_ref() != Some(&moving) || self.drop_target != target {
            self.drag_range = Some(moving);
            self.drop_target = target;
            cx.notify();
        }
        self.ensure_autoscroll(cx);
    }

    pub(crate) fn drop_dragged(&mut self, drag: &DragBlock, cx: &mut Context<Self>) {
        let target = self.drop_target.take();
        self.drag_range = None;
        let (Some(target), Some(ix)) = (target, self.index_of(drag.id)) else {
            cx.notify();
            return;
        };
        let moving = self.moving_range(ix);
        let mut blocks = self.blocks.clone();
        let Some(placed) = move_blocks(&mut blocks, moving, target.gap, target.indent) else {
            cx.notify();
            return;
        };
        self.checkpoint(EditKind::Other);
        self.blocks = blocks;
        // Like Notion: the moved blocks stay selected.
        self.block_sel = Some((placed.start, placed.end - 1));
        self.sel = Sel::caret(placed.start, 0);
        self.close_popups();
        self.after_edit(cx);
    }

    /// Forget drag state once the drag is over (dropped elsewhere or cancelled).
    pub(crate) fn clear_stale_drag(&mut self, cx: &App) {
        if !cx.has_active_drag() {
            self.drop_target = None;
            self.drag_range = None;
            self.resize_start = None;
        }
    }

    // ----------------------------------------------------------------------
    // images
    // ----------------------------------------------------------------------

    /// Natural pixel size of an image file (cached; reads only the header).
    pub(crate) fn image_size(&self, path: &std::path::Path) -> Option<(u32, u32)> {
        if let Some(d) = self.image_dims.borrow().get(path) {
            return *d;
        }
        let d = image::image_dimensions(path).ok();
        self.image_dims.borrow_mut().insert(path.to_path_buf(), d);
        d
    }

    /// Insert image blocks (vault-root-relative paths) at `target`, or after
    /// the current block. The new images end up selected.
    fn insert_images(&mut self, assets: Vec<(String, String)>, target: Option<DropTarget>, cx: &mut Context<Self>) {
        if assets.is_empty() {
            return;
        }
        let page = self.page_id.clone();
        let make = |s: &Store, indent: u8, (rel, alt): &(String, String)| {
            Block::new(BlockKind::Image { src: s.vault.src_for_page(&page, rel), width: None }, alt.as_str())
                .with_indent(indent)
        };
        self.checkpoint(EditKind::Other);
        let (mut at, indent) = match target {
            Some(t) => (t.gap.min(self.blocks.len()), t.indent),
            None => {
                let ix = self.sel.block.min(self.blocks.len() - 1);
                let b = &self.blocks[ix];
                if b.kind == BlockKind::Paragraph && b.text.is_empty() {
                    // replace the empty line the caret is on
                    let indent = b.indent;
                    self.blocks.remove(ix);
                    (ix, indent)
                } else {
                    (subtree_end(&self.blocks, ix), b.indent)
                }
            }
        };
        let store = self.store.read(cx);
        let new: Vec<Block> = assets.iter().map(|a| make(store, indent, a)).collect();
        let n = new.len();
        let start = at;
        for b in new {
            self.blocks.insert(at, b);
            at += 1;
        }
        if at == self.blocks.len() {
            // keep somewhere to type after an image at the end of the page
            self.blocks.push(Block::paragraph("").with_indent(0));
        }
        self.block_sel = Some((start, start + n - 1));
        self.sel = Sel::caret(start, 0);
        self.close_popups();
        self.after_edit(cx);
    }

    /// Files dragged in from Finder (or a screenshot thumbnail).
    pub(crate) fn drop_files(&mut self, paths: &[std::path::PathBuf], window: &mut Window, cx: &mut Context<Self>) {
        let target = self.drop_target.take();
        let images: Vec<&std::path::PathBuf> = paths.iter().filter(|p| is_image_path(p)).collect();
        if images.is_empty() {
            self.store.update(cx, |s, cx| {
                s.last_error = Some("Only image files can be dropped into a page.".into());
                cx.notify();
            });
            return;
        }
        let mut assets = Vec::new();
        for p in images {
            let alt = p.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
            match self.store.read(cx).vault.import_asset(p) {
                Ok(rel) => assets.push((rel, alt)),
                Err(err) => self.store.update(cx, |s, cx| {
                    s.last_error = Some(format!("Could not add the image: {err:#}"));
                    cx.notify();
                }),
            }
        }
        window.focus(&self.focus);
        self.insert_images(assets, target, cx);
    }

    pub(crate) fn files_dragged(&mut self, pos: Point<Pixels>, inside: bool, cx: &mut Context<Self>) {
        self.last_mouse = Some(pos);
        // nothing moves: use a range that contains no block
        let target = if inside { self.drop_target_at(pos, &(usize::MAX..usize::MAX)) } else { None };
        if self.drop_target != target {
            self.drop_target = target;
            cx.notify();
        }
        self.ensure_autoscroll(cx);
    }

    /// Paste an image from the clipboard (e.g. a screenshot taken with ⌃⇧⌘4).
    /// Returns false when the clipboard holds no image.
    fn paste_image(&mut self, item: &gpui::ClipboardItem, cx: &mut Context<Self>) -> bool {
        let image = item.entries().iter().find_map(|e| match e {
            gpui::ClipboardEntry::Image(img) => Some(img),
            _ => None,
        });
        let Some(image) = image else { return false };
        use gpui::ImageFormat as F;
        let ext = match image.format {
            F::Png => "png",
            F::Jpeg => "jpg",
            F::Webp => "webp",
            F::Gif => "gif",
            F::Svg => "svg",
            F::Bmp => "bmp",
            F::Tiff => "tiff",
        };
        let name = format!("Pasted image {}", chrono::Local::now().format("%Y-%m-%d %H.%M.%S"));
        match self.store.read(cx).vault.add_asset(&name, ext, &image.bytes) {
            Ok(rel) => self.insert_images(vec![(rel, String::new())], None, cx),
            Err(err) => self.store.update(cx, |s, cx| {
                s.last_error = Some(format!("Could not save the image: {err:#}"));
                cx.notify();
            }),
        }
        true
    }

    pub(crate) fn start_resize(&mut self, id: u64, x: Pixels, width: f32) {
        self.checkpoint(EditKind::Other);
        self.resize_start = Some((id, x, width));
    }

    pub(crate) fn resize_moved(&mut self, drag: &ResizeImage, x: Pixels, max: f32, cx: &mut Context<Self>) {
        let Some((id, start_x, start_w)) = self.resize_start.filter(|r| r.0 == drag.id) else { return };
        let Some(ix) = self.index_of(id) else { return };
        // images are centred, so each side moves by half the change
        let w = (start_w + 2.0 * drag.side * ((x - start_x) / px(1.))).clamp(48.0, max).round() as u32;
        if let BlockKind::Image { width, .. } = &mut self.blocks[ix].kind {
            if *width != Some(w) {
                *width = Some(w);
                self.after_edit(cx);
            }
        }
    }

    // ----------------------------------------------------------------------
    // rubber-band selection and auto-scroll
    // ----------------------------------------------------------------------

    /// Start a selection rectangle at `pos` (window coordinates).
    pub fn begin_marquee(&mut self, pos: Point<Pixels>, from_tail: bool, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus);
        self.close_popups();
        self.selecting = false;
        let offset = self.scroll.offset();
        self.marquee = Some(Marquee {
            start: point(pos.x - offset.x, pos.y - offset.y),
            current: pos,
            active: false,
            from_tail,
        });
        self.last_mouse = Some(pos);
        cx.notify();
    }

    /// The rectangle's start point in window coordinates (follows scrolling).
    pub(crate) fn marquee_origin(&self) -> Option<Point<Pixels>> {
        let m = self.marquee.as_ref()?;
        let offset = self.scroll.offset();
        Some(point(m.start.x + offset.x, m.start.y + offset.y))
    }

    fn update_marquee(&mut self, pos: Point<Pixels>, cx: &mut Context<Self>) {
        let Some(origin) = self.marquee_origin() else { return };
        let Some(m) = self.marquee.as_mut() else { return };
        m.current = pos;
        if !m.active {
            if (pos - origin).magnitude() < 4.0 {
                return;
            }
            m.active = true;
            self.sel.range = self.sel.cursor()..self.sel.cursor();
        }
        let (top, bottom) = (origin.y.min(pos.y), origin.y.max(pos.y));
        let hit: Vec<usize> = self
            .visible_rows()
            .into_iter()
            .filter(|(_, b)| b.bottom() > top && b.top() < bottom)
            .map(|(i, _)| i)
            .collect();
        self.block_sel = match (hit.first(), hit.last()) {
            (Some(&first), Some(&last)) if pos.y >= origin.y => Some((first, last)),
            (Some(&first), Some(&last)) => Some((last, first)),
            _ => None,
        };
        cx.notify();
    }

    fn end_marquee(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(m) = self.marquee.take() {
            if !m.active {
                self.block_sel = None;
                if m.from_tail {
                    self.click_below(window, cx);
                }
            }
            cx.notify();
        }
    }

    /// Scroll speed for a pointer near the top or bottom edge of the page.
    fn edge_speed(&self, pos: Point<Pixels>) -> Option<Pixels> {
        let vp = self.scroll.bounds();
        if vp.size.height <= px(0.) {
            return None;
        }
        let edge = px(56.);
        let dy = if pos.y < vp.top() + edge {
            vp.top() + edge - pos.y
        } else if pos.y > vp.bottom() - edge {
            -(pos.y - (vp.bottom() - edge))
        } else {
            return None;
        };
        let step = (dy / px(1.) * 0.35).clamp(-24.0, 24.0);
        (step.abs() >= 0.5).then_some(px(step))
    }

    fn pointer_busy(&self, cx: &App) -> bool {
        self.selecting || self.marquee.as_ref().is_some_and(|m| m.active) || cx.has_active_drag()
    }

    /// Keep scrolling while the pointer rests near an edge during a drag.
    fn ensure_autoscroll(&mut self, cx: &mut Context<Self>) {
        if self.autoscrolling || !self.pointer_busy(cx) {
            return;
        }
        if self.last_mouse.and_then(|p| self.edge_speed(p)).is_none() {
            return;
        }
        self.autoscrolling = true;
        self._autoscroll = Some(cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(Duration::from_millis(16)).await;
            let more = this.update(cx, |this, cx| this.autoscroll_step(cx)).unwrap_or(false);
            if !more {
                this.update(cx, |this, _| this.autoscrolling = false).ok();
                break;
            }
        }));
    }

    fn autoscroll_step(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.pointer_busy(cx) {
            return false;
        }
        let Some(pos) = self.last_mouse else { return false };
        let Some(step) = self.edge_speed(pos) else { return false };
        let mut offset = self.scroll.offset();
        let max = self.scroll.max_offset().height;
        let y = (offset.y + step).min(px(0.)).max(-max);
        if y == offset.y {
            return false;
        }
        offset.y = y;
        self.scroll.set_offset(offset);
        if self.marquee.is_some() {
            self.update_marquee(pos, cx);
        } else if self.selecting {
            self.extend_selection(pos, cx);
        }
        cx.notify();
        true
    }

    fn move_block(&mut self, up: bool, cx: &mut Context<Self>) {
        let ix = self.sel.block;
        let end = subtree_end(&self.blocks, ix);
        let indent = self.blocks[ix].indent;
        let caret = self.sel.clone();
        if up {
            // find the previous sibling-or-shallower block start
            let Some(mut prev) = ix.checked_sub(1) else {
                return;
            };
            while prev > 0 && self.blocks[prev].indent > indent {
                prev -= 1;
            }
            self.checkpoint(EditKind::Other);
            let moved: Vec<Block> = self.blocks.drain(ix..end).collect();
            for (i, b) in moved.into_iter().enumerate() {
                self.blocks.insert(prev + i, b);
            }
            self.sel = Sel {
                block: prev,
                ..caret
            };
        } else {
            if end >= self.blocks.len() {
                return;
            }
            let next_end = subtree_end(&self.blocks, end);
            self.checkpoint(EditKind::Other);
            let moved: Vec<Block> = self.blocks.drain(ix..end).collect();
            let at = next_end - moved.len();
            for (i, b) in moved.into_iter().enumerate() {
                self.blocks.insert(at + i, b);
            }
            self.sel = Sel { block: at, ..caret };
        }
        self.after_edit(cx);
    }

    fn indent_blocks(&mut self, outdent: bool, cx: &mut Context<Self>) {
        let range = match self.block_sel_range() {
            Some(r) => expand_to_subtrees(&self.blocks, r),
            None => self.sel.block..subtree_end(&self.blocks, self.sel.block),
        };
        if range.start == 0 && !outdent {
            return;
        }
        if outdent {
            if self.blocks[range.start].indent == 0 {
                return;
            }
        } else {
            let prev_indent = self.blocks[range.start - 1].indent;
            if self.blocks[range.start].indent > prev_indent {
                return;
            }
        }
        self.checkpoint(EditKind::Other);
        for b in &mut self.blocks[range] {
            if outdent {
                b.indent = b.indent.saturating_sub(1);
            } else {
                b.indent = b.indent.saturating_add(1);
            }
        }
        self.after_edit(cx);
    }

    pub(crate) fn toggle_todo(&mut self, ix: usize, cx: &mut Context<Self>) {
        if let BlockKind::Todo { checked } = self.blocks[ix].kind {
            self.checkpoint(EditKind::Other);
            self.blocks[ix].kind = BlockKind::Todo { checked: !checked };
            self.after_edit(cx);
        }
    }

    pub(crate) fn toggle_open(&mut self, ix: usize, cx: &mut Context<Self>) {
        if let BlockKind::Toggle { open } = self.blocks[ix].kind {
            self.blocks[ix].kind = BlockKind::Toggle { open: !open };
            // keep the caret out of hidden blocks
            let vis = visibility(&self.blocks);
            if !vis[self.sel.block] {
                self.sel = Sel::caret(ix, self.blocks[ix].text.len());
            }
            // An empty toggle that is opened gets a child to type into.
            if !open && subtree_end(&self.blocks, ix) == ix + 1 {
                let indent = self.blocks[ix].indent + 1;
                self.blocks
                    .insert(ix + 1, Block::paragraph("").with_indent(indent));
            }
            cx.emit(EditorEvent::Changed);
            cx.notify();
        }
    }

    pub(crate) fn cycle_callout_icon(&mut self, ix: usize, cx: &mut Context<Self>) {
        const ICONS: [&str; 8] = ["💡", "📌", "⚠️", "✅", "❗", "📝", "🔥", "❓"];
        if let BlockKind::Callout { icon } = &self.blocks[ix].kind {
            let pos = ICONS.iter().position(|i| i == icon).unwrap_or(ICONS.len() - 1);
            let next = ICONS[(pos + 1) % ICONS.len()].to_string();
            self.checkpoint(EditKind::Other);
            self.blocks[ix].kind = BlockKind::Callout { icon: next };
            self.after_edit(cx);
        }
    }

    pub(crate) fn add_block_below(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.checkpoint(EditKind::Other);
        let indent = self.blocks[ix].indent;
        let end = subtree_end(&self.blocks, ix);
        let at = if self.blocks[ix].kind == BlockKind::Paragraph && self.blocks[ix].text.is_empty() {
            ix
        } else {
            self.blocks
                .insert(end, Block::paragraph("").with_indent(indent));
            end
        };
        self.blocks[at].text = RichText::plain("/");
        self.sel = Sel::caret(at, 1);
        self.block_sel = None;
        self.slash = Some(SlashState {
            block: at,
            start: 0,
            selected: 0,
        });
        window.focus(&self.focus);
        self.after_edit(cx);
    }

    pub(crate) fn toggle_mark(&mut self, kind: MarkKind, cx: &mut Context<Self>) {
        let ix = self.sel.block;
        if self.blocks[ix].kind.is_raw() || !self.blocks[ix].kind.has_text() {
            return;
        }
        if self.sel.range.is_empty() {
            let mut marks = self
                .pending_marks
                .clone()
                .unwrap_or_else(|| self.blocks[ix].text.marks_at(self.sel.cursor()));
            let on = marks.get(kind);
            marks.set(kind, !on);
            self.pending_marks = Some(marks);
            cx.notify();
        } else {
            self.checkpoint(EditKind::Other);
            self.blocks[ix].text.toggle(self.sel.range.clone(), kind);
            self.after_edit(cx);
        }
    }

    pub(crate) fn selection_has(&self, kind: MarkKind) -> bool {
        let b = self.cur();
        if self.sel.range.is_empty() {
            self.pending_marks
                .clone()
                .unwrap_or_else(|| b.text.marks_at(self.sel.cursor()))
                .get(kind)
        } else {
            b.text.range_has(self.sel.range.clone(), kind)
        }
    }

    // ----------------------------------------------------------------------
    // actions
    // ----------------------------------------------------------------------

    pub(crate) fn backspace(&mut self, _: &Backspace, _: &mut Window, cx: &mut Context<Self>) {
        if self.delete_block_selection(cx) {
            return;
        }
        let ix = self.sel.block;
        if !self.sel.range.is_empty() {
            let r = self.sel.range.clone();
            self.checkpoint(EditKind::Other);
            self.blocks[ix].text.replace(r.clone(), "", None);
            self.sel = Sel::caret(ix, r.start);
            self.after_slash_delete();
            self.after_edit(cx);
            return;
        }
        let caret = self.sel.cursor();
        if caret > 0 {
            let p = prev_grapheme(self.text(), caret);
            self.checkpoint(EditKind::Typing);
            self.blocks[ix].text.replace(p..caret, "", None);
            self.sel = Sel::caret(ix, p);
            self.after_slash_delete();
            self.after_edit(cx);
            return;
        }
        // caret at the start of the block
        let kind = self.blocks[ix].kind.clone();
        if kind != BlockKind::Paragraph {
            self.checkpoint(EditKind::Other);
            self.blocks[ix].kind = BlockKind::Paragraph;
            self.after_edit(cx);
            return;
        }
        if self.blocks[ix].indent > 0 {
            self.indent_blocks(true, cx);
            return;
        }
        let Some(prev) = self.prev_visible(ix) else {
            return;
        };
        if !self.blocks[prev].kind.has_text() {
            if self.blocks[ix].text.is_empty() && ix + 1 < self.blocks.len() {
                // an empty line after a divider: just remove the line
                self.checkpoint(EditKind::Other);
                self.blocks.remove(ix);
                self.block_sel = Some((prev, prev));
                self.sel = Sel::caret(prev, 0);
                self.after_edit(cx);
            } else {
                self.block_sel = Some((prev, prev));
                cx.notify();
            }
            return;
        }
        self.checkpoint(EditKind::Other);
        let block = self.blocks.remove(ix);
        let at = self.blocks[prev].text.len();
        if self.blocks[prev].kind.is_raw() {
            self.blocks[prev].text.replace(at..at, &block.text.text, Some(Marks::default()));
        } else {
            self.blocks[prev].text.append(&block.text);
        }
        self.sel = Sel::caret(prev, at);
        self.after_edit(cx);
    }

    fn after_slash_delete(&mut self) {
        if let Some(slash) = &self.slash {
            let caret = self.sel.cursor();
            let text = &self.blocks[self.sel.block].text.text;
            if slash.block != self.sel.block
                || caret <= slash.start
                || text.get(slash.start..slash.start + 1) != Some("/")
            {
                self.slash = None;
            }
        }
    }

    pub(crate) fn delete(&mut self, _: &Delete, _: &mut Window, cx: &mut Context<Self>) {
        if self.delete_block_selection(cx) {
            return;
        }
        let ix = self.sel.block;
        if !self.sel.range.is_empty() {
            let r = self.sel.range.clone();
            self.checkpoint(EditKind::Other);
            self.blocks[ix].text.replace(r.clone(), "", None);
            self.sel = Sel::caret(ix, r.start);
            self.after_edit(cx);
            return;
        }
        let caret = self.sel.cursor();
        if caret < self.text().len() {
            let n = next_grapheme(self.text(), caret);
            self.checkpoint(EditKind::Typing);
            self.blocks[ix].text.replace(caret..n, "", None);
            self.after_edit(cx);
            return;
        }
        let Some(next) = self.next_visible(ix) else {
            return;
        };
        self.checkpoint(EditKind::Other);
        if !self.blocks[next].kind.has_text() {
            self.blocks.remove(next);
        } else {
            let block = self.blocks.remove(next);
            self.blocks[ix].text.append(&block.text);
        }
        self.after_edit(cx);
    }

    pub(crate) fn delete_word_left(&mut self, _: &DeleteWordLeft, window: &mut Window, cx: &mut Context<Self>) {
        if self.block_sel.is_none() && self.sel.range.is_empty() && self.cursor() > 0 {
            let p = prev_word(self.text(), self.cursor());
            let ix = self.sel.block;
            let caret = self.cursor();
            self.checkpoint(EditKind::Other);
            self.blocks[ix].text.replace(p..caret, "", None);
            self.sel = Sel::caret(ix, p);
            self.after_slash_delete();
            self.after_edit(cx);
        } else {
            self.backspace(&Backspace, window, cx);
        }
    }

    pub(crate) fn delete_to_line_start(&mut self, _: &DeleteToLineStart, window: &mut Window, cx: &mut Context<Self>) {
        if self.block_sel.is_none() && self.sel.range.is_empty() && self.cursor() > 0 {
            let ix = self.sel.block;
            let caret = self.cursor();
            let start = self
                .layout_of(ix)
                .map(|l| l.row_range(caret).start)
                .unwrap_or(0)
                .min(caret);
            let start = if start == caret { prev_grapheme(self.text(), caret) } else { start };
            self.checkpoint(EditKind::Other);
            self.blocks[ix].text.replace(start..caret, "", None);
            self.sel = Sel::caret(ix, start);
            self.after_slash_delete();
            self.after_edit(cx);
        } else {
            self.backspace(&Backspace, window, cx);
        }
    }

    pub(crate) fn enter(&mut self, _: &Enter, window: &mut Window, cx: &mut Context<Self>) {
        if self.slash.is_some() {
            let selected = self.slash.as_ref().map(|s| s.selected).unwrap_or(0);
            if self.filtered_slash_items().is_empty() {
                self.slash = None;
            } else {
                self.apply_slash(selected, window, cx);
                return;
            }
        }
        if let Some(range) = self.block_sel_range() {
            // Enter on a block selection: start editing the first text block.
            self.block_sel = None;
            if let Some(ix) = range.clone().find(|&i| self.blocks[i].kind.has_text()) {
                let len = self.blocks[ix].text.len();
                self.set_caret(ix, len, cx);
            } else {
                self.checkpoint(EditKind::Other);
                let indent = self.blocks[range.end - 1].indent;
                self.blocks
                    .insert(range.end, Block::paragraph("").with_indent(indent));
                self.sel = Sel::caret(range.end, 0);
                self.after_edit(cx);
            }
            return;
        }
        let ix = self.sel.block;
        let kind = self.blocks[ix].kind.clone();
        if kind.is_raw() {
            // keep the indentation of the current line
            let caret = self.sel.range.start;
            let text = self.text();
            let line_start = text[..caret].rfind('\n').map(|i| i + 1).unwrap_or(0);
            let indent: String = text[line_start..caret]
                .chars()
                .take_while(|c| *c == ' ' || *c == '\t')
                .collect();
            let r = self.sel.range.clone();
            self.insert_text(r, &format!("\n{}", indent), cx);
            return;
        }
        self.checkpoint(EditKind::Other);
        // remove any selected text first
        if !self.sel.range.is_empty() {
            let r = self.sel.range.clone();
            self.blocks[ix].text.replace(r.clone(), "", None);
            self.sel = Sel::caret(ix, r.start);
        }
        let caret = self.sel.cursor();
        let indent = self.blocks[ix].indent;
        let empty = self.blocks[ix].text.is_empty();

        // An empty list item / quote / heading turns back into text (or outdents).
        if empty && kind != BlockKind::Paragraph {
            if indent > 0 && kind.is_list() {
                self.blocks[ix].indent -= 1;
            } else {
                self.blocks[ix].kind = BlockKind::Paragraph;
            }
            self.after_edit(cx);
            return;
        }

        let next_kind = match &kind {
            BlockKind::Bullet => BlockKind::Bullet,
            BlockKind::Numbered => BlockKind::Numbered,
            BlockKind::Todo { .. } => BlockKind::Todo { checked: false },
            _ => BlockKind::Paragraph,
        };
        let has_children = subtree_end(&self.blocks, ix) > ix + 1;
        let at_end = caret == self.blocks[ix].text.len();
        let open_toggle = matches!(kind, BlockKind::Toggle { open: true });

        if caret == 0 && !empty {
            // Enter at the start: push the block down, leaving an empty one above.
            self.blocks
                .insert(ix, Block::paragraph("").with_indent(indent));
            self.sel = Sel::caret(ix + 1, 0);
        } else if at_end && (open_toggle || (has_children && kind.is_list())) {
            // new first child
            let child_kind = if open_toggle { BlockKind::Paragraph } else { next_kind };
            self.blocks
                .insert(ix + 1, Block::new(child_kind, "").with_indent(indent + 1));
            self.sel = Sel::caret(ix + 1, 0);
        } else {
            let tail = self.blocks[ix].text.split_off(caret);
            let insert_at = if matches!(kind, BlockKind::Toggle { open: false }) {
                subtree_end(&self.blocks, ix)
            } else {
                ix + 1
            };
            let new_kind = if matches!(kind, BlockKind::Toggle { .. }) {
                BlockKind::Toggle { open: true }
            } else {
                next_kind
            };
            self.blocks
                .insert(insert_at, Block::new(new_kind, tail).with_indent(indent));
            self.sel = Sel::caret(insert_at, 0);
        }
        self.pending_marks = None;
        self.after_edit(cx);
    }

    pub(crate) fn shift_enter(&mut self, _: &ShiftEnter, _: &mut Window, cx: &mut Context<Self>) {
        if self.block_sel.is_some() || !self.cur().kind.has_text() {
            return;
        }
        let r = self.sel.range.clone();
        self.insert_text(r, "\n", cx);
    }

    pub(crate) fn tab(&mut self, _: &Tab, _: &mut Window, cx: &mut Context<Self>) {
        if self.block_sel.is_none() && self.cur().kind.is_raw() {
            let r = self.sel.range.clone();
            self.insert_text(r, "    ", cx);
            return;
        }
        self.indent_blocks(false, cx);
    }

    pub(crate) fn shift_tab(&mut self, _: &ShiftTab, _: &mut Window, cx: &mut Context<Self>) {
        self.indent_blocks(true, cx);
    }

    pub(crate) fn escape(&mut self, _: &Escape, _: &mut Window, cx: &mut Context<Self>) {
        if self.slash.is_some() || self.block_menu.is_some() {
            self.close_popups();
        } else if self.block_sel.is_some() {
            self.block_sel = None;
        } else if !self.sel.range.is_empty() {
            let c = self.cursor();
            self.sel = Sel::caret(self.sel.block, c);
        } else {
            // select the current block, like Notion
            self.block_sel = Some((self.sel.block, self.sel.block));
        }
        cx.notify();
    }

    pub(crate) fn left(&mut self, _: &Left, _: &mut Window, cx: &mut Context<Self>) {
        self.goal_x = None;
        if let Some(r) = self.block_sel_range() {
            self.block_sel = None;
            if let Some(ix) = r.clone().find(|&i| self.blocks[i].kind.has_text()) {
                self.set_caret(ix, 0, cx);
            }
            cx.notify();
            return;
        }
        if !self.sel.range.is_empty() {
            let s = self.sel.range.start;
            self.set_caret(self.sel.block, s, cx);
        } else if self.cursor() > 0 {
            let p = prev_grapheme(self.text(), self.cursor());
            self.set_caret(self.sel.block, p, cx);
        } else if let Some(prev) = self.prev_text_block(self.sel.block) {
            let len = self.blocks[prev].text.len();
            self.set_caret(prev, len, cx);
        }
        self.after_slash_delete();
    }

    pub(crate) fn right(&mut self, _: &Right, _: &mut Window, cx: &mut Context<Self>) {
        self.goal_x = None;
        if let Some(r) = self.block_sel_range() {
            self.block_sel = None;
            if let Some(ix) = r.clone().rev().find(|&i| self.blocks[i].kind.has_text()) {
                let len = self.blocks[ix].text.len();
                self.set_caret(ix, len, cx);
            }
            cx.notify();
            return;
        }
        if !self.sel.range.is_empty() {
            let e = self.sel.range.end;
            self.set_caret(self.sel.block, e, cx);
        } else if self.cursor() < self.text().len() {
            let n = next_grapheme(self.text(), self.cursor());
            self.set_caret(self.sel.block, n, cx);
        } else if let Some(next) = self.next_text_block(self.sel.block) {
            self.set_caret(next, 0, cx);
        }
    }

    pub(crate) fn up(&mut self, _: &Up, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(slash) = &mut self.slash {
            slash.selected = slash.selected.saturating_sub(1);
            cx.notify();
            return;
        }
        if let Some(r) = self.block_sel_range() {
            self.block_sel = None;
            let target = self.prev_visible(r.start).unwrap_or(r.start);
            if self.blocks[target].kind.has_text() {
                let len = self.blocks[target].text.len();
                self.set_caret(target, len, cx);
            } else {
                self.block_sel = Some((target, target));
            }
            cx.notify();
            return;
        }
        let ix = self.sel.block;
        let caret = self.cursor();
        let layout = self.layout_of(ix);
        let x = self
            .goal_x
            .or_else(|| layout.as_ref().map(|l| l.x_of(caret)))
            .unwrap_or(px(0.));
        if let Some(target) = layout.as_ref().and_then(|l| l.index_above(caret, Some(x))) {
            self.set_caret(ix, target, cx);
            self.goal_x = Some(x);
            return;
        }
        match self.prev_text_block(ix) {
            Some(prev) => {
                let offset = self
                    .layout_of(prev)
                    .map(|l| l.index_at_x_on_edge_row(true, x))
                    .unwrap_or(self.blocks[prev].text.len());
                self.set_caret(prev, offset, cx);
                self.goal_x = Some(x);
            }
            None => {
                if caret == 0 || layout.is_none() {
                    cx.emit(EditorEvent::FocusTitle);
                } else {
                    self.set_caret(ix, 0, cx);
                }
            }
        }
    }

    pub(crate) fn down(&mut self, _: &Down, _: &mut Window, cx: &mut Context<Self>) {
        if self.slash.is_some() {
            let n = self.filtered_slash_items().len();
            if let Some(slash) = &mut self.slash {
                if n > 0 {
                    slash.selected = (slash.selected + 1).min(n - 1);
                }
            }
            cx.notify();
            return;
        }
        if let Some(r) = self.block_sel_range() {
            self.block_sel = None;
            let target = self.next_visible(r.end - 1).unwrap_or(r.end - 1);
            if self.blocks[target].kind.has_text() {
                self.set_caret(target, 0, cx);
            } else {
                self.block_sel = Some((target, target));
            }
            cx.notify();
            return;
        }
        let ix = self.sel.block;
        let caret = self.cursor();
        let layout = self.layout_of(ix);
        let x = self
            .goal_x
            .or_else(|| layout.as_ref().map(|l| l.x_of(caret)))
            .unwrap_or(px(0.));
        if let Some(target) = layout.as_ref().and_then(|l| l.index_below(caret, Some(x))) {
            self.set_caret(ix, target, cx);
            self.goal_x = Some(x);
            return;
        }
        match self.next_text_block(ix) {
            Some(next) => {
                let offset = self
                    .layout_of(next)
                    .map(|l| l.index_at_x_on_edge_row(false, x))
                    .unwrap_or(0);
                self.set_caret(next, offset, cx);
                self.goal_x = Some(x);
            }
            None => {
                let len = self.text().len();
                if caret == len && !(self.cur().kind == BlockKind::Paragraph && len == 0) && ix + 1 == self.blocks.len() {
                    // leaving the last block downwards: open a new line
                    self.checkpoint(EditKind::Other);
                    self.blocks.push(Block::paragraph(""));
                    self.sel = Sel::caret(self.blocks.len() - 1, 0);
                    self.after_edit(cx);
                } else {
                    self.set_caret(ix, len, cx);
                }
            }
        }
    }

    pub(crate) fn select_left(&mut self, _: &SelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        if self.block_sel.is_some() {
            return;
        }
        let p = prev_grapheme(self.text(), self.cursor());
        self.select_to(p, cx);
    }

    pub(crate) fn select_right(&mut self, _: &SelectRight, _: &mut Window, cx: &mut Context<Self>) {
        if self.block_sel.is_some() {
            return;
        }
        let n = next_grapheme(self.text(), self.cursor());
        self.select_to(n, cx);
    }

    pub(crate) fn select_up(&mut self, _: &SelectUp, _: &mut Window, cx: &mut Context<Self>) {
        if let Some((a, h)) = self.block_sel {
            let h = self.prev_visible(h).unwrap_or(h);
            self.block_sel = Some((a, h));
            cx.notify();
            return;
        }
        let ix = self.sel.block;
        match self.layout_of(ix).and_then(|l| l.index_above(self.cursor(), None)) {
            Some(t) => self.select_to(t, cx),
            None => {
                if self.cursor() > 0 && self.sel.range.is_empty() {
                    self.select_to(0, cx);
                } else if let Some(prev) = self.prev_visible(ix) {
                    self.block_sel = Some((ix, prev));
                    cx.notify();
                } else {
                    self.select_to(0, cx);
                }
            }
        }
    }

    pub(crate) fn select_down(&mut self, _: &SelectDown, _: &mut Window, cx: &mut Context<Self>) {
        if let Some((a, h)) = self.block_sel {
            let h = self.next_visible(h).unwrap_or(h);
            self.block_sel = Some((a, h));
            cx.notify();
            return;
        }
        let ix = self.sel.block;
        let len = self.text().len();
        match self.layout_of(ix).and_then(|l| l.index_below(self.cursor(), None)) {
            Some(t) => self.select_to(t, cx),
            None => {
                if self.cursor() < len && self.sel.range.is_empty() {
                    self.select_to(len, cx);
                } else if let Some(next) = self.next_visible(ix) {
                    self.block_sel = Some((ix, next));
                    cx.notify();
                } else {
                    self.select_to(len, cx);
                }
            }
        }
    }

    pub(crate) fn word_left(&mut self, _: &WordLeft, window: &mut Window, cx: &mut Context<Self>) {
        if self.block_sel.is_some() || self.cursor() == 0 {
            self.left(&Left, window, cx);
            return;
        }
        let p = prev_word(self.text(), self.cursor());
        self.set_caret(self.sel.block, p, cx);
    }

    pub(crate) fn word_right(&mut self, _: &WordRight, window: &mut Window, cx: &mut Context<Self>) {
        if self.block_sel.is_some() || self.cursor() == self.text().len() {
            self.right(&Right, window, cx);
            return;
        }
        let n = next_word(self.text(), self.cursor());
        self.set_caret(self.sel.block, n, cx);
    }

    pub(crate) fn select_word_left(&mut self, _: &SelectWordLeft, _: &mut Window, cx: &mut Context<Self>) {
        if self.block_sel.is_none() {
            let p = prev_word(self.text(), self.cursor());
            self.select_to(p, cx);
        }
    }

    pub(crate) fn select_word_right(&mut self, _: &SelectWordRight, _: &mut Window, cx: &mut Context<Self>) {
        if self.block_sel.is_none() {
            let n = next_word(self.text(), self.cursor());
            self.select_to(n, cx);
        }
    }

    fn row_range(&self) -> Range<usize> {
        self.layout_of(self.sel.block)
            .map(|l| l.row_range(self.cursor()))
            .unwrap_or(0..self.text().len())
    }

    pub(crate) fn line_start(&mut self, _: &LineStart, _: &mut Window, cx: &mut Context<Self>) {
        if self.block_sel.is_none() {
            let s = self.row_range().start;
            self.set_caret(self.sel.block, s, cx);
        }
    }

    pub(crate) fn line_end(&mut self, _: &LineEnd, _: &mut Window, cx: &mut Context<Self>) {
        if self.block_sel.is_none() {
            let e = self.row_range().end;
            self.set_caret(self.sel.block, e, cx);
        }
    }

    pub(crate) fn select_line_start(&mut self, _: &SelectLineStart, _: &mut Window, cx: &mut Context<Self>) {
        if self.block_sel.is_none() {
            let s = self.row_range().start;
            self.select_to(s, cx);
        }
    }

    pub(crate) fn select_line_end(&mut self, _: &SelectLineEnd, _: &mut Window, cx: &mut Context<Self>) {
        if self.block_sel.is_none() {
            let e = self.row_range().end;
            self.select_to(e, cx);
        }
    }

    pub(crate) fn doc_start(&mut self, _: &DocStart, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(ix) = self.first_text_block() {
            self.set_caret(ix, 0, cx);
        }
    }

    pub(crate) fn doc_end(&mut self, _: &DocEnd, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(ix) = self.last_text_block() {
            let len = self.blocks[ix].text.len();
            self.set_caret(ix, len, cx);
        }
    }

    pub(crate) fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        let len = self.text().len();
        if self.block_sel.is_some() || self.sel.range == (0..len) {
            self.block_sel = Some((0, self.blocks.len() - 1));
        } else {
            self.sel.range = 0..len;
            self.sel.reversed = false;
        }
        cx.notify();
    }

    /// The current selection, for Nota AI.
    pub fn ai_selection(&self) -> Option<AiSelection> {
        let (plain, markdown) = self.clipboard_contents()?;
        if plain.trim().is_empty() {
            return None;
        }
        let target = match self.block_sel_range() {
            Some(r) => {
                let r = expand_to_subtrees(&self.blocks, r);
                AiTarget::Blocks { first: self.blocks[r.start].id, last: self.blocks[r.end - 1].id }
            }
            None => AiTarget::Text { block: self.cur().id, range: self.sel.range.clone() },
        };
        let flat = plain.split_whitespace().collect::<Vec<_>>().join(" ");
        let preview: String = flat.chars().take(48).collect();
        let preview = if flat.chars().count() > 48 { format!("{preview}…") } else { preview };
        Some(AiSelection { markdown, preview, target })
    }

    /// Put an AI answer into the page: in place of `target` (`replace`) or
    /// right below it. Returns false if the target no longer exists.
    pub fn apply_ai(&mut self, md: &str, target: &AiTarget, replace: bool, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let mut blocks = from_markdown(md);
        if blocks.is_empty() {
            return true;
        }
        let range = match target {
            AiTarget::Text { block, range } => {
                let Some(ix) = self.index_of(*block) else { return false };
                if replace && self.blocks[ix].kind.has_text() {
                    let len = self.blocks[ix].text.len();
                    let text = &self.blocks[ix].text.text;
                    let r = clamp(text, range.start.min(len))..clamp(text, range.end.min(len));
                    self.block_sel = None;
                    self.sel = Sel { block: ix, range: r, reversed: false };
                    window.focus(&self.focus);
                    self.insert_markdown(md.to_string(), true, cx);
                    return true;
                }
                ix..subtree_end(&self.blocks, ix)
            }
            AiTarget::Blocks { first, last } => {
                let (Some(a), Some(b)) = (self.index_of(*first), self.index_of(*last)) else { return false };
                expand_to_subtrees(&self.blocks, a.min(b)..a.max(b) + 1)
            }
        };
        self.checkpoint(EditKind::Other);
        let indent = self.blocks[range.start].indent;
        for b in &mut blocks {
            b.indent = b.indent.saturating_add(indent);
        }
        let n = blocks.len();
        let at = if replace {
            self.blocks.splice(range.clone(), blocks);
            range.start
        } else {
            self.blocks.splice(range.end..range.end, blocks);
            range.end
        };
        self.block_sel = Some((at, at + n - 1));
        self.sel = Sel::caret(at, 0);
        self.scroll_pending = true;
        window.focus(&self.focus);
        self.after_edit(cx);
        true
    }

    /// Selected blocks (with their children) as Markdown.
    fn selected_blocks_markdown(&self) -> Option<String> {
        let r = expand_to_subtrees(&self.blocks, self.block_sel_range()?);
        let mut blocks: Vec<Block> = self.blocks[r].to_vec();
        let base = blocks.iter().map(|b| b.indent).min().unwrap_or(0);
        for b in &mut blocks {
            b.indent -= base;
        }
        Some(to_markdown(&blocks))
    }

    /// What goes on the clipboard: (plain text, Markdown kept for pasting back into Nota).
    fn clipboard_contents(&self) -> Option<(String, String)> {
        if let Some(md) = self.selected_blocks_markdown() {
            return Some((md.clone(), md));
        }
        if self.sel.range.is_empty() {
            return None;
        }
        let r = self.sel.range.clone();
        let plain = self.text()[r.clone()].to_string();
        if self.cur().kind.is_raw() {
            return Some((plain.clone(), plain));
        }
        let mut rich = self.cur().text.clone();
        rich.split_off(r.end);
        let part = rich.split_off(r.start);
        Some((plain, nota_core::markdown::inline_to_md(&part)))
    }

    fn selected_markdown(&self) -> Option<String> {
        if let Some(r) = self.block_sel_range() {
            let r = expand_to_subtrees(&self.blocks, r);
            let mut blocks: Vec<Block> = self.blocks[r].to_vec();
            let base = blocks.iter().map(|b| b.indent).min().unwrap_or(0);
            for b in &mut blocks {
                b.indent -= base;
            }
            return Some(to_markdown(&blocks));
        }
        if self.sel.range.is_empty() {
            return None;
        }
        Some(self.text()[self.sel.range.clone()].to_string())
    }

    pub(crate) fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if let Some((plain, md)) = self.clipboard_contents() {
            cx.write_to_clipboard(ClipboardItem::new_string_with_json_metadata(plain, md));
        }
    }

    pub(crate) fn cut(&mut self, _: &Cut, window: &mut Window, cx: &mut Context<Self>) {
        if let Some((plain, md)) = self.clipboard_contents() {
            cx.write_to_clipboard(ClipboardItem::new_string_with_json_metadata(plain, md));
            self.backspace(&Backspace, window, cx);
        }
    }

    pub(crate) fn paste(&mut self, _: &Paste, window: &mut Window, cx: &mut Context<Self>) {
        let Some(item) = cx.read_from_clipboard() else {
            return;
        };
        let text = item.text();
        if text.is_none() && self.paste_image(&item, cx) {
            return;
        }
        // Copied from Nota: the Markdown version keeps formatting and structure.
        let nota_md = item.entries().first().and_then(|e| match e {
            gpui::ClipboardEntry::String(s) => s.metadata_json::<String>(),
            _ => None,
        });
        let from_nota = nota_md.is_some();
        let Some(text) = nota_md.or(text) else {
            return;
        };
        // a copied image file (path or file:// URL)
        let as_path = text.trim().strip_prefix("file://").map(|p| nota_core::markdown::decode_url(p)).unwrap_or_else(|| text.trim().to_string());
        let path = std::path::PathBuf::from(&as_path);
        if !as_path.contains('\n') && path.is_absolute() && is_image_path(&path) && path.is_file() {
            self.drop_target = None;
            self.drop_files(&[path], window, cx);
            return;
        }
        self.insert_markdown(text, from_nota, cx);
    }

    /// Insert pasted (or AI-written) text at the caret, replacing the selection.
    /// `structured` text is Markdown from Nota; anything else is plain text.
    fn insert_markdown(&mut self, text: String, structured: bool, cx: &mut Context<Self>) {
        let from_nota = structured;
        let text = text.replace("\r\n", "\n");
        if text.is_empty() {
            return;
        }
        self.delete_block_selection(cx);
        let ix = self.sel.block;
        if !self.blocks[ix].kind.has_text() {
            return;
        }
        let is_code = self.blocks[ix].kind.is_raw();
        if is_code {
            let r = self.sel.range.clone();
            self.insert_text(r, &text, cx);
            return;
        }
        // Pasting a URL over a selection makes a link.
        if !self.sel.range.is_empty() && looks_like_url(&text) {
            self.checkpoint(EditKind::Other);
            let r = self.sel.range.clone();
            self.blocks[ix].text.set_link(r, Some(text.trim().to_string()));
            self.after_edit(cx);
            return;
        }
        let trimmed = text.trim_end_matches('\n');
        if !trimmed.contains('\n') {
            // single line: keep inline formatting from markdown
            self.checkpoint(EditKind::Other);
            let r = self.sel.range.clone();
            let rich = if looks_like_url(trimmed) {
                let mut rt = RichText::plain(trimmed.trim());
                let len = rt.len();
                rt.set_link(0..len, Some(trimmed.trim().to_string()));
                rt
            } else {
                nota_core::markdown::inline_from_md(trimmed)
            };
            self.blocks[ix].text.replace(r.clone(), "", None);
            self.blocks[ix].text.insert_rich(r.start, &rich);
            self.sel = Sel::caret(ix, r.start + rich.len());
            self.after_edit(cx);
            return;
        }
        let source = if from_nota { text.clone() } else { lines_as_blocks(&text) };
        let mut pasted = from_markdown(&source);
        if pasted.is_empty() {
            return;
        }
        self.checkpoint(EditKind::Other);
        // the pasted blocks replace the selected text
        let r = self.sel.range.clone();
        if !r.is_empty() {
            self.blocks[ix].text.replace(r.clone(), "", None);
        }
        let caret = r.start;
        let indent = self.blocks[ix].indent;
        for b in &mut pasted {
            b.indent = b.indent.saturating_add(indent);
        }
        if self.blocks[ix].kind == BlockKind::Paragraph && self.blocks[ix].text.is_empty() {
            let n = pasted.len();
            self.blocks.splice(ix..ix + 1, pasted);
            let last = ix + n - 1;
            let end = if self.blocks[last].kind.has_text() { self.blocks[last].text.len() } else { 0 };
            self.sel = Sel::caret(last, end);
            self.after_edit(cx);
            return;
        }
        // Split the current line around the caret, like Notion: the first
        // pasted paragraph continues it and the rest of the line follows the paste.
        let tail = self.blocks[ix].text.split_off(caret);
        if pasted[0].kind == BlockKind::Paragraph && !self.blocks[ix].kind.is_raw() {
            let first = pasted.remove(0);
            self.blocks[ix].text.append(&first.text);
        }
        let at = if tail.is_empty() { subtree_end(&self.blocks, ix) } else { ix + 1 };
        let n = pasted.len();
        self.blocks.splice(at..at, pasted);
        let last = if n == 0 { ix } else { at + n - 1 };
        let end = if self.blocks[last].kind.has_text() { self.blocks[last].text.len() } else { 0 };
        if !tail.is_empty() {
            if self.blocks[last].kind.has_text() && !self.blocks[last].kind.is_raw() {
                self.blocks[last].text.append(&tail);
            } else {
                let depth = self.blocks[ix].indent;
                self.blocks.insert(last + 1, Block::paragraph(tail).with_indent(depth));
            }
        }
        self.sel = Sel::caret(last, end);
        self.after_edit(cx);
    }

    pub(crate) fn undo(&mut self, _: &Undo, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(snap) = self.undo.pop() {
            self.redo.push(Snapshot {
                blocks: self.blocks.clone(),
                sel: self.sel.clone(),
            });
            self.blocks = snap.blocks;
            self.sel = snap.sel;
            self.block_sel = None;
            self.marked = None;
            self.last_edit = None;
            self.close_popups();
            self.after_edit(cx);
        }
    }

    pub(crate) fn redo(&mut self, _: &Redo, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(snap) = self.redo.pop() {
            self.undo.push(Snapshot {
                blocks: self.blocks.clone(),
                sel: self.sel.clone(),
            });
            self.blocks = snap.blocks;
            self.sel = snap.sel;
            self.block_sel = None;
            self.marked = None;
            self.last_edit = None;
            self.close_popups();
            self.after_edit(cx);
        }
    }

    pub(crate) fn move_block_up(&mut self, _: &MoveBlockUp, _: &mut Window, cx: &mut Context<Self>) {
        self.move_block(true, cx);
    }

    pub(crate) fn move_block_down(&mut self, _: &MoveBlockDown, _: &mut Window, cx: &mut Context<Self>) {
        self.move_block(false, cx);
    }

    pub(crate) fn duplicate(&mut self, _: &DuplicateBlock, _: &mut Window, cx: &mut Context<Self>) {
        self.duplicate_block(self.sel.block, cx);
    }

    // ----------------------------------------------------------------------
    // mouse
    // ----------------------------------------------------------------------

    pub(crate) fn mouse_down_on_block(
        &mut self,
        ix: usize,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.stop_propagation();
        self.goal_x = None;
        if ix >= self.blocks.len() {
            return;
        }
        // A press in the margin beside a block starts a selection rectangle.
        let row = self.row_bounds.borrow().get(&self.blocks[ix].id).copied();
        if let Some(row) = row {
            let x = event.position.x;
            if !event.modifiers.shift
                && (x < self.content_left(ix, &row) - px(2.) || x > row.right() - px(GUTTER_PX) + px(2.))
            {
                self.begin_marquee(event.position, false, window, cx);
                return;
            }
        }
        window.focus(&self.focus);
        self.close_popups();
        if !self.blocks[ix].kind.has_text() {
            self.block_sel = Some((ix, ix));
            cx.notify();
            return;
        }
        let offset = self
            .layout_of(ix)
            .map(|l| l.index_for_point(event.position))
            .unwrap_or(0);
        if event.modifiers.shift {
            if ix == self.sel.block && self.block_sel.is_none() {
                self.select_to(offset, cx);
            } else {
                let anchor = self.block_sel.map(|(a, _)| a).unwrap_or(self.sel.block);
                self.block_sel = Some((anchor, ix));
                cx.notify();
            }
            return;
        }
        // cmd/ctrl-click opens links
        if event.modifiers.secondary() {
            if let Some(url) = self.blocks[ix]
                .text
                .marks_of_char(offset)
                .and_then(|m| m.link.clone())
            {
                cx.open_url(&url);
                return;
            }
        }
        self.set_caret(ix, offset, cx);
        self.scroll_pending = false;
        match event.click_count {
            2 => {
                let r = word_range_at(&self.blocks[ix].text.text, offset);
                self.sel.range = r;
            }
            n if n >= 3 => {
                self.sel.range = 0..self.blocks[ix].text.len();
            }
            _ => {
                self.selecting = true;
                self.drag_anchor = Some((ix, offset));
            }
        }
        cx.notify();
    }

    /// Pointer moved anywhere in the window while a text selection or a
    /// selection rectangle is in progress.
    pub(crate) fn on_mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if !self.selecting && self.marquee.is_none() {
            return;
        }
        if event.pressed_button != Some(MouseButton::Left) {
            self.selecting = false;
            self.marquee = None;
            cx.notify();
            return;
        }
        self.last_mouse = Some(event.position);
        if self.marquee.is_some() {
            self.update_marquee(event.position, cx);
        } else {
            self.extend_selection(event.position, cx);
        }
        self.ensure_autoscroll(cx);
    }

    /// The visible block whose row is at height `y` (or the nearest one).
    fn row_at_y(&self, y: Pixels) -> Option<usize> {
        let rows = self.visible_rows();
        rows.iter().find(|(_, b)| y <= b.bottom()).or(rows.last()).map(|(i, _)| *i)
    }

    fn extend_selection(&mut self, pos: Point<Pixels>, cx: &mut Context<Self>) {
        let Some((anchor_block, anchor_offset)) = self.drag_anchor else {
            return;
        };
        let anchor_block = anchor_block.min(self.blocks.len() - 1);
        let Some(target) = self.row_at_y(pos.y) else {
            return;
        };
        let offset = self.layout_of(anchor_block).map(|l| l.index_for_point(pos));
        if target == anchor_block {
            self.block_sel = None;
            if let Some(offset) = offset {
                self.sel.block = anchor_block;
                let (s, e) = (anchor_offset.min(offset), anchor_offset.max(offset));
                self.sel.range = s..e;
                self.sel.reversed = offset < anchor_offset;
            }
        } else {
            self.block_sel = Some((anchor_block, target));
        }
        cx.notify();
    }

    pub(crate) fn on_mouse_up(&mut self, _: &MouseUpEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.selecting {
            self.selecting = false;
            cx.notify();
        }
        if self.marquee.is_some() {
            self.end_marquee(window, cx);
        }
    }

    /// Whether the pointer is currently being tracked outside normal hover.
    pub(crate) fn tracking_pointer(&self) -> bool {
        self.selecting || self.marquee.is_some()
    }

    /// Click in the empty space under the last block.
    pub(crate) fn click_below(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus);
        self.close_popups();
        let last = self.blocks.len() - 1;
        let b = &self.blocks[last];
        if b.kind == BlockKind::Paragraph && b.text.is_empty() {
            self.set_caret(last, 0, cx);
        } else {
            self.checkpoint(EditKind::Other);
            self.blocks.push(Block::paragraph(""));
            self.sel = Sel::caret(last + 1, 0);
            self.block_sel = None;
            self.after_edit(cx);
        }
    }
}

// --------------------------------------------------------------------------
// IME / text input
// --------------------------------------------------------------------------

/// Plain text from other apps: every line becomes its own block (as in
/// Notion), except inside code fences and tables.
fn lines_as_blocks(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 16);
    let mut in_fence = false;
    let mut prev_content = false;
    for line in text.split('\n') {
        let t = line.trim_start();
        let fence = t.starts_with("```");
        let table = t.starts_with('|');
        if !in_fence && prev_content && !t.is_empty() && !table && !fence {
            out.push('\n');
        }
        out.push_str(line);
        out.push('\n');
        if fence {
            in_fence = !in_fence;
        }
        prev_content = !in_fence && !t.is_empty() && !table && !fence;
    }
    out
}

impl EntityInputHandler for Editor {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let text = self.text();
        let range = range_from_utf16(text, &range_utf16);
        actual_range.replace(range_to_utf16(text, &range));
        Some(text[range].to_string())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: range_to_utf16(self.text(), &self.sel.range),
            reversed: self.sel.reversed,
        })
    }

    fn marked_text_range(&self, _window: &mut Window, _cx: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked.as_ref().map(|r| range_to_utf16(self.text(), r))
    }

    fn unmark_text(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        self.marked = None;
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.block_sel.is_some() {
            self.delete_block_selection(cx);
        }
        if !self.cur().kind.has_text() {
            return;
        }
        let range = range_utf16
            .as_ref()
            .map(|r| range_from_utf16(self.text(), r))
            .or(self.marked.clone())
            .unwrap_or(self.sel.range.clone());
        self.insert_text(range, new_text, cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.block_sel.is_some() {
            self.delete_block_selection(cx);
        }
        if !self.cur().kind.has_text() {
            return;
        }
        let range = range_utf16
            .as_ref()
            .map(|r| range_from_utf16(self.text(), r))
            .or(self.marked.clone())
            .unwrap_or(self.sel.range.clone());
        self.checkpoint(EditKind::Typing);
        let ix = self.sel.block;
        let marks = if self.blocks[ix].kind.is_raw() {
            Some(Marks::default())
        } else if range.is_empty() {
            self.pending_marks.clone()
        } else {
            None
        };
        self.blocks[ix].text.replace(range.clone(), new_text, marks);
        self.marked = if new_text.is_empty() {
            None
        } else {
            Some(range.start..range.start + new_text.len())
        };
        self.sel.range = new_selected_range_utf16
            .as_ref()
            .map(|r| range_from_utf16(new_text, r))
            .map(|r| range.start + r.start..range.start + r.end)
            .unwrap_or_else(|| {
                let at = range.start + new_text.len();
                at..at
            });
        self.sel.reversed = false;
        self.scroll_pending = true;
        cx.emit(EditorEvent::Changed);
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        _bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let layout = self.layout_of(self.sel.block)?;
        let range = range_from_utf16(self.text(), &range_utf16);
        Some(layout.caret_bounds(range.start))
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        let layout = self.layout_of(self.sel.block)?;
        Some(offset_to_utf16(self.text(), layout.index_for_point(point)))
    }
}
