use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::mpsc;

use egui::{
    Align, Color32, Context, FontId, Key, Rect, ScrollArea, Sense, Stroke, TextureHandle,
    TextureOptions, Vec2,
};

use crate::catalog::{load_folder, ImageEntry, Mark};
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
}

struct LoadResult {
    index: usize,
    kind: LoadKind,
    image: Result<egui::ColorImage, String>,
}

// ── filter ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum Filter { All, Picks, Unrated }

// ── app state ──────────────────────────────────────────────────────────────

pub struct CullApp {
    folder: Option<PathBuf>,
    images: Vec<ImageEntry>,

    selected: usize,
    selected_set: HashSet<usize>,
    anchor: usize,

    filter: Filter,

    thumb_textures: HashMap<usize, TextureHandle>,
    full_textures: HashMap<usize, TextureHandle>,
    loading: HashSet<(usize, LoadKind)>,

    req_tx: mpsc::SyncSender<LoadRequest>,
    res_rx: mpsc::Receiver<LoadResult>,

    status: String,
    needs_scroll: bool,
    filmstrip_vis: (usize, usize),

    /// Filmstrip panel height — managed manually to avoid egui's sticky resize.
    filmstrip_height: f32,
    /// Previous frame's window height — used to make filmstrip absorb window
    /// resize deltas so the preview stays stable.
    prev_frame_height: f32,

    /// Explorer sidebar visibility
    show_explorer: bool,
    /// Root directory for the explorer tree (parent of current folder)
    explorer_root: Option<PathBuf>,
}

impl CullApp {
    pub fn new(_cc: &eframe::CreationContext<'_>, preload: Option<PathBuf>) -> Self {
        let (req_tx, req_rx) = mpsc::sync_channel::<LoadRequest>(64);
        let (res_tx, res_rx) = mpsc::channel::<LoadResult>();

        std::thread::spawn(move || {
            for req in req_rx {
                let res_tx = res_tx.clone();
                rayon::spawn(move || {
                    let image = match req.kind {
                        LoadKind::Thumb => load_thumbnail(&req.path, req.rotation),
                        LoadKind::Full  => load_preview(&req.path, req.rotation),
                    }.map_err(|e| e.to_string());
                    let _ = res_tx.send(LoadResult { index: req.index, kind: req.kind, image });
                });
            }
        });

        let saved = SavedState::load();

        let mut app = Self {
            folder: None,
            images: Vec::new(),
            selected: 0,
            selected_set: HashSet::new(),
            anchor: 0,
            filter: Filter::All,
            thumb_textures: HashMap::new(),
            full_textures: HashMap::new(),
            loading: HashSet::new(),
            req_tx,
            res_rx,
            status: "Drop a folder here or click Open".into(),
            needs_scroll: true,
            filmstrip_vis: (0, 0),
            filmstrip_height: saved.filmstrip_height,
            prev_frame_height: 0.0,
            show_explorer: false,
            explorer_root: None,
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
        self.loading.clear();
        self.status = format!("{count} images");
        self.needs_scroll = true;

        // Explorer root = parent of current folder (shows siblings in tree)
        let explorer_root = path.parent().map(|p| p.to_path_buf()).unwrap_or_else(|| path.clone());
        self.explorer_root = Some(explorer_root);
        self.folder = Some(path);
    }

    fn visible_indices(&self) -> Vec<usize> {
        self.images
            .iter()
            .enumerate()
            .filter(|(_, img)| match self.filter {
                Filter::All     => true,
                Filter::Picks   => img.mark == Mark::Pick,
                Filter::Unrated => img.mark == Mark::None,
            })
            .map(|(i, _)| i)
            .collect()
    }

    fn request(&mut self, idx: usize, kind: LoadKind) {
        let key = (idx, kind);
        let have = match kind {
            LoadKind::Thumb => self.thumb_textures.contains_key(&idx),
            LoadKind::Full  => self.full_textures.contains_key(&idx),
        };
        if idx < self.images.len() && !have && !self.loading.contains(&key) {
            self.loading.insert(key);
            let _ = self.req_tx.try_send(LoadRequest {
                index: idx,
                path: self.images[idx].path.clone(),
                kind,
                rotation: self.images[idx].rotation,
            });
        }
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
            self.loading.remove(&(idx, LoadKind::Full));
            self.loading.remove(&(idx, LoadKind::Thumb));
        }
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
        // 1. Drain loader
        while let Ok(r) = self.res_rx.try_recv() {
            self.loading.remove(&(r.index, r.kind));
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
        let (nav_next, nav_prev, do_pick, do_reject, do_unmark, do_export,
             rotate_ccw, rotate_cw, toggle_explorer, shift, cmd) = ctx.input(|i| (
            i.key_pressed(Key::ArrowRight) || i.key_pressed(Key::ArrowDown),
            i.key_pressed(Key::ArrowLeft)  || i.key_pressed(Key::ArrowUp),
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
            if nav_next && cur + 1 < visible.len() {
                let next = visible[cur + 1];
                if shift { self.shift_select_to(next, &visible); } else { self.nav_to(next); }
            }
            if nav_prev && cur > 0 {
                let prev = visible[cur - 1];
                if shift { self.shift_select_to(prev, &visible); } else { self.nav_to(prev); }
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

        // 5. Preload
        let visible = self.visible_indices();
        let (fv_s, fv_e) = self.filmstrip_vis;
        let buf = 8;
        for i in fv_s.saturating_sub(buf)..=(fv_e + buf).min(visible.len().saturating_sub(1)) {
            if let Some(&idx) = visible.get(i) { self.request(idx, LoadKind::Thumb); }
        }
        if let Some(pos) = visible.iter().position(|&i| i == self.selected) {
            for d in 0..=4usize {
                if pos + d < visible.len() { self.request(visible[pos + d], LoadKind::Full); }
                if d > 0 && pos >= d        { self.request(visible[pos - d], LoadKind::Full); }
            }
        }

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
        let sel_n   = self.selected_set.len();
        let visible = self.visible_indices();

        egui::TopBottomPanel::top("toolbar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                // Explorer toggle
                let explorer_label = if self.show_explorer { "Hide" } else { "Files" };
                if ui.button(explorer_label).clicked() {
                    self.show_explorer = !self.show_explorer;
                }

                ui.separator();

                // Folder breadcrumb — shows current folder name, click to change
                if let Some(folder) = &self.folder.clone() {
                    let label = folder.file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("…");
                    if ui.button(format!("📁 {label}")).on_hover_text(folder.display().to_string()).clicked() {
                        if let Some(p) = rfd::FileDialog::new()
                            .set_directory(folder)
                            .pick_folder()
                        {
                            self.open_folder(p);
                        }
                    }
                } else if ui.button("📁 Open Folder").clicked() {
                    if let Some(p) = rfd::FileDialog::new().pick_folder() {
                        self.open_folder(p);
                    }
                }

                ui.separator();

                ui.selectable_value(&mut self.filter, Filter::All,     format!("All  {total}"));
                ui.selectable_value(&mut self.filter, Filter::Picks,   format!("Picks  {picks}"));
                ui.selectable_value(&mut self.filter, Filter::Unrated, format!("Unrated  {unrated}"));

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
                            if ui.small_button("⬆").on_hover_text("Go up one level").clicked() {
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

        // Manual resize separator — avoids egui's sticky resizable() behavior.
        // Paint a thin drag handle above the filmstrip panel.
        let filmstrip_h = self.filmstrip_height;
        egui::TopBottomPanel::bottom("filmstrip_resize")
            .exact_height(6.0)
            .show(ctx, |ui| {
                let response = ui.allocate_response(
                    Vec2::new(ui.available_width(), 6.0),
                    Sense::drag(),
                );
                // Subtle visual handle
                let rect = response.rect;
                ui.painter().rect_filled(rect, 0.0, Color32::from_gray(45));
                ui.painter().line_segment(
                    [rect.center_top() + egui::vec2(0.0, 2.0),
                     rect.center_top() + egui::vec2(0.0, 4.0)],
                    Stroke::new(20.0, Color32::from_gray(70)),
                );
                if response.dragged() {
                    let max_fs = (ui.ctx().screen_rect().height() - MIN_PREVIEW).max(FILMSTRIP_MIN);
                    self.filmstrip_height = (filmstrip_h - response.drag_delta().y)
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
        let is_loading = self.loading.contains(&(selected, LoadKind::Full));

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

            let rot_label = match rotation { 1 => " ↺90", 2 => " ↻180", 3 => " ↻90", _ => "" };
            let info = format!(
                "{}{}   {vis_pos}/{vis_total}   \
                 [P] pick  [X] reject  [U] unmark  [R] rotate  [←→] nav  [⌘B] files  [⌘E] export",
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
        self.loading.clear();

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
