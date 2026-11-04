//! Putting a downloaded addon zip into `AddOns`, and taking addons out.

use std::{
    collections::BTreeSet,
    path::{Component, Path, PathBuf},
};

use crate::error::{Error, Result};

const STAGING: &str = ".wowzers-staging";
const TRASH: &str = ".wowzers-trash";

fn zip_err(zip: &Path, e: impl std::fmt::Display) -> Error {
    Error::Archive(format!("{}: {e}", zip.display()))
}

/// Top-level folder an archive entry belongs to, if it is inside one.
fn top_folder(entry: &Path, is_dir: bool) -> Option<String> {
    let mut parts = entry.components().filter_map(|c| match c {
        Component::Normal(n) => Some(n.to_string_lossy().into_owned()),
        _ => None,
    });
    let first = parts.next()?;
    let inside = is_dir || parts.next().is_some();
    (inside && first != "__MACOSX" && !first.starts_with('.')).then_some(first)
}

/// Installs the addon folders in `zip` into `addons_dir`, replacing folders
/// of the same name, and removes `stale` folders (ones an older version of the
/// same addon shipped that this one doesn't). Returns the folders installed.
///
/// All-or-nothing: if any folder can't be put in place, the previous ones
/// are put back.
pub fn install_zip(addons_dir: &Path, zip: &Path, stale: &[String]) -> Result<Vec<String>> {
    let file =
        std::fs::File::open(zip).map_err(|e| Error::io(format!("opening {}", zip.display()), e))?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| zip_err(zip, e))?;

    let mut folders = BTreeSet::new();
    for i in 0..archive.len() {
        let entry = archive.by_index(i).map_err(|e| zip_err(zip, e))?;
        if let Some(path) = entry.enclosed_name() {
            folders.extend(top_folder(&path, entry.is_dir()));
        }
    }
    if folders.is_empty() {
        return Err(Error::Archive(format!(
            "{} contains no addon folders",
            zip.display()
        )));
    }

    let staging = addons_dir.join(STAGING);
    let trash = addons_dir.join(TRASH);
    for dir in [&staging, &trash] {
        let _ = std::fs::remove_dir_all(dir);
        std::fs::create_dir_all(dir)
            .map_err(|e| Error::io(format!("creating {}", dir.display()), e))?;
    }
    let result = extract(&mut archive, zip, &staging, &folders)
        .and_then(|()| swap_in(addons_dir, &staging, &trash, &folders, stale));
    let _ = std::fs::remove_dir_all(&staging);
    let _ = std::fs::remove_dir_all(&trash);
    result?;
    Ok(folders.into_iter().collect())
}

fn extract(
    archive: &mut zip::ZipArchive<std::fs::File>,
    zip: &Path,
    staging: &Path,
    folders: &BTreeSet<String>,
) -> Result<()> {
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|e| zip_err(zip, e))?;
        // `enclosed_name` rejects `..` and absolute paths (zip-slip).
        let Some(rel) = entry.enclosed_name() else {
            continue;
        };
        let Some(top) = top_folder(&rel, entry.is_dir()) else {
            continue;
        };
        debug_assert!(folders.contains(&top));
        let is_symlink = entry.unix_mode().is_some_and(|m| m & 0o170000 == 0o120000);
        if is_symlink {
            continue;
        }
        let out = staging.join(&rel);
        if entry.is_dir() {
            std::fs::create_dir_all(&out)
                .map_err(|e| Error::io(format!("creating {}", out.display()), e))?;
            continue;
        }
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| Error::io(format!("creating {}", parent.display()), e))?;
        }
        let mut file = std::fs::File::create(&out)
            .map_err(|e| Error::io(format!("creating {}", out.display()), e))?;
        std::io::copy(&mut entry, &mut file)
            .map_err(|e| Error::io(format!("extracting {}", out.display()), e))?;
    }
    // A folder that only had directory entries still needs to exist.
    for folder in folders {
        std::fs::create_dir_all(staging.join(folder))
            .map_err(|e| Error::io(format!("creating {folder}"), e))?;
    }
    Ok(())
}

fn swap_in(
    addons_dir: &Path,
    staging: &Path,
    trash: &Path,
    folders: &BTreeSet<String>,
    stale: &[String],
) -> Result<()> {
    // (folder, whether the old one was moved to trash)
    let mut placed: Vec<(&str, bool)> = Vec::new();
    let mut trashed_stale: Vec<&str> = Vec::new();

    let mut attempt = || -> Result<()> {
        for folder in folders {
            let target = addons_dir.join(folder);
            let had_old = target.exists();
            if had_old {
                std::fs::rename(&target, trash.join(folder))
                    .map_err(|e| Error::io(format!("replacing {}", target.display()), e))?;
            }
            placed.push((folder, had_old));
            std::fs::rename(staging.join(folder), &target)
                .map_err(|e| Error::io(format!("installing {}", target.display()), e))?;
        }
        for folder in stale.iter().filter(|f| !folders.contains(*f)) {
            let target = addons_dir.join(folder);
            if is_safe_folder_name(folder) && target.is_dir() {
                std::fs::rename(&target, trash.join(format!("stale-{folder}")))
                    .map_err(|e| Error::io(format!("removing {}", target.display()), e))?;
                trashed_stale.push(folder);
            }
        }
        Ok(())
    };

    let result = attempt();
    if result.is_err() {
        for (folder, had_old) in placed.into_iter().rev() {
            let target = addons_dir.join(folder);
            let _ = std::fs::remove_dir_all(&target);
            if had_old {
                let _ = std::fs::rename(trash.join(folder), &target);
            }
        }
        for folder in trashed_stale {
            let _ = std::fs::rename(
                trash.join(format!("stale-{folder}")),
                addons_dir.join(folder),
            );
        }
    }
    result
}

fn is_safe_folder_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.contains(['/', '\\'])
        && !name.starts_with('.')
}

/// Deletes addon folders by name. Names that aren't plain folder names are
/// refused, since they may come from an edited manifest.
pub fn remove_folders(addons_dir: &Path, folders: &[String]) -> Result<()> {
    for folder in folders {
        if !is_safe_folder_name(folder) {
            return Err(Error::Archive(format!(
                "refusing to delete folder named {folder:?}"
            )));
        }
        let path: PathBuf = addons_dir.join(folder);
        match std::fs::remove_dir_all(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(Error::io(format!("deleting {}", path.display()), e)),
        }
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use std::io::Write;

    use super::*;

    /// Builds a zip with `(path, contents)` entries; paths ending in `/` are dirs.
    pub fn make_zip(path: &Path, entries: &[(&str, &str)]) {
        let mut w = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
        let opts = zip::write::SimpleFileOptions::default();
        for (name, body) in entries {
            if name.ends_with('/') {
                w.add_directory(*name, opts).unwrap();
            } else {
                w.start_file(*name, opts).unwrap();
                w.write_all(body.as_bytes()).unwrap();
            }
        }
        w.finish().unwrap();
    }

    fn read(p: PathBuf) -> String {
        std::fs::read_to_string(p).unwrap()
    }

    #[test]
    fn installs_folders_replaces_old_and_removes_stale() {
        let tmp = tempfile::tempdir().unwrap();
        let addons = tmp.path().join("AddOns");
        std::fs::create_dir_all(addons.join("Foo")).unwrap();
        std::fs::write(addons.join("Foo/old.lua"), "old").unwrap();
        std::fs::create_dir_all(addons.join("Foo_Legacy")).unwrap();
        std::fs::create_dir_all(addons.join("Unrelated")).unwrap();

        let zip = tmp.path().join("foo.zip");
        make_zip(
            &zip,
            &[
                ("Foo/Foo.toc", "## Title: Foo"),
                ("Foo/sub/code.lua", "new"),
                ("Foo_Options/Foo_Options.toc", "## Title: Opts"),
                ("README.txt", "root file is ignored"),
                ("__MACOSX/Foo/._Foo.toc", "junk"),
            ],
        );
        let installed = install_zip(&addons, &zip, &["Foo".into(), "Foo_Legacy".into()]).unwrap();
        assert_eq!(installed, ["Foo", "Foo_Options"]);
        assert_eq!(read(addons.join("Foo/sub/code.lua")), "new");
        assert!(
            !addons.join("Foo/old.lua").exists(),
            "old files of a replaced folder are gone"
        );
        assert!(!addons.join("Foo_Legacy").exists(), "stale folder removed");
        assert!(addons.join("Unrelated").exists());
        assert!(!addons.join("README.txt").exists());
        assert!(!addons.join("__MACOSX").exists());
        // No work dirs left behind.
        let names: Vec<_> = std::fs::read_dir(&addons)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert!(names.iter().all(|n| !n.starts_with('.')), "{names:?}");
    }

    #[test]
    fn archive_without_folders_is_rejected_and_changes_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let addons = tmp.path().join("AddOns");
        std::fs::create_dir_all(addons.join("Keep")).unwrap();
        let zip = tmp.path().join("bad.zip");
        make_zip(&zip, &[("loose.lua", "x")]);
        assert!(matches!(
            install_zip(&addons, &zip, &["Keep".into()]),
            Err(Error::Archive(_))
        ));
        assert!(addons.join("Keep").exists());
        std::fs::write(&zip, "not a zip").unwrap();
        assert!(matches!(
            install_zip(&addons, &zip, &[]),
            Err(Error::Archive(_))
        ));
    }

    #[test]
    fn zip_slip_entries_are_not_extracted() {
        let tmp = tempfile::tempdir().unwrap();
        let addons = tmp.path().join("AddOns");
        std::fs::create_dir_all(&addons).unwrap();
        let zip = tmp.path().join("evil.zip");
        make_zip(
            &zip,
            &[
                ("Good/Good.toc", "x"),
                ("Good/../../escaped.txt", "pwned"),
                ("../escaped2.txt", "pwned"),
            ],
        );
        install_zip(&addons, &zip, &[]).unwrap();
        assert!(!tmp.path().join("escaped.txt").exists());
        assert!(!tmp.path().join("escaped2.txt").exists());
        assert!(!addons.join("escaped.txt").exists());
        assert!(addons.join("Good/Good.toc").exists());
    }

    /// A directory without write permission can't be renamed away, which makes
    /// the second folder's swap fail after the first already succeeded.
    #[cfg(unix)]
    #[test]
    fn failed_swap_restores_previous_folders() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::tempdir().unwrap();
        let addons = tmp.path().join("AddOns");
        for f in ["A", "B"] {
            std::fs::create_dir_all(addons.join(f)).unwrap();
            std::fs::write(addons.join(f).join("v"), "1").unwrap();
        }
        let locked = std::fs::Permissions::from_mode(0o555);
        std::fs::set_permissions(addons.join("B"), locked).unwrap();
        let restore =
            || std::fs::set_permissions(addons.join("B"), std::fs::Permissions::from_mode(0o755));
        if std::fs::rename(addons.join("B"), tmp.path().join("probe")).is_ok() {
            // Running as root: permissions don't bite, so the failure can't be forced.
            return;
        }

        let zip = tmp.path().join("z.zip");
        make_zip(&zip, &[("A/v", "2"), ("B/v", "2")]);
        let result = install_zip(&addons, &zip, &[]);
        restore().unwrap();
        assert!(result.is_err());
        assert_eq!(read(addons.join("A/v")), "1", "A rolled back");
        assert_eq!(read(addons.join("B/v")), "1", "B untouched");
    }

    #[test]
    fn remove_refuses_path_tricks() {
        let tmp = tempfile::tempdir().unwrap();
        let addons = tmp.path().join("AddOns");
        std::fs::create_dir_all(addons.join("A")).unwrap();
        std::fs::create_dir_all(tmp.path().join("victim")).unwrap();
        assert!(remove_folders(&addons, &["../victim".into()]).is_err());
        assert!(tmp.path().join("victim").exists());
        remove_folders(&addons, &["A".into(), "AlreadyGone".into()]).unwrap();
        assert!(!addons.join("A").exists());
    }
}
