//! Small Notion-styled building blocks shared by every view.

use crate::theme::Theme;
use gpui::{
    anchored, deferred, div, point, prelude::*, px, svg, AnyElement, App, BoxShadow, Corner, Div,
    ElementId, FontWeight, Hsla, MouseButton, MouseDownEvent, Pixels, Point, SharedString,
    Stateful, Svg, Window,
};

pub fn icon(name: &str, size: f32, color: Hsla) -> Svg {
    svg()
        .path(SharedString::from(format!("icons/{}.svg", name)))
        .size(px(size))
        .flex_none()
        .text_color(color)
}

/// A square, borderless icon button.
pub fn icon_button(id: impl Into<ElementId>, name: &str, t: &Theme) -> Stateful<Div> {
    let hover = t.hover;
    let active = t.active;
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(px(26.))
        .rounded(px(6.))
        .cursor_pointer()
        .hover(move |s| s.bg(hover))
        .active(move |s| s.bg(active))
        .child(icon(name, 16., t.text_muted))
}

pub fn popover_shadow(t: &Theme) -> Vec<BoxShadow> {
    let mut soft = t.shadow;
    soft.a *= 1.4;
    vec![
        BoxShadow {
            color: soft,
            offset: point(px(0.), px(14.)),
            blur_radius: px(28.),
            spread_radius: px(-6.),
        },
        BoxShadow {
            color: t.shadow,
            offset: point(px(0.), px(2.)),
            blur_radius: px(4.),
            spread_radius: px(-1.),
        },
    ]
}

pub fn card_shadow(t: &Theme) -> Vec<BoxShadow> {
    vec![
        BoxShadow {
            color: t.shadow,
            offset: point(px(0.), px(2.)),
            blur_radius: px(4.),
            spread_radius: px(0.),
        },
        BoxShadow {
            color: t.border,
            offset: point(px(0.), px(0.)),
            blur_radius: px(0.),
            spread_radius: px(1.),
        },
    ]
}

/// The surface of menus and popovers.
pub fn menu_surface(t: &Theme) -> Div {
    div()
        .flex()
        .flex_col()
        .bg(t.panel_bg)
        .border_1()
        .border_color(t.border)
        .rounded(px(10.))
        .shadow(popover_shadow(t))
        .text_size(px(14.))
        .text_color(t.text)
        .py(px(4.))
}

pub fn menu_item(id: impl Into<ElementId>, icon_name: Option<&str>, label: impl Into<SharedString>, t: &Theme) -> Stateful<Div> {
    let hover = t.hover;
    let mut row = div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .gap(px(8.))
        .h(px(28.))
        .mx(px(4.))
        .px(px(8.))
        .rounded(px(6.))
        .cursor_pointer()
        .hover(move |s| s.bg(hover));
    if let Some(name) = icon_name {
        row = row.child(icon(name, 16., t.text_muted));
    }
    row.child(
        div()
            .flex_1()
            .min_w_0()
            .whitespace_nowrap()
            .overflow_hidden()
            .text_ellipsis()
            .child(label.into()),
    )
}

pub fn menu_label(label: impl Into<SharedString>, t: &Theme) -> Div {
    div()
        .flex_none()
        .px(px(12.))
        .pt(px(6.))
        .pb(px(4.))
        .text_size(px(12.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(t.text_faint)
        .child(label.into())
}

pub fn menu_separator(t: &Theme) -> Div {
    div().flex_none().h(px(1.)).my(px(4.)).bg(t.border)
}

/// A coloured tag ("select option" pill).
pub fn pill(label: impl Into<SharedString>, color: &str, t: &Theme) -> Div {
    let (bg, fg) = t.tag(color);
    div()
        .flex_none()
        .h(px(20.))
        .px(px(6.))
        .rounded(px(3.))
        .bg(bg)
        .text_color(fg)
        .line_height(px(20.))
        .whitespace_nowrap()
        .max_w(px(220.))
        .overflow_hidden()
        .text_ellipsis()
        .child(label.into())
}

/// A status pill: rounded with a leading dot.
pub fn status_pill(label: impl Into<SharedString>, color: &str, t: &Theme) -> Div {
    let (bg, fg) = t.tag(color);
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(5.))
        .h(px(20.))
        .pl(px(7.))
        .pr(px(9.))
        .rounded(px(10.))
        .bg(bg)
        .text_color(fg)
        .line_height(px(20.))
        .whitespace_nowrap()
        .child(div().size(px(8.)).rounded(px(4.)).bg(t.tag_dot(color)))
        .child(label.into())
}

pub fn checkbox(checked: bool, t: &Theme) -> Div {
    let b = div()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(px(16.))
        .rounded(px(3.));
    if checked {
        b.bg(t.accent)
            .child(icon("check", 12., gpui::white()))
    } else {
        b.border_1().border_color(t.text_muted)
    }
}

/// Text button used in toolbars ("Filter", "Sort" …).
pub fn text_button(id: impl Into<ElementId>, label: impl Into<SharedString>, t: &Theme) -> Stateful<Div> {
    let hover = t.hover;
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .gap(px(6.))
        .h(px(28.))
        .px(px(8.))
        .rounded(px(6.))
        .text_size(px(14.))
        .text_color(t.text_muted)
        .cursor_pointer()
        .whitespace_nowrap()
        .hover(move |s| s.bg(hover))
        .child(label.into())
}

pub fn primary_button(id: impl Into<ElementId>, label: impl Into<SharedString>, t: &Theme) -> Stateful<Div> {
    let hover = t.accent_hover;
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .h(px(28.))
        .px(px(10.))
        .rounded(px(6.))
        .bg(t.accent)
        .text_color(gpui::white())
        .text_size(px(14.))
        .font_weight(FontWeight::MEDIUM)
        .cursor_pointer()
        .whitespace_nowrap()
        .hover(move |s| s.bg(hover))
        .child(label.into())
}

/// Float `content` at a window position, above everything else. Clicking
/// outside calls `on_dismiss`.
pub fn popover(
    position: Point<Pixels>,
    corner: Corner,
    content: impl IntoElement,
    on_dismiss: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    deferred(
        anchored()
            .position(position)
            .anchor(corner)
            .snap_to_window_with_margin(px(8.))
            .child(
                div()
                    .occlude()
                    .on_mouse_down_out(on_dismiss)
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(content),
            ),
    )
    .with_priority(1)
    .into_any_element()
}

/// The page glyph shown where a page has no emoji icon.
pub fn page_icon(emoji: Option<&str>, is_database: bool, size: f32, t: &Theme) -> AnyElement {
    match emoji {
        Some(e) if !e.is_empty() => div()
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .size(px(size + 4.))
            .text_size(px(size))
            .line_height(px(size + 4.))
            .child(SharedString::from(e.to_string()))
            .into_any_element(),
        _ => div()
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .size(px(size + 4.))
            .child(icon(
                if is_database { "database" } else { "page" },
                size + 2.,
                t.text_muted,
            ))
            .into_any_element(),
    }
}

pub const EMOJIS: &[&str] = &[
    "😀", "😄", "😊", "🙂", "😎", "🤔", "🥳", "😴", "👋", "👍", "🙏", "💪", "🔥", "✨", "⭐", "🌈", "☀️", "🌙",
    "🌱", "🌳", "🌸", "🍀", "🍎", "🍊", "🍋", "🍓", "🍜", "☕", "🍵", "🍰", "🏠", "🏫", "🏢", "✈️", "🚗", "🚲",
    "🗺️", "⛰️", "🏝️", "⚽", "🏀", "🏃", "🎮", "🎨", "🎵", "🎬", "📷", "🎤", "📚", "📖", "📝", "✏️", "📌", "📎",
    "📁", "🗂️", "📅", "📊", "📈", "💡", "💻", "⌨️", "🖥️", "📱", "🔍", "🔒", "🔑", "⚙️", "🧪", "🧠", "💼", "💰",
    "🎯", "🚀", "✅", "❌", "⚠️", "❓", "❗", "💬", "📣", "🔔", "❤️", "🧡", "💛", "💚", "💙", "💜", "🐱", "🐶",
];
