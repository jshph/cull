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

// ── background loading ─────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum LoadKind {
    Thumb,
    Full,
}

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
pub enum Filter {
    All,
    Picks,
    Unrated,
}

// ── app state ──────────────────────────────────────────────────────────────

pub struct CullApp {
    folder: Option<PathBuf>,
    images: Vec<ImageEntry>,

    /// Cursor — the image shown in the main view.
    selected: usize,
    /// Multi-select set for batch operations. Always contains `selected`.
    selected_set: HashSet<usize>,
    /// Anchor for shift-range selection.
    anchor: usize,

    filter: Filter,

    thumb_textures: HashMap<usize, TextureHandle>,
    full_textures: HashMap<usize, TextureHandle>,
    loading: HashSet<(usize, LoadKind)>,

    req_tx: mpsc::SyncSender<LoadRequest>,
    res_rx: mpsc::Receiver<LoadResult>,

    status: String,

    /// Set when keyboard navigation changes the cursor. Cleared after one
    /// frame of scroll_to_me so manual filmstrip scrolling persists.
    needs_scroll: bool,
    /// Visible range (indices into current `visible` array) from last frame,
    /// used for scroll-aware thumb preloading.
    filmstrip_vis: (usize, usize),
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
                    }
                    .map_err(|e| e.to_string());
                    let _ = res_tx.send(LoadResult { index: req.index, kind: req.kind, image });
                });
            }
        });

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
        self.selected_set.insert(0);
        self.thumb_textures.clear();
        self.full_textures.clear();
        self.loading.clear();
        self.status = format!("{count} images");
        self.folder = Some(path);
        self.needs_scroll = true;
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

    /// Apply mark to all images in the selection set.
    fn apply_mark(&mut self, mark: Mark) {
        for idx in self.selected_set.clone() {
            self.set_mark_single(idx, mark.clone());
        }
    }

    fn rotate(&mut self, delta: i8) {
        let idx = self.selected;
        if idx >= self.images.len() { return; }
        let img = &mut self.images[idx];
        img.rotation = ((img.rotation as i8 + delta).rem_euclid(4)) as u8;
        xmp::write_rotation(&img.path.clone(), img.rotation);

        // Evict textures so they re-decode with new rotation
        self.full_textures.remove(&idx);
        self.thumb_textures.remove(&idx);
        self.loading.remove(&(idx, LoadKind::Full));
        self.loading.remove(&(idx, LoadKind::Thumb));
    }

    fn export_picks(&mut self) {
        let folder = match &self.folder {
            Some(f) => f.clone(),
            None => return,
        };
        let dest = folder.join("_picks");
        if let Err(e) = std::fs::create_dir_all(&dest) {
            self.status = format!("Export failed: {e}");
            return;
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

    /// Move cursor, clear multi-select.
    fn nav_to(&mut self, idx: usize) {
        self.selected = idx;
        self.anchor = idx;
        self.selected_set.clear();
        self.selected_set.insert(idx);
        self.needs_scroll = true;
    }

    /// Extend selection from anchor to idx (inclusive range in `visible` order).
    fn shift_select_to(&mut self, idx: usize, visible: &[usize]) {
        self.selected = idx;
        let a = visible.iter().position(|&i| i == self.anchor).unwrap_or(0);
        let b = visible.iter().position(|&i| i == idx).unwrap_or(0);
        let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
        self.selected_set.clear();
        for i in lo..=hi {
            self.selected_set.insert(visible[i]);
        }
        self.needs_scroll = true;
    }

    /// Toggle a single index in selection (⌘-click).
    fn toggle_select(&mut self, idx: usize) {
        if self.selected_set.contains(&idx) && self.selected_set.len() > 1 {
            self.selected_set.remove(&idx);
            if self.selected == idx {
                // move cursor to another member of the set
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
        // 1. Drain loader results
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

        // 2. Drag-and-drop
        let dropped = ctx.input(|i| i.raw.dropped_files.first().and_then(|f| f.path.clone()));
        if let Some(p) = dropped {
            let folder = if p.is_dir() { p } else { p.parent().unwrap_or(&p).to_path_buf() };
            self.open_folder(folder);
        }

        // 3. Keyboard input — extract everything from ctx.input in one go
        let (nav_next, nav_prev, do_pick, do_reject, do_unmark, do_export,
             rotate_ccw, rotate_cw, shift, cmd) = ctx.input(|i| (
            i.key_pressed(Key::ArrowRight) || i.key_pressed(Key::ArrowDown),
            i.key_pressed(Key::ArrowLeft)  || i.key_pressed(Key::ArrowUp),
            i.key_pressed(Key::P) || i.key_pressed(Key::Space),
            i.key_pressed(Key::X),
            i.key_pressed(Key::U),
            i.key_pressed(Key::E) && i.modifiers.command,
            i.key_pressed(Key::R) && !i.modifiers.shift,
            i.key_pressed(Key::R) && i.modifiers.shift,
            i.modifiers.shift,
            i.modifiers.command,
        ));

        // 4. Process input
        let visible = self.visible_indices();
        if !visible.is_empty() {
            let cur = visible.iter().position(|&i| i == self.selected).unwrap_or(0);

            if nav_next && cur + 1 < visible.len() {
                let next = visible[cur + 1];
                if shift {
                    self.shift_select_to(next, &visible);
                } else {
                    self.nav_to(next);
                }
            }
            if nav_prev && cur > 0 {
                let prev = visible[cur - 1];
                if shift {
                    self.shift_select_to(prev, &visible);
                } else {
                    self.nav_to(prev);
                }
            }
            if do_pick {
                let mark = if self.selected_set.iter().all(|&i| self.images[i].mark == Mark::Pick) {
                    Mark::None
                } else { Mark::Pick };
                self.apply_mark(mark);
            }
            if do_reject {
                let mark = if self.selected_set.iter().all(|&i| self.images[i].mark == Mark::Reject) {
                    Mark::None
                } else { Mark::Reject };
                self.apply_mark(mark);
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
        let ts = fv_s.saturating_sub(buf);
        let te = (fv_e + buf).min(visible.len().saturating_sub(1));
        for i in ts..=te {
            if let Some(&idx) = visible.get(i) { self.request(idx, LoadKind::Thumb); }
        }
        if let Some(pos) = visible.iter().position(|&i| i == self.selected) {
            for d in 0..=4usize {
                if pos + d < visible.len() { self.request(visible[pos + d], LoadKind::Full); }
                if d > 0 && pos >= d        { self.request(visible[pos - d], LoadKind::Full); }
            }
        }

        // 6. Render
        let visible = self.visible_indices();
        self.render_toolbar(ctx, &visible, cmd);
        self.render_filmstrip(ctx, &visible, shift, cmd);
        self.render_main(ctx, &visible);
    }
}

// ── rendering ──────────────────────────────────────────────────────────────

impl CullApp {
    fn render_toolbar(&mut self, ctx: &Context, visible: &[usize], _cmd: bool) {
        let total   = self.images.len();
        let picks   = self.images.iter().filter(|i| i.mark == Mark::Pick).count();
        let unrated = self.images.iter().filter(|i| i.mark == Mark::None).count();
        let sel_n   = self.selected_set.len();

        egui::TopBottomPanel::top("toolbar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                if ui.button("Open Folder").clicked() {
                    if let Some(p) = rfd::FileDialog::new().pick_folder() {
                        self.open_folder(p);
                    }
                }
                ui.separator();

                ui.selectable_value(&mut self.filter, Filter::All,     format!("All  {total}"));
                ui.selectable_value(&mut self.filter, Filter::Picks,   format!("Picks  {picks}"));
                ui.selectable_value(&mut self.filter, Filter::Unrated, format!("Unrated  {unrated}"));

                ui.separator();

                // Batch ops — always shown, operate on selected_set
                if sel_n > 1 {
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

    fn render_filmstrip(&mut self, ctx: &Context, visible: &[usize], shift: bool, cmd: bool) {
        // Pre-collect data for the closure
        struct TD {
            idx: usize,
            vis_pos: usize,
            is_cursor: bool,
            in_set: bool,
            mark: Mark,
            tex_id: Option<egui::TextureId>,
        }

        let td: Vec<TD> = visible.iter().enumerate().map(|(vis_pos, &idx)| TD {
            idx,
            vis_pos,
            is_cursor: idx == self.selected,
            in_set: self.selected_set.contains(&idx),
            mark: self.images[idx].mark.clone(),
            tex_id: self.thumb_textures.get(&idx)
                .or_else(|| self.full_textures.get(&idx))
                .map(|t| t.id()),
        }).collect();

        let needs_scroll = self.needs_scroll;
        let mut clicked: Option<(usize, bool, bool)> = None; // (idx, shift, cmd)
        let mut new_vis: (usize, usize) = (usize::MAX, 0);

        egui::TopBottomPanel::bottom("filmstrip")
            .resizable(true)
            .min_height(80.0)
            .default_height(108.0)
            .show(ctx, |ui| {
                // Responsive item size — scales with panel height
                let avail_h = ui.available_height() - 8.0;
                let n_rows = ((avail_h / 100.0).floor() as usize).max(1).min(4);
                let item_px = ((avail_h / n_rows as f32) - 6.0).clamp(50.0, 280.0);

                ScrollArea::horizontal()
                    .id_salt("filmstrip_scroll")
                    .show(ui, |ui| {
                        ui.horizontal_top(|ui| {
                            ui.add_space(4.0);

                            let n_cols = (td.len() + n_rows - 1) / n_rows;
                            for col in 0..n_cols {
                                ui.vertical(|ui| {
                                    for row in 0..n_rows {
                                        let item_i = col * n_rows + row;
                                        let Some(t) = td.get(item_i) else {
                                            // empty slot at end of last column
                                            ui.add_space(item_px + 4.0);
                                            continue;
                                        };

                                        let (response, painter) = ui.allocate_painter(
                                            Vec2::splat(item_px + 4.0), Sense::click(),
                                        );

                                        // Track viewport for smart preloading
                                        if response.rect.intersects(ui.clip_rect()) {
                                            new_vis.0 = new_vis.0.min(t.vis_pos);
                                            new_vis.1 = new_vis.1.max(t.vis_pos);
                                        }

                                        if response.clicked() {
                                            clicked = Some((t.idx, shift, cmd));
                                        }
                                        if t.is_cursor && needs_scroll {
                                            response.scroll_to_me(Some(Align::Center));
                                        }

                                        // Background — blue tint when in multi-select
                                        let bg = if t.in_set && !t.is_cursor {
                                            Color32::from_rgba_premultiplied(30, 55, 110, 255)
                                        } else {
                                            Color32::from_gray(20)
                                        };
                                        let rect = response.rect.shrink(2.0);
                                        painter.rect_filled(rect, 2.0, bg);

                                        // Image
                                        if let Some(tex_id) = t.tex_id {
                                            painter.image(
                                                tex_id, rect,
                                                Rect::from_min_max(
                                                    egui::pos2(0.0, 0.0),
                                                    egui::pos2(1.0, 1.0),
                                                ),
                                                Color32::WHITE,
                                            );
                                        }

                                        // Border
                                        let (bcolor, bwidth) = if t.is_cursor {
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
                                        painter.rect_stroke(
                                            response.rect.shrink(1.0), 2.0,
                                            Stroke::new(bwidth, bcolor),
                                        );

                                        // Mark badge
                                        if let Some((label, color)) = match t.mark {
                                            Mark::Pick   => Some(("P", Color32::from_rgb(72, 199, 116))),
                                            Mark::Reject => Some(("R", Color32::from_rgb(220, 80, 80))),
                                            Mark::None   => None,
                                        } {
                                            let b = Rect::from_min_size(rect.min, Vec2::new(14.0, 14.0));
                                            painter.rect_filled(b, 0.0, Color32::from_black_alpha(160));
                                            painter.text(b.center(), egui::Align2::CENTER_CENTER, label, FontId::proportional(10.0), color);
                                        }

                                        ui.add_space(2.0);
                                    }
                                });
                                ui.add_space(2.0);
                            }
                        });
                    });
            });

        // Process clicks after the panel (no borrow conflict)
        if let Some((idx, is_shift, is_cmd)) = clicked {
            let vis = self.visible_indices();
            if is_shift {
                self.shift_select_to(idx, &vis);
            } else if is_cmd {
                self.toggle_select(idx);
            } else {
                self.nav_to(idx);
            }
            // Don't scroll on click — the clicked item is already visible
            self.needs_scroll = false;
        }

        if new_vis.0 != usize::MAX {
            self.filmstrip_vis = new_vis;
        }
        self.needs_scroll = false;
    }

    fn render_main(&mut self, ctx: &Context, visible: &[usize]) {
        let is_empty    = self.images.is_empty();
        let no_visible  = visible.is_empty();
        let selected    = self.selected;
        let filename    = self.images.get(selected).map(|e| e.filename().to_string());
        let mark        = self.images.get(selected).map(|e| e.mark.clone());
        let rotation    = self.images.get(selected).map(|e| e.rotation).unwrap_or(0);
        let vis_pos     = visible.iter().position(|&i| i == selected).map(|p| p + 1).unwrap_or(1);
        let vis_total   = visible.len();

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

            // Bottom bar overlay
            let rot_label = match rotation { 1 => " ↺90", 2 => " ↻180", 3 => " ↻90", _ => "" };
            let info = format!(
                "{}{}   {vis_pos}/{vis_total}   \
                 [P] pick  [X] reject  [U] unmark  [R] rotate  [←→] nav  [⌘E] export",
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

            // Mark badge
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
}
