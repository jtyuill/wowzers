//! The window's contents.

use rune::{Cx, El, ScrollSpec, Spring, TextStyle, col, el, icon, row, text};
use rune_kit::{
    Button, ButtonVariant, Checkbox, IconButton, IconButtonVariant, Kit, SegmentedControl, Spinner,
    StateColors, TextInput, Toast, ToastRegion, glyphs,
};
use wowzers_core::{
    config::Appearance, curseforge::Mod, installed::InstalledAddon, manager::UpdateStatus,
};

use crate::{
    app::{Busy, NoteKind, SearchState, Tab, Wowzers},
    format::{compact_count, truncate},
    theme::{self, DOWNLOAD, MOON, REFRESH, SUN, TRASH},
};

const TABS: [(Tab, &str); 3] = [
    (Tab::Installed, "Installed"),
    (Tab::Find, "Find addons"),
    (Tab::Settings, "Settings"),
];

const APPEARANCES: [(Appearance, &str); 3] = [
    (Appearance::Auto, "Auto"),
    (Appearance::Light, "Light"),
    (Appearance::Dark, "Dark"),
];

const TITLE: TextStyle = TextStyle::new(17.0, 700.0);
const HEADING: TextStyle = TextStyle::new(14.0, 600.0);

pub fn view(s: &Wowzers, cx: &mut Cx<Wowzers>) -> El {
    let dark = s.config.appearance.is_dark(cx.system_dark());
    let k = theme::kit_for(dark);
    let mut body = col()
        .key("window")
        .w_full()
        .h_full()
        .fill(k.c.bg_base)
        .child(header(s, k, dark))
        .child(rule(k));
    if let Some(error) = &s.error {
        body = body.child(
            row()
                .key("error")
                .w_full()
                .px(16.0)
                .py(8.0)
                .fill(k.c.danger.bg)
                .child(text(error.clone()).font(k.ty.ui).color(k.c.danger.fg)),
        );
    }
    body = body.child(match s.tab {
        Tab::Installed => installed_tab(s, k),
        Tab::Find => find_tab(s, k),
        Tab::Settings => settings_tab(s, k),
    });
    rune_kit::root(body, [notes(s, k)])
}

fn header(s: &Wowzers, k: &Kit, dark: bool) -> El {
    let tab = TABS.iter().position(|(t, _)| *t == s.tab);
    let labels = TABS.map(|(_, l)| l);
    let tabs = SegmentedControl::new(&labels, tab)
        .label("Sections")
        .fit()
        .on_pick(|s: &mut Wowzers, i, _| s.open_tab(TABS[i].0))
        .build(k)
        .key("tabs");

    let mut bar = row()
        .key("header")
        .w_full()
        .h(52.0)
        .px(16.0)
        .gap(16.0)
        .center_y()
        .fill(k.c.layer_01)
        .child(
            text("wowzers")
                .key("title")
                .font(TITLE)
                .color(k.c.text_accent),
        )
        .child(tabs)
        .child(el().grow(1.0));

    let flavors: Vec<_> = s
        .current_install()
        .map(|i| i.flavors.iter().map(|f| f.flavor).collect())
        .unwrap_or_default();
    if !flavors.is_empty() {
        let labels: Vec<&str> = flavors.iter().map(|f| f.label()).collect();
        let selected = flavors.iter().position(|f| Some(*f) == s.selected_flavor);
        bar = bar.child(
            SegmentedControl::new(&labels, selected)
                .label("Game client")
                .fit()
                .on_pick(move |s: &mut Wowzers, i, _| s.select_flavor(flavors[i]))
                .build(k)
                .key("flavors"),
        );
    }

    // One click flips to the other theme; Settings also offers following the desktop.
    let (glyph, label, other) = if dark {
        (&SUN, "Switch to the light theme", Appearance::Light)
    } else {
        (&MOON, "Switch to the dark theme", Appearance::Dark)
    };
    bar.child(
        IconButton::new(glyph)
            .variant(IconButtonVariant::Ghost)
            .label(label)
            .help(label)
            .on_press_tx(move |s: &mut Wowzers, tx| s.set_appearance(other, tx))
            .build(k)
            .key("theme-toggle"),
    )
}

// ---- installed ------------------------------------------------------------

fn installed_tab(s: &Wowzers, k: &Kit) -> El {
    let Some(flavor) = s.current_flavor() else {
        return no_install(k);
    };
    let outdated = s.outdated().len();
    let idle = s.busy.is_none();
    let checking = s.busy == Some(Busy::Checking);

    let summary = match (&s.busy, s.addons.len()) {
        (Some(Busy::Scanning), _) => "Reading AddOns folder…".to_string(),
        (_, 0) => "No addons installed".to_string(),
        (_, n) if outdated > 0 => format!("{n} addons, {outdated} with updates"),
        (_, n) if s.statuses.is_empty() => format!("{n} addons"),
        (_, n) => format!("{n} addons, all up to date"),
    };

    let toolbar = row()
        .key("toolbar")
        .w_full()
        .px(16.0)
        .py(10.0)
        .gap(12.0)
        .center_y()
        .child(
            col()
                .grow(1.0)
                .min_w(0.0)
                .child(
                    text(summary)
                        .key("summary")
                        .font(HEADING)
                        .color(k.c.text_base),
                )
                .child(
                    text(flavor.addons_dir.display().to_string())
                        .key("addons-dir")
                        .font(k.ty.caption())
                        .color(k.c.text_muted)
                        .truncate_head(),
                ),
        )
        .child(
            Button::new(if checking {
                "Checking…"
            } else {
                "Check for updates"
            })
            .icon(&REFRESH)
            .disabled(!idle)
            .on_press(|s: &mut Wowzers| s.check())
            .build(k)
            .key("check"),
        )
        .child(
            Button::new(if outdated > 0 {
                format!("Update all ({outdated})")
            } else {
                "Update all".to_string()
            })
            .variant(ButtonVariant::Submit)
            .icon(&DOWNLOAD)
            .disabled(!idle || outdated == 0)
            .on_press(|s: &mut Wowzers| s.update_all())
            .build(k)
            .key("update-all"),
        );

    let mut list = col().key("addon-list").px(8.0).py(4.0);
    for (i, addon) in s.addons.iter().enumerate() {
        if i > 0 {
            list = list.child(hairline(k));
        }
        list = list.child(addon_row(s, k, addon, idle));
    }

    let mut tab = col()
        .key("installed")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .child(toolbar);
    if let Some(progress) = install_progress(s, k) {
        tab = tab.child(progress);
    }
    tab.child(rule(k)).child(scroller("addon-scroll", list))
}

fn addon_row(s: &Wowzers, k: &Kit, addon: &InstalledAddon, idle: bool) -> El {
    let status = s.status_of(addon);
    let details: Vec<String> = [
        addon.version.as_ref().map(|v| format!("v{v}")),
        (addon.folders.len() > 1).then(|| format!("{} folders", addon.folders.len())),
        addon.author.clone().filter(|a| !a.is_empty()),
    ]
    .into_iter()
    .flatten()
    .collect();
    let folder = addon.folders.first().cloned().unwrap_or_default();
    let confirming = s.confirm_remove.as_ref() == Some(&folder);

    let mut r = item_row(format!("addon-{folder}"), k).child(
        col()
            .grow(1.0)
            .min_w(0.0)
            .child(
                text(addon.name.clone())
                    .key("name")
                    .font(k.ty.ui_medium)
                    .color(k.c.text_base)
                    .truncate(),
            )
            .child(
                text(details.join("  ·  "))
                    .key("details")
                    .font(k.ty.caption())
                    .color(k.c.text_muted)
                    .truncate(),
            ),
    );
    if let Some(tag) = status_tag(s, k, addon, status) {
        r = r.child(tag);
    }
    if let Some(UpdateStatus::Outdated(candidate)) = status {
        let candidate = (**candidate).clone();
        r = r.child(
            Button::new("Update")
                .variant(ButtonVariant::Submit)
                .disabled(!idle)
                .on_press(move |s: &mut Wowzers| s.update_one(candidate.clone()))
                .build(k)
                .key("update"),
        );
    }
    let target = addon.clone();
    let remove = move |s: &mut Wowzers| s.remove(target.clone());
    r.child(if confirming {
        Button::new("Confirm remove")
            .variant(ButtonVariant::Danger)
            .disabled(!idle)
            .on_press(remove)
            .build(k)
            .key("remove")
    } else {
        IconButton::new(&TRASH)
            .variant(IconButtonVariant::Ghost)
            .label(format!("Remove {}", addon.name))
            .help("Remove")
            .disabled(!idle)
            .on_press(remove)
            .build(k)
            .key("remove")
    })
}

fn status_tag(
    s: &Wowzers,
    k: &Kit,
    addon: &InstalledAddon,
    status: Option<&UpdateStatus>,
) -> Option<El> {
    let flavor = s.selected_flavor.map_or("this client", |f| f.label());
    Some(match (status, addon.project_id) {
        (Some(UpdateStatus::Outdated(c)), _) => tag(
            k,
            format!("→ {}", truncate(&c.file.display_name, 32)),
            Some(k.c.warning),
        ),
        (Some(UpdateStatus::UpToDate), _) => tag(k, "Up to date".into(), Some(k.c.success)),
        (Some(UpdateStatus::NoCompatibleFile), _) => tag(k, format!("No {flavor} file"), None),
        (Some(UpdateStatus::NotFound), _) => tag(k, "Not on CurseForge".into(), None),
        (None, None) => tag(k, "Not from CurseForge".into(), None),
        (None, Some(_)) if s.busy == Some(Busy::Checking) => {
            Spinner::new().size(14.0).build(k).key("checking")
        }
        (None, Some(_)) => return None,
    })
}

fn no_install(k: &Kit) -> El {
    col()
        .key("no-install")
        .w_full()
        .grow(1.0)
        .center()
        .gap(12.0)
        .child(
            text("No World of Warcraft folder found")
                .font(HEADING)
                .color(k.c.text_base),
        )
        .child(
            text("Pick the folder that contains _retail_ or _classic_.")
                .font(k.ty.ui)
                .color(k.c.text_muted),
        )
        .child(
            Button::new("Choose folder…")
                .variant(ButtonVariant::Submit)
                .icon(&glyphs::FOLDER_OPEN)
                .on_press_tx(|s: &mut Wowzers, tx| s.browse(tx))
                .build(k)
                .key("browse-empty"),
        )
}

fn install_progress(s: &Wowzers, k: &Kit) -> Option<El> {
    let Some(Busy::Installing(label)) = &s.busy else {
        return None;
    };
    let (message, fraction) = match &s.progress {
        Some(p) => (
            format!("{} ({}/{})", p.message, p.done, p.total),
            p.done as f32 / p.total.max(1) as f32,
        ),
        None => (format!("Preparing {label}…"), 0.0),
    };
    Some(
        col()
            .key("install-progress")
            .w_full()
            .px(16.0)
            .pb(10.0)
            .gap(6.0)
            .child(text(message).font(k.ty.ui).color(k.c.text_muted))
            .child(
                el().key("track")
                    .w_full()
                    .h(6.0)
                    .radius(3.0)
                    .fill(k.c.layer_03)
                    .child(
                        el().key("bar")
                            .w_frac(fraction.clamp(0.0, 1.0))
                            .h(6.0)
                            .radius(3.0)
                            .fill(k.c.bg_accent)
                            .motion(Spring::new(0.4, 0.0)),
                    ),
            ),
    )
}

// ---- find -----------------------------------------------------------------

fn find_tab(s: &Wowzers, k: &Kit) -> El {
    // The button only waits on the query that's in the box; editing it allows a new search.
    let query = s.search.text.trim();
    let searching =
        matches!(&s.search_state, SearchState::Searching { query: q } if q == query);
    let can_install = s.busy.is_none() && s.current_flavor().is_some();
    let (status_line, failed) = match &s.search_state {
        SearchState::Idle => ("Search by addon name or author.".to_string(), false),
        SearchState::Searching { query } if query.is_empty() => {
            ("Loading popular addons…".to_string(), false)
        }
        SearchState::Searching { .. } => ("Searching…".to_string(), false),
        SearchState::Results { query, .. } if query.is_empty() => {
            ("Most popular addons".to_string(), false)
        }
        SearchState::Results { query, total } => (format!("{total} results for “{query}”"), false),
        SearchState::Failed(e) => (e.clone(), true),
    };

    let bar = row()
        .key("search-bar")
        .w_full()
        .px(16.0)
        .pt(12.0)
        .gap(8.0)
        .center_y()
        .child(
            el().grow(1.0).min_w(0.0).child(
                TextInput::new(&s.search)
                    .placeholder("Search CurseForge addons")
                    .width(None)
                    .label("Search")
                    .leading_icon(&glyphs::SEARCH)
                    .on_edit(|s: &mut Wowzers| &mut s.search)
                    .on_submit(|s: &mut Wowzers, _| s.search())
                    .build(k)
                    .key("search-input"),
            ),
        )
        .child(
            Button::new(if searching { "Searching…" } else { "Search" })
                .variant(ButtonVariant::Submit)
                .disabled(searching)
                .on_press(|s: &mut Wowzers| s.search())
                .build(k)
                .key("search"),
        );

    let mut list = col().key("result-list").px(8.0).py(4.0);
    for (i, m) in s.search_results.iter().enumerate() {
        if i > 0 {
            list = list.child(hairline(k));
        }
        list = list.child(result_row(s, k, m, can_install));
    }

    let mut tab = col()
        .key("find")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .child(bar)
        .child(
            text(status_line)
                .key("search-status")
                .px(16.0)
                .py(8.0)
                .font(k.ty.caption())
                .color(if failed {
                    k.c.danger.fg
                } else {
                    k.c.text_muted
                }),
        );
    if let Some(progress) = install_progress(s, k) {
        tab = tab.child(progress);
    }
    tab.child(rule(k)).child(scroller("result-scroll", list))
}

fn result_row(s: &Wowzers, k: &Kit, m: &Mod, can_install: bool) -> El {
    let byline = format!(
        "by {}  ·  {} downloads",
        m.author_names(),
        compact_count(m.download_count)
    );
    let r = item_row(format!("result-{}", m.id), k).child(
        col()
            .grow(1.0)
            .min_w(0.0)
            .child(
                row()
                    .gap(8.0)
                    .min_w(0.0)
                    .child(
                        text(m.name.clone())
                            .key("name")
                            .font(k.ty.ui_medium)
                            .color(k.c.text_base)
                            .truncate(),
                    )
                    .child(
                        text(byline)
                            .key("byline")
                            .font(k.ty.caption())
                            .color(k.c.text_muted)
                            .truncate(),
                    ),
            )
            .child(
                text(truncate(&m.summary, 160))
                    .key("summary")
                    .font(k.ty.ui)
                    .color(k.c.text_muted)
                    .truncate(),
            ),
    );
    if s.is_installed(m.id) {
        return r.child(tag(k, "Installed".into(), Some(k.c.success)));
    }
    let target = m.clone();
    r.child(
        Button::new("Install")
            .variant(ButtonVariant::Submit)
            .icon(&DOWNLOAD)
            .disabled(!can_install)
            .on_press(move |s: &mut Wowzers| s.install(target.clone()))
            .build(k)
            .key("install"),
    )
}

// ---- settings -------------------------------------------------------------

fn settings_tab(s: &Wowzers, k: &Kit) -> El {
    let appearance = APPEARANCES
        .iter()
        .position(|(a, _)| *a == s.config.appearance);
    let labels = APPEARANCES.map(|(_, l)| l);
    let theme = section(
        k,
        "appearance-section",
        "Appearance",
        "Light and dark themes; Auto follows your desktop.",
    )
    .child(
        SegmentedControl::new(&labels, appearance)
            .label("Appearance")
            .width(Some(240.0))
            .on_pick(|s: &mut Wowzers, i, tx| s.set_appearance(APPEARANCES[i].0, tx))
            .build(k)
            .key("appearance"),
    );

    let mut folders = section(
        k,
        "folders-section",
        "World of Warcraft folders",
        if s.installs.is_empty() {
            "None found automatically. Choose the folder that contains _retail_ or _classic_."
        } else {
            "Found automatically or added by you."
        },
    );
    for install in &s.installs {
        let root = install.root.clone();
        let current = s.selected_root.as_ref() == Some(&install.root);
        let flavors = install
            .flavors
            .iter()
            .map(|f| f.flavor.label())
            .collect::<Vec<_>>()
            .join(", ");
        let key = format!("install-{}", install.root.display());
        folders = folders.child(
            row()
                .key(key)
                .w_full()
                .gap(12.0)
                .center_y()
                .child(
                    text(install.root.display().to_string())
                        .font(k.ty.ui)
                        .color(k.c.text_base)
                        .grow(1.0)
                        .min_w(0.0)
                        .truncate_head(),
                )
                .child(text(flavors).font(k.ty.caption()).color(k.c.text_muted))
                .child(if current {
                    tag(k, "In use".into(), Some(k.c.success))
                } else {
                    Button::new("Use")
                        .variant(ButtonVariant::Outline)
                        .on_press(move |s: &mut Wowzers| s.select_install(root.clone()))
                        .build(k)
                        .key("use")
                }),
        );
    }
    folders = folders.child(
        row()
            .key("add-folder")
            .w_full()
            .gap(8.0)
            .center_y()
            .child(
                el().grow(1.0).min_w(0.0).child(
                    TextInput::new(&s.path)
                        .placeholder("/path/to/World of Warcraft")
                        .width(None)
                        .label("World of Warcraft folder")
                        .on_edit(|s: &mut Wowzers| &mut s.path)
                        .on_submit(|s: &mut Wowzers, tx| s.add_typed_path(tx))
                        .build(k)
                        .key("path-input"),
                ),
            )
            .child(
                Button::new("Add")
                    .variant(ButtonVariant::Outline)
                    .on_press_tx(|s: &mut Wowzers, tx| s.add_typed_path(tx))
                    .build(k)
                    .key("add-path"),
            )
            .child(
                Button::new("Browse…")
                    .variant(ButtonVariant::Outline)
                    .icon(&glyphs::FOLDER_OPEN)
                    .on_press_tx(|s: &mut Wowzers, tx| s.browse(tx))
                    .build(k)
                    .key("browse"),
            ),
    );

    let channel = section(
        k,
        "channel-section",
        "Release channel",
        "Stable releases only, unless you opt in.",
    )
    .child(
        Checkbox::new(s.config.allow_beta)
            .label("Also install beta releases")
            .on_toggle(|s: &mut Wowzers| s.set_allow_beta(!s.config.allow_beta))
            .build(k)
            .key("allow-beta"),
    )
    .child(
        Checkbox::new(s.config.allow_alpha)
            .label("Also install alpha releases")
            .on_toggle(|s: &mut Wowzers| s.set_allow_alpha(!s.config.allow_alpha))
            .build(k)
            .key("allow-alpha"),
    );

    let content = col()
        .key("settings")
        .w_full()
        .max_w(760.0)
        .padding(24.0)
        .gap(28.0)
        .child(theme)
        .child(folders)
        .child(channel);
    scroller("settings-scroll", content)
}

fn section(k: &Kit, key: &str, title: &str, hint: &str) -> El {
    col()
        .key(key)
        .w_full()
        .gap(8.0)
        .child(text(title).font(HEADING).color(k.c.text_base))
        .child(text(hint).font(k.ty.ui).color(k.c.text_muted))
}

// ---- pieces ---------------------------------------------------------------

/// A vertical scroll viewport filling the rest of the tab.
fn scroller(key: &str, content: El) -> El {
    col()
        .key(key)
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .scroll(ScrollSpec {
            stick_bottom: false,
        })
        .child(content.w_full())
}

fn item_row(key: String, k: &Kit) -> El {
    row()
        .key(key)
        .w_full()
        .px(10.0)
        .py(8.0)
        .gap(12.0)
        .center_y()
        .radius(k.radius.md)
        .hover(k.c.hover)
}

/// A small rounded label; `None` paints it neutral.
fn tag(k: &Kit, label: String, role: Option<StateColors>) -> El {
    let (fill, border, ink) = match role {
        Some(c) => (c.bg, c.border, c.fg),
        None => (k.c.layer_02, k.c.border_base, k.c.text_muted),
    };
    row()
        .key("tag")
        .h(22.0)
        .px(9.0)
        .radius(11.0)
        .center_y()
        .fill(fill)
        .border(1.0, border)
        .child(text(label).font(k.ty.small_medium).color(ink))
}

fn rule(k: &Kit) -> El {
    el().w_full().h(1.0).fill(k.c.border_base).a11y_hidden()
}

fn hairline(k: &Kit) -> El {
    el().h(1.0).mx(10.0).fill(k.c.border_muted).a11y_hidden()
}

fn notes(s: &Wowzers, k: &Kit) -> El {
    let mut region = ToastRegion::new();
    for note in &s.notes {
        let id = note.id;
        let (glyph, ink) = match note.kind {
            NoteKind::Success => (&glyphs::CHECK_SMALL, k.c.success.icon),
            NoteKind::Error => (&glyphs::CIRCLE_X, k.c.danger.icon),
        };
        region = region.toast(
            Toast::new(format!("note-{id}"), note.text.clone())
                .lead(icon(glyph, 16.0).color(ink))
                .on_close(move |s: &mut Wowzers| s.dismiss_note(id))
                .build(k),
        );
    }
    region.build(k)
}
