#!/bin/sh
# Cuts the GLO-30 land pyramid band by band (crates/terrain-tiler/src/glo30.rs):
# for each of the 45 bands of four degrees of latitude, north first, downloads
# that band's raw Copernicus GLO-30 tiles (DEM and water body mask) from the
# public open data bucket, cuts them with terrain-tiler glo30-band, and deletes
# them before the next band, so the raw set never sits on disk whole. Then
# builds the coarse levels, the presence map, and the manifest
# (glo30-finish).
#
#   sh scripts/glo30.sh <work dir> <GEBCO grid file> <out dir>
#
# <work dir> holds tileList.txt and one band of raw tiles at a time. Rerun to
# resume: done bands are skipped, a band in progress downloads only what is
# missing. Needs curl and the release build of terrain-tiler
# (cargo build --release -p terrain-tiler).
set -eu

[ $# -eq 3 ] || { echo "usage: sh scripts/glo30.sh <work dir> <GEBCO grid file> <out dir>" >&2; exit 2; }
work=$1 gebco=$2 out=$3
cd "$(dirname "$0")/.."
tiler=$(pwd)/target/release/terrain-tiler
[ -x "$tiler" ] || { echo "build first: cargo build --release -p terrain-tiler" >&2; exit 1; }
bucket=https://copernicus-dem-30m.s3.amazonaws.com
parallel=${GLO30_DOWNLOADS:-16}
mkdir -p "$work" "$out"

# The list has CRLF line ends; keep it with LF.
[ -s "$work/tileList.txt" ] || curl -fsS --retry 5 "$bucket/tileList.txt" | tr -d '\r' > "$work/tileList.txt"
tr -d '\r' < "$work/tileList.txt" > "$work/tileList.lf" && mv "$work/tileList.lf" "$work/tileList.txt"
echo "$(wc -l < "$work/tileList.txt" | tr -d ' ') raw tiles listed"

band=0
while [ $band -lt 45 ]; do
    if "$tiler" glo30-done $band "$out"; then band=$((band + 1)); continue; fi
    north=$((90 - 4 * band))
    dir=$work/band-$band
    mkdir -p "$dir"
    # Raw tiles whose south edge is north-1 .. north-4.
    names=$(awk -v n=$north '{
        lat = substr($0, 24, 2) + 0; if (substr($0, 23, 1) == "S") lat = -lat
        if (lat <= n - 1 && lat >= n - 4) print
    }' "$work/tileList.txt")
    count=$(printf '%s\n' "$names" | grep -c . || true)
    echo "== band $band ($((north - 4)) to $north degrees): $count raw tiles, $(date '+%H:%M:%S')"
    if [ "$count" -gt 0 ]; then
        # One "url destination" pair per file not yet downloaded.
        printf '%s\n' "$names" | while read -r n; do
            b=${n%_DEM}
            [ -s "$dir/$n.tif" ] || echo "$bucket/$n/$n.tif $dir/$n.tif"
            [ -s "$dir/${b}_WBM.tif" ] || echo "$bucket/$n/AUXFILES/${b}_WBM.tif $dir/${b}_WBM.tif"
        done | xargs -n 2 -P "$parallel" sh -c \
            'curl -fsS --retry 8 --retry-all-errors -o "$1.part" "$0" && mv "$1.part" "$1"'
        missing=$(printf '%s\n' "$names" | while read -r n; do
            b=${n%_DEM}; [ -s "$dir/$n.tif" ] && [ -s "$dir/${b}_WBM.tif" ] || echo "$n"
        done | grep -c . || true)
        [ "$missing" -eq 0 ] || { echo "   $missing raw tiles failed to download; rerun to resume" >&2; exit 1; }
        echo "   downloaded $(du -sh "$dir" | cut -f1), $(date '+%H:%M:%S')"
    fi
    "$tiler" glo30-band $band "$dir" "$gebco" "$out"
    rm -rf "$dir"
    echo "   tiles so far $(du -sh "$out" | cut -f1), disk free $(df -h "$out" | awk 'NR==2 {print $4}')"
    band=$((band + 1))
done

"$tiler" glo30-finish "$out"
