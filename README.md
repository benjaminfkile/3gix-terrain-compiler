# 3gix-terrain-compiler

A 3GIX compiler module that builds the matter of one frame from elevation data, plus the tool that cuts the elevation grid into source tiles. See the 3GIXHub architecture docs (`space-model.md`, `client-compute.md`, `matter-format.md`).

The module implements `space-model.md` section 11 item 2: attached exclusively to the third planet's frame (frame 4 of the system module's registry), it builds that frame's whole matter, solid rock up to the elevation surface on the WGS84 ellipsoid with the Preliminary Reference Earth Model's density below, and no water. The tiler implements the source data side of `client-compute.md` section 5 for one dataset, the GEBCO global elevation grid.

Compilers are allowed to know what things are. This one knows it builds a planet's crust and interior; nothing it emits says so.

## Crates

| Crate | What it is |
|---|---|
| `crates/terrain-tiles` | The tile format, shared by the tiler and the module: grid geometry, tile bytes and ids, nearest sample and bicubic interpolation across tile edges. Builds for `wasm32-unknown-unknown`. |
| `crates/terrain-tiler` | The native CLI that cuts the grid file into tiles and uploads them to a hub. |
| `crates/terrain-module` | The module: library crate `terrain_module` (cdylib: the WebAssembly module; rlib: the CLI and the tests) and binary `terrain-module`. |
| `tools/tiles-wasm-check` | The determinism digest of `terrain-tiles` as a WebAssembly export, run under Node by `scripts/ci.sh`. |

`gx-core` and `gx-module-guest` come from GitHub, pinned by revision in the workspace `Cargo.toml`. The zstd crates are locked exactly as the core library locks them, and `scripts/ci.sh` checks that.

## The module

One request in (`client-compute.md` section 4), one section out. The origin and the span are ignored.

- **`registry`**: the empty registry. The system module declares frame 4; this module only fills it.
- **A cell of frame 4**, depth 0 to 21, resolution 32 at every depth. Each sample averages 4 x 4 x 4 stratified points, exactly as the system module samples its balls. A point is solid when its height above the ellipsoid is at or below the elevation surface there, vacuum above. Density is the solid fraction times the local density (the sum of the solid points' densities over 64); temperature and albedo are density-weighted means. Material: state solid, albedo 0.3 / 0.28 / 0.25 at or above the ellipsoid and 0.2 / 0.2 / 0.22 below it, roughness 0.9, attenuation 0 (opaque).
- **Geodesy** (`geodesy.rs`). Body z is the pole and body x points at the prime meridian, as the system module's registry orientation says. Latitude, longitude, and height come from Bowring's method with three iterations and a written-out inverse tangent (`gx_core::detmath` has none), from `+`, `-`, `*`, `/`, and `sqrt` only.
- **The surface** (`plan.rs`, `surface.rs`). Bicubic (Catmull-Rom) interpolation of one pyramid level, the coarsest whose equatorial spacing is no larger than the cell's sample spacing: level 8 for depths 0 to 3, then one level finer per depth, level 0 from depth 11. Below the data's spacing (about 463 m) the surface is that smooth interpolation; nothing is invented. Elevations are used as heights above the ellipsoid; the grid's own datum is mean sea level, and no geoid correction is applied (the two differ by up to about 100 m).
- **The interior** (`interior.rs`). Density: the PREM piecewise polynomials (Dziewonski and Anderson 1981), evaluated at the point's ellipsoidal radius scaled to the model's 6371 km, with the model's ocean layer replaced by its upper crust. Temperature: 288 K at the surface, along a geotherm through 1600 K at 100 km, 1830 K at 410 km, 1960 K at 660 km, 2600 K at 2700 km, 4000 K at the core boundary (2891 km), and 6230 K at the inner core boundary, held below; the sources are in the module docs.
- **Sources.** `gx_sources` lists the tiles of the chosen level that the bicubic stencil can read anywhere in the cell's latitude and longitude range, the neighbors the interpolation needs included. A cell entirely more than 11 km below the ellipsoid lists none and is filled from the profile alone; a cell entirely more than 9 km above it lists none and is empty. A cell over a pole spans every longitude and lists every tile of its rows.
- **Other keys.** Cells deeper than 21 and cells of every other frame the system module declares are empty sections. A frame nobody declares has no known geometry, and the module fails for it.
- **Mass.** With the fixture tiles, the matter at depth 3 adds up to 5.970e24 kg against the registry's 5.972e24 kg (0.04 percent low; the test requires 1 percent). The single depth 0 cell, 1.6e6 m per sample, gives 5.994e24 kg.

## Layer config

The matter depends on nothing but the key and the tiles, so the layer runs with `usesOrigin` false and its sections are shared by every space that runs the same module and manifest. It never changes, so `spanSeconds` is 0. It is attached to frame 4 alone and exclusively, so the system module's coarse ball is not directed there:

```json
{
  "module": "<sha256 of target/module/terrain.wasm>",
  "sourceManifest": "<sha256 of out/manifest.bin>",
  "attachment": [4],
  "exclusive": true,
  "maxDepth": { "4": 21 },
  "spanSeconds": 0,
  "usesOrigin": false
}
```

## Data source and license

The elevation and seafloor come from the GEBCO global grid (General Bathymetric Chart of the Oceans), the 15 arcsecond grid in its NetCDF4 form, published by the GEBCO Compilation Group under the International Hydrographic Organization and the Intergovernmental Oceanographic Commission of UNESCO. The grid is in the public domain and may be used free of charge, including commercially, provided its source is acknowledged. Acknowledge it as "GEBCO Compilation Group (year) GEBCO year Grid", with the release year of the grid you cut, and keep that acknowledgement with anything built from the tiles. The grid is not for navigation. This repository holds no GEBCO data: the tiles are cut on your machine and stored on your hub, and the committed fixture tiles are synthetic.

The interior density is the Preliminary Reference Earth Model: A. M. Dziewonski and D. L. Anderson (1981), Physics of the Earth and Planetary Interiors 25, 297 to 356.

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

## Publishing

Cut the grid and upload the tiles first (the two sections above); `upload` prints the manifest hash. Then

```sh
GX_HUB_URL=... GX_API_KEY=... GX_SPACE_ID=... sh scripts/publish.sh MANIFEST_SHA256
```

builds the module, uploads it (`PUT /module`, checking the returned hash), and puts layer `terrain` of the space (`PUT /space/{spaceId}/layer/terrain`) with the config above and the manifest hash from the argument. The key needs the admin capability. The variables come from the environment only; `.env.example` lists them.

## The module locally

```sh
sh scripts/build-module.sh                       # target/module/terrain.wasm, size and hash
cargo run --release -p terrain-module -- sources 4-16-40979-32775-32774
cargo run --release -p terrain-module -- describe 4-16-40979-32775-32774 --fixture
cargo run --release -p terrain-module -- describe KEY --tiles out/tiles
cargo run --release -p terrain-module -- compile KEY --tiles out/tiles --out cell.bin
cargo run --release -p terrain-module -- cell LAT_DEG LON_DEG HEIGHT_M DEPTH
```

`--tiles` reads the tiles `terrain-tiler cut` wrote; `--fixture` uses the synthetic fixture world. `cell` prints the key of the cell holding a place.

## Conformance

`conformance/module/` holds twelve vectors (`NAME.request.bin`, `NAME.tiles.bin`, `NAME.sha256`) and `index.json` saying what each covers: the registry, cells at depths 0, 3, 6, 8, 9, 11, 12, 14, 16, and 21, an interior cell, a coastline cell, a cell against a tile edge, the fixture summit, the fixture trench floor, and an empty cell above the surface. The tiles are fixture tiles: real ids of the dataset's pyramid filled from a synthetic world (`crates/terrain-module/src/fixture.rs`), mostly flat seafloor, so each vector's tiles take at most a few kilobytes. `tests/conformance.rs` regenerates them and requires every file to match; `scripts/ci.sh` runs them through the built module with the core determinism harness, natively and under Node. After an intended change:

```sh
cargo run --release -p terrain-module -- conformance conformance/module
```

and review the diff. A change of any hash is a new module hash, so the layer must be published again.

## Timing

Measured on the build container (10 cores, aarch64) with a synthetic grid of 8640 x 17280 samples (149 million, 1/25 of the full grid) written by `terrain-tiler synth`: the cut took 2.3 s and wrote 3470 tiles, 201 MB in all, about 58 KB per tile. Scaled to the full grid's 3.73 billion samples and 86459 tiles, that is about 1 minute of cutting and about 5 GB of tiles. HDF5 decompression of the real file may add some time, because the file is larger per sample than the synthetic one. Real relief compresses differently from the synthetic relief, so the true size could be anywhere from about half of that estimate up to the uncompressed bound of 10 GB.

## Checks

```sh
sh scripts/ci.sh
```

The script runs fmt, clippy with warnings as errors, the zstd pin check, the WebAssembly builds of the tile crate and the module, the tile determinism check under Node, the core module determinism harness over `conformance/module` (native host and Node), the tests, and a dash check. The harness comes from the core checkout cargo fetched for `gx-core`, or from `GX_CORE_DIR` when set. Node must be on the path. The tests write their own synthetic grid files in the dataset's HDF5 layout. They compare every tile with an independent in-memory pyramid, pin golden hashes, and cover resuming, both row orders, the file checks, and upload against a stand-in hub. The module's tests cover the summit (rock to the fixture elevation within one sample spacing, vacuum above), the join of two cells across a tile edge, the mass at depth 3 against the registry mass, compiling twice, the registry and other frames, cells beyond the elevation shell, and that the listed tiles cover every read near the poles, the antimeridian, and tile corners.
