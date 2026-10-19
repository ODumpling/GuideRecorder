#!/bin/sh
# The app version lives in three files: Cargo.toml ([workspace.package]),
# app/package.json and app/src-tauri/tauri.conf.json. This keeps them equal.
#
#   scripts/version.sh             print the version (fails if the files differ)
#   scripts/version.sh check [tag] same, and fail unless `tag` is v<version>
#   scripts/version.sh set X.Y.Z   write X.Y.Z to all three (and Cargo.lock)
#
# Releasing: `scripts/version.sh set 0.2.0`, commit, `git tag v0.2.0`, push the
# commit and the tag. CI builds a draft GitHub Release (see docs/releasing.md).
set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
cargo_toml="$root/Cargo.toml"
package_json="$root/app/package.json"
tauri_conf="$root/app/src-tauri/tauri.conf.json"

cargo_version() {
    # The first `version = "..."` after [workspace.package].
    awk '/^\[workspace\.package\]/ { in_pkg = 1; next }
         /^\[/ { in_pkg = 0 }
         in_pkg && /^version *=/ { gsub(/.*= *"|".*/, ""); print; exit }' "$cargo_toml"
}

json_version() {
    node -p "require(process.argv[1]).version" "$1"
}

current() {
    c=$(cargo_version)
    p=$(json_version "$package_json")
    t=$(json_version "$tauri_conf")
    if [ "$c" != "$p" ] || [ "$c" != "$t" ]; then
        echo "versions differ: Cargo.toml $c, app/package.json $p, tauri.conf.json $t" >&2
        echo "run: scripts/version.sh set <version>" >&2
        exit 1
    fi
    echo "$c"
}

case "${1:-}" in
"")
    current
    ;;
check)
    v=$(current)
    if [ -n "${2:-}" ] && [ "$2" != "v$v" ]; then
        echo "tag $2 does not match the version $v (expected v$v)" >&2
        exit 1
    fi
    echo "$v"
    ;;
set)
    v=${2:?usage: scripts/version.sh set X.Y.Z}
    if ! echo "$v" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$'; then
        echo "not a semver version: $v" >&2
        exit 1
    fi
    awk -v v="$v" '/^\[workspace\.package\]/ { in_pkg = 1 }
         /^\[/ && !/^\[workspace\.package\]/ { in_pkg = 0 }
         in_pkg && /^version *=/ && !done { print "version = \"" v "\""; done = 1; next }
         { print }' "$cargo_toml" >"$cargo_toml.tmp"
    mv "$cargo_toml.tmp" "$cargo_toml"
    for f in "$package_json" "$tauri_conf"; do
        node -e '
            const fs = require("fs");
            const [file, version] = process.argv.slice(1);
            const json = JSON.parse(fs.readFileSync(file, "utf8"));
            json.version = version;
            fs.writeFileSync(file, JSON.stringify(json, null, 2) + "\n");
        ' "$f" "$v"
    done
    (cd "$root" && cargo update --workspace --quiet)
    current
    ;;
*)
    echo "usage: scripts/version.sh [check [tag] | set X.Y.Z]" >&2
    exit 2
    ;;
esac
