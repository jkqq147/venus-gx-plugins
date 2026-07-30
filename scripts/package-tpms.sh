#!/bin/sh
set -eu

TARGET="armv7-unknown-linux-musleabihf"
RTL433_VERSION="25.12"
ARCHIVE="plugins/tpms/vendor/rtl433-25.12-armv7-static.zip"
ARCHIVE_SHA256="955199bd371077107cbc481d19ff1557a21282a454131068d16e6def2a2b429b"
RTL433_SHA256="57afd45f1528ef9b786bd4e5a4be7143dab50072fd66e11af0c9eb52f63fd108"
STAGE="target/vplugin/tpms"
MANIFEST="plugins/tpms/manifest.json"
VERSION="$(jq -r '.version' "$MANIFEST")"
OUTPUT="dist/tpms-$VERSION.vplugin"
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
	echo "missing vendored rtl_433 $RTL433_VERSION archive: $ARCHIVE" >&2
	exit 1
}
[ "$(sha256_file "$ARCHIVE")" = "$ARCHIVE_SHA256" ] || {
	echo "vendored rtl_433 archive checksum mismatch" >&2
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
cargo build --locked --release --target "$TARGET" --bin venus-tpms

rm -rf "$STAGE"
mkdir -p "$STAGE/bin" "$STAGE/qml" "$STAGE/licenses" dist
cp "$MANIFEST" "$STAGE/manifest.json"
cp "target/$TARGET/release/venus-tpms" "$STAGE/bin/venus-tpms"
cp plugins/tpms/qml/*.qml "$STAGE/qml/"
cp plugins/tpms/licenses/*.txt "$STAGE/licenses/"
unzip -p "$ARCHIVE" rtl_433 > "$STAGE/bin/rtl_433"
chmod 0755 "$STAGE/bin/rtl_433"

[ "$(sha256_file "$STAGE/bin/rtl_433")" = "$RTL433_SHA256" ] || {
	echo "vendored rtl_433 binary checksum mismatch" >&2
	exit 1
}
file "$STAGE/bin/rtl_433" | grep -Eq 'ELF 32-bit LSB.*ARM.*statically linked' || {
	echo "vendored rtl_433 binary is not a static 32-bit ARM ELF" >&2
	exit 1
}
file "$STAGE/bin/venus-tpms" | grep -Eq 'ELF 32-bit LSB.*ARM.*statically linked' || {
	echo "TPMS adapter is not a static 32-bit ARM ELF" >&2
	exit 1
}

cargo run --locked --quiet -p venus-plugin-manager -- pack-vplugin "$STAGE" "$OUTPUT"
echo "Built $OUTPUT with rtl_433 $RTL433_VERSION"
