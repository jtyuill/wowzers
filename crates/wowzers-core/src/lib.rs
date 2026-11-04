//! Core of wowzers: find WoW installs, read the addons in them, and keep
//! those addons current through the CurseForge API. No UI here.

pub mod config;
pub mod curseforge;
pub mod detect;
pub mod error;
pub mod flavor;
pub mod install;
pub mod installed;
pub mod manager;
pub mod manifest;
pub mod toc;

#[cfg(feature = "testing")]
pub mod testing;

pub use error::{Error, Result};
