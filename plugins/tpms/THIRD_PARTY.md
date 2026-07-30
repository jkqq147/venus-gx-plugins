# TPMS third-party software

The TPMS package contains a statically linked `rtl_433` companion executable for ARMv7. It is kept as a separate process from the Rust plugin adapter.

| Component | Version | Source | License |
| --- | --- | --- | --- |
| rtl_433 | 25.12, commit `ea7d504877df751a202432d47dbb0c425ab0a93c` | <https://github.com/merbanan/rtl_433> | GPL-2.0 |
| rtl-sdr | commit `84195f169f5b4b7dc06a10efb1e210d02b49e51c` | <https://github.com/osmocom/rtl-sdr> | GPL-2.0 |
| libusb | 1.0.29 | <https://github.com/libusb/libusb/releases/tag/v1.0.29> | LGPL-2.1-or-later |

The corresponding license texts are in `plugins/tpms/licenses/` and are included in every `.vplugin` package.

The only rtl_433 source modification is `plugins/tpms/patches/rtl433-low-memory.patch`. It lowers the maximum sample-buffer allocation from 4 MiB to 64 KiB so the process can run safely on a memory-constrained CCGX. The plugin also starts rtl_433 with a 32 KiB runtime buffer.

Run `scripts/build-rtl433-armv7.sh` to download the exact sources, apply the patch, and reproduce the static ARMv7 executable. The vendored binary has SHA-256:

```text
57afd45f1528ef9b786bd4e5a4be7143dab50072fd66e11af0c9eb52f63fd108  rtl_433
```
