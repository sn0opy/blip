# blip

**B**atch **L**ightframe **I**nspection & **P**review: a minimal, fast FITS and XISF
blinker for astro images, hardware-accelerated on the GPU.

![blip on Windows](assets/screenshot-windows.jpg)

## Install

Download the Windows installer (or the portable `.zip`, which needs no setup), the macOS disk
image, or the Linux Flatpak (x86_64 or aarch64) or `.tar.gz` from the
[releases](../../releases). Install the Flatpak with
`flatpak install --user blip-<version>-linux-<arch>.flatpak`.

To build from source you need Rust 1.95 or newer (the MSVC toolchain on Windows).

```sh
cargo build --release
```

## Keys

| Key | Action |
| --- | --- |
| <kbd>↓</kbd>, <kbd>→</kbd>, <kbd>Space</kbd> or <kbd>J</kbd> | Next image |
| <kbd>↑</kbd>, <kbd>←</kbd> or <kbd>K</kbd> | Previous image |
| <kbd>PgDn</kbd> / <kbd>PgUp</kbd> | Jump 10 images |
| <kbd>Home</kbd> / <kbd>End</kbd> | First / last image |
| <kbd>Delete</kbd> or <kbd>Backspace</kbd> | Delete file permanently (asks first) |
| Wheel / drag | Zoom / pan (kept while blinking) |
| Double-click, <kbd>F</kbd> or <kbd>0</kbd> | Reset zoom |
| <kbd>A</kbd> | Toggle auto stretch for this image |
| <kbd>C</kbd> | Toggle flashing clipped pixels on this image |
| <kbd>S</kbd> | Toggle star sizes (HFR) on this image |
| <kbd>T</kbd> | Toggle sensor tilt (corner HFR) on this image |
| <kbd>P</kbd> | Toggle plate solve annotation on this image |
| <kbd>G</kbd> | Toggle corner mosaic (aberration inspection) |
| <kbd>B</kbd> | Toggle binned preview / full resolution |
| <kbd>N</kbd> | Toggle background neutralization (white balance) |
| <kbd>I</kbd> | Toggle info overlay |
| <kbd>D</kbd> | Toggle debug info (timings, stretch, cache, GPU) |
| <kbd>E</kbd> or <kbd>Ctrl</kbd>+<kbd>S</kbd> | Export as PNG / JPEG (full resolution) |
| <kbd>O</kbd> or <kbd>Ctrl</kbd>+<kbd>O</kbd> | Open folder |
| <kbd>F5</kbd> or <kbd>Ctrl</kbd>+<kbd>R</kbd> | Rescan folder |
| <kbd>?</kbd> or <kbd>F1</kbd> | This help |

On macOS, use <kbd>Cmd</kbd> instead of <kbd>Ctrl</kbd>. A folder or FITS / XISF file can also be
dropped onto the window to open it.

## License

[MIT](LICENSE). Release packages include the third-party licenses in
`THIRD_PARTY_LICENSES.txt`.
