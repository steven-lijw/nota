#![allow(dead_code)]
//! Nota — a local, Notion-style notes app built with Rust and GPUI.

mod actions;
mod ai;
mod assets;
mod blocks_view;
mod database;
mod edit;
mod editor;
mod editor_render;
mod page_view;
mod props;
mod store;
mod text_input;
mod text_view;
mod theme;
mod ui;
mod workspace;

use gpui::{
    point, prelude::*, px, size, App, Application, Bounds, Menu, MenuItem, TitlebarOptions, WindowBounds,
    WindowOptions,
};
use std::path::{Path, PathBuf};

fn config_file() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("nota").join("config.json"))
}

/// Remember which folder is the vault.
pub fn save_vault_path(path: &Path) {
    if let Some(file) = config_file() {
        if let Some(dir) = file.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let json = serde_json::json!({ "vault": path.to_string_lossy() });
        let _ = std::fs::write(file, serde_json::to_string_pretty(&json).unwrap_or_default());
    }
}

/// Vault folder: first CLI argument, then `NOTA_VAULT`, then the saved
/// choice, then `~/Nota`.
fn vault_path() -> PathBuf {
    if let Some(arg) = std::env::args().nth(1).filter(|a| !a.starts_with('-')) {
        return PathBuf::from(arg);
    }
    if let Some(env) = std::env::var_os("NOTA_VAULT").filter(|v| !v.is_empty()) {
        return PathBuf::from(env);
    }
    let saved = config_file()
        .and_then(|f| std::fs::read_to_string(f).ok())
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .and_then(|v| v["vault"].as_str().map(PathBuf::from));
    saved.unwrap_or_else(|| dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")).join("Nota"))
}

struct StderrLogger;

impl log::Log for StderrLogger {
    fn enabled(&self, m: &log::Metadata) -> bool {
        m.level() <= log::Level::Warn
    }
    fn log(&self, r: &log::Record) {
        if self.enabled(r.metadata()) {
            eprintln!("[{}] {}: {}", r.level(), r.target(), r.args());
        }
    }
    fn flush(&self) {}
}

static LOGGER: StderrLogger = StderrLogger;

fn main() {
    let _ = log::set_logger(&LOGGER).map(|()| log::set_max_level(log::LevelFilter::Warn));
    let root = vault_path();
    let vault = match nota_core::Vault::open(&root) {
        Ok(v) => v,
        Err(err) => {
            eprintln!("Nota could not open the vault at {}: {err:#}", root.display());
            std::process::exit(1);
        }
    };

    Application::new().with_assets(assets::Assets).run(move |cx: &mut App| {
        actions::bind_keys(cx);
        cx.on_action(|_: &actions::Quit, cx| cx.quit());
        cx.set_menus(vec![
            Menu {
                name: "Nota".into(),
                items: vec![
                    MenuItem::action("Toggle Dark Mode", actions::ToggleTheme),
                    MenuItem::separator(),
                    MenuItem::action("Quit Nota", actions::Quit),
                ],
            },
            Menu {
                name: "File".into(),
                items: vec![
                    MenuItem::action("New Page", actions::NewPage),
                    MenuItem::action("Search…", actions::QuickFind),
                ],
            },
            Menu {
                name: "Edit".into(),
                items: vec![
                    MenuItem::action("Undo", actions::Undo),
                    MenuItem::action("Redo", actions::Redo),
                    MenuItem::separator(),
                    MenuItem::action("Cut", actions::Cut),
                    MenuItem::action("Copy", actions::Copy),
                    MenuItem::action("Paste", actions::Paste),
                    MenuItem::action("Select All", actions::SelectAll),
                ],
            },
            Menu {
                name: "View".into(),
                items: vec![
                    MenuItem::action("Toggle Sidebar", actions::ToggleSidebar),
                    MenuItem::action("Toggle AI Panel", actions::ToggleAi),
                    MenuItem::separator(),
                    MenuItem::action("Back", actions::GoBack),
                    MenuItem::action("Forward", actions::GoForward),
                ],
            },
        ]);
        cx.on_window_closed(|cx| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();

        let store = cx.new(|cx| store::Store::new(vault, cx));
        cx.set_global(theme::Theme::get(store.read(cx).state.dark));
        let bounds = Bounds::centered(None, size(px(1320.), px(860.)), cx);
        let opened = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(size(px(760.), px(480.))),
                titlebar: Some(TitlebarOptions {
                    title: Some("Nota".into()),
                    appears_transparent: true,
                    traffic_light_position: Some(point(px(14.), px(14.))),
                }),
                ..Default::default()
            },
            |window, cx| cx.new(|cx| workspace::Workspace::new(store.clone(), window, cx)),
        );
        if let Err(err) = opened {
            eprintln!("Nota could not open a window: {err:#}");
            cx.quit();
        }
        cx.activate(true);
    });
}
