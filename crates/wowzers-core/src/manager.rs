//! Checking for updates, installing, updating and removing addons.

use std::{
    collections::HashMap,
    path::Path,
    sync::atomic::{AtomicUsize, Ordering},
};

use crate::{
    curseforge::{Client, File, Mod, ReleaseType},
    detect::FlavorInstall,
    error::{Error, Result},
    install,
    installed::InstalledAddon,
    manifest::{Manifest, Tracked, now_secs},
};

/// Concurrent downloads.
const DOWNLOAD_WORKERS: usize = 4;
const DOWNLOADS: &str = ".wowzers-downloads";

/// A file to install, with the project it belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub mod_: Mod,
    pub file: File,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateStatus {
    UpToDate,
    Outdated(Box<Candidate>),
    /// CurseForge has no file for this client version and release channel.
    NoCompatibleFile,
    /// CurseForge doesn't know the project id.
    NotFound,
}

/// Whether `toc_version` appears to be the version `file` carries. CurseForge
/// has no installed-version field, and file names are free text, so this is a
/// containment check; a miss means "treat as outdated", which only costs a
/// reinstall.
pub fn version_matches(toc_version: Option<&str>, file: &File) -> bool {
    let Some(version) = toc_version.map(|v| v.trim().trim_start_matches(['v', 'V']).to_lowercase())
    else {
        return false;
    };
    // `@project-version@` is what an unpackaged checkout carries.
    if version.is_empty() || version.contains('@') {
        return false;
    }
    [&file.display_name, &file.file_name]
        .iter()
        .any(|name| name.to_lowercase().contains(&version))
}

/// Compares every addon that has a CurseForge project id against the newest
/// suitable file. One project page per addon; the manifest's slug saves the
/// redirect lookup for addons wowzers installed.
pub fn check_updates(
    client: &Client,
    flavor: &FlavorInstall,
    addons: &[InstalledAddon],
    channel: ReleaseType,
) -> Result<HashMap<u32, UpdateStatus>> {
    // One entry per project; the wowzers-installed copy is the authority.
    let mut subjects: HashMap<u32, &InstalledAddon> = HashMap::new();
    for addon in addons {
        let Some(id) = addon.project_id else { continue };
        let slot = subjects.entry(id).or_insert(addon);
        if slot.tracked.is_none() && addon.tracked.is_some() {
            *slot = addon;
        }
    }
    let mut ids: Vec<u32> = subjects.keys().copied().collect();
    ids.sort_unstable();
    let slugs: HashMap<u32, String> = subjects
        .iter()
        .filter_map(|(id, addon)| {
            let slug = &addon.tracked.as_ref()?.slug;
            (!slug.is_empty()).then(|| (*id, slug.clone()))
        })
        .collect();
    let mods: HashMap<u32, Mod> = client
        .get_mods(&ids, &slugs)?
        .into_iter()
        .map(|m| (m.id, m))
        .collect();

    let target = flavor.target();
    let mut statuses: HashMap<u32, UpdateStatus> = HashMap::new();
    for &id in &ids {
        let Some(mod_) = mods.get(&id) else {
            statuses.insert(id, UpdateStatus::NotFound);
            continue;
        };
        let Some(best) = mod_.best_file(target, channel) else {
            statuses.insert(id, UpdateStatus::NoCompatibleFile);
            continue;
        };
        let subject = subjects[&id];
        let status = match subject.tracked.as_ref() {
            // File ids grow with upload time; an equal or newer one is current.
            Some(t) if t.file_id >= best.id => UpdateStatus::UpToDate,
            Some(_) => UpdateStatus::Outdated(Box::new(Candidate {
                mod_: mod_.clone(),
                file: best.clone(),
            })),
            // CurseForge has no installed-version field; match the TOC version
            // against the file's name.
            None if version_matches(subject.version.as_deref(), best) => UpdateStatus::UpToDate,
            None => UpdateStatus::Outdated(Box::new(Candidate {
                mod_: mod_.clone(),
                file: best.clone(),
            })),
        };
        statuses.insert(id, status);
    }
    Ok(statuses)
}

/// The newest suitable file of `mod_`, for installing from a search result.
pub fn resolve_install(
    client: &Client,
    flavor: &FlavorInstall,
    mod_: &Mod,
    channel: ReleaseType,
) -> Result<Candidate> {
    // The search card carries identity only; the project page has the files.
    let project = client
        .get_mod(mod_.id, Some(&mod_.slug))?
        .ok_or_else(|| Error::Http(format!("{} is no longer on CurseForge", mod_.name)))?;
    let best = project
        .best_file(flavor.target(), channel)
        .cloned()
        .ok_or_else(|| {
            Error::Http(format!(
                "{} has no file for {}",
                project.name,
                flavor.flavor.label()
            ))
        })?;
    Ok(Candidate {
        mod_: project,
        file: best,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Progress {
    pub done: usize,
    pub total: usize,
    pub message: String,
}

#[derive(Debug, Default)]
pub struct InstallReport {
    /// Names of addons installed or updated.
    pub installed: Vec<String>,
    /// `(addon name, reason)`
    pub failed: Vec<(String, String)>,
}

/// Downloads `jobs` (a few at a time), then installs them one by one. One
/// failing addon doesn't stop the others. Fails outright only if the
/// manifest can't be read, before anything is touched.
pub fn install_all(
    client: &Client,
    addons_dir: &Path,
    jobs: &[Candidate],
    progress: &(dyn Fn(Progress) + Sync),
) -> Result<InstallReport> {
    let mut manifest = Manifest::load(addons_dir)?;
    let mut report = InstallReport::default();
    if jobs.is_empty() {
        return Ok(report);
    }
    let work = addons_dir.join(DOWNLOADS);
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work)
        .map_err(|e| Error::io(format!("creating {}", work.display()), e))?;

    let total = jobs.len();
    let finished = AtomicUsize::new(0);
    let emit = |message: String| {
        progress(Progress {
            done: finished.load(Ordering::SeqCst),
            total,
            message,
        })
    };

    // Phase 1: downloads, in parallel. Slot `i` belongs to `jobs[i]`.
    let next = AtomicUsize::new(0);
    let downloads: Vec<std::sync::Mutex<Option<Result<std::path::PathBuf>>>> =
        jobs.iter().map(|_| std::sync::Mutex::new(None)).collect();
    std::thread::scope(|scope| {
        for _ in 0..DOWNLOAD_WORKERS.min(total) {
            scope.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::SeqCst);
                    let Some(job) = jobs.get(i) else { break };
                    emit(format!("Downloading {}", job.mod_.name));
                    let dest = work.join(format!("{}-{}.zip", job.mod_.id, job.file.id));
                    let url = client.download_url(&job.file);
                    let result = client.download(&url, &job.file, &dest).map(|()| dest);
                    *downloads[i].lock().expect("download slot") = Some(result);
                }
            });
        }
    });

    // Phase 2: install sequentially; they share `AddOns` and the manifest.
    for (job, slot) in jobs.iter().zip(downloads) {
        let name = job.mod_.name.clone();
        emit(format!("Installing {name}"));
        let outcome = slot
            .into_inner()
            .expect("download slot")
            .expect("every job was attempted")
            .and_then(|zip| {
                let stale = manifest
                    .get(job.mod_.id)
                    .map(|t| t.folders.clone())
                    .unwrap_or_default();
                let folders = install::install_zip(addons_dir, &zip, &stale)?;
                manifest.upsert(Tracked {
                    project_id: job.mod_.id,
                    name: name.clone(),
                    slug: job.mod_.slug.clone(),
                    file_id: job.file.id,
                    file_name: job.file.file_name.clone(),
                    display_name: job.file.display_name.clone(),
                    installed_at: now_secs(),
                    folders,
                });
                // Saved per addon so a crash keeps what was installed so far.
                manifest.save(addons_dir)
            });
        finished.fetch_add(1, Ordering::SeqCst);
        match outcome {
            Ok(()) => report.installed.push(name),
            Err(e) => report.failed.push((name, e.to_string())),
        }
    }
    emit("Done".to_string());
    let _ = std::fs::remove_dir_all(&work);
    Ok(report)
}

/// Deletes an addon's folders and forgets it.
pub fn uninstall(addons_dir: &Path, addon: &InstalledAddon) -> Result<()> {
    install::remove_folders(addons_dir, &addon.folders)?;
    if let Some(tracked) = &addon.tracked {
        let mut manifest = Manifest::load(addons_dir)?;
        manifest.remove(tracked.project_id);
        manifest.save(addons_dir)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::curseforge::ReleaseType;

    fn file(display: &str, name: &str) -> File {
        File {
            id: 1,
            display_name: display.into(),
            file_name: name.into(),
            release_type: ReleaseType::Release,
            game_versions: vec![],
        }
    }

    #[test]
    fn version_matching_is_a_conservative_containment_check() {
        let f = file("v11.0.2.5", "Details-v11.0.2.5.zip");
        assert!(version_matches(Some("11.0.2.5"), &f));
        assert!(version_matches(Some("v11.0.2.5"), &f));
        assert!(!version_matches(Some("11.0.2.4"), &f));
        assert!(!version_matches(None, &f));
        assert!(!version_matches(Some("  "), &f));
        // Unpackaged checkout: never claims to be current.
        assert!(!version_matches(
            Some("@project-version@"),
            &file("@project-version@", "x.zip")
        ));
    }
}
