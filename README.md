# 3gix-terrain-compiler

A 3GIX compiler module that builds the matter of one frame from elevation data, plus the tool that cuts the elevation grid into source tiles. See the 3GIXHub architecture docs (`space-model.md`, `client-compute.md`, `matter-format.md`).

This repository implements the source data side of `client-compute.md` section 5 for one dataset, the GEBCO global elevation grid. The module itself (task H2) is not written yet.

## Crates

| Crate | What it is |
|---|---|
| `crates/terrain-tiles` | The tile format, shared by the tiler and the module: grid geometry, tile bytes and ids, nearest sample and bicubic interpolation across tile edges. Builds for `wasm32-unknown-unknown`. |
| `crates/terrain-tiler` | The native CLI that cuts the grid file into tiles and uploads them to a hub. |
| `crates/terrain-module` | Placeholder for the module (H2). |
| `tools/tiles-wasm-check` | The determinism digest of `terrain-tiles` as a WebAssembly export, run under Node by `scripts/ci.sh`. |

`gx-core` comes from GitHub, pinned by revision in the workspace `Cargo.toml`. The zstd crates are locked exactly as the core library locks them, and `scripts/ci.sh` checks that.

## Tile format

The full definition is in the module docs of `terrain-tiles` (`grid` and `tile`). In short:

- **Grid.** Equirectangular in geodetic latitude and longitude, cell centered: data row `r` (first row northernmost) is centered on latitude `90 deg - (r + 1/2) s`, data column `c` on longitude `-180 deg + (c + 1/2) s`, with `s = 15 arcseconds` (43200 rows, 86400 columns). No sample sits on a pole or on the antimeridian.
- **Levels.** Level 0 has the data's spacing; level `k` has `2^k` times it. Every tile is 240 x 240 samples, so a level `k` tile spans `2^k` degrees. Levels run up to the first that covers the globe in at most 2 x 1 tiles: level 8, nine levels, 86459 tiles in all.
- **Coarser levels.** Each sample is the mean of the 2 x 2 samples below it, in integer arithmetic, rounded half away from zero. Columns past 180 degrees wrap. Rows past the south pole repeat the last row that touches the globe.
- **Bytes.** One zstd frame (level 9) of: `"3GTE"`, version `u16` 1, level `u8`, reserved `u8` 0, row `u32`, col `u32`, then 240 x 240 `i16` LE elevations in meters, north row first, west column first. Everything little-endian.
- **Id.** `(level << 48) | (row << 24) | col`.

Angles in the API are radians. `Grid::position`, `Grid::bicubic_tiles`, and `Grid::tiles_for_region` say where a point falls and which tiles it needs. `nearest` and `bicubic` (Catmull-Rom) read elevations from tiles the caller supplies. Only `+`, `-`, `*`, `/`, and `floor` are used, so native code and WebAssembly agree bit for bit. `scripts/ci.sh` proves it: it computes a digest over encoding, decoding, and interpolation natively and under Node and requires the two to match.

## Cutting

```sh
cargo build --release -p terrain-tiler
target/release/terrain-tiler cut GEBCO_grid.nc out
```

The file must be the NetCDF4 grid: one `elevation` variable of `i16`, 43200 x 86400, with `lat` and `lon` coordinate variables. The tiler checks the shape, the type, and the first and last coordinates before it cuts. Latitude may ascend (the dataset's file stores the south row first) or descend; tiles are always north first. HDF5 is built from source with zlib (`hdf5-metno` with `static` and `zlib`), so no system package is needed. Only a C compiler and CMake are required.

Output:

```
out/tiles/{level}/{row}/{col}.3gte   every tile of every level
out/manifest.bin                     the source manifest (client-compute.md 5.1)
out/summary.json                     counts and bytes per level, manifest hash, elevation range
```

The cut is deterministic: the same file always gives the same bytes, whatever the thread count. It is resumable: if you run it again, tiles already on disk with the right hash are not rewritten, and damaged or missing ones are. It streams: the grid is read one band of 240 rows at a time, and each level keeps at most two full-width rows of tiles in memory (about 250 MB at most for the full grid).

`--grid ROWSxCOLS` cuts a grid of another shape, and `terrain-tiler synth FILE ROWS` writes a synthetic grid in the same layout. The tests and the timing below use these.

## Uploading

```sh
GX_HUB_URL=... GX_API_KEY=... target/release/terrain-tiler upload out
```

For each tile in the manifest: read it, check it against the manifest, then `HEAD /source/{hash}` and `PUT /source/{hash}` unless the hub already has it. Tiles with identical bytes go up once. Then the manifest goes up with `PUT /source-manifest`, unless `HEAD /source-manifest/{hash}` says it is stored. The tool prints progress and the manifest hash. The key needs the admin capability. See `.env.example`.

## Timing

Measured on the build container (10 cores, aarch64) with a synthetic grid of 8640 x 17280 samples (149 million, 1/25 of the full grid) written by `terrain-tiler synth`: the cut took 2.3 s and wrote 3470 tiles, 201 MB in all, about 58 KB per tile. Scaled to the full grid's 3.73 billion samples and 86459 tiles, that is about 1 minute of cutting and about 5 GB of tiles. HDF5 decompression of the real file may add some time, because the file is larger per sample than the synthetic one. Real relief compresses differently from the synthetic relief, so the true size could be anywhere from about half of that estimate up to the uncompressed bound of 10 GB.

## Checks

```sh
sh scripts/ci.sh
```

The script runs fmt, clippy with warnings as errors, the zstd pin check, the WebAssembly build of the tile crate and the placeholder module, the Node determinism check, the tests, and a dash check. The tests write their own synthetic grid files in the dataset's HDF5 layout. They compare every tile with an independent in-memory pyramid, pin golden hashes, and cover resuming, both row orders, the file checks, and upload against a stand-in hub.
