//! Application state and the commands that change it. Rendering is in
//! `render.rs`; everything slow runs on a background thread.

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

use gpui_kit::component::{
    WindowExt as _,
    input::{InputEvent, InputState},
    notification::{Notification, NotificationType},
};
use gpui_kit::{
    AppContext as _, Context, Entity, PathPromptOptions, SharedString, Subscription, Task, Window,
};
use wowzers_core::{
    config::Config,
    curseforge::{Client, Mod},
    detect::{self, FlavorInstall, WowInstall},
    flavor::Flavor,
    installed::{InstalledAddon, list_installed},
    manager::{self, Candidate, InstallReport, Progress, UpdateStatus},
};

/// Overrides the CurseForge server, for testing against a fake.
const BASE_URL_ENV: &str = "WOWZERS_CURSEFORGE_URL";

/// Where the app reads its settings and which servers and folders it uses.
pub struct Options {
    pub config_path: Option<PathBuf>,
    /// CurseForge site URL; `None` for the real site.
    pub base_url: Option<String>,
    /// Look for WoW in the usual install locations, not only saved paths.
    pub scan_default_locations: bool,
}

impl Options {
    pub fn from_env() -> Self {
        Options {
            config_path: Config::default_path(),
            base_url: std::env::var(BASE_URL_ENV).ok().filter(|u| !u.is_empty()),
            scan_default_locations: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Installed,
    Find,
    Settings,
}

/// The one long-running operation in flight, if any.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Busy {
    Scanning,
    Checking,
    Installing(String),
}

#[derive(Debug, Clone)]
pub enum SearchState {
    Idle,
    Searching,
    Results { query: String, total: u32 },
    Failed(String),
}

pub struct Wowzers {
    pub config: Config,
    options: Options,
    pub installs: Vec<WowInstall>,
    pub selected_root: Option<PathBuf>,
    pub selected_flavor: Option<Flavor>,
    pub addons: Vec<InstalledAddon>,
    /// Update status by CurseForge project id; empty until a check has run.
    pub statuses: HashMap<u32, UpdateStatus>,
    pub tab: Tab,
    pub busy: Option<Busy>,
    pub progress: Arc<Mutex<Option<Progress>>>,
    pub search_input: Entity<InputState>,
    pub search_results: Vec<Mod>,
    pub search_state: SearchState,
    pub path_input: Entity<InputState>,
    /// Folder name of the addon whose Remove button awaits confirmation.
    pub confirm_remove: Option<String>,
    /// Why the last scan or check failed, shown above the list.
    pub error: Option<SharedString>,
    _subscriptions: Vec<Subscription>,
    _tasks: Vec<Task<()>>,
}

impl Wowzers {
    pub fn new(options: Options, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let (config, config_error) = match options.config_path.as_deref().map(Config::load) {
            Some(Ok(config)) => (config, None),
            Some(Err(e)) => (Config::default(), Some(e.to_string())),
            None => (Config::default(), None),
        };

        let search_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("Search CurseForge addons"));
        let path_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("/path/to/World of Warcraft"));
        let subscriptions = vec![
            cx.subscribe_in(
                &search_input,
                window,
                |this, _, event: &InputEvent, window, cx| {
                    if let InputEvent::PressEnter { .. } = event {
                        this.search(window, cx);
                    }
                },
            ),
            cx.subscribe_in(
                &path_input,
                window,
                |this, _, event: &InputEvent, window, cx| {
                    if let InputEvent::PressEnter { .. } = event {
                        this.add_typed_path(window, cx);
                    }
                },
            ),
        ];

        let mut this = Self {
            config,
            options,
            installs: Vec::new(),
            selected_root: None,
            selected_flavor: None,
            addons: Vec::new(),
            statuses: HashMap::new(),
            tab: Tab::Installed,
            busy: None,
            progress: Arc::new(Mutex::new(None)),
            search_input,
            search_results: Vec::new(),
            search_state: SearchState::Idle,
            path_input,
            confirm_remove: None,
            error: config_error.map(Into::into),
            _subscriptions: subscriptions,
            _tasks: Vec::new(),
        };
        this.detect(window, cx);
        this
    }

    // ---- selection ----------------------------------------------------

    pub fn current_install(&self) -> Option<&WowInstall> {
        let root = self.selected_root.as_ref()?;
        self.installs.iter().find(|i| &i.root == root)
    }

    pub fn current_flavor(&self) -> Option<&FlavorInstall> {
        let flavor = self.selected_flavor?;
        self.current_install()?
            .flavors
            .iter()
            .find(|f| f.flavor == flavor)
    }

    /// Finds installs (saved paths first, then the usual places) and picks one.
    fn detect(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut roots = self.config.wow_roots.clone();
        if self.options.scan_default_locations {
            roots.extend(detect::candidate_roots());
        }
        self.installs = detect::detect_in(&roots);

        let install = self
            .config
            .selected_root
            .as_ref()
            .and_then(|root| self.installs.iter().find(|i| &i.root == root))
            .or_else(|| self.installs.first());
        self.selected_root = install.map(|i| i.root.clone());
        self.selected_flavor = install.and_then(|i| {
            self.config
                .selected_flavor
                .filter(|f| i.flavors.iter().any(|x| x.flavor == *f))
                .or_else(|| {
                    i.flavors
                        .iter()
                        .find(|f| f.flavor == Flavor::Retail)
                        .map(|f| f.flavor)
                })
                .or_else(|| i.flavors.first().map(|f| f.flavor))
        });
        if self.selected_root.is_none() {
            self.tab = Tab::Settings;
        }
        self.reload(true, window, cx);
    }

    pub fn select_install(&mut self, root: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let Some(install) = self.installs.iter().find(|i| i.root == root) else {
            return;
        };
        let flavor = install
            .flavors
            .iter()
            .find(|f| Some(f.flavor) == self.selected_flavor)
            .or_else(|| install.flavors.first())
            .map(|f| f.flavor);
        self.selected_root = Some(root);
        self.selected_flavor = flavor;
        self.selection_changed(window, cx);
    }

    pub fn select_flavor(&mut self, flavor: Flavor, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_flavor == Some(flavor) || self.busy.is_some() {
            return;
        }
        self.selected_flavor = Some(flavor);
        self.selection_changed(window, cx);
    }

    fn selection_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.config.selected_root = self.selected_root.clone();
        self.config.selected_flavor = self.selected_flavor;
        self.save_config();
        self.reload(true, window, cx);
    }

    /// Adds a WoW folder the user picked or typed.
    fn add_root(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        match detect::resolve_user_path(&path) {
            Ok(install) => {
                let root = install.root.clone();
                if !self.config.wow_roots.contains(&root) {
                    self.config.wow_roots.push(root.clone());
                }
                if !self.installs.iter().any(|i| i.root == root) {
                    self.installs.push(install);
                }
                self.tab = Tab::Installed;
                self.select_install(root, window, cx);
            }
            Err(e) => self.notify(NotificationType::Error, e.to_string(), window, cx),
        }
    }

    pub fn add_typed_path(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.path_input.read(cx).value().trim().to_string();
        if !text.is_empty() {
            self.add_root(PathBuf::from(text), window, cx);
        }
    }

    pub fn browse(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Select your World of Warcraft folder".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = picked.await else {
                return;
            };
            if let Some(path) = paths.into_iter().next() {
                let _ = this.update_in(cx, |this, window, cx| this.add_root(path, window, cx));
            }
        })
        .detach();
    }

    // ---- settings -----------------------------------------------------

    fn save_config(&self) {
        if let Some(path) = &self.options.config_path
            && let Err(e) = self.config.save(path)
        {
            tracing::warn!("saving settings failed: {e}");
        }
    }

    pub fn set_allow_beta(&mut self, on: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.config.allow_beta = on;
        if !on {
            self.config.allow_alpha = false;
        }
        self.channel_changed(window, cx);
    }

    pub fn set_allow_alpha(&mut self, on: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.config.allow_alpha = on;
        if on {
            self.config.allow_beta = true;
        }
        self.channel_changed(window, cx);
    }

    fn channel_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.save_config();
        self.statuses.clear();
        self.check(window, cx);
        cx.notify();
    }

    fn client(&self) -> Client {
        let client = Client::new();
        match &self.options.base_url {
            Some(url) => client.with_base_url(url),
            None => client,
        }
    }

    // ---- scanning and checking ---------------------------------------

    /// Re-reads the AddOns folder; with `then_check`, follows with an update
    /// check when a key is set.
    pub fn reload(&mut self, then_check: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.addons.clear();
        self.statuses.clear();
        self.confirm_remove = None;
        self.error = None;
        let Some(flavor) = self.current_flavor().cloned() else {
            cx.notify();
            return;
        };
        self.busy = Some(Busy::Scanning);
        cx.notify();
        let task = cx.spawn_in(window, async move |this, cx| {
            let addons_dir = flavor.addons_dir.clone();
            let result = cx
                .background_spawn(async move { list_installed(&addons_dir) })
                .await;
            let _ = this.update_in(cx, |this, window, cx| {
                // The selection may have moved on while scanning.
                if this.current_flavor().map(|f| &f.addons_dir) != Some(&flavor.addons_dir) {
                    return;
                }
                this.busy = None;
                match result {
                    Ok(addons) => this.addons = addons,
                    Err(e) => this.error = Some(e.to_string().into()),
                }
                if then_check && !this.addons.is_empty() {
                    this.check(window, cx);
                }
                cx.notify();
            });
        });
        self._tasks.push(task);
    }

    /// Asks CurseForge which addons have newer files.
    pub fn check(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(self.busy, Some(Busy::Checking | Busy::Installing(_))) {
            return;
        }
        let Some(flavor) = self.current_flavor().cloned() else {
            return;
        };
        let client = self.client();
        let addons = self.addons.clone();
        let channel = self.config.channel();
        self.busy = Some(Busy::Checking);
        self.error = None;
        cx.notify();
        let task = cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_spawn({
                    let flavor = flavor.clone();
                    async move { manager::check_updates(&client, &flavor, &addons, channel) }
                })
                .await;
            let _ = this.update_in(cx, |this, _, cx| {
                if this.current_flavor().map(|f| &f.addons_dir) != Some(&flavor.addons_dir) {
                    return;
                }
                this.busy = None;
                match result {
                    Ok(statuses) => this.statuses = statuses,
                    Err(e) => this.error = Some(e.to_string().into()),
                }
                cx.notify();
            });
        });
        self._tasks.push(task);
    }

    pub fn status_of(&self, addon: &InstalledAddon) -> Option<&UpdateStatus> {
        self.statuses.get(&addon.project_id?)
    }

    pub fn outdated(&self) -> Vec<Candidate> {
        let mut seen = std::collections::HashSet::new();
        self.addons
            .iter()
            .filter_map(|a| match self.status_of(a)? {
                UpdateStatus::Outdated(c) if seen.insert(c.mod_.id) => Some((**c).clone()),
                _ => None,
            })
            .collect()
    }

    // ---- installing ---------------------------------------------------

    /// One click: update every outdated addon.
    pub fn update_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let jobs = self.outdated();
        let label = format!("{} addons", jobs.len());
        self.run_installs(jobs, label, window, cx);
    }

    pub fn update_one(
        &mut self,
        candidate: Candidate,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let label = candidate.mod_.name.clone();
        self.run_installs(vec![candidate], label, window, cx);
    }

    /// Installs the newest suitable file of a search result.
    pub fn install(&mut self, mod_: Mod, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy.is_some() {
            return;
        }
        let Some(flavor) = self.current_flavor().cloned() else {
            return;
        };
        let client = self.client();
        let channel = self.config.channel();
        self.begin(Busy::Installing(mod_.name.clone()), cx);
        let progress = self.progress.clone();
        let task = cx.spawn_in(window, async move |this, cx| {
            let work = cx.background_spawn(async move {
                let candidate = manager::resolve_install(&client, &flavor, &mod_, channel)?;
                run_install(&client, &flavor, vec![candidate], channel, &progress)
            });
            let outcome = work.await;
            let _ = this.update_in(cx, |this, window, cx| this.finished(outcome, window, cx));
        });
        self._tasks.push(task);
    }

    fn run_installs(
        &mut self,
        jobs: Vec<Candidate>,
        label: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if jobs.is_empty() || self.busy.is_some() {
            return;
        }
        let Some(flavor) = self.current_flavor().cloned() else {
            return;
        };
        let client = self.client();
        let channel = self.config.channel();
        self.begin(Busy::Installing(label), cx);
        let progress = self.progress.clone();
        let task = cx.spawn_in(window, async move |this, cx| {
            let outcome = cx
                .background_spawn(
                    async move { run_install(&client, &flavor, jobs, channel, &progress) },
                )
                .await;
            let _ = this.update_in(cx, |this, window, cx| this.finished(outcome, window, cx));
        });
        self._tasks.push(task);
    }

    /// Marks the operation started and polls its progress for the UI.
    fn begin(&mut self, busy: Busy, cx: &mut Context<Self>) {
        self.busy = Some(busy);
        self.error = None;
        *self.progress.lock().expect("progress lock") = None;
        cx.notify();
        let poll = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(120))
                    .await;
                let Ok(still_running) = this.update(cx, |this, cx| {
                    cx.notify();
                    matches!(this.busy, Some(Busy::Installing(_)))
                }) else {
                    break;
                };
                if !still_running {
                    break;
                }
            }
        });
        self._tasks.push(poll);
    }

    fn finished(&mut self, outcome: InstallOutcome, window: &mut Window, cx: &mut Context<Self>) {
        self.busy = None;
        match outcome {
            Ok(done) => {
                let InstallDone {
                    report,
                    addons,
                    statuses,
                } = done;
                self.addons = addons;
                self.statuses = statuses;
                self.confirm_remove = None;
                if !report.installed.is_empty() {
                    let message = match report.installed.as_slice() {
                        [one] => format!("Installed {one}"),
                        many => format!("Installed {} addons", many.len()),
                    };
                    self.notify(NotificationType::Success, message, window, cx);
                }
                for (name, reason) in report.failed {
                    self.notify(
                        NotificationType::Error,
                        format!("{name}: {reason}"),
                        window,
                        cx,
                    );
                }
            }
            Err(e) => self.notify(NotificationType::Error, e.to_string(), window, cx),
        }
        cx.notify();
    }

    // ---- removing -----------------------------------------------------

    /// First click arms the button; the second deletes.
    pub fn remove(&mut self, addon: InstalledAddon, window: &mut Window, cx: &mut Context<Self>) {
        let key = addon.folders.first().cloned().unwrap_or_default();
        if self.confirm_remove.as_ref() != Some(&key) {
            self.confirm_remove = Some(key);
            cx.notify();
            return;
        }
        let Some(flavor) = self.current_flavor().cloned() else {
            return;
        };
        self.confirm_remove = None;
        self.busy = Some(Busy::Scanning);
        cx.notify();
        let name = addon.name.clone();
        let task = cx.spawn_in(window, async move |this, cx| {
            let addons_dir = flavor.addons_dir.clone();
            let result = cx
                .background_spawn(async move {
                    manager::uninstall(&addons_dir, &addon)?;
                    list_installed(&addons_dir)
                })
                .await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.busy = None;
                match result {
                    Ok(addons) => {
                        this.addons = addons;
                        this.notify(
                            NotificationType::Success,
                            format!("Removed {name}"),
                            window,
                            cx,
                        );
                    }
                    Err(e) => this.notify(NotificationType::Error, e.to_string(), window, cx),
                }
                cx.notify();
            });
        });
        self._tasks.push(task);
    }

    // ---- search -------------------------------------------------------

    pub fn search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let query = self.search_input.read(cx).value().trim().to_string();
        if query.is_empty() || matches!(self.search_state, SearchState::Searching) {
            return;
        }
        let client = self.client();
        self.search_state = SearchState::Searching;
        cx.notify();
        let task = cx.spawn_in(window, async move |this, cx| {
            let q = query.clone();
            let result = cx
                .background_spawn(async move { client.search(&q, 0, 30) })
                .await;
            let _ = this.update_in(cx, |this, _, cx| {
                match result {
                    Ok(page) => {
                        this.search_state = SearchState::Results {
                            query,
                            total: page.total,
                        };
                        this.search_results = page.items;
                    }
                    Err(e) => this.search_state = SearchState::Failed(e.to_string()),
                }
                cx.notify();
            });
        });
        self._tasks.push(task);
    }

    // ---- helpers ------------------------------------------------------

    fn notify(
        &self,
        kind: NotificationType,
        message: impl Into<SharedString>,
        window: &mut Window,
        cx: &mut gpui_kit::App,
    ) {
        window.push_notification(Notification::new().message(message).with_type(kind), cx);
    }

    /// Whether `project_id` is already in the AddOns folder.
    pub fn is_installed(&self, project_id: u32) -> bool {
        self.addons.iter().any(|a| a.project_id == Some(project_id))
    }
}

struct InstallDone {
    report: InstallReport,
    addons: Vec<InstalledAddon>,
    statuses: HashMap<u32, UpdateStatus>,
}

type InstallOutcome = Result<InstallDone, wowzers_core::Error>;

/// Installs `jobs`, then re-reads the folder and re-checks so the list shows
/// what is really on disk.
fn run_install(
    client: &Client,
    flavor: &FlavorInstall,
    jobs: Vec<Candidate>,
    channel: wowzers_core::curseforge::ReleaseType,
    progress: &Mutex<Option<Progress>>,
) -> InstallOutcome {
    let report = manager::install_all(client, &flavor.addons_dir, &jobs, &|p| {
        *progress.lock().expect("progress lock") = Some(p);
    })?;
    let addons = list_installed(&flavor.addons_dir)?;
    let statuses = manager::check_updates(client, flavor, &addons, channel).unwrap_or_default();
    Ok(InstallDone {
        report,
        addons,
        statuses,
    })
}
