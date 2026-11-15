//! Uploading a cut source set to a stand-in hub that implements the routes
//! of `client-compute.md` section 2.1 the uploader uses: `HEAD` and `PUT
//! /source/{hash}`, `HEAD /source-manifest/{hash}`, and `PUT
//! /source-manifest`, with the hub's checks.

use std::collections::BTreeMap;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use gx_core::manifest;
use gx_core::sha256::{sha256, to_hex};
use terrain_tiler::cut::{cut, tile_path, CutOptions, CutReport};
use terrain_tiler::source::MemorySource;
use terrain_tiler::synth;
use terrain_tiler::upload::{upload, Hub};
use terrain_tiles::{Grid, TileKey};

const KEY: &str = "test-admin-key";

#[derive(Default)]
struct State {
    sources: BTreeMap<String, Vec<u8>>,
    manifests: BTreeMap<String, Vec<u8>>,
    puts: usize,
}

type Shared = Arc<Mutex<State>>;

/// Starts the stand-in hub on a free loopback port and returns its URL.
fn start_hub(state: Shared) -> String {
    let listener = TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0))).unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let state = state.clone();
            std::thread::spawn(move || serve(stream, state));
        }
    });
    url
}

fn serve(stream: TcpStream, state: Shared) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut stream = stream;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap_or(0) == 0 {
            return;
        }
        let mut parts = line.split_whitespace();
        let method = parts.next().unwrap_or("").to_string();
        let path = parts.next().unwrap_or("").to_string();
        let (mut len, mut key) = (0usize, String::new());
        loop {
            let mut h = String::new();
            reader.read_line(&mut h).unwrap();
            let h = h.trim_end();
            if h.is_empty() {
                break;
            }
            let (name, value) = h.split_once(':').unwrap();
            match name.to_ascii_lowercase().as_str() {
                "content-length" => len = value.trim().parse().unwrap(),
                "x-api-key" => key = value.trim().to_string(),
                _ => {}
            }
        }
        let mut body = vec![0; len];
        reader.read_exact(&mut body).unwrap();
        let (status, text) = handle(&state, &method, &path, &key, body);
        let reply = format!(
            "HTTP/1.1 {status} X\r\nContent-Length: {}\r\n\r\n{}",
            if method == "HEAD" { 0 } else { text.len() },
            if method == "HEAD" { "" } else { &text }
        );
        stream.write_all(reply.as_bytes()).unwrap();
    }
}

fn handle(state: &Shared, method: &str, path: &str, key: &str, body: Vec<u8>) -> (u16, String) {
    if key != KEY {
        return (401, "no".into());
    }
    let mut s = state.lock().unwrap();
    match (method, path) {
        ("HEAD", p) if p.starts_with("/source/") => {
            let found = s.sources.contains_key(&p["/source/".len()..]);
            (if found { 200 } else { 404 }, String::new())
        }
        ("HEAD", p) if p.starts_with("/source-manifest/") => {
            let found = s.manifests.contains_key(&p["/source-manifest/".len()..]);
            (if found { 200 } else { 404 }, String::new())
        }
        ("PUT", "/source-manifest") => {
            let Ok(m) = manifest::decode(&body) else {
                return (400, "malformed".into());
            };
            if m.entries()
                .iter()
                .any(|e| !s.sources.contains_key(&to_hex(&e.sha256)))
            {
                return (400, "a tile is not stored".into());
            }
            let hash = to_hex(&sha256(&body));
            s.manifests.insert(hash.clone(), body);
            s.puts += 1;
            (201, format!("{{\"hash\":\"{hash}\"}}"))
        }
        ("PUT", p) if p.starts_with("/source/") => {
            let hash = &p["/source/".len()..];
            if to_hex(&sha256(&body)) != hash {
                return (400, "hash mismatch".into());
            }
            s.sources.insert(hash.to_string(), body);
            s.puts += 1;
            (201, String::new())
        }
        _ => (404, String::new()),
    }
}

fn cut_small(name: &str) -> (PathBuf, CutReport) {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = fs::remove_dir_all(&dir);
    let g = Grid::new(300, 600).unwrap();
    let mut mem = MemorySource {
        grid: g,
        samples: (0..300)
            .flat_map(|r| (0..600).map(move |c| synth::elevation(&g, r, c)))
            .collect(),
    };
    let opts = CutOptions {
        threads: 2,
        progress: false,
    };
    let report = cut(&mut mem, g, &dir, opts).unwrap();
    (dir, report)
}

#[test]
fn uploads_tiles_then_the_manifest_and_skips_what_the_hub_has() {
    let (out, report) = cut_small("upload-ok");
    let state = Shared::default();
    let url = start_hub(state.clone());
    // The hub already has one tile.
    let first = tile_path(
        &out,
        TileKey {
            level: 0,
            row: 0,
            col: 0,
        },
    );
    let bytes = fs::read(first).unwrap();
    state
        .lock()
        .unwrap()
        .sources
        .insert(to_hex(&sha256(&bytes)), bytes);

    let hub = Hub {
        url: format!("{url}/"),
        api_key: KEY.into(),
    };
    let r = upload(&out, hub.clone(), false).unwrap();
    assert_eq!(r.tiles, report.tiles);
    assert_eq!(r.sent, report.distinct - 1);
    assert_eq!(r.sent + r.skipped, report.tiles);
    assert!(r.manifest_sent);
    assert_eq!(r.manifest_hash, report.manifest_hash);
    {
        let s = state.lock().unwrap();
        assert_eq!(s.sources.len() as u64, report.distinct);
        assert!(s.manifests.contains_key(&to_hex(&report.manifest_hash)));
    }

    // Running it again sends nothing.
    let puts = state.lock().unwrap().puts;
    let again = upload(&out, hub, false).unwrap();
    assert_eq!(
        (again.sent, again.skipped, again.manifest_sent),
        (0, report.tiles, false)
    );
    assert_eq!(state.lock().unwrap().puts, puts);
}

#[test]
fn refuses_a_tile_that_does_not_match_the_manifest() {
    let (out, _) = cut_small("upload-tampered");
    let url = start_hub(Shared::default());
    let path = tile_path(
        &out,
        TileKey {
            level: 1,
            row: 0,
            col: 1,
        },
    );
    fs::write(&path, b"not the tile").unwrap();
    let hub = Hub {
        url,
        api_key: KEY.into(),
    };
    let e = upload(&out, hub, false).unwrap_err();
    assert!(e.to_string().contains("does not match the manifest"), "{e}");
}

#[test]
fn reports_a_refused_key() {
    let (out, _) = cut_small("upload-key");
    let url = start_hub(Shared::default());
    let hub = Hub {
        url,
        api_key: "wrong".into(),
    };
    let e = upload(&out, hub, false).unwrap_err();
    assert!(e.to_string().contains("401"), "{e}");
}
