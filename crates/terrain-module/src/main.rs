//! The terrain module's command line: compile, describe, and list the
//! sources of a key natively, find the cell at a place, and regenerate the
//! conformance vectors. It calls the same functions the module runs.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use gx_core::key::ChunkKey;
use gx_core::matter;
use gx_core::sha256::{sha256, to_hex};
use terrain_module::fixture::DEG;
use terrain_module::geodesy::{frame_point, Geodetic};
use terrain_module::{compile, conformance, fixture, plan};
use terrain_tiles::{Tile, TileKey};

const USAGE: &str = "usage:
  terrain-module sources KEY
  terrain-module compile KEY (--fixture | --tiles DIR) [--out FILE]
  terrain-module describe KEY (--fixture | --tiles DIR)
  terrain-module cell LAT_DEG LON_DEG HEIGHT_M DEPTH
  terrain-module conformance DIR
  terrain-module bench RUNS KEY... (--fixture | --tiles DIR)

--tiles DIR reads DIR/{level}/{row}/{col}.3gte, the layout terrain-tiler cut
writes under out/tiles. --fixture uses the synthetic fixture tiles.

bench times what gx_compile does for each key (decode the tile bytes,
compile, encode, validate), RUNS times, and prints the median, minimum, and
maximum in milliseconds. The tiles are read or made once, before timing.";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: &[String]) -> Result<(), String> {
    let cmd = args.first().ok_or(USAGE)?;
    match cmd.as_str() {
        "sources" => {
            let key = parse_key(args.get(1))?;
            for id in compile::sources(&key) {
                let k = TileKey::from_id(id).ok_or("bad id")?;
                println!("{id} {k}");
            }
            Ok(())
        }
        "compile" | "describe" => {
            let key = parse_key(args.get(1))?;
            let tiles = load_tiles(&key, &args[2..])?;
            let bytes = compile::compile(&key, &tiles).map_err(|e| e.to_string())?;
            eprintln!("{} bytes sha256 {}", bytes.len(), to_hex(&sha256(&bytes)));
            if cmd == "describe" {
                describe(&key, &bytes)?;
            } else if let Some(out) = flag(&args[2..], "--out") {
                std::fs::write(&out, &bytes).map_err(|e| format!("{out}: {e}"))?;
            } else {
                use std::io::Write;
                std::io::stdout()
                    .write_all(&bytes)
                    .map_err(|e| e.to_string())?;
            }
            Ok(())
        }
        "cell" => {
            let num = |i: usize| -> Result<f64, String> {
                args.get(i)
                    .ok_or(USAGE)?
                    .parse::<f64>()
                    .map_err(|e| e.to_string())
            };
            let depth = num(4)? as u8;
            let p = frame_point(Geodetic {
                lat: num(1)? * DEG,
                lon: num(2)? * DEG,
                h: num(3)?,
            });
            let key = plan::cell_at(depth, p).ok_or("outside the frame")?;
            println!("{}", ChunkKey::Cell(key));
            Ok(())
        }
        "bench" => bench(&args[1..]),
        "conformance" => {
            let dir = PathBuf::from(args.get(1).ok_or(USAGE)?);
            conformance::write(&dir).map_err(|e| e.to_string())
        }
        _ => Err(USAGE.to_string()),
    }
}

/// `bench RUNS KEY... (--fixture | --tiles DIR)`: see [`USAGE`].
fn bench(args: &[String]) -> Result<(), String> {
    let runs: usize = args
        .first()
        .ok_or(USAGE)?
        .parse()
        .map_err(|e| format!("runs: {e}"))?;
    let opts = args
        .iter()
        .position(|a| a.starts_with("--"))
        .unwrap_or(args.len());
    let mut medians = Vec::new();
    for k in &args[1..opts] {
        let key = parse_key(Some(k))?;
        let tiles = load_tiles(&key, &args[opts..])?;
        let encoded: Vec<(u64, Vec<u8>)> = tiles.iter().map(|(&id, t)| (id, t.encode())).collect();
        let mut times = Vec::with_capacity(runs);
        let mut out = Vec::new();
        for _ in 0..runs.max(1) {
            let start = std::time::Instant::now();
            let decoded = compile::decode_tiles(encoded.iter().map(|(id, b)| (*id, b.as_slice())))
                .map_err(|e| e.to_string())?;
            out = compile::compile(&key, &decoded).map_err(|e| e.to_string())?;
            times.push(start.elapsed().as_secs_f64() * 1e3);
        }
        times.sort_by(f64::total_cmp);
        let median = times[times.len() / 2];
        medians.push(median);
        println!(
            "{key} tiles {} bytes {} sha256 {} median {median:.2} ms min {:.2} max {:.2}",
            encoded.len(),
            out.len(),
            &to_hex(&sha256(&out))[..16],
            times[0],
            times[times.len() - 1]
        );
    }
    medians.sort_by(f64::total_cmp);
    if let Some(m) = medians.get(medians.len() / 2) {
        println!("median of the keys' medians {m:.2} ms");
    }
    Ok(())
}

fn parse_key(s: Option<&String>) -> Result<ChunkKey, String> {
    s.ok_or(USAGE)?
        .parse::<ChunkKey>()
        .map_err(|e| format!("key: {e}"))
}

fn flag(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1).cloned())
}

fn load_tiles(key: &ChunkKey, args: &[String]) -> Result<BTreeMap<u64, Tile>, String> {
    let ids = compile::sources(key);
    if args.iter().any(|a| a == "--fixture") {
        return Ok(fixture::tiles(&ids));
    }
    let dir = flag(args, "--tiles").ok_or(USAGE)?;
    let mut out = BTreeMap::new();
    for id in ids {
        let k = TileKey::from_id(id).ok_or("bad id")?;
        let path = Path::new(&dir)
            .join(k.level.to_string())
            .join(k.row.to_string())
            .join(format!("{}.3gte", k.col));
        let bytes = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        out.insert(id, Tile::decode_id(id, &bytes).map_err(|e| e.to_string())?);
    }
    Ok(out)
}

fn describe(key: &ChunkKey, bytes: &[u8]) -> Result<(), String> {
    let ChunkKey::Cell(cell) = key else {
        println!("{{\"registry\":\"empty\"}}");
        return Ok(());
    };
    let s = matter::decode(cell, bytes).map_err(|e| e.to_string())?;
    let solid = s
        .samples()
        .map(|v| (0..v.len()).filter(|&i| v.density(i).value() > 0.0).count())
        .unwrap_or(0);
    println!(
        "{{\"key\":\"{key}\",\"origin\":[{},{},{}],\"edge\":{},\"resolution\":{},\"solid\":{solid},\"mass\":{}}}",
        s.origin().x,
        s.origin().y,
        s.origin().z,
        s.edge().value(),
        s.resolution(),
        s.mass().value()
    );
    Ok(())
}
