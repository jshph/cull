<p align="center">
  <img src="assets/icon.png" width="128" height="128" alt="cull icon" />
</p>

<h1 align="center">cull</h1>

<p align="center">
  <strong>The fastest way to pick your keepers.</strong><br />
  Browse thousands of RAW photos in under a second. Pick, reject, hand off to Lightroom or Capture One.<br />
  A small native Rust app.
</p>

<p align="center">
  <a href="https://www.getcull.fyi">Website</a> &middot;
  <a href="#keyboard-shortcuts">Shortcuts</a> &middot;
  <a href="#lightroom-and-capture-one-integration">LR / C1 Integration</a>
</p>

---

## Why cull?

Every RAW file already contains a full-resolution JPEG preview baked in by your camera. Most software ignores this and decodes the entire RAW file. `cull` extracts these embedded previews directly — no decoding, no rendering pipeline, no waiting.

**No catalog. No import. No subscription.** Point at a folder and go.

### What makes it fast

- **Instant RAW preview** — extracts embedded JPEG previews, no RAW decoding needed
- **Sub-second folder loading** — browse thousands of images the moment you open a folder
- **Small native binary** — smaller than a single RAW file
- **Native Rust + GPU rendering** — not Electron, not a web wrapper

### Professional culling workflow

- **Pick / Reject / Unmark** — keyboard-driven (`P`, `X`, `U`) for rapid image selection
- **Preserved metadata** — surgical XMP updates keep independent stars and existing adjustments
- **Export picks** — create a fresh, dated snapshot in `Exports/`
- **Open shoot** — reuse native decision views in Capture One and Lightroom Classic, or browse Lightroom Local with native flags
- **Grid and loupe views** — filmstrip grid for overview, full-resolution loupe for detail
- **EXIF at a glance** — camera body, lens, focal length, aperture, shutter speed, ISO
- **Camera and lens filters** — filter by body or lens to compare setups

## Supported RAW formats

| Format | Camera |
|---|---|
| CR2, CR3 | Canon |
| NEF | Nikon |
| ARW | Sony |
| RAF | Fujifilm |
| DNG | Adobe, Leica, others |
| ORF | Olympus / OM System |
| RW2 | Panasonic |
| PEF | Pentax |
| SRW | Samsung |
| JPEG | Universal |

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
| `Cmd+E` | Open / refresh shoot in the selected editor |
| `Cmd+Shift+E` | Export a fresh picks snapshot to `Exports/` |

## Lightroom and Capture One integration

Cull keeps one pick / reject / unmarked decision and preserves independent **0–5 star ratings**. Marking writes an XMP sidecar atomically, retaining unknown properties and adjustment metadata. Original image bytes are never rewritten by Cull.

| Cull | XMP decision | Capture One | Lightroom Classic plugin |
|---|---|---|---|
| Pick (`P`) | `xmpDM:good = True`, Green label | Green tag; Picks smart album | Native Pick flag; Picks smart collection |
| Reject (`X`) | `xmpDM:good = False`, Red label | Red tag; Rejects smart album | Native Reject flag; Rejects smart collection |
| Unmark (`U`) | Remove `xmpDM:good` | Restored prior non-decision tag; Unmarked album | Unflagged; restored prior label; Unmarked collection |

Green and red are reserved for decisions. Cull remembers a previous custom label and restores it on unmark. Legacy `Rating=-1` rejects are migrated; old ratings that an earlier Cull version already erased cannot be recovered.

### Shoot organization

**Shoot → New shoot…** creates:

```text
Shoot/
  Originals/                  # Stable source files; subfolders supported
  Exports/
    Picks-<date>-<unique-id>/  # Each delivery is a fresh snapshot
  .cull/
    shoot.json                # Persistent shoot identity
    handoff.cull              # Lightroom handoff data
    Capture One/              # Dedicated native session
```

Copy your photos into `Originals/`, then open the shoot in Cull. Existing folders can be opened directly: the first handoff or export adopts them **in place**, adding `.cull/` and `Exports/` without moving originals. Open a specific shoot, rather than your entire Pictures library. Managed state, exports, old `_picks/`, and Capture One caches are excluded from source scanning.

**Cmd+E / Open shoot** always hands off the shoot and its saved decisions. It does not narrow the shoot to a temporary multi-selection. **Folder → Open whole folder** browses the source folder directly (Capture One needs an open session; Open shoot creates one). **Cmd+Shift+E** copies exactly the current picks, their original bytes and XMP, preserving subfolders. Earlier snapshots remain unchanged; failed copies do not publish a completed snapshot. **Folder → Open exported picks folder** opens the most recent successful snapshot.

### Editor setup

Cull detects installed editors and bundles both adapters. Choose your editor, open a photo folder, and press **Cmd+E**.

| Editor | Setup you do | Cull handles automatically |
|---|---|---|
| Capture One | Allow macOS control permission when prompted | Creates/reopens the session, favorites and decision albums; updates tags |
| Lightroom Local | Select Local before confirming the handoff; enable Include subfolders if needed | Opens the folder, supplies native flags/colors, and offers a graceful restart to refresh cached metadata |
| Lightroom Classic | Enable Cull Shoot Handoff in Plug-in Manager once | Installs the plugin, imports originals in place, and creates/refreshes decision collections |

No XMP auto-sync preference changes or manual album creation are required. Capture One 22 and Lightroom Local 9.5.1 were tested on copied Ricoh DNGs. Classic's plugin has automated contract coverage but has not yet been tested in a live Classic installation.

### Capture One

Cull creates or reopens a dedicated session beneath `.cull/Capture One/`, adds source folders as session favorites, and creates **Picks**, **Rejects**, and **Unmarked** smart albums. The session browses originals in place. Repeating the handoff updates tags without duplicating its albums or reloading metadata over existing star ratings and edits. Allow Cull to control Capture One if macOS prompts.

Smart albums react immediately to color-tag changes inside Capture One. Like other session smart albums, they search session folders, albums, and favorites: adding unrelated favorites or tagged processed files to that session expands its scope. Cull's export snapshots live in subfolders of `Exports/`.

### Lightroom (Local)

Choose **Lightroom (Local)** in Cull's editor picker and use **Open shoot**. Cull brings Lightroom forward and asks you to select **Local** at the top left before continuing. After you select Local, return to Cull's dialog and choose **Open Local folder**. This mode check is required: Adobe routes the same macOS folder-open event to a cloud import dialog when Cloud is active. Cull does not submit that import. Use Lightroom's flag filters for Picks, Rejects, and Unflagged; enable **Include subfolders** for nested source folders. This route uses native XMP flags and colors, with no plugin or catalog collections.

Lightroom 9.5.1 caches metadata for already-viewed files. Reopening the folder alone did not refresh changed flags in live testing. After finishing pending Lightroom edits, use **Shoot → Restart Lightroom & refresh shoot** to gracefully quit and reopen Lightroom with the current decisions. Cull never force-quits it. CLI: `cull handoff <shoot> --editor <Lightroom.app> --restart-lightroom`.

Verified with Lightroom 9.5.1: Adobe serializes Pick / Reject as `xmpDM:good=True/False` and removes that property for Unflagged. Older Cull-authored sidecars using `xmpDM:pick` are migrated during this handoff. Lightroom may embed metadata into DNG/JPEG files and remove their sidecars when you edit in Local mode; Cull itself only writes sidecars. Keep backups before editing originals in either editor.

### Lightroom Classic

Cull installs its bundled plugin under `~/Library/Application Support/Adobe/Lightroom/Modules/Cull.lrplugin`. On first use, enable **Cull Shoot Handoff** in **File → Plug-in Manager** (restart Classic if necessary), then retry **Open shoot**. Alternatively use **File → Plug-in Extras → Open / Refresh Cull Shoot…**, choosing the shoot's `.cull/handoff.cull` file; press Cmd+Shift+G in the picker to enter the hidden path.

The plugin adds missing originals to the active catalog **in place** and reuses a collection set identified by the shoot ID. It maintains an Originals collection and scoped **Picks / Rejects / Unmarked** smart collections, applying native flags and labels while preserving existing catalog stars and edits. Lightroom confirms completion; Cull's “Sent shoot” message means only that the request was delivered.

This plugin targets **Lightroom Classic**; the cloud edition uses the Local workflow above. Native flag application also works for DNG/JPEG, whose embedded metadata behavior makes sidecar-only import unreliable. The plugin never invokes Save Metadata to File. New catalog photos use Lightroom's normal metadata import behavior; existing catalog photos retain their grades.

### Refresh behavior and limits

Cull → editor is an **explicit handoff**, not a background two-way synchronization service. Reopening a shoot reapplies saved Cull decisions; it does not automatically import unsaved catalog decisions back into Cull. For sidecar-based workflows, metadata saved by the editor can be reread by reopening the folder in Cull. If flags and color tags conflict, a changed native flag takes precedence. DNG/JPEG edits saved inside an editor's catalog or embedded file do not supersede an existing Cull sidecar automatically.

Rotations and keywords are preserved in XMP; a repeat handoff updates **decisions only**, avoiding a destructive reload over editor adjustments. Malformed metadata produces an error instead of being replaced. RAW/JPEG pairs with a shared basename also share an XMP sidecar and a Cull decision.

## Get cull

**[Download](https://www.getcull.fyi)** a pre-built binary from the website, or build from source:

```
cargo build --release
./target/release/cull
```

## CLI

```
cull                        # Open empty (pick a folder in the UI)
cull ~/Pictures/wedding     # Open a shoot or existing folder
cull stats ~/Pictures/wedding # Show pick/reject/unmarked counts
cull picks ~/Pictures/wedding # List picked files
cull export ~/Pictures/wedding # Fresh snapshot under Exports/
cull mark IMG_001.RAF pick  # Mark a single file
cull new-shoot ~/Pictures/NewShoot
cull handoff ~/Pictures/wedding --editor "/Applications/Capture One 22.app"
cull open-folder ~/Pictures/wedding/Originals --editor "/Applications/Capture One 22.app"
cull install-lightroom-plugin
```

When installed from the .app bundle, `cull` is automatically added to your PATH.

## How it works

`cull` extracts embedded JPEG previews from RAW files on background threads — the same previews your camera bakes into every file. EXIF metadata is parsed asynchronously. XMP is parsed by namespace and patched at property boundaries, then atomically replaced.

Built with [egui](https://github.com/emilk/egui). Editor adapters and the Lightroom plugin are bundled into the executable. Capture One integration uses its native macOS scripting interface.

## Verification

```sh
cargo test
CULL_TEST_PHOTO=/path/to/a/real.DNG cargo test -- --ignored
uv run scripts/test-lightroom.py
bash scripts/bundle.sh
```

The Lua contract suite checks import idempotence, native flag mapping, scoped smart collections, custom-label restoration, and preservation of existing grades. It does not replace live Lightroom Classic testing.

## License

MIT

### Release packaging

`bash scripts/bundle.sh` builds the native architecture's app and DMG. Set `CULL_SIGNING_IDENTITY` to a Developer ID Application identity for signed distribution. Set `CULL_NOTARY_PROFILE` to an existing notarytool keychain profile to submit, staple, and validate the DMG automatically. Notarization requires signing. The script also writes `dist/SHA256SUMS`.
