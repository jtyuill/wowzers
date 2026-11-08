//! Light and dark themes, and the app's own icons.

use std::sync::LazyLock;

use rune::Icon;
use rune_kit::Kit;

static LIGHT: LazyLock<Kit> = LazyLock::new(|| Kit::neutral(false));
static DARK: LazyLock<Kit> = LazyLock::new(|| Kit::neutral(true));

/// The dark or light theme.
pub fn kit_for(dark: bool) -> &'static Kit {
    if dark { &DARK } else { &LIGHT }
}

pub static SUN: Icon = Icon {
    name: "wowzers-sun",
    view: 16.0,
    body: r#"<circle cx="8" cy="8" r="3" stroke="currentColor"/><path d="M8 1.5V3M8 13V14.5M1.5 8H3M13 8H14.5M3.4 3.4L4.45 4.45M11.55 11.55L12.6 12.6M3.4 12.6L4.45 11.55M11.55 4.45L12.6 3.4" stroke="currentColor" stroke-linecap="round"/>"#,
};

pub static MOON: Icon = Icon {
    name: "wowzers-moon",
    view: 16.0,
    body: r#"<path d="M13.5 9.6A5.6 5.6 0 1 1 6.4 2.5A4.6 4.6 0 0 0 13.5 9.6Z" stroke="currentColor" stroke-linejoin="round"/>"#,
};

pub static REFRESH: Icon = Icon {
    name: "wowzers-refresh",
    view: 16.0,
    body: r#"<path d="M13.5 8A5.5 5.5 0 1 1 11.9 4.1" stroke="currentColor" stroke-linecap="round"/><path d="M12.5 1.5V4.5H9.5" stroke="currentColor" stroke-linecap="round" stroke-linejoin="round"/>"#,
};

pub static DOWNLOAD: Icon = Icon {
    name: "wowzers-download",
    view: 16.0,
    body: r#"<path d="M8 2.5V10.5M4.5 7L8 10.5L11.5 7M3 13.5H13" stroke="currentColor" stroke-linecap="round" stroke-linejoin="round"/>"#,
};

pub static TRASH: Icon = Icon {
    name: "wowzers-trash",
    view: 16.0,
    body: r#"<path d="M2.5 4.5H13.5M6 4.5V2.5H10V4.5M4 4.5L4.75 13.5H11.25L12 4.5" stroke="currentColor" stroke-linejoin="round"/>"#,
};
