#!/usr/bin/env bash
#
# Write one version number into every manifest that carries one.
#
# There are five, and a release is wrong the moment they disagree with the tag
# it was cut from: `tauri.conf.json` is what names the disk image and fills in
# CFBundleShortVersionString, `Cargo.toml` is what `CARGO_PKG_VERSION` reads,
# `package.json` is the one a person looks at first, and the two lockfiles have
# to follow their manifests or `npm ci` and a `--locked` cargo build refuse to
# start. Left to a human they drift, which is how a tag of v0.0.1 shipped a
# Swync_0.1.0_universal.dmg.
#
# The release workflows call this with the tag before they build, so the tag is
# the version and cannot be anything else. Run it by hand to bump the committed
# version between releases:
#
#   scripts/set-version.sh 0.2.0
#
# A leading `v` is accepted, so a tag name can be passed through unedited.

set -euo pipefail

die() { printf '\033[31mset-version: %s\033[0m\n' "$1" >&2; exit 1; }

[ $# -eq 1 ] || die "usage: set-version.sh <version>"

VERSION="${1#v}"

# Three numbers and nothing else. A prerelease suffix is tempting to allow, but
# an MSI's ProductVersion is strictly numeric and CFBundleShortVersionString is
# too, so `v0.2.0-beta.1` would sail through this check and then fail deep
# inside the bundler on whichever platform got there first. Better to be told
# here, by name, before twenty minutes of compilation.
[[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || die \
	"'$1' is not a version of the form x.y.z.
Windows installers and macOS bundles both require three plain numbers, so a
suffix like -beta.1 is rejected here rather than by the bundler later."

cd "$(dirname "$0")/.."

# Each of these is a targeted substitution rather than a JSON or TOML rewrite,
# because reformatting a manifest to change one field makes a diff nobody can
# read. The anchors are chosen to match exactly one line: two-space indent for
# the top-level "version" key in the JSON files (nothing nested sits at that
# depth), and a line start for Cargo.toml's, where dependency versions are all
# inline in tables.
edit() {
	local file="$1" pattern="$2"
	[ -f "$file" ] || die "$file is missing"
	# A temporary file rather than `sed -i`, whose spelling differs between
	# BSD and GNU and whose difference is a runner-only failure.
	sed -E "$pattern" "$file" > "$file.tmp" && mv "$file.tmp" "$file"
}

edit package.json                 "s|^  \"version\": \".*\"|  \"version\": \"$VERSION\"|"
edit src-tauri/tauri.conf.json    "s|^  \"version\": \".*\"|  \"version\": \"$VERSION\"|"
edit src-tauri/Cargo.toml         "s|^version = \".*\"|version = \"$VERSION\"|"

# `package-lock.json` records the root package's version twice — once at the
# top and once as the `""` entry under `packages` — and `npm ci` compares both
# against `package.json` before it installs anything. Skipping this file would
# turn every tagged build into a lockfile-out-of-sync failure on the step after
# this one. Only the first is safe to anchor by indent; the second sits at the
# same depth as the version of every dependency in the file, so it takes the
# `""` key above it to tell them apart.
edit package-lock.json            "s|^  \"version\": \".*\"|  \"version\": \"$VERSION\"|"

awk -v v="$VERSION" '
	/^    "": \{$/ { root = 1 }
	root && /^      "version": / { print "      \"version\": \"" v "\","; root = 0; next }
	{ print }
' package-lock.json > package-lock.json.tmp && mv package-lock.json.tmp package-lock.json

# The lockfile lists every crate in the build, so the anchor has to be the one
# entry named `swync` rather than a line pattern — `version = "..."` on its own
# matches four hundred other packages.
awk -v v="$VERSION" '
	/^name = "swync"$/ { swync = 1 }
	swync && /^version = / { print "version = \"" v "\""; swync = 0; next }
	{ print }
' src-tauri/Cargo.lock > src-tauri/Cargo.lock.tmp && mv src-tauri/Cargo.lock.tmp src-tauri/Cargo.lock

# Read every one of them back. A substitution that matched nothing exits 0 and
# leaves the file untouched, which is the failure mode worth catching: it would
# otherwise surface as a correctly named workflow step that quietly did nothing
# and a release that is wrong in exactly the way this script exists to prevent.
check() {
	local file="$1" found="$2"
	[ "$found" = "$VERSION" ] || die \
		"$file still reads '$found' after the edit — its formatting has moved
and the pattern in this script no longer matches it."
	printf '  %-28s %s\n' "$file" "$found"
}

check package.json              "$(sed -nE 's|^  "version": "(.*)",?$|\1|p' package.json | head -1)"
check src-tauri/tauri.conf.json "$(sed -nE 's|^  "version": "(.*)",?$|\1|p' src-tauri/tauri.conf.json | head -1)"
check src-tauri/Cargo.toml      "$(sed -nE 's|^version = "(.*)"$|\1|p' src-tauri/Cargo.toml | head -1)"
check src-tauri/Cargo.lock      "$(awk '/^name = "swync"$/ { getline; sub(/^version = "/, ""); sub(/"$/, ""); print; exit }' src-tauri/Cargo.lock)"
check package-lock.json         "$(sed -nE 's|^  "version": "(.*)",?$|\1|p' package-lock.json | head -1)"
check 'package-lock.json (root)' "$(awk '/^    "": \{$/ { root = 1 } root && /^      "version": / { sub(/^      "version": "/, ""); sub(/",$/, ""); print; exit }' package-lock.json)"
