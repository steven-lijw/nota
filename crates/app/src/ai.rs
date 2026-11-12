//! The AI side panel. It talks to the locally installed Claude Code CLI
//! (`claude -p`) with the vault as its working directory, and streams the
//! answer back as rendered blocks.

use crate::blocks_view::static_blocks;
use crate::editor::{AiSelection, AiTarget};
use crate::store::Store;
use crate::text_input::{InputEvent, TextInput};
use crate::theme::{theme, Theme};
use crate::ui::*;
use gpui::{
    div, prelude::*, px, AnyElement, ClipboardItem, Context, Corner, Entity, EventEmitter, FontWeight, Pixels,
    Point, ScrollHandle, SharedString, Subscription, Task, Window,
};
use nota_core::markdown::{from_markdown, to_markdown};
use nota_core::query::prop_text;
use nota_core::Block;
use smol::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use smol::stream::StreamExt;
use std::path::PathBuf;
use std::process::Stdio;

pub enum AiEvent {
    /// Insert this Markdown at the end of the open page.
    Insert(String),
    /// Insert below the part of the page the question was about.
    InsertBelow(String, AiTarget),
    /// Replace the part of the page the question was about.
    Replace(String, AiTarget),
    Close,
}

struct Msg {
    user: bool,
    text: String,
    blocks: Vec<Block>,
    error: bool,
    /// The selection this answer is about (answers only).
    target: Option<AiTarget>,
    /// Shown above a question that was sent with a selection.
    quote: Option<String>,
}

impl Msg {
    fn new(user: bool, text: impl Into<String>) -> Self {
        let text = text.into();
        let blocks = if user { vec![] } else { from_markdown(&text) };
        Msg { user, text, blocks, error: false, target: None, quote: None }
    }
}

/// The dropdowns under the prompt.
#[derive(Clone, Copy, PartialEq)]
enum ComposerMenu {
    Access,
    Model,
    Effort,
}

const MODELS: [(Option<&str>, &str); 5] = [
    (None, "Default"),
    (Some("fable"), "Fable"),
    (Some("opus"), "Opus"),
    (Some("sonnet"), "Sonnet"),
    (Some("haiku"), "Haiku"),
];

const EFFORTS: [(Option<&str>, &str); 6] = [
    (None, "Default"),
    (Some("low"), "Low"),
    (Some("medium"), "Medium"),
    (Some("high"), "High"),
    (Some("xhigh"), "Extra high"),
    (Some("max"), "Max"),
];

fn label_of(options: &[(Option<&str>, &'static str)], value: Option<&str>) -> String {
    options
        .iter()
        .find(|(v, _)| *v == value)
        .map(|(_, l)| l.to_string())
        .unwrap_or_else(|| value.unwrap_or("Default").to_string())
}

#[derive(Clone)]
struct ClaudeEnv {
    bin: PathBuf,
    path: String,
}

/// Find the `claude` executable. Apps launched from Finder don't inherit the
/// shell's PATH, so ask the login shell and also try the usual install spots.
fn resolve_claude() -> Result<ClaudeEnv, String> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    #[cfg(unix)]
    {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into());
        if let Ok(mut child) = std::process::Command::new(&shell)
            .args(["-lic", "printf '__NOTA_PATH__%s__END__' \"$PATH\""])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
        {
            let started = std::time::Instant::now();
            loop {
                match child.try_wait() {
                    Ok(Some(_)) => break,
                    Ok(None) if started.elapsed().as_secs() < 6 => {
                        std::thread::sleep(std::time::Duration::from_millis(40))
                    }
                    _ => {
                        let _ = child.kill();
                        break;
                    }
                }
            }
            if let Ok(out) = child.wait_with_output() {
                let s = String::from_utf8_lossy(&out.stdout);
                if let Some(p) = s.split("__NOTA_PATH__").nth(1).and_then(|r| r.split("__END__").next()) {
                    dirs.extend(std::env::split_paths(p));
                }
            }
        }
    }
    if let Some(p) = std::env::var_os("PATH") {
        dirs.extend(std::env::split_paths(&p));
    }
    if let Some(home) = dirs::home_dir() {
        for d in [".claude/local", ".local/bin", ".homebrew/bin", ".bun/bin", ".npm-global/bin", ".volta/bin"] {
            dirs.push(home.join(d));
        }
    }
    dirs.push("/opt/homebrew/bin".into());
    dirs.push("/usr/local/bin".into());
    let mut seen = std::collections::HashSet::new();
    dirs.retain(|d| !d.as_os_str().is_empty() && seen.insert(d.clone()));
    let path = std::env::join_paths(&dirs).map(|p| p.to_string_lossy().to_string()).unwrap_or_default();

    if let Some(custom) = std::env::var_os("NOTA_CLAUDE") {
        return Ok(ClaudeEnv { bin: custom.into(), path });
    }
    let names: &[&str] = if cfg!(windows) { &["claude.exe", "claude.cmd"] } else { &["claude"] };
    for d in &dirs {
        for n in names {
            let candidate = d.join(n);
            if candidate.is_file() {
                return Ok(ClaudeEnv { bin: candidate, path });
            }
        }
    }
    Err("Could not find the `claude` command. Install Claude Code (https://claude.com/claude-code) and make sure \
         `claude` works in your terminal, or set the NOTA_CLAUDE environment variable to its full path."
        .into())
}

const SYSTEM: &str = "You are the built-in AI assistant of Nota, a local Notion-style notes app. \
The working directory is the user's vault: every page is one Markdown file with YAML front matter \
(id, title, icon, parent, kind, props, schema, views). Database pages have `kind: database`; their rows are \
pages whose `parent` is the database id and whose `props` hold the property values. Use Read, Grep and Glob \
to look things up in the vault when the question is about the user's notes. Reply in the user's language, \
in concise Markdown (headings, lists, `- [ ]` to-dos, fenced code). The reply may be inserted into a page as \
blocks, so never wrap the whole reply in a code fence and do not use HTML. When the message includes a \
<selection> and asks to change it (rewrite, translate, shorten, fix, continue…), reply with only the new \
version of that content, with no preamble, so it can replace the selection. The <access> tag in each \
message says whether you may edit files; it can change between messages.";

const ACCESS_READONLY: &str = "<access>read-only: you cannot modify files now. If the user wants content added \
to a page, write that content as your reply; they can insert it with one click, or switch Nota AI to \
\"Can edit\".</access>";

const ACCESS_EDIT: &str = "<access>can edit: you may create and edit files in the vault. Rules: never change an \
existing `id`; keep the front matter valid YAML; to create a page, write a new .md file with a fresh \
32-character lowercase hex `id`, a `title`, optional `parent` (the id of the parent page or database) and \
ISO-8601 `created`/`updated`; body indentation for nested blocks is 4 spaces. After editing, briefly say what \
you changed.</access>";

pub struct AiPanel {
    store: Entity<Store>,
    input: Entity<TextInput>,
    messages: Vec<Msg>,
    session: Option<String>,
    running: Option<Task<()>>,
    status: String,
    context_page: Option<String>,
    use_context: bool,
    /// What is selected in the open page right now.
    selection: Option<AiSelection>,
    use_selection: bool,
    menu: Option<(ComposerMenu, Point<Pixels>)>,
    got_delta: bool,
    edited: bool,
    env: Option<ClaudeEnv>,
    scroll: ScrollHandle,
    _subs: Vec<Subscription>,
}

impl EventEmitter<AiEvent> for AiPanel {}

impl AiPanel {
    pub fn new(store: Entity<Store>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| {
            let mut i = TextInput::new(window, cx);
            i.placeholder = "Ask AI anything…".into();
            i.multiline = true;
            i.font_size = px(14.);
            i.line_height = px(21.);
            i
        });
        let subs = vec![
            cx.subscribe_in(&input, window, |this: &mut Self, _, ev: &InputEvent, window, cx| match ev {
                InputEvent::Submit => this.send(None, window, cx),
                InputEvent::Cancel => cx.emit(AiEvent::Close),
                _ => {}
            }),
            cx.observe(&store, |_, _, cx| cx.notify()),
        ];
        AiPanel {
            store,
            input,
            messages: Vec::new(),
            session: None,
            running: None,
            status: String::new(),
            context_page: None,
            use_context: true,
            selection: None,
            use_selection: true,
            menu: None,
            got_delta: false,
            edited: false,
            env: None,
            scroll: ScrollHandle::new(),
            _subs: subs,
        }
    }

    pub fn set_context(&mut self, page: Option<String>, cx: &mut Context<Self>) {
        if self.context_page != page {
            self.selection = None;
        }
        self.context_page = page;
        cx.notify();
    }

    /// The editor's selection changed.
    pub fn set_selection(&mut self, selection: Option<AiSelection>, cx: &mut Context<Self>) {
        if self.selection != selection {
            // a new selection is attached again even if the last one was dismissed
            if selection.as_ref().map(|s| &s.target) != self.selection.as_ref().map(|s| &s.target) {
                self.use_selection = true;
            }
            self.selection = selection;
            cx.notify();
        }
    }

    fn active_selection(&self) -> Option<&AiSelection> {
        self.selection.as_ref().filter(|_| self.use_selection)
    }

    pub fn focus_input(&self, window: &mut Window, cx: &gpui::App) {
        self.input.read(cx).focus(window);
    }

    fn new_chat(&mut self, cx: &mut Context<Self>) {
        self.running = None;
        self.messages.clear();
        self.session = None;
        self.status.clear();
        cx.notify();
    }

    fn stop(&mut self, cx: &mut Context<Self>) {
        if self.running.take().is_some() {
            self.status.clear();
            if let Some(m) = self.messages.last_mut().filter(|m| !m.user) {
                if m.text.is_empty() {
                    m.text = "Stopped.".into();
                    m.blocks = from_markdown(&m.text);
                }
            }
            self.finish(cx);
        }
    }

    fn finish(&mut self, cx: &mut Context<Self>) {
        self.running = None;
        self.status.clear();
        if self.edited {
            self.edited = false;
            self.store.update(cx, |s, cx| s.reload(cx));
        }
        cx.notify();
    }

    /// Describe the open page for the model.
    fn context_text(&self, cx: &Context<Self>) -> String {
        let Some(id) = self.context_page.as_ref().filter(|_| self.use_context) else {
            return String::new();
        };
        let s = self.store.read(cx);
        let Some(page) = s.page(id) else {
            return String::new();
        };
        let file = page.file.as_ref().map(|f| f.to_string_lossy().to_string()).unwrap_or_default();
        let mut body = String::new();
        if let Some(db) = &page.database {
            body.push_str("This page is a database. Properties: ");
            body.push_str(
                &db.schema.iter().map(|p| format!("{} ({})", p.name, p.kind.label())).collect::<Vec<_>>().join(", "),
            );
            body.push_str("\nRows:\n");
            for row in s.vault.rows(id).into_iter().take(200) {
                let cells: Vec<String> = db
                    .schema
                    .iter()
                    .map(|p| (p, prop_text(row, p)))
                    .filter(|(_, v)| !v.is_empty())
                    .map(|(p, v)| format!("{}: {}", p.name, v))
                    .collect();
                body.push_str(&format!("- {}\n", cells.join(" | ")));
            }
        } else {
            if let Some(dbp) = s.vault.parent_database(id) {
                if let Some(db) = &dbp.database {
                    for p in &db.schema {
                        let v = prop_text(page, p);
                        if !v.is_empty() && p.kind != nota_core::PropKind::Title {
                            body.push_str(&format!("{}: {}\n", p.name, v));
                        }
                    }
                    body.push('\n');
                }
            }
            body.push_str(&to_markdown(&page.blocks));
        }
        if body.len() > 60_000 {
            let mut cut = 60_000;
            while !body.is_char_boundary(cut) {
                cut -= 1;
            }
            body.truncate(cut);
            body.push_str("\n…(truncated)");
        }
        let mut out = format!(
            "<current_page title=\"{}\" file=\"{}\">\n{}\n</current_page>\n\n",
            page.display_title().replace('"', "'"),
            file,
            body
        );
        if let Some(sel) = self.active_selection() {
            out.push_str(&format!(
                "<selection>\nThe user selected this part of the page:\n{}\n</selection>\n\n",
                sel.markdown.trim_end()
            ));
        }
        out
    }

    fn send(&mut self, preset: Option<&str>, window: &mut Window, cx: &mut Context<Self>) {
        if self.running.is_some() {
            return;
        }
        let text = match preset {
            Some(p) => p.to_string(),
            None => self.input.read(cx).text().trim().to_string(),
        };
        if text.is_empty() {
            return;
        }
        if preset.is_none() {
            self.input.update(cx, |i, cx| i.set_text("", cx));
        }
        self.input.read(cx).focus(window);
        self.store.update(cx, |s, _| s.flush());
        let allow_edits = self.store.read(cx).state.ai_allow_edits;
        let access = if allow_edits { ACCESS_EDIT } else { ACCESS_READONLY };
        let prompt = format!("{}\n\n{}{}", access, self.context_text(cx), text);
        let sel = self.active_selection().cloned();
        let mut question = Msg::new(true, text);
        question.quote = sel.as_ref().map(|s| s.preview.clone());
        self.messages.push(question);
        let mut answer = Msg::new(false, "");
        answer.target = sel.map(|s| s.target);
        self.messages.push(answer);
        self.menu = None;
        self.status = "Thinking…".into();
        self.got_delta = false;
        self.edited = false;
        self.scroll.scroll_to_bottom();

        let (model, effort) = {
            let st = &self.store.read(cx).state;
            (st.ai_model.clone(), st.ai_effort.clone())
        };
        let root = self.store.read(cx).vault.root.clone();
        let session = self.session.clone();
        let known_env = self.env.clone();
        let mut args: Vec<String> =
            ["-p", "--output-format", "stream-json", "--verbose", "--include-partial-messages"].map(String::from).into();
        args.push("--append-system-prompt".into());
        args.push(SYSTEM.to_string());
        if let Some(m) = model {
            args.extend(["--model".to_string(), m]);
        }
        if let Some(e) = effort {
            args.extend(["--effort".to_string(), e]);
        }
        if let Some(s) = session {
            args.push("--resume".into());
            args.push(s);
        }
        if allow_edits {
            args.extend(["--permission-mode", "acceptEdits"].map(String::from));
        } else {
            args.extend(["--allowedTools", "Read,Glob,Grep"].map(String::from));
            args.extend(["--disallowedTools", "Write,Edit,MultiEdit,NotebookEdit,Bash"].map(String::from));
        }

        self.running = Some(cx.spawn(async move |this, cx| {
            let env = match known_env {
                Some(e) => Ok(e),
                None => cx.background_executor().spawn(async { resolve_claude() }).await,
            };
            let env = match env {
                Ok(e) => e,
                Err(msg) => {
                    this.update(cx, |this, cx| this.fail(msg, cx)).ok();
                    return;
                }
            };
            this.update(cx, |this, _| this.env = Some(env.clone())).ok();
            let mut cmd = smol::process::Command::new(&env.bin);
            cmd.args(&args)
                .current_dir(&root)
                .env("PATH", &env.path)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .kill_on_drop(true);
            let mut child = match cmd.spawn() {
                Ok(c) => c,
                Err(err) => {
                    let msg = format!("Could not start `{}`: {err}", env.bin.display());
                    this.update(cx, |this, cx| this.fail(msg, cx)).ok();
                    return;
                }
            };
            if let Some(mut stdin) = child.stdin.take() {
                let _ = stdin.write_all(prompt.as_bytes()).await;
                let _ = stdin.close().await;
            }
            let stderr = child.stderr.take();
            let stderr_task = cx.background_executor().spawn(async move {
                let mut buf = String::new();
                if let Some(mut e) = stderr {
                    let _ = e.read_to_string(&mut buf).await;
                }
                buf
            });
            if let Some(stdout) = child.stdout.take() {
                let mut lines = BufReader::new(stdout).lines();
                while let Some(Ok(line)) = lines.next().await {
                    if this.update(cx, |this, cx| this.on_line(&line, cx)).is_err() {
                        return;
                    }
                }
            }
            let status = child.status().await;
            let stderr = stderr_task.await;
            this.update(cx, |this, cx| {
                let empty = this.messages.last().is_some_and(|m| !m.user && m.text.trim().is_empty());
                if empty {
                    let detail = stderr.trim();
                    let msg = if !detail.is_empty() {
                        let mut m = detail.lines().rev().take(6).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n");
                        if detail.contains("unknown option") || detail.contains("Unknown option") {
                            m.push_str("\n\nYour Claude Code is too old for the selected model or effort. Run `claude update`, or set them back to Default.");
                        }
                        m
                    } else {
                        match status {
                            Ok(s) if s.success() => "Claude returned an empty answer.".to_string(),
                            Ok(s) => format!("`claude` exited with {s}. Run `claude` once in a terminal to check that you are logged in."),
                            Err(e) => format!("`claude` failed: {e}"),
                        }
                    };
                    this.fail(msg, cx);
                } else {
                    this.finish(cx);
                }
            })
            .ok();
        }));
        cx.notify();
    }

    fn fail(&mut self, msg: String, cx: &mut Context<Self>) {
        if let Some(m) = self.messages.last_mut().filter(|m| !m.user) {
            m.text = msg;
            m.blocks = vec![Block::paragraph(m.text.clone())];
            m.error = true;
        }
        self.finish(cx);
    }

    fn append(&mut self, text: &str) {
        if let Some(m) = self.messages.last_mut().filter(|m| !m.user) {
            m.text.push_str(text);
            m.blocks = from_markdown(&m.text);
        }
        self.status.clear();
        self.scroll.scroll_to_bottom();
    }

    fn tool_status(&mut self, name: &str, input: &serde_json::Value) {
        let file = input["file_path"]
            .as_str()
            .or(input["path"].as_str())
            .and_then(|p| std::path::Path::new(p).file_stem())
            .map(|s| s.to_string_lossy().to_string());
        // file names look like "<Title> <id8>": drop the id for display
        let file = file.map(|f| match f.rsplit_once(' ') {
            Some((title, id)) if id.len() == 8 && id.chars().all(|c| c.is_ascii_hexdigit()) => title.to_string(),
            _ => f,
        });
        self.status = match name {
            "Read" => format!("Reading {}…", file.unwrap_or_else(|| "a page".into())),
            "Grep" => match input["pattern"].as_str() {
                Some(p) => format!("Searching for “{p}”…"),
                None => "Searching your notes…".into(),
            },
            "Glob" => "Looking through the vault…".into(),
            "Write" | "Edit" | "MultiEdit" => {
                self.edited = true;
                format!("Editing {}…", file.unwrap_or_else(|| "a page".into()))
            }
            other => format!("Using {other}…"),
        };
    }

    fn on_line(&mut self, line: &str, cx: &mut Context<Self>) {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            return;
        };
        if let Some(s) = v["session_id"].as_str() {
            self.session = Some(s.to_string());
        }
        match v["type"].as_str() {
            Some("stream_event") => {
                let ev = &v["event"];
                match ev["type"].as_str() {
                    Some("content_block_delta") => match ev["delta"]["type"].as_str() {
                        Some("text_delta") => {
                            if let Some(text) = ev["delta"]["text"].as_str() {
                                self.got_delta = true;
                                self.append(text);
                            }
                        }
                        Some("thinking_delta") => self.status = "Thinking…".into(),
                        _ => {}
                    },
                    Some("content_block_start") => {
                        let block = &ev["content_block"];
                        match block["type"].as_str() {
                            Some("tool_use") => {
                                if let Some(name) = block["name"].as_str() {
                                    self.tool_status(name, &block["input"]);
                                }
                            }
                            Some("text") => {
                                let needs_gap = self
                                    .messages
                                    .last()
                                    .is_some_and(|m| !m.user && !m.text.is_empty() && !m.text.ends_with("\n\n"));
                                if needs_gap {
                                    self.append("\n\n");
                                }
                            }
                            _ => {}
                        }
                    }
                    _ => {}
                }
            }
            Some("assistant") => {
                if let Some(content) = v["message"]["content"].as_array() {
                    for block in content {
                        match block["type"].as_str() {
                            Some("text") if !self.got_delta => {
                                if let Some(text) = block["text"].as_str() {
                                    let dup = self.messages.last().is_some_and(|m| m.text.ends_with(text));
                                    if !dup {
                                        let gap = self.messages.last().is_some_and(|m| !m.text.is_empty());
                                        if gap {
                                            self.append("\n\n");
                                        }
                                        self.append(text);
                                    }
                                }
                            }
                            Some("tool_use") => {
                                if let Some(name) = block["name"].as_str() {
                                    self.tool_status(name, &block["input"]);
                                }
                            }
                            _ => {}
                        }
                    }
                }
                self.got_delta = false;
            }
            Some("result") => {
                let empty = self.messages.last().is_some_and(|m| m.text.trim().is_empty());
                if let Some(text) = v["result"].as_str().filter(|t| !t.is_empty()) {
                    if v["is_error"].as_bool() == Some(true) {
                        if let Some(m) = self.messages.last_mut().filter(|m| !m.user) {
                            m.text = text.to_string();
                            m.blocks = vec![Block::paragraph(text.to_string())];
                            m.error = true;
                        }
                    } else if empty {
                        self.append(text);
                    }
                }
            }
            _ => {}
        }
        cx.notify();
    }

    // --- rendering -----------------------------------------------------------

    fn render_message(&self, ix: usize, m: &Msg, last: bool, t: &Theme, cx: &Context<Self>) -> AnyElement {
        if m.user {
            let mut col = div().flex().flex_col().items_end().gap(px(4.));
            if let Some(q) = &m.quote {
                col = col.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(4.))
                        .max_w(px(300.))
                        .text_size(px(12.))
                        .text_color(t.text_faint)
                        .child(icon("quote", 12., t.text_faint))
                        .child(div().whitespace_nowrap().overflow_hidden().text_ellipsis().child(SharedString::from(q.clone()))),
                );
            }
            return col
                .child(
                    div()
                        .max_w(px(300.))
                        .px(px(14.))
                        .py(px(8.))
                        .rounded(px(16.))
                        .bg(t.user_bubble)
                        .text_size(px(14.))
                        .line_height(px(21.))
                        .child(SharedString::from(m.text.clone())),
                )
                .into_any_element();
        }
        let streaming = last && self.running.is_some();
        let mut col = div().flex().flex_col().gap(px(6.));
        if !m.text.is_empty() {
            let body = static_blocks(&m.blocks, t, 14.);
            col = col.child(if m.error { body.text_color(t.danger) } else { body });
        }
        if streaming {
            let label = if self.status.is_empty() { "Writing…".to_string() } else { self.status.clone() };
            col = col.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .h(px(24.))
                    .text_size(px(13.))
                    .text_color(t.text_faint)
                    .child(icon("sparkle", 13., t.text_faint))
                    .child(SharedString::from(label)),
            );
        } else if !m.error && !m.text.is_empty() {
            let (a, b) = (m.text.clone(), m.text.clone());
            let can_insert = self.page_has_editor(cx);
            let small = |id: (&'static str, usize), name: &'static str, label: &'static str| {
                let hover = t.hover;
                div()
                    .id(id)
                    .flex()
                    .items_center()
                    .gap(px(5.))
                    .h(px(24.))
                    .px(px(6.))
                    .rounded(px(5.))
                    .text_size(px(12.))
                    .text_color(t.text_muted)
                    .cursor_pointer()
                    .hover(move |s| s.bg(hover))
                    .child(icon(name, 13., t.text_muted))
                    .child(label)
            };
            let mut actions = div().flex().flex_wrap().items_center().gap(px(2.)).ml(px(-6.));
            match (&m.target, can_insert) {
                (Some(target), true) => {
                    let (r_text, r_target) = (m.text.clone(), target.clone());
                    let (b_text, b_target) = (m.text.clone(), target.clone());
                    actions = actions
                        .child(small(("ai-replace", ix), "check", "Replace selection").on_click(cx.listener(
                            move |_, _, _, cx| cx.emit(AiEvent::Replace(r_text.clone(), r_target.clone())),
                        )))
                        .child(small(("ai-below", ix), "insert", "Insert below").on_click(cx.listener(
                            move |_, _, _, cx| cx.emit(AiEvent::InsertBelow(b_text.clone(), b_target.clone())),
                        )));
                }
                (None, true) => {
                    actions = actions.child(small(("ai-insert", ix), "insert", "Insert into page").on_click(
                        cx.listener(move |_, _, _, cx| cx.emit(AiEvent::Insert(a.clone()))),
                    ));
                }
                _ => {}
            }
            col = col.child(actions.child(small(("ai-copy", ix), "copy", "Copy").on_click(cx.listener(
                move |_, _, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(b.clone())),
            ))));
        }
        col.into_any_element()
    }

    fn render_empty(&self, t: &Theme, cx: &Context<Self>) -> AnyElement {
        let suggestions: [(&str, &str, &str); 4] = [
            ("text", "Summarize this page", "Summarize this page in a few bullet points."),
            ("checkbox", "Find action items", "List the action items on this page as a to-do list."),
            ("edit", "Continue writing", "Continue writing this page in the same style and language. Reply with only the new content."),
            ("search", "Ask about my notes", "Give me a short overview of what is in my vault, grouped by topic."),
        ];
        let mut col = div()
            .flex()
            .flex_col()
            .gap(px(4.))
            .pt(px(56.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .size(px(44.))
                    .mb(px(10.))
                    .rounded(px(22.))
                    .bg(t.hover)
                    .child(icon("sparkles", 24., t.text)),
            )
            .child(
                div()
                    .text_size(px(22.))
                    .line_height(px(28.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .pb(px(14.))
                    .child("How can I help you today?"),
            );
        for (i, (icon_name, label, prompt)) in suggestions.into_iter().enumerate() {
            let hover = t.hover;
            col = col.child(
                div()
                    .id(("ai-suggest", i))
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .h(px(34.))
                    .px(px(8.))
                    .ml(px(-8.))
                    .rounded(px(8.))
                    .text_size(px(14.))
                    .cursor_pointer()
                    .hover(move |s| s.bg(hover))
                    .on_click(cx.listener(move |this, _, window, cx| this.send(Some(prompt), window, cx)))
                    .child(icon(icon_name, 16., t.text_muted))
                    .child(label),
            );
        }
        col.into_any_element()
    }

    /// Whether answers can be put into the open page (databases have no body).
    fn page_has_editor(&self, cx: &Context<Self>) -> bool {
        let s = self.store.read(cx);
        self.context_page
            .as_ref()
            .and_then(|id| s.page(id))
            .is_some_and(|p| !p.is_database() && p.load_error.is_none())
    }

    fn render_menu(&self, t: &Theme, cx: &Context<Self>) -> Option<AnyElement> {
        let (menu, pos) = self.menu?;
        let st = &self.store.read(cx).state;
        let check = |on: bool| on.then(|| icon("check", 14., t.text_muted));
        let mut surface = menu_surface(t).w(px(230.));
        match menu {
            ComposerMenu::Access => {
                let allow = st.ai_allow_edits;
                for (i, (on, label, hint)) in [
                    (false, "Read-only", "Can read your notes, can't change them"),
                    (true, "Can edit", "May create and edit pages"),
                ]
                .into_iter()
                .enumerate()
                {
                    let mut item = menu_item(("ai-acc", i), Some(if on { "edit" } else { "eye" }), label, t)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.menu = None;
                            this.store.update(cx, |s, cx| {
                                s.state.ai_allow_edits = on;
                                s.save_state();
                                cx.notify();
                            })
                        }));
                    if let Some(c) = check(allow == on) {
                        item = item.child(c);
                    }
                    surface = surface.child(item).child(
                        div().px(px(38.)).pb(px(4.)).mt(px(-4.)).text_size(px(12.)).text_color(t.text_faint).child(hint),
                    );
                }
            }
            ComposerMenu::Model | ComposerMenu::Effort => {
                let (options, current): (&[(Option<&str>, &str)], Option<String>) = if menu == ComposerMenu::Model {
                    (&MODELS, st.ai_model.clone())
                } else {
                    (&EFFORTS, st.ai_effort.clone())
                };
                surface = surface.child(menu_label(if menu == ComposerMenu::Model { "Model" } else { "Thinking effort" }, t));
                for (i, (value, label)) in options.iter().enumerate() {
                    let v = value.map(String::from);
                    let mut item = menu_item(("ai-opt", i), None, *label, t).on_click(cx.listener(move |this, _, _, cx| {
                        this.menu = None;
                        let v = v.clone();
                        this.store.update(cx, |s, cx| {
                            if menu == ComposerMenu::Model {
                                s.state.ai_model = v;
                            } else {
                                s.state.ai_effort = v;
                            }
                            s.save_state();
                            cx.notify();
                        })
                    }));
                    if let Some(c) = check(current.as_deref() == *value) {
                        item = item.child(c);
                    }
                    surface = surface.child(item);
                }
            }
        }
        Some(popover(
            pos,
            Corner::BottomLeft,
            surface,
            cx.listener(|this, _, _, cx| {
                this.menu = None;
                cx.notify();
            }),
        ))
    }

    fn render_composer(&self, t: &Theme, cx: &Context<Self>) -> AnyElement {
        let s = self.store.read(cx);
        let allow = s.state.ai_allow_edits;
        let running = self.running.is_some();
        let hover = t.hover;
        let mut chips = div().flex().items_center().gap(px(6.)).pb(px(8.)).min_w_0();
        if let Some(page) = self.context_page.as_ref().and_then(|id| s.page(id)) {
            let on = self.use_context;
            chips = chips.child(
                div()
                    .id("ai-context")
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .h(px(24.))
                    .max_w(px(240.))
                    .pl(px(4.))
                    .pr(px(8.))
                    .rounded(px(6.))
                    .border_1()
                    .border_color(t.border_strong)
                    .text_size(px(12.))
                    .text_color(if on { t.text_muted } else { t.text_faint })
                    .cursor_pointer()
                    .hover(move |s| s.bg(hover))
                    .opacity(if on { 1.0 } else { 0.55 })
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.use_context = !this.use_context;
                        cx.notify();
                    }))
                    .child(page_icon(page.icon.as_deref(), page.is_database(), 12., t))
                    .child(
                        div()
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(SharedString::from(page.display_title().to_string())),
                    )
                    .child(icon(if on { "check" } else { "x" }, 11., t.text_faint)),
            );
        }
        if let Some(sel) = &self.selection {
            let on = self.use_selection;
            chips = chips.child(
                div()
                    .id("ai-selection")
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .items_center()
                    .gap(px(4.))
                    .h(px(24.))
                    .pl(px(6.))
                    .pr(px(8.))
                    .rounded(px(6.))
                    .border_1()
                    .border_color(if on { t.accent } else { t.border_strong })
                    .bg(if on { t.accent_bg } else { gpui::transparent_black() })
                    .text_size(px(12.))
                    .text_color(if on { t.text_muted } else { t.text_faint })
                    .cursor_pointer()
                    .hover(move |s| s.bg(hover))
                    .opacity(if on { 1.0 } else { 0.55 })
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.use_selection = !this.use_selection;
                        cx.notify();
                    }))
                    .child(icon("quote", 12., t.text_faint))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(SharedString::from(sel.preview.clone())),
                    )
                    .child(icon(if on { "check" } else { "x" }, 11., t.text_faint)),
            );
        }
        let send = div()
            .id("ai-send")
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .size(px(28.))
            .rounded(px(14.))
            .bg(if running { t.text } else { t.accent })
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, window, cx| {
                if this.running.is_some() {
                    this.stop(cx)
                } else {
                    this.send(None, window, cx)
                }
            }))
            .child(icon(if running { "stop" } else { "arrow-up" }, 15., if running { t.bg } else { gpui::white() }));
        let accent = t.accent;
        let chip = |id: &'static str, icon_name: &'static str, label: String, color: gpui::Hsla, menu: ComposerMenu| {
            let open = self.menu.is_some_and(|(m, _)| m == menu);
            div()
                .id(id)
                .flex()
                .flex_none()
                .items_center()
                .gap(px(4.))
                .h(px(26.))
                .px(px(6.))
                .rounded(px(6.))
                .text_size(px(12.))
                .text_color(color)
                .cursor_pointer()
                .when(open, |d| d.bg(hover))
                .hover(move |s| s.bg(hover))
                .on_click(cx.listener(move |this, ev: &gpui::ClickEvent, _, cx| {
                    this.menu = if this.menu.is_some_and(|(m, _)| m == menu) { None } else { Some((menu, ev.position())) };
                    cx.notify();
                }))
                .child(icon(icon_name, 13., color))
                .child(label)
                .child(icon("chevron-down", 11., t.text_faint))
        };
        let model = label_of(&MODELS, s.state.ai_model.as_deref());
        let effort = label_of(&EFFORTS, s.state.ai_effort.as_deref());
        let toggle = div()
            .flex()
            .items_center()
            .ml(px(-4.))
            .child(chip(
                "ai-access",
                if allow { "edit" } else { "eye" },
                if allow { "Can edit".into() } else { "Read-only".into() },
                if allow { accent } else { t.text_faint },
                ComposerMenu::Access,
            ))
            .child(chip("ai-model", "sparkle", model, t.text_faint, ComposerMenu::Model))
            .child(chip("ai-effort", "clock", effort, t.text_faint, ComposerMenu::Effort));
        div()
            .flex_none()
            .px(px(12.))
            .pb(px(12.))
            .pt(px(4.))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .p(px(10.))
                    .rounded(px(14.))
                    .bg(t.panel_bg)
                    .border_1()
                    .border_color(t.border_strong)
                    .shadow(card_shadow(t))
                    .child(chips)
                    .child(div().min_h(px(42.)).max_h(px(180.)).overflow_hidden().child(self.input.clone()))
                    .child(div().flex().items_center().pt(px(6.)).child(toggle).child(div().flex_1()).child(send)),
            )
            .into_any_element()
    }
}

impl Render for AiPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let cx: &Context<Self> = cx;
        let header = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(6.))
            .h(px(44.))
            .pl(px(14.))
            .pr(px(8.))
            .child(icon("sparkles", 16., t.text))
            .child(div().flex_1().text_size(px(14.)).font_weight(FontWeight::MEDIUM).child("Nota AI"))
            .child(icon_button("ai-new", "edit", &t).on_click(cx.listener(|this, _, _, cx| this.new_chat(cx))))
            .child(icon_button("ai-close", "x", &t).on_click(cx.listener(|_, _, _, cx| cx.emit(AiEvent::Close))));

        let mut list = div().flex().flex_col().gap(px(18.)).px(px(16.)).pt(px(8.)).pb(px(20.));
        if self.messages.is_empty() {
            list = list.child(self.render_empty(&t, cx));
        }
        let n = self.messages.len();
        for (i, m) in self.messages.iter().enumerate() {
            list = list.child(self.render_message(i, m, i + 1 == n, &t, cx));
        }
        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(t.bg)
            .text_color(t.text)
            .child(header)
            .child(
                div()
                    .id("ai-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .child(list),
            )
            .child(self.render_composer(&t, cx))
            .children(self.render_menu(&t, cx))
    }
}
