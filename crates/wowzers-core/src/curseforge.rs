//! Thin blocking client for the parts of the CurseForge REST API that an
//! addon manager needs, plus the response models.

use std::{
    io::{Read, Write},
    path::Path,
    time::Duration,
};

use serde::{Deserialize, de::DeserializeOwned};
use sha1::{Digest, Sha1};

use crate::{
    error::{Error, Result},
    flavor::GameTarget,
};

pub const DEFAULT_BASE_URL: &str = "https://api.curseforge.com";
/// World of Warcraft on CurseForge.
pub const GAME_ID: u32 = 1;

/// The API's cap for list-by-id requests we send per call.
const BATCH: usize = 100;
/// Refuse absurd downloads; the largest addons are a few hundred MB.
const MAX_DOWNLOAD_BYTES: u64 = 2 * 1024 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum ReleaseType {
    #[default]
    Release,
    Beta,
    Alpha,
}

impl ReleaseType {
    fn from_code(code: u8) -> ReleaseType {
        match code {
            2 => ReleaseType::Beta,
            3 => ReleaseType::Alpha,
            _ => ReleaseType::Release,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ReleaseType::Release => "release",
            ReleaseType::Beta => "beta",
            ReleaseType::Alpha => "alpha",
        }
    }
}

fn release_type<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> std::result::Result<ReleaseType, D::Error> {
    Ok(ReleaseType::from_code(u8::deserialize(d)?))
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Mod {
    pub id: u32,
    pub name: String,
    #[serde(default)]
    pub slug: String,
    #[serde(default)]
    pub summary: String,
    #[serde(default, deserialize_with = "count")]
    pub download_count: u64,
    #[serde(default)]
    pub links: Links,
    #[serde(default)]
    pub authors: Vec<Author>,
    #[serde(default)]
    pub logo: Option<Logo>,
    #[serde(default)]
    pub latest_files_indexes: Vec<FileIndex>,
    /// `None` when the API doesn't say; only `Some(false)` forbids download.
    #[serde(default)]
    pub allow_mod_distribution: Option<bool>,
    #[serde(default)]
    pub date_modified: String,
}

/// `downloadCount` is a JSON number that may be fractional.
fn count<'de, D: serde::Deserializer<'de>>(d: D) -> std::result::Result<u64, D::Error> {
    Ok(f64::deserialize(d)?.max(0.0) as u64)
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Links {
    #[serde(default)]
    pub website_url: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct Author {
    pub name: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Logo {
    #[serde(default)]
    pub thumbnail_url: String,
}

/// "The latest file for this game version and release type."
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FileIndex {
    pub game_version: String,
    pub file_id: u32,
    #[serde(default)]
    pub filename: String,
    #[serde(default, deserialize_with = "release_type")]
    pub release_type: ReleaseType,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct File {
    pub id: u32,
    pub mod_id: u32,
    #[serde(default)]
    pub display_name: String,
    pub file_name: String,
    #[serde(default, deserialize_with = "release_type")]
    pub release_type: ReleaseType,
    #[serde(default)]
    pub file_date: String,
    #[serde(default)]
    pub file_length: u64,
    #[serde(default)]
    pub download_url: Option<String>,
    #[serde(default)]
    pub hashes: Vec<Hash>,
    #[serde(default)]
    pub game_versions: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct Hash {
    pub value: String,
    /// 1 = SHA-1, 2 = MD5.
    pub algo: u8,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Pagination {
    pub index: u32,
    pub result_count: u32,
    pub total_count: u32,
}

#[derive(Debug, Clone)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub total: u32,
}

impl File {
    fn sha1(&self) -> Option<&str> {
        self.hashes
            .iter()
            .find(|h| h.algo == 1)
            .map(|h| h.value.as_str())
    }
}

fn game_major(version: &str) -> Option<u32> {
    version.split('.').next()?.parse().ok()
}

impl Mod {
    /// The newest file that suits `target` and the allowed release channel.
    pub fn best_file(&self, target: GameTarget, channel: ReleaseType) -> Option<&FileIndex> {
        let suitable = self.latest_files_indexes.iter().filter(|i| {
            i.release_type <= channel
                && match target {
                    GameTarget::Major(m) => game_major(&i.game_version) == Some(m),
                    GameTarget::Newest | GameTarget::Any => true,
                }
        });
        match target {
            GameTarget::Newest => suitable.max_by_key(|i| (game_major(&i.game_version), i.file_id)),
            _ => suitable.max_by_key(|i| i.file_id),
        }
    }

    pub fn author_names(&self) -> String {
        self.authors
            .iter()
            .map(|a| a.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

pub struct Client {
    agent: ureq::Agent,
    base_url: String,
    api_key: String,
}

impl Client {
    pub fn new(api_key: &str) -> Result<Client> {
        let api_key = api_key.trim();
        if api_key.is_empty() {
            return Err(Error::MissingApiKey);
        }
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(120)))
            .timeout_connect(Some(Duration::from_secs(15)))
            .http_status_as_error(false)
            .user_agent(concat!("wowzers/", env!("CARGO_PKG_VERSION")))
            .build()
            .into();
        Ok(Client {
            agent,
            base_url: DEFAULT_BASE_URL.to_string(),
            api_key: api_key.to_string(),
        })
    }

    /// Points the client at another server (tests, or a caching proxy).
    pub fn with_base_url(mut self, url: &str) -> Client {
        self.base_url = url.trim_end_matches('/').to_string();
        self
    }

    fn check(
        &self,
        response: ureq::http::Response<ureq::Body>,
    ) -> Result<ureq::http::Response<ureq::Body>> {
        let status = response.status().as_u16();
        match status {
            200..=299 => Ok(response),
            401 | 403 => Err(Error::BadApiKey(status)),
            _ => {
                let mut body = response.into_body();
                let text = body.read_to_string().unwrap_or_default();
                let snippet: String = text.chars().take(200).collect();
                Err(Error::Http(
                    format!("HTTP {status} {snippet}").trim().to_string(),
                ))
            }
        }
    }

    fn decode<T: DeserializeOwned>(mut response: ureq::http::Response<ureq::Body>) -> Result<T> {
        response
            .body_mut()
            .read_json::<T>()
            .map_err(|e| Error::Decode(e.to_string()))
    }

    fn get<T: DeserializeOwned>(&self, path: &str, query: &[(&str, String)]) -> Result<T> {
        let mut request = self
            .agent
            .get(format!("{}{path}", self.base_url))
            .header("x-api-key", &self.api_key)
            .header("accept", "application/json");
        for (k, v) in query {
            request = request.query(k, v);
        }
        let response = request.call().map_err(|e| Error::Http(e.to_string()))?;
        Self::decode(self.check(response)?)
    }

    fn post<T: DeserializeOwned>(&self, path: &str, body: &serde_json::Value) -> Result<T> {
        let response = self
            .agent
            .post(format!("{}{path}", self.base_url))
            .header("x-api-key", &self.api_key)
            .header("accept", "application/json")
            .send_json(body)
            .map_err(|e| Error::Http(e.to_string()))?;
        Self::decode(self.check(response)?)
    }

    /// Searches WoW projects (all of them addons) by name, most popular first.
    pub fn search(&self, query: &str, index: u32, page_size: u32) -> Result<Page<Mod>> {
        #[derive(Deserialize)]
        struct Response {
            data: Vec<Mod>,
            pagination: Pagination,
        }
        let r: Response = self.get(
            "/v1/mods/search",
            &[
                ("gameId", GAME_ID.to_string()),
                ("searchFilter", query.to_string()),
                ("sortField", "2".to_string()), // popularity
                ("sortOrder", "desc".to_string()),
                ("index", index.to_string()),
                ("pageSize", page_size.min(50).to_string()),
            ],
        )?;
        Ok(Page {
            items: r.data,
            total: r.pagination.total_count,
        })
    }

    /// Mods by id. Unknown ids are absent from the result.
    pub fn get_mods(&self, ids: &[u32]) -> Result<Vec<Mod>> {
        #[derive(Deserialize)]
        struct Response {
            data: Vec<Mod>,
        }
        let mut out = Vec::new();
        for chunk in ids.chunks(BATCH) {
            let r: Response = self.post(
                "/v1/mods",
                &serde_json::json!({ "modIds": chunk, "filterPcOnly": true }),
            )?;
            out.extend(r.data);
        }
        Ok(out)
    }

    /// Files by id.
    pub fn get_files(&self, ids: &[u32]) -> Result<Vec<File>> {
        #[derive(Deserialize)]
        struct Response {
            data: Vec<File>,
        }
        let mut out = Vec::new();
        for chunk in ids.chunks(BATCH) {
            let r: Response =
                self.post("/v1/mods/files", &serde_json::json!({ "fileIds": chunk }))?;
            out.extend(r.data);
        }
        Ok(out)
    }

    /// Where to fetch `file` from. Authors may withhold `downloadUrl`; the
    /// CDN path is then derived, unless they also forbid distribution.
    pub fn download_url(&self, mod_: &Mod, file: &File) -> Result<String> {
        if let Some(url) = file.download_url.as_deref().filter(|u| !u.is_empty()) {
            return Ok(url.to_string());
        }
        if mod_.allow_mod_distribution == Some(false) {
            return Err(Error::DistributionDisabled(mod_.name.clone()));
        }
        Ok(cdn_url(file))
    }

    /// Downloads `url` to `dest`, verifying `file`'s SHA-1 when it has one.
    pub fn download(&self, url: &str, file: &File, dest: &Path) -> Result<()> {
        let response = self
            .agent
            .get(url)
            .call()
            .map_err(|e| Error::Http(e.to_string()))?;
        let status = response.status().as_u16();
        if !(200..300).contains(&status) {
            return Err(Error::Http(format!(
                "downloading {}: HTTP {status}",
                file.file_name
            )));
        }
        let mut body = response.into_body();
        let mut reader = body.with_config().limit(MAX_DOWNLOAD_BYTES).reader();
        let out = std::fs::File::create(dest)
            .map_err(|e| Error::io(format!("creating {}", dest.display()), e))?;
        let mut out = std::io::BufWriter::new(out);
        let mut hasher = Sha1::new();
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            let n = reader
                .read(&mut buf)
                .map_err(|e| Error::Http(format!("downloading {}: {e}", file.file_name)))?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
            out.write_all(&buf[..n])
                .map_err(|e| Error::io(format!("writing {}", dest.display()), e))?;
        }
        out.flush()
            .map_err(|e| Error::io(format!("writing {}", dest.display()), e))?;
        if let Some(expected) = file.sha1() {
            let actual = hex(&hasher.finalize());
            if !actual.eq_ignore_ascii_case(expected) {
                return Err(Error::Checksum {
                    file: file.file_name.clone(),
                    expected: expected.to_string(),
                    actual,
                });
            }
        }
        Ok(())
    }
}

/// `edge.forgecdn.net/files/<id / 1000>/<id % 1000>/<name>`
fn cdn_url(file: &File) -> String {
    format!(
        "https://edge.forgecdn.net/files/{}/{}/{}",
        file.id / 1000,
        file.id % 1000,
        file.file_name
    )
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn index(version: &str, id: u32, release: ReleaseType) -> FileIndex {
        FileIndex {
            game_version: version.into(),
            file_id: id,
            filename: String::new(),
            release_type: release,
        }
    }

    fn mod_with(indexes: Vec<FileIndex>) -> Mod {
        Mod {
            id: 1,
            name: "m".into(),
            slug: String::new(),
            summary: String::new(),
            download_count: 0,
            links: Links::default(),
            authors: vec![],
            logo: None,
            latest_files_indexes: indexes,
            allow_mod_distribution: None,
            date_modified: String::new(),
        }
    }

    #[test]
    fn best_file_filters_by_major_and_channel() {
        use ReleaseType::*;
        let m = mod_with(vec![
            index("11.0.2", 300, Release),
            index("11.0.5", 400, Beta),
            index("1.15.4", 350, Release),
            index("4.4.1", 500, Release),
        ]);
        let pick = |t, c| m.best_file(t, c).map(|i| i.file_id);
        assert_eq!(pick(GameTarget::Major(11), Release), Some(300));
        assert_eq!(pick(GameTarget::Major(11), Beta), Some(400));
        assert_eq!(pick(GameTarget::Major(1), Release), Some(350));
        assert_eq!(pick(GameTarget::Major(5), Release), None);
        assert_eq!(pick(GameTarget::Any, Release), Some(500));
        assert_eq!(pick(GameTarget::Newest, Release), Some(300));
    }

    #[test]
    fn newest_prefers_highest_major_over_file_id() {
        use ReleaseType::*;
        let m = mod_with(vec![
            index("11.0.2", 300, Release),
            index("1.15.4", 900, Release),
        ]);
        assert_eq!(
            m.best_file(GameTarget::Newest, Release).map(|i| i.file_id),
            Some(300)
        );
    }

    #[test]
    fn missing_key_is_rejected_and_cdn_path_is_derived() {
        assert!(matches!(Client::new("  "), Err(Error::MissingApiKey)));
        let file = File {
            id: 5_123_456,
            mod_id: 1,
            display_name: String::new(),
            file_name: "Addon-1.0.zip".into(),
            release_type: ReleaseType::Release,
            file_date: String::new(),
            file_length: 0,
            download_url: None,
            hashes: vec![],
            game_versions: vec![],
        };
        assert_eq!(
            cdn_url(&file),
            "https://edge.forgecdn.net/files/5123/456/Addon-1.0.zip"
        );
        let client = Client::new("k").unwrap();
        let mut m = mod_with(vec![]);
        m.allow_mod_distribution = Some(false);
        assert!(matches!(
            client.download_url(&m, &file),
            Err(Error::DistributionDisabled(_))
        ));
        m.allow_mod_distribution = None;
        assert!(client.download_url(&m, &file).unwrap().contains("forgecdn"));
    }
}
