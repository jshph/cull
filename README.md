# cull

Blazing-fast photo culling for photographers. Opens a folder, shows instant previews, lets you pick or reject images, writes XMP sidecars Lightroom reads on import. Nothing else.

## The one problem it solves

Photo Mechanic's speed trick is just showing the JPEG preview embedded in every RAW file — no decoding needed. `cull` does the same, adds a clean UI, and costs $0.

After culling, import your folder into Lightroom. Your picks (5-star, green label) and rejects (1-star, red label) are already there via the XMP sidecars.

## Keyboard shortcuts

| Key | Action |
|---|---|
| `←` / `→` | Previous / next image |
| `P` or `Space` | Toggle pick |
| `X` | Toggle reject |
| `U` | Unmark |
| `⌘E` / `Ctrl+E` | Export picks to `_picks/` subfolder |

Filter tabs at the top let you view All / Picks / Unrated.

## Build

```
cargo build --release
./target/release/cull
```

Drag a folder of RAW files onto the window, or click **Open Folder**.

## Supported formats

CR2, CR3, NEF, ARW, DNG, ORF, RAF, RW2, PEF, SRW, JPEG

All RAW formats embed a full-resolution JPEG preview. `cull` extracts and displays that preview with zero RAW decoding — same as Photo Mechanic.

## Lightroom handoff

XMP sidecars are written to the same folder as your RAWs the moment you hit P or X. When you import that folder into Lightroom, ratings and color labels come with them automatically. No plugin required.
