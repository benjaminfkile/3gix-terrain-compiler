//! `terrain-tiler`: cuts the elevation dataset into the terrain tile
//! pyramid and uploads it to a hub (`client-compute.md` section 5.3).
//!
//! ```text
//! terrain-tiler cut <grid file> <out dir> [--grid ROWSxCOLS] [--threads N]
//! terrain-tiler upload <out dir>
//! terrain-tiler synth <grid file> <rows>
//! ```
//!
//! `cut` checks the file against the dataset's grid (43200 by 86400) unless
//! `--grid` names another shape. `upload` reads `GX_HUB_URL` and
//! `GX_API_KEY` from the environment. `synth` writes a synthetic grid file of
//! `rows` by `2 rows` samples in the dataset's layout, for timing a cut.

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

use gx_core::sha256::to_hex;
use terrain_tiler::cut::{cut, CutOptions};
use terrain_tiler::source::{GridFile, RowOrder};
use terrain_tiler::synth;
use terrain_tiler::upload::{upload, Hub};
use terrain_tiles::Grid;

const USAGE: &str = "usage:
  terrain-tiler cut <grid file> <out dir> [--grid ROWSxCOLS] [--threads N]
  terrain-tiler upload <out dir>
  terrain-tiler synth <grid file> <rows>";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("terrain-tiler: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: &[String]) -> Result<(), String> {
    match args.first().map(String::as_str) {
        Some("cut") => run_cut(&args[1..]),
        Some("upload") => {
            let [dir] = &args[1..] else {
                return Err(USAGE.into());
            };
            let hub = Hub::from_env().map_err(|e| e.to_string())?;
            let r = upload(&PathBuf::from(dir), hub, true).map_err(|e| e.to_string())?;
            println!(
                "upload: {} tiles, {} sent, {} skipped, {} bytes sent",
                r.tiles, r.sent, r.skipped, r.bytes_sent
            );
            println!("manifest {}", to_hex(&r.manifest_hash));
            Ok(())
        }
        Some("synth") => {
            let [path, rows] = &args[1..] else {
                return Err(USAGE.into());
            };
            let rows: u32 = rows.parse().map_err(|_| format!("rows: {rows}"))?;
            let grid = Grid::new(rows, 2 * rows).ok_or("no pyramid has that many rows")?;
            synth::write(&PathBuf::from(path), grid, RowOrder::SouthFirst)
                .map_err(|e| e.to_string())?;
            println!("synth: {rows} x {} grid written to {path}", 2 * rows);
            Ok(())
        }
        _ => Err(USAGE.into()),
    }
}

fn run_cut(args: &[String]) -> Result<(), String> {
    let mut positional = Vec::new();
    let mut grid = Grid::GEBCO;
    let mut opts = CutOptions {
        progress: true,
        ..CutOptions::default()
    };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--grid" => {
                let v = it.next().ok_or(USAGE)?;
                let (r, c) = v.split_once('x').ok_or_else(|| format!("--grid: {v}"))?;
                let parse = |s: &str| s.parse::<u32>().map_err(|_| format!("--grid: {v}"));
                grid = Grid::new(parse(r)?, parse(c)?)
                    .ok_or_else(|| format!("--grid: {v} is not a pyramid grid"))?;
            }
            "--threads" => {
                let v = it.next().ok_or(USAGE)?;
                opts.threads = v.parse().map_err(|_| format!("--threads: {v}"))?;
            }
            _ => positional.push(a),
        }
    }
    let [path, out] = positional[..] else {
        return Err(USAGE.into());
    };
    let start = Instant::now();
    let mut file = GridFile::open(&PathBuf::from(path), grid).map_err(|e| e.to_string())?;
    let r = cut(&mut file, grid, &PathBuf::from(out), opts).map_err(|e| e.to_string())?;
    let secs = start.elapsed().as_secs_f64();
    println!(
        "cut: {} tiles ({} written, {} already present), {} tile bytes, elevation {} m to {} m",
        r.tiles, r.written, r.skipped, r.bytes, r.min_elevation, r.max_elevation
    );
    println!(
        "cut: {secs:.2} s for {} data samples",
        u64::from(grid.rows()) * u64::from(grid.cols())
    );
    println!("manifest {}", to_hex(&r.manifest_hash));
    Ok(())
}
