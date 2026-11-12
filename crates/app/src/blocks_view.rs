//! Shared block styling (fonts, runs, markers) and a read-only block renderer
//! used for AI answers and previews.

use crate::text_view::TextView;
use crate::theme::{mono_font, ui_font, Theme};
use crate::ui::{checkbox, icon};
use gpui::{
    div, font, prelude::*, px, AnyElement, Div, FontStyle, FontWeight, Hsla, Pixels, SharedString,
    StrikethroughStyle, TextRun, UnderlineStyle,
};
use nota_core::{list_number, visibility, Block, BlockKind};
use std::ops::Range;

pub struct BlockText {
    pub text: SharedString,
    pub runs: Vec<TextRun>,
    pub highlights: Vec<(Range<usize>, Hsla)>,
    pub font_size: Pixels,
    pub line_height: Pixels,
}

/// Font size and line height for a block kind. `base` is the body size (16 in pages).
pub fn metrics(kind: &BlockKind, base: f32) -> (f32, f32) {
    match kind {
        BlockKind::Heading(1) => (base * 1.875, (base * 1.875 * 1.3).round()),
        BlockKind::Heading(2) => (base * 1.5, (base * 1.5 * 1.3).round()),
        BlockKind::Heading(_) => (base * 1.25, (base * 1.25 * 1.3).round()),
        BlockKind::Code { .. } => ((base * 0.85).round(), (base * 0.85 * 1.55).round()),
        _ => (base, (base * 1.5).round()),
    }
}

pub fn block_text(block: &Block, t: &Theme, base: f32) -> BlockText {
    let (font_size, line_height) = metrics(&block.kind, base);
    let is_code_block = matches!(block.kind, BlockKind::Code { .. });
    let heading = matches!(block.kind, BlockKind::Heading(_));
    let done = matches!(block.kind, BlockKind::Todo { checked: true });
    let base_color = if done { t.text_faint } else { t.text };

    let mut runs = Vec::with_capacity(block.text.runs.len());
    let mut highlights = Vec::new();
    for (range, marks) in block.text.segments() {
        if range.is_empty() {
            continue;
        }
        let mono = is_code_block || marks.code;
        let mut f = font(if mono { mono_font() } else { ui_font() });
        f.weight = if marks.bold && !is_code_block {
            FontWeight::BOLD
        } else if heading {
            FontWeight::SEMIBOLD
        } else {
            FontWeight::NORMAL
        };
        if marks.italic && !is_code_block {
            f.style = FontStyle::Italic;
        }
        let mut color = base_color;
        let mut underline = None;
        if !is_code_block {
            if marks.code {
                color = t.code_text;
                highlights.push((range.clone(), t.code_bg));
            }
            if marks.link.is_some() {
                underline = Some(UnderlineStyle {
                    thickness: px(1.),
                    color: Some(t.text_faint),
                    wavy: false,
                });
                if !marks.code {
                    color = t.text_muted;
                }
            } else if marks.underline {
                underline = Some(UnderlineStyle {
                    thickness: px(1.),
                    color: Some(color),
                    wavy: false,
                });
            }
        }
        let strikethrough = ((marks.strike && !is_code_block) || done).then(|| StrikethroughStyle {
            thickness: px(1.),
            color: Some(color),
        });
        runs.push(TextRun {
            len: range.len(),
            font: f,
            color,
            background_color: None,
            underline,
            strikethrough,
        });
    }
    if runs.is_empty() {
        let mut f = font(if is_code_block { mono_font() } else { ui_font() });
        if heading {
            f.weight = FontWeight::SEMIBOLD;
        }
        runs.push(TextRun {
            len: 0,
            font: f,
            color: base_color,
            background_color: None,
            underline: None,
            strikethrough: None,
        });
    }
    BlockText {
        text: SharedString::from(block.text.text.clone()),
        runs,
        highlights,
        font_size: px(font_size),
        line_height: px(line_height),
    }
}

/// Space above a block, Notion-style (headings get more air).
pub fn top_margin(kind: &BlockKind, first: bool, base: f32) -> f32 {
    let k = base / 16.0;
    if first {
        return 0.0;
    }
    match kind {
        BlockKind::Heading(1) => 30.0 * k,
        BlockKind::Heading(2) => 22.0 * k,
        BlockKind::Heading(_) => 16.0 * k,
        BlockKind::Callout { .. } | BlockKind::Code { .. } => 5.0 * k,
        _ => 1.0,
    }
}

/// The fixed-width marker column for list-like blocks.
pub fn marker_box(line_height: Pixels) -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .w(px(24.))
        .h(line_height)
}

pub fn bullet_glyph(indent: u8) -> &'static str {
    match indent % 3 {
        0 => "•",
        1 => "◦",
        _ => "▪",
    }
}

/// Render blocks read-only (no caret, no handles).
pub fn static_blocks(blocks: &[Block], t: &Theme, base: f32) -> Div {
    let visible = visibility(blocks);
    let mut col = div().flex().flex_col().w_full();
    for ix in 0..blocks.len() {
        if !visible[ix] {
            continue;
        }
        col = col.child(static_block(blocks, ix, t, base));
    }
    col
}

fn static_block(blocks: &[Block], ix: usize, t: &Theme, base: f32) -> AnyElement {
    let block = &blocks[ix];
    let bt = block_text(block, t, base);
    let lh = bt.line_height;
    let text = TextView::new(bt.text.clone(), bt.runs, bt.font_size, lh).highlights(bt.highlights);
    let text = div().flex_1().min_w_0().child(text);
    let row = div()
        .flex()
        .w_full()
        .pl(px(block.indent as f32 * 20.))
        .mt(px(top_margin(&block.kind, ix == 0, base).min(18.)))
        .py(px(2.));
    match &block.kind {
        BlockKind::Bullet => row
            .child(marker_box(lh).w(px(20.)).child(bullet_glyph(block.indent)))
            .child(text)
            .into_any_element(),
        BlockKind::Numbered => row
            .child(
                marker_box(lh)
                    .w(px(22.))
                    .justify_start()
                    .child(format!("{}.", list_number(blocks, ix))),
            )
            .child(text)
            .into_any_element(),
        BlockKind::Todo { checked } => row
            .child(marker_box(lh).w(px(22.)).justify_start().child(checkbox(*checked, t)))
            .child(text)
            .into_any_element(),
        BlockKind::Toggle { .. } => row
            .child(
                marker_box(lh)
                    .w(px(20.))
                    .child(icon("triangle-down", 12., t.text_muted)),
            )
            .child(text)
            .into_any_element(),
        BlockKind::Quote => row
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .border_l_3()
                    .border_color(t.text)
                    .pl(px(12.))
                    .child(text),
            )
            .into_any_element(),
        BlockKind::Callout { icon: emoji } => row
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .gap(px(8.))
                    .p(px(12.))
                    .rounded(px(6.))
                    .bg(t.callout_bg)
                    .child(div().flex_none().child(SharedString::from(emoji.clone())))
                    .child(text),
            )
            .into_any_element(),
        BlockKind::Divider => row
            .child(div().w_full().h(px(1.)).my(px(6.)).bg(t.border_strong))
            .into_any_element(),
        BlockKind::Code { .. } => row
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .px(px(12.))
                    .py(px(10.))
                    .rounded(px(6.))
                    .bg(t.codeblock_bg)
                    .child(text),
            )
            .into_any_element(),
        BlockKind::PageLink { .. } => row
            .child(icon("page", 16., t.text_muted))
            .child(div().pl(px(6.)).child(bt.text))
            .into_any_element(),
        BlockKind::Image { src, .. } => row
            .text_color(t.text_muted)
            .child(icon("image", 16., t.text_muted))
            .child(div().pl(px(6.)).child(SharedString::from(src.clone())))
            .into_any_element(),
        _ => row.child(text).into_any_element(),
    }
}
