# blip

**B**atch **L**ightframe **I**nspection & **P**review: a minimal, fast FITS blinker for
astro images, hardware-accelerated on the GPU.

![blip on Windows](assets/screenshot-windows.jpg)

## Install

Download the Windows installer, macOS disk image or Linux `.deb`/`.tar.gz` from the
[releases](../../releases).

To build from source you need Rust 1.95 or newer (the MSVC toolchain on Windows).

```sh
cargo build --release
```

Without an argument blip starts empty; open a folder with <kbd>O</kbd> or drop one onto the
window. To try another GPU backend run with: `--backend dx12|vulkan|gl`.

The installers are built with `scripts/package-{windows.ps1,macos.sh,linux.sh}`.

## Keys

| Key | Action |
| --- | --- |
| <kbd>↓</kbd> <kbd>→</kbd> <kbd>Space</kbd> <kbd>J</kbd> | next image |
| <kbd>↑</kbd> <kbd>←</kbd> <kbd>K</kbd> | previous image |
| <kbd>PgDn</kbd> <kbd>PgUp</kbd> / <kbd>Home</kbd> <kbd>End</kbd> | jump 10 / first / last |
| <kbd>Delete</kbd> <kbd>Backspace</kbd> | permanently delete the file, bypassing the recycle bin (asks first) |
| Mouse wheel / drag / double-click | zoom / pan / reset (zoom is kept while blinking) |
| <kbd>F</kbd> <kbd>0</kbd> | reset zoom |
| <kbd>I</kbd> | toggle the info overlay (capture details, star size, clipping stats) |
| <kbd>T</kbd> | file list: show the end of long file names instead of the start |
| <kbd>D</kbd> | toggle the debug panel (timings, stretch values, cache, GPU) |
| <kbd>N</kbd> | toggle background neutralization (auto white balance) |
| <kbd>S</kbd> | show the measured star sizes (HFR) on the current image |
| <kbd>B</kbd> | toggle binned preview / full resolution |
| <kbd>C</kbd> | flash clipped pixels (red = saturated, blue = black) |
| <kbd>F5</kbd> | rescan the folder |
| <kbd>O</kbd> | open a folder (or drag a folder or FITS file onto the window) |
| <kbd>?</kbd> <kbd>F1</kbd> | show all shortcuts |
| <kbd>E</kbd> <kbd>Ctrl</kbd>+<kbd>S</kbd> | export the stretched image (full resolution) as PNG or JPEG |

## License

[MIT](LICENSE). Release packages include the third-party licenses in
`THIRD_PARTY_LICENSES.txt`.
