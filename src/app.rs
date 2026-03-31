use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::{mpsc, Arc, Condvar, Mutex};

use egui::{
    Align, Color32, Context, FontId, Key, Rect, ScrollArea, Sense, Stroke, TextureHandle,
    TextureOptions, Vec2,
};

use crate::catalog::{load_folder, ImageEntry, Mark};
use crate::exif::ExifInfo;
use crate::preview::{load_preview, load_thumbnail};
use crate::xmp;

// ── layout constants ───────────────────────────────────────────────────────

const FILMSTRIP_MIN: f32 = 60.0;
const PREVIEW_MAX: f32 = 800.0;
const MIN_PREVIEW: f32 = 200.0;

// ── persistence ────────────────────────────────────────────────────────────

fn state_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".cull-state")
}

pub struct SavedState {
    pub filmstrip_height: f32,
    pub window_width: f32,
    pub window_height: f32,
}

impl SavedState {
    pub fn load() -> Self {
        std::fs::read_to_string(state_path()).ok()
            .and_then(|text| {
                let mut lines = text.lines();
                Some(Self {
                    filmstrip_height: lines.next()?.parse().ok()?,
                    window_width: lines.next()?.parse().ok()?,
                    window_height: lines.next()?.parse().ok()?,
                })
            })
            .unwrap_or(Self {
                filmstrip_height: 108.0,
                window_width: 1400.0,
                window_height: 900.0,
            })
    }

    fn save(filmstrip_height: f32, window_width: f32, window_height: f32) {
        let _ = std::fs::write(
            state_path(),
            format!("{filmstrip_height}\n{window_width}\n{window_height}\n"),
        );
    }
}

// ── background loading ─────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum LoadKind { Thumb, Full }

struct LoadRequest {
    index: usize,
    path: PathBuf,
    kind: LoadKind,
    rotation: u8,
    generation: u64,
}

struct LoadResult {
    index: usize,
    kind: LoadKind,
    rotation: u8,
    generation: u64,
    image: Result<egui::ColorImage, String>,
}

// ── load pool (shared priority queue) ─────────────────────────────────────
//
// Pull-based: UI rebuilds the pending queue every frame in priority order.
// Workers pop from the front (highest priority). Stale requests from prior
// viewport positions are simply gone on the next rebuild — no FIFO backlog.

struct LoadPool {
    inner: Mutex<LoadPoolInner>,
    wake: Condvar,
}

struct LoadPoolInner {
    pending: VecDeque<LoadRequest>,
    in_progress: HashSet<(usize, LoadKind)>,
    generation: u64,
}

impl LoadPool {
    fn new() -> Self {
        Self {
            inner: Mutex::new(LoadPoolInner {
                pending: VecDeque::new(),
                in_progress: HashSet::new(),
                generation: 0,
            }),
            wake: Condvar::new(),
        }
    }
}

// ── filter ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum Filter { All, Picks, Unrated }

#[derive(Debug, Clone, PartialEq)]
pub enum FileFilter { AllTypes, RawOnly, JpegOnly }

#[derive(Debug, Clone, PartialEq)]
pub enum SortOrder {
    /// Filename ascending (default — same as filesystem order)
    Name,
    /// File modification time ascending (oldest first = chronological)
    DateAsc,
    /// File modification time descending (newest first = cull backwards)
    DateDesc,
}

// ── app state ──────────────────────────────────────────────────────────────

pub struct CullApp {
    folder: Option<PathBuf>,
    images: Vec<ImageEntry>,

    selected: usize,
    selected_set: HashSet<usize>,
    anchor: usize,

    filter: Filter,
    file_filter: FileFilter,
    sort_order: SortOrder,

    thumb_textures: HashMap<usize, TextureHandle>,
    full_textures: HashMap<usize, TextureHandle>,

    load_pool: Arc<LoadPool>,
    res_rx: mpsc::Receiver<LoadResult>,
    generation: u64,

    status: String,
    needs_scroll: bool,
    filmstrip_vis: (usize, usize),
    /// Number of columns in the filmstrip grid (1 when in strip mode).
    filmstrip_cols: usize,

    /// Filmstrip panel height — managed manually to avoid egui's sticky resize.
    filmstrip_height: f32,
    /// Previous frame's window height — used to make filmstrip absorb window
    /// resize deltas so the preview stays stable.
    prev_frame_height: f32,

    /// Explorer sidebar visibility
    show_explorer: bool,
    /// Root directory for the explorer tree (parent of current folder)
    explorer_root: Option<PathBuf>,

    /// EXIF info per image index — populated in background after folder open.
    exif_data: HashMap<usize, ExifInfo>,
    exif_rx: mpsc::Receiver<(usize, ExifInfo)>,
    exif_tx_template: mpsc::Sender<(usize, ExifInfo)>,
    /// Unique camera bodies found (for filter dropdown)
    cameras_found: Vec<String>,
    /// Unique lenses found
    lenses_found: Vec<String>,
    /// Active EXIF filters (empty = no filter)
    camera_filter: String,
    lens_filter: String,
}

impl CullApp {
    pub fn new(_cc: &eframe::CreationContext<'_>, preload: Option<PathBuf>) -> Self {
        let pool = Arc::new(LoadPool::new());
        let (res_tx, res_rx) = mpsc::channel::<LoadResult>();

        // Spawn persistent worker threads that pull from the shared priority queue.
        let n_workers = std::thread::available_parallelism()
            .map(|n| n.get().clamp(2, 8))
            .unwrap_or(4);
        for _ in 0..n_workers {
            let pool = pool.clone();
            let res_tx = res_tx.clone();
            std::thread::spawn(move || {
                loop {
                    let req = {
                        let mut q = pool.inner.lock().unwrap();
                        loop {
                            // Skip stale-generation items still in the queue
                            while q.pending.front()
                                .map_or(false, |r| r.generation != q.generation)
                            {
                                q.pending.pop_front();
                            }
                            if let Some(req) = q.pending.pop_front() {
                                q.in_progress.insert((req.index, req.kind));
                                break req;
                            }
                            q = pool.wake.wait(q).unwrap();
                        }
                    };
                    let image = match req.kind {
                        LoadKind::Thumb => load_thumbnail(&req.path, req.rotation),
                        LoadKind::Full  => load_preview(&req.path, req.rotation),
                    }.map_err(|e| e.to_string());
                    {
                        let mut q = pool.inner.lock().unwrap();
                        q.in_progress.remove(&(req.index, req.kind));
                    }
                    let _ = res_tx.send(LoadResult {
                        index: req.index,
                        kind: req.kind,
                        rotation: req.rotation,
                        generation: req.generation,
                        image,
                    });
                }
            });
        }

        let saved = SavedState::load();
        let (exif_tx_init, exif_rx_init) = mpsc::channel::<(usize, ExifInfo)>();

        let mut app = Self {
            folder: None,
            images: Vec::new(),
            selected: 0,
            selected_set: HashSet::new(),
            anchor: 0,
            filter: Filter::All,
            file_filter: FileFilter::AllTypes,
            sort_order: SortOrder::Name,
            thumb_textures: HashMap::new(),
            full_textures: HashMap::new(),
            load_pool: pool,
            res_rx,
            generation: 0,
            status: "Drop a folder here or click Open".into(),
            needs_scroll: true,
            filmstrip_vis: (0, 0),
            filmstrip_cols: 1,
            filmstrip_height: saved.filmstrip_height,
            prev_frame_height: 0.0,
            show_explorer: false,
            explorer_root: None,
            exif_data: HashMap::new(),
            exif_rx: exif_rx_init,
            exif_tx_template: exif_tx_init,
            cameras_found: Vec::new(),
            lenses_found: Vec::new(),
            camera_filter: String::new(),
            lens_filter: String::new(),
        };

        if let Some(path) = preload {
            app.open_folder(path);
        }
        app
    }

    fn open_folder(&mut self, path: PathBuf) {
        let images = load_folder(&path);
        let count = images.len();
        self.images = images;
        self.selected = 0;
        self.anchor = 0;
        self.selected_set.clear();
        if count > 0 { self.selected_set.insert(0); }
        self.thumb_textures.clear();
        self.full_textures.clear();
        self.generation += 1;
        {
            let mut q = self.load_pool.inner.lock().unwrap();
            q.pending.clear();
            q.in_progress.clear();
            q.generation = self.generation;
        }
        self.status = format!("{count} images");
        self.needs_scroll = true;

        // Explorer root = parent of current folder (shows siblings in tree)
        let explorer_root = path.parent().map(|p| p.to_path_buf()).unwrap_or_else(|| path.clone());
        self.explorer_root = Some(explorer_root);
        self.folder = Some(path);

        // Background EXIF scan — reads ~8KB per file, populates camera/lens/ISO
        self.exif_data.clear();
        self.cameras_found.clear();
        self.lenses_found.clear();
        self.camera_filter.clear();
        self.lens_filter.clear();
        let (tx, rx) = mpsc::channel();
        self.exif_rx = rx;
        self.exif_tx_template = tx.clone();
        let paths: Vec<(usize, PathBuf)> = self.images.iter().enumerate()
            .map(|(i, img)| (i, img.path.clone()))
            .collect();
        std::thread::spawn(move || {
            for (idx, path) in paths {
                if let Some(info) = crate::exif::read_exif(&path) {
                    if tx.send((idx, info)).is_err() { break; }
                }
            }
        });
    }

    fn visible_indices(&self) -> Vec<usize> {
        let cam = &self.camera_filter;
        let lens = &self.lens_filter;
        let mut indices: Vec<usize> = self.images
            .iter()
            .enumerate()
            .filter(|(_, img)| match self.filter {
                Filter::All     => true,
                Filter::Picks   => img.mark == Mark::Pick,
                Filter::Unrated => img.mark == Mark::None,
            })
            .filter(|(_, img)| match self.file_filter {
                FileFilter::AllTypes => true,
                FileFilter::RawOnly => img.is_raw(),
                FileFilter::JpegOnly => img.is_jpeg(),
            })
            .filter(|(i, _)| {
                if !cam.is_empty() {
                    if let Some(exif) = self.exif_data.get(i) {
                        if exif.camera != *cam { return false; }
                    } else { return false; }
                }
                if !lens.is_empty() {
                    if let Some(exif) = self.exif_data.get(i) {
                        if exif.lens != *lens { return false; }
                    } else { return false; }
                }
                true
            })
            .map(|(i, _)| i)
            .collect();

        match self.sort_order {
            SortOrder::Name => {} // already sorted by filename from load_folder
            SortOrder::DateAsc => {
                indices.sort_by(|&a, &b| self.images[a].modified.cmp(&self.images[b].modified));
            }
            SortOrder::DateDesc => {
                indices.sort_by(|&a, &b| self.images[b].modified.cmp(&self.images[a].modified));
            }
        }

        indices
    }

    /// Rebuild the shared load queue from scratch based on current state.
    /// Called every frame — clears stale requests and re-enqueues by priority.
    fn rebuild_load_queue(&self) {
        let visible = self.visible_indices();
        if visible.is_empty() || self.images.is_empty() { return; }

        let (fv_s, fv_e) = self.filmstrip_vis;
        let gen = self.generation;
        let n = self.images.len();

        // Build priority-ordered wanted list (deduped)
        let mut seen = HashSet::new();
        let mut wanted: Vec<(usize, LoadKind)> = Vec::new();
        {
            let mut enqueue = |idx: usize, kind: LoadKind| {
                if idx < n && seen.insert((idx, kind)) {
                    wanted.push((idx, kind));
                }
            };

            // P1: Full preview for selected image (user is looking at this NOW)
            enqueue(self.selected, LoadKind::Full);

            // P2: Thumbnails for visible viewport (filmstrip must never be black;
            //     thumbs decode in ~1ms so they clear fast even behind one full)
            for i in fv_s..=fv_e.min(visible.len().saturating_sub(1)) {
                if let Some(&idx) = visible.get(i) { enqueue(idx, LoadKind::Thumb); }
            }

            // P3: Full previews for selected ± 4 (arrow-key anticipation)
            if let Some(pos) = visible.iter().position(|&i| i == self.selected) {
                for d in 1..=4usize {
                    if pos + d < visible.len() { enqueue(visible[pos + d], LoadKind::Full); }
                    if pos >= d                { enqueue(visible[pos - d], LoadKind::Full); }
                }
            }

            // P4: Full previews for visible filmstrip items + buffer
            //     (clicking any visible thumb should show full preview instantly)
            let preview_buf = 4;
            let plo = fv_s.saturating_sub(preview_buf);
            let phi = (fv_e + preview_buf).min(visible.len().saturating_sub(1));
            for i in plo..=phi {
                if let Some(&idx) = visible.get(i) { enqueue(idx, LoadKind::Full); }
            }

            // P5: Thumbnails for wider buffer (scroll anticipation)
            let thumb_buf = 12;
            let tlo = fv_s.saturating_sub(thumb_buf);
            let thi = (fv_e + thumb_buf).min(visible.len().saturating_sub(1));
            for i in tlo..=thi {
                if let Some(&idx) = visible.get(i) { enqueue(idx, LoadKind::Thumb); }
            }
        }

        // Flush to shared queue — replaces all previous pending items
        let mut q = self.load_pool.inner.lock().unwrap();
        q.pending.clear();
        for (idx, kind) in wanted {
            let has = match kind {
                LoadKind::Thumb => self.thumb_textures.contains_key(&idx),
                LoadKind::Full  => self.full_textures.contains_key(&idx),
            };
            if !has && !q.in_progress.contains(&(idx, kind)) {
                q.pending.push_back(LoadRequest {
                    index: idx,
                    path: self.images[idx].path.clone(),
                    kind,
                    rotation: self.images[idx].rotation,
                    generation: gen,
                });
            }
        }
        drop(q);
        self.load_pool.wake.notify_all();
    }

    /// Evict full textures far from both viewport and selection to cap memory.
    fn evict_textures(&mut self) {
        let visible = self.visible_indices();
        if visible.is_empty() { return; }

        let (fv_s, fv_e) = self.filmstrip_vis;
        let margin = 30;

        // Keep around viewport
        let vp_lo = fv_s.saturating_sub(margin);
        let vp_hi = (fv_e + margin).min(visible.len().saturating_sub(1));

        // Also keep around selected (might be outside viewport after keyboard nav)
        let sel_pos = visible.iter().position(|&i| i == self.selected).unwrap_or(0);
        let sel_lo = sel_pos.saturating_sub(margin);
        let sel_hi = (sel_pos + margin).min(visible.len().saturating_sub(1));

        let keep: HashSet<usize> = (vp_lo..=vp_hi)
            .chain(sel_lo..=sel_hi)
            .filter_map(|i| visible.get(i).copied())
            .collect();
        self.full_textures.retain(|idx, _| keep.contains(idx));
    }

    /// Check if a full preview is queued or being decoded for this image.
    fn is_load_pending(&self, idx: usize) -> bool {
        let q = self.load_pool.inner.lock().unwrap();
        q.in_progress.contains(&(idx, LoadKind::Full))
            || q.pending.iter().any(|r| r.index == idx && r.kind == LoadKind::Full)
    }

    // ── mutations ──────────────────────────────────────────────────────────

    fn set_mark_single(&mut self, idx: usize, mark: Mark) {
        if idx < self.images.len() {
            xmp::write_mark(&self.images[idx].path, &mark);
            self.images[idx].mark = mark;
        }
    }

    fn apply_mark(&mut self, mark: Mark) {
        for idx in self.selected_set.clone() {
            self.set_mark_single(idx, mark.clone());
        }
    }

    /// Rotate all images in the selection set.
    fn rotate(&mut self, delta: i8) {
        for idx in self.selected_set.clone() {
            if idx >= self.images.len() { continue; }
            let img = &mut self.images[idx];
            img.rotation = ((img.rotation as i8 + delta).rem_euclid(4)) as u8;
            xmp::write_rotation(&img.path.clone(), img.rotation);
            self.full_textures.remove(&idx);
            self.thumb_textures.remove(&idx);
        }
        // Queue will be rebuilt next frame; stale in-flight results are
        // discarded in the drain loop via rotation mismatch check.
    }

    fn export_picks(&mut self) {
        let folder = match &self.folder { Some(f) => f.clone(), None => return };
        let dest = folder.join("_picks");
        if let Err(e) = std::fs::create_dir_all(&dest) {
            self.status = format!("Export failed: {e}"); return;
        }
        let mut n = 0usize;
        for img in self.images.iter().filter(|i| i.mark == Mark::Pick) {
            if let Some(name) = img.path.file_name() {
                if std::fs::copy(&img.path, dest.join(name)).is_ok() { n += 1; }
            }
        }
        self.status = format!("Exported {n} picks → _picks/");
    }

    // ── selection helpers ──────────────────────────────────────────────────

    fn nav_to(&mut self, idx: usize) {
        self.selected = idx;
        self.anchor = idx;
        self.selected_set.clear();
        self.selected_set.insert(idx);
        self.needs_scroll = true;
    }

    fn shift_select_to(&mut self, idx: usize, visible: &[usize]) {
        self.selected = idx;
        let a = visible.iter().position(|&i| i == self.anchor).unwrap_or(0);
        let b = visible.iter().position(|&i| i == idx).unwrap_or(0);
        let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
        self.selected_set.clear();
        for i in lo..=hi { self.selected_set.insert(visible[i]); }
        self.needs_scroll = true;
    }

    fn toggle_select(&mut self, idx: usize) {
        if self.selected_set.contains(&idx) && self.selected_set.len() > 1 {
            self.selected_set.remove(&idx);
            if self.selected == idx {
                self.selected = *self.selected_set.iter().next().unwrap();
            }
        } else {
            self.selected_set.insert(idx);
            self.selected = idx;
        }
        self.anchor = idx;
    }
}

// ── eframe::App ────────────────────────────────────────────────────────────

impl eframe::App for CullApp {
    fn update(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        // 1. Drain loader — discard stale generation / rotation results
        while let Ok(r) = self.res_rx.try_recv() {
            if r.generation != self.generation { continue; }
            // Rotation may have changed while decode was in flight
            if let Some(img) = self.images.get(r.index) {
                if img.rotation != r.rotation { continue; }
            }
            if let Ok(img) = r.image {
                let tex = ctx.load_texture(
                    format!("img_{}_{}", r.index, r.kind as u8),
                    img, TextureOptions::LINEAR,
                );
                match r.kind {
                    LoadKind::Thumb => { self.thumb_textures.insert(r.index, tex); }
                    LoadKind::Full  => { self.full_textures.insert(r.index, tex); }
                }
                ctx.request_repaint();
            }
        }

        // 1a. Drain background EXIF results
        {
            let mut changed = false;
            while let Ok((idx, info)) = self.exif_rx.try_recv() {
                self.exif_data.insert(idx, info);
                changed = true;
            }
            if changed {
                // Rebuild unique camera/lens lists
                let mut cameras: Vec<String> = self.exif_data.values()
                    .filter(|e| !e.camera.is_empty())
                    .map(|e| e.camera.clone())
                    .collect::<HashSet<_>>().into_iter().collect();
                cameras.sort();
                self.cameras_found = cameras;

                let mut lenses: Vec<String> = self.exif_data.values()
                    .filter(|e| !e.lens.is_empty())
                    .map(|e| e.lens.clone())
                    .collect::<HashSet<_>>().into_iter().collect();
                lenses.sort();
                self.lenses_found = lenses;

                ctx.request_repaint();
            }
        }

        // 1b. Window resize → filmstrip absorbs the delta so preview stays stable
        let screen = ctx.screen_rect();
        let current_h = screen.height();
        let current_w = screen.width();
        if self.prev_frame_height > 0.0 {
            let delta = current_h - self.prev_frame_height;
            if delta.abs() > 0.5 {
                let max_fs = (current_h - MIN_PREVIEW).max(FILMSTRIP_MIN);
                self.filmstrip_height = (self.filmstrip_height + delta).clamp(FILMSTRIP_MIN, max_fs);
                SavedState::save(self.filmstrip_height, current_w, current_h);
            }
        }
        self.prev_frame_height = current_h;

        // Cap preview: if preview exceeds PREVIEW_MAX, give excess to filmstrip
        let overhead = 46.0; // approx toolbar + divider
        let preview_h = current_h - overhead - self.filmstrip_height;
        if preview_h > PREVIEW_MAX {
            self.filmstrip_height += preview_h - PREVIEW_MAX;
        }
        let max_fs = (current_h - MIN_PREVIEW).max(FILMSTRIP_MIN);
        self.filmstrip_height = self.filmstrip_height.clamp(FILMSTRIP_MIN, max_fs);

        // 2. Drag-and-drop
        let dropped = ctx.input(|i| i.raw.dropped_files.first().and_then(|f| f.path.clone()));
        if let Some(p) = dropped {
            let folder = if p.is_dir() { p } else { p.parent().unwrap_or(&p).to_path_buf() };
            self.open_folder(folder);
        }

        // 3. Keyboard
        let (nav_right, nav_left, nav_down, nav_up,
             do_pick, do_reject, do_unmark, do_export,
             rotate_ccw, rotate_cw, toggle_explorer, shift, cmd) = ctx.input(|i| (
            i.key_pressed(Key::ArrowRight),
            i.key_pressed(Key::ArrowLeft),
            i.key_pressed(Key::ArrowDown),
            i.key_pressed(Key::ArrowUp),
            i.key_pressed(Key::P) || i.key_pressed(Key::Space),
            i.key_pressed(Key::X),
            i.key_pressed(Key::U),
            i.key_pressed(Key::E) && i.modifiers.command,
            i.key_pressed(Key::R) && !i.modifiers.shift,
            i.key_pressed(Key::R) && i.modifiers.shift,
            i.key_pressed(Key::B) && i.modifiers.command,
            i.modifiers.shift,
            i.modifiers.command,
        ));

        if toggle_explorer { self.show_explorer = !self.show_explorer; }

        // 4. Process input
        let visible = self.visible_indices();
        if !visible.is_empty() {
            let cur = visible.iter().position(|&i| i == self.selected).unwrap_or(0);
            let cols = self.filmstrip_cols;

            // Left/Right: move by one item
            if nav_right && cur + 1 < visible.len() {
                let next = visible[cur + 1];
                if shift { self.shift_select_to(next, &visible); } else { self.nav_to(next); }
            }
            if nav_left && cur > 0 {
                let prev = visible[cur - 1];
                if shift { self.shift_select_to(prev, &visible); } else { self.nav_to(prev); }
            }
            // Down/Up: move by one row (cols items) in grid mode, or one item in strip mode
            if nav_down {
                let target = (cur + cols).min(visible.len() - 1);
                if target != cur {
                    let next = visible[target];
                    if shift { self.shift_select_to(next, &visible); } else { self.nav_to(next); }
                }
            }
            if nav_up {
                let target = cur.saturating_sub(cols);
                if target != cur {
                    let prev = visible[target];
                    if shift { self.shift_select_to(prev, &visible); } else { self.nav_to(prev); }
                }
            }
            if do_pick {
                let m = if self.selected_set.iter().all(|&i| self.images[i].mark == Mark::Pick)
                    { Mark::None } else { Mark::Pick };
                self.apply_mark(m);
            }
            if do_reject {
                let m = if self.selected_set.iter().all(|&i| self.images[i].mark == Mark::Reject)
                    { Mark::None } else { Mark::Reject };
                self.apply_mark(m);
            }
            if do_unmark  { self.apply_mark(Mark::None); }
            if rotate_ccw { self.rotate(1); }
            if rotate_cw  { self.rotate(-1); }
        }
        if do_export { self.export_picks(); }

        // 5. Rebuild load queue + evict distant textures
        self.rebuild_load_queue();
        self.evict_textures();

        // 6. Render
        //    render_explorer may navigate to a new folder, changing self.images.
        //    Recompute visible after explorer so filmstrip/main use fresh indices.
        self.render_toolbar(ctx);
        self.render_explorer(ctx);
        let visible = self.visible_indices();
        self.render_filmstrip(ctx, &visible, shift, cmd);
        self.render_main(ctx, &visible);
    }
}

// ── thumbnail data for filmstrip ───────────────────────────────────────────

struct TD {
    idx: usize,
    vis_pos: usize,
    is_cursor: bool,
    in_set: bool,
    mark: Mark,
    tex_id: Option<egui::TextureId>,
}

// ── rendering ──────────────────────────────────────────────────────────────

impl CullApp {
    fn render_toolbar(&mut self, ctx: &Context) {
        let total   = self.images.len();
        let picks   = self.images.iter().filter(|i| i.mark == Mark::Pick).count();
        let unrated = self.images.iter().filter(|i| i.mark == Mark::None).count();
        let n_raw   = self.images.iter().filter(|i| i.is_raw()).count();
        let n_jpeg  = self.images.iter().filter(|i| i.is_jpeg()).count();
        let sel_n   = self.selected_set.len();
        let visible = self.visible_indices();

        // Only show file type filter when both types are present
        let has_mixed_types = n_raw > 0 && n_jpeg > 0;

        egui::TopBottomPanel::top("toolbar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                // Explorer toggle
                let explorer_label = if self.show_explorer { "Hide" } else { "Files" };
                if ui.button(explorer_label).clicked() {
                    self.show_explorer = !self.show_explorer;
                }

                ui.separator();

                // Folder breadcrumb
                if let Some(folder) = &self.folder.clone() {
                    let label = folder.file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("…");
                    if ui.button(format!(">{label}")).on_hover_text(folder.display().to_string()).clicked() {
                        if let Some(p) = rfd::FileDialog::new()
                            .set_directory(folder)
                            .pick_folder()
                        {
                            self.open_folder(p);
                        }
                    }
                } else if ui.button(">Open Folder").clicked() {
                    if let Some(p) = rfd::FileDialog::new().pick_folder() {
                        self.open_folder(p);
                    }
                }

                ui.separator();

                // Mark filter
                ui.selectable_value(&mut self.filter, Filter::All,     format!("All  {total}"));
                ui.selectable_value(&mut self.filter, Filter::Picks,   format!("Picks  {picks}"));
                ui.selectable_value(&mut self.filter, Filter::Unrated, format!("Unrated  {unrated}"));

                // File type filter — only shown when folder has both RAW and JPEG
                if has_mixed_types {
                    ui.separator();
                    ui.selectable_value(&mut self.file_filter, FileFilter::AllTypes, "All types");
                    ui.selectable_value(&mut self.file_filter, FileFilter::RawOnly,  format!("RAW  {n_raw}"));
                    ui.selectable_value(&mut self.file_filter, FileFilter::JpegOnly, format!("JPEG  {n_jpeg}"));
                }

                // Camera filter — only shown when multiple bodies detected
                if self.cameras_found.len() > 1 {
                    ui.separator();
                    let cam_label = if self.camera_filter.is_empty() {
                        "Camera: All".to_string()
                    } else {
                        self.camera_filter.clone()
                    };
                    egui::ComboBox::from_id_salt("cam_filter")
                        .selected_text(&cam_label)
                        .width(120.0)
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut self.camera_filter, String::new(), "All cameras");
                            for c in &self.cameras_found.clone() {
                                ui.selectable_value(&mut self.camera_filter, c.clone(), c);
                            }
                        });
                }

                // Lens filter
                if self.lenses_found.len() > 1 {
                    ui.separator();
                    let lens_label = if self.lens_filter.is_empty() {
                        "Lens: All".to_string()
                    } else {
                        self.lens_filter.clone()
                    };
                    egui::ComboBox::from_id_salt("lens_filter")
                        .selected_text(&lens_label)
                        .width(140.0)
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut self.lens_filter, String::new(), "All lenses");
                            for l in &self.lenses_found.clone() {
                                ui.selectable_value(&mut self.lens_filter, l.clone(), l);
                            }
                        });
                }

                // Sort order — cycles through options on click
                ui.separator();
                let sort_label = match self.sort_order {
                    SortOrder::Name     => "Name A-Z",
                    SortOrder::DateAsc  => "Date old",
                    SortOrder::DateDesc => "Date new",
                };
                if ui.button(sort_label)
                    .on_hover_text("Click to cycle: Name / Date / Date (reversed)")
                    .clicked()
                {
                    self.sort_order = match self.sort_order {
                        SortOrder::Name     => SortOrder::DateAsc,
                        SortOrder::DateAsc  => SortOrder::DateDesc,
                        SortOrder::DateDesc => SortOrder::Name,
                    };
                }

                if sel_n > 1 {
                    ui.separator();
                    ui.label(format!("{sel_n} selected"));
                    if ui.button("Pick").clicked()   { self.apply_mark(Mark::Pick); }
                    if ui.button("Reject").clicked() { self.apply_mark(Mark::Reject); }
                    if ui.button("Unmark").clicked() { self.apply_mark(Mark::None); }
                }

                ui.separator();
                ui.label(&self.status);

                ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                    if ui.add_enabled(picks > 0, egui::Button::new(format!("Export {picks}"))).clicked() {
                        self.export_picks();
                    }
                    if !visible.is_empty() {
                        let pos = visible.iter().position(|&i| i == self.selected).map(|p| p + 1).unwrap_or(1);
                        ui.label(format!("{pos} / {}", visible.len()));
                    }
                });
            });
        });
    }

    fn render_explorer(&mut self, ctx: &Context) {
        if !self.show_explorer { return; }

        let current_folder = self.folder.clone();
        let root = self.explorer_root.clone();
        let sel_n = self.selected_set.len();
        let mut navigate_to: Option<PathBuf> = None;
        let mut move_to: Option<PathBuf> = None;

        egui::SidePanel::left("explorer")
            .resizable(true)
            .default_width(200.0)
            .min_width(140.0)
            .max_width(400.0)
            .show(ctx, |ui| {
                // Header with root path
                if let Some(root) = &root {
                    let root_name = root.file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("…");
                    ui.horizontal(|ui| {
                        // Up button
                        if let Some(parent) = root.parent() {
                            if ui.small_button("..").on_hover_text("Go up one level").clicked() {
                                navigate_to = Some(parent.to_path_buf());
                            }
                        }
                        ui.strong(root_name);
                    });
                }
                ui.separator();

                ScrollArea::vertical().show(ui, |ui| {
                    // Render tree starting from explorer_root
                    if let Some(root) = &root {
                        render_dir_tree(
                            ui,
                            root,
                            current_folder.as_deref(),
                            sel_n,
                            0,
                            &mut navigate_to,
                            &mut move_to,
                        );
                    }
                });
            });

        if let Some(path) = move_to {
            self.move_selected_to(&path);
        }
        if let Some(path) = navigate_to {
            self.open_folder(path);
        }
    }

    fn render_filmstrip(&mut self, ctx: &Context, visible: &[usize], shift: bool, cmd: bool) {

        let td: Vec<TD> = visible.iter().enumerate().map(|(vis_pos, &idx)| TD {
            idx, vis_pos,
            is_cursor: idx == self.selected,
            in_set: self.selected_set.contains(&idx),
            mark: self.images[idx].mark.clone(),
            tex_id: self.thumb_textures.get(&idx)
                .or_else(|| self.full_textures.get(&idx))
                .map(|t| t.id()),
        }).collect();

        let needs_scroll = self.needs_scroll;
        let mut clicked: Option<(usize, bool, bool)> = None;
        let mut new_vis: (usize, usize) = (usize::MAX, 0);
        let mut computed_cols: usize = 1;

        // Bottom panels stack upward in egui — render filmstrip FIRST so it
        // occupies the bottom edge, then the resize handle sits ABOVE it
        // (between the preview and filmstrip, where the user expects it).
        egui::TopBottomPanel::bottom("filmstrip")
            .exact_height(self.filmstrip_height)
            .show(ctx, |ui| {
                let avail_h = ui.available_height();
                let avail_w = ui.available_width();

                // Fixed thumb size — always 88px. The panel height determines
                // how many rows fit, which determines the layout mode.
                let item_px: f32 = 88.0;
                let cell = item_px + 6.0; // item + padding
                let n_rows = ((avail_h / cell).floor() as usize).max(1);
                let multi_row = n_rows >= 2;

                if multi_row {
                    // ── GRID MODE: vertical scroll, items wrap left→right top→bottom ──
                    let cols = ((avail_w - 8.0) / cell).floor().max(1.0) as usize;
                    computed_cols = cols;

                    ScrollArea::vertical()
                        .id_salt("filmstrip_scroll")
                        .show(ui, |ui| {
                            let total_rows = (td.len() + cols - 1) / cols;
                            for row in 0..total_rows {
                                ui.horizontal(|ui| {
                                    ui.add_space(4.0);
                                    for col in 0..cols {
                                        let item_i = row * cols + col;
                                        if let Some(t) = td.get(item_i) {
                                            let response = self.paint_thumb(ui, t, item_px, needs_scroll);
                                            if response.rect.intersects(ui.clip_rect()) {
                                                new_vis.0 = new_vis.0.min(t.vis_pos);
                                                new_vis.1 = new_vis.1.max(t.vis_pos);
                                            }
                                            if response.clicked() {
                                                clicked = Some((t.idx, shift, cmd));
                                            }
                                        }
                                    }
                                });
                            }
                        });
                } else {
                    // ── STRIP MODE: horizontal scroll, single row ──
                    ScrollArea::horizontal()
                        .id_salt("filmstrip_scroll")
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.add_space(4.0);
                                for t in &td {
                                    let response = self.paint_thumb(ui, t, item_px, needs_scroll);
                                    if response.rect.intersects(ui.clip_rect()) {
                                        new_vis.0 = new_vis.0.min(t.vis_pos);
                                        new_vis.1 = new_vis.1.max(t.vis_pos);
                                    }
                                    if response.clicked() {
                                        clicked = Some((t.idx, shift, cmd));
                                    }
                                }
                            });
                        });
                }
            });

        // Resize handle — rendered AFTER filmstrip so it stacks above it,
        // sitting between the preview pane and the filmstrip.
        egui::TopBottomPanel::bottom("filmstrip_resize")
            .exact_height(6.0)
            .show(ctx, |ui| {
                let response = ui.allocate_response(
                    Vec2::new(ui.available_width(), 6.0),
                    Sense::drag(),
                );
                let rect = response.rect;
                ui.painter().rect_filled(rect, 0.0, Color32::from_gray(45));
                ui.painter().line_segment(
                    [rect.center_top() + egui::vec2(0.0, 2.0),
                     rect.center_top() + egui::vec2(0.0, 4.0)],
                    Stroke::new(20.0, Color32::from_gray(70)),
                );
                if response.dragged() {
                    let max_fs = (ui.ctx().screen_rect().height() - MIN_PREVIEW).max(FILMSTRIP_MIN);
                    self.filmstrip_height = (self.filmstrip_height - response.drag_delta().y)
                        .clamp(FILMSTRIP_MIN, max_fs);
                }
                if response.drag_stopped() {
                    let screen = ui.ctx().screen_rect();
                    SavedState::save(self.filmstrip_height, screen.width(), screen.height());
                }
                if response.hovered() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
                }
            });

        if let Some((idx, is_shift, is_cmd)) = clicked {
            let vis = self.visible_indices();
            if is_shift {
                self.shift_select_to(idx, &vis);
            } else if is_cmd {
                self.toggle_select(idx);
            } else {
                self.nav_to(idx);
            }
            self.needs_scroll = false;
        }

        if new_vis.0 != usize::MAX { self.filmstrip_vis = new_vis; }
        self.needs_scroll = false;
    }

    /// Paint a single filmstrip thumbnail. Returns the click response.
    fn paint_thumb(&self, ui: &mut egui::Ui, t: &TD, item_px: f32, needs_scroll: bool) -> egui::Response {
        let (response, painter) = ui.allocate_painter(
            Vec2::splat(item_px + 4.0), Sense::click(),
        );

        if t.is_cursor && needs_scroll {
            response.scroll_to_me(Some(Align::Center));
        }

        let bg = if t.in_set && !t.is_cursor {
            Color32::from_rgba_premultiplied(30, 55, 110, 255)
        } else {
            Color32::from_gray(20)
        };
        let rect = response.rect.shrink(2.0);
        painter.rect_filled(rect, 2.0, bg);

        if let Some(tex_id) = t.tex_id {
            painter.image(
                tex_id, rect,
                Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                Color32::WHITE,
            );
        }

        let (bc, bw) = if t.is_cursor {
            (Color32::WHITE, 2.5)
        } else if t.in_set {
            (Color32::from_rgb(80, 140, 230), 2.0)
        } else {
            match &t.mark {
                Mark::Pick   => (Color32::from_rgb(72, 199, 116), 1.5),
                Mark::Reject => (Color32::from_rgb(220, 80, 80), 1.5),
                Mark::None   => (Color32::from_gray(50), 1.0),
            }
        };
        painter.rect_stroke(response.rect.shrink(1.0), 2.0, Stroke::new(bw, bc));

        let badge_sz = (item_px * 0.16).clamp(12.0, 20.0);
        if let Some((label, color)) = match t.mark {
            Mark::Pick   => Some(("P", Color32::from_rgb(72, 199, 116))),
            Mark::Reject => Some(("R", Color32::from_rgb(220, 80, 80))),
            Mark::None   => None,
        } {
            let b = Rect::from_min_size(rect.min, Vec2::splat(badge_sz));
            painter.rect_filled(b, 0.0, Color32::from_black_alpha(160));
            painter.text(
                b.center(), egui::Align2::CENTER_CENTER, label,
                FontId::proportional(badge_sz * 0.65), color,
            );
        }

        ui.add_space(2.0);
        response
    }

    fn render_main(&mut self, ctx: &Context, visible: &[usize]) {
        let is_empty   = self.images.is_empty();
        let no_visible = visible.is_empty();
        let selected   = self.selected;
        let filename   = self.images.get(selected).map(|e| e.filename().to_string());
        let mark       = self.images.get(selected).map(|e| e.mark.clone());
        let rotation   = self.images.get(selected).map(|e| e.rotation).unwrap_or(0);
        let vis_pos    = visible.iter().position(|&i| i == selected).map(|p| p + 1).unwrap_or(1);
        let vis_total  = visible.len();

        let tex_info = self.full_textures.get(&selected)
            .or_else(|| self.thumb_textures.get(&selected))
            .map(|t| (t.id(), t.size_vec2()));
        let is_thumb_only = tex_info.is_some() && !self.full_textures.contains_key(&selected);
        let is_loading = self.is_load_pending(selected);

        egui::CentralPanel::default().show(ctx, |ui| {
            if is_empty {
                ui.centered_and_justified(|ui| {
                    ui.heading("Drop a folder of RAW files here");
                    ui.label("Supports CR2, CR3, NEF, ARW, DNG, ORF, RAF, RW2, JPEG");
                });
                return;
            }
            if no_visible {
                ui.centered_and_justified(|ui| { ui.label("No images match the current filter."); });
                return;
            }

            let available = ui.available_size();
            match tex_info {
                Some((tex_id, tex_size)) => {
                    let scale = (available.x / tex_size.x)
                        .min((available.y - 30.0) / tex_size.y)
                        .min(1.0);
                    let img_size = tex_size * scale;
                    ui.centered_and_justified(|ui| {
                        ui.add(egui::Image::new((tex_id, img_size)).maintain_aspect_ratio(true));
                    });
                }
                None => {
                    ui.centered_and_justified(|ui| {
                        if is_loading { ui.spinner(); } else { ui.label("Failed to load preview"); }
                    });
                }
            }

            let painter    = ui.painter();
            let panel_rect = ui.max_rect();

            let rot_label = match rotation { 1 => " -90", 2 => " 180", 3 => " +90", _ => "" };
            let exif_label = self.exif_data.get(&selected).map(|e| {
                let mut parts = Vec::new();
                if !e.camera.is_empty() { parts.push(e.camera.as_str()); }
                if !e.lens.is_empty() { parts.push(e.lens.as_str()); }
                let mut extra = String::new();
                if e.focal_mm > 0.0 { extra.push_str(&format!("{}mm", e.focal_mm as u32)); }
                if e.iso > 0 {
                    if !extra.is_empty() { extra.push_str("  "); }
                    extra.push_str(&format!("ISO {}", e.iso));
                }
                if !extra.is_empty() { parts.push(&extra); }
                // return owned string from parts joined
                parts.join("  ")
            }).unwrap_or_default();
            // We need the exif_label to live long enough
            let exif_suffix = if exif_label.is_empty() { String::new() } else { format!("   {exif_label}") };
            let info = format!(
                "{}{}   {vis_pos}/{vis_total}{exif_suffix}",
                filename.as_deref().unwrap_or(""), rot_label,
            );
            painter.rect_filled(
                Rect::from_min_max(
                    egui::pos2(panel_rect.left(), panel_rect.bottom() - 26.0),
                    panel_rect.right_bottom(),
                ),
                0.0, Color32::from_black_alpha(180),
            );
            painter.text(
                egui::pos2(panel_rect.left() + 10.0, panel_rect.bottom() - 13.0),
                egui::Align2::LEFT_CENTER, &info,
                FontId::proportional(12.0), Color32::from_gray(200),
            );

            if is_thumb_only {
                painter.text(
                    egui::pos2(panel_rect.left() + 10.0, panel_rect.top() + 20.0),
                    egui::Align2::LEFT_TOP, "loading…",
                    FontId::proportional(12.0), Color32::from_gray(140),
                );
            }

            if let Some(mark) = mark {
                let (text, color) = match mark {
                    Mark::Pick   => ("PICK",   Color32::from_rgb(72, 199, 116)),
                    Mark::Reject => ("REJECT", Color32::from_rgb(220, 80, 80)),
                    Mark::None   => ("",       Color32::TRANSPARENT),
                };
                if !text.is_empty() {
                    painter.text(
                        egui::pos2(panel_rect.right() - 12.0, panel_rect.top() + 20.0),
                        egui::Align2::RIGHT_TOP, text,
                        FontId::proportional(18.0), color,
                    );
                }
            }
        });
    }

    /// Move all selected images to a target directory.
    fn move_selected_to(&mut self, target: &std::path::Path) {
        let mut moved = 0usize;
        let mut indices: Vec<usize> = self.selected_set.iter().cloned().collect();
        indices.sort_unstable_by(|a, b| b.cmp(a)); // reverse order so removal doesn't shift

        for idx in &indices {
            let img = &self.images[*idx];
            if let Some(name) = img.path.file_name() {
                let dest = target.join(name);
                if std::fs::rename(&img.path, &dest).is_ok() {
                    // Also move XMP sidecar if it exists
                    let sidecar = xmp::sidecar_path(&img.path);
                    if sidecar.exists() {
                        let sidecar_dest = target.join(sidecar.file_name().unwrap());
                        let _ = std::fs::rename(&sidecar, sidecar_dest);
                    }
                    moved += 1;
                }
            }
        }

        // Remove moved images from the list (indices are sorted descending)
        for idx in &indices {
            self.images.remove(*idx);
            self.thumb_textures.remove(idx);
            self.full_textures.remove(idx);
        }

        // Fix selected state
        self.selected_set.clear();
        if !self.images.is_empty() {
            self.selected = self.selected.min(self.images.len() - 1);
            self.selected_set.insert(self.selected);
        } else {
            self.selected = 0;
        }
        self.anchor = self.selected;
        self.needs_scroll = true;

        // Rebuild texture index since indices shifted
        self.thumb_textures.clear();
        self.full_textures.clear();
        // Queue will be rebuilt next frame with correct indices

        self.status = format!("Moved {moved} images");
    }
}

// ── helpers ────────────────────────────────────────────────────────────────

/// Extensions that indicate macOS bundles / non-navigable directories.
const BUNDLE_EXTS: &[&str] = &[
    "app", "photoslibrary", "photolibrary", "cocatalog", "fcpbundle",
    "lrcat", "lrdata", "bundle", "framework", "plugin", "kext",
    "band",  // Time Machine
];

/// Names to always hide.
const HIDDEN_DIRS: &[&str] = &[
    "_picks", "node_modules", "__pycache__", ".git", "target",
    "Photo Booth Library",
];

fn is_real_dir(entry: &std::fs::DirEntry) -> bool {
    // Must be a directory
    if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
        return false;
    }
    let name = entry.file_name();
    let n = name.to_str().unwrap_or("");

    // Hidden (dotfiles)
    if n.starts_with('.') { return false; }

    // Known non-dir names
    if HIDDEN_DIRS.contains(&n) { return false; }

    // macOS bundles look like dirs but aren't navigable image folders
    if let Some(ext) = std::path::Path::new(n).extension().and_then(|e| e.to_str()) {
        if BUNDLE_EXTS.contains(&ext.to_lowercase().as_str()) {
            return false;
        }
    }

    true
}

fn sorted_subdirs(dir: &std::path::Path) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok())
        .filter(|e| is_real_dir(e))
        .map(|e| e.path())
        .collect();
    dirs.sort();
    dirs
}

/// Render a collapsible directory tree. Recurses up to `max_depth` levels.
fn render_dir_tree(
    ui: &mut egui::Ui,
    dir: &std::path::Path,
    current: Option<&std::path::Path>,
    sel_count: usize,
    depth: usize,
    navigate_to: &mut Option<PathBuf>,
    move_to: &mut Option<PathBuf>,
) {
    let max_depth = 3;
    let children = sorted_subdirs(dir);

    for child in &children {
        let name = child.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("?");

        let is_current = current == Some(child.as_path());
        let is_ancestor = current.map_or(false, |c| c.starts_with(child));
        let has_children = depth < max_depth && has_subdirs(child);

        // Auto-expand if this is an ancestor of the current folder
        let default_open = is_ancestor;
        let id = ui.make_persistent_id(child);

        if has_children {
            let header = egui::collapsing_header::CollapsingState::load_with_default_open(
                ui.ctx(), id, default_open,
            );

            header.show_header(ui, |ui| {
                let resp = ui.selectable_label(is_current, name);
                if resp.clicked() && !is_current {
                    *navigate_to = Some(child.clone());
                }
                // Move-to button (only when images are selected)
                if !is_current && sel_count > 0 {
                    if resp.secondary_clicked() ||
                        (resp.hovered() && ui.input(|i| i.key_pressed(Key::M)))
                    {
                        *move_to = Some(child.clone());
                    }
                    resp.on_hover_text("Click to browse • Right-click to move selection here");
                }
            })
            .body(|ui| {
                render_dir_tree(ui, child, current, sel_count, depth + 1, navigate_to, move_to);
            });
        } else {
            let resp = ui.selectable_label(is_current, format!("   {name}"));
            if resp.clicked() && !is_current {
                *navigate_to = Some(child.clone());
            }
            if !is_current && sel_count > 0 {
                if resp.secondary_clicked() {
                    *move_to = Some(child.clone());
                }
                resp.on_hover_text("Click to browse • Right-click to move selection here");
            }
        }
    }
}

/// Quick check if a directory has any navigable subdirectories.
fn has_subdirs(dir: &std::path::Path) -> bool {
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .any(|e| e.ok().map_or(false, |e| is_real_dir(&e)))
}
