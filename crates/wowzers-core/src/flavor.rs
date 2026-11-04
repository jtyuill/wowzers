use serde::{Deserialize, Serialize};

/// A game client variant. Each lives in its own folder under the WoW root.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Flavor {
    Retail,
    Classic,
    ClassicEra,
    /// WoW Forever. Its game version is `1.60.x`, in the same major as
    /// Classic Era (`1.15.x`), so the major alone can't tell them apart.
    Forever,
    Ptr,
    Xptr,
    Beta,
    ClassicPtr,
    ClassicBeta,
}

/// Forever's minor version is far above Classic Era's (60 vs 15 at launch).
/// Within major 1, minors from here up are Forever. A threshold rather than
/// "60" so Forever patches (1.61, ...) keep matching.
const FOREVER_MIN_MINOR: u32 = 50;

/// Which game versions an addon file must target to suit a client.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GameTarget {
    /// Files tagged `"<major>.x.y"`. Known from the client's `.build.info`.
    Major(u32),
    /// `1.x.y` with a Classic Era minor.
    ClassicEra,
    /// `1.x.y` with a Forever minor.
    Forever,
    /// Client version unknown; retail is always the newest major on offer.
    Newest,
    /// Client version unknown and no safe guess.
    Any,
}

fn version_parts(version: &str) -> Option<(u32, u32)> {
    let mut parts = version.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next().and_then(|m| m.parse().ok()).unwrap_or(0);
    Some((major, minor))
}

impl GameTarget {
    /// The target for a client whose `.build.info` version is `version`.
    pub fn from_client_version(version: &str) -> Option<GameTarget> {
        let (major, minor) = version_parts(version)?;
        Some(match (major, minor) {
            (1, m) if m >= FOREVER_MIN_MINOR => GameTarget::Forever,
            (1, _) => GameTarget::ClassicEra,
            (major, _) => GameTarget::Major(major),
        })
    }

    /// Whether a file tagged with game version `version` suits this target.
    pub fn accepts(self, version: &str) -> bool {
        let Some((major, minor)) = version_parts(version) else {
            // Non-numeric tags ("Retail") say nothing; only `Any` takes them.
            return matches!(self, GameTarget::Newest | GameTarget::Any);
        };
        match self {
            GameTarget::Major(m) => major == m,
            GameTarget::ClassicEra => major == 1 && minor < FOREVER_MIN_MINOR,
            GameTarget::Forever => major == 1 && minor >= FOREVER_MIN_MINOR,
            GameTarget::Newest | GameTarget::Any => true,
        }
    }
}

impl Flavor {
    pub const ALL: [Flavor; 9] = [
        Flavor::Retail,
        Flavor::Classic,
        Flavor::ClassicEra,
        Flavor::Forever,
        Flavor::Ptr,
        Flavor::Xptr,
        Flavor::Beta,
        Flavor::ClassicPtr,
        Flavor::ClassicBeta,
    ];

    /// Folder name under the WoW root.
    pub fn dir_name(self) -> &'static str {
        match self {
            Flavor::Retail => "_retail_",
            Flavor::Classic => "_classic_",
            Flavor::ClassicEra => "_classic_era_",
            Flavor::Forever => "_forever_",
            Flavor::Ptr => "_ptr_",
            Flavor::Xptr => "_xptr_",
            Flavor::Beta => "_beta_",
            Flavor::ClassicPtr => "_classic_ptr_",
            Flavor::ClassicBeta => "_classic_beta_",
        }
    }

    /// Battle.net product code, as found in `.build.info`.
    pub fn product(self) -> &'static str {
        match self {
            Flavor::Retail => "wow",
            Flavor::Classic => "wow_classic",
            Flavor::ClassicEra => "wow_classic_era",
            Flavor::Forever => "wowf",
            Flavor::Ptr => "wowt",
            Flavor::Xptr => "wowxptr",
            Flavor::Beta => "wow_beta",
            Flavor::ClassicPtr => "wow_classic_ptr",
            Flavor::ClassicBeta => "wow_classic_beta",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Flavor::Retail => "Retail",
            Flavor::Classic => "Classic",
            Flavor::ClassicEra => "Classic Era",
            Flavor::Forever => "Forever",
            Flavor::Ptr => "PTR",
            Flavor::Xptr => "XPTR",
            Flavor::Beta => "Beta",
            Flavor::ClassicPtr => "Classic PTR",
            Flavor::ClassicBeta => "Classic Beta",
        }
    }

    pub fn from_dir_name(name: &str) -> Option<Flavor> {
        Flavor::ALL.into_iter().find(|f| f.dir_name() == name)
    }

    pub fn from_product(product: &str) -> Option<Flavor> {
        Flavor::ALL.into_iter().find(|f| f.product() == product)
    }

    /// The target used when `.build.info` doesn't give the client version.
    pub fn fallback_target(self) -> GameTarget {
        match self {
            Flavor::Retail => GameTarget::Newest,
            Flavor::ClassicEra => GameTarget::ClassicEra,
            Flavor::Forever => GameTarget::Forever,
            _ => GameTarget::Any,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_products_round_trip() {
        for f in Flavor::ALL {
            assert_eq!(Flavor::from_dir_name(f.dir_name()), Some(f));
            assert_eq!(Flavor::from_product(f.product()), Some(f));
        }
        assert_eq!(Flavor::from_dir_name("Data"), None);
        assert_eq!(Flavor::from_dir_name("_forever_"), Some(Flavor::Forever));
        assert_eq!(Flavor::from_product("wowf"), Some(Flavor::Forever));
    }

    #[test]
    fn forever_and_classic_era_share_major_one_but_not_files() {
        let era = GameTarget::from_client_version("1.15.4.57000").unwrap();
        let forever = GameTarget::from_client_version("1.60.1.70790").unwrap();
        assert_eq!(
            (era, forever),
            (GameTarget::ClassicEra, GameTarget::Forever)
        );
        assert!(era.accepts("1.15.4") && !era.accepts("1.60.1"));
        assert!(
            forever.accepts("1.60.1") && forever.accepts("1.61.0") && !forever.accepts("1.15.4")
        );
        assert!(!forever.accepts("11.0.2"));
        // Other flavors keep matching on the major alone.
        let retail = GameTarget::from_client_version("11.0.2.57388").unwrap();
        assert!(retail.accepts("11.2.0") && !retail.accepts("1.60.1") && !retail.accepts("4.4.1"));
        // Tags that aren't versions never match a specific client.
        assert!(!forever.accepts("Forever") && GameTarget::Any.accepts("Forever"));
    }
}
