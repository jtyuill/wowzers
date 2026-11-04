//! App settings, kept in the user's config directory.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{
    curseforge::ReleaseType,
    error::{Error, Result},
    flavor::Flavor,
};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub api_key: Option<String>,
    /// WoW roots the user added by hand; detected ones aren't stored.
    #[serde(default)]
    pub wow_roots: Vec<PathBuf>,
    #[serde(default)]
    pub selected_root: Option<PathBuf>,
    #[serde(default)]
    pub selected_flavor: Option<Flavor>,
    /// Also offer beta files (and alpha, with `allow_alpha`).
    #[serde(default)]
    pub allow_beta: bool,
    #[serde(default)]
    pub allow_alpha: bool,
}

impl Config {
    pub fn default_path() -> Option<PathBuf> {
        dirs::config_dir().map(|d| d.join("wowzers").join("config.json"))
    }

    /// Missing file: defaults. Unreadable file: an error, so the user's key
    /// and paths aren't silently replaced.
    pub fn load(path: &Path) -> Result<Config> {
        match std::fs::read_to_string(path) {
            Ok(text) => serde_json::from_str(&text)
                .map_err(|e| Error::Decode(format!("{} is corrupt ({e})", path.display()))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
            Err(e) => Err(Error::io(format!("reading {}", path.display()), e)),
        }
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| Error::io(format!("creating {}", parent.display()), e))?;
        }
        let tmp = path.with_extension("json.tmp");
        let text = serde_json::to_string_pretty(self).expect("config serializes");
        std::fs::write(&tmp, text)
            .map_err(|e| Error::io(format!("writing {}", tmp.display()), e))?;
        // The file holds an API key.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
        }
        std::fs::rename(&tmp, path).map_err(|e| Error::io(format!("writing {}", path.display()), e))
    }

    /// The key from the config, else `CURSEFORGE_API_KEY`.
    pub fn effective_api_key(&self) -> Option<String> {
        self.api_key
            .clone()
            .filter(|k| !k.trim().is_empty())
            .or_else(|| std::env::var("CURSEFORGE_API_KEY").ok())
            .map(|k| k.trim().to_string())
            .filter(|k| !k.is_empty())
    }

    pub fn channel(&self) -> ReleaseType {
        if self.allow_alpha {
            ReleaseType::Alpha
        } else if self.allow_beta {
            ReleaseType::Beta
        } else {
            ReleaseType::Release
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_with_private_permissions() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("sub/config.json");
        assert_eq!(Config::load(&path).unwrap(), Config::default());
        let config = Config {
            api_key: Some("secret".into()),
            wow_roots: vec!["/games/wow".into()],
            selected_flavor: Some(Flavor::ClassicEra),
            allow_beta: true,
            ..Config::default()
        };
        config.save(&path).unwrap();
        assert_eq!(Config::load(&path).unwrap(), config);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn channel_widens_with_flags_and_key_is_trimmed() {
        let mut c = Config::default();
        assert_eq!(c.channel(), ReleaseType::Release);
        c.allow_beta = true;
        assert_eq!(c.channel(), ReleaseType::Beta);
        c.allow_alpha = true;
        assert_eq!(c.channel(), ReleaseType::Alpha);
        c.api_key = Some("  key \n".into());
        assert_eq!(c.effective_api_key().as_deref(), Some("key"));
    }
}
