#!/usr/bin/env bash
#
# Read the version out of every manifest that carries one and fail if they
# disagree.
#
# The mirror of `set-version.sh`, which writes all six of these at once. A
# release cannot drift — the workflows stamp the tag in before they build — but
# a hand-edited `package.json` between releases can, and the first sign of it
# is `npm ci` refusing to install or a `--locked` cargo build refusing to
# start, on a runner, twenty minutes into someone else's pull request.
#
# So this runs before a merge instead. It takes no argument: the claim is that
# the six agree with each other, not that they equal any particular number.
#
#   scripts/check-version.sh

set -euo pipefail

cd "$(dirname "$0")/.."

# The same six extractors `set-version.sh` reads back with, and they have to
# stay the same six — a pattern that stops matching returns the empty string,
# which would otherwise read here as "this manifest disagrees" and send someone
# looking at the wrong file. Missing is therefore reported as missing, below.
read_version() {
	case "$1" in
	package.json | src-tauri/tauri.conf.json | package-lock.json)
		sed -nE 's|^  "version": "(.*)",?$|\1|p' "$1" | head -1
		;;
	src-tauri/Cargo.toml)
		sed -nE 's|^version = "(.*)"$|\1|p' "$1" | head -1
		;;
	src-tauri/Cargo.lock)
		awk '/^name = "swync"$/ { getline; sub(/^version = "/, ""); sub(/"$/, ""); print; exit }' "$1"
		;;
	esac
}

# `package-lock.json` carries the root package's version twice and npm compares
# both, so both are checked. The second sits at the same depth as every
# dependency's version in the file, hence the `""` key above it as the anchor.
root_lock_version() {
	awk '/^    "": \{$/ { root = 1 }
	     root && /^      "version": / { sub(/^      "version": "/, ""); sub(/",$/, ""); print; exit }' \
		package-lock.json
}

files=(package.json src-tauri/tauri.conf.json src-tauri/Cargo.toml
	src-tauri/Cargo.lock package-lock.json)

names=()
found=()
for file in "${files[@]}"; do
	[ -f "$file" ] || { printf '\033[31mcheck-version: %s is missing\033[0m\n' "$file" >&2; exit 1; }
	names+=("$file")
	found+=("$(read_version "$file")")
done
names+=('package-lock.json (root)')
found+=("$(root_lock_version)")

status=0
for i in "${!names[@]}"; do
	if [ -z "${found[$i]}" ]; then
		printf '  %-28s \033[31mno version found\033[0m\n' "${names[$i]}"
		status=1
	else
		printf '  %-28s %s\n' "${names[$i]}" "${found[$i]}"
		[ "${found[$i]}" = "${found[0]}" ] || status=1
	fi
done

if [ "$status" -ne 0 ]; then
	printf '\033[31m\ncheck-version: these do not all say the same thing.\033[0m\n' >&2
	printf 'Run scripts/set-version.sh <version> to write one number into all of them.\n' >&2
	exit 1
fi

printf '\nall six say %s\n' "${found[0]}"
