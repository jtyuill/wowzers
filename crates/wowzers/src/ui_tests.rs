//! The real window against an in-process fake CurseForge and a throwaway
//! WoW folder, driven through clicks.

use std::{path::PathBuf, time::Duration};

use gpui_kit::test::{TestAppContextExt as _, TestWindowExt as _};
use gpui_kit::{
    AnyWindowHandle, App, AppContext as _, Bounds, Entity, TestAppContext, Window, WindowBounds,
    WindowOptions, px, size,
};
use wowzers_core::{
    manager::UpdateStatus,
    testing::{FakeFile, FakeProject, Server, serve, wow_with_addons, zip_bytes},
};

use crate::app::{Options, SearchState, Tab, Wowzers};

const WAIT: Duration = Duration::from_secs(15);

struct Harness {
    window: AnyWindowHandle,
    view: Entity<Wowzers>,
    addons_dir: PathBuf,
    _server: Server,
    _tmp: tempfile::TempDir,
}

impl Harness {
    async fn start(cx: &mut TestAppContext) -> Self {
        // Downloads and API calls block background threads; GPUI waits for them.
        cx.executor().allow_parking();
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
            config_path: Some(config_path),
            base_url: Some(server.base.clone()),
            scan_default_locations: false,
        };
        let (window, view) = cx.update(|cx| {
            gpui_kit::init(cx);
            gpui_kit::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                        Default::default(),
                        size(px(980.), px(720.)),
                    ))),
                    ..Default::default()
                },
                cx,
                |window, cx| cx.new(|cx| Wowzers::new(options, window, cx)),
            )
            .expect("open the test window")
        });
        Harness {
            window,
            view,
            addons_dir: flavor.addons_dir,
            _server: server,
            _tmp: tmp,
        }
    }

    async fn until(&self, cx: &mut TestAppContext, check: impl Fn(&Wowzers) -> bool) {
        let view = self.view.clone();
        cx.wait_for(self.window, WAIT, move |_, cx| check(view.read(cx)))
            .await;
    }

    /// The startup scan and update check are done.
    async fn checked(&self, cx: &mut TestAppContext) {
        self.until(cx, |app| app.busy.is_none() && !app.statuses.is_empty())
            .await;
    }

    fn update<R>(&self, cx: &mut TestAppContext, f: impl FnOnce(&mut Window, &mut App) -> R) -> R {
        cx.update_window(self.window, |_, window, cx| f(window, cx))
            .expect("the test window is open")
    }

    fn read(&self, path: &str) -> Option<String> {
        std::fs::read_to_string(self.addons_dir.join(path)).ok()
    }
}

#[gpui_kit::test]
async fn startup_finds_the_install_and_flags_outdated_addons(cx: &mut TestAppContext) {
    let app = Harness::start(cx).await;
    app.checked(cx).await;
    app.view.read_with(cx, |app, _| {
        let names: Vec<_> = app.addons.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, ["Alpha", "Beta", "Local only"]);
        assert_eq!(app.outdated().len(), 2);
        assert!(matches!(
            app.status_of(&app.addons[0]),
            Some(UpdateStatus::Outdated(_))
        ));
        assert!(
            app.status_of(&app.addons[2]).is_none(),
            "no project id, nothing to check"
        );
    });
}

#[gpui_kit::test]
async fn update_all_installs_every_outdated_addon_in_one_click(cx: &mut TestAppContext) {
    let app = Harness::start(cx).await;
    app.checked(cx).await;
    assert_eq!(app.read("Alpha/new.lua"), None);

    app.update(cx, |window, cx| window.click("update-all", cx));
    app.until(cx, |app| {
        app.busy.is_none() && !app.statuses.is_empty() && app.outdated().is_empty()
    })
    .await;

    assert_eq!(app.read("Alpha/new.lua").as_deref(), Some("new"));
    assert_eq!(app.read("Beta/new.lua").as_deref(), Some("new"));
    assert_eq!(app.read("Alpha/old.lua"), None, "replaced, not merged");
    assert!(
        app.read("Local/old.lua").is_some(),
        "other addons are untouched"
    );
    app.view.read_with(cx, |app, _| {
        assert!(app.addons.iter().filter(|a| a.tracked.is_some()).count() == 2);
        assert!(matches!(
            app.status_of(&app.addons[0]),
            Some(UpdateStatus::UpToDate)
        ));
    });
}

#[gpui_kit::test]
async fn find_tab_searches_and_installs(cx: &mut TestAppContext) {
    let app = Harness::start(cx).await;
    app.checked(cx).await;
    app.update(cx, |window, cx| window.click("tab-Find addons", cx));
    app.until(cx, |app| app.tab == Tab::Find).await;

    let view = app.view.clone();
    app.update(cx, |window, cx| {
        view.update(cx, |app, cx| {
            app.search_input
                .update(cx, |input, cx| input.set_value("gamma", window, cx))
        });
        window.click("search", cx);
    });
    app.until(cx, |app| {
        matches!(app.search_state, SearchState::Results { .. })
    })
    .await;
    app.view.read_with(cx, |app, _| {
        assert_eq!(app.search_results.len(), 1);
        assert_eq!(app.search_results[0].name, "Gamma Search");
        assert!(!app.is_installed(300));
    });

    app.update(cx, |window, cx| window.click(("install", 0usize), cx));
    app.until(cx, |app| app.busy.is_none() && app.is_installed(300))
        .await;
    assert_eq!(app.read("Gamma/new.lua").as_deref(), Some("new"));
    app.view.read_with(cx, |app, _| {
        let installed = app
            .addons
            .iter()
            .find(|a| a.project_id == Some(300))
            .unwrap();
        assert_eq!(installed.tracked.as_ref().unwrap().file_id, 3002);
    });
}

#[gpui_kit::test]
async fn remove_needs_a_second_click(cx: &mut TestAppContext) {
    let app = Harness::start(cx).await;
    app.checked(cx).await;
    let local = app.addons_dir.join("Local");
    assert!(local.exists());

    app.update(cx, |window, cx| window.click(("remove", 2usize), cx));
    app.until(cx, |app| app.confirm_remove.as_deref() == Some("Local"))
        .await;
    assert!(local.exists(), "the first click only arms the button");

    app.update(cx, |window, cx| window.click(("remove", 2usize), cx));
    app.until(cx, |app| app.busy.is_none() && app.addons.len() == 2)
        .await;
    assert!(!local.exists());
    assert!(app.addons_dir.join("Alpha").exists());
}
