//! Markdown <-> blocks.
//!
//! The dialect is plain CommonMark-ish Markdown so that vault files stay
//! readable in any editor (and by Claude Code), with a few conventions:
//!
//! * nesting = 4 spaces of indentation per level (any block may have children)
//! * `+ text`            toggle list item
//! * `> [!💡] text`      callout (icon in the brackets)
//! * `[[<id>|Title]]`    link to a sub-page, on its own line
//! * `<br>`              an intentionally empty paragraph
//! * a trailing `\`      soft line break inside a block
//! * `<u>text</u>`       underline

use crate::model::{list_number, normalize_indents, Block, BlockKind};
use crate::rich_text::{Marks, RichText, Run};

// ---------------------------------------------------------------------------
// Inline: RichText -> Markdown
// ---------------------------------------------------------------------------

fn escape_text(text: &str, out: &mut String) {
    let bytes = text.as_bytes();
    for (i, c) in text.char_indices() {
        match c {
            '\\' | '*' | '`' | '~' | '[' | ']' => {
                out.push('\\');
                out.push(c);
            }
            '<' => {
                let rest = &bytes[i..];
                if rest.starts_with(b"<u>") || rest.starts_with(b"</u>") {
                    out.push('\\');
                }
                out.push(c);
            }
            _ => out.push(c),
        }
    }
}

fn encode_url(url: &str) -> String {
    url.replace(' ', "%20").replace('(', "%28").replace(')', "%29")
}

/// Undo `%XX` escapes (for local file paths in image links).
pub fn decode_url(url: &str) -> String {
    let hex = |b: u8| (b as char).to_digit(16).map(|d| d as u8);
    let bytes = url.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(h), Some(l)) = (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                out.push(h * 16 + l);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| url.to_string())
}

/// `|---|:---:|` style table separator rows.
fn is_table_separator(line: &str) -> bool {
    let t = line.trim();
    t.starts_with('|') && t.contains('-') && t.chars().all(|c| matches!(c, '|' | '-' | ':' | ' ' | '\t'))
}

/// Split one table row into cell texts (inline Markdown), honouring `\|`.
pub fn table_cells(line: &str) -> Vec<String> {
    let t = line.trim();
    let t = t.strip_prefix('|').unwrap_or(t);
    let t = t.strip_suffix('|').filter(|r| !r.ends_with('\\')).unwrap_or(t);
    let mut cells = vec![String::new()];
    let mut chars = t.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' if chars.peek() == Some(&'|') => {
                cells.last_mut().unwrap().push('|');
                chars.next();
            }
            '|' => cells.push(String::new()),
            c => cells.last_mut().unwrap().push(c),
        }
    }
    cells.into_iter().map(|c| c.trim().to_string()).collect()
}

/// Header and body rows of a table block's source (the separator row is dropped).
pub fn table_rows(src: &str) -> Vec<Vec<String>> {
    src.lines()
        .enumerate()
        .filter(|(i, l)| !(*i == 1 && is_table_separator(l)) && !l.trim().is_empty())
        .map(|(_, l)| table_cells(l))
        .collect()
}

/// A line that is just `![alt](src)` (Obsidian-style `![alt|400](src)` sets a
/// width). Returns the byte length of the alt text, the decoded source and the width.
fn parse_image_line(content: &str) -> Option<(usize, String, Option<u32>)> {
    let rest = content.strip_prefix("![")?.strip_suffix(')')?;
    let (alt, src) = rest.split_once("](")?;
    let spaced = src.contains(char::is_whitespace) && !src.starts_with('<');
    if alt.contains(']') || src.is_empty() || spaced {
        return None;
    }
    let src = src.strip_prefix('<').and_then(|s| s.strip_suffix('>')).unwrap_or(src);
    let (alt, width) = match alt.rsplit_once('|') {
        Some((a, w)) if !w.is_empty() && w.chars().all(|c| c.is_ascii_digit()) => (a, w.parse().ok()),
        _ => (alt, None),
    };
    Some((alt.len(), decode_url(src), width))
}

fn push_code_span(text: &str, out: &mut String) {
    let mut longest = 0;
    let mut cur = 0;
    for c in text.chars() {
        if c == '`' {
            cur += 1;
            longest = longest.max(cur);
        } else {
            cur = 0;
        }
    }
    let fence = "`".repeat(longest + 1);
    let pad = text.starts_with('`')
        || text.ends_with('`')
        || (text.starts_with(' ') && text.ends_with(' ') && !text.trim().is_empty());
    out.push_str(&fence);
    if pad {
        out.push(' ');
    }
    out.push_str(text);
    if pad {
        out.push(' ');
    }
    out.push_str(&fence);
}

pub fn inline_to_md(rt: &RichText) -> String {
    let mut out = String::new();
    let mut active = Marks::default();

    fn close_styles(out: &mut String, active: &mut Marks, want: &Marks) {
        if active.underline && !want.underline {
            out.push_str("</u>");
            active.underline = false;
        }
        if active.strike && !want.strike {
            out.push_str("~~");
            active.strike = false;
        }
        if active.italic && !want.italic {
            out.push('*');
            active.italic = false;
        }
        if active.bold && !want.bold {
            out.push_str("**");
            active.bold = false;
        }
    }

    fn open_styles(out: &mut String, active: &mut Marks, want: &Marks) {
        if want.bold && !active.bold {
            out.push_str("**");
            active.bold = true;
        }
        if want.italic && !active.italic {
            out.push('*');
            active.italic = true;
        }
        if want.strike && !active.strike {
            out.push_str("~~");
            active.strike = true;
        }
        if want.underline && !active.underline {
            out.push_str("<u>");
            active.underline = true;
        }
    }

    let none = Marks::default();
    for (range, marks) in rt.segments() {
        let text = &rt.text[range];
        if text.is_empty() {
            continue;
        }
        if marks.link != active.link {
            close_styles(&mut out, &mut active, &none);
            if let Some(url) = active.link.take() {
                out.push_str("](");
                out.push_str(&encode_url(&url));
                out.push(')');
            }
            if let Some(url) = &marks.link {
                out.push('[');
                active.link = Some(url.clone());
            }
        }
        close_styles(&mut out, &mut active, marks);
        open_styles(&mut out, &mut active, marks);
        if marks.code {
            push_code_span(text, &mut out);
        } else {
            escape_text(text, &mut out);
        }
    }
    close_styles(&mut out, &mut active, &none);
    if let Some(url) = active.link.take() {
        out.push_str("](");
        out.push_str(&encode_url(&url));
        out.push(')');
    }
    out
}

// ---------------------------------------------------------------------------
// Inline: Markdown -> RichText
// ---------------------------------------------------------------------------

#[derive(Debug)]
enum Tok {
    Text(String),
    Code(String),
    Bold,
    Italic,
    Strike,
    UOn,
    UOff,
    Link(Vec<Tok>, String),
}

fn starts_with(chars: &[char], i: usize, pat: &str) -> bool {
    let mut j = i;
    for p in pat.chars() {
        if j >= chars.len() || chars[j] != p {
            return false;
        }
        j += 1;
    }
    true
}

fn tokenize(chars: &[char]) -> Vec<Tok> {
    let n = chars.len();
    let mut toks: Vec<Tok> = Vec::new();
    let mut buf = String::new();
    let mut i = 0;

    macro_rules! flush {
        () => {
            if !buf.is_empty() {
                toks.push(Tok::Text(std::mem::take(&mut buf)));
            }
        };
    }

    while i < n {
        let c = chars[i];
        match c {
            '\\' if i + 1 < n
                && (chars[i + 1].is_ascii_punctuation()
                    || chars[i + 1] == ' '
                    || chars[i + 1] == '\t') =>
            {
                buf.push(chars[i + 1]);
                i += 2;
            }
            '`' => {
                let mut k = 0;
                while i + k < n && chars[i + k] == '`' {
                    k += 1;
                }
                // find a closing run of exactly k backticks
                let mut j = i + k;
                let mut close = None;
                while j < n {
                    if chars[j] == '`' {
                        let mut m = 0;
                        while j + m < n && chars[j + m] == '`' {
                            m += 1;
                        }
                        if m == k {
                            close = Some(j);
                            break;
                        }
                        j += m;
                    } else {
                        j += 1;
                    }
                }
                if let Some(j) = close {
                    let mut content: String = chars[i + k..j].iter().collect();
                    if content.len() >= 2
                        && content.starts_with(' ')
                        && content.ends_with(' ')
                        && !content.trim().is_empty()
                    {
                        content = content[1..content.len() - 1].to_string();
                    }
                    flush!();
                    toks.push(Tok::Code(content));
                    i = j + k;
                } else {
                    for _ in 0..k {
                        buf.push('`');
                    }
                    i += k;
                }
            }
            '*' => {
                let mut k = 0;
                while i + k < n && chars[i + k] == '*' {
                    k += 1;
                }
                flush!();
                for _ in 0..k / 2 {
                    toks.push(Tok::Bold);
                }
                if k % 2 == 1 {
                    toks.push(Tok::Italic);
                }
                i += k;
            }
            '~' if i + 1 < n && chars[i + 1] == '~' => {
                flush!();
                toks.push(Tok::Strike);
                i += 2;
            }
            '<' if starts_with(chars, i, "<u>") => {
                flush!();
                toks.push(Tok::UOn);
                i += 3;
            }
            '<' if starts_with(chars, i, "</u>") => {
                flush!();
                toks.push(Tok::UOff);
                i += 4;
            }
            '[' => {
                // find the matching ']' followed by '('
                let mut depth = 0i32;
                let mut j = i + 1;
                let mut close = None;
                while j < n {
                    match chars[j] {
                        '\\' => j += 1,
                        '[' => depth += 1,
                        ']' => {
                            if depth == 0 {
                                close = Some(j);
                                break;
                            }
                            depth -= 1;
                        }
                        _ => {}
                    }
                    j += 1;
                }
                let mut parsed = false;
                if let Some(close) = close {
                    if close + 1 < n && chars[close + 1] == '(' {
                        let mut k = close + 2;
                        let mut pdepth = 0i32;
                        let mut end = None;
                        while k < n {
                            match chars[k] {
                                '(' => pdepth += 1,
                                ')' => {
                                    if pdepth == 0 {
                                        end = Some(k);
                                        break;
                                    }
                                    pdepth -= 1;
                                }
                                '\n' => break,
                                _ => {}
                            }
                            k += 1;
                        }
                        if let Some(end) = end {
                            let url: String = chars[close + 2..end].iter().collect();
                            let url = url
                                .trim()
                                .replace("%20", " ")
                                .replace("%28", "(")
                                .replace("%29", ")");
                            if !url.is_empty() {
                                let mut inner = tokenize(&chars[i + 1..close]);
                                fix_parity(&mut inner);
                                flush!();
                                toks.push(Tok::Link(inner, url));
                                i = end + 1;
                                parsed = true;
                            }
                        }
                    }
                }
                if !parsed {
                    buf.push('[');
                    i += 1;
                }
            }
            _ => {
                buf.push(c);
                i += 1;
            }
        }
    }
    flush!();
    toks
}

/// A delimiter without a partner is literal text.
fn fix_parity(toks: &mut [Tok]) {
    fn fix(toks: &mut [Tok], is: fn(&Tok) -> bool, lit: &str) {
        let count = toks.iter().filter(|t| is(t)).count();
        if count % 2 == 1 {
            if let Some(ix) = toks.iter().rposition(is) {
                toks[ix] = Tok::Text(lit.to_string());
            }
        }
    }
    fix(toks, |t| matches!(t, Tok::Bold), "**");
    fix(toks, |t| matches!(t, Tok::Italic), "*");
    fix(toks, |t| matches!(t, Tok::Strike), "~~");
}

fn build(toks: &[Tok], marks: &mut Marks, out: &mut RichText) {
    for tok in toks {
        match tok {
            Tok::Text(s) => {
                out.text.push_str(s);
                out.runs.push(Run {
                    len: s.len(),
                    marks: marks.clone(),
                });
            }
            Tok::Code(s) => {
                let mut m = marks.clone();
                m.code = true;
                out.text.push_str(s);
                out.runs.push(Run { len: s.len(), marks: m });
            }
            Tok::Bold => marks.bold = !marks.bold,
            Tok::Italic => marks.italic = !marks.italic,
            Tok::Strike => marks.strike = !marks.strike,
            Tok::UOn => marks.underline = true,
            Tok::UOff => marks.underline = false,
            Tok::Link(inner, url) => {
                let mut m = marks.clone();
                m.link = Some(url.clone());
                build(inner, &mut m, out);
            }
        }
    }
}

pub fn inline_from_md(s: &str) -> RichText {
    let chars: Vec<char> = s.chars().collect();
    let mut toks = tokenize(&chars);
    fix_parity(&mut toks);
    let mut out = RichText::default();
    let mut marks = Marks::default();
    build(&toks, &mut marks, &mut out);
    out.normalize();
    out
}

// ---------------------------------------------------------------------------
// Blocks -> Markdown
// ---------------------------------------------------------------------------

const INDENT: &str = "    ";

fn is_divider(s: &str) -> bool {
    let t = s.trim();
    t.len() >= 3
        && (t.chars().all(|c| c == '-') || t.chars().all(|c| c == '*') || t.chars().all(|c| c == '_'))
}

fn callout_icon(tag: &str) -> String {
    match tag.to_ascii_lowercase().as_str() {
        "note" => "📝".into(),
        "tip" | "hint" | "info" | "callout" => "💡".into(),
        "important" => "❗".into(),
        "warning" | "attention" => "⚠️".into(),
        "caution" | "danger" | "error" => "🚨".into(),
        "success" | "check" | "done" => "✅".into(),
        "question" | "faq" => "❓".into(),
        _ => {
            if tag.is_empty() || tag.chars().all(|c| c.is_ascii_alphanumeric()) {
                "💡".into()
            } else {
                tag.to_string()
            }
        }
    }
}

/// Classify one line (indentation already removed). Returns the kind and the
/// raw inline markdown of its text.
fn classify(content: &str) -> (BlockKind, &str) {
    if content == "<br>" || content == "<br/>" || content == "<br />" {
        return (BlockKind::Paragraph, "");
    }
    if is_divider(content) {
        return (BlockKind::Divider, "");
    }
    if content.starts_with("[[") && content.ends_with("]]") && content.len() > 4 {
        let inner = &content[2..content.len() - 2];
        if !inner.contains("]]") && !inner.contains("[[") {
            let (id, _title) = inner.split_once('|').unwrap_or((inner, ""));
            if !id.is_empty() && !id.contains(char::is_whitespace) {
                return (
                    BlockKind::PageLink {
                        page: id.to_string(),
                    },
                    inner.split_once('|').map(|x| x.1).unwrap_or(""),
                );
            }
        }
    }
    if let Some((alt_len, src, width)) = parse_image_line(content) {
        return (BlockKind::Image { src, width }, &content[2..2 + alt_len]);
    }
    // headings
    let hashes = content.chars().take_while(|c| *c == '#').count();
    if (1..=6).contains(&hashes) {
        let rest = &content[hashes..];
        if rest.is_empty() {
            return (BlockKind::Heading(hashes.min(3) as u8), "");
        }
        if let Some(rest) = rest.strip_prefix(' ') {
            return (BlockKind::Heading(hashes.min(3) as u8), rest.trim_start());
        }
    }
    // to-dos
    for (prefix, checked) in [
        ("- [ ] ", false),
        ("- [x] ", true),
        ("- [X] ", true),
        ("* [ ] ", false),
        ("* [x] ", true),
        ("* [X] ", true),
    ] {
        if let Some(rest) = content.strip_prefix(prefix) {
            return (BlockKind::Todo { checked }, rest);
        }
        if content == prefix.trim_end() {
            return (BlockKind::Todo { checked }, "");
        }
    }
    if let Some(rest) = content.strip_prefix("- ").or_else(|| content.strip_prefix("* ")) {
        return (BlockKind::Bullet, rest);
    }
    if content == "-" {
        return (BlockKind::Bullet, "");
    }
    if let Some(rest) = content.strip_prefix("+ ") {
        return (BlockKind::Toggle { open: false }, rest);
    }
    if content == "+" {
        return (BlockKind::Toggle { open: false }, "");
    }
    // numbered
    let digits = content.chars().take_while(|c| c.is_ascii_digit()).count();
    if (1..=9).contains(&digits) {
        let rest = &content[digits..];
        if let Some(rest) = rest.strip_prefix('.').or_else(|| rest.strip_prefix(')')) {
            if rest.is_empty() {
                return (BlockKind::Numbered, "");
            }
            if let Some(rest) = rest.strip_prefix(' ') {
                return (BlockKind::Numbered, rest);
            }
        }
    }
    // quotes and callouts
    if let Some(rest) = content.strip_prefix('>') {
        let rest = rest.strip_prefix(' ').unwrap_or(rest);
        if let Some(after) = rest.strip_prefix("[!") {
            if let Some(end) = after.find(']') {
                let tag = &after[..end];
                let text = &after[end + 1..];
                let text = text.strip_prefix(' ').unwrap_or(text);
                return (
                    BlockKind::Callout {
                        icon: callout_icon(tag),
                    },
                    text,
                );
            }
        }
        return (BlockKind::Quote, rest);
    }
    (BlockKind::Paragraph, content)
}

/// Make sure a paragraph's markdown is not mistaken for another block type.
fn escape_block_start(line: &str) -> String {
    let needs = line.starts_with(char::is_whitespace)
        || line.starts_with("```")
        || !matches!(classify(line), (BlockKind::Paragraph, t) if t == line);
    if !needs {
        return line.to_string();
    }
    let first = line.chars().next().unwrap_or(' ');
    if first.is_ascii_digit() {
        let digits = line.chars().take_while(|c| c.is_ascii_digit()).count();
        format!("{}\\{}", &line[..digits], &line[digits..])
    } else {
        format!("\\{}", line)
    }
}

fn is_listish(kind: &BlockKind) -> bool {
    matches!(
        kind,
        BlockKind::Bullet | BlockKind::Numbered | BlockKind::Todo { .. } | BlockKind::Toggle { .. }
    )
}

pub fn to_markdown(blocks: &[Block]) -> String {
    // drop trailing empty paragraphs
    let mut end = blocks.len();
    while end > 0 && blocks[end - 1].kind == BlockKind::Paragraph && blocks[end - 1].text.is_empty() {
        end -= 1;
    }
    let blocks = &blocks[..end];

    let mut out = String::new();
    let mut prev_indent: i32 = -1;
    let mut prev_listish = false;
    for (i, b) in blocks.iter().enumerate() {
        let indent = (b.indent as i32).min(prev_indent + 1).max(0) as usize;
        let pad = INDENT.repeat(indent);
        if i > 0 {
            out.push('\n');
            if !(prev_listish && is_listish(&b.kind)) {
                out.push('\n');
            }
        }
        let cont = format!("\\\n{}", pad);
        match &b.kind {
            BlockKind::Paragraph => {
                if b.text.is_empty() {
                    out.push_str(&pad);
                    out.push_str("<br>");
                } else {
                    let md = escape_block_start(&inline_to_md(&b.text));
                    out.push_str(&pad);
                    out.push_str(&md.replace('\n', &cont));
                }
            }
            BlockKind::Heading(n) => {
                out.push_str(&pad);
                out.push_str(&"#".repeat((*n).clamp(1, 3) as usize));
                out.push(' ');
                out.push_str(&inline_to_md(&b.text).replace('\n', &cont));
            }
            BlockKind::Bullet => {
                out.push_str(&pad);
                out.push_str("- ");
                out.push_str(&inline_to_md(&b.text).replace('\n', &cont));
            }
            BlockKind::Numbered => {
                out.push_str(&pad);
                out.push_str(&format!("{}. ", list_number(blocks, i)));
                out.push_str(&inline_to_md(&b.text).replace('\n', &cont));
            }
            BlockKind::Todo { checked } => {
                out.push_str(&pad);
                out.push_str(if *checked { "- [x] " } else { "- [ ] " });
                out.push_str(&inline_to_md(&b.text).replace('\n', &cont));
            }
            BlockKind::Toggle { .. } => {
                out.push_str(&pad);
                out.push_str("+ ");
                out.push_str(&inline_to_md(&b.text).replace('\n', &cont));
            }
            BlockKind::Quote | BlockKind::Callout { .. } => {
                let md = inline_to_md(&b.text);
                for (li, line) in md.split('\n').enumerate() {
                    if li > 0 {
                        out.push('\n');
                    }
                    out.push_str(&pad);
                    out.push_str("> ");
                    if li == 0 {
                        if let BlockKind::Callout { icon } = &b.kind {
                            out.push_str(&format!("[!{}] ", icon.replace(']', "")));
                        }
                    }
                    out.push_str(line);
                }
            }
            BlockKind::Divider => {
                out.push_str(&pad);
                out.push_str("---");
            }
            BlockKind::Code { lang } => {
                let mut longest = 0;
                let mut cur = 0;
                for c in b.text.text.chars() {
                    if c == '`' {
                        cur += 1;
                        longest = longest.max(cur);
                    } else {
                        cur = 0;
                    }
                }
                let fence = "`".repeat(longest.max(2) + 1);
                out.push_str(&pad);
                out.push_str(&fence);
                out.push_str(lang.trim());
                out.push('\n');
                for line in b.text.text.split('\n') {
                    out.push_str(&pad);
                    out.push_str(line);
                    out.push('\n');
                }
                out.push_str(&pad);
                out.push_str(&fence);
            }
            BlockKind::PageLink { page } => {
                let title = b.text.text.replace(['[', ']', '|', '\n'], " ");
                out.push_str(&pad);
                out.push_str(&format!("[[{}|{}]]", page, title.trim()));
            }
            BlockKind::Table => {
                for (li, line) in b.text.text.split('\n').enumerate() {
                    if li > 0 {
                        out.push('\n');
                    }
                    out.push_str(&pad);
                    out.push_str(line.trim_end());
                }
            }
            BlockKind::Image { src, width } => {
                let alt = b.text.text.replace(['[', ']', '|', '\n'], " ");
                out.push_str(&pad);
                out.push_str("![");
                out.push_str(alt.trim());
                if let Some(w) = width {
                    out.push_str(&format!("|{w}"));
                }
                out.push_str("](");
                out.push_str(&encode_url(src));
                out.push(')');
            }
        }
        prev_indent = indent as i32;
        prev_listish = is_listish(&b.kind);
    }
    if !out.is_empty() {
        out.push('\n');
    }
    out
}

// ---------------------------------------------------------------------------
// Markdown -> Blocks
// ---------------------------------------------------------------------------

struct Pending {
    kind: BlockKind,
    indent: u8,
    raw: String,
}

/// Strip a trailing soft-break backslash (odd number of trailing backslashes).
fn strip_continuation(s: &str) -> (&str, bool) {
    let trailing = s.chars().rev().take_while(|c| *c == '\\').count();
    if trailing % 2 == 1 {
        (&s[..s.len() - 1], true)
    } else {
        (s, false)
    }
}

fn leading_columns(line: &str) -> (usize, usize) {
    let mut col = 0;
    let mut bytes = 0;
    for c in line.chars() {
        match c {
            ' ' => col += 1,
            '\t' => col += 4,
            _ => break,
        }
        bytes += c.len_utf8();
    }
    (col, bytes)
}

fn strip_columns(line: &str, cols: usize) -> &str {
    let mut col = 0;
    let mut bytes = 0;
    for c in line.chars() {
        if col >= cols {
            break;
        }
        match c {
            ' ' => col += 1,
            '\t' => col += 4,
            _ => break,
        }
        bytes += c.len_utf8();
    }
    &line[bytes..]
}

pub fn from_markdown(src: &str) -> Vec<Block> {
    let mut out: Vec<Pending> = Vec::new();
    let mut stack: Vec<usize> = Vec::new();
    let mut lines = src.lines().peekable();
    let mut prev_blank = true;
    let mut continuation = false;

    while let Some(raw_line) = lines.next() {
        let line = raw_line.trim_end_matches('\r');

        if continuation {
            let content = line.trim_start();
            let (content, cont) = strip_continuation(content);
            if let Some(last) = out.last_mut() {
                last.raw.push('\n');
                last.raw.push_str(content);
            }
            continuation = cont;
            prev_blank = false;
            continue;
        }

        if line.trim().is_empty() {
            prev_blank = true;
            continue;
        }

        let (col, skip) = leading_columns(line);
        let content = &line[skip..];
        while stack.last().is_some_and(|c| *c >= col) {
            stack.pop();
        }
        let depth = stack.len().min(u8::MAX as usize) as u8;
        stack.push(col);

        // fenced code
        if content.starts_with("```") {
            let fence_len = content.chars().take_while(|c| *c == '`').count();
            let lang = content[fence_len..].trim().to_string();
            let mut code = String::new();
            let mut first = true;
            for code_line in lines.by_ref() {
                let code_line = code_line.trim_end_matches('\r');
                let t = code_line.trim();
                if t.len() >= fence_len && t.chars().all(|c| c == '`') {
                    break;
                }
                if !first {
                    code.push('\n');
                }
                first = false;
                code.push_str(strip_columns(code_line, col));
            }
            out.push(Pending {
                kind: BlockKind::Code { lang },
                indent: depth,
                raw: code,
            });
            prev_blank = false;
            continue;
        }

        // GFM table: a `|` row followed by a `|---|` separator row
        if content.starts_with('|') && lines.peek().is_some_and(|next| is_table_separator(next)) {
            let mut raw = content.trim_end().to_string();
            while let Some(next) = lines.peek() {
                let t = next.trim_start();
                if !t.starts_with('|') {
                    break;
                }
                raw.push('\n');
                raw.push_str(t.trim_end());
                lines.next();
            }
            out.push(Pending {
                kind: BlockKind::Table,
                indent: depth,
                raw,
            });
            prev_blank = false;
            continue;
        }

        let (kind, text) = classify(content);
        let no_text = matches!(kind, BlockKind::Divider | BlockKind::PageLink { .. } | BlockKind::Image { .. });
        let (text, cont) = if no_text {
            (text, false)
        } else {
            strip_continuation(text)
        };

        let mut merged = false;
        if !prev_blank {
            if let Some(last) = out.last_mut() {
                if last.indent == depth {
                    let last_quote =
                        matches!(last.kind, BlockKind::Quote | BlockKind::Callout { .. });
                    if kind == BlockKind::Quote && last_quote {
                        // `> [!NOTE]` on its own line: the text starts on the next line
                        let bare_callout = last.raw.is_empty()
                            && matches!(last.kind, BlockKind::Callout { .. });
                        if !bare_callout {
                            last.raw.push('\n');
                        }
                        last.raw.push_str(text);
                        merged = true;
                    } else if kind == BlockKind::Paragraph
                        && last.kind == BlockKind::Paragraph
                        && !last.raw.is_empty()
                        && !text.is_empty()
                    {
                        last.raw.push(' ');
                        last.raw.push_str(text);
                        merged = true;
                    }
                }
            }
        }
        if !merged {
            out.push(Pending {
                kind,
                indent: depth,
                raw: text.to_string(),
            });
        }
        continuation = cont;
        prev_blank = false;
    }

    let mut blocks: Vec<Block> = out
        .into_iter()
        .map(|p| {
            let text = match p.kind {
                BlockKind::Code { .. } | BlockKind::Table | BlockKind::PageLink { .. } | BlockKind::Image { .. } => {
                    RichText::plain(p.raw)
                }
                BlockKind::Divider => RichText::default(),
                _ => inline_from_md(&p.raw),
            };
            Block::new(p.kind, text).with_indent(p.indent)
        })
        .collect();
    while blocks
        .last()
        .is_some_and(|b| b.kind == BlockKind::Paragraph && b.text.is_empty())
    {
        blocks.pop();
    }
    normalize_indents(&mut blocks);
    blocks
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rich_text::MarkKind;

    fn strip_ids(blocks: &[Block]) -> Vec<(BlockKind, RichText, u8)> {
        blocks
            .iter()
            .map(|b| (b.kind.clone(), b.text.clone(), b.indent))
            .collect()
    }

    fn roundtrip(blocks: Vec<Block>) {
        let md = to_markdown(&blocks);
        let parsed = from_markdown(&md);
        assert_eq!(strip_ids(&parsed), strip_ids(&blocks), "markdown was:\n{}", md);
        // and stable on a second pass
        assert_eq!(to_markdown(&parsed), md);
    }

    #[test]
    fn inline_roundtrip() {
        let mut t = RichText::plain("plain bold italic both code strike under end");
        t.toggle(6..10, MarkKind::Bold);
        t.toggle(11..17, MarkKind::Italic);
        t.toggle(18..22, MarkKind::Bold);
        t.toggle(18..22, MarkKind::Italic);
        t.toggle(23..27, MarkKind::Code);
        t.toggle(28..34, MarkKind::Strike);
        t.toggle(35..40, MarkKind::Underline);
        let md = inline_to_md(&t);
        assert_eq!(
            md,
            "plain **bold** *italic* ***both*** `code` ~~strike~~ <u>under</u> end"
        );
        assert_eq!(inline_from_md(&md), t);
    }

    #[test]
    fn inline_adjacent_marks() {
        let mut t = RichText::plain("abcdef");
        t.toggle(0..2, MarkKind::Bold);
        t.toggle(2..4, MarkKind::Italic);
        t.toggle(3..6, MarkKind::Bold);
        let md = inline_to_md(&t);
        assert_eq!(inline_from_md(&md), t, "{}", md);

        let mut t = RichText::plain("x  a  y");
        t.toggle(2..5, MarkKind::Bold);
        let md = inline_to_md(&t);
        assert_eq!(inline_from_md(&md), t, "{}", md);
    }

    #[test]
    fn inline_escapes_and_links() {
        let mut t = RichText::plain("2 * 3 = [6] `x` a\\b ~~ <u> see docs now");
        t.set_link(27..35, Some("https://example.com/a(b) c".into()));
        t.toggle(31..35, MarkKind::Bold);
        let md = inline_to_md(&t);
        assert_eq!(inline_from_md(&md), t, "{}", md);

        let mut t = RichText::plain("has ` tick and `` two");
        t.toggle(4..5, MarkKind::Code);
        t.toggle(15..21, MarkKind::Code);
        let md = inline_to_md(&t);
        assert_eq!(inline_from_md(&md), t, "{}", md);
    }

    #[test]
    fn unbalanced_is_literal() {
        assert_eq!(inline_from_md("a * b").text, "a * b");
        assert_eq!(inline_from_md("**bold").text, "**bold");
        assert_eq!(inline_from_md("[not a link]").text, "[not a link]");
        let t = inline_from_md("**b** and *i*");
        assert_eq!(t.text, "b and i");
        assert!(t.range_has(0..1, MarkKind::Bold));
        assert!(t.range_has(6..7, MarkKind::Italic));
    }

    #[test]
    fn block_roundtrip() {
        let mut rich = RichText::plain("some bold text");
        rich.toggle(5..9, MarkKind::Bold);
        roundtrip(vec![
            Block::new(BlockKind::Heading(1), "Title"),
            Block::paragraph(rich),
            Block::paragraph(""),
            Block::paragraph("line one\nline two\\"),
            Block::new(BlockKind::Bullet, "a"),
            Block::new(BlockKind::Bullet, "a.1").with_indent(1),
            Block::paragraph("child para").with_indent(2),
            Block::new(BlockKind::Bullet, "b"),
            Block::new(BlockKind::Numbered, "one"),
            Block::new(BlockKind::Numbered, "two"),
            Block::new(BlockKind::Todo { checked: false }, "todo"),
            Block::new(BlockKind::Todo { checked: true }, "done"),
            Block::new(BlockKind::Toggle { open: false }, "toggle"),
            Block::paragraph("inside").with_indent(1),
            Block::new(BlockKind::Quote, "quote\nsecond line"),
            Block::new(BlockKind::Quote, "another quote"),
            Block::new(
                BlockKind::Callout {
                    icon: "💡".into(),
                },
                "callout\nmore",
            ),
            Block::new(BlockKind::Divider, ""),
            Block::new(BlockKind::Image { src: "assets/Screen Shot (1).png".into(), width: None }, "shot"),
            Block::new(BlockKind::Image { src: "assets/b.png".into(), width: Some(420) }, ""),
            Block::new(
                BlockKind::Code {
                    lang: "rust".into(),
                },
                "fn main() {\n    println!(\"hi\");\n}\n\n// ``` fence inside",
            ),
            Block::new(
                BlockKind::PageLink {
                    page: "abc123".into(),
                },
                "Sub page",
            ),
            Block::new(BlockKind::Heading(3), ""),
            Block::new(BlockKind::Bullet, ""),
            Block::paragraph("end"),
        ]);
    }

    #[test]
    fn tricky_paragraphs() {
        roundtrip(vec![
            Block::paragraph("- not a bullet"),
            Block::paragraph("# not a heading"),
            Block::paragraph("1. not numbered"),
            Block::paragraph("> not a quote"),
            Block::paragraph("+ not a toggle"),
            Block::paragraph("---"),
            Block::paragraph("<br>"),
            Block::paragraph("  leading spaces"),
            Block::paragraph("[[x|y]]"),
            Block::paragraph("```"),
            Block::paragraph("中文 **星号** 和 `反引号`"),
            Block::new(BlockKind::Bullet, "[ ] not a todo"),
            Block::new(BlockKind::Code { lang: "".into() }, ""),
            Block::paragraph("x"),
        ]);
    }

    #[test]
    fn external_markdown() {
        let md = "# Title\n\nFirst line\nsecond line.\n\n* item one\n* item two\n  * nested\n\n1. a\n2. b\n\n> [!WARNING]\n> careful\n\n```python\nprint(1)\n```\n";
        let blocks = from_markdown(md);
        let kinds: Vec<_> = blocks.iter().map(|b| (b.kind.label(), b.indent)).collect();
        assert_eq!(
            kinds,
            vec![
                ("Heading 1", 0),
                ("Text", 0),
                ("Bulleted list", 0),
                ("Bulleted list", 0),
                ("Bulleted list", 1),
                ("Numbered list", 0),
                ("Numbered list", 0),
                ("Callout", 0),
                ("Code", 0),
            ]
        );
        assert_eq!(blocks[1].text.text, "First line second line.");
        assert_eq!(blocks[7].text.text, "careful");
        assert_eq!(blocks[8].text.text, "print(1)");
    }
}

#[cfg(test)]
mod image_tests {
    use super::*;

    #[test]
    fn image_lines() {
        let b = from_markdown("![shot](assets/a%20b.png)\n\n![w|300](<x y.png>)\n\ntext ![i](j.png)\n");
        assert_eq!(b[0].kind, BlockKind::Image { src: "assets/a b.png".into(), width: None });
        assert_eq!(b[0].text.text, "shot");
        assert_eq!(b[1].kind, BlockKind::Image { src: "x y.png".into(), width: Some(300) });
        assert_eq!(b[1].text.text, "w");
        // inline images stay part of the paragraph
        assert_eq!(b[2].kind, BlockKind::Paragraph);
        // a paragraph that merely looks like an image is escaped
        let p = vec![Block::paragraph("![not](an image)x")];
        assert_eq!(from_markdown(&to_markdown(&p))[0].kind, BlockKind::Paragraph);
        assert_eq!(decode_url("a%2"), "a%2");
        assert_eq!(decode_url("%E4%BD%A0"), "你");
    }
}
