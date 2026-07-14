#!/bin/sh
set -eu

TARGET="armv7-unknown-linux-musleabihf"
STAGE="target/vplugin/loxone-tanks"
MANIFEST="plugins/loxone-tanks/manifest.json"
VERSION="$(jq -r '.version' "$MANIFEST")"
OUTPUT="dist/loxone-tanks-$VERSION.vplugin"
HOST="$(rustc -vV | sed -n 's/^host: //p')"
SYSROOT="$(rustc --print sysroot)"
RUST_LLD="$SYSROOT/lib/rustlib/$HOST/bin/rust-lld"

if ! rustup target list --installed | grep -qx "$TARGET"; then
	echo "missing Rust target: $TARGET" >&2
	exit 1
fi

if [ ! -x "$RUST_LLD" ]; then
	echo "rust-lld was not found at $RUST_LLD" >&2
	exit 1
fi

export CARGO_TARGET_ARMV7_UNKNOWN_LINUX_MUSLEABIHF_LINKER="$RUST_LLD"
cargo build --locked --release --target "$TARGET" --bin venus-loxone-tanks

rm -rf "$STAGE"
mkdir -p "$STAGE/bin" "$STAGE/qml" dist
cp "$MANIFEST" "$STAGE/manifest.json"
cp "target/$TARGET/release/venus-loxone-tanks" "$STAGE/bin/venus-loxone-tanks"
cp plugins/loxone-tanks/qml/*.qml "$STAGE/qml/"

file "$STAGE/bin/venus-loxone-tanks" | grep -Eq 'ELF 32-bit LSB.*ARM.*statically linked' || {
	echo "Loxone Tanks binary is not a static 32-bit ARM ELF" >&2
	exit 1
}

cargo run --locked --quiet -p venus-plugin-manager -- pack-vplugin "$STAGE" "$OUTPUT"
echo "Built $OUTPUT"
