//! An in-process fake of the CurseForge site plus fixtures, for tests in this
//! crate and in the UI (enable the `testing` feature).

use std::{
    collections::HashMap,
    io::{BufRead, BufReader, Read, Write},
    net::{TcpListener, TcpStream},
    path::Path,
    sync::Arc,
};

use serde_json::{Value, json};

use crate::detect::{FlavorInstall, inspect_root};

pub fn zip_bytes(entries: &[(&str, &str)]) -> Vec<u8> {
    use zip::write::SimpleFileOptions;
    let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for (name, body) in entries {
        w.start_file(*name, SimpleFileOptions::default()).unwrap();
        w.write_all(body.as_bytes()).unwrap();
    }
    w.finish().unwrap().into_inner()
}

/// A file the fake project offers.
#[derive(Debug, Clone)]
pub struct FakeFile {
    pub id: u32,
    pub display: String,
    pub release_type: u8,
    pub game_versions: Vec<String>,
    /// Served from the CDN path when present.
    pub blob: Option<Vec<u8>>,
}

impl FakeFile {
    pub fn new(id: u32, display: &str, game_versions: &[&str]) -> FakeFile {
        FakeFile {
            id,
            display: display.into(),
            release_type: 1,
            game_versions: game_versions.iter().map(|v| v.to_string()).collect(),
            blob: None,
        }
    }

    pub fn beta(mut self) -> FakeFile {
        self.release_type = 2;
        self
    }

    pub fn with_blob(mut self, blob: Vec<u8>) -> FakeFile {
        self.blob = Some(blob);
        self
    }

    pub fn file_name(&self) -> String {
        format!("{}.zip", self.display)
    }

    /// `<id / 1000>/<id % 1000>/<file name>`, under `/files`.
    pub fn cdn_path(&self) -> String {
        format!(
            "/files/{}/{}/{}",
            self.id / 1000,
            self.id % 1000,
            self.file_name()
        )
    }
}

#[derive(Debug, Clone)]
pub struct FakeProject {
    pub id: u32,
    pub name: String,
    pub slug: String,
    pub summary: String,
    pub files: Vec<FakeFile>,
}

impl FakeProject {
    pub fn new(id: u32, name: &str, files: Vec<FakeFile>) -> FakeProject {
        FakeProject {
            id,
            name: name.into(),
            slug: name.to_lowercase().replace(' ', "-"),
            summary: "s".into(),
            files,
        }
    }

    pub fn at_slug(mut self, slug: &str) -> FakeProject {
        self.slug = slug.into();
        self
    }
}

pub struct Server {
    pub base: String,
    /// Requests made, to assert on traffic.
    pub log: Arc<std::sync::Mutex<Vec<String>>>,
}

/// Starts a server on a free port. Each project is reachable by slug, by id
/// redirect, and by its files' CDN paths.
pub fn serve(projects: Vec<FakeProject>) -> Server {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let log = Arc::new(std::sync::Mutex::new(Vec::new()));
    let log2 = log.clone();
    let base2 = base.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let (projects, log, base) = (projects.clone(), log2.clone(), base2.clone());
            std::thread::spawn(move || handle(stream, &projects, &log, &base));
        }
    });
    Server { base, log }
}

fn handle(
    mut stream: TcpStream,
    projects: &[FakeProject],
    log: &std::sync::Mutex<Vec<String>>,
    base: &str,
) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut request_line = String::new();
    reader.read_line(&mut request_line).unwrap();
    let mut parts = request_line.split_whitespace();
    let (method, target) = (
        parts.next().unwrap_or("GET").to_string(),
        parts.next().unwrap_or("/").to_string(),
    );
    let mut headers = HashMap::new();
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        if line.trim().is_empty() {
            break;
        }
        if let Some((k, v)) = line.split_once(':') {
            headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
        }
    }
    let len: usize = headers
        .get("content-length")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let mut body = vec![0; len];
    reader.read_exact(&mut body).unwrap();
    let (path, query) = target
        .split_once('?')
        .map(|(p, q)| (p.to_string(), q.to_string()))
        .unwrap_or((target.clone(), String::new()));
    log.lock().unwrap().push(format!("{method} {target}"));

    let param = |name: &str| -> Option<String> {
        query
            .split('&')
            .find_map(|kv| kv.strip_prefix(&format!("{name}=")).map(str::to_string))
    };

    let (status, content_type, payload, location): (u16, &str, Vec<u8>, Option<String>) =
        if path.starts_with("/files/") {
            match projects
                .iter()
                .flat_map(|p| p.files.iter())
                .find(|f| f.blob.is_some() && f.cdn_path() == path)
            {
                Some(file) => (200, "application/zip", file.blob.clone().unwrap(), None),
                None => (404, "text/plain", b"missing".to_vec(), None),
            }
        } else if path == "/wow/search" {
            let needle = param("search").unwrap_or_default().to_lowercase();
            let hits: Vec<&FakeProject> = projects
                .iter()
                .filter(|p| p.name.to_lowercase().contains(&needle))
                .collect();
            let page: usize = param("page").and_then(|p| p.parse().ok()).unwrap_or(1);
            let page_size: usize = param("pageSize")
                .and_then(|p| p.parse().ok())
                .unwrap_or(20)
                .clamp(1, 50);
            let start = (page.saturating_sub(1)) * page_size;
            let mut tiles = Vec::new();
            for project in hits.iter().skip(start).take(page_size) {
                tiles.push(json!({
                    "id": project.id,
                    "name": project.name,
                    "slug": project.slug,
                    "summary": project.summary,
                    "author": {"name": "me"},
                    "downloadsCount": 1234,
                    "thumbnails": {"thumbnailUrl256": ""},
                }));
            }
            let payload = format!(
                "1:\"x\"\n81:{}\n4:\"{} Projects found\"\n",
                json!({"gameSlug": "wow", "tileProjects": tiles}),
                hits.len()
            );
            (200, "text/x-component", payload.into_bytes(), None)
        } else if let Some(slug) = path.strip_prefix("/wow/addons/") {
            match projects.iter().find(|p| p.slug == slug) {
                Some(project) => (
                    200,
                    "text/x-component",
                    flight_project(project).into_bytes(),
                    None,
                ),
                // The real site answers unknown slugs with an error page.
                None => (
                    200,
                    "text/x-component",
                    b"1:\"x\"\n2:\"not found\"\n".to_vec(),
                    None,
                ),
            }
        } else if let Some(id) = path.strip_prefix("/projects/") {
            match id
                .parse::<u32>()
                .ok()
                .and_then(|id| projects.iter().find(|p| p.id == id))
            {
                Some(project) => (
                    308,
                    "text/html",
                    Vec::new(),
                    Some(format!("{base}/wow/addons/{}", project.slug)),
                ),
                None => (404, "text/html", Vec::new(), None),
            }
        } else {
            (404, "text/plain", b"missing".to_vec(), None)
        };

    let mut response = format!(
        "HTTP/1.1 {status} X\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\nconnection: close\r\n",
        payload.len()
    );
    if let Some(location) = location {
        response.push_str(&format!("location: {location}\r\n"));
    }
    response.push_str("\r\n");
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.write_all(&payload);
}

/// A project page payload: one row whose props carry the project record.
fn flight_project(project: &FakeProject) -> String {
    let files: Vec<Value> = project
        .files
        .iter()
        .map(|file| {
            json!({
                "id": file.id,
                "fileName": file.file_name(),
                "displayName": file.display,
                "releaseType": file.release_type,
                "gameVersions": file.game_versions,
            })
        })
        .collect();
    let project = json!({
        "id": project.id,
        "gameId": 1,
        "name": project.name,
        "slug": project.slug,
        "summary": project.summary,
        "downloads": 1234,
        "author": {"name": "me"},
        "thumbnails": {"thumbnailUrl256": ""},
        "latestFiles": [{
            "gameVersion": {"id": 517, "name": "WoW Retail"},
            "files": files,
        }],
    });
    format!(
        "1:\"x\"\n26:[[\"$\",\"div\",null,{}]]\n",
        json!({"project": project})
    )
}

pub fn wow_with_addons(root: &Path, addons: &[(&str, &str)]) -> FlavorInstall {
    std::fs::create_dir_all(root.join("_retail_/Interface/AddOns")).unwrap();
    std::fs::write(
        root.join(".build.info"),
        "Branch!STRING:0|Version!STRING:0|Product!STRING:0\nus|11.0.2.57388|wow\n",
    )
    .unwrap();
    let install = inspect_root(root).unwrap();
    let flavor = install.flavors.into_iter().next().unwrap();
    for (folder, toc) in addons {
        let dir = flavor.addons_dir.join(folder);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(format!("{folder}.toc")), toc).unwrap();
        std::fs::write(dir.join("old.lua"), "old").unwrap();
    }
    flavor
}
