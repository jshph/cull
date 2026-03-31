# cull

Blazing-fast photo culling for photographers. Opens a folder, shows instant previews, lets you pick or reject images, writes XMP sidecars that Lightroom and Capture One read on import.

## The one problem it solves

Photo Mechanic's speed trick is just showing the JPEG preview embedded in every RAW file — no decoding needed. `cull` does the same, adds a clean UI, and costs $0.

## Keyboard shortcuts

| Key | Action |
|---|---|
| `←` / `→` | Previous / next image |
| `↑` / `↓` | Previous / next row (grid mode) |
| `P` or `Space` | Toggle pick |
| `X` | Toggle reject |
| `U` | Unmark |
| `R` | Rotate 90° CCW |
| `Shift+R` | Rotate 90° CW |
| `Shift+←→` | Extend selection |
| `Cmd+click` | Toggle individual selection |
| `Cmd+B` | Toggle file explorer |
| `Cmd+E` | Export picks to `_picks/` subfolder |

## Build

```
cargo build --release
./target/release/cull
```

Or install globally: `cargo install --path .`

Open from terminal: `cull` (opens CWD), `cull ~/Photos/shoot`, or drag a folder onto the window.

## Supported formats

CR2, CR3, NEF, ARW, DNG, ORF, RAF, RW2, PEF, SRW, JPEG

All RAW formats embed a full-resolution JPEG preview. `cull` extracts and displays that preview with zero RAW decoding — same as Photo Mechanic.

## Lightroom / Capture One handoff

XMP sidecars are written to the same folder as your RAWs the moment you hit P or X.

| Cull action | XMP written | Lightroom reads as | Capture One reads as |
|---|---|---|---|
| Pick (P) | `xmp:Label = "Green"` | Green color label | Green color tag |
| Reject (X) | `xmp:Rating = -1`, `xmp:Label = "Red"` | Reject flag (X) + Red label | Red color tag |
| Unmark (U) | `xmp:Rating = 0`, no label | Unflagged, no label | No tag |
| Rotate (R) | `tiff:Orientation` | Correct rotation on import | Correct rotation |

**Why this mapping:**
- Star ratings (1-5) are left free for your own grading within picks. Cull doesn't touch them.
- Lightroom's native Reject flag is `Rating = -1` in XMP — cull writes this directly so rejects show up as rejected in LR, not just "1 star."
- Lightroom's Pick flag has no XMP representation (it's catalog-only). Green label is the standard visual proxy that every photographer recognizes.

**Workflow:**
1. Cull your folder in `cull` — press P for keepers, X for rejects
2. Import the folder into Lightroom (Include Subfolders if you exported to `_picks/`)
3. Picks appear with green labels. Rejects are already flagged. Stars are at 0, ready for your grading pass.

## CLI

```
cull                        # Open GUI with current directory
cull ~/Photos/wedding       # Open GUI with specific folder
cull stats ~/Photos/wedding # Show pick/reject/unrated counts
cull picks ~/Photos/wedding # List picked files (one per line)
cull export ~/Photos/wedding # Copy picks to _picks/
cull mark IMG_001.RAF pick  # Mark a single file
```
