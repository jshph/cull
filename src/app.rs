use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::mpsc;

use egui::{
    Align, Color32, Context, FontId, Key, Rect, ScrollArea, Sense, Stroke, TextureHandle,
    TextureOptions, Vec2,
};

use crate::catalog::{load_folder, ImageEntry, Mark};
use crate::preview::{load_preview, load_thumbnail};
use crate::xmp::write_mark;

// ── background loading ─────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum LoadKind {
    /// Small texture (300px) for filmstrip. Fast GPU upload, loads first.
    Thumb,
    /// Full texture (2400px) for main view.
    Full,
}

struct LoadRequest {
    index: usize,
    path: PathBuf,
    kind: LoadKind,
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
    selected: usize,
    filter: Filter,

    /// Small textures used in the filmstrip.
    thumb_textures: HashMap<usize, TextureHandle>,
    /// Full-res textures used in the main view.
    full_textures: HashMap<usize, TextureHandle>,
    loading: HashSet<(usize, LoadKind)>,

    req_tx: mpsc::SyncSender<LoadRequest>,
    res_rx: mpsc::Receiver<LoadResult>,

    status: String,

    /// Visible range (indices into the current `visible` array) of filmstrip
    /// items that were actually in the viewport last frame. Used to restrict
    /// thumb loading to only what the user can see + a small buffer.
    filmstrip_vis: (usize, usize),
}

impl CullApp {
    pub fn new(_cc: &eframe::CreationContext<'_>, preload: Option<PathBuf>) -> Self {
        // Bounded channel: if the queue fills up we drop stale requests rather
        // than piling up work for images the user has already scrolled past.
        let (req_tx, req_rx) = mpsc::sync_channel::<LoadRequest>(64);
        let (res_tx, res_rx) = mpsc::channel::<LoadResult>();

        // Coordinator thread: pulls requests off the queue and dispatches each
        // to the rayon thread pool so multiple images decode in parallel.
        std::thread::spawn(move || {
            for req in req_rx {
                let res_tx = res_tx.clone();
                rayon::spawn(move || {
                    let image = match req.kind {
                        LoadKind::Thumb => load_thumbnail(&req.path),
                        LoadKind::Full => load_preview(&req.path),
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
            filter: Filter::All,
            thumb_textures: HashMap::new(),
            full_textures: HashMap::new(),
            loading: HashSet::new(),
            req_tx,
            res_rx,
            status: "Drop a folder here or click Open".into(),
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
        self.thumb_textures.clear();
        self.full_textures.clear();
        self.loading.clear();
        self.status = format!("{count} images");
        self.folder = Some(path);
    }

    fn visible_indices(&self) -> Vec<usize> {
        self.images
            .iter()
            .enumerate()
            .filter(|(_, img)| match self.filter {
                Filter::All => true,
                Filter::Picks => img.mark == Mark::Pick,
                Filter::Unrated => img.mark == Mark::None,
            })
            .map(|(i, _)| i)
            .collect()
    }

    fn request(&mut self, idx: usize, kind: LoadKind) {
        let key = (idx, kind);
        let already_have = match kind {
            LoadKind::Thumb => self.thumb_textures.contains_key(&idx),
            LoadKind::Full => self.full_textures.contains_key(&idx),
        };
        if idx < self.images.len() && !already_have && !self.loading.contains(&key) {
            self.loading.insert(key);
            // try_send: if the queue is full, silently skip — we'll retry next frame
            let _ = self.req_tx.try_send(LoadRequest {
                index: idx,
                path: self.images[idx].path.clone(),
                kind,
            });
        }
    }

    fn set_mark(&mut self, idx: usize, mark: Mark) {
        if idx < self.images.len() {
            write_mark(&self.images[idx].path.clone(), &mark);
            self.images[idx].mark = mark;
        }
    }

    fn export_picks(&mut self) {
        let folder = match &self.folder {
            Some(f) => f.clone(),
            None => return,
        };

        let dest_dir = folder.join("_picks");
        if let Err(e) = std::fs::create_dir_all(&dest_dir) {
            self.status = format!("Export failed: {e}");
            return;
        }

        let mut count = 0usize;
        for img in self.images.iter().filter(|i| i.mark == Mark::Pick) {
            if let Some(name) = img.path.file_name() {
                if std::fs::copy(&img.path, dest_dir.join(name)).is_ok() {
                    count += 1;
                }
            }
        }
        self.status = format!("Exported {count} picks → _picks/");
    }
}

// ── eframe::App ────────────────────────────────────────────────────────────

impl eframe::App for CullApp {
    fn update(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        // 1. Drain loader results
        while let Ok(result) = self.res_rx.try_recv() {
            self.loading.remove(&(result.index, result.kind));
            if let Ok(img) = result.image {
                let tex = ctx.load_texture(
                    format!("img_{}_{}", result.index, result.kind as u8),
                    img,
                    TextureOptions::LINEAR,
                );
                match result.kind {
                    LoadKind::Thumb => { self.thumb_textures.insert(result.index, tex); }
                    LoadKind::Full  => { self.full_textures.insert(result.index, tex); }
                }
                ctx.request_repaint();
            }
        }

        // 2. Drag-and-drop
        let dropped_path = ctx.input(|i| i.raw.dropped_files.first().and_then(|f| f.path.clone()));
        if let Some(path) = dropped_path {
            let folder = if path.is_dir() { path } else { path.parent().unwrap_or(&path).to_path_buf() };
            self.open_folder(folder);
        }

        // 3. Keyboard input
        let (nav_next, nav_prev, do_pick, do_reject, do_unmark, do_export) = ctx.input(|i| (
            i.key_pressed(Key::ArrowRight) || i.key_pressed(Key::ArrowDown),
            i.key_pressed(Key::ArrowLeft)  || i.key_pressed(Key::ArrowUp),
            i.key_pressed(Key::P) || i.key_pressed(Key::Space),
            i.key_pressed(Key::X),
            i.key_pressed(Key::U),
            i.key_pressed(Key::E) && i.modifiers.command,
        ));

        // 4. Process input
        let visible = self.visible_indices();
        if !visible.is_empty() {
            let cur = visible.iter().position(|&i| i == self.selected).unwrap_or(0);
            if nav_next && cur + 1 < visible.len() { self.selected = visible[cur + 1]; }
            if nav_prev && cur > 0                 { self.selected = visible[cur - 1]; }
            if do_pick {
                let m = if self.images[self.selected].mark == Mark::Pick { Mark::None } else { Mark::Pick };
                self.set_mark(self.selected, m);
            }
            if do_reject {
                let m = if self.images[self.selected].mark == Mark::Reject { Mark::None } else { Mark::Reject };
                self.set_mark(self.selected, m);
            }
            if do_unmark { self.set_mark(self.selected, Mark::None); }
        }
        if do_export { self.export_picks(); }

        // 5. Preloading strategy:
        //    • Thumbs: only for items visible in the filmstrip viewport ±8 buffer.
        //      filmstrip_vis is updated by render_filmstrip each frame from actual
        //      clip rect intersection — so we only load what the user can see.
        //    • Full: selected ±4 only (expensive — keep queue tight).
        let visible = self.visible_indices();
        let (fv_start, fv_end) = self.filmstrip_vis;
        let buf = 8usize;
        let thumb_start = fv_start.saturating_sub(buf);
        let thumb_end   = (fv_end + buf).min(visible.len().saturating_sub(1));
        for i in thumb_start..=thumb_end {
            if let Some(&idx) = visible.get(i) {
                self.request(idx, LoadKind::Thumb);
            }
        }
        if let Some(pos) = visible.iter().position(|&i| i == self.selected) {
            for delta in 0..=4usize {
                if pos + delta < visible.len() { self.request(visible[pos + delta], LoadKind::Full); }
                if delta > 0 && pos >= delta   { self.request(visible[pos - delta], LoadKind::Full); }
            }
        }

        // 6. Render
        let visible = self.visible_indices();
        self.render_toolbar(ctx, &visible);
        self.render_filmstrip(ctx, &visible);
        self.render_main(ctx, &visible);
    }
}

// ── rendering ──────────────────────────────────────────────────────────────

impl CullApp {
    fn render_toolbar(&mut self, ctx: &Context, visible: &[usize]) {
        let total   = self.images.len();
        let picks   = self.images.iter().filter(|i| i.mark == Mark::Pick).count();
        let unrated = self.images.iter().filter(|i| i.mark == Mark::None).count();

        egui::TopBottomPanel::top("toolbar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                if ui.button("Open Folder").clicked() {
                    if let Some(path) = rfd::FileDialog::new().pick_folder() {
                        self.open_folder(path);
                    }
                }

                ui.separator();
                ui.selectable_value(&mut self.filter, Filter::All,     format!("All  {total}"));
                ui.selectable_value(&mut self.filter, Filter::Picks,   format!("Picks  {picks}"));
                ui.selectable_value(&mut self.filter, Filter::Unrated, format!("Unrated  {unrated}"));
                ui.separator();
                ui.label(&self.status);

                ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                    if ui.add_enabled(picks > 0, egui::Button::new(format!("Export {picks} Picks"))).clicked() {
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

    fn render_filmstrip(&mut self, ctx: &Context, visible: &[usize]) {
        struct ThumbData {
            idx: usize,
            is_selected: bool,
            mark: Mark,
            tex_id: Option<egui::TextureId>,
        }

        let thumb_data: Vec<ThumbData> = visible.iter().map(|&idx| ThumbData {
            idx,
            is_selected: idx == self.selected,
            mark: self.images[idx].mark.clone(),
            // Prefer thumb texture; fall back to full if thumb not ready yet
            tex_id: self.thumb_textures.get(&idx)
                .or_else(|| self.full_textures.get(&idx))
                .map(|t| t.id()),
        }).collect();

        let mut clicked: Option<usize> = None;
        let mut new_vis: (usize, usize) = (usize::MAX, 0); // (first, last) in thumb_data index

        egui::TopBottomPanel::bottom("filmstrip")
            .exact_height(104.0)
            .show(ctx, |ui| {
                ScrollArea::horizontal()
                    .id_salt("filmstrip_scroll")
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.add_space(4.0);
                            for (td_pos, td) in thumb_data.iter().enumerate() {
                                let border_color = match (&td.mark, td.is_selected) {
                                    (Mark::Pick,   _)    => Color32::from_rgb(72, 199, 116),
                                    (Mark::Reject, _)    => Color32::from_rgb(220, 80, 80),
                                    (_,            true) => Color32::WHITE,
                                    (_,            false)=> Color32::from_gray(50),
                                };
                                let border_width = if td.is_selected { 2.5 } else { 1.5 };

                                let (response, painter) = ui.allocate_painter(Vec2::splat(88.0), Sense::click());

                                // Track viewport visibility for smart preloading
                                if response.rect.intersects(ui.clip_rect()) {
                                    new_vis.0 = new_vis.0.min(td_pos);
                                    new_vis.1 = new_vis.1.max(td_pos);
                                }

                                if response.clicked() { clicked = Some(td.idx); }
                                if td.is_selected { response.scroll_to_me(Some(Align::Center)); }

                                let rect = response.rect.shrink(2.0);
                                painter.rect_filled(rect, 2.0, Color32::from_gray(20));
                                painter.rect_stroke(
                                    response.rect.shrink(1.0), 2.0,
                                    Stroke::new(border_width, border_color),
                                );

                                if let Some(tex_id) = td.tex_id {
                                    painter.image(
                                        tex_id, rect,
                                        Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                                        Color32::WHITE,
                                    );
                                }

                                if let Some((label, color)) = match td.mark {
                                    Mark::Pick   => Some(("P", Color32::from_rgb(72, 199, 116))),
                                    Mark::Reject => Some(("R", Color32::from_rgb(220, 80, 80))),
                                    Mark::None   => None,
                                } {
                                    let badge = Rect::from_min_size(rect.min, Vec2::new(14.0, 14.0));
                                    painter.rect_filled(badge, 0.0, Color32::from_black_alpha(160));
                                    painter.text(badge.center(), egui::Align2::CENTER_CENTER, label, FontId::proportional(10.0), color);
                                }

                                ui.add_space(2.0);
                            }
                        });
                    });
            });

        if let Some(idx) = clicked { self.selected = idx; }

        // Persist viewport range for next-frame preloading.
        // If nothing was visible (empty folder / filter), keep previous range.
        if new_vis.0 != usize::MAX {
            self.filmstrip_vis = new_vis;
        }
    }

    fn render_main(&mut self, ctx: &Context, visible: &[usize]) {
        let is_empty    = self.images.is_empty();
        let no_visible  = visible.is_empty();
        let selected    = self.selected;
        let filename    = self.images.get(selected).map(|e| e.filename().to_string());
        let mark        = self.images.get(selected).map(|e| e.mark.clone());
        let vis_pos     = visible.iter().position(|&i| i == selected).map(|p| p + 1).unwrap_or(1);
        let vis_total   = visible.len();

        // Progressive: prefer full texture; fall back to thumb while full loads.
        // The user sees something immediately, then it sharpens up.
        let tex_info = self.full_textures.get(&selected)
            .or_else(|| self.thumb_textures.get(&selected))
            .map(|t| (t.id(), t.size_vec2()));
        let is_thumb_only = tex_info.is_some()
            && self.full_textures.get(&selected).is_none();
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

            let painter   = ui.painter();
            let panel_rect = ui.max_rect();

            // Status bar overlay
            let info = format!(
                "{}   {vis_pos}/{vis_total}   [P] pick   [X] reject   [U] unmark   [←→] navigate   [⌘E] export",
                filename.as_deref().unwrap_or("")
            );
            painter.rect_filled(
                Rect::from_min_max(egui::pos2(panel_rect.left(), panel_rect.bottom() - 26.0), panel_rect.right_bottom()),
                0.0, Color32::from_black_alpha(180),
            );
            painter.text(
                egui::pos2(panel_rect.left() + 10.0, panel_rect.bottom() - 13.0),
                egui::Align2::LEFT_CENTER, &info,
                FontId::proportional(12.0), Color32::from_gray(200),
            );

            // "Loading…" shimmer badge when showing thumb while full decodes
            if is_thumb_only {
                painter.text(
                    egui::pos2(panel_rect.left() + 10.0, panel_rect.top() + 20.0),
                    egui::Align2::LEFT_TOP, "loading…",
                    FontId::proportional(12.0), Color32::from_gray(140),
                );
            }

            // Pick / reject badge
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
