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
#
# The layer config: attached to frame 4 alone and exclusively (it replaces
# the system module's coarse matter there), max depth 21, usesOrigin false
# (the matter does not depend on the origin), spanSeconds 0 (it never
# changes).
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

config=$(printf '{"module":"%s","sourceManifest":"%s","attachment":[4],"exclusive":true,"maxDepth":{"4":21},"spanSeconds":0,"usesOrigin":false}' \
    "$hash" "$manifest")

echo "== PUT /space/{id}/layer/terrain"
echo "$config"
curl -sS --fail-with-body -X PUT \
    -H "X-API-Key: $GX_API_KEY" \
    -H "Content-Type: application/json" \
    --data-binary "$config" \
    "$GX_HUB_URL/space/$GX_SPACE_ID/layer/terrain"
echo
