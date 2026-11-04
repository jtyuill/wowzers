//! The addons currently in an `AddOns` folder, grouped into projects.

use std::{
    collections::{BTreeMap, HashSet},
    path::Path,
};

use crate::{
    error::{Error, Result},
    manifest::{Manifest, Tracked},
    toc::Toc,
};

/// One addon folder on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Folder {
    pub name: String,
    pub toc: Option<Toc>,
}

/// An addon as the user sees it: one or more folders from one project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledAddon {
    pub name: String,
    pub folders: Vec<String>,
    pub version: Option<String>,
    pub notes: Option<String>,
    pub author: Option<String>,
    /// CurseForge project, from the manifest or the TOC.
    pub project_id: Option<u32>,
    /// Set when wowzers installed it.
    pub tracked: Option<Tracked>,
}

/// Lists the addon folders, skipping Blizzard's own, hidden folders, and
/// anything without a `.toc`.
pub fn scan_folders(addons_dir: &Path) -> Result<Vec<Folder>> {
    let entries = match std::fs::read_dir(addons_dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(Error::io(format!("reading {}", addons_dir.display()), e)),
    };
    let mut folders = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') || name.starts_with("Blizzard_") || !entry.path().is_dir() {
            continue;
        }
        let toc = read_folder_toc(&entry.path(), &name);
        folders.push(Folder { name, toc });
    }
    folders.sort_by_key(|f| f.name.to_lowercase());
    Ok(folders)
}

/// `Name.toc`, else a per-flavor `Name_Mainline.toc`-style file, else any.
fn read_folder_toc(dir: &Path, name: &str) -> Option<Toc> {
    let exact = dir.join(format!("{name}.toc"));
    if exact.is_file() {
        return Toc::read(&exact).ok();
    }
    let mut tocs: Vec<_> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("toc")))
        .collect();
    tocs.sort();
    tocs.first().and_then(|p| Toc::read(p).ok())
}

fn normalize(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// Folder to describe a group by: the one named like the addon, else the
/// shortest name (`Details` over `Details_Streamer`).
fn primary<'a>(folders: &[&'a Folder], title: Option<&str>) -> Option<&'a Folder> {
    let want = title.map(normalize);
    folders
        .iter()
        .copied()
        .find(|f| want.as_deref().is_some_and(|w| normalize(&f.name) == w))
        .or_else(|| {
            folders
                .iter()
                .copied()
                .filter(|f| f.toc.is_some())
                .min_by_key(|f| f.name.len())
        })
        .or_else(|| folders.first().copied())
}

fn describe(group: &[&Folder], tracked: Option<&Tracked>, id: Option<u32>) -> InstalledAddon {
    let primary_folder = primary(group, tracked.map(|t| t.name.as_str()));
    let toc = primary_folder.and_then(|f| f.toc.as_ref());
    let name = tracked
        .map(|t| t.name.clone())
        .or_else(|| toc.and_then(|t| t.title.clone()))
        .or_else(|| primary_folder.map(|f| f.name.clone()))
        .unwrap_or_default();
    InstalledAddon {
        name,
        folders: group.iter().map(|f| f.name.clone()).collect(),
        version: toc.and_then(|t| t.version.clone()),
        notes: toc.and_then(|t| t.notes.clone()).filter(|n| !n.is_empty()),
        author: toc.and_then(|t| t.author.clone()),
        project_id: id,
        tracked: tracked.cloned(),
    }
}

/// Groups the folders in `addons_dir`: tracked addons by their recorded
/// folders, the rest by the TOC's CurseForge project id, else one by one.
pub fn list_installed(addons_dir: &Path) -> Result<Vec<InstalledAddon>> {
    let manifest = Manifest::load(addons_dir)?;
    let folders = scan_folders(addons_dir)?;
    let mut claimed: HashSet<&str> = HashSet::new();
    let mut out = Vec::new();

    for tracked in &manifest.addons {
        let group: Vec<&Folder> = folders
            .iter()
            .filter(|f| tracked.folders.contains(&f.name))
            .collect();
        if group.is_empty() {
            continue; // deleted by hand
        }
        claimed.extend(group.iter().map(|f| f.name.as_str()));
        out.push(describe(&group, Some(tracked), Some(tracked.project_id)));
    }

    let mut by_project: BTreeMap<u32, Vec<&Folder>> = BTreeMap::new();
    for folder in folders
        .iter()
        .filter(|f| !claimed.contains(f.name.as_str()))
    {
        // No TOC: not an addon.
        let Some(toc) = folder.toc.as_ref() else {
            continue;
        };
        match toc.curse_project_id {
            Some(id) => by_project.entry(id).or_default().push(folder),
            None => out.push(describe(&[folder], None, None)),
        }
    }
    for (id, group) in by_project {
        out.push(describe(&group, None, Some(id)));
    }

    out.sort_by_key(|a| a.name.to_lowercase());
    Ok(out)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub fn make_addon(addons: &Path, folder: &str, toc: &str) {
        let dir = addons.join(folder);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(format!("{folder}.toc")), toc).unwrap();
    }

    #[test]
    fn groups_by_manifest_then_project_id_then_alone() {
        let tmp = tempfile::tempdir().unwrap();
        let a = tmp.path();
        make_addon(
            a,
            "Details",
            "## Title: Details!\n## Version: 1.0\n## X-Curse-Project-ID: 61284\n",
        );
        make_addon(
            a,
            "Details_Streamer",
            "## Title: Streamer\n## X-Curse-Project-ID: 61284\n",
        );
        make_addon(a, "Solo", "## Title: |cffff0000Solo|r\n## Version: 2\n");
        make_addon(a, "Mine", "## Title: Managed\n## X-Curse-Project-ID: 7\n");
        make_addon(a, "Mine_Extra", "## Title: Extra\n");
        make_addon(a, "Blizzard_Foo", "## Title: nope\n");
        std::fs::create_dir(a.join("NoToc")).unwrap();
        std::fs::create_dir(a.join(".wowzers-tmp")).unwrap();

        let mut manifest = Manifest::default();
        manifest.upsert(Tracked {
            project_id: 7,
            name: "Managed".into(),
            slug: "managed".into(),
            file_id: 70,
            file_name: "m.zip".into(),
            display_name: String::new(),
            installed_at: 0,
            folders: vec!["Mine".into(), "Mine_Extra".into(), "Gone".into()],
        });
        manifest.save(a).unwrap();

        let list = list_installed(a).unwrap();
        let summary: Vec<_> = list
            .iter()
            .map(|x| {
                (
                    x.name.as_str(),
                    x.folders.clone(),
                    x.project_id,
                    x.tracked.is_some(),
                )
            })
            .collect();
        assert_eq!(
            summary,
            vec![
                (
                    "Details!",
                    vec!["Details".to_string(), "Details_Streamer".into()],
                    Some(61284),
                    false
                ),
                (
                    "Managed",
                    vec!["Mine".to_string(), "Mine_Extra".into()],
                    Some(7),
                    true
                ),
                ("Solo", vec!["Solo".to_string()], None, false),
            ]
        );
        assert_eq!(list[0].version.as_deref(), Some("1.0"));
    }

    #[test]
    fn missing_addons_dir_is_empty() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(list_installed(&tmp.path().join("nope")).unwrap().is_empty());
    }

    #[test]
    fn per_flavor_toc_is_used_when_there_is_no_plain_one() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("Flav");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("Flav_Mainline.toc"), "## Title: Flavored\n").unwrap();
        let list = list_installed(tmp.path()).unwrap();
        assert_eq!(list[0].name, "Flavored");
    }
}
