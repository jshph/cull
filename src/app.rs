use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::mpsc;

use egui::{
    Align, Color32, Context, FontId, Key, Rect, ScrollArea, Sense, Stroke, TextureHandle,
    TextureOptions, Vec2,
};

use crate::catalog::{load_folder, ImageEntry, Mark};
use crate::preview::load_preview;
use crate::xmp::write_mark;

// ── background loading ─────────────────────────────────────────────────────

struct LoadRequest {
    index: usize,
    path: PathBuf,
}

struct LoadResult {
    index: usize,
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

    textures: HashMap<usize, TextureHandle>,
    loading: HashSet<usize>,
    req_tx: mpsc::Sender<LoadRequest>,
    res_rx: mpsc::Receiver<LoadResult>,

    status: String,
}

impl CullApp {
    pub fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        let (req_tx, req_rx) = mpsc::channel::<LoadRequest>();
        let (res_tx, res_rx) = mpsc::channel::<LoadResult>();

        // Single background thread — IO-bound, one at a time is fine
        std::thread::spawn(move || {
            for req in req_rx {
                let image = load_preview(&req.path).map_err(|e| e.to_string());
                let _ = res_tx.send(LoadResult { index: req.index, image });
            }
        });

        Self {
            folder: None,
            images: Vec::new(),
            selected: 0,
            filter: Filter::All,
            textures: HashMap::new(),
            loading: HashSet::new(),
            req_tx,
            res_rx,
            status: "Drop a folder here or click Open".into(),
        }
    }

    fn open_folder(&mut self, path: PathBuf) {
        let images = load_folder(&path);
        let count = images.len();
        self.images = images;
        self.selected = 0;
        self.textures.clear();
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

    fn request_texture(&mut self, idx: usize) {
        if idx < self.images.len()
            && !self.textures.contains_key(&idx)
            && !self.loading.contains(&idx)
        {
            self.loading.insert(idx);
            let _ = self.req_tx.send(LoadRequest {
                index: idx,
                path: self.images[idx].path.clone(),
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

        let picks_dir = folder.join("_picks");
        if let Err(e) = std::fs::create_dir_all(&picks_dir) {
            self.status = format!("Export failed: {e}");
            return;
        }

        let mut count = 0usize;
        for img in self.images.iter().filter(|i| i.mark == Mark::Pick) {
            if let Some(name) = img.path.file_name() {
                let dest = picks_dir.join(name);
                if std::fs::copy(&img.path, dest).is_ok() {
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
        // 1. Drain background loader results
        while let Ok(result) = self.res_rx.try_recv() {
            self.loading.remove(&result.index);
            if let Ok(img) = result.image {
                let tex = ctx.load_texture(
                    format!("img_{}", result.index),
                    img,
                    TextureOptions::LINEAR,
                );
                self.textures.insert(result.index, tex);
                ctx.request_repaint();
            }
        }

        // 2. Drag-and-drop folder
        let dropped_path = ctx.input(|i| {
            i.raw.dropped_files.first().and_then(|f| f.path.clone())
        });
        if let Some(path) = dropped_path {
            let folder = if path.is_dir() {
                path
            } else {
                path.parent().unwrap_or(&path).to_path_buf()
            };
            self.open_folder(folder);
        }

        // 3. Collect keyboard input (must extract before rendering to avoid re-borrow)
        let (nav_next, nav_prev, do_pick, do_reject, do_unmark, do_export) =
            ctx.input(|i| {
                (
                    i.key_pressed(Key::ArrowRight) || i.key_pressed(Key::ArrowDown),
                    i.key_pressed(Key::ArrowLeft) || i.key_pressed(Key::ArrowUp),
                    i.key_pressed(Key::P) || i.key_pressed(Key::Space),
                    i.key_pressed(Key::X),
                    i.key_pressed(Key::U),
                    i.key_pressed(Key::E) && i.modifiers.command,
                )
            });

        // 4. Process input
        let visible = self.visible_indices();
        if !visible.is_empty() {
            let cur_pos = visible
                .iter()
                .position(|&i| i == self.selected)
                .unwrap_or(0);

            if nav_next && cur_pos + 1 < visible.len() {
                self.selected = visible[cur_pos + 1];
            }
            if nav_prev && cur_pos > 0 {
                self.selected = visible[cur_pos - 1];
            }
            if do_pick {
                let mark = if self.images[self.selected].mark == Mark::Pick {
                    Mark::None
                } else {
                    Mark::Pick
                };
                self.set_mark(self.selected, mark);
            }
            if do_reject {
                let mark = if self.images[self.selected].mark == Mark::Reject {
                    Mark::None
                } else {
                    Mark::Reject
                };
                self.set_mark(self.selected, mark);
            }
            if do_unmark {
                self.set_mark(self.selected, Mark::None);
            }
        }
        if do_export {
            self.export_picks();
        }

        // 5. Preload: selected + ±8 neighbours
        let visible = self.visible_indices();
        if let Some(pos) = visible.iter().position(|&i| i == self.selected) {
            for delta in 0..=8usize {
                if pos + delta < visible.len() {
                    self.request_texture(visible[pos + delta]);
                }
                if delta > 0 && pos >= delta {
                    self.request_texture(visible[pos - delta]);
                }
            }
        }

        // 6. Render — pre-collect what closures need so there's no &mut self borrow conflict
        let visible = self.visible_indices();
        self.render_toolbar(ctx, &visible);
        self.render_filmstrip(ctx, &visible);
        self.render_main(ctx, &visible);
    }
}

// ── rendering ──────────────────────────────────────────────────────────────

impl CullApp {
    fn render_toolbar(&mut self, ctx: &Context, visible: &[usize]) {
        let total = self.images.len();
        let picks = self.images.iter().filter(|i| i.mark == Mark::Pick).count();
        let unrated = self.images.iter().filter(|i| i.mark == Mark::None).count();
        let vis_count = visible.len();

        egui::TopBottomPanel::top("toolbar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                if ui.button("Open Folder").clicked() {
                    if let Some(path) = rfd::FileDialog::new().pick_folder() {
                        self.open_folder(path);
                    }
                }

                ui.separator();

                ui.selectable_value(&mut self.filter, Filter::All, format!("All  {total}"));
                ui.selectable_value(&mut self.filter, Filter::Picks, format!("Picks  {picks}"));
                ui.selectable_value(
                    &mut self.filter,
                    Filter::Unrated,
                    format!("Unrated  {unrated}"),
                );

                ui.separator();
                ui.label(&self.status);

                ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                    let enabled = picks > 0;
                    if ui
                        .add_enabled(enabled, egui::Button::new(format!("Export {picks} Picks")))
                        .clicked()
                    {
                        self.export_picks();
                    }

                    if !visible.is_empty() {
                        let pos = visible
                            .iter()
                            .position(|&i| i == self.selected)
                            .map(|p| p + 1)
                            .unwrap_or(1);
                        ui.label(format!("{pos} / {vis_count}"));
                    }
                });
            });
        });
    }

    fn render_filmstrip(&mut self, ctx: &Context, visible: &[usize]) {
        // Pre-collect everything the closure needs — avoids &mut self in closure
        struct ThumbData {
            idx: usize,
            is_selected: bool,
            mark: Mark,
            tex_id: Option<egui::TextureId>,
        }

        let thumb_data: Vec<ThumbData> = visible
            .iter()
            .map(|&idx| ThumbData {
                idx,
                is_selected: idx == self.selected,
                mark: self.images[idx].mark.clone(),
                tex_id: self.textures.get(&idx).map(|t| t.id()),
            })
            .collect();

        let mut clicked: Option<usize> = None;

        egui::TopBottomPanel::bottom("filmstrip")
            .exact_height(104.0)
            .show(ctx, |ui| {
                ScrollArea::horizontal()
                    .id_salt("filmstrip_scroll")
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.add_space(4.0);
                            for td in &thumb_data {
                                let border_color = match (&td.mark, td.is_selected) {
                                    (Mark::Pick, _) => Color32::from_rgb(72, 199, 116),
                                    (Mark::Reject, _) => Color32::from_rgb(220, 80, 80),
                                    (_, true) => Color32::WHITE,
                                    (_, false) => Color32::from_gray(50),
                                };
                                let border_width = if td.is_selected { 2.5 } else { 1.5 };

                                let (response, painter) = ui.allocate_painter(
                                    Vec2::splat(88.0),
                                    Sense::click(),
                                );

                                if response.clicked() {
                                    clicked = Some(td.idx);
                                }
                                if td.is_selected {
                                    response.scroll_to_me(Some(Align::Center));
                                }

                                let rect = response.rect.shrink(2.0);
                                painter.rect_filled(rect, 2.0, Color32::from_gray(20));
                                painter.rect_stroke(
                                    response.rect.shrink(1.0),
                                    2.0,
                                    Stroke::new(border_width, border_color),
                                );

                                if let Some(tex_id) = td.tex_id {
                                    painter.image(
                                        tex_id,
                                        rect,
                                        Rect::from_min_max(
                                            egui::pos2(0.0, 0.0),
                                            egui::pos2(1.0, 1.0),
                                        ),
                                        Color32::WHITE,
                                    );
                                }

                                // Mark badge on thumbnail
                                let badge = match td.mark {
                                    Mark::Pick => Some(("P", Color32::from_rgb(72, 199, 116))),
                                    Mark::Reject => Some(("R", Color32::from_rgb(220, 80, 80))),
                                    Mark::None => None,
                                };
                                if let Some((label, color)) = badge {
                                    let badge_rect = Rect::from_min_size(
                                        rect.min,
                                        Vec2::new(14.0, 14.0),
                                    );
                                    painter.rect_filled(badge_rect, 0.0, Color32::from_black_alpha(160));
                                    painter.text(
                                        badge_rect.center(),
                                        egui::Align2::CENTER_CENTER,
                                        label,
                                        FontId::proportional(10.0),
                                        color,
                                    );
                                }

                                ui.add_space(2.0);
                            }
                        });
                    });
            });

        if let Some(idx) = clicked {
            self.selected = idx;
        }
    }

    fn render_main(&mut self, ctx: &Context, visible: &[usize]) {
        // Pre-collect to avoid borrow in closure
        let is_empty = self.images.is_empty();
        let no_visible = visible.is_empty();
        let selected = self.selected;
        let filename = self.images.get(selected).map(|e| e.filename().to_string());
        let mark = self.images.get(selected).map(|e| e.mark.clone());
        let is_loading = self.loading.contains(&selected);
        let tex_info = self.textures.get(&selected).map(|t| (t.id(), t.size_vec2()));
        let vis_pos = visible.iter().position(|&i| i == selected).map(|p| p + 1).unwrap_or(1);
        let vis_total = visible.len();

        egui::CentralPanel::default().show(ctx, |ui| {
            if is_empty {
                ui.centered_and_justified(|ui| {
                    ui.heading("Drop a folder of RAW files here");
                    ui.label("Supports CR2, CR3, NEF, ARW, DNG, ORF, RAF, RW2, JPEG");
                });
                return;
            }
            if no_visible {
                ui.centered_and_justified(|ui| {
                    ui.label("No images match the current filter.");
                });
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
                        ui.add(
                            egui::Image::new((tex_id, img_size))
                                .maintain_aspect_ratio(true),
                        );
                    });
                }
                None => {
                    ui.centered_and_justified(|ui| {
                        if is_loading {
                            ui.spinner();
                        } else {
                            ui.label("Failed to load preview");
                        }
                    });
                }
            }

            // Overlay — filename + position + shortcuts
            let painter = ui.painter();
            let panel_rect = ui.max_rect();

            let info = format!(
                "{}   {vis_pos}/{vis_total}   \
                 [P] pick   [X] reject   [U] unmark   [←→] navigate   [⌘E] export",
                filename.as_deref().unwrap_or("")
            );

            painter.rect_filled(
                Rect::from_min_max(
                    egui::pos2(panel_rect.left(), panel_rect.bottom() - 26.0),
                    panel_rect.right_bottom(),
                ),
                0.0,
                Color32::from_black_alpha(180),
            );
            painter.text(
                egui::pos2(panel_rect.left() + 10.0, panel_rect.bottom() - 13.0),
                egui::Align2::LEFT_CENTER,
                &info,
                FontId::proportional(12.0),
                Color32::from_gray(200),
            );

            // Pick / reject badge top-right
            if let Some(mark) = mark {
                let (badge_text, badge_color) = match mark {
                    Mark::Pick => ("PICK", Color32::from_rgb(72, 199, 116)),
                    Mark::Reject => ("REJECT", Color32::from_rgb(220, 80, 80)),
                    Mark::None => ("", Color32::TRANSPARENT),
                };
                if !badge_text.is_empty() {
                    let badge_pos = egui::pos2(panel_rect.right() - 12.0, panel_rect.top() + 20.0);
                    painter.text(
                        badge_pos,
                        egui::Align2::RIGHT_TOP,
                        badge_text,
                        FontId::proportional(18.0),
                        badge_color,
                    );
                }
            }
        });
    }
}
