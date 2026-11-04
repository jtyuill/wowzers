//! Runs the whole check -> update-all -> verify flow against the fake site.

use wowzers_core::{
    curseforge::{Client, ReleaseType},
    installed::list_installed,
    manager::{Progress, UpdateStatus, check_updates, install_all, resolve_install, uninstall},
    manifest::{Manifest, Tracked},
    testing::{FakeFile, FakeProject, serve, wow_with_addons, zip_bytes},
};

fn tracked(id: u32, name: &str, file_id: u32, folders: &[&str]) -> Tracked {
    Tracked {
        project_id: id,
        name: name.into(),
        slug: name.to_lowercase(),
        file_id,
        file_name: "old.zip".into(),
        display_name: "old".into(),
        installed_at: 1,
        folders: folders.iter().map(|s| s.to_string()).collect(),
    }
}

fn no_progress() -> impl Fn(Progress) + Sync {
    |_| {}
}

#[test]
fn check_then_update_all_end_to_end() {
    let zip_a = zip_bytes(&[
        (
            "Alpha/Alpha.toc",
            "## Title: Alpha\n## Version: 2.0\n## X-Curse-Project-ID: 100\n",
        ),
        ("Alpha/new.lua", "new"),
    ]);
    let zip_c = zip_bytes(&[("Gamma/Gamma.toc", "## Title: Gamma\n")]);
    let server = serve(vec![
        FakeProject::new(
            100,
            "Alpha",
            vec![
                FakeFile::new(1002, "Alpha-2.0", &["11.0.2"]).with_blob(zip_a),
                // A beta and a classic file must not win for a release client.
                FakeFile::new(1100, "Alpha-2.1-beta", &["11.0.5"]).beta(),
                FakeFile::new(1500, "Alpha-era", &["1.15.4"]),
            ],
        ),
        // Untracked, TOC version doesn't match its file -> outdated; the
        // download is not a zip, so installing it fails.
        FakeProject::new(
            200,
            "Beta",
            vec![FakeFile::new(2002, "Beta-1.0", &["11.0.2"]).with_blob(b"not a zip".to_vec())],
        ),
        // Untracked, TOC version appears in the file name -> current.
        FakeProject::new(
            300,
            "Gamma",
            vec![FakeFile::new(3002, "Gamma-3.1", &["11.0.2"]).with_blob(zip_c)],
        ),
        // Classic files only.
        FakeProject::new(
            400,
            "Delta",
            vec![FakeFile::new(4002, "Delta-1.0", &["1.15.4"])],
        ),
        // Tracked at the newest already.
        FakeProject::new(
            500,
            "Epsilon",
            vec![FakeFile::new(5002, "Epsilon-1.0", &["11.0.2"])],
        ),
    ]);

    let tmp = tempfile::tempdir().unwrap();
    let flavor = wow_with_addons(
        tmp.path(),
        &[
            ("Alpha", "## Title: Alpha\n## Version: 1.0\n"),
            (
                "Beta",
                "## Title: Beta\n## Version: 0.9\n## X-Curse-Project-ID: 200\n",
            ),
            (
                "Gamma",
                "## Title: Gamma\n## Version: 3.1\n## X-Curse-Project-ID: 300\n",
            ),
            ("Delta", "## Title: Delta\n## X-Curse-Project-ID: 400\n"),
            ("Epsilon", "## Title: Epsilon\n"),
            ("Local", "## Title: Local only\n"),
        ],
    );
    let mut manifest = Manifest::default();
    manifest.upsert(tracked(100, "Alpha", 1001, &["Alpha"]));
    manifest.upsert(tracked(500, "Epsilon", 5002, &["Epsilon"]));
    manifest.save(&flavor.addons_dir).unwrap();

    let client = Client::new().with_base_url(&server.base);
    let addons = list_installed(&flavor.addons_dir).unwrap();
    let statuses = check_updates(&client, &flavor, &addons, ReleaseType::Release).unwrap();

    assert!(
        matches!(&statuses[&100], UpdateStatus::Outdated(c) if c.file.id == 1002),
        "{:?}",
        statuses[&100]
    );
    assert!(matches!(&statuses[&200], UpdateStatus::Outdated(c) if c.file.id == 2002));
    assert_eq!(statuses[&300], UpdateStatus::UpToDate);
    assert_eq!(statuses[&400], UpdateStatus::NoCompatibleFile);
    assert_eq!(statuses[&500], UpdateStatus::UpToDate);
    assert_eq!(statuses.len(), 5, "the local-only addon has no project id");
    {
        let log = server.log.lock().unwrap();
        let page_calls: Vec<_> = log.iter().filter(|l| l.contains("/wow/addons/")).collect();
        assert_eq!(
            page_calls.len(),
            5,
            "one project page per addon: {page_calls:?}"
        );
        let redirects: Vec<_> = log.iter().filter(|l| l.contains("/projects/")).collect();
        assert_eq!(
            redirects.len(),
            3,
            "only addons without a manifest slug need the redirect: {redirects:?}"
        );
    }

    // One click: update everything outdated.
    let jobs: Vec<_> = statuses
        .values()
        .filter_map(|s| match s {
            UpdateStatus::Outdated(c) => Some((**c).clone()),
            _ => None,
        })
        .collect();
    assert_eq!(jobs.len(), 2);
    let report = install_all(&client, &flavor.addons_dir, &jobs, &no_progress()).unwrap();

    assert_eq!(report.installed, ["Alpha"]);
    assert_eq!(report.failed.len(), 1);
    assert_eq!(report.failed[0].0, "Beta");

    let addons_dir = &flavor.addons_dir;
    assert_eq!(
        std::fs::read_to_string(addons_dir.join("Alpha/new.lua")).unwrap(),
        "new"
    );
    assert!(!addons_dir.join("Alpha/old.lua").exists());
    assert!(
        addons_dir.join("Beta/old.lua").exists(),
        "failed update leaves the old version in place"
    );
    assert!(!addons_dir.join("Beta/new.lua").exists());
    assert!(!addons_dir.join(".wowzers-downloads").exists());

    let manifest = Manifest::load(addons_dir).unwrap();
    assert_eq!(manifest.get(100).unwrap().file_id, 1002);
    assert_eq!(manifest.get(500).unwrap().file_id, 5002, "untouched");
    assert!(manifest.get(200).is_none());

    // Alpha is current now; Beta still isn't.
    let addons = list_installed(addons_dir).unwrap();
    let statuses = check_updates(&client, &flavor, &addons, ReleaseType::Release).unwrap();
    assert_eq!(statuses[&100], UpdateStatus::UpToDate);
    assert!(matches!(statuses[&200], UpdateStatus::Outdated(_)));
}

#[test]
fn search_install_and_uninstall() {
    let zip = zip_bytes(&[
        ("Fresh/Fresh.toc", "## Title: Fresh\n"),
        ("Fresh_Opts/Fresh_Opts.toc", "## Title: o\n"),
    ]);
    let server = serve(vec![FakeProject::new(
        900,
        "Fresh",
        vec![FakeFile::new(9002, "Fresh-1", &["11.0.2"]).with_blob(zip)],
    )]);
    let tmp = tempfile::tempdir().unwrap();
    let flavor = wow_with_addons(tmp.path(), &[]);
    let client = Client::new().with_base_url(&server.base);

    let page = client.search("fresh", 0, 20).unwrap();
    assert_eq!(
        (
            page.total,
            page.items[0].name.as_str(),
            page.items[0].download_count
        ),
        (1, "Fresh", 1234)
    );
    assert_eq!(page.items[0].author_names(), "me");

    let candidate =
        resolve_install(&client, &flavor, &page.items[0], ReleaseType::Release).unwrap();
    let progress = std::sync::Mutex::new(Vec::new());
    let report = install_all(&client, &flavor.addons_dir, &[candidate], &|p| {
        progress.lock().unwrap().push(p)
    })
    .unwrap();
    assert_eq!(report.installed, ["Fresh"]);
    let last = progress.lock().unwrap().last().cloned().unwrap();
    assert_eq!((last.done, last.total), (1, 1));

    let addons = list_installed(&flavor.addons_dir).unwrap();
    assert_eq!(addons.len(), 1);
    assert_eq!(addons[0].folders, ["Fresh", "Fresh_Opts"]);
    assert_eq!(addons[0].tracked.as_ref().unwrap().file_id, 9002);

    uninstall(&flavor.addons_dir, &addons[0]).unwrap();
    assert!(list_installed(&flavor.addons_dir).unwrap().is_empty());
    assert!(!flavor.addons_dir.join("Fresh").exists());
    assert!(
        Manifest::load(&flavor.addons_dir)
            .unwrap()
            .addons
            .is_empty()
    );
}

#[test]
fn unknown_project_is_reported_as_not_found() {
    let server = serve(vec![]);
    let tmp = tempfile::tempdir().unwrap();
    let flavor = wow_with_addons(
        tmp.path(),
        &[(
            "Ghost",
            "## Title: Ghost\n## Version: 1\n## X-Curse-Project-ID: 424242\n",
        )],
    );
    let client = Client::new().with_base_url(&server.base);
    assert!(client.get_mod(424242, Some("ghost")).unwrap().is_none());
    assert!(client.get_mod(424242, None).unwrap().is_none());

    let addons = list_installed(&flavor.addons_dir).unwrap();
    let statuses = check_updates(&client, &flavor, &addons, ReleaseType::Release).unwrap();
    assert_eq!(statuses[&424242], UpdateStatus::NotFound);
}
