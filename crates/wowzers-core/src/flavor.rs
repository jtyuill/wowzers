use serde::{Deserialize, Serialize};

/// A game client variant. Each lives in its own folder under the WoW root.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Flavor {
    Retail,
    Classic,
    ClassicEra,
    Ptr,
    Xptr,
    Beta,
    ClassicPtr,
    ClassicBeta,
}

/// Which game versions an addon file must target to suit a client.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GameTarget {
    /// Files tagged `"<major>.x.y"`. Known from the client's `.build.info`.
    Major(u32),
    /// Client version unknown; retail is always the newest major on offer.
    Newest,
    /// Client version unknown and no safe guess.
    Any,
}

impl Flavor {
    pub const ALL: [Flavor; 8] = [
        Flavor::Retail,
        Flavor::Classic,
        Flavor::ClassicEra,
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
            Flavor::ClassicEra => GameTarget::Major(1),
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
    }
}
