//! wowzers: a World of Warcraft addon manager backed by the CurseForge site.

mod app;
mod format;
mod render;
mod theme;
#[cfg(test)]
mod ui_tests;

use rune::{App, AppSpec};
use tracing_subscriber::EnvFilter;
use wowzers_core::config::Appearance;

use crate::app::{Options, Tab, Wowzers};

/// Wayland app id / X11 WM_CLASS.
const APP_ID: &str = "dev.jacobyuill.wowzers";

fn main() {
    rune::prewarm_gpu();
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();
    rune::set_app_id(APP_ID);

    App::new(AppSpec {
        min_size: Some((640.0, 420.0)),
        setup: Some(app::setup),
        poll: Some(app::poll),
        scenes: &[
            ("light", |s| s.config.appearance = Appearance::Light),
            ("dark", |s| s.config.appearance = Appearance::Dark),
            ("find", |s| s.tab = Tab::Find),
            ("settings", |s| s.tab = Tab::Settings),
        ],
        ..AppSpec::new(
            "wowzers",
            (980.0, 720.0),
            || Wowzers::new(Options::from_env()),
            render::view,
        )
    })
    .run();
}
