//! Embedded icon set. Icons are drawn on a 16×16 grid and rendered through
//! GPUI's SVG pipeline, tinted with the element's text colour.

use gpui::{AssetSource, Result, SharedString};
use std::borrow::Cow;

/// (name, inner svg markup). Stroke icons unless marked with `fill`.
const ICONS: &[(&str, &str)] = &[
    ("chevron-right", r#"<path d="M6 3.5 10.5 8 6 12.5"/>"#),
    ("chevron-down", r#"<path d="M3.5 6 8 10.5 12.5 6"/>"#),
    ("chevron-left", r#"<path d="M10 3.5 5.5 8 10 12.5"/>"#),
    ("chevrons-left", r#"<path d="M8 4 4 8l4 4M12.5 4l-4 4 4 4"/>"#),
    ("chevrons-right", r#"<path d="M8 4l4 4-4 4M3.5 4l4 4-4 4"/>"#),
    ("triangle-right", r#"<path d="M5.5 3.6 11.2 8l-5.7 4.4Z" fill="black" stroke="none"/>"#),
    ("triangle-down", r#"<path d="M3.6 5.5 8 11.2l4.4-5.7Z" fill="black" stroke="none"/>"#),
    ("plus", r#"<path d="M8 3v10M3 8h10"/>"#),
    ("search", r#"<circle cx="7" cy="7" r="4.25"/><path d="M10.2 10.2 13.5 13.5"/>"#),
    ("home", r#"<path d="M2.5 7.4 8 2.5l5.5 4.9V13a.5.5 0 0 1-.5.5h-3v-4H6v4H3a.5.5 0 0 1-.5-.5Z"/>"#),
    ("trash", r#"<path d="M2.75 4.5h10.5M6.25 4.5V3h3.5v1.5M4.25 4.5l.5 8.5h6.5l.5-8.5M6.75 7v3.75M9.25 7v3.75"/>"#),
    ("settings", r#"<path d="M2.5 5h6M12.5 5h1M2.5 11h1M7.5 11h6"/><circle cx="10.5" cy="5" r="1.6"/><circle cx="5.5" cy="11" r="1.6"/>"#),
    ("more", r#"<g fill="black" stroke="none"><circle cx="3.5" cy="8" r="1.15"/><circle cx="8" cy="8" r="1.15"/><circle cx="12.5" cy="8" r="1.15"/></g>"#),
    ("grip", r#"<g fill="black" stroke="none"><circle cx="5.75" cy="3.75" r="1.05"/><circle cx="10.25" cy="3.75" r="1.05"/><circle cx="5.75" cy="8" r="1.05"/><circle cx="10.25" cy="8" r="1.05"/><circle cx="5.75" cy="12.25" r="1.05"/><circle cx="10.25" cy="12.25" r="1.05"/></g>"#),
    ("star", r#"<path d="M8 2.2l1.75 3.6 3.95.55-2.85 2.8.67 3.95L8 11.25 4.48 13.1l.67-3.95L2.3 6.35l3.95-.55Z"/>"#),
    ("star-filled", r#"<path d="M8 2.2l1.75 3.6 3.95.55-2.85 2.8.67 3.95L8 11.25 4.48 13.1l.67-3.95L2.3 6.35l3.95-.55Z" fill="black"/>"#),
    ("page", r#"<path d="M4 2.5h5.25L12.5 5.75V13a.5.5 0 0 1-.5.5H4a.5.5 0 0 1-.5-.5V3a.5.5 0 0 1 .5-.5Z"/><path d="M9.25 2.5v3.25h3.25"/><path d="M5.75 8.75h4.5M5.75 11h4.5"/>"#),
    ("page-empty", r#"<path d="M4 2.5h5.25L12.5 5.75V13a.5.5 0 0 1-.5.5H4a.5.5 0 0 1-.5-.5V3a.5.5 0 0 1 .5-.5Z"/><path d="M9.25 2.5v3.25h3.25"/>"#),
    ("table", r#"<rect x="2.5" y="3" width="11" height="10" rx="1.5"/><path d="M2.5 6.5h11M6.5 6.5V13"/>"#),
    ("board", r#"<rect x="2.5" y="3" width="3" height="10" rx=".75"/><rect x="6.5" y="3" width="3" height="6.5" rx=".75"/><rect x="10.5" y="3" width="3" height="8.5" rx=".75"/>"#),
    ("list", r#"<path d="M6 4.5h7.5M6 8h7.5M6 11.5h7.5"/><g fill="black" stroke="none"><circle cx="3" cy="4.5" r=".9"/><circle cx="3" cy="8" r=".9"/><circle cx="3" cy="11.5" r=".9"/></g>"#),
    ("gallery", r#"<rect x="2.5" y="2.5" width="4.5" height="4.5" rx="1"/><rect x="9" y="2.5" width="4.5" height="4.5" rx="1"/><rect x="2.5" y="9" width="4.5" height="4.5" rx="1"/><rect x="9" y="9" width="4.5" height="4.5" rx="1"/>"#),
    ("calendar", r#"<rect x="2.5" y="3.5" width="11" height="9.5" rx="1.5"/><path d="M2.5 6.75h11M5.5 2v2.5M10.5 2v2.5"/>"#),
    ("sparkle", r#"<path d="M8 1.5C8.4 5 9.5 7 14.5 8 9.5 9 8.4 11 8 14.5 7.6 11 6.5 9 1.5 8 6.5 7 7.6 5 8 1.5Z" fill="black" stroke="none"/>"#),
    ("sparkles", r#"<path d="M6.5 3C6.8 5.8 7.7 7.2 11.5 8 7.7 8.8 6.8 10.2 6.5 13 6.2 10.2 5.3 8.8 1.5 8 5.3 7.2 6.2 5.8 6.5 3Z" fill="black" stroke="none"/><path d="M12.25 1.5c.15 1.3.55 1.95 2.25 2.25-1.7.3-2.1.95-2.25 2.25-.15-1.3-.55-1.95-2.25-2.25 1.7-.3 2.1-.95 2.25-2.25Z" fill="black" stroke="none"/>"#),
    ("arrow-up", r#"<path d="M8 12.5v-9M4 7.5l4-4 4 4"/>"#),
    ("arrow-down", r#"<path d="M8 3.5v9M4 8.5l4 4 4-4"/>"#),
    ("arrow-up-right", r#"<path d="M4.75 11.25 11 5M5.75 4.75h5.5v5.5"/>"#),
    ("x", r#"<path d="M4 4l8 8M12 4l-8 8"/>"#),
    ("check", r#"<path d="M3 8.5 6.5 12 13 4.5"/>"#),
    ("clock", r#"<circle cx="8" cy="8" r="5.5"/><path d="M8 5v3.25l2 1.25"/>"#),
    ("filter", r#"<path d="M2.5 4.5h11M4.5 8h7M6.5 11.5h3"/>"#),
    ("sort", r#"<path d="M5 3v10M2.75 10.5 5 13l2.25-2.5M11 13V3M8.75 5.5 11 3l2.25 2.5"/>"#),
    ("text", r#"<path d="M3 4.5h10M3 8h10M3 11.5h6"/>"#),
    ("title", r#"<path d="M2.5 12.5 5.6 4h.6l3.1 8.5M3.7 9.6h4.4"/><path d="M13.5 12.5v-3c0-1.2-.7-1.9-1.8-1.9-.8 0-1.4.3-1.8.8M13.5 10.2c-2.2-.3-3.2.3-3.2 1.2 0 1.4 2.4 1.5 3.2.1"/>"#),
    ("hash", r#"<path d="M6.5 2.5 5 13.5M11 2.5 9.5 13.5M3 6h10.5M2.5 10H13"/>"#),
    ("select", r#"<circle cx="8" cy="8" r="5.5"/><path d="M5.5 7 8 9.5 10.5 7"/>"#),
    ("status", r#"<circle cx="8" cy="8" r="5.5" stroke-dasharray="2.2 2"/><circle cx="8" cy="8" r="2" fill="black" stroke="none"/>"#),
    ("checkbox", r#"<rect x="2.75" y="2.75" width="10.5" height="10.5" rx="2"/><path d="M5.25 8.25l2 2 3.5-4.5"/>"#),
    ("link", r#"<path d="M6.75 9.25l2.5-2.5M7.5 4.75l1-1a2.65 2.65 0 0 1 3.75 3.75l-1 1M8.5 11.25l-1 1a2.65 2.65 0 0 1-3.75-3.75l1-1"/>"#),
    ("copy", r#"<rect x="5.5" y="5.5" width="8" height="8" rx="1.5"/><path d="M10.5 5.5V4A1.5 1.5 0 0 0 9 2.5H4A1.5 1.5 0 0 0 2.5 4v5A1.5 1.5 0 0 0 4 10.5h1.5"/>"#),
    ("edit", r#"<path d="M13.5 8.75V12a1.5 1.5 0 0 1-1.5 1.5H4A1.5 1.5 0 0 1 2.5 12V4A1.5 1.5 0 0 1 4 2.5h3.25"/><path d="M11.4 2.6l2 2L8 10H6V8Z"/>"#),
    ("sidebar", r#"<rect x="2.5" y="3" width="11" height="10" rx="1.5"/><path d="M6.25 3v10"/>"#),
    ("panel-right", r#"<rect x="2.5" y="3" width="11" height="10" rx="1.5"/><path d="M9.75 3v10"/>"#),
    ("undo", r#"<path d="M5.5 3.5l-3 3 3 3M2.5 6.5h7a3.5 3.5 0 0 1 0 7H6.5"/>"#),
    ("turn", r#"<path d="M3 5.5h7.5a2.75 2.75 0 0 1 0 5.5H6.5M5.25 3.25 3 5.5l2.25 2.25M8.5 8.75 6.25 11l2.25 2.25"/>"#),
    ("code", r#"<path d="M5.5 4.5 2.5 8l3 3.5M10.5 4.5l3 3.5-3 3.5"/>"#),
    ("quote", r#"<path d="M3.5 3v10" stroke-width="2"/><path d="M7 5h6.5M7 8h6.5M7 11h4"/>"#),
    ("divider", r#"<path d="M2.5 8h11"/>"#),
    ("callout", r#"<rect x="2.5" y="3.5" width="11" height="9" rx="1.5"/><path d="M5.25 6.5h.01M7.5 6.5h3.5M5.25 9.5h5.75"/>"#),
    ("numbered", r#"<path d="M7 4.5h6.5M7 8h6.5M7 11.5h6.5"/><path d="M2.75 3.75 3.75 3.25V6M2.75 9.6c.2-.5.6-.75 1.1-.75.6 0 1 .35 1 .85 0 .9-1.6 1.4-2.1 2.55h2.2" stroke-width="1"/>"#),
    ("moon", r#"<path d="M13 9.5A5.5 5.5 0 0 1 6.5 3a5.5 5.5 0 1 0 6.5 6.5Z"/>"#),
    ("sun", r#"<circle cx="8" cy="8" r="2.75"/><path d="M8 1.75v1.5M8 12.75v1.5M1.75 8h1.5M12.75 8h1.5M3.6 3.6l1.05 1.05M11.35 11.35l1.05 1.05M3.6 12.4l1.05-1.05M11.35 4.65l1.05-1.05"/>"#),
    ("eye-off", r#"<path d="M2 8s2.25-4 6-4 6 4 6 4-2.25 4-6 4-6-4-6-4Z"/><circle cx="8" cy="8" r="1.75"/><path d="M3 13 13 3"/>"#),
    ("eye", r#"<path d="M2 8s2.25-4 6-4 6 4 6 4-2.25 4-6 4-6-4-6-4Z"/><circle cx="8" cy="8" r="1.75"/>"#),
    ("stop", r#"<rect x="4.5" y="4.5" width="7" height="7" rx="1.5" fill="black" stroke="none"/>"#),
    ("insert", r#"<path d="M8 2.5v7M5 6.75l3 3 3-3M3 13h10"/>"#),
    ("folder", r#"<path d="M2.5 4.5a1 1 0 0 1 1-1h2.75l1.5 1.5h4.75a1 1 0 0 1 1 1V12a1 1 0 0 1-1 1h-9a1 1 0 0 1-1-1Z"/>"#),
    ("smile", r#"<circle cx="8" cy="8" r="5.5"/><path d="M5.75 9.5c.5.8 1.3 1.25 2.25 1.25s1.75-.45 2.25-1.25M6 6.5h.01M10 6.5h.01"/>"#),
    ("database", r#"<rect x="2.5" y="3" width="11" height="10" rx="1.5"/><path d="M2.5 6.5h11M2.5 9.75h11M6.5 6.5V13"/>"#),
    ("refresh", r#"<path d="M13 8a5 5 0 0 1-9.2 2.7M3 8a5 5 0 0 1 9.2-2.7M12.5 2.75v2.75H9.75M3.5 13.25V10.5h2.75"/>"#),
    ("bold", r#"<path d="M4.5 3h4.25a2.5 2.5 0 0 1 0 5H4.5ZM4.5 8h4.75a2.5 2.5 0 0 1 0 5H4.5Z" stroke-width="1.6"/>"#),
    ("italic", r#"<path d="M7 3h5M4 13h5M9.75 3 6.25 13"/>"#),
    ("underline", r#"<path d="M4.5 3v4.5a3.5 3.5 0 0 0 7 0V3M3.5 13.5h9"/>"#),
    ("strike", r#"<path d="M2.5 8h11M11.25 5.25C11 4 9.75 3 8 3 6.2 3 5 4 5 5.3c0 .9.5 1.6 1.4 2M5 10.9C5.3 12.2 6.5 13 8 13c1.8 0 3-.9 3-2.3 0-.5-.1-.9-.4-1.2"/>"#),
];

pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        let name = path
            .trim_start_matches("icons/")
            .trim_end_matches(".svg");
        Ok(ICONS.iter().find(|(n, _)| *n == name).map(|(_, inner)| {
            let svg = format!(
                r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16" fill="none" stroke="black" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round">{}</svg>"#,
                inner
            );
            Cow::Owned(svg.into_bytes())
        }))
    }

    fn list(&self, _path: &str) -> Result<Vec<SharedString>> {
        Ok(ICONS
            .iter()
            .map(|(n, _)| format!("icons/{}.svg", n).into())
            .collect())
    }
}
