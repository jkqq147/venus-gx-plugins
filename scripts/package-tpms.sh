#!/bin/sh
set -eu

TARGET="armv7-unknown-linux-musleabihf"
TPMS_REV="dbb46a53808dd09c792acb28b4e7e7ed0e9adf1c"
SOURCE="${TPMS_SOURCE_DIR:-../venus-tpms-ble}"
STAGE="target/vplugin/tpms"
MANIFEST="plugins/tpms/manifest.json"
VERSION="$(jq -r '.version' "$MANIFEST")"
OUTPUT="dist/tpms-$VERSION.vplugin"
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

if [ ! -d "$SOURCE/.git" ]; then
	echo "TPMS source repository was not found at $SOURCE" >&2
	exit 1
fi

if [ "$(git -C "$SOURCE" rev-parse HEAD)" != "$TPMS_REV" ]; then
	echo "TPMS source must be at revision $TPMS_REV" >&2
	exit 1
fi

export CARGO_TARGET_ARMV7_UNKNOWN_LINUX_MUSLEABIHF_LINKER="$RUST_LLD"
cargo build --locked --release --target "$TARGET" --bin venus-tpms-ble

rm -rf "$STAGE"
mkdir -p "$STAGE/bin" "$STAGE/qml" dist
cp "$MANIFEST" "$STAGE/manifest.json"
cp "target/$TARGET/release/venus-tpms-ble" "$STAGE/bin/venus-tpms-ble"
cp "$SOURCE/gui/qml/PageTpms.qml" "$STAGE/qml/PageTpmsSettings.qml"
cp "$SOURCE/gui/qml/PageTpmsBind.qml" "$STAGE/qml/PageTpmsBind.qml"
cp "$SOURCE/gui/qml/PageTpmsDiagnostics.qml" "$STAGE/qml/PageTpmsDiagnostics.qml"
cp "$SOURCE/gui/qml/PageTpmsDiscovered.qml" "$STAGE/qml/PageTpmsDiscovered.qml"
cp "$SOURCE/gui/qml/PageTpmsSensorDetails.qml" "$STAGE/qml/PageTpmsSensorDetails.qml"
cp "$SOURCE/gui/qml/PageTpmsWheel.qml" "$STAGE/qml/PageTpmsWheel.qml"
cp "$SOURCE/gui/qml/OverviewTpms.qml" "$STAGE/qml/OverviewTpms.qml"

# Plugin QML is loaded from the package directory, outside Venus GUI's own QML
# directory. Import the host directory explicitly so types such as MbPage and
# OverviewPage remain resolvable on Venus OS v3.55.
for qml in "$STAGE/qml/"*.qml; do
	tmp="$qml.tmp"
	awk 'NR == 3 { print "import \"/opt/victronenergy/gui/qml\"" } { print }' "$qml" > "$tmp"
	mv "$tmp" "$qml"
done

cargo run --locked --quiet -p venus-plugin-manager -- pack-vplugin "$STAGE" "$OUTPUT"
echo "Built $OUTPUT"
