//! Application state and the commands that change it. Views are in
//! `render.rs`; everything slow runs on a worker thread and lands in `poll`.

use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::{
        Arc, Mutex,
        mpsc::{Receiver, Sender, channel},
    },
    time::Duration,
};

use rune::{FileDialogOptions, TextEdit, Tx, Waker, tok::motion};
use wowzers_core::{
    config::{Appearance, Config},
    curseforge::{Client, Mod, Page, ReleaseType},
    detect::{self, FlavorInstall, WowInstall},
    flavor::Flavor,
    installed::{InstalledAddon, list_installed},
    manager::{self, Candidate, InstallReport, Progress, UpdateStatus},
};

/// Overrides the CurseForge server, for testing against a fake.
const BASE_URL_ENV: &str = "WOWZERS_CURSEFORGE_URL";

/// How long a notification stays up, in seconds.
const NOTE_SECONDS: f64 = 5.0;
const ERROR_NOTE_SECONDS: f64 = 9.0;

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

/// An empty query means the most popular addons.
#[derive(Debug, Clone)]
pub enum SearchState {
    Idle,
    Searching { query: String },
    Results { query: String, total: u32 },
    Failed(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoteKind {
    Success,
    Error,
}

/// A notification in the corner, gone at `until` (app clock seconds).
#[derive(Debug, Clone)]
pub struct Note {
    pub id: u64,
    pub kind: NoteKind,
    pub text: String,
    until: f64,
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
    pub progress: Option<Progress>,
    pub search: TextEdit,
    pub search_results: Vec<Mod>,
    pub search_state: SearchState,
    pub path: TextEdit,
    /// Folder name of the addon whose Remove button awaits confirmation.
    pub confirm_remove: Option<String>,
    /// Why the last scan or check failed, shown above the list.
    pub error: Option<String>,
    pub notes: Vec<Note>,
    next_note: u64,
    /// The app clock at the last `poll`, for timing notifications.
    clock: f64,
    worker: Worker,
}

/// Work finished on the worker thread, applied in `poll`.
enum Done {
    Scanned {
        addons_dir: PathBuf,
        then_check: bool,
        result: wowzers_core::Result<Vec<InstalledAddon>>,
    },
    Checked {
        addons_dir: PathBuf,
        result: wowzers_core::Result<HashMap<u32, UpdateStatus>>,
    },
    Progress(Progress),
    Installed(InstallOutcome),
    Removed {
        name: String,
        result: wowzers_core::Result<Vec<InstalledAddon>>,
    },
    Searched {
        query: String,
        result: wowzers_core::Result<Page<Mod>>,
    },
}

/// Runs jobs on their own threads and wakes the window when one finishes.
struct Worker {
    send: Sender<Done>,
    receive: Receiver<Done>,
    /// Set by `AppSpec::setup`; until then answers wait for the first frame.
    waker: Arc<Mutex<Option<Waker>>>,
}

impl Worker {
    fn new() -> Self {
        let (send, receive) = channel();
        Worker {
            send,
            receive,
            waker: Arc::default(),
        }
    }

    /// Returns a function that delivers to `poll` from any thread.
    fn sink(&self) -> impl Fn(Done) + Send + Sync + Clone + 'static {
        let send = self.send.clone();
        let waker = self.waker.clone();
        move |done| {
            if send.send(done).is_ok() {
                let wake = waker.lock().expect("waker lock").clone();
                if let Some(wake) = wake {
                    wake();
                }
            }
        }
    }

    fn spawn(&self, job: impl FnOnce() -> Done + Send + 'static) {
        let deliver = self.sink();
        std::thread::spawn(move || deliver(job()));
    }
}

/// `AppSpec::setup`: background answers wake the window from now on.
pub fn setup(app: &mut Wowzers, waker: Waker) {
    *app.worker.waker.lock().expect("waker lock") = Some(waker);
}

/// `AppSpec::poll`: lands finished work and expires notifications.
pub fn poll(app: &mut Wowzers, clock: f64, tx: &mut Tx) -> bool {
    app.clock = clock;
    let mut changed = false;
    while let Ok(done) = app.worker.receive.try_recv() {
        app.apply(done, tx);
        changed = true;
    }
    let before = app.notes.len();
    app.notes.retain(|n| n.until > clock);
    changed |= app.notes.len() != before;
    app.schedule_note_expiry(tx);
    changed
}

impl Wowzers {
    pub fn new(options: Options) -> Self {
        let (config, config_error) = match options.config_path.as_deref().map(Config::load) {
            Some(Ok(config)) => (config, None),
            Some(Err(e)) => (Config::default(), Some(e.to_string())),
            None => (Config::default(), None),
        };
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
            progress: None,
            search: TextEdit::new(""),
            search_results: Vec::new(),
            search_state: SearchState::Idle,
            path: TextEdit::new(""),
            confirm_remove: None,
            error: None,
            notes: Vec::new(),
            next_note: 0,
            clock: 0.0,
            worker: Worker::new(),
        };
        this.detect();
        this.error = config_error;
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
    fn detect(&mut self) {
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
        self.reload(true);
    }

    pub fn select_install(&mut self, root: PathBuf) {
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
        self.selection_changed();
    }

    pub fn select_flavor(&mut self, flavor: Flavor) {
        if self.selected_flavor == Some(flavor) || self.busy.is_some() {
            return;
        }
        self.selected_flavor = Some(flavor);
        self.selection_changed();
    }

    fn selection_changed(&mut self) {
        self.config.selected_root = self.selected_root.clone();
        self.config.selected_flavor = self.selected_flavor;
        self.save_config();
        self.reload(true);
    }

    /// Adds a WoW folder the user picked or typed.
    fn add_root(&mut self, path: PathBuf, tx: &mut Tx) {
        match detect::resolve_user_path(&path) {
            Ok(install) => {
                let root = install.root.clone();
                if !self.config.wow_roots.contains(&root) {
                    self.config.wow_roots.push(root.clone());
                }
                if !self.installs.iter().any(|i| i.root == root) {
                    self.installs.push(install);
                }
                self.path.clear();
                self.tab = Tab::Installed;
                self.select_install(root);
            }
            Err(e) => self.note(NoteKind::Error, e.to_string(), tx),
        }
    }

    pub fn add_typed_path(&mut self, tx: &mut Tx) {
        let text = self.path.text.trim().to_string();
        if !text.is_empty() {
            self.add_root(PathBuf::from(text), tx);
        }
    }

    pub fn browse(&mut self, tx: &mut Tx) {
        let options = FileDialogOptions {
            title: "Select your World of Warcraft folder".into(),
            ..Default::default()
        };
        tx.open_folder(options, |this: &mut Wowzers, result, tx| match result {
            Ok(Some(path)) => this.add_root(path, tx),
            Ok(None) => {}
            Err(e) => this.note(NoteKind::Error, e.to_string(), tx),
        });
    }

    // ---- settings -----------------------------------------------------

    fn save_config(&self) {
        if let Some(path) = &self.options.config_path
            && let Err(e) = self.config.save(path)
        {
            tracing::warn!("saving settings failed: {e}");
        }
    }

    pub fn set_allow_beta(&mut self, on: bool) {
        self.config.allow_beta = on;
        if !on {
            self.config.allow_alpha = false;
        }
        self.channel_changed();
    }

    pub fn set_allow_alpha(&mut self, on: bool) {
        self.config.allow_alpha = on;
        if on {
            self.config.allow_beta = true;
        }
        self.channel_changed();
    }

    fn channel_changed(&mut self) {
        self.save_config();
        self.statuses.clear();
        self.check();
    }

    /// Light, dark, or follow the desktop; the colours cross-fade.
    pub fn set_appearance(&mut self, appearance: Appearance, tx: &mut Tx) {
        if self.config.appearance == appearance {
            return;
        }
        self.config.appearance = appearance;
        self.save_config();
        tx.color_motion(motion::CALM);
    }

    fn client(&self) -> Client {
        let client = Client::new();
        match &self.options.base_url {
            Some(url) => client.with_base_url(url),
            None => client,
        }
    }

    // ---- scanning and checking ---------------------------------------

    /// Re-reads the AddOns folder; with `then_check`, follows with an
    /// update check.
    pub fn reload(&mut self, then_check: bool) {
        self.addons.clear();
        self.statuses.clear();
        self.confirm_remove = None;
        self.error = None;
        let Some(flavor) = self.current_flavor().cloned() else {
            return;
        };
        self.busy = Some(Busy::Scanning);
        self.worker.spawn(move || {
            let result = list_installed(&flavor.addons_dir);
            Done::Scanned {
                addons_dir: flavor.addons_dir,
                then_check,
                result,
            }
        });
    }

    /// Asks CurseForge which addons have newer files.
    pub fn check(&mut self) {
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
        self.worker.spawn(move || {
            let result = manager::check_updates(&client, &flavor, &addons, channel);
            Done::Checked {
                addons_dir: flavor.addons_dir,
                result,
            }
        });
    }

    pub fn status_of(&self, addon: &InstalledAddon) -> Option<&UpdateStatus> {
        self.statuses.get(&addon.project_id?)
    }

    pub fn outdated(&self) -> Vec<Candidate> {
        let mut seen = HashSet::new();
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
    pub fn update_all(&mut self) {
        let jobs = self.outdated();
        let label = format!("{} addons", jobs.len());
        self.run_installs(jobs, label);
    }

    pub fn update_one(&mut self, candidate: Candidate) {
        let label = candidate.mod_.name.clone();
        self.run_installs(vec![candidate], label);
    }

    /// Installs the newest suitable file of a search result.
    pub fn install(&mut self, mod_: Mod) {
        if self.busy.is_some() {
            return;
        }
        let Some(flavor) = self.current_flavor().cloned() else {
            return;
        };
        let client = self.client();
        let channel = self.config.channel();
        self.begin(Busy::Installing(mod_.name.clone()));
        let progress = self.worker.sink();
        self.worker.spawn(move || {
            let outcome =
                manager::resolve_install(&client, &flavor, &mod_, channel).and_then(|candidate| {
                    run_install(&client, &flavor, vec![candidate], channel, &progress)
                });
            Done::Installed(outcome)
        });
    }

    fn run_installs(&mut self, jobs: Vec<Candidate>, label: String) {
        if jobs.is_empty() || self.busy.is_some() {
            return;
        }
        let Some(flavor) = self.current_flavor().cloned() else {
            return;
        };
        let client = self.client();
        let channel = self.config.channel();
        self.begin(Busy::Installing(label));
        let progress = self.worker.sink();
        self.worker.spawn(move || {
            Done::Installed(run_install(&client, &flavor, jobs, channel, &progress))
        });
    }

    fn begin(&mut self, busy: Busy) {
        self.busy = Some(busy);
        self.error = None;
        self.progress = None;
    }

    // ---- removing -----------------------------------------------------

    /// First click arms the button; the second deletes.
    pub fn remove(&mut self, addon: InstalledAddon) {
        let key = addon.folders.first().cloned().unwrap_or_default();
        if self.confirm_remove.as_ref() != Some(&key) {
            self.confirm_remove = Some(key);
            return;
        }
        let Some(flavor) = self.current_flavor().cloned() else {
            return;
        };
        self.confirm_remove = None;
        self.busy = Some(Busy::Scanning);
        self.worker.spawn(move || {
            let addons_dir = flavor.addons_dir;
            let result =
                manager::uninstall(&addons_dir, &addon).and_then(|()| list_installed(&addons_dir));
            Done::Removed {
                name: addon.name,
                result,
            }
        });
    }

    // ---- search -------------------------------------------------------

    /// Switches tabs; the first visit to Find lists the most popular addons.
    pub fn open_tab(&mut self, tab: Tab) {
        self.tab = tab;
        if tab == Tab::Find && matches!(self.search_state, SearchState::Idle) {
            self.search();
        }
    }

    /// Searches for the box's text; an empty box lists the most popular addons.
    /// A new query supersedes one still in flight.
    pub fn search(&mut self) {
        let query = self.search.text.trim().to_string();
        if matches!(&self.search_state, SearchState::Searching { query: q } if *q == query) {
            return;
        }
        let client = self.client();
        self.search_state = SearchState::Searching {
            query: query.clone(),
        };
        self.worker.spawn(move || {
            let result = client.search(&query, 0, 30);
            Done::Searched { query, result }
        });
    }

    // ---- results ------------------------------------------------------

    fn apply(&mut self, done: Done, tx: &mut Tx) {
        match done {
            Done::Scanned {
                addons_dir,
                then_check,
                result,
            } => {
                // The selection may have moved on while scanning.
                if !self.is_current(&addons_dir) {
                    return;
                }
                self.busy = None;
                // A fresh list lands whole rather than every row fading in.
                tx.instant();
                match result {
                    Ok(addons) => self.addons = addons,
                    Err(e) => self.error = Some(e.to_string()),
                }
                if then_check && !self.addons.is_empty() {
                    self.check();
                }
            }
            Done::Checked { addons_dir, result } => {
                if !self.is_current(&addons_dir) {
                    return;
                }
                self.busy = None;
                match result {
                    Ok(statuses) => self.statuses = statuses,
                    Err(e) => self.error = Some(e.to_string()),
                }
            }
            Done::Progress(p) => {
                if matches!(self.busy, Some(Busy::Installing(_))) {
                    self.progress = Some(p);
                }
            }
            Done::Installed(outcome) => self.finished(outcome, tx),
            Done::Removed { name, result } => {
                self.busy = None;
                match result {
                    Ok(addons) => {
                        self.addons = addons;
                        self.note(NoteKind::Success, format!("Removed {name}"), tx);
                    }
                    Err(e) => self.note(NoteKind::Error, e.to_string(), tx),
                }
            }
            Done::Searched { query, result } => {
                // Only the newest query's answer counts.
                if !matches!(&self.search_state, SearchState::Searching { query: q } if *q == query)
                {
                    return;
                }
                match result {
                    Ok(page) => {
                        self.search_state = SearchState::Results {
                            query,
                            total: page.total,
                        };
                        self.search_results = page.items;
                    }
                    Err(e) => self.search_state = SearchState::Failed(e.to_string()),
                }
            }
        }
    }

    fn is_current(&self, addons_dir: &PathBuf) -> bool {
        self.current_flavor().map(|f| &f.addons_dir) == Some(addons_dir)
    }

    fn finished(&mut self, outcome: InstallOutcome, tx: &mut Tx) {
        self.busy = None;
        self.progress = None;
        match outcome {
            Ok(InstallDone {
                report,
                addons,
                statuses,
            }) => {
                self.addons = addons;
                self.statuses = statuses;
                self.confirm_remove = None;
                if !report.installed.is_empty() {
                    let message = match report.installed.as_slice() {
                        [one] => format!("Installed {one}"),
                        many => format!("Installed {} addons", many.len()),
                    };
                    self.note(NoteKind::Success, message, tx);
                }
                for (name, reason) in report.failed {
                    self.note(NoteKind::Error, format!("{name}: {reason}"), tx);
                }
            }
            Err(e) => self.note(NoteKind::Error, e.to_string(), tx),
        }
    }

    // ---- notifications ------------------------------------------------

    fn note(&mut self, kind: NoteKind, text: String, tx: &mut Tx) {
        let seconds = match kind {
            NoteKind::Success => NOTE_SECONDS,
            NoteKind::Error => ERROR_NOTE_SECONDS,
        };
        self.next_note += 1;
        self.notes.push(Note {
            id: self.next_note,
            kind,
            text,
            until: self.clock + seconds,
        });
        self.schedule_note_expiry(tx);
    }

    pub fn dismiss_note(&mut self, id: u64) {
        self.notes.retain(|n| n.id != id);
    }

    /// Wakes `poll` when the oldest notification is due to go.
    fn schedule_note_expiry(&self, tx: &mut Tx) {
        if let Some(next) = self.notes.iter().map(|n| n.until).min_by(f64::total_cmp) {
            tx.wake_after(Duration::from_secs_f64((next - self.clock).max(0.0)));
        }
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

type InstallOutcome = wowzers_core::Result<InstallDone>;

/// Installs `jobs`, then re-reads the folder and re-checks so the list shows
/// what is really on disk.
fn run_install(
    client: &Client,
    flavor: &FlavorInstall,
    jobs: Vec<Candidate>,
    channel: ReleaseType,
    deliver: &(impl Fn(Done) + Sync),
) -> InstallOutcome {
    let report = manager::install_all(client, &flavor.addons_dir, &jobs, &|p| {
        deliver(Done::Progress(p))
    })?;
    let addons = list_installed(&flavor.addons_dir)?;
    let statuses = manager::check_updates(client, flavor, &addons, channel).unwrap_or_default();
    Ok(InstallDone {
        report,
        addons,
        statuses,
    })
}
