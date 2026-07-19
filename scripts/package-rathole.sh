#!/bin/sh
set -eu

RATHOLE_VERSION="v0.5.0"
TARGET="armv7-unknown-linux-musleabihf"
ARCHIVE="plugins/rathole/vendor/rathole-armv7-unknown-linux-musleabihf.zip"
ARCHIVE_SHA256="e8662d80d2cc9acc5f8f4d8a1c1a5ff7717b2fa71919a405d0eed8b64c8c1d88"
BINARY_SHA256="f8f6765cbb045108d572a40f7280840e5c9df79d520b7f067f74a06e28fda3db"
MANIFEST="plugins/rathole/manifest.json"
STAGE="target/vplugin/rathole"
VERSION="$(jq -r '.version' "$MANIFEST")"
OUTPUT="dist/rathole-$VERSION.vplugin"
HOST="$(rustc -vV | sed -n 's/^host: //p')"
SYSROOT="$(rustc --print sysroot)"
RUST_LLD="$SYSROOT/lib/rustlib/$HOST/bin/rust-lld"

sha256_file() {
	if command -v sha256sum >/dev/null 2>&1; then
		sha256sum "$1" | awk '{print $1}'
	else
		shasum -a 256 "$1" | awk '{print $1}'
	fi
}

[ -f "$ARCHIVE" ] || {
	echo "missing vendored Rathole $RATHOLE_VERSION archive: $ARCHIVE" >&2
	exit 1
}
[ "$(sha256_file "$ARCHIVE")" = "$ARCHIVE_SHA256" ] || {
	echo "vendored Rathole archive checksum mismatch" >&2
	exit 1
}

if ! rustup target list --installed | grep -qx "$TARGET"; then
	echo "missing Rust target: $TARGET" >&2
	exit 1
fi

if [ ! -x "$RUST_LLD" ]; then
	echo "rust-lld was not found at $RUST_LLD" >&2
	exit 1
fi

export CARGO_TARGET_ARMV7_UNKNOWN_LINUX_MUSLEABIHF_LINKER="$RUST_LLD"
cargo build --locked --release --target "$TARGET" --bin venus-rathole

rm -rf "$STAGE"
mkdir -p "$STAGE/bin" "$STAGE/qml" "$STAGE/licenses" dist
cp "$MANIFEST" "$STAGE/manifest.json"
cp "target/$TARGET/release/venus-rathole" "$STAGE/bin/venus-rathole"
cp plugins/rathole/qml/*.qml "$STAGE/qml/"
cp plugins/rathole/licenses/Apache-2.0.txt "$STAGE/licenses/rathole-Apache-2.0.txt"
unzip -p "$ARCHIVE" rathole > "$STAGE/bin/rathole"
chmod 0755 "$STAGE/bin/rathole"

[ "$(sha256_file "$STAGE/bin/rathole")" = "$BINARY_SHA256" ] || {
	echo "vendored Rathole binary checksum mismatch" >&2
	exit 1
}
file "$STAGE/bin/rathole" | grep -Eq 'ELF 32-bit LSB.*ARM.*statically linked' || {
	echo "vendored Rathole binary is not a static 32-bit ARM ELF" >&2
	exit 1
}
file "$STAGE/bin/venus-rathole" | grep -Eq 'ELF 32-bit LSB.*ARM.*statically linked' || {
	echo "Rathole adapter is not a static 32-bit ARM ELF" >&2
	exit 1
}

cargo run --locked --quiet -p venus-plugin-manager -- pack-vplugin "$STAGE" "$OUTPUT"
echo "Built $OUTPUT with Rathole $RATHOLE_VERSION"
