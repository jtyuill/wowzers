//! Keyless CurseForge client.
//!
//! The public site is a Next.js app. Its pages are React Server Components
//! payloads: a page requested with `RSC: 1` and `?_rsc` returns compact
//! JSON rows instead of HTML. Those rows carry the same project and search
//! data the HTML is rendered from, so no API key is needed. Downloads come
//! straight from the CDN.
//!
//! Three quirks are load-bearing:
//! - The CDN path is `<id / 1000>/<id % 1000>/<file name>`.
//! - Page requests need a `RSC` header: Cloudflare challenges unmarked ones
//!   and passes RSC-marked ones. `RSC: 0` asks for a navigation (used for the
//!   id-to-slug redirect); `RSC: 1` asks for the RSC payload.
//! - The user agent must not claim to be a browser; Cloudflare challenges
//!   `RSC` requests that carry a browser user agent.

use std::{
    collections::HashMap,
    io::{Read, Write},
    path::Path,
    sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use serde::Deserialize;
use serde_json::Value;

use crate::{
    error::{Error, Result},
    flavor::GameTarget,
};

pub const DEFAULT_BASE_URL: &str = "https://www.curseforge.com";
/// Downloads redirect here from `edge.forgecdn.net`.
pub const DEFAULT_CDN_URL: &str = "https://edge.forgecdn.net";

/// Cloudflare passes RSC-marked requests from non-browser clients but
/// challenges requests that claim to be a browser, so identify honestly.
const USER_AGENT: &str = concat!("wowzers/", env!("CARGO_PKG_VERSION"));
/// Refuse absurd downloads; the largest addons are a few hundred MB.
const MAX_DOWNLOAD_BYTES: u64 = 2 * 1024 * 1024 * 1024;
/// Project pages are small; six at a time keeps an update check quick.
const FETCH_WORKERS: usize = 6;

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

/// `downloadsCount` is a JSON number that may be fractional.
fn count<'de, D: serde::Deserializer<'de>>(d: D) -> std::result::Result<u64, D::Error> {
    Ok(f64::deserialize(d)?.max(0.0) as u64)
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct Author {
    #[serde(default)]
    pub name: String,
}

/// One downloadable file. The site's payloads carry no hashes.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct File {
    pub id: u32,
    #[serde(default)]
    pub display_name: String,
    pub file_name: String,
    #[serde(default, deserialize_with = "release_type")]
    pub release_type: ReleaseType,
    #[serde(default)]
    pub game_versions: Vec<String>,
}

/// A project, with the newest file of every client flavor it supports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mod {
    pub id: u32,
    pub name: String,
    pub slug: String,
    pub summary: String,
    pub download_count: u64,
    pub authors: Vec<Author>,
    pub latest_files: Vec<File>,
}

#[derive(Debug, Clone)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub total: u32,
}

impl Mod {
    /// The newest file that suits `target` and the allowed release channel.
    pub fn best_file(&self, target: GameTarget, channel: ReleaseType) -> Option<&File> {
        let suitable = || {
            self.latest_files.iter().filter(|f| {
                f.release_type <= channel && f.game_versions.iter().any(|v| target.accepts(v))
            })
        };
        match target {
            GameTarget::Newest => suitable().max_by_key(|f| {
                let major = f
                    .game_versions
                    .iter()
                    .filter_map(|v| game_major(v))
                    .max()
                    .unwrap_or(0);
                (major, f.id)
            }),
            _ => suitable().max_by_key(|f| f.id),
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

fn game_major(version: &str) -> Option<u32> {
    version.split('.').next()?.parse().ok()
}

// ---- page payloads ----------------------------------------------------

/// A project card on the search page.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Tile {
    id: u32,
    name: String,
    slug: String,
    #[serde(default)]
    summary: String,
    #[serde(default)]
    author: Option<Author>,
    #[serde(default, deserialize_with = "count")]
    downloads_count: u64,
}

/// A project page.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Project {
    id: u32,
    name: String,
    slug: String,
    #[serde(default)]
    summary: String,
    #[serde(default)]
    author: Option<Author>,
    #[serde(default, deserialize_with = "count")]
    downloads: u64,
    #[serde(default)]
    latest_files: Vec<LatestFiles>,
}

impl Tile {
    fn into_mod(self) -> Mod {
        Mod {
            id: self.id,
            name: self.name,
            slug: self.slug,
            summary: self.summary,
            download_count: self.downloads_count,
            authors: self.author.into_iter().collect(),
            latest_files: Vec::new(),
        }
    }
}

impl Project {
    fn into_mod(self) -> Mod {
        // The same file is listed under every flavor it supports.
        let mut seen = std::collections::HashSet::new();
        let mut latest_files: Vec<File> = Vec::new();
        for group in self.latest_files {
            for file in group.files {
                if seen.insert(file.id) {
                    latest_files.push(file);
                }
            }
        }
        Mod {
            id: self.id,
            name: self.name,
            slug: self.slug,
            summary: self.summary,
            download_count: self.downloads,
            authors: self.author.into_iter().collect(),
            latest_files,
        }
    }
}

#[derive(Debug, Deserialize)]
struct LatestFiles {
    #[serde(default)]
    files: Vec<File>,
}

/// The RSC payload is a sequence of `<hex id>:<row>` rows. A row is either
/// JSON or a text chunk `T<byte length>,<text>`; text chunks may contain
/// newlines and are not newline-terminated, so rows cannot be split naively.
fn flight_values(payload: &str) -> Vec<Value> {
    let bytes = payload.as_bytes();
    let mut values = Vec::new();
    let mut pos = 0;
    while pos < bytes.len() {
        if bytes[pos] == b'\n' {
            pos += 1;
            continue;
        }
        let id_start = pos;
        while pos < bytes.len() && bytes[pos].is_ascii_hexdigit() {
            pos += 1;
        }
        if pos == id_start || bytes.get(pos) != Some(&b':') {
            pos = next_row(bytes, pos + 1);
            continue;
        }
        pos += 1;
        if bytes.get(pos) == Some(&b'T') {
            // T<hex length>,<text>: the length counts UTF-8 bytes.
            let mut len = 0usize;
            pos += 1;
            while let Some(&b) = bytes.get(pos) {
                match b.to_ascii_lowercase() {
                    b'0'..=b'9' => len = len * 16 + usize::from(b - b'0'),
                    b'a'..=b'f' => len = len * 16 + usize::from(b - b'a') + 10,
                    b',' => {
                        pos += 1;
                        break;
                    }
                    _ => break,
                }
                pos += 1;
            }
            pos = (pos + len).min(bytes.len());
            continue;
        }
        let mut stream = serde_json::Deserializer::from_str(&payload[pos..]).into_iter::<Value>();
        match stream.next() {
            Some(Ok(value)) => {
                pos += stream.byte_offset();
                values.push(value);
            }
            _ => pos = next_row(bytes, pos),
        }
    }
    values
}

fn next_row(bytes: &[u8], from: usize) -> usize {
    match bytes[from.min(bytes.len())..]
        .iter()
        .position(|&b| b == b'\n')
    {
        Some(offset) => from + offset + 1,
        None => bytes.len(),
    }
}

fn find_key<'a>(value: &'a Value, key: &str) -> Option<&'a Value> {
    match value {
        Value::Object(map) => map
            .get(key)
            .or_else(|| map.values().find_map(|v| find_key(v, key))),
        Value::Array(items) => items.iter().find_map(|v| find_key(v, key)),
        _ => None,
    }
}

/// The project record is the object carrying both its identity and its
/// per-flavor latest files.
fn find_project(value: &Value) -> Option<&Value> {
    match value {
        Value::Object(map) => {
            if map.contains_key("latestFiles")
                && map.contains_key("gameId")
                && map.contains_key("slug")
            {
                return Some(value);
            }
            map.values().find_map(find_project)
        }
        Value::Array(items) => items.iter().find_map(find_project),
        _ => None,
    }
}

/// The search page spells the result count out in visible text.
fn result_count(payload: &str) -> u32 {
    for needle in ["Projects found", "Project found"] {
        let Some(at) = payload.find(needle) else {
            continue;
        };
        let bytes = payload.as_bytes();
        let mut end = at;
        while end > 0 && bytes[end - 1].is_ascii_whitespace() {
            end -= 1;
        }
        let mut start = end;
        while start > 0 && bytes[start - 1].is_ascii_digit() {
            start -= 1;
        }
        if start < end
            && let Ok(count) = payload[start..end].parse()
        {
            return count;
        }
    }
    0
}

// ---- client -----------------------------------------------------------

pub struct Client {
    agent: ureq::Agent,
    /// Does not follow redirects, for the id-to-slug lookup.
    probe: ureq::Agent,
    base_url: String,
    cdn_url: String,
    /// Project id -> slug, learned from redirects and the installed manifest.
    slugs: Mutex<HashMap<u32, String>>,
}

impl Default for Client {
    fn default() -> Self {
        Self::new()
    }
}

impl Client {
    pub fn new() -> Client {
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(120)))
            .timeout_connect(Some(Duration::from_secs(15)))
            .http_status_as_error(false)
            .user_agent(USER_AGENT)
            .build()
            .into();
        let probe = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(60)))
            .timeout_connect(Some(Duration::from_secs(15)))
            .http_status_as_error(false)
            .max_redirects(0)
            .user_agent(USER_AGENT)
            .build()
            .into();
        Client {
            agent,
            probe,
            base_url: DEFAULT_BASE_URL.to_string(),
            cdn_url: DEFAULT_CDN_URL.to_string(),
            slugs: Mutex::new(HashMap::new()),
        }
    }

    /// Points the client at another server (tests). Downloads follow it too.
    pub fn with_base_url(mut self, url: &str) -> Client {
        self.base_url = url.trim_end_matches('/').to_string();
        self.cdn_url = self.base_url.clone();
        self
    }

    pub fn with_cdn_url(mut self, url: &str) -> Client {
        self.cdn_url = url.trim_end_matches('/').to_string();
        self
    }

    fn get_page(&self, url: &str, rsc: bool) -> Result<Option<String>> {
        let mut request = self
            .agent
            .get(url)
            .header("accept", if rsc { "text/x-component" } else { "text/html" });
        request = if rsc {
            request.header("rsc", "1")
        } else {
            request.header("rsc", "0")
        };
        let response = request.call().map_err(|e| Error::Http(e.to_string()))?;
        let status = response.status().as_u16();
        match status {
            200..=299 => {
                let mut body = response.into_body();
                let text = body
                    .read_to_string()
                    .map_err(|e| Error::Http(e.to_string()))?;
                Ok(Some(text))
            }
            404 => Ok(None),
            _ => Err(Error::Http(format!("HTTP {status} for {url}"))),
        }
    }

    fn get_rsc(&self, url: &str) -> Result<Option<String>> {
        self.get_page(url, true)
    }

    /// Searches WoW addons, most popular first. `index` is a 0-based item
    /// offset; the site pages in whole pages.
    pub fn search(&self, query: &str, index: u32, page_size: u32) -> Result<Page<Mod>> {
        let page_size = page_size.clamp(1, 50);
        let page = index / page_size + 1;
        let mut request = self
            .agent
            .get(format!("{}/wow/search", self.base_url))
            .header("accept", "text/x-component")
            .header("rsc", "1")
            .query("class", "addons")
            .query("page", page.to_string())
            .query("pageSize", page_size.to_string())
            .query("sortBy", "popularity")
            .query("_rsc", "1");
        if !query.trim().is_empty() {
            request = request.query("search", query.trim());
        }
        let response = request.call().map_err(|e| Error::Http(e.to_string()))?;
        let status = response.status().as_u16();
        if !(200..300).contains(&status) {
            return Err(Error::Http(format!("searching CurseForge: HTTP {status}")));
        }
        let mut body = response.into_body();
        let payload = body
            .read_to_string()
            .map_err(|e| Error::Http(e.to_string()))?;
        let tiles: Vec<Tile> = flight_values(&payload)
            .iter()
            .find_map(|value| find_key(value, "tileProjects"))
            .and_then(|value| serde_json::from_value(value.clone()).ok())
            .unwrap_or_default();
        let total = result_count(&payload).max(tiles.len() as u32);
        Ok(Page {
            items: tiles.into_iter().map(Tile::into_mod).collect(),
            total,
        })
    }

    /// The project with `id`, if CurseForge still has it. `slug_hint` avoids
    /// the redirect lookup for addons whose manifest remembers the slug.
    pub fn get_mod(&self, id: u32, slug_hint: Option<&str>) -> Result<Option<Mod>> {
        if let Some(slug) = slug_hint.filter(|s| !s.is_empty())
            && let Some(mod_) = self.project(id, slug)?
        {
            return Ok(Some(mod_));
        }
        let Some(slug) = self.slug_for(id)? else {
            return Ok(None);
        };
        self.project(id, &slug)
    }

    /// Projects by id. Unknown ids are absent from the result.
    pub fn get_mods(&self, ids: &[u32], slugs: &HashMap<u32, String>) -> Result<Vec<Mod>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let next = AtomicUsize::new(0);
        let results: Vec<Mutex<Option<Result<Option<Mod>>>>> =
            ids.iter().map(|_| Mutex::new(None)).collect();
        std::thread::scope(|scope| {
            for _ in 0..FETCH_WORKERS.min(ids.len()) {
                scope.spawn(|| {
                    loop {
                        let i = next.fetch_add(1, Ordering::SeqCst);
                        let Some(&id) = ids.get(i) else { break };
                        let result = self.get_mod(id, slugs.get(&id).map(String::as_str));
                        *results[i].lock().expect("project slot") = Some(result);
                    }
                });
            }
        });

        let mut mods = Vec::with_capacity(ids.len());
        for slot in results {
            match slot.into_inner().expect("project slot") {
                Some(Ok(Some(mod_))) => mods.push(mod_),
                Some(Ok(None)) => {}
                Some(Err(e)) => return Err(e),
                None => return Err(Error::Http("project lookup was skipped".into())),
            }
        }
        Ok(mods)
    }

    fn project(&self, id: u32, slug: &str) -> Result<Option<Mod>> {
        let url = format!(
            "{}/wow/addons/{}?_rsc=1",
            self.base_url,
            encode_segment(slug)
        );
        let Some(payload) = self.get_rsc(&url)? else {
            return Ok(None);
        };
        let Some(value) = flight_values(&payload)
            .iter()
            .find_map(|value| find_project(value))
            .cloned()
        else {
            return Ok(None);
        };
        let project: Project = serde_json::from_value(value)
            .map_err(|e| Error::Decode(format!("project {id}: {e}")))?;
        // A stale slug can redirect to another project; trust only the id.
        if project.id != id {
            return Ok(None);
        }
        self.slugs
            .lock()
            .expect("slug cache")
            .insert(id, project.slug.clone());
        Ok(Some(project.into_mod()))
    }

    /// `/projects/<id>` redirects navigations to the project's slug.
    fn slug_for(&self, id: u32) -> Result<Option<String>> {
        if let Some(slug) = self.slugs.lock().expect("slug cache").get(&id).cloned() {
            return Ok(Some(slug));
        }
        let url = format!("{}/projects/{id}", self.base_url);
        let response = self
            .probe
            .get(&url)
            .header("accept", "text/html")
            .header("rsc", "0")
            .call()
            .map_err(|e| Error::Http(e.to_string()))?;
        let status = response.status().as_u16();
        let slug = match status {
            300..=399 => response
                .headers()
                .get("location")
                .and_then(|value| value.to_str().ok())
                .and_then(slug_from_location),
            404 => return Ok(None),
            _ => {
                return Err(Error::Http(format!(
                    "resolving project {id}: HTTP {status}"
                )));
            }
        };
        if let Some(slug) = &slug {
            self.slugs
                .lock()
                .expect("slug cache")
                .insert(id, slug.clone());
        }
        Ok(slug)
    }

    /// Where to fetch `file` from. The site's download page is behind a bot
    /// challenge; the CDN path is plain and canonical.
    pub fn download_url(&self, file: &File) -> String {
        format!(
            "{}/files/{}/{}/{}",
            self.cdn_url,
            file.id / 1000,
            file.id % 1000,
            encode_segment(&file.file_name)
        )
    }

    /// Downloads `url` to `dest`.
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
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            let n = reader
                .read(&mut buf)
                .map_err(|e| Error::Http(format!("downloading {}: {e}", file.file_name)))?;
            if n == 0 {
                break;
            }
            out.write_all(&buf[..n])
                .map_err(|e| Error::io(format!("writing {}", dest.display()), e))?;
        }
        out.flush()
            .map_err(|e| Error::io(format!("writing {}", dest.display()), e))
    }
}

fn slug_from_location(location: &str) -> Option<String> {
    let path = location.split(['?', '#']).next()?;
    let slug = path.trim_end_matches('/').rsplit('/').next()?;
    (!slug.is_empty() && slug != "addons").then(|| slug.to_string())
}

fn encode_segment(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use ReleaseType::Release;

    fn file(id: u32, name: &str, versions: &[&str], release: ReleaseType) -> File {
        File {
            id,
            display_name: name.into(),
            file_name: format!("{name}.zip"),
            release_type: release,
            game_versions: versions.iter().map(|v| v.to_string()).collect(),
        }
    }

    fn mod_with(files: Vec<File>) -> Mod {
        Mod {
            id: 1,
            name: "m".into(),
            slug: "m".into(),
            summary: String::new(),
            download_count: 0,
            authors: vec![],
            latest_files: files,
        }
    }

    #[test]
    fn best_file_filters_by_major_and_channel() {
        use ReleaseType::*;
        let m = mod_with(vec![
            file(300, "retail", &["11.0.2"], Release),
            file(400, "retail-beta", &["11.0.5"], Beta),
            file(350, "era", &["1.15.4"], Release),
            file(500, "mop", &["4.4.1"], Release),
        ]);
        let pick = |t, c| m.best_file(t, c).map(|i| i.id);
        assert_eq!(pick(GameTarget::Major(11), Release), Some(300));
        assert_eq!(pick(GameTarget::Major(11), Beta), Some(400));
        assert_eq!(pick(GameTarget::Major(1), Release), Some(350));
        assert_eq!(pick(GameTarget::Major(5), Release), None);
        assert_eq!(pick(GameTarget::Any, Release), Some(500));
        assert_eq!(pick(GameTarget::Newest, Release), Some(300));
    }

    #[test]
    fn forever_and_classic_era_files_are_not_mixed_up() {
        use ReleaseType::*;
        let m = mod_with(vec![
            file(350, "era", &["1.15.4"], Release),
            file(600, "forever", &["1.60.1"], Release),
            file(900, "era-new", &["1.15.5"], Release),
        ]);
        let pick = |t| m.best_file(t, Release).map(|i| i.id);
        assert_eq!(pick(GameTarget::Forever), Some(600));
        assert_eq!(pick(GameTarget::ClassicEra), Some(900));
    }

    #[test]
    fn one_file_can_suit_several_flavors() {
        let m = mod_with(vec![file(700, "multi", &["11.0.2", "1.15.4"], Release)]);
        assert_eq!(
            m.best_file(GameTarget::ClassicEra, Release).map(|f| f.id),
            Some(700)
        );
        assert_eq!(
            m.best_file(GameTarget::Major(11), Release).map(|f| f.id),
            Some(700)
        );
    }

    #[test]
    fn newest_prefers_highest_major_over_file_id() {
        use ReleaseType::*;
        let m = mod_with(vec![
            file(300, "retail", &["11.0.2"], Release),
            file(900, "era", &["1.15.4"], Release),
        ]);
        assert_eq!(
            m.best_file(GameTarget::Newest, Release).map(|f| f.id),
            Some(300)
        );
    }

    #[test]
    fn flight_rows_survive_text_chunks_with_newlines_and_multibyte_text() {
        let text = "héllo\nwörld\n";
        let payload = format!(
            "1:\"$Sreact.fragment\"\n3f:T{:x},{text}26:[[\"$\",\"div\",null,{{\"project\":{{\"id\":7}}}}]]\n",
            text.len()
        );
        let values = flight_values(&payload);
        assert_eq!(values.len(), 2, "{values:?}");
        assert!(find_key(&values[1], "project").is_some());
    }

    #[test]
    fn project_records_are_found_and_flattened() {
        let payload = concat!(
            "26:[[\"$\",\"div\",null,{\"project\":{",
            "\"id\":7,\"gameId\":1,\"name\":\"Seven\",\"slug\":\"seven\",",
            "\"downloads\":12,\"author\":{\"name\":\"me\"},",
            "\"latestFiles\":[",
            "{\"gameVersion\":{\"id\":517,\"name\":\"Retail\"},\"files\":[",
            "{\"id\":70,\"fileName\":\"Seven.zip\",\"displayName\":\"1.0\",",
            "\"releaseType\":1,\"gameVersions\":[\"11.0.2\"]}]},",
            "{\"gameVersion\":{\"id\":67408,\"name\":\"Classic\"},\"files\":[",
            "{\"id\":70,\"fileName\":\"Seven.zip\",\"displayName\":\"1.0\",",
            "\"releaseType\":1,\"gameVersions\":[\"11.0.2\"]}]}",
            "]}}]]\n"
        );
        let value = flight_values(payload).into_iter().next().unwrap();
        let project: Project =
            serde_json::from_value(find_project(&value).unwrap().clone()).unwrap();
        let mod_ = project.into_mod();
        assert_eq!(mod_.name, "Seven");
        assert_eq!(mod_.author_names(), "me");
        assert_eq!(mod_.latest_files.len(), 1, "duplicate file is collapsed");
    }

    #[test]
    fn result_count_is_read_from_visible_text() {
        assert_eq!(result_count("1:\"125 Projects found for \"\n"), 125);
        assert_eq!(result_count("1:\"1 Project found\"\n"), 1);
        assert_eq!(result_count("no count here"), 0);
    }

    #[test]
    fn slugs_come_from_redirect_locations() {
        assert_eq!(
            slug_from_location("/wow/addons/deadly-boss-mods").as_deref(),
            Some("deadly-boss-mods")
        );
        assert_eq!(
            slug_from_location("https://www.curseforge.com/wow/addons/weakauras-2?x=1").as_deref(),
            Some("weakauras-2")
        );
        assert_eq!(slug_from_location("/wow/addons/"), None);
    }

    #[test]
    fn cdn_paths_encode_the_file_name() {
        let f = file(5_123_456, "Addon 1.0", &[], ReleaseType::Release);
        let client = Client::new().with_cdn_url("https://cdn.test");
        let url = client.download_url(&f);
        assert_eq!(url, "https://cdn.test/files/5123/456/Addon%201.0.zip");
    }
}
