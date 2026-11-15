//! Uploading a cut source set to a hub.
//!
//! `client-compute.md` section 2.1 (directory routes) and section 5.3: each
//! tile goes up with `PUT /source/{hash}` unless the hub already has it
//! (`HEAD /source/{hash}` returns 200), then the manifest with
//! `PUT /source-manifest` unless `HEAD /source-manifest/{hash}` returns 200.
//! The hub checks every tile's hash and that the manifest only lists stored
//! tiles. Every request carries the `X-API-Key` header, which needs the
//! admin capability.
//!
//! The hub's base URL and the key come from the environment
//! ([`HUB_URL_VAR`], [`API_KEY_VAR`]); nothing that names a hub is in this
//! repository. Every tile is read from disk and checked against the
//! manifest before it is sent. Tiles with the same bytes are sent once.

use std::collections::BTreeSet;
use std::fmt;
use std::fs;
use std::path::Path;
use std::time::Duration;

use gx_core::manifest::{self, Manifest};
use gx_core::sha256::{sha256, to_hex, DIGEST_LEN};
use terrain_tiles::TileKey;

use crate::cut::{tile_path, MANIFEST_FILE};

/// The environment variable holding the hub's base URL.
pub const HUB_URL_VAR: &str = "GX_HUB_URL";
/// The environment variable holding the API key.
pub const API_KEY_VAR: &str = "GX_API_KEY";

/// Why an upload failed.
#[derive(Debug)]
pub struct UploadError(pub String);

impl fmt::Display for UploadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for UploadError {}

/// Where to upload.
#[derive(Clone, Debug)]
pub struct Hub {
    /// Base URL. A trailing slash is ignored.
    pub url: String,
    /// The API key.
    pub api_key: String,
}

impl Hub {
    /// The hub named by [`HUB_URL_VAR`] and [`API_KEY_VAR`].
    pub fn from_env() -> Result<Hub, UploadError> {
        let get = |name: &str| {
            std::env::var(name)
                .ok()
                .filter(|v| !v.is_empty())
                .ok_or_else(|| UploadError(format!("set {name}")))
        };
        Ok(Hub {
            url: get(HUB_URL_VAR)?.trim_end_matches('/').to_string(),
            api_key: get(API_KEY_VAR)?,
        })
    }
}

/// What an upload did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UploadReport {
    /// Tiles in the manifest.
    pub tiles: u64,
    /// Tiles sent.
    pub sent: u64,
    /// Tiles the hub already had, or that had the bytes of a tile already
    /// handled in this run.
    pub skipped: u64,
    /// Bytes sent, tiles and manifest.
    pub bytes_sent: u64,
    /// Whether the manifest was sent.
    pub manifest_sent: bool,
    /// The manifest's SHA-256.
    pub manifest_hash: [u8; DIGEST_LEN],
}

/// Attempts per request before giving up on transport errors and 5xx
/// answers.
const ATTEMPTS: u32 = 4;

struct Client {
    agent: ureq::Agent,
    hub: Hub,
}

impl Client {
    fn new(mut hub: Hub) -> Client {
        hub.url = hub.url.trim_end_matches('/').to_string();
        let agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_global(Some(Duration::from_secs(300)))
            .build()
            .into();
        Client { agent, hub }
    }

    /// Whether `HEAD {path}` returns 200.
    fn has(&self, path: &str) -> Result<bool, UploadError> {
        let url = format!("{}{path}", self.hub.url);
        let status = self.retry(&format!("HEAD {path}"), || {
            self.agent
                .head(&url)
                .header("X-API-Key", &self.hub.api_key)
                .call()
                .map(|r| (r.status().as_u16(), String::new()))
        })?;
        Ok(status.0 == 200)
    }

    /// `PUT {path}` with `body`. Returns the status and the response body;
    /// fails unless the status is 200 or 201.
    fn put(&self, path: &str, body: &[u8]) -> Result<String, UploadError> {
        let url = format!("{}{path}", self.hub.url);
        let what = format!("PUT {path}");
        let (status, text) = self.retry(&what, || {
            let mut r = self
                .agent
                .put(&url)
                .header("X-API-Key", &self.hub.api_key)
                .header("Content-Type", "application/octet-stream")
                .send(body)?;
            let status = r.status().as_u16();
            let text = r.body_mut().read_to_string().unwrap_or_default();
            Ok((status, text))
        })?;
        if status != 200 && status != 201 {
            return Err(UploadError(format!("{what}: {status} {text}")));
        }
        Ok(text)
    }

    fn retry(
        &self,
        what: &str,
        f: impl Fn() -> Result<(u16, String), ureq::Error>,
    ) -> Result<(u16, String), UploadError> {
        let mut last = String::new();
        for attempt in 0..ATTEMPTS {
            if attempt > 0 {
                std::thread::sleep(Duration::from_millis(500 << attempt));
            }
            match f() {
                Ok((s, _)) if s >= 500 => last = format!("{what}: {s}"),
                Ok(r) => return Ok(r),
                Err(e) => last = format!("{what}: {e}"),
            }
        }
        Err(UploadError(last))
    }
}

/// Uploads the source set in `out` (written by [`crate::cut::cut`]) to
/// `hub` (module documentation). With `progress`, prints a line every 1000
/// tiles and at the end.
pub fn upload(out: &Path, hub: Hub, progress: bool) -> Result<UploadReport, UploadError> {
    let mpath = out.join(MANIFEST_FILE);
    let mbytes = fs::read(&mpath).map_err(|e| UploadError(format!("{}: {e}", mpath.display())))?;
    let m: Manifest = manifest::decode(&mbytes)
        .map_err(|e| UploadError(format!("{}: {}", mpath.display(), e.reason)))?;
    let mhash = manifest::manifest_hash(&mbytes);
    let client = Client::new(hub);
    let mut report = UploadReport {
        tiles: m.entries().len() as u64,
        sent: 0,
        skipped: 0,
        bytes_sent: 0,
        manifest_sent: false,
        manifest_hash: mhash,
    };
    let mut done: BTreeSet<[u8; DIGEST_LEN]> = BTreeSet::new();
    for (i, e) in m.entries().iter().enumerate() {
        if done.contains(&e.sha256) {
            report.skipped += 1;
        } else {
            let key = TileKey::from_id(e.id)
                .ok_or_else(|| UploadError(format!("manifest: id {:#x} is not a tile id", e.id)))?;
            let path = tile_path(out, key);
            let bytes =
                fs::read(&path).map_err(|err| UploadError(format!("{}: {err}", path.display())))?;
            if bytes.len() as u64 != e.length || sha256(&bytes) != e.sha256 {
                return Err(UploadError(format!(
                    "{}: does not match the manifest; cut again",
                    path.display()
                )));
            }
            let route = format!("/source/{}", to_hex(&e.sha256));
            if client.has(&route)? {
                report.skipped += 1;
            } else {
                client.put(&route, &bytes)?;
                report.sent += 1;
                report.bytes_sent += bytes.len() as u64;
            }
            done.insert(e.sha256);
        }
        if progress && ((i + 1) % 1000 == 0 || i + 1 == m.entries().len()) {
            println!(
                "upload: {} of {} tiles, {} sent, {} skipped",
                i + 1,
                report.tiles,
                report.sent,
                report.skipped
            );
        }
    }
    let hex = to_hex(&mhash);
    if !client.has(&format!("/source-manifest/{hex}"))? {
        let reply = client.put("/source-manifest", &mbytes)?;
        if !reply.contains(&hex) {
            return Err(UploadError(format!(
                "PUT /source-manifest: the hub answered {reply}, expected hash {hex}"
            )));
        }
        report.manifest_sent = true;
        report.bytes_sent += mbytes.len() as u64;
    }
    if progress {
        println!(
            "upload: manifest {}",
            if report.manifest_sent {
                "sent"
            } else {
                "already stored"
            }
        );
    }
    Ok(report)
}
