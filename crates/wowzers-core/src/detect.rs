//! Finding World of Warcraft installs and the flavors inside them.

use std::path::{Path, PathBuf};

use crate::{
    error::{Error, Result},
    flavor::{Flavor, GameTarget},
};

/// A WoW root folder (the one holding `_retail_`, `Data`, ...).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WowInstall {
    pub root: PathBuf,
    pub flavors: Vec<FlavorInstall>,
}

/// One client flavor inside an install.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlavorInstall {
    pub flavor: Flavor,
    /// `<root>/_retail_`
    pub dir: PathBuf,
    /// `<root>/_retail_/Interface/AddOns`; may not exist yet.
    pub addons_dir: PathBuf,
    /// Client version from `.build.info`, e.g. `11.0.2.57388`.
    pub version: Option<String>,
}

impl FlavorInstall {
    pub fn target(&self) -> GameTarget {
        self.version
            .as_deref()
            .and_then(GameTarget::from_client_version)
            .unwrap_or_else(|| self.flavor.fallback_target())
    }
}

/// Reads the WoW install at `root`. Fails when no flavor folder is present.
pub fn inspect_root(root: &Path) -> Result<WowInstall> {
    let versions = read_build_info(&root.join(".build.info"));
    let mut flavors: Vec<FlavorInstall> = Flavor::ALL
        .into_iter()
        .filter(|f| root.join(f.dir_name()).is_dir())
        .map(|flavor| {
            let dir = root.join(flavor.dir_name());
            FlavorInstall {
                flavor,
                addons_dir: dir.join("Interface").join("AddOns"),
                version: versions
                    .iter()
                    .find(|(product, _)| product == flavor.product())
                    .map(|(_, v)| v.clone()),
                dir,
            }
        })
        .collect();
    if flavors.is_empty() {
        return Err(Error::NotWow(root.to_path_buf()));
    }
    flavors.sort_by_key(|f| f.flavor);
    Ok(WowInstall {
        root: root.to_path_buf(),
        flavors,
    })
}

/// Accepts a path the user picked at any level (the root, a flavor folder,
/// `Interface`, or `AddOns`) and returns the install it belongs to.
pub fn resolve_user_path(path: &Path) -> Result<WowInstall> {
    for candidate in path.ancestors().take(5) {
        // Directly inside a flavor folder, e.g. `<root>/_retail_/Interface`.
        let is_flavor = candidate
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| Flavor::from_dir_name(n).is_some());
        let root = if is_flavor {
            candidate.parent()
        } else {
            Some(candidate)
        };
        if let Some(Ok(install)) = root.map(inspect_root) {
            return Ok(install);
        }
    }
    Err(Error::NotWow(path.to_path_buf()))
}

/// `(product, version)` rows of `.build.info`, a `|`-separated table whose
/// header cells look like `Product!STRING:0`.
pub fn read_build_info(path: &Path) -> Vec<(String, String)> {
    std::fs::read_to_string(path)
        .map(|text| parse_build_info(&text))
        .unwrap_or_default()
}

pub fn parse_build_info(text: &str) -> Vec<(String, String)> {
    let mut lines = text.lines().filter(|l| !l.trim().is_empty());
    let Some(header) = lines.next() else {
        return Vec::new();
    };
    let column = |name: &str| {
        header
            .split('|')
            .position(|cell| cell.split('!').next() == Some(name))
    };
    let (Some(product), Some(version)) = (column("Product"), column("Version")) else {
        return Vec::new();
    };
    lines
        .filter_map(|line| {
            let cells: Vec<&str> = line.split('|').collect();
            Some((
                cells.get(product)?.to_string(),
                cells.get(version)?.to_string(),
            ))
        })
        .filter(|(p, v)| !p.is_empty() && !v.is_empty())
        .collect()
}

/// Every WoW install found in the usual places, deduplicated.
pub fn detect_installs() -> Vec<WowInstall> {
    detect_in(&candidate_roots())
}

pub fn detect_in(candidates: &[PathBuf]) -> Vec<WowInstall> {
    let mut found: Vec<WowInstall> = Vec::new();
    for candidate in candidates {
        let Ok(install) = inspect_root(candidate) else {
            continue;
        };
        let canonical =
            std::fs::canonicalize(&install.root).unwrap_or_else(|_| install.root.clone());
        let duplicate = found.iter().any(|i| {
            std::fs::canonicalize(&i.root).unwrap_or_else(|_| i.root.clone()) == canonical
        });
        if !duplicate {
            found.push(install);
        }
    }
    found
}

/// Folder names Blizzard's installer and the common Wine front ends use.
const WOW_DIR_NAMES: [&str; 2] = ["World of Warcraft", "WoW"];

/// Locations of Windows-style `drive_c` trees under a Wine prefix.
fn wine_prefix_candidates(drive_c: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for base in ["Program Files (x86)", "Program Files", "Games"] {
        for name in WOW_DIR_NAMES {
            out.push(drive_c.join(base).join(name));
        }
    }
    for name in WOW_DIR_NAMES {
        out.push(drive_c.join(name));
    }
    out
}

fn subdirs(dir: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect()
}

pub fn candidate_roots() -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();

    #[cfg(target_os = "macos")]
    {
        out.push("/Applications/World of Warcraft".into());
        if let Some(home) = dirs::home_dir() {
            out.push(home.join("Applications/World of Warcraft"));
        }
    }

    #[cfg(windows)]
    for drive in 'C'..='Z' {
        let drive = format!("{drive}:\\");
        for base in ["Program Files (x86)", "Program Files", "Games", ""] {
            for name in WOW_DIR_NAMES {
                out.push(Path::new(&drive).join(base).join(name));
            }
        }
    }

    #[cfg(target_os = "linux")]
    if let Some(home) = dirs::home_dir() {
        // Wine prefixes: plain Wine, Lutris, Faugus, Heroic, Bottles (native and
        // Flatpak) and Steam's Proton prefixes for Battle.net.
        let mut prefixes: Vec<PathBuf> = vec![home.join(".wine")];
        let prefix_parents = [
            home.join("Games"),
            home.join("Faugus"), // Faugus Launcher
            home.join("Games/Heroic/Prefixes"),
            home.join("Games/Heroic/Prefixes/default"),
            home.join(".local/share/bottles/bottles"),
            home.join(".var/app/com.usebottles.bottles/data/bottles/bottles"),
            home.join(".local/share/Steam/steamapps/compatdata"),
            home.join(".steam/steam/steamapps/compatdata"),
            home.join(".local/share/wineprefixes"),
            home.join(".local/share/lutris/prefixes"),
        ];
        for parent in prefix_parents {
            for prefix in subdirs(&parent) {
                prefixes.push(prefix.clone());
                prefixes.push(prefix.join("pfx")); // Proton
            }
        }
        for prefix in prefixes {
            out.extend(wine_prefix_candidates(&prefix.join("drive_c")));
        }
        for name in WOW_DIR_NAMES {
            out.push(home.join("Games").join(name));
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const BUILD_INFO: &str = "Branch!STRING:0|Active!DEC:1|Build Key!HEX:16|Version!STRING:0|Product!STRING:0\n\
        ## seqn = 1\n\
        us|1|abc|11.0.2.57388|wow\n\
        us|1|def|1.15.4.57000|wow_classic_era\n";

    #[test]
    fn build_info_rows_are_product_and_version() {
        assert_eq!(
            parse_build_info(BUILD_INFO),
            vec![
                ("wow".to_string(), "11.0.2.57388".to_string()),
                ("wow_classic_era".to_string(), "1.15.4.57000".to_string())
            ]
        );
        assert!(parse_build_info("garbage").is_empty());
    }

    #[test]
    fn inspects_flavors_with_versions_and_targets() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join(".build.info"), BUILD_INFO).unwrap();
        for d in ["_retail_", "_classic_era_", "_classic_", "Data"] {
            std::fs::create_dir(tmp.path().join(d)).unwrap();
        }
        let install = inspect_root(tmp.path()).unwrap();
        let flavors: Vec<_> = install.flavors.iter().map(|f| f.flavor).collect();
        assert_eq!(
            flavors,
            [Flavor::Retail, Flavor::Classic, Flavor::ClassicEra]
        );
        assert_eq!(install.flavors[0].target(), GameTarget::Major(11));
        assert_eq!(install.flavors[2].target(), GameTarget::ClassicEra);
        // No build info row for `wow_classic`: unknown, not guessed.
        assert_eq!(install.flavors[1].target(), GameTarget::Any);
        assert_eq!(
            install.flavors[0].addons_dir,
            tmp.path().join("_retail_/Interface/AddOns")
        );
    }

    #[test]
    fn forever_is_detected_from_its_folder_and_product() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join(".build.info"),
            "Branch!STRING:0|Version!STRING:0|Product!STRING:0\nus|1.60.1.70790|wowf\n",
        )
        .unwrap();
        std::fs::create_dir(tmp.path().join("_forever_")).unwrap();
        let install = inspect_root(tmp.path()).unwrap();
        let forever = &install.flavors[0];
        assert_eq!(forever.flavor, Flavor::Forever);
        assert_eq!(forever.version.as_deref(), Some("1.60.1.70790"));
        assert_eq!(forever.target(), GameTarget::Forever);
        assert_eq!(
            forever.addons_dir,
            tmp.path().join("_forever_/Interface/AddOns")
        );
        // Without .build.info the folder name alone still picks the target.
        std::fs::remove_file(tmp.path().join(".build.info")).unwrap();
        let install = inspect_root(tmp.path()).unwrap();
        assert_eq!(install.flavors[0].target(), GameTarget::Forever);
    }

    #[test]
    fn retail_without_build_info_targets_newest() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join("_retail_")).unwrap();
        assert_eq!(
            inspect_root(tmp.path()).unwrap().flavors[0].target(),
            GameTarget::Newest
        );
    }

    #[test]
    fn user_path_at_any_level_resolves_to_the_root() {
        let tmp = tempfile::tempdir().unwrap();
        let addons = tmp.path().join("wow/_retail_/Interface/AddOns");
        std::fs::create_dir_all(&addons).unwrap();
        for p in [
            addons.clone(),
            addons.parent().unwrap().to_path_buf(),
            tmp.path().join("wow/_retail_"),
            tmp.path().join("wow"),
        ] {
            assert_eq!(
                resolve_user_path(&p).unwrap().root,
                tmp.path().join("wow"),
                "{p:?}"
            );
        }
        assert!(matches!(
            resolve_user_path(tmp.path()),
            Err(Error::NotWow(_))
        ));
    }

    #[test]
    fn detect_in_dedupes_and_skips_non_installs() {
        let tmp = tempfile::tempdir().unwrap();
        let wow = tmp.path().join("wow");
        std::fs::create_dir_all(wow.join("_retail_")).unwrap();
        let found = detect_in(&[tmp.path().join("nope"), wow.clone(), wow.join("../wow")]);
        assert_eq!(found.len(), 1);
    }
}
