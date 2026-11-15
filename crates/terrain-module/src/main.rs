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

--tiles DIR reads DIR/{level}/{row}/{col}.3gte, the layout terrain-tiler cut
writes under out/tiles. --fixture uses the synthetic fixture tiles.";

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
        "conformance" => {
            let dir = PathBuf::from(args.get(1).ok_or(USAGE)?);
            conformance::write(&dir).map_err(|e| e.to_string())
        }
        _ => Err(USAGE.to_string()),
    }
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
