//! Runs the whole check -> update-all -> verify flow against the fake API.

use std::collections::HashMap;

use serde_json::{Value, json};
use wowzers_core::{
    curseforge::{Client, ReleaseType},
    error::Error,
    installed::list_installed,
    manager::{Progress, UpdateStatus, check_updates, install_all, resolve_install, uninstall},
    manifest::{Manifest, Tracked},
    testing::{blobs, file_json, index, mod_json, serve, sha1_hex, wow_with_addons, zip_bytes},
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
    let zip_b = zip_bytes(&[
        ("Beta/Beta.toc", "## Title: Beta\n"),
        ("Beta/new.lua", "new"),
    ]);
    let zip_c = zip_bytes(&[("Gamma/Gamma.toc", "## Title: Gamma\n")]);
    let server = serve(
        vec![
            // tracked at 1001; newest release for 11.x is 1002 (a beta 1100 and a classic 1500 must not win)
            mod_json(
                100,
                "Alpha",
                vec![
                    index("11.0.2", 1002, 1),
                    index("11.0.5", 1100, 2),
                    index("1.15.4", 1500, 1),
                ],
            ),
            // untracked, TOC version doesn't match its file -> outdated; the download is corrupt
            mod_json(200, "Beta", vec![index("11.0.2", 2002, 1)]),
            // untracked, TOC version appears in the file name -> current
            mod_json(300, "Gamma", vec![index("11.0.2", 3002, 1)]),
            // classic files only
            mod_json(400, "Delta", vec![index("1.15.4", 4002, 1)]),
            // tracked at the newest already
            mod_json(500, "Epsilon", vec![index("11.0.2", 5002, 1)]),
        ],
        vec![
            file_json(1002, 100, "Alpha-2.0", "/dl/a.zip", &sha1_hex(&zip_a)),
            file_json(2002, 200, "Beta-1.0", "/dl/b.zip", &"0".repeat(40)),
            file_json(3002, 300, "Gamma-v3.1", "/dl/c.zip", &sha1_hex(&zip_c)),
        ],
        blobs(&[
            ("/dl/a.zip", &zip_a),
            ("/dl/b.zip", &zip_b),
            ("/dl/c.zip", &zip_c),
        ]),
        "KEY",
    );

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

    let client = Client::new("KEY").unwrap().with_base_url(&server.base);
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
        let api_calls: Vec<_> = log.iter().filter(|l| !l.contains("/dl/")).collect();
        assert_eq!(
            api_calls.len(),
            2,
            "one batched mods call and one batched files call: {api_calls:?}"
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
    assert!(
        report.failed[0].1.contains("checksum mismatch"),
        "{}",
        report.failed[0].1
    );

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
    let server = serve(
        vec![mod_json(900, "Fresh", vec![index("11.0.2", 9002, 1)])],
        vec![file_json(
            9002,
            900,
            "Fresh-1",
            "/dl/f.zip",
            &sha1_hex(&zip),
        )],
        blobs(&[("/dl/f.zip", &zip)]),
        "KEY",
    );
    let tmp = tempfile::tempdir().unwrap();
    let flavor = wow_with_addons(tmp.path(), &[]);
    let client = Client::new("KEY").unwrap().with_base_url(&server.base);

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
fn rejected_key_is_reported_as_such() {
    let server = serve(vec![], vec![], HashMap::new(), "KEY");
    let client = Client::new("WRONG").unwrap().with_base_url(&server.base);
    assert!(matches!(
        client.search("x", 0, 10),
        Err(Error::BadApiKey(403))
    ));
    assert!(matches!(client.get_mods(&[1]), Err(Error::BadApiKey(403))));
}

#[test]
fn author_disabled_distribution_is_not_offered_for_update() {
    let mut m = mod_json(600, "Locked", vec![index("11.0.2", 6002, 1)]);
    m["allowModDistribution"] = json!(false);
    let mut f = file_json(6002, 600, "Locked-1", "/dl/x.zip", &"0".repeat(40));
    f["downloadUrl"] = Value::Null;
    let server = serve(vec![m], vec![f], HashMap::new(), "KEY");
    let tmp = tempfile::tempdir().unwrap();
    let flavor = wow_with_addons(
        tmp.path(),
        &[("Locked", "## Title: Locked\n## X-Curse-Project-ID: 600\n")],
    );
    let client = Client::new("KEY").unwrap().with_base_url(&server.base);
    let addons = list_installed(&flavor.addons_dir).unwrap();
    let statuses = check_updates(&client, &flavor, &addons, ReleaseType::Release).unwrap();
    assert_eq!(statuses[&600], UpdateStatus::NotDownloadable);
}
