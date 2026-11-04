//! What wowzers installed, recorded next to the addons it installed.
//!
//! Stored as `<AddOns>/.wowzers.json`: the game ignores files in `AddOns`,
//! and the record stays with the install it describes.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

pub const FILE_NAME: &str = ".wowzers.json";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tracked {
    pub project_id: u32,
    pub name: String,
    #[serde(default)]
    pub slug: String,
    pub file_id: u32,
    pub file_name: String,
    #[serde(default)]
    pub display_name: String,
    /// Seconds since the Unix epoch.
    pub installed_at: u64,
    /// Top-level folders the file put in `AddOns`.
    pub folders: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    #[serde(default)]
    pub addons: Vec<Tracked>,
}

impl Manifest {
    pub fn path(addons_dir: &Path) -> PathBuf {
        addons_dir.join(FILE_NAME)
    }

    /// A missing file is an empty manifest. An unreadable one is an error:
    /// carrying on would overwrite the user's record on the next save.
    pub fn load(addons_dir: &Path) -> Result<Manifest> {
        let path = Self::path(addons_dir);
        match std::fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str(&text).map_err(|e| {
                Error::Decode(format!(
                    "{} is corrupt ({e}); fix or delete it",
                    path.display()
                ))
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Manifest::default()),
            Err(e) => Err(Error::io(format!("reading {}", path.display()), e)),
        }
    }

    pub fn save(&self, addons_dir: &Path) -> Result<()> {
        std::fs::create_dir_all(addons_dir)
            .map_err(|e| Error::io(format!("creating {}", addons_dir.display()), e))?;
        let path = Self::path(addons_dir);
        let tmp = path.with_extension("json.tmp");
        let text = serde_json::to_string_pretty(self).expect("manifest serializes");
        std::fs::write(&tmp, text)
            .map_err(|e| Error::io(format!("writing {}", tmp.display()), e))?;
        std::fs::rename(&tmp, &path)
            .map_err(|e| Error::io(format!("writing {}", path.display()), e))
    }

    pub fn get(&self, project_id: u32) -> Option<&Tracked> {
        self.addons.iter().find(|t| t.project_id == project_id)
    }

    /// Records `tracked`, taking its folders away from any other entry (a
    /// shared library folder belongs to whichever project wrote it last).
    pub fn upsert(&mut self, tracked: Tracked) {
        for other in self
            .addons
            .iter_mut()
            .filter(|t| t.project_id != tracked.project_id)
        {
            other.folders.retain(|f| !tracked.folders.contains(f));
        }
        self.addons
            .retain(|t| !t.folders.is_empty() || t.project_id == tracked.project_id);
        match self
            .addons
            .iter_mut()
            .find(|t| t.project_id == tracked.project_id)
        {
            Some(slot) => *slot = tracked,
            None => self.addons.push(tracked),
        }
    }

    pub fn remove(&mut self, project_id: u32) -> Option<Tracked> {
        let at = self
            .addons
            .iter()
            .position(|t| t.project_id == project_id)?;
        Some(self.addons.remove(at))
    }
}

pub fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    pub fn tracked(id: u32, folders: &[&str]) -> Tracked {
        Tracked {
            project_id: id,
            name: format!("addon{id}"),
            slug: String::new(),
            file_id: id * 10,
            file_name: "x.zip".into(),
            display_name: String::new(),
            installed_at: 1,
            folders: folders.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn round_trips_and_missing_is_empty() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(Manifest::load(tmp.path()).unwrap(), Manifest::default());
        let mut m = Manifest::default();
        m.upsert(tracked(1, &["A"]));
        m.save(tmp.path()).unwrap();
        assert_eq!(Manifest::load(tmp.path()).unwrap(), m);
    }

    #[test]
    fn corrupt_manifest_is_an_error_not_a_reset() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(Manifest::path(tmp.path()), "{nope").unwrap();
        assert!(matches!(Manifest::load(tmp.path()), Err(Error::Decode(_))));
    }

    #[test]
    fn upsert_moves_shared_folders_to_the_newest_owner() {
        let mut m = Manifest::default();
        m.upsert(tracked(1, &["A", "Lib"]));
        m.upsert(tracked(2, &["B", "Lib"]));
        assert_eq!(m.get(1).unwrap().folders, ["A"]);
        assert_eq!(m.get(2).unwrap().folders, ["B", "Lib"]);
        // An entry left with no folders disappears.
        m.upsert(tracked(3, &["A"]));
        assert!(m.get(1).is_none());
        // Updating in place replaces, never duplicates.
        m.upsert(tracked(3, &["A", "C"]));
        assert_eq!(m.addons.iter().filter(|t| t.project_id == 3).count(), 1);
    }
}
