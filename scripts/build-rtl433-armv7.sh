#!/bin/sh
set -eu

ROOT="$(unset CDPATH; cd -- "$(dirname "$0")/.." && pwd)"
RTL433_COMMIT="ea7d504877df751a202432d47dbb0c425ab0a93c"
RTLSDR_COMMIT="84195f169f5b4b7dc06a10efb1e210d02b49e51c"
LIBUSB_VERSION="1.0.29"
EXPECTED_SHA256="57afd45f1528ef9b786bd4e5a4be7143dab50072fd66e11af0c9eb52f63fd108"
OUTPUT="${1:-$ROOT/target/rtl433-armv7/rtl_433}"
WORK="$(mktemp -d "${TMPDIR:-/tmp}/venus-rtl433.XXXXXX")"

cleanup() {
	rm -rf "$WORK"
}
trap cleanup EXIT HUP INT TERM

sha256_file() {
	if command -v sha256sum >/dev/null 2>&1; then
		sha256sum "$1" | awk '{print $1}'
	else
		shasum -a 256 "$1" | awk '{print $1}'
	fi
}

for command in curl git cmake ninja make patch tar zig; do
	command -v "$command" >/dev/null 2>&1 || {
		echo "missing build dependency: $command" >&2
		exit 1
	}
done

LLVM_STRIP="${LLVM_STRIP:-}"
if [ -z "$LLVM_STRIP" ]; then
	LLVM_STRIP="$(command -v llvm-strip 2>/dev/null || true)"
fi
if [ -z "$LLVM_STRIP" ] && [ -x /opt/homebrew/opt/llvm/bin/llvm-strip ]; then
	LLVM_STRIP="/opt/homebrew/opt/llvm/bin/llvm-strip"
fi
[ -x "$LLVM_STRIP" ] || {
	echo "missing llvm-strip; set LLVM_STRIP to an LLVM strip executable" >&2
	exit 1
}

JOBS="$(getconf _NPROCESSORS_ONLN 2>/dev/null || echo 4)"
PREFIX="$WORK/prefix"
CFLAGS="-target arm-linux-musleabihf -mcpu=cortex_a8 -Os"

curl -fsSL \
	"https://github.com/libusb/libusb/releases/download/v${LIBUSB_VERSION}/libusb-${LIBUSB_VERSION}.tar.bz2" \
	-o "$WORK/libusb.tar.bz2"
tar -xjf "$WORK/libusb.tar.bz2" -C "$WORK"
mkdir -p "$WORK/build-libusb"
(
	cd "$WORK/build-libusb"
	CC="zig cc $CFLAGS" AR="zig ar" RANLIB="zig ranlib" STRIP="zig strip" \
		"$WORK/libusb-${LIBUSB_VERSION}/configure" \
		--host=arm-linux-musleabihf \
		--prefix="$PREFIX" \
		--disable-shared \
		--enable-static \
		--disable-udev \
		--disable-examples-build \
		--disable-tests-build
	make -j"$JOBS"
	make install
)

git clone --quiet https://github.com/osmocom/rtl-sdr.git "$WORK/rtl-sdr"
git -C "$WORK/rtl-sdr" checkout --quiet "$RTLSDR_COMMIT"
cmake -S "$WORK/rtl-sdr" -B "$WORK/build-rtlsdr" -G Ninja \
	-DCMAKE_SYSTEM_NAME=Linux \
	-DCMAKE_SYSTEM_PROCESSOR=arm \
	-DCMAKE_BUILD_TYPE=Release \
	-DCMAKE_C_COMPILER=zig \
	-DCMAKE_C_COMPILER_ARG1=cc \
	-DCMAKE_C_FLAGS="$CFLAGS" \
	-DCMAKE_EXE_LINKER_FLAGS=-static \
	-DBUILD_SHARED_LIBS=OFF \
	-DDETACH_KERNEL_DRIVER=ON \
	-DINSTALL_UDEV_RULES=OFF \
	-DLIBUSB_INCLUDE_DIRS="$PREFIX/include/libusb-1.0" \
	-DLIBUSB_LIBRARIES="$PREFIX/lib/libusb-1.0.a"
cmake --build "$WORK/build-rtlsdr" --target rtlsdr_static

git clone --quiet https://github.com/merbanan/rtl_433.git "$WORK/rtl_433"
git -C "$WORK/rtl_433" checkout --quiet "$RTL433_COMMIT"
patch -d "$WORK/rtl_433" -p1 < "$ROOT/plugins/tpms/patches/rtl433-low-memory.patch"
cmake -S "$WORK/rtl_433" -B "$WORK/build-rtl433" -G Ninja \
	-DCMAKE_SYSTEM_NAME=Linux \
	-DCMAKE_SYSTEM_PROCESSOR=arm \
	-DCMAKE_BUILD_TYPE=MinSizeRel \
	-DCMAKE_C_COMPILER=zig \
	-DCMAKE_C_COMPILER_ARG1=cc \
	-DCMAKE_C_FLAGS="$CFLAGS" \
	-DCMAKE_EXE_LINKER_FLAGS=-static \
	-DCMAKE_LINK_DEPENDS_USE_LINKER=FALSE \
	-DENABLE_OPENSSL=OFF \
	-DENABLE_RTLSDR=ON \
	-DENABLE_SOAPYSDR=OFF \
	-DLibRTLSDR_INCLUDE_DIR="$WORK/rtl-sdr/include" \
	-DLibRTLSDR_LIBRARY="$WORK/build-rtlsdr/src/librtlsdr.a" \
	-DLibUSB_INCLUDE_DIR="$PREFIX/include/libusb-1.0" \
	-DLibUSB_LIBRARY="$PREFIX/lib/libusb-1.0.a"
cmake --build "$WORK/build-rtl433" --target rtl_433

mkdir -p "$(dirname "$OUTPUT")"
"$LLVM_STRIP" "$WORK/build-rtl433/src/rtl_433"
cp "$WORK/build-rtl433/src/rtl_433" "$OUTPUT"
file "$OUTPUT" | grep -Eq 'ELF 32-bit LSB.*ARM.*statically linked' || {
	echo "rtl_433 output is not a static 32-bit ARM ELF" >&2
	exit 1
}
ACTUAL_SHA256="$(sha256_file "$OUTPUT")"
echo "Built $OUTPUT"
echo "SHA-256: $ACTUAL_SHA256"
if [ "$ACTUAL_SHA256" != "$EXPECTED_SHA256" ]; then
	echo "Note: binary hash differs from the reviewed artifact; toolchain metadata may differ." >&2
fi
