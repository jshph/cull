# cull

The fastest photo culling software for professional photographers. Browse thousands of RAW images instantly, pick your keepers, reject the rest, and hand off to Lightroom or Capture One — all in a 4.5 MB native app.

## Why cull?

Every RAW file already contains a full-resolution JPEG preview. Instead of decoding massive RAW files, `cull` extracts and displays these embedded previews — giving you instant image loading with zero wait time. Open a folder of 5,000 CR3s and start culling immediately.

**No catalog. No import. No waiting.** Just point `cull` at a folder and go.

### What makes it fast

- **Instant RAW preview** — extracts embedded JPEG previews from RAW files, no decoding needed
- **Sub-second folder loading** — browse thousands of images the moment you open a folder
- **4.5 MB binary** — a single native executable, smaller than a single RAW file
- **Native performance** — built in Rust with GPU-accelerated rendering, not Electron

### Professional culling workflow

- **Pick / Reject / Unmark** — keyboard-driven workflow (`P`, `X`, `U`) for rapid image selection
- **XMP sidecar output** — writes industry-standard XMP files that Lightroom and Capture One read on import
- **Export picks** — copy your selections to a `_picks/` subfolder with one shortcut
- **Send to editor** — open images directly in Lightroom, Capture One, or any external editor
- **Grid and loupe views** — filmstrip grid for overview, full-resolution loupe for detail
- **EXIF data** — camera body, lens, focal length, aperture, shutter speed, ISO at a glance
- **Camera and lens filters** — filter your shoot by body or lens to compare setups

## Supported RAW formats

CR2, CR3, NEF, ARW, DNG, ORF, RAF, RW2, PEF, SRW, JPEG

Works with Canon, Nikon, Sony, Fujifilm, Olympus/OM System, Panasonic, Pentax, and Samsung RAW files.

## Keyboard shortcuts

| Key | Action |
|---|---|
| `Left` / `Right` | Previous / next image |
| `Up` / `Down` | Previous / next row (grid mode) |
| `P` or `Space` | Pick (mark as keeper) |
| `X` | Reject |
| `U` | Unmark (clear pick/reject) |
| `R` | Rotate 90 CCW |
| `Shift+R` | Rotate 90 CW |
| `Shift+Arrow` | Extend selection |
| `Cmd+click` | Toggle individual selection |
| `Cmd+B` | Toggle file browser |
| `Cmd+E` | Open in external editor |
| `Cmd+Shift+E` | Export picks to `_picks/` |

## Lightroom and Capture One integration

XMP sidecars are written the instant you pick or reject. Import your folder and everything is already tagged.

| Action | XMP written | Lightroom | Capture One |
|---|---|---|---|
| Pick (`P`) | `xmp:Label = "Green"` | Green color label | Green color tag |
| Reject (`X`) | `xmp:Rating = -1`, `xmp:Label = "Red"` | Reject flag + Red label | Red color tag |
| Unmark (`U`) | `xmp:Rating = 0`, no label | Unflagged, no label | No tag |
| Rotate (`R`) | `tiff:Orientation` | Correct rotation on import | Correct rotation |

**Design decisions:**
- Star ratings (1-5) are untouched — they're reserved for your own grading within picks
- Lightroom's native Reject flag is `Rating = -1` in XMP, so rejects show up correctly in LR
- Green label is the standard visual proxy for picks since Lightroom's Pick flag is catalog-only

**Workflow:**
1. Open your folder in `cull` — press `P` for keepers, `X` for rejects
2. Import the folder into Lightroom or Capture One
3. Picks appear with green labels. Rejects are flagged. Stars are at 0, ready for grading.

## Get cull

**Buy a license** at [getcull.fyi](https://getcull.fyi) — $14.99 early bird, $29.99 after.

Or build from source (open source, MIT):

```
cargo build --release
./target/release/cull
```

## Usage

```
cull                        # Open current directory
cull ~/Photos/wedding       # Open a specific folder
cull stats ~/Photos/wedding # Show pick/reject/unrated counts
cull picks ~/Photos/wedding # List picked files
cull export ~/Photos/wedding # Copy picks to _picks/
cull mark IMG_001.RAF pick  # Mark a single file
```

## How it works

`cull` is ~3,000 lines of Rust. It uses [egui](https://github.com/emilk/egui) for the UI and extracts embedded JPEG previews from RAW files on background threads. EXIF metadata is parsed asynchronously. XMP sidecars are written as simple XML — no library dependencies for the format.

The entire application compiles to a single 4.5 MB binary with no runtime dependencies.
