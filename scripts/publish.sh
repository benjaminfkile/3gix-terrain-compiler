#!/bin/sh
# Builds the terrain module and publishes it to a hub (client-compute.md
# section 2.1): uploads the module bytes (PUT /module), then puts layer
# `terrain` of a space with the module's hash and the source manifest
# given as the argument (PUT /space/{id}/layer/terrain).
#
# Usage: sh scripts/publish.sh SOURCE_MANIFEST_SHA256
#
# The manifest hash is the one `terrain-tiler upload` prints; the manifest
# and every tile it lists must already be stored on the hub.
#
# Reads everything that names a hub from the environment; nothing of it is
# in this repository:
#
#   GX_HUB_URL    hub base URL, without a trailing slash
#   GX_API_KEY    an API key with the admin capability (X-API-Key)
#   GX_SPACE_ID   the space to put the layer in
#   GX_FRAME_ID   the frame the layer fills (default 4, the system
#                 module's frame for this data)
#
# The layer config: attached to frame GX_FRAME_ID alone and exclusively (it
# replaces the system module's coarse matter there), max depth 21,
# usesOrigin false (the matter does not depend on the origin), spanSeconds 0
# (it never changes), and one input: that frame's record from the `system`
# layer (client-compute.md section 10.2), slot 0. The module fills whichever
# frame the record names.
set -eu

cd "$(dirname "$0")/.."

if [ "$#" -ne 1 ]; then
    echo "usage: sh scripts/publish.sh SOURCE_MANIFEST_SHA256" >&2
    exit 2
fi
manifest=$1
case "$manifest" in
    *[!0-9a-f]*) echo "the manifest hash must be 64 lowercase hex digits" >&2; exit 2 ;;
esac
if [ "${#manifest}" -ne 64 ]; then
    echo "the manifest hash must be 64 lowercase hex digits" >&2
    exit 2
fi

: "${GX_HUB_URL:?set GX_HUB_URL}"
: "${GX_API_KEY:?set GX_API_KEY}"
: "${GX_SPACE_ID:?set GX_SPACE_ID}"
frame=${GX_FRAME_ID:-4}
case "$frame" in
    ""|*[!0-9]*) echo "GX_FRAME_ID must be a frame id (digits)" >&2; exit 2 ;;
esac

sh scripts/build-module.sh
wasm=target/module/terrain.wasm
hash=$(sha256sum "$wasm" | cut -d ' ' -f 1)

echo "== PUT /module"
reply=$(curl -sS --fail-with-body -X PUT \
    -H "X-API-Key: $GX_API_KEY" \
    -H "Content-Type: application/wasm" \
    --data-binary "@$wasm" \
    "$GX_HUB_URL/module")
echo "$reply"
case "$reply" in
    *"$hash"*) ;;
    *) echo "the hub returned a hash other than $hash" >&2; exit 1 ;;
esac

config=$(printf '{"module":"%s","sourceManifest":"%s","attachment":[%s],"exclusive":true,"maxDepth":{"%s":21},"spanSeconds":0,"usesOrigin":false,"inputs":[{"layer":"system","frames":[%s]}]}' \
    "$hash" "$manifest" "$frame" "$frame" "$frame")

echo "== PUT /space/{id}/layer/terrain"
echo "$config"
curl -sS --fail-with-body -X PUT \
    -H "X-API-Key: $GX_API_KEY" \
    -H "Content-Type: application/json" \
    --data-binary "$config" \
    "$GX_HUB_URL/space/$GX_SPACE_ID/layer/terrain"
echo
