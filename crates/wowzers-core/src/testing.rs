//! An in-process fake of the CurseForge API plus fixtures, for tests in this
//! crate and in the UI (enable the `testing` feature).

use std::{
    collections::HashMap,
    io::{BufRead, BufReader, Read, Write},
    net::{TcpListener, TcpStream},
    path::Path,
    sync::Arc,
};

use serde_json::{Value, json};
use sha1::{Digest, Sha1};

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

pub fn sha1_hex(bytes: &[u8]) -> String {
    Sha1::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

pub struct Server {
    pub base: String,
    /// Paths requested, to assert on batching.
    pub log: Arc<std::sync::Mutex<Vec<String>>>,
}

/// Starts a server on a free port. `blobs`: url path -> bytes. `mods`/`files`: API payloads by id.
pub fn serve(
    mods: Vec<Value>,
    files: Vec<Value>,
    blobs: HashMap<String, Vec<u8>>,
    key: &'static str,
) -> Server {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let base2 = base.clone();
    let log = Arc::new(std::sync::Mutex::new(Vec::new()));
    let log2 = log.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let (mods, files, blobs, log, base) = (
                mods.clone(),
                files.clone(),
                blobs.clone(),
                log2.clone(),
                base2.clone(),
            );
            std::thread::spawn(move || handle(stream, &mods, &files, &blobs, key, &log, &base));
        }
    });
    Server { base, log }
}

fn handle(
    mut stream: TcpStream,
    mods: &[Value],
    files: &[Value],
    blobs: &HashMap<String, Vec<u8>>,
    key: &str,
    log: &std::sync::Mutex<Vec<String>>,
    base: &str,
) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut request_line = String::new();
    reader.read_line(&mut request_line).unwrap();
    let mut parts = request_line.split_whitespace();
    let (method, target) = (
        parts.next().unwrap().to_string(),
        parts.next().unwrap().to_string(),
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
    let path = target.split('?').next().unwrap().to_string();
    log.lock().unwrap().push(format!("{method} {target}"));

    let (status, content_type, payload): (u16, &str, Vec<u8>) = if let Some(blob) = blobs.get(&path)
    {
        (200, "application/zip", blob.clone())
    } else if headers.get("x-api-key").map(String::as_str) != Some(key) {
        (403, "text/plain", b"forbidden".to_vec())
    } else {
        let json_body: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
        let pick = |all: &[Value], ids: &Value| -> Vec<Value> {
            all.iter()
                .filter(|v| ids.as_array().is_some_and(|ids| ids.contains(&v["id"])))
                .cloned()
                .collect()
        };
        let data = match (method.as_str(), path.as_str()) {
            ("GET", "/v1/mods/search") => {
                assert!(target.contains("gameId=1"), "{target}");
                let query = target
                    .split(['?', '&'])
                    .find_map(|kv| kv.strip_prefix("searchFilter="))
                    .unwrap_or("")
                    .to_lowercase();
                let hits: Vec<&Value> = mods
                    .iter()
                    .filter(|m| {
                        m["name"]
                            .as_str()
                            .is_some_and(|n| n.to_lowercase().contains(&query))
                    })
                    .collect();
                json!({ "data": hits, "pagination": { "index": 0, "pageSize": 50, "resultCount": hits.len(), "totalCount": hits.len() } })
            }
            ("POST", "/v1/mods") => json!({ "data": pick(mods, &json_body["modIds"]) }),
            ("POST", "/v1/mods/files") => json!({ "data": pick(files, &json_body["fileIds"]) }),
            _ => {
                let _ = write!(
                    stream,
                    "HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
                );
                return;
            }
        };
        (
            200,
            "application/json",
            data.to_string().replace("{BASE}", base).into_bytes(),
        )
    };
    let _ = write!(
        stream,
        "HTTP/1.1 {status} X\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
        payload.len()
    );
    let _ = stream.write_all(&payload);
}

pub fn mod_json(id: u32, name: &str, indexes: Vec<Value>) -> Value {
    json!({ "id": id, "name": name, "slug": name.to_lowercase(), "summary": "s", "downloadCount": 1234.0,
            "authors": [{"name": "me"}], "latestFilesIndexes": indexes })
}

pub fn index(game_version: &str, file_id: u32, release_type: u8) -> Value {
    json!({ "gameVersion": game_version, "fileId": file_id, "filename": "f.zip", "releaseType": release_type })
}

pub fn file_json(id: u32, mod_id: u32, display: &str, blob_path: &str, sha: &str) -> Value {
    json!({ "id": id, "modId": mod_id, "displayName": display, "fileName": format!("{display}.zip"),
            "releaseType": 1, "downloadUrl": format!("{{BASE}}{blob_path}"),
            "hashes": [{"value": sha, "algo": 1}], "gameVersions": ["11.0.2"] })
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

pub fn blobs(entries: &[(&str, &[u8])]) -> HashMap<String, Vec<u8>> {
    entries
        .iter()
        .map(|(p, b)| (p.to_string(), b.to_vec()))
        .collect()
}
