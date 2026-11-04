//! wowzers: a World of Warcraft addon manager over the CurseForge API.

mod app;
mod format;
mod render;
#[cfg(test)]
mod ui_tests;

use gpui_kit::{
    App, AppContext as _, Bounds, WindowBounds, WindowOptions, assets::AllAssets, px, size,
};
use tracing_subscriber::EnvFilter;

/// Wayland app id / X11 WM_CLASS.
const APP_ID: &str = "dev.jacobyuill.wowzers";

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    gpui_kit::application()
        .with_assets(AllAssets)
        .run(|cx: &mut App| {
            gpui_kit::init(cx);
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                    None,
                    size(px(980.), px(720.)),
                    cx,
                ))),
                window_min_size: Some(size(px(640.), px(420.))),
                app_id: Some(APP_ID.into()),
                ..Default::default()
            };
            let opened = gpui_kit::open_window(options, cx, |window, cx| {
                cx.new(|cx| app::Wowzers::new(app::Options::from_env(), window, cx))
            });
            if let Err(e) = opened {
                tracing::error!("opening the window failed: {e:#}");
                cx.quit();
                return;
            }
            cx.on_window_closed(|cx, _| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();
            cx.activate(true);
        });
}
