//! The viewer UI: full-screen image area plus a thumbnail column on the left.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use egui::{
    pos2, vec2, Align2, Color32, FontId, Key, Rect, Sense, TextureHandle, TextureOptions,
    ViewportCommand,
};

use crate::source::{self, Page};
use crate::worker::{Output, Workers};

const SIDEBAR_WIDTH: f32 = 190.0;
const THUMB_BOX: f32 = 140.0;
const LABEL_HEIGHT: f32 = 20.0;
const ROW_PADDING: f32 = 8.0;
const ROW_HEIGHT: f32 = THUMB_BOX + LABEL_HEIGHT + ROW_PADDING * 2.0;
/// How many pages either side of the current one are kept decoded on the GPU.
const KEEP_RADIUS: usize = 2;

const TEXTURE_OPTIONS: TextureOptions = TextureOptions {
    magnification: egui::TextureFilter::Linear,
    minification: egui::TextureFilter::Linear,
    wrap_mode: egui::TextureWrapMode::ClampToEdge,
    mipmap_mode: Some(egui::TextureFilter::Linear),
};

enum Thumb {
    NotRequested,
    Pending,
    Ready(TextureHandle),
    Failed,
}

/// A decoded page uploaded to the GPU.
struct Loaded {
    frames: Vec<(TextureHandle, Duration)>,
    original_size: [u32; 2],
}

enum Slot {
    Loaded(Loaded),
    Failed(String),
}

struct Animation {
    index: usize,
    frame: usize,
    frame_started: Instant,
}

pub struct LiteView {
    folder: PathBuf,
    pages: Arc<Vec<Page>>,
    workers: Workers,
    current: usize,
    cache: HashMap<usize, Slot>,
    thumbs: Vec<Thumb>,
    anim: Animation,
    show_sidebar: bool,
    scroll_to_current: bool,
    last_wheel_nav: Instant,
}

impl LiteView {
    pub fn new(cc: &eframe::CreationContext<'_>, folder: PathBuf) -> Self {
        cc.egui_ctx.set_visuals(egui::Visuals::dark());
        let pages = Arc::new(source::scan(&folder));
        let mut app = LiteView {
            workers: Workers::new(pages.clone(), cc.egui_ctx.clone()),
            thumbs: (0..pages.len()).map(|_| Thumb::NotRequested).collect(),
            pages,
            folder,
            current: 0,
            cache: HashMap::new(),
            anim: Animation { index: usize::MAX, frame: 0, frame_started: Instant::now() },
            show_sidebar: true,
            scroll_to_current: true,
            last_wheel_nav: Instant::now(),
        };
        app.go_to(&cc.egui_ctx, 0);
        app
    }

    fn open_folder(&mut self, ctx: &egui::Context, folder: PathBuf) {
        let pages = Arc::new(source::scan(&folder));
        // Dropping the old pool shuts its threads down.
        self.workers = Workers::new(pages.clone(), ctx.clone());
        self.thumbs = (0..pages.len()).map(|_| Thumb::NotRequested).collect();
        self.pages = pages;
        self.folder = folder;
        self.cache.clear();
        self.anim.index = usize::MAX;
        self.current = 0;
        self.go_to(ctx, 0);
    }

    fn go_to(&mut self, ctx: &egui::Context, index: usize) {
        let n = self.pages.len();
        if n == 0 {
            ctx.send_viewport_cmd(ViewportCommand::Title(format!(
                "LiteView — {}",
                self.folder.display()
            )));
            return;
        }
        let index = index.min(n - 1);
        if index != self.current {
            self.scroll_to_current = true;
        }
        self.current = index;

        // Free pages that are far away, then queue the current page and its neighbours.
        self.cache
            .retain(|&i, _| i.abs_diff(index) <= KEEP_RADIUS);
        let mut wanted = vec![index];
        for d in 1..=KEEP_RADIUS {
            if index + d < n {
                wanted.push(index + d);
            }
            if let Some(i) = index.checked_sub(d) {
                wanted.push(i);
            }
        }
        wanted.retain(|i| !self.cache.contains_key(i));
        self.workers.request_full(&wanted);

        ctx.send_viewport_cmd(ViewportCommand::Title(format!(
            "LiteView — {} ({}/{})",
            self.pages[index].display_name,
            index + 1,
            n
        )));
    }

    fn receive_results(&mut self, ctx: &egui::Context) {
        while let Ok(out) = self.workers.results.try_recv() {
            match out {
                Output::Full { index, result } => {
                    if index.abs_diff(self.current) > KEEP_RADIUS || self.cache.contains_key(&index)
                    {
                        continue;
                    }
                    let slot = match result {
                        Ok(decoded) => Slot::Loaded(Loaded {
                            original_size: decoded.original_size,
                            frames: decoded
                                .frames
                                .into_iter()
                                .enumerate()
                                .map(|(f, frame)| {
                                    let tex = ctx.load_texture(
                                        format!("page-{index}-{f}"),
                                        frame.image,
                                        TEXTURE_OPTIONS,
                                    );
                                    (tex, frame.delay)
                                })
                                .collect(),
                        }),
                        Err(e) => Slot::Failed(e),
                    };
                    self.cache.insert(index, slot);
                }
                Output::Thumb { index, result } => {
                    if let Some(t) = self.thumbs.get_mut(index) {
                        *t = match result {
                            Ok(img) => Thumb::Ready(ctx.load_texture(
                                format!("thumb-{index}"),
                                img,
                                TEXTURE_OPTIONS,
                            )),
                            Err(_) => Thumb::Failed,
                        };
                    }
                }
            }
        }
    }

    fn handle_keys(&mut self, ctx: &egui::Context) {
        let n = self.pages.len();
        let (next, prev, first, last, pg_down, pg_up, esc, tab, f11, open) = ctx.input(|i| {
            (
                i.key_pressed(Key::ArrowRight),
                i.key_pressed(Key::ArrowLeft),
                i.key_pressed(Key::Home),
                i.key_pressed(Key::End),
                i.key_pressed(Key::PageDown),
                i.key_pressed(Key::PageUp),
                i.key_pressed(Key::Escape),
                i.key_pressed(Key::Tab),
                i.key_pressed(Key::F11),
                i.key_pressed(Key::O),
            )
        });
        if esc {
            ctx.send_viewport_cmd(ViewportCommand::Close);
            return;
        }
        if tab {
            self.show_sidebar = !self.show_sidebar;
            self.scroll_to_current = true;
        }
        if f11 {
            let fs = ctx.input(|i| i.viewport().fullscreen.unwrap_or(false));
            ctx.send_viewport_cmd(ViewportCommand::Fullscreen(!fs));
        }
        if open {
            let dialog = rfd::FileDialog::new()
                .set_title("LiteView — choose a folder of images")
                .set_directory(&self.folder);
            if let Some(folder) = dialog.pick_folder() {
                self.open_folder(ctx, folder);
            }
            return;
        }
        if n == 0 {
            return;
        }
        let cur = self.current;
        let target = if next {
            Some((cur + 1).min(n - 1))
        } else if prev {
            Some(cur.saturating_sub(1))
        } else if first {
            Some(0)
        } else if last {
            Some(n - 1)
        } else if pg_down {
            Some((cur + 10).min(n - 1))
        } else if pg_up {
            Some(cur.saturating_sub(10))
        } else {
            None
        };
        if let Some(t) = target {
            if t != cur {
                self.go_to(ctx, t);
            }
        }
    }

    fn sidebar(&mut self, ctx: &egui::Context) {
        let mut clicked = None;
        egui::SidePanel::left("thumbnails")
            .resizable(false)
            .exact_width(SIDEBAR_WIDTH)
            .frame(egui::Frame::new().fill(Color32::from_gray(24)))
            .show(ctx, |ui| {
                ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
                let mut area = egui::ScrollArea::vertical().auto_shrink([false, false]);
                if self.scroll_to_current {
                    let view_h = ui.available_height();
                    let y = self.current as f32 * ROW_HEIGHT - (view_h - ROW_HEIGHT) / 2.0;
                    area = area.vertical_scroll_offset(y.max(0.0));
                    self.scroll_to_current = false;
                }
                area.show_rows(ui, ROW_HEIGHT, self.pages.len(), |ui, range| {
                    for i in range {
                        let width = ui.available_width();
                        let (rect, resp) =
                            ui.allocate_exact_size(vec2(width, ROW_HEIGHT), Sense::click());
                        let selected = i == self.current;
                        if selected || resp.hovered() {
                            let fill = if selected {
                                Color32::from_rgb(40, 90, 160)
                            } else {
                                Color32::from_gray(45)
                            };
                            ui.painter().rect_filled(rect.shrink(3.0), 4.0, fill);
                        }

                        let thumb_rect = Rect::from_center_size(
                            pos2(rect.center().x, rect.top() + ROW_PADDING + THUMB_BOX / 2.0),
                            vec2(THUMB_BOX, THUMB_BOX),
                        );
                        match &self.thumbs[i] {
                            Thumb::Ready(tex) => {
                                let r = fit_rect(tex.size_vec2(), thumb_rect, false);
                                ui.painter().image(tex.id(), r, unit_uv(), Color32::WHITE);
                            }
                            Thumb::NotRequested | Thumb::Pending => {
                                if matches!(self.thumbs[i], Thumb::NotRequested) {
                                    self.workers.request_thumb(i);
                                    self.thumbs[i] = Thumb::Pending;
                                }
                                ui.painter().rect_filled(
                                    thumb_rect.shrink(20.0),
                                    4.0,
                                    Color32::from_gray(36),
                                );
                            }
                            Thumb::Failed => {
                                ui.painter().text(
                                    thumb_rect.center(),
                                    Align2::CENTER_CENTER,
                                    "⚠",
                                    FontId::proportional(32.0),
                                    Color32::from_rgb(200, 120, 60),
                                );
                            }
                        }

                        let name = &self.pages[i].display_name;
                        let label_rect = Rect::from_min_size(
                            pos2(rect.left() + 6.0, thumb_rect.bottom() + 2.0),
                            vec2(width - 12.0, LABEL_HEIGHT),
                        );
                        let galley = truncated_galley(ui, name, label_rect.width());
                        let pos = pos2(
                            label_rect.center().x - galley.size().x / 2.0,
                            label_rect.center().y - galley.size().y / 2.0,
                        );
                        ui.painter().galley(pos, galley, Color32::from_gray(220));

                        if resp.clicked() {
                            clicked = Some(i);
                        }
                        resp.on_hover_text(name);
                    }
                });
            });
        if let Some(i) = clicked {
            self.go_to(ctx, i);
            // The clicked thumbnail is already visible: don't jump the list around.
            self.scroll_to_current = false;
        }
    }

    fn viewer(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(Color32::BLACK))
            .show(ctx, |ui| {
                let area = ui.max_rect();
                let resp = ui.allocate_rect(area, Sense::hover());
                let painter = ui.painter_at(area);

                if self.pages.is_empty() {
                    painter.text(
                        area.center(),
                        Align2::CENTER_CENTER,
                        format!(
                            "No viewable images found in\n{}\n\nPress O to open another folder, Esc to quit.",
                            self.folder.display()
                        ),
                        FontId::proportional(20.0),
                        Color32::from_gray(180),
                    );
                    return;
                }

                // Mouse wheel over the image flips pages.
                if resp.hovered() {
                    let dy = ctx.input(|i| i.raw_scroll_delta.y);
                    if dy != 0.0 && self.last_wheel_nav.elapsed() > Duration::from_millis(120) {
                        self.last_wheel_nav = Instant::now();
                        let n = self.pages.len();
                        let t = if dy < 0.0 {
                            (self.current + 1).min(n - 1)
                        } else {
                            self.current.saturating_sub(1)
                        };
                        if t != self.current {
                            self.go_to(ctx, t);
                        }
                    }
                }

                let index = self.current;
                let mut info = format!(
                    "{} / {}   {}",
                    index + 1,
                    self.pages.len(),
                    self.pages[index].display_name
                );
                match self.cache.get(&index) {
                    Some(Slot::Loaded(loaded)) => {
                        if self.anim.index != index {
                            self.anim = Animation { index, frame: 0, frame_started: Instant::now() };
                        }
                        let frame_count = loaded.frames.len();
                        if frame_count > 1 {
                            let delay = loaded.frames[self.anim.frame].1;
                            let elapsed = self.anim.frame_started.elapsed();
                            if elapsed >= delay {
                                self.anim.frame = (self.anim.frame + 1) % frame_count;
                                // Keep cadence steady without spiralling after a stall.
                                self.anim.frame_started = if elapsed < delay * 2 {
                                    self.anim.frame_started + delay
                                } else {
                                    Instant::now()
                                };
                            }
                            let next_delay = loaded.frames[self.anim.frame].1;
                            let remaining =
                                next_delay.saturating_sub(self.anim.frame_started.elapsed());
                            ctx.request_repaint_after(remaining);
                        }
                        let tex = &loaded.frames[self.anim.frame].0;
                        let r = fit_rect(tex.size_vec2(), area, true);
                        painter.image(tex.id(), r, unit_uv(), Color32::WHITE);
                        let [w, h] = loaded.original_size;
                        info.push_str(&format!("   {w}×{h}"));
                        if frame_count > 1 {
                            info.push_str(&format!("   frame {}/{}", self.anim.frame + 1, frame_count));
                        }
                    }
                    Some(Slot::Failed(err)) => {
                        painter.text(
                            area.center(),
                            Align2::CENTER_CENTER,
                            format!("Could not open {}\n{err}", self.pages[index].display_name),
                            FontId::proportional(18.0),
                            Color32::from_rgb(230, 140, 90),
                        );
                    }
                    None => {
                        // Show the thumbnail as a placeholder while the full image decodes.
                        if let Thumb::Ready(tex) = &self.thumbs[index] {
                            let r = fit_rect(tex.size_vec2(), area, true);
                            painter.image(tex.id(), r, unit_uv(), Color32::from_gray(160));
                        }
                        painter.text(
                            area.center(),
                            Align2::CENTER_CENTER,
                            "Loading…",
                            FontId::proportional(18.0),
                            Color32::from_gray(200),
                        );
                    }
                }

                // Page indicator, bottom-right.
                let galley = painter.layout_no_wrap(
                    info,
                    FontId::proportional(13.0),
                    Color32::from_gray(210),
                );
                let size = galley.size() + vec2(16.0, 8.0);
                let bg = Rect::from_min_size(area.right_bottom() - size - vec2(8.0, 8.0), size);
                painter.rect_filled(bg, 4.0, Color32::from_black_alpha(160));
                painter.galley(bg.min + vec2(8.0, 4.0), galley, Color32::from_gray(210));
            });
    }
}

impl eframe::App for LiteView {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.receive_results(ctx);
        self.handle_keys(ctx);
        if self.show_sidebar {
            self.sidebar(ctx);
        }
        self.viewer(ctx);
    }
}

fn unit_uv() -> Rect {
    Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0))
}

/// Largest rect with the image's aspect ratio that fits inside `bounds`, centred.
fn fit_rect(image: egui::Vec2, bounds: Rect, allow_upscale: bool) -> Rect {
    if image.x <= 0.0 || image.y <= 0.0 {
        return bounds;
    }
    let mut scale = (bounds.width() / image.x).min(bounds.height() / image.y);
    if !allow_upscale {
        scale = scale.min(1.0);
    }
    let size = image * scale;
    // Snap to whole pixels to avoid shimmering.
    let min = (bounds.center() - size / 2.0).round();
    Rect::from_min_size(min, size.round())
}

fn truncated_galley(ui: &egui::Ui, text: &str, width: f32) -> Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::single_section(
        text.to_owned(),
        egui::TextFormat::simple(FontId::proportional(12.0), Color32::from_gray(220)),
    );
    job.wrap = egui::text::TextWrapping {
        max_width: width,
        max_rows: 1,
        break_anywhere: true,
        overflow_character: Some('…'),
    };
    ui.fonts(|f| f.layout_job(job))
}
