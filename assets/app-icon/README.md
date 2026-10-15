# LayerShop app icon

**Three stacked sheets on a dark tile**: layers, seen from above at an angle. The top sheet carries
the ember gradient of the LayerShop website; the two below it are plain greys. It is the mark of
the website (`layershop` logo and favicon) on a tile, so the app, its window and the site match.

This fork ships as LayerShop, so this folder holds LayerShop's icon in place of upstream
PhotoCraft's kitsune. The file names are unchanged (`photocraft.*`, `ai.storyteller.photocraft.*`):
they are machine names, and keeping them means the build, the tests and the packaging scripts need
no edits.

## Palette

| Colour | Hex | Used for |
|---|---|---|
| Tile | `#1b1f26` | the rounded tile, with a `#3a414d` edge |
| Ember gradient | `#ffc24d` → `#ff8a3d` → `#ff5d6c` | the top sheet, left to right |
| Light grey | `#cbcfd6` | the middle sheet |
| Mid grey | `#7c828d` | the bottom sheet |

## Geometry

A 512-unit tile (`viewBox="0 0 512 512"`), rounded square with `rx=105`. The mark is centred and
70% of the tile wide (80% at 24 px and below, so it reads at 16 px). The macOS render pads the
tile onto Apple's 824/1024 icon grid; the Windows and Linux renders fill the canvas.

## Files

- `photocraft.svg`, `photocraft-small.svg`: the master (the same drawing; upstream kept two traces).
- `photocraft-1024.png`: 1024 px render on the macOS grid.
- `photocraft.icns`: macOS bundle icon (`CFBundleIconFile`).
- `photocraft.ico`: Windows icon, 16–256 px, embedded in the `.exe` by `apps/photocraft/build.rs`.
- `hicolor/<size>/apps/ai.storyteller.photocraft.png` (16–512) and `hicolor/scalable/...svg`:
  Linux icon theme. The 128 px one is also the mark at the left of the title bar
  (`crates/ui-egui/src/brand.rs`), and the 256 px one the window icon
  (`apps/photocraft/src/app_icon.rs`).

On Windows the taskbar shows the icon of the Start Menu shortcut that launches the exe, so the
MSI's `<Icon Id>` keeps the `.exe` extension (ICE50); `app_icon.rs` documents the details and tests
the `.ico` sizes and the WiX icon references.

## Regenerate

The drawing is code, not a traced picture: `tools/make-app-icon.py` in the LayerShop website
project writes the SVGs, the PNGs and the `.icns` from one set of numbers (it needs Pillow), then
prints the `cargo xtask ico` command that packs the `.ico`. `packaging/icons.sh` (upstream's
script, which renders the SVG with `resvg`) works too, but crops the tile for the small sizes as
the kitsune needed; prefer the Python script for this icon.

License: see `LICENSE.txt`.
