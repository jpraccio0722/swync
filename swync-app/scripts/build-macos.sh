#!/usr/bin/env bash
#
# Build, sign, notarize and staple Swync for macOS.
#
# What comes out is a .dmg a stranger can download and open — which is the
# whole point of the exercise. A locally compiled app runs on the machine that
# built it whatever its signature, because Gatekeeper only objects to what
# arrived with a quarantine attribute on it. A download always has one, so
# everything below is about the one case building from source does not cover.
#
# Credentials never appear here. The signing key lives in the login keychain
# and the notarization credentials in a notarytool keychain profile, both put
# there by you; this script names them and nothing more.
#
# Usage:
#   scripts/build-macos.sh                 universal, signed, notarized
#   scripts/build-macos.sh --no-notarize   signed only, for a quick check
#   scripts/build-macos.sh --host-arch     this Mac's architecture alone

set -euo pipefail

# The notarytool keychain profile holding the Apple ID, team and app-specific
# password. Override if you named yours something else.
PROFILE="${SWYNC_NOTARY_PROFILE:-swync-notary}"

TARGET="universal-apple-darwin"
NOTARIZE=1

for arg in "$@"; do
	case "$arg" in
		--no-notarize) NOTARIZE=0 ;;
		--host-arch) TARGET="" ;;
		-h|--help) sed -n '3,18p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
		*) echo "unknown option: $arg" >&2; exit 2 ;;
	esac
done

cd "$(dirname "$0")/.."

say() { printf '\n\033[1m▸ %s\033[0m\n' "$1"; }
die() { printf '\n\033[31m✗ %s\033[0m\n' "$1" >&2; exit 1; }

# ---------------------------------------------------------------------------
# Preflight. Every check here fails in a way that is expensive to discover
# later: a wrong certificate is found after a twenty minute build, and a
# missing notary profile after the upload has already started.
# ---------------------------------------------------------------------------

say "Checking the signing identity"

# Developer ID Application is the only certificate notarization accepts. An
# "Apple Development" certificate signs fine and is rejected at submission,
# which is a long way to go to be told no.
# The `|| true` is load-bearing under `set -e`: no certificate means `grep`
# exits 1, which would kill the script here — silently, and one line before
# the message explaining what to do about it.
IDENTITY="$(security find-identity -v -p codesigning \
	| grep "Developer ID Application" \
	| head -1 \
	| sed -E 's/^[[:space:]]*[0-9]+\)[[:space:]]+[A-F0-9]+[[:space:]]+"(.*)"$/\1/' || true)"

if [ -z "$IDENTITY" ]; then
	die "No 'Developer ID Application' certificate in the keychain.

Create one in Xcode — Settings ▸ Accounts ▸ your Apple ID ▸ Manage
Certificates ▸ + ▸ Developer ID Application — or at
https://developer.apple.com/account/resources/certificates

An 'Apple Development' certificate is not a substitute: it signs, and
notarization refuses it."
fi

echo "  $IDENTITY"

if [ "$NOTARIZE" = 1 ]; then
	say "Checking the notarization credentials"

	# Stored once, interactively, by you — never by this script, and never
	# passed on a command line where it would land in shell history:
	#
	#   xcrun notarytool store-credentials "swync-notary" \
	#     --apple-id "you@example.com" --team-id "YOURTEAMID"
	#
	# It asks for an app-specific password, which is made at
	# https://account.apple.com under Sign-In and Security ▸ App-Specific
	# Passwords. Your ordinary Apple ID password will not do.
	if ! xcrun notarytool history --keychain-profile "$PROFILE" >/dev/null 2>&1; then
		die "No notarytool profile named '$PROFILE'.

Create it once with:

  xcrun notarytool store-credentials \"$PROFILE\" \\
    --apple-id \"your@apple.id\" --team-id \"YOURTEAMID\"

It will ask for an app-specific password — made at account.apple.com
under Sign-In and Security, not your ordinary Apple ID password.

Then run this script again. Or pass --no-notarize to skip it, which
produces a signed build that still warns on another machine."
	fi
	echo "  profile '$PROFILE' answers"
fi

if [ -n "$TARGET" ]; then
	# A universal binary is two builds glued together, so the Intel half has to
	# be installed even on an Apple Silicon Mac.
	if ! rustup target list --installed | grep -q "^x86_64-apple-darwin$"; then
		say "Adding the x86_64 target for the universal build"
		rustup target add x86_64-apple-darwin
	fi
fi

# ---------------------------------------------------------------------------
# Build. Tauri signs the app as it bundles when this is set.
# ---------------------------------------------------------------------------

say "Building"

export APPLE_SIGNING_IDENTITY="$IDENTITY"

if [ -n "$TARGET" ]; then
	npm run tauri build -- --target "$TARGET"
	BUNDLE="src-tauri/target/$TARGET/release/bundle"
else
	npm run tauri build
	BUNDLE="src-tauri/target/release/bundle"
fi

APP="$(find "$BUNDLE/macos" -maxdepth 1 -name '*.app' | head -1)"
DMG="$(find "$BUNDLE/dmg" -maxdepth 1 -name '*.dmg' | head -1)"

[ -n "$APP" ] || die "No .app under $BUNDLE/macos — the build did not get that far."
[ -n "$DMG" ] || die "No .dmg under $BUNDLE/dmg — the build did not get that far."

say "Verifying the signature"
codesign --verify --deep --strict --verbose=2 "$APP"

if [ "$NOTARIZE" = 0 ]; then
	printf '\n\033[33m! Signed but not notarized.\033[0m %s\n' "$DMG"
	echo "  This opens on this Mac and warns on any other. Drop --no-notarize to ship it."
	exit 0
fi

# ---------------------------------------------------------------------------
# Notarize. The disk image is what gets submitted, so the app inside is
# covered by the same ticket and the download is the thing that was checked.
# ---------------------------------------------------------------------------

say "Notarizing — this waits on Apple, usually a few minutes"

xcrun notarytool submit "$DMG" --keychain-profile "$PROFILE" --wait

# Stapling writes the ticket into the file, so the first launch does not
# depend on the machine being online to ask Apple whether this is known.
say "Stapling"
xcrun stapler staple "$DMG"
xcrun stapler staple "$APP"   # so the .app is also distributable on its own

say "Checking it the way Gatekeeper will"
spctl --assess --type open --context context:primary-signature -vv "$DMG"

printf '\n\033[32m✓ Ready to ship\033[0m\n%s\n' "$DMG"
