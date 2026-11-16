//! `terrain-tiler`: cuts the elevation dataset into the terrain tile
//! pyramid and uploads it to a hub (`client-compute.md` section 5.3).
//!
//! ```text
//! terrain-tiler cut <grid file> <out dir> [--grid ROWSxCOLS] [--threads N]
//! terrain-tiler upload <out dir>
//! terrain-tiler synth <grid file> <rows>
//! terrain-tiler glo30-band <band> <raw dir> <gebco grid file> <out dir> [--threads N]
//! terrain-tiler glo30-done <band> <out dir>
//! terrain-tiler glo30-finish <out dir>
//! ```
//!
//! `glo30-band` cuts one band of the GLO-30 pyramid (`glo30`), `glo30-done`
//! exits 0 when a band is done and 1 when not, and `glo30-finish` builds
//! the coarse levels, the presence map, the manifest, and the summary.
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
use terrain_tiler::glo30;
use terrain_tiler::source::{GridFile, RowOrder};
use terrain_tiler::synth;
use terrain_tiler::upload::{upload, Hub};
use terrain_tiles::Grid;

const USAGE: &str = "usage:
  terrain-tiler cut <grid file> <out dir> [--grid ROWSxCOLS] [--threads N]
  terrain-tiler upload <out dir>
  terrain-tiler synth <grid file> <rows>
  terrain-tiler glo30-band <band> <raw dir> <gebco grid file> <out dir> [--threads N]
  terrain-tiler glo30-done <band> <out dir>
  terrain-tiler glo30-finish <out dir>";

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
        Some("glo30-band") => run_glo30_band(&args[1..]),
        Some("glo30-done") => {
            let [band, out] = &args[1..] else {
                return Err(USAGE.into());
            };
            let band: u32 = band.parse().map_err(|_| format!("band: {band}"))?;
            if glo30::band_done(&PathBuf::from(out), band) {
                Ok(())
            } else {
                std::process::exit(1)
            }
        }
        Some("glo30-finish") => {
            let [out] = &args[1..] else {
                return Err(USAGE.into());
            };
            let start = Instant::now();
            let f = glo30::finish(&PathBuf::from(out)).map_err(|e| e.to_string())?;
            println!(
                "glo30-finish: {} tiles, {} bytes, {} level 0 tiles, manifest {} bytes, {:.0} s",
                f.tiles,
                f.bytes,
                f.level0,
                f.manifest_bytes,
                start.elapsed().as_secs_f64()
            );
            println!("manifest {}", to_hex(&f.manifest_hash));
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

fn run_glo30_band(args: &[String]) -> Result<(), String> {
    let mut positional = Vec::new();
    let mut threads = std::thread::available_parallelism().map_or(1, |n| n.get());
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == "--threads" {
            let v = it.next().ok_or(USAGE)?;
            threads = v.parse().map_err(|_| format!("--threads: {v}"))?;
        } else {
            positional.push(a);
        }
    }
    let [band, raw, gebco, out] = positional[..] else {
        return Err(USAGE.into());
    };
    let band: u32 = band.parse().map_err(|_| format!("band: {band}"))?;
    let start = Instant::now();
    let w = glo30::cut_band(
        band,
        &PathBuf::from(raw),
        &PathBuf::from(gebco),
        &PathBuf::from(out),
        threads,
    )
    .map_err(|e| e.to_string())?;
    let bytes: u64 = w.entries.iter().map(|e| e.length).sum();
    println!(
        "glo30-band {band}: {} tiles ({} land tiles at level 0), {bytes} bytes, {:.0} s",
        w.entries.len(),
        w.level0.len(),
        start.elapsed().as_secs_f64()
    );
    Ok(())
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
