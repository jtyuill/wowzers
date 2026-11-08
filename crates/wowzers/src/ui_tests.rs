//! The real view against an in-process fake CurseForge and a throwaway WoW
//! folder, driven through clicks on a headless Rune runtime.

use std::{
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use rune::{Env, Runtime};
use wowzers_core::{
    config::{Appearance, Config},
    manager::UpdateStatus,
    testing::{FakeFile, FakeProject, Server, serve, wow_with_addons, zip_bytes},
};

use crate::{
    app::{self, Options, SearchState, Tab, Wowzers},
    render, theme,
};

const WAIT: Duration = Duration::from_secs(15);
const FRAME: f64 = 1.0 / 60.0;

struct Harness {
    rt: Runtime<Wowzers>,
    addons_dir: PathBuf,
    config_path: PathBuf,
    _server: Server,
    _tmp: tempfile::TempDir,
}

impl Harness {
    fn start() -> Self {
        Self::start_with(Env::new(false, 1.0))
    }

    fn start_with(env: Env) -> Self {
        let alpha = zip_bytes(&[
            (
                "Alpha/Alpha.toc",
                "## Title: Alpha\n## Version: 2.0\n## X-Curse-Project-ID: 100\n",
            ),
            ("Alpha/new.lua", "new"),
        ]);
        let beta = zip_bytes(&[
            (
                "Beta/Beta.toc",
                "## Title: Beta\n## X-Curse-Project-ID: 200\n",
            ),
            ("Beta/new.lua", "new"),
        ]);
        let gamma = zip_bytes(&[
            ("Gamma/Gamma.toc", "## Title: Gamma\n"),
            ("Gamma/new.lua", "new"),
        ]);
        let server = serve(vec![
            FakeProject::new(
                100,
                "Alpha",
                vec![FakeFile::new(1002, "Alpha-2.0", &["11.0.2"]).with_blob(alpha)],
            ),
            FakeProject::new(
                200,
                "Beta",
                vec![FakeFile::new(2002, "Beta-1.0", &["11.0.2"]).with_blob(beta)],
            ),
            FakeProject::new(
                300,
                "Gamma Search",
                vec![FakeFile::new(3002, "Gamma-3.0", &["11.0.2"]).with_blob(gamma)],
            ),
        ]);

        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("World of Warcraft");
        let flavor = wow_with_addons(
            &root,
            &[
                (
                    "Alpha",
                    "## Title: Alpha\n## Version: 1.0\n## X-Curse-Project-ID: 100\n",
                ),
                (
                    "Beta",
                    "## Title: Beta\n## Version: 0.9\n## X-Curse-Project-ID: 200\n",
                ),
                ("Local", "## Title: Local only\n"),
            ],
        );
        let config_path = tmp.path().join("config/config.json");
        std::fs::create_dir_all(config_path.parent().unwrap()).unwrap();
        std::fs::write(
            &config_path,
            serde_json::json!({ "wow_roots": [root] }).to_string(),
        )
        .unwrap();

        let options = Options {
            config_path: Some(config_path.clone()),
            base_url: Some(server.base.clone()),
            scan_default_locations: false,
        };
        let mut rt = Runtime::new(Wowzers::new(options), render::view, (980.0, 720.0), env);
        rt.poll = Some(app::poll);
        app::setup(&mut rt.state, Arc::new(|| {}));
        rt.frame(0.0);
        Harness {
            rt,
            addons_dir: flavor.addons_dir,
            config_path,
            _server: server,
            _tmp: tmp,
        }
    }

    /// Runs frames until `check` holds; worker threads answer in real time.
    fn until(&mut self, check: impl Fn(&Wowzers) -> bool) {
        let start = Instant::now();
        loop {
            self.rt.frame(FRAME);
            if check(&self.rt.state) {
                return;
            }
            assert!(start.elapsed() < WAIT, "timed out waiting");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// The startup scan and update check are done.
    fn checked(&mut self) {
        self.until(|app| app.busy.is_none() && !app.statuses.is_empty());
    }

    /// Presses the element at `key` (a key or key path) once motion has landed.
    fn click(&mut self, key: &str) {
        self.rt.settle_all();
        self.rt.frame(FRAME);
        let (x, y) = self
            .rt
            .find(key)
            .unwrap_or_else(|| panic!("no element {key:?} on screen"));
        self.rt.pointer_down(x, y);
        self.rt.pointer_up(x, y);
        self.rt.frame(FRAME);
    }

    /// The window's background colour, as drawn once colours have landed.
    fn background(&mut self) -> String {
        self.rt.settle_all();
        self.rt.frame(FRAME);
        let window = self
            .rt
            .elements()
            .into_iter()
            .find(|e| e.key.as_deref() == Some("window"));
        window.expect("the window element").fill.hex()
    }

    fn read(&self, path: &str) -> Option<String> {
        std::fs::read_to_string(self.addons_dir.join(path)).ok()
    }
}

fn light_bg() -> String {
    theme::kit_for(false).c.bg_base.hex()
}

fn dark_bg() -> String {
    theme::kit_for(true).c.bg_base.hex()
}

#[test]
fn startup_finds_the_install_and_flags_outdated_addons() {
    let mut app = Harness::start();
    app.checked();
    let state = &app.rt.state;
    let names: Vec<_> = state.addons.iter().map(|a| a.name.as_str()).collect();
    assert_eq!(names, ["Alpha", "Beta", "Local only"]);
    assert_eq!(state.outdated().len(), 2);
    assert!(matches!(
        state.status_of(&state.addons[0]),
        Some(UpdateStatus::Outdated(_))
    ));
    assert!(
        state.status_of(&state.addons[2]).is_none(),
        "no project id, nothing to check"
    );
}

#[test]
fn update_all_installs_every_outdated_addon_in_one_click() {
    let mut app = Harness::start();
    app.checked();
    assert_eq!(app.read("Alpha/new.lua"), None);

    app.click("update-all");
    app.until(|app| app.busy.is_none() && !app.statuses.is_empty() && app.outdated().is_empty());

    assert_eq!(app.read("Alpha/new.lua").as_deref(), Some("new"));
    assert_eq!(app.read("Beta/new.lua").as_deref(), Some("new"));
    assert_eq!(app.read("Alpha/old.lua"), None, "replaced, not merged");
    assert!(
        app.read("Local/old.lua").is_some(),
        "other addons are untouched"
    );
    let state = &app.rt.state;
    assert_eq!(
        state.addons.iter().filter(|a| a.tracked.is_some()).count(),
        2
    );
    assert!(matches!(
        state.status_of(&state.addons[0]),
        Some(UpdateStatus::UpToDate)
    ));
    assert!(
        state.notes.iter().any(|n| n.text == "Installed 2 addons"),
        "a notification says what happened"
    );
}

#[test]
fn find_tab_searches_and_installs() {
    let mut app = Harness::start();
    app.checked();
    app.click("tabs/segment1");
    assert_eq!(app.rt.state.tab, Tab::Find);

    app.rt.state.search.replace_text("gamma");
    app.click("search");
    app.until(|app| matches!(app.search_state, SearchState::Results { .. }));
    assert_eq!(app.rt.state.search_results.len(), 1);
    assert_eq!(app.rt.state.search_results[0].name, "Gamma Search");
    assert!(!app.rt.state.is_installed(300));

    app.click("result-300/install");
    app.until(|app| app.busy.is_none() && app.is_installed(300));
    assert_eq!(app.read("Gamma/new.lua").as_deref(), Some("new"));
    let installed = app
        .rt
        .state
        .addons
        .iter()
        .find(|a| a.project_id == Some(300))
        .unwrap();
    assert_eq!(installed.tracked.as_ref().unwrap().file_id, 3002);
}

#[test]
fn find_tab_lists_popular_addons_on_first_visit() {
    let mut app = Harness::start();
    app.checked();
    app.click("tabs/segment1");
    app.until(|app| {
        matches!(&app.search_state, SearchState::Results { query, .. } if query.is_empty())
    });
    let names: Vec<_> = app.rt.state.search_results.iter().map(|m| m.name.as_str()).collect();
    assert_eq!(names, ["Alpha", "Beta", "Gamma Search"]);

    app.click("tabs/segment0");
    app.rt.state.search.replace_text("gamma");
    app.click("tabs/segment1");
    assert!(
        matches!(&app.rt.state.search_state, SearchState::Results { query, .. } if query.is_empty()),
        "coming back keeps the list instead of searching again"
    );
}

#[test]
fn remove_needs_a_second_click() {
    let mut app = Harness::start();
    app.checked();
    let local = app.addons_dir.join("Local");
    assert!(local.exists());

    app.click("addon-Local/remove");
    assert_eq!(app.rt.state.confirm_remove.as_deref(), Some("Local"));
    assert!(local.exists(), "the first click only arms the button");

    app.click("addon-Local/remove");
    app.until(|app| app.busy.is_none() && app.addons.len() == 2);
    assert!(!local.exists());
    assert!(app.addons_dir.join("Alpha").exists());
}

#[test]
fn notifications_leave_on_their_own() {
    let mut app = Harness::start();
    app.checked();
    app.click("addon-Local/remove");
    app.click("addon-Local/remove");
    app.until(|app| !app.notes.is_empty());
    assert!(app.rt.find("toast-region").is_some());

    // The app clock is virtual here: ten seconds of frames, no waiting.
    for _ in 0..600 {
        app.rt.frame(FRAME);
    }
    assert!(app.rt.state.notes.is_empty());
}

#[test]
fn theme_follows_the_desktop_until_the_toggle_picks_one() {
    let mut app = Harness::start_with(Env::new(true, 1.0));
    assert_eq!(app.rt.state.config.appearance, Appearance::Auto);
    assert_eq!(
        app.background(),
        dark_bg(),
        "Auto on a dark desktop is dark"
    );

    app.click("theme-toggle");
    assert_eq!(app.rt.state.config.appearance, Appearance::Light);
    assert_eq!(app.background(), light_bg());
    let saved = Config::load(&app.config_path).unwrap();
    assert_eq!(saved.appearance, Appearance::Light, "the choice is saved");

    app.rt.set_system_dark(false);
    app.click("theme-toggle");
    assert_eq!(
        app.background(),
        dark_bg(),
        "a forced theme ignores the desktop"
    );

    app.click("tabs/segment2");
    app.click("appearance/segment0");
    assert_eq!(app.rt.state.config.appearance, Appearance::Auto);
    assert_eq!(
        app.background(),
        light_bg(),
        "Auto follows the light desktop again"
    );
}
