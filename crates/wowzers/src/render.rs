//! The window's contents.

use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, IconName, Selectable as _, Sizable as _, StyledExt as _,
    button::{Button, ButtonGroup, ButtonVariants as _},
    checkbox::Checkbox,
    h_flex,
    input::Input,
    progress::Progress,
    spinner::Spinner,
    tag::Tag,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Context, InteractiveElement as _, IntoElement, ParentElement as _, Render,
    SharedString, StatefulInteractiveElement as _, Styled as _, Window, div, px,
};
use wowzers_core::{
    curseforge::Mod, flavor::Flavor, installed::InstalledAddon, manager::UpdateStatus,
};

use crate::{
    app::{Busy, SearchState, Tab, Wowzers},
    format::{compact_count, truncate},
};

const CURSEFORGE_KEYS_URL: &str = "https://console.curseforge.com/#/api-keys";

impl Render for Wowzers {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        v_flex()
            .size_full()
            .bg(theme.background)
            .text_color(theme.foreground)
            .child(self.header(cx))
            .when_some(self.error.clone(), |this, error| {
                this.child(
                    div()
                        .px_4()
                        .py_2()
                        .bg(theme.danger.opacity(0.15))
                        .text_color(theme.danger)
                        .text_sm()
                        .child(error),
                )
            })
            .when(!self.has_api_key() && self.tab != Tab::Settings, |this| {
                this.child(self.key_banner(cx))
            })
            .child(match self.tab {
                Tab::Installed => self.installed_tab(cx),
                Tab::Find => self.find_tab(cx),
                Tab::Settings => self.settings_tab(cx),
            })
    }
}

impl Wowzers {
    fn header(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let selected = self.selected_flavor;
        let flavors: Vec<Flavor> = self
            .current_install()
            .map(|i| i.flavors.iter().map(|f| f.flavor).collect())
            .unwrap_or_default();
        let tabs = [
            (Tab::Installed, "Installed"),
            (Tab::Find, "Find addons"),
            (Tab::Settings, "Settings"),
        ];
        h_flex()
            .px_4()
            .py_3()
            .gap_4()
            .items_center()
            .border_b_1()
            .border_color(theme.border)
            .child(div().text_lg().font_semibold().child("wowzers"))
            .child(
                ButtonGroup::new("tabs")
                    .outline()
                    .small()
                    .children(tabs.iter().map(|(tab, label)| {
                        Button::new(SharedString::from(format!("tab-{label}")))
                            .label(*label)
                            .selected(self.tab == *tab)
                    }))
                    .on_click(cx.listener(move |this, clicked: &Vec<usize>, _, cx| {
                        if let Some(&ix) = clicked.first() {
                            this.tab = tabs[ix].0;
                            cx.notify();
                        }
                    })),
            )
            .child(div().flex_1())
            .when(!flavors.is_empty(), |this| {
                let options = flavors.clone();
                this.child(
                    ButtonGroup::new("flavors")
                        .outline()
                        .small()
                        .children(flavors.iter().map(|f| {
                            Button::new(SharedString::from(format!("flavor-{}", f.dir_name())))
                                .label(f.label())
                                .selected(selected == Some(*f))
                        }))
                        .on_click(cx.listener(move |this, clicked: &Vec<usize>, window, cx| {
                            if let Some(&ix) = clicked.first() {
                                this.select_flavor(options[ix], window, cx);
                            }
                        })),
                )
            })
            .into_any_element()
    }

    fn key_banner(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        h_flex()
            .px_4()
            .py_2()
            .gap_3()
            .items_center()
            .bg(theme.warning.opacity(0.15))
            .text_sm()
            .child(
                div().flex_1().child(
                    "wowzers needs a free CurseForge API key to look up and download addons.",
                ),
            )
            .child(
                Button::new("banner-key")
                    .small()
                    .label("Add key")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.tab = Tab::Settings;
                        cx.notify();
                    })),
            )
            .into_any_element()
    }

    // ---- installed ----------------------------------------------------

    fn installed_tab(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let Some(flavor) = self.current_flavor() else {
            return self.no_install(cx);
        };
        let outdated = self.outdated().len();
        let busy = self.busy.clone();
        let idle = busy.is_none();
        let checking = busy == Some(Busy::Checking);

        let summary = match (&busy, self.addons.len()) {
            (Some(Busy::Scanning), _) => "Reading AddOns folder…".to_string(),
            (_, 0) => "No addons installed".to_string(),
            (_, n) if outdated > 0 => format!("{n} addons, {outdated} with updates"),
            (_, n) if self.statuses.is_empty() => format!("{n} addons"),
            (_, n) => format!("{n} addons, all up to date"),
        };

        let toolbar = h_flex()
            .px_4()
            .py_2()
            .gap_3()
            .items_center()
            .border_b_1()
            .border_color(theme.border)
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(div().font_medium().child(summary))
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .truncate()
                            .child(flavor.addons_dir.display().to_string()),
                    ),
            )
            .child(
                Button::new("check")
                    .outline()
                    .icon(IconName::RefreshCw)
                    .label("Check for updates")
                    .loading(checking)
                    .disabled(!idle || !self.has_api_key())
                    .on_click(cx.listener(|this, _, window, cx| this.check(window, cx))),
            )
            .child(
                Button::new("update-all")
                    .primary()
                    .icon(IconName::ArrowDown)
                    .label(if outdated > 0 {
                        format!("Update all ({outdated})")
                    } else {
                        "Update all".to_string()
                    })
                    .disabled(!idle || outdated == 0)
                    .on_click(cx.listener(|this, _, window, cx| this.update_all(window, cx))),
            );

        let rows: Vec<AnyElement> = self
            .addons
            .iter()
            .enumerate()
            .map(|(ix, addon)| self.addon_row(ix, addon, idle, cx))
            .collect();

        v_flex()
            .flex_1()
            .min_h_0()
            .child(toolbar)
            .when_some(self.install_progress(cx), |this, progress| {
                this.child(progress)
            })
            .child(
                div()
                    .id("addon-list")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(v_flex().children(rows)),
            )
            .into_any_element()
    }

    fn install_progress(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let Some(Busy::Installing(label)) = &self.busy else {
            return None;
        };
        let theme = cx.theme().clone();
        let progress = self.progress.lock().expect("progress lock").clone();
        let (text, percent) = match progress {
            Some(p) => (
                format!("{} ({}/{})", p.message, p.done, p.total),
                (p.done as f32 / p.total.max(1) as f32) * 100.0,
            ),
            None => (format!("Preparing {label}…"), 0.0),
        };
        Some(
            v_flex()
                .px_4()
                .py_2()
                .gap_1()
                .border_b_1()
                .border_color(theme.border)
                .child(div().text_sm().child(text))
                .child(Progress::new("install-progress").value(percent))
                .into_any_element(),
        )
    }

    fn addon_row(
        &self,
        ix: usize,
        addon: &InstalledAddon,
        idle: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme().clone();
        let status = self.status_of(addon);
        let details: Vec<String> = [
            addon.version.as_ref().map(|v| format!("v{v}")),
            (addon.folders.len() > 1).then(|| format!("{} folders", addon.folders.len())),
            addon.author.clone().filter(|a| !a.is_empty()),
        ]
        .into_iter()
        .flatten()
        .collect();

        let confirming = addon.folders.first() == self.confirm_remove.as_ref();
        let candidate = match status {
            Some(UpdateStatus::Outdated(c)) => Some((**c).clone()),
            _ => None,
        };
        let remove_target = addon.clone();

        h_flex()
            .id(("addon", ix))
            .px_4()
            .py_2()
            .gap_3()
            .items_center()
            .border_b_1()
            .border_color(theme.border.opacity(0.5))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(div().font_medium().truncate().child(addon.name.clone()))
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .truncate()
                            .child(details.join("  ·  ")),
                    ),
            )
            .child(self.status_tag(addon, status))
            .when_some(candidate, |this, candidate| {
                this.child(
                    Button::new(("update", ix))
                        .small()
                        .primary()
                        .label("Update")
                        .disabled(!idle)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.update_one(candidate.clone(), window, cx)
                        })),
                )
            })
            .child({
                let button = Button::new(("remove", ix)).small().disabled(!idle);
                if confirming {
                    button.danger().label("Confirm remove")
                } else {
                    button.ghost().icon(IconName::Delete).tooltip("Remove")
                }
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.remove(remove_target.clone(), window, cx)
                }))
            })
            .into_any_element()
    }

    fn status_tag(&self, addon: &InstalledAddon, status: Option<&UpdateStatus>) -> AnyElement {
        let flavor = self.selected_flavor.map_or("this client", |f| f.label());
        match (status, addon.project_id) {
            (Some(UpdateStatus::Outdated(c)), _) => Tag::warning()
                .small()
                .child(format!("→ {}", truncate(&c.file.display_name, 32))),
            (Some(UpdateStatus::UpToDate), _) => Tag::success().small().child("Up to date"),
            (Some(UpdateStatus::NoCompatibleFile), _) => {
                Tag::secondary().small().child(format!("No {flavor} file"))
            }
            (Some(UpdateStatus::NotDownloadable), _) => {
                Tag::secondary().small().child("Manual download only")
            }
            (Some(UpdateStatus::NotFound), _) => {
                Tag::secondary().small().child("Not on CurseForge")
            }
            (None, None) => Tag::secondary().small().child("Not from CurseForge"),
            (None, Some(_)) if self.busy == Some(Busy::Checking) => {
                return Spinner::new().xsmall().into_any_element();
            }
            (None, Some(_)) => return div().into_any_element(),
        }
        .into_any_element()
    }

    fn no_install(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        v_flex()
            .flex_1()
            .items_center()
            .justify_center()
            .gap_3()
            .child(
                div()
                    .text_lg()
                    .font_medium()
                    .child("No World of Warcraft folder found"),
            )
            .child(
                div()
                    .text_color(theme.muted_foreground)
                    .child("Pick the folder that contains _retail_ or _classic_."),
            )
            .child(
                Button::new("browse-empty")
                    .primary()
                    .icon(IconName::FolderOpen)
                    .label("Choose folder…")
                    .on_click(cx.listener(|this, _, window, cx| this.browse(window, cx))),
            )
            .into_any_element()
    }

    // ---- find ---------------------------------------------------------

    fn find_tab(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let searching = matches!(self.search_state, SearchState::Searching);
        let idle = self.busy.is_none();
        let status_line = match &self.search_state {
            SearchState::Idle => "Search by addon name or author.".to_string(),
            SearchState::Searching => "Searching…".to_string(),
            SearchState::Results { query, total } => format!("{total} results for “{query}”"),
            SearchState::Failed(e) => e.clone(),
        };
        let installable = self.current_flavor().is_some();
        let rows: Vec<AnyElement> = self
            .search_results
            .iter()
            .enumerate()
            .map(|(ix, m)| self.result_row(ix, m, idle && installable, cx))
            .collect();

        v_flex()
            .flex_1()
            .min_h_0()
            .child(
                h_flex()
                    .px_4()
                    .py_2()
                    .gap_2()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(div().flex_1().child(Input::new(&self.search_input)))
                    .child(
                        Button::new("search")
                            .primary()
                            .icon(IconName::Search)
                            .label("Search")
                            .loading(searching)
                            .on_click(cx.listener(|this, _, window, cx| this.search(window, cx))),
                    ),
            )
            .child(
                div()
                    .px_4()
                    .py_1()
                    .text_xs()
                    .text_color(if matches!(self.search_state, SearchState::Failed(_)) {
                        theme.danger
                    } else {
                        theme.muted_foreground
                    })
                    .child(status_line),
            )
            .when_some(self.install_progress(cx), |this, progress| {
                this.child(progress)
            })
            .child(
                div()
                    .id("result-list")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(v_flex().children(rows)),
            )
            .into_any_element()
    }

    fn result_row(
        &self,
        ix: usize,
        m: &Mod,
        can_install: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme().clone();
        let installed = self.is_installed(m.id);
        let target = m.clone();
        h_flex()
            .id(("result", ix))
            .px_4()
            .py_2()
            .gap_3()
            .items_center()
            .border_b_1()
            .border_color(theme.border.opacity(0.5))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(
                        h_flex()
                            .gap_2()
                            .items_baseline()
                            .child(div().font_medium().child(m.name.clone()))
                            .child(div().text_xs().text_color(theme.muted_foreground).child(
                                format!(
                                    "by {}  ·  {} downloads",
                                    m.author_names(),
                                    compact_count(m.download_count)
                                ),
                            )),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme.muted_foreground)
                            .truncate()
                            .child(truncate(&m.summary, 160)),
                    ),
            )
            .child(if installed {
                Tag::success().small().child("Installed").into_any_element()
            } else {
                Button::new(("install", ix))
                    .small()
                    .primary()
                    .icon(IconName::ArrowDown)
                    .label("Install")
                    .disabled(!can_install)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.install(target.clone(), window, cx)
                    }))
                    .into_any_element()
            })
            .into_any_element()
    }

    // ---- settings -----------------------------------------------------

    fn settings_tab(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let heading = |text: &'static str| div().font_semibold().child(text);
        let hint = |text: String| {
            div()
                .text_sm()
                .text_color(theme.muted_foreground)
                .child(text)
        };

        let key_hint = if self
            .config
            .api_key
            .as_deref()
            .is_some_and(|k| !k.trim().is_empty())
        {
            "A key is saved. Enter a new one to replace it."
        } else if self.has_api_key() {
            "Using the key from the CURSEFORGE_API_KEY environment variable."
        } else {
            "No key yet."
        };

        let installs: Vec<AnyElement> = self
            .installs
            .iter()
            .enumerate()
            .map(|(ix, install)| {
                let root = install.root.clone();
                let current = self.selected_root.as_ref() == Some(&install.root);
                h_flex()
                    .gap_3()
                    .items_center()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .child(install.root.display().to_string()),
                    )
                    .child(
                        div().text_xs().text_color(theme.muted_foreground).child(
                            install
                                .flavors
                                .iter()
                                .map(|f| f.flavor.label())
                                .collect::<Vec<_>>()
                                .join(", "),
                        ),
                    )
                    .child(if current {
                        Tag::success().small().child("In use").into_any_element()
                    } else {
                        Button::new(("use-install", ix))
                            .small()
                            .outline()
                            .label("Use")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.select_install(root.clone(), window, cx)
                            }))
                            .into_any_element()
                    })
                    .into_any_element()
            })
            .collect();

        v_flex()
            .id("settings")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .p_4()
            .gap_6()
            .max_w(px(760.))
            .child(
                v_flex()
                    .gap_2()
                    .child(heading("CurseForge API key"))
                    .child(hint(format!(
                        "{key_hint} Keys are free at console.curseforge.com and stay on this computer."
                    )))
                    .child(
                        h_flex()
                            .gap_2()
                            .child(div().flex_1().child(Input::new(&self.key_input)))
                            .child(
                                Button::new("save-key")
                                    .primary()
                                    .label("Save")
                                    .on_click(cx.listener(|this, _, window, cx| this.save_api_key(window, cx))),
                            )
                            .child(
                                Button::new("get-key")
                                    .outline()
                                    .label("Get a key")
                                    .on_click(cx.listener(|_, _, _, cx| cx.open_url(CURSEFORGE_KEYS_URL))),
                            ),
                    ),
            )
            .child(
                v_flex()
                    .gap_2()
                    .child(heading("World of Warcraft folders"))
                    .child(hint(if self.installs.is_empty() {
                        "None found automatically. Choose the folder that contains _retail_ or _classic_.".into()
                    } else {
                        "Found automatically or added by you.".to_string()
                    }))
                    .children(installs)
                    .child(
                        h_flex()
                            .gap_2()
                            .child(div().flex_1().child(Input::new(&self.path_input)))
                            .child(
                                Button::new("add-path")
                                    .outline()
                                    .label("Add")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.add_typed_path(window, cx)
                                    })),
                            )
                            .child(
                                Button::new("browse")
                                    .outline()
                                    .icon(IconName::FolderOpen)
                                    .label("Browse…")
                                    .on_click(cx.listener(|this, _, window, cx| this.browse(window, cx))),
                            ),
                    ),
            )
            .child(
                v_flex()
                    .gap_2()
                    .child(heading("Release channel"))
                    .child(hint("Stable releases only, unless you opt in.".to_string()))
                    .child(
                        Checkbox::new("allow-beta")
                            .label("Also install beta releases")
                            .checked(self.config.allow_beta)
                            .on_click(cx.listener(|this, checked: &bool, window, cx| {
                                this.set_allow_beta(*checked, window, cx)
                            })),
                    )
                    .child(
                        Checkbox::new("allow-alpha")
                            .label("Also install alpha releases")
                            .checked(self.config.allow_alpha)
                            .on_click(cx.listener(|this, checked: &bool, window, cx| {
                                this.set_allow_alpha(*checked, window, cx)
                            })),
                    ),
            )
            .into_any_element()
    }
}
