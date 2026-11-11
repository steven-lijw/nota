//! Colours and typography, matched to Notion's light and dark themes.

use gpui::{rgb, App, Global, Hsla, SharedString};

fn c(hex: u32) -> Hsla {
    rgb(hex).into()
}

fn ca(hex: u32, a: f32) -> Hsla {
    let mut h: Hsla = rgb(hex).into();
    h.a = a;
    h
}

#[derive(Clone)]
pub struct Theme {
    pub dark: bool,
    pub bg: Hsla,
    pub sidebar_bg: Hsla,
    pub panel_bg: Hsla,
    pub text: Hsla,
    pub text_muted: Hsla,
    pub text_faint: Hsla,
    pub placeholder: Hsla,
    pub border: Hsla,
    pub border_strong: Hsla,
    pub hover: Hsla,
    pub active: Hsla,
    pub accent: Hsla,
    pub accent_hover: Hsla,
    pub accent_bg: Hsla,
    pub selection: Hsla,
    pub block_selection: Hsla,
    pub code_bg: Hsla,
    pub code_text: Hsla,
    pub callout_bg: Hsla,
    pub codeblock_bg: Hsla,
    pub card_bg: Hsla,
    pub board_bg: Hsla,
    pub danger: Hsla,
    pub shadow: Hsla,
    pub overlay: Hsla,
    pub user_bubble: Hsla,
    pub sidebar_text: Hsla,
}

impl Global for Theme {}

impl Theme {
    pub fn light() -> Self {
        Theme {
            dark: false,
            bg: c(0xffffff),
            sidebar_bg: c(0xf7f7f5),
            panel_bg: c(0xffffff),
            text: c(0x37352f),
            text_muted: ca(0x37352f, 0.65),
            text_faint: ca(0x37352f, 0.45),
            placeholder: ca(0x37352f, 0.35),
            border: ca(0x37352f, 0.09),
            border_strong: ca(0x37352f, 0.16),
            hover: ca(0x37352f, 0.06),
            active: ca(0x37352f, 0.10),
            accent: c(0x2383e2),
            accent_hover: c(0x0077d4),
            accent_bg: ca(0x2383e2, 0.07),
            selection: ca(0x2383e2, 0.28),
            block_selection: ca(0x2383e2, 0.14),
            code_bg: ca(0x878378, 0.15),
            code_text: c(0xeb5757),
            callout_bg: c(0xf1f1ef),
            codeblock_bg: c(0xf7f6f3),
            card_bg: c(0xffffff),
            board_bg: c(0xf7f7f5),
            danger: c(0xeb5757),
            shadow: ca(0x0f0f0f, 0.10),
            overlay: ca(0x0f0f0f, 0.40),
            user_bubble: c(0xf2f1ee),
            sidebar_text: c(0x5f5e5b),
        }
    }

    pub fn dark() -> Self {
        Theme {
            dark: true,
            bg: c(0x191919),
            sidebar_bg: c(0x202020),
            panel_bg: c(0x252525),
            text: ca(0xffffff, 0.81),
            text_muted: ca(0xffffff, 0.50),
            text_faint: ca(0xffffff, 0.35),
            placeholder: ca(0xffffff, 0.28),
            border: ca(0xffffff, 0.09),
            border_strong: ca(0xffffff, 0.16),
            hover: ca(0xffffff, 0.055),
            active: ca(0xffffff, 0.10),
            accent: c(0x2383e2),
            accent_hover: c(0x3b93ea),
            accent_bg: ca(0x2383e2, 0.14),
            selection: ca(0x2383e2, 0.35),
            block_selection: ca(0x2383e2, 0.20),
            code_bg: ca(0x878378, 0.25),
            code_text: c(0xff7369),
            callout_bg: c(0x252525),
            codeblock_bg: c(0x202020),
            card_bg: c(0x262626),
            board_bg: c(0x1f1f1f),
            danger: c(0xff7369),
            shadow: ca(0x000000, 0.35),
            overlay: ca(0x000000, 0.55),
            user_bubble: c(0x2c2c2c),
            sidebar_text: c(0x9b9b9b),
        }
    }

    pub fn get(dark: bool) -> Self {
        if dark {
            Theme::dark()
        } else {
            Theme::light()
        }
    }

    /// Background and text colour for a select option ("tag") colour name.
    pub fn tag(&self, color: &str) -> (Hsla, Hsla) {
        if self.dark {
            let bg = match color {
                "gray" => 0x5a5a5a,
                "brown" => 0x603b2c,
                "orange" => 0x854c1d,
                "yellow" => 0x89632a,
                "green" => 0x2b593f,
                "blue" => 0x28456c,
                "purple" => 0x492f64,
                "pink" => 0x69314c,
                "red" => 0x6e3630,
                _ => 0x373737,
            };
            (c(bg), ca(0xffffff, 0.88))
        } else {
            let (bg, fg) = match color {
                "gray" => (0xe3e2e0, 0x32302c),
                "brown" => (0xeee0da, 0x442a1e),
                "orange" => (0xfadec9, 0x49290e),
                "yellow" => (0xfdecc8, 0x402c1b),
                "green" => (0xdbeddb, 0x1c3829),
                "blue" => (0xd3e5ef, 0x183347),
                "purple" => (0xe8deee, 0x412454),
                "pink" => (0xf5e0e9, 0x4c2337),
                "red" => (0xffe2dd, 0x5d1715),
                _ => (0xecebe9, 0x32302c),
            };
            (c(bg), c(fg))
        }
    }

    /// The strong dot colour used by status options.
    pub fn tag_dot(&self, color: &str) -> Hsla {
        c(match color {
            "gray" => 0x91918e,
            "brown" => 0xbb846c,
            "orange" => 0xd7813a,
            "yellow" => 0xcb9433,
            "green" => 0x6c9b7d,
            "blue" => 0x5b97bd,
            "purple" => 0xa782c3,
            "pink" => 0xcd749f,
            "red" => 0xe16f64,
            _ => 0x91918e,
        })
    }
}

pub fn theme(cx: &App) -> Theme {
    cx.global::<Theme>().clone()
}

/// The UI font. `NOTA_FONT` overrides the platform default.
pub fn ui_font() -> SharedString {
    static FONT: std::sync::OnceLock<SharedString> = std::sync::OnceLock::new();
    FONT.get_or_init(|| {
        if let Some(custom) = std::env::var("NOTA_FONT").ok().filter(|f| !f.is_empty()) {
            custom.into()
        } else if cfg!(target_os = "macos") {
            ".SystemUIFont".into()
        } else if cfg!(target_os = "windows") {
            "Segoe UI".into()
        } else {
            "Inter".into()
        }
    })
    .clone()
}

pub fn mono_font() -> SharedString {
    if cfg!(target_os = "macos") {
        "Menlo".into()
    } else if cfg!(target_os = "windows") {
        "Consolas".into()
    } else {
        "DejaVu Sans Mono".into()
    }
}

pub const SIDEBAR_W: f32 = 240.0;
pub const AI_PANEL_W: f32 = 385.0;
pub const TOPBAR_H: f32 = 44.0;
pub const PAGE_MAX_W: f32 = 708.0;
