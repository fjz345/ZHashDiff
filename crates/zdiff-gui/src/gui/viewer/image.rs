//! Image viewer: both sides decoded off the UI thread, uploaded once as textures and drawn side
//! by side, or in one view (swipe, overlay), with one zoom and pan shared by both. A decoded pair
//! is compared pixel by pixel off the UI thread too, for the statistics and the difference map.

use std::{
    io::Cursor,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
};

use eframe::egui::{
    self, Color32, ColorImage, Pos2, Rect, Sense, Stroke, StrokeKind, TextureFilter, TextureHandle,
    TextureOptions, Vec2, pos2, vec2,
};
use zdiff::pixel::{PixelDiff, PixelStats, RgbaBuffer, pixel_diff};

use super::{hex::is_same_side, path_extension};
use crate::file::LoadedFile;

/// A decoded image. Animated formats keep their first frame.
pub struct DecodedImage {
    pub width: u32,
    pub height: u32,
    /// Upper-case name of the decoded format, such as "PNG".
    pub format: &'static str,
    /// Straight (not premultiplied) RGBA8, row-major.
    pub rgba: Vec<u8>,
}

// Input logging prints the decoded sides; the pixels can be hundreds of MB.
impl std::fmt::Debug for DecodedImage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DecodedImage")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("format", &self.format)
            .finish()
    }
}

impl DecodedImage {
    fn buffer(&self) -> RgbaBuffer<'_> {
        RgbaBuffer {
            width: self.width,
            height: self.height,
            rgba: &self.rgba,
        }
    }
}

/// Decodes by content, falling back to `extension` (normalized) for formats without a signature
/// such as TGA.
pub fn decode_image(bytes: &[u8], extension: Option<&str>) -> Result<DecodedImage, String> {
    let mut reader = image::ImageReader::new(Cursor::new(bytes));
    if let Some(format) = extension.and_then(image::ImageFormat::from_extension) {
        reader.set_format(format);
    }
    let reader = reader.with_guessed_format().map_err(|e| e.to_string())?;
    let Some(format) = reader.format() else {
        return Err("not a known image format".to_string());
    };
    let image = reader.decode().map_err(|e| e.to_string())?.into_rgba8();
    Ok(DecodedImage {
        width: image.width(),
        height: image.height(),
        format: format_name(format),
        rgba: image.into_raw(),
    })
}

fn format_name(format: image::ImageFormat) -> &'static str {
    use image::ImageFormat as F;
    match format {
        F::Png => "PNG",
        F::Jpeg => "JPEG",
        F::Bmp => "BMP",
        F::Gif => "GIF",
        F::WebP => "WEBP",
        F::Tga => "TGA",
        // Only the enabled decoders get this far.
        _ => "image",
    }
}

pub const MIN_ZOOM: f32 = 1.0 / 64.0;
pub const MAX_ZOOM: f32 = 64.0;

/// Zoom and pan shared by both sides: image pixel `p` is drawn at `side_origin + pan + p * zoom`,
/// so differently sized images stay aligned at their top-left corners.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ImageView {
    pub zoom: f32,
    pub pan: Vec2,
}

impl Default for ImageView {
    fn default() -> Self {
        Self {
            zoom: 1.0,
            pan: Vec2::ZERO,
        }
    }
}

impl ImageView {
    /// The whole `canvas` (the union of both sides' sizes) centered in `viewport`.
    pub fn fit(canvas: Vec2, viewport: Vec2) -> Self {
        if canvas.min_elem() <= 0.0 || viewport.min_elem() <= 0.0 {
            return Self::default();
        }
        let zoom = (viewport / canvas).min_elem().clamp(MIN_ZOOM, MAX_ZOOM);
        Self {
            zoom,
            pan: (viewport - canvas * zoom) / 2.0,
        }
    }

    /// Zooms by `factor`, clamped, keeping the image pixel under `point` (side-relative) in place.
    pub fn zoomed_about(self, point: Vec2, factor: f32) -> Self {
        let zoom = (self.zoom * factor).clamp(MIN_ZOOM, MAX_ZOOM);
        Self {
            zoom,
            pan: point - (point - self.pan) * (zoom / self.zoom),
        }
    }
}

enum SideState {
    Empty,
    Decoding,
    /// The texture is uploaded on the first frame the side is drawn.
    Decoded {
        image: Arc<DecodedImage>,
        texture: Option<TextureHandle>,
    },
    Failed(String),
}

impl std::fmt::Debug for SideState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SideState::Empty => write!(f, "Empty"),
            SideState::Decoding => write!(f, "Decoding"),
            SideState::Decoded { image, texture } => f
                .debug_struct("Decoded")
                .field("image", image)
                .field("uploaded", &texture.is_some())
                .finish(),
            SideState::Failed(reason) => f.debug_tuple("Failed").field(reason).finish(),
        }
    }
}

/// How the decoded sides are drawn. Kept across pairs, not persisted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ImageMode {
    #[default]
    SideBySide,
    /// Both sides dimmed, with the changed pixels of the union canvas drawn over them.
    Difference,
    /// Both sides in one view: the first left of a draggable divider, the second right of it.
    Swipe,
    /// Both sides in one view, the second drawn over the first at an adjustable opacity.
    Overlay,
}

/// Where an image of `size` pixels is drawn in a view whose top-left is `origin`. Every image
/// starts at the same point, so differently sized images stay aligned at their top-left corners.
pub fn image_screen_rect(view: ImageView, origin: Pos2, size: Vec2) -> Rect {
    Rect::from_min_size(origin + view.pan, size * view.zoom)
}

/// The parts of `rect` left and right of a divider at `divider` (0..=1, clamped) of its width.
pub fn swipe_clips(rect: Rect, divider: f32) -> [Rect; 2] {
    let x = rect.left() + rect.width() * divider.clamp(0.0, 1.0);
    [
        Rect::from_min_max(rect.min, pos2(x, rect.bottom())),
        Rect::from_min_max(pos2(x, rect.top()), rect.max),
    ]
}

/// The divider fraction (0..=1) for a pointer at `x`.
pub fn divider_at(rect: Rect, x: f32) -> f32 {
    if rect.width() <= 0.0 {
        return 0.5;
    }
    ((x - rect.left()) / rect.width()).clamp(0.0, 1.0)
}

/// The second image's tint in overlay mode. Textures are premultiplied, so every channel scales.
fn overlay_tint(opacity: f32) -> Color32 {
    Color32::WHITE.gamma_multiply(opacity.clamp(0.0, 1.0))
}

/// Changed pixels in the difference map.
const DIFF_COLOR: Color32 = Color32::from_rgb(255, 0, 255);

fn mask_image(diff: &PixelDiff) -> ColorImage {
    let pixels = diff
        .mask
        .iter()
        .map(|&changed| {
            if changed {
                DIFF_COLOR
            } else {
                Color32::TRANSPARENT
            }
        })
        .collect();
    ColorImage::new([diff.width as usize, diff.height as usize], pixels)
}

/// The changed-pixel count and share, never rounded down to 0% while a pixel is changed.
pub fn stats_text(stats: &PixelStats) -> String {
    let percent = stats.changed_percent();
    let percent = if percent < 0.01 {
        "<0.01%".to_string()
    } else {
        format!("{percent:.2}%")
    };
    match stats.changed {
        0 => "No changed pixels".to_string(),
        1 => format!("1 changed pixel ({percent})"),
        changed => format!("{changed} changed pixels ({percent})"),
    }
}

/// A finished comparison of the current pair.
struct DiffMap {
    tolerance: u8,
    stats: PixelStats,
    /// The union canvas, in image pixels.
    size: Vec2,
    /// Uploaded (and dropped) on the first frame the difference map is drawn.
    mask: Option<ColorImage>,
    texture: Option<TextureHandle>,
}

impl std::fmt::Debug for DiffMap {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DiffMap")
            .field("tolerance", &self.tolerance)
            .field("stats", &self.stats)
            .field("uploaded", &self.texture.is_some())
            .finish()
    }
}

type DecodeResult = (usize, u64, Result<DecodedImage, String>);
type CompareResult = (u64, u8, PixelStats, ColorImage);

/// Decodes each side on a worker thread when its load changes. A result for an older load of the
/// side is dropped. Decoding can't be interrupted; a stale decode just runs to its end.
#[derive(Debug)]
pub struct ImageDiffProcessor {
    files: [Option<LoadedFile>; 2],
    sides: [SideState; 2],
    generations: [u64; 2],
    channel: (mpsc::Sender<DecodeResult>, mpsc::Receiver<DecodeResult>),
    view: ImageView,
    /// Fit once both sides are done decoding, so the second side arriving doesn't move the view.
    fit_pending: bool,
    mode: ImageMode,
    /// The largest per-channel difference that still counts as unchanged.
    tolerance: u8,
    /// The swipe divider, as a fraction of the view's width: it stays put while panning.
    swipe: f32,
    /// The second image's opacity in overlay mode.
    opacity: f32,
    /// What the current pair was last sent to compare with; `None` until both sides decode.
    compared_tolerance: Option<u8>,
    compare_generation: u64,
    /// Set when the pair or the tolerance changes, so a superseded comparison stops early.
    compare_cancel: Arc<AtomicBool>,
    compare_channel: (mpsc::Sender<CompareResult>, mpsc::Receiver<CompareResult>),
    /// Kept while another tolerance is compared, so dragging the slider doesn't blank the map.
    diff_map: Option<DiffMap>,
}

impl Default for ImageDiffProcessor {
    fn default() -> Self {
        Self {
            files: [None, None],
            sides: [SideState::Empty, SideState::Empty],
            generations: [0, 0],
            channel: mpsc::channel(),
            view: ImageView::default(),
            fit_pending: false,
            mode: ImageMode::default(),
            tolerance: 0,
            swipe: 0.5,
            opacity: 0.5,
            compared_tolerance: None,
            compare_generation: 0,
            compare_cancel: Arc::new(AtomicBool::new(false)),
            compare_channel: mpsc::channel(),
            diff_map: None,
        }
    }
}

impl ImageDiffProcessor {
    /// No-op for the loads already requested; only a changed side is decoded again.
    pub fn request(&mut self, file_1: Option<LoadedFile>, file_2: Option<LoadedFile>) {
        for (index, file) in [file_1, file_2].into_iter().enumerate() {
            if is_same_side(&self.files[index], &file) {
                continue;
            }
            self.generations[index] += 1;
            self.fit_pending = true;
            self.sides[index] = match &file {
                None => SideState::Empty,
                Some(file) => {
                    log::info!("Image decode requested: {}", file.path());
                    let (file, tx) = (file.clone(), self.channel.0.clone());
                    let generation = self.generations[index];
                    std::thread::spawn(move || {
                        let extension = path_extension(file.path());
                        let decoded =
                            decode_image(file.bytes(), extension.as_deref()).map_err(|e| {
                                format!("{} can't be decoded as an image: {}", file.path(), e)
                            });
                        let _ = tx.send((index, generation, decoded));
                    });
                    SideState::Decoding
                }
            };
            self.files[index] = file;
            self.reset_comparison();
        }
    }

    fn reset_comparison(&mut self) {
        self.compare_cancel.store(true, Ordering::Release);
        self.compare_cancel = Arc::new(AtomicBool::new(false));
        self.compare_generation += 1;
        self.compared_tolerance = None;
        self.diff_map = None;
    }

    /// Takes a finished comparison, and compares the pair again once both sides are decoded or
    /// the tolerance changed.
    fn compare(&mut self) {
        while let Ok((generation, tolerance, stats, mask)) = self.compare_channel.1.try_recv() {
            if generation != self.compare_generation {
                continue;
            }
            self.diff_map = Some(DiffMap {
                tolerance,
                stats,
                size: vec2(mask.size[0] as f32, mask.size[1] as f32),
                mask: Some(mask),
                texture: None,
            });
        }

        let decoded = |side: &SideState| match side {
            SideState::Decoded { image, .. } => Some(image.clone()),
            _ => None,
        };
        let (Some(first), Some(second)) = (decoded(&self.sides[0]), decoded(&self.sides[1])) else {
            return;
        };
        if self.compared_tolerance == Some(self.tolerance) {
            return;
        }
        // Keeps the shown map; only a result for this tolerance replaces it.
        let diff_map = self.diff_map.take();
        self.reset_comparison();
        self.diff_map = diff_map;
        self.compared_tolerance = Some(self.tolerance);

        let (tolerance, generation) = (self.tolerance, self.compare_generation);
        let (cancel_flag, tx) = (self.compare_cancel.clone(), self.compare_channel.0.clone());
        std::thread::spawn(move || {
            let Some(diff) = pixel_diff(first.buffer(), second.buffer(), tolerance, &cancel_flag)
            else {
                return;
            };
            let mask = mask_image(&diff);
            let _ = tx.send((generation, tolerance, diff.stats, mask));
        });
    }

    /// `max_texture_side` is the renderer's limit; a larger image can't be uploaded and fails
    /// like a decode error.
    pub fn poll(&mut self, max_texture_side: usize) {
        while let Ok((index, generation, decoded)) = self.channel.1.try_recv() {
            if generation != self.generations[index] {
                continue;
            }
            let path = self.files[index].as_ref().map(|f| f.path().to_string());
            self.sides[index] = match decoded {
                Ok(image) if image.width == 0 || image.height == 0 => {
                    SideState::Failed(format!("{} is an empty image", path.unwrap_or_default()))
                }
                Ok(image) if image.width.max(image.height) as usize > max_texture_side => {
                    SideState::Failed(format!(
                        "{} is {} x {}, larger than the renderer's {} pixel limit",
                        path.unwrap_or_default(),
                        image.width,
                        image.height,
                        max_texture_side
                    ))
                }
                Ok(image) => SideState::Decoded {
                    image: Arc::new(image),
                    texture: None,
                },
                Err(reason) => SideState::Failed(reason),
            };
        }
        self.compare();
    }

    /// One reason per side that can't be shown as an image.
    pub fn failures(&self) -> Vec<String> {
        self.sides
            .iter()
            .filter_map(|side| match side {
                SideState::Failed(reason) => Some(reason.clone()),
                _ => None,
            })
            .collect()
    }

    fn decoded(&self, index: usize) -> Option<&DecodedImage> {
        match &self.sides[index] {
            SideState::Decoded { image, .. } => Some(image),
            _ => None,
        }
    }

    /// The toolbar status and whether it is a warning (the sizes differ).
    pub fn status_text(&self) -> (String, bool) {
        if self
            .sides
            .iter()
            .any(|side| matches!(side, SideState::Decoding))
        {
            return ("Decoding...".to_string(), false);
        }
        let size = |image: &DecodedImage| format!("{} x {}", image.width, image.height);
        match (self.decoded(0), self.decoded(1)) {
            (Some(a), Some(b)) if (a.width, a.height) != (b.width, b.height) => {
                (format!("Sizes differ: {} vs {}", size(a), size(b)), true)
            }
            (Some(image), _) | (None, Some(image)) => (size(image), false),
            (None, None) => (String::new(), false),
        }
    }

    /// Both sides are decoded and the shown comparison isn't for the current tolerance yet.
    fn comparing(&self) -> bool {
        self.decoded(0).is_some()
            && self.decoded(1).is_some()
            && self.diff_map.as_ref().map(|map| map.tolerance) != Some(self.tolerance)
    }

    fn compare_status(&self) -> String {
        match (&self.diff_map, self.comparing()) {
            (Some(map), false) => stats_text(&map.stats),
            (Some(map), true) => format!("{} (comparing...)", stats_text(&map.stats)),
            (None, true) => "Comparing...".to_string(),
            (None, false) => String::new(),
        }
    }
}

/// Both sides, left one `left_width` wide like the table's left column, in the rest of `ui`.
/// Wheel zooms about the cursor, drag pans, double-click fits; both sides share the view. Swipe
/// and overlay draw both sides in one view over the whole area.
pub fn show(ui: &mut egui::Ui, processor: &mut ImageDiffProcessor, left_width: f32) {
    let mut fit = false;
    let mut actual_size = false;
    ui.horizontal(|ui| {
        fit = ui.button("Fit").clicked();
        actual_size = ui.button("1:1").clicked();
        ui.label(format!("{:.0}%", processor.view.zoom * 100.0));
        ui.separator();
        ui.selectable_value(&mut processor.mode, ImageMode::SideBySide, "Side by side");
        ui.selectable_value(&mut processor.mode, ImageMode::Difference, "Difference");
        ui.selectable_value(&mut processor.mode, ImageMode::Swipe, "Swipe")
            .on_hover_text("Drag the divider to reveal the other image");
        ui.selectable_value(&mut processor.mode, ImageMode::Overlay, "Overlay");
        if processor.mode == ImageMode::Overlay {
            ui.add(egui::Slider::new(&mut processor.opacity, 0.0..=1.0).text("Opacity"))
                .on_hover_text("How opaque the right image is over the left one");
        }
        ui.add(egui::Slider::new(&mut processor.tolerance, 0..=255).text("Tolerance"))
            .on_hover_text("The largest per-channel difference that still counts as unchanged");
        ui.label(processor.compare_status());
    });
    if processor.comparing() {
        // The result arrives from a worker; keep frames coming until it is polled.
        ui.ctx().request_repaint();
    }

    let area = ui.available_rect_before_wrap();
    let info_height = ui.text_style_height(&egui::TextStyle::Monospace) + 4.0;
    let gap = 12.0 + 2.0 * ui.spacing().item_spacing.x;
    let left_right = (area.left() + left_width).min(area.right());
    // Swipe and overlay draw both sides in one view, so until both are decoded the pair is drawn
    // side by side, where a spinner or a failure has its own column.
    let combined = matches!(processor.mode, ImageMode::Swipe | ImageMode::Overlay)
        && processor.decoded(0).is_some()
        && processor.decoded(1).is_some();
    let columns = if combined {
        [area, area]
    } else {
        [
            Rect::from_min_max(area.min, pos2(left_right, area.bottom())),
            Rect::from_min_max(
                pos2((left_right + gap).min(area.right()), area.top()),
                area.max,
            ),
        ]
    };
    let image_rects =
        columns.map(|c| Rect::from_min_max(pos2(c.left(), c.top() + info_height), c.max));
    // Fit for the narrower side, so the canvas fits in both.
    let viewport = vec2(
        image_rects[0].width().min(image_rects[1].width()),
        image_rects[0].height(),
    );

    let response = ui.allocate_rect(area, Sense::click_and_drag());
    let decoding = processor
        .sides
        .iter()
        .any(|side| matches!(side, SideState::Decoding));
    if (processor.fit_pending && !decoding) || fit || response.double_clicked() {
        let canvas = (0..2)
            .filter_map(|i| processor.decoded(i))
            .fold(Vec2::ZERO, |canvas, image| {
                canvas.max(vec2(image.width as f32, image.height as f32))
            });
        processor.view = ImageView::fit(canvas, viewport);
        processor.fit_pending = false;
    }
    if actual_size {
        processor.view = processor
            .view
            .zoomed_about(viewport / 2.0, 1.0 / processor.view.zoom);
    }
    if response.dragged() {
        processor.view.pan += response.drag_delta();
    }
    if let Some(pointer) = response.hover_pos() {
        let (scroll, pinch) = ui.input(|i| (i.smooth_scroll_delta.y, i.zoom_delta()));
        let factor = (scroll / 200.0).exp() * pinch;
        let hovered_side = image_rects.iter().find(|rect| rect.contains(pointer));
        if let Some(side_rect) = hovered_side
            && factor != 1.0
        {
            processor.view = processor.view.zoomed_about(pointer - side_rect.min, factor);
        }
    }

    // Registered after the view's drag area, so a drag on the divider moves it instead of panning.
    let swipe = (combined && processor.mode == ImageMode::Swipe).then(|| {
        let view_rect = image_rects[0];
        let x = swipe_clips(view_rect, processor.swipe)[0].right();
        let strip = Rect::from_x_y_ranges(x - 4.0..=x + 4.0, view_rect.y_range());
        let handle = ui.interact(strip, ui.id().with("image_swipe_divider"), Sense::drag());
        if handle.hovered() || handle.dragged() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
        }
        if handle.dragged()
            && let Some(pointer) = handle.interact_pointer_pos()
        {
            processor.swipe = divider_at(view_rect, pointer.x);
        }
        swipe_clips(view_rect, processor.swipe)
    });
    let overlay_opacity =
        (combined && processor.mode == ImageMode::Overlay).then_some(processor.opacity);

    let view = processor.view;
    let diff_overlay = match &mut processor.diff_map {
        Some(map) if processor.mode == ImageMode::Difference => {
            if let Some(mask) = map.mask.take() {
                // Nearest without mipmaps: a lone changed pixel must not be averaged away.
                map.texture = Some(ui.ctx().load_texture(
                    "image_diff_mask",
                    mask,
                    TextureOptions::NEAREST,
                ));
            }
            map.texture
                .as_ref()
                .map(|texture| (texture.id(), map.size, map.stats.bounds.clone()))
        }
        _ => None,
    };
    let text_color = ui.visuals().text_color();
    let font = egui::TextStyle::Monospace.resolve(ui.style());
    for (index, side) in processor.sides.iter_mut().enumerate() {
        let painter = ui.painter_at(columns[index]);
        let image_rect = image_rects[index];
        match side {
            SideState::Empty => {}
            SideState::Decoding => {
                let size = 32.0;
                egui::Spinner::new().size(size).paint_at(
                    ui,
                    Rect::from_center_size(image_rect.center(), Vec2::splat(size)),
                );
            }
            SideState::Failed(reason) => {
                painter.text(
                    columns[index].min,
                    egui::Align2::LEFT_TOP,
                    reason.as_str(),
                    font.clone(),
                    ui.visuals().error_fg_color,
                );
            }
            SideState::Decoded { image, texture } => {
                // In one view the second side's info goes on the right, where its image is shown.
                let (info_pos, info_align) = if combined && index == 1 {
                    (columns[index].right_top(), egui::Align2::RIGHT_TOP)
                } else {
                    (columns[index].min, egui::Align2::LEFT_TOP)
                };
                painter.text(
                    info_pos,
                    info_align,
                    format!("{}, {} x {}", image.format, image.width, image.height),
                    font.clone(),
                    text_color,
                );
                let texture = texture.get_or_insert_with(|| {
                    let pixels = ColorImage::from_rgba_unmultiplied(
                        [image.width as usize, image.height as usize],
                        &image.rgba,
                    );
                    // Nearest when magnified, so zoomed-in pixels stay sharp.
                    let options = TextureOptions {
                        magnification: TextureFilter::Nearest,
                        mipmap_mode: Some(TextureFilter::Linear),
                        ..TextureOptions::LINEAR
                    };
                    ui.ctx()
                        .load_texture(format!("image_diff_side_{index}"), pixels, options)
                });
                let drawn = image_screen_rect(
                    view,
                    image_rect.min,
                    vec2(image.width as f32, image.height as f32),
                );
                // Swipe shows each side only on its side of the divider.
                let painter = ui.painter_at(swipe.map_or(image_rect, |clips| clips[index]));
                let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
                let tint = match (&diff_overlay, overlay_opacity) {
                    // Dimmed under the difference map, so the highlight stands out.
                    (Some(_), _) => Color32::from_gray(80),
                    // Sides draw in order, so the second blends over the first.
                    (None, Some(opacity)) if index == 1 => overlay_tint(opacity),
                    _ => Color32::WHITE,
                };
                painter.image(texture.id(), drawn, uv, tint);
                painter.rect_stroke(
                    drawn,
                    0.0,
                    Stroke::new(1.0, Color32::from_gray(80)),
                    StrokeKind::Outside,
                );

                if let Some((mask, canvas, bounds)) = &diff_overlay {
                    // The canvas is aligned top-left like both images, so the map lines up on
                    // either side.
                    let origin = image_rect.min + view.pan;
                    painter.image(
                        *mask,
                        Rect::from_min_size(origin, *canvas * view.zoom),
                        uv,
                        Color32::WHITE,
                    );
                    if let Some(bounds) = bounds {
                        let corner = |x: u32, y: u32| origin + vec2(x as f32, y as f32) * view.zoom;
                        let changed = Rect::from_min_max(
                            corner(bounds.x.start, bounds.y.start),
                            corner(bounds.x.end, bounds.y.end),
                        );
                        // A few points across at any zoom, so a one-pixel edit can be found.
                        let changed = Rect::from_center_size(
                            changed.center(),
                            changed.size().max(Vec2::splat(9.0)),
                        );
                        painter.rect_stroke(
                            changed.expand(2.0),
                            0.0,
                            Stroke::new(1.5, DIFF_COLOR),
                            StrokeKind::Outside,
                        );
                    }
                }
            }
        }
    }

    if let Some([left, _]) = swipe {
        // Dark under light, so the divider shows on any image.
        let painter = ui.painter_at(image_rects[0]);
        let x = left.right();
        painter.vline(
            x,
            left.y_range(),
            Stroke::new(4.0, Color32::from_black_alpha(160)),
        );
        painter.vline(x, left.y_range(), Stroke::new(2.0, Color32::WHITE));
        painter.circle(
            pos2(x, left.center().y),
            6.0,
            Color32::WHITE,
            Stroke::new(1.0, Color32::from_black_alpha(160)),
        );
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use image::{ImageFormat, RgbaImage};
    use zdiff::universal_path::UniversalPath;

    use super::*;
    use crate::file::BinaryFile;
    use crate::viewer::{ViewerKind, ViewerResolution, image_decode_fallback};

    fn pixel_under(view: ImageView, point: Vec2) -> Vec2 {
        (point - view.pan) / view.zoom
    }

    fn assert_near(a: Vec2, b: Vec2) {
        assert!((a - b).length() < 1e-3, "{a:?} != {b:?}");
    }

    #[test]
    fn fit_shows_the_whole_canvas_centered() {
        let view = ImageView::fit(vec2(200.0, 100.0), vec2(100.0, 100.0));
        assert_eq!(view.zoom, 0.5);
        assert_near(view.pan, vec2(0.0, 25.0));

        // A small image is magnified to fit too.
        let view = ImageView::fit(vec2(10.0, 20.0), vec2(100.0, 100.0));
        assert_eq!(view.zoom, 5.0);
        assert_near(view.pan, vec2(25.0, 0.0));

        // Nothing to fit, or nowhere to fit it: 1:1 at the origin.
        assert_eq!(
            ImageView::fit(Vec2::ZERO, vec2(100.0, 100.0)),
            ImageView::default()
        );
        assert_eq!(
            ImageView::fit(vec2(10.0, 10.0), Vec2::ZERO),
            ImageView::default()
        );
    }

    #[test]
    fn zooming_keeps_the_pixel_under_the_cursor_in_place() {
        let view = ImageView {
            zoom: 2.0,
            pan: vec2(10.0, 20.0),
        };
        let cursor = vec2(50.0, 60.0);
        let zoomed = view.zoomed_about(cursor, 1.5);
        assert_eq!(zoomed.zoom, 3.0);
        assert_near(pixel_under(zoomed, cursor), pixel_under(view, cursor));
    }

    #[test]
    fn zoom_is_clamped_and_still_keeps_the_cursor_pixel() {
        let view = ImageView::default();
        let cursor = vec2(30.0, 40.0);
        let zoomed = view.zoomed_about(cursor, 1e6);
        assert_eq!(zoomed.zoom, MAX_ZOOM);
        assert_near(pixel_under(zoomed, cursor), pixel_under(view, cursor));
        assert_eq!(view.zoomed_about(cursor, 1e-6).zoom, MIN_ZOOM);
        assert_eq!(
            ImageView::fit(vec2(1.0, 1.0), vec2(1e6, 1e6)).zoom,
            MAX_ZOOM
        );
    }

    fn encode(image: &RgbaImage, format: ImageFormat) -> Vec<u8> {
        let mut bytes = Cursor::new(Vec::new());
        image.write_to(&mut bytes, format).unwrap();
        bytes.into_inner()
    }

    fn two_pixels() -> RgbaImage {
        // The second pixel is half transparent, to check alpha stays straight.
        RgbaImage::from_raw(2, 1, vec![255, 0, 0, 255, 200, 100, 50, 128]).unwrap()
    }

    #[test]
    fn a_png_decodes_to_straight_rgba_with_its_size_and_format() {
        let decoded = decode_image(&encode(&two_pixels(), ImageFormat::Png), Some("png")).unwrap();
        assert_eq!((decoded.width, decoded.height), (2, 1));
        assert_eq!(decoded.format, "PNG");
        assert_eq!(decoded.rgba, two_pixels().into_raw());
    }

    #[test]
    fn content_beats_the_extension() {
        // A custom extension, or a wrong one, still decodes by its signature.
        let png = encode(&two_pixels(), ImageFormat::Png);
        assert_eq!(decode_image(&png, Some("zpng")).unwrap().format, "PNG");
        assert_eq!(decode_image(&png, Some("jpg")).unwrap().format, "PNG");
        assert_eq!(decode_image(&png, None).unwrap().format, "PNG");
    }

    #[test]
    fn a_tga_without_a_signature_decodes_by_its_extension() {
        let tga = encode(&two_pixels(), ImageFormat::Tga);
        let decoded = decode_image(&tga, Some("tga")).unwrap();
        assert_eq!(decoded.format, "TGA");
        assert_eq!(decoded.rgba, two_pixels().into_raw());
        assert!(decode_image(&tga, None).is_err());
    }

    #[test]
    fn a_corrupt_png_resolves_to_the_hex_fallback() {
        let mut corrupt = encode(&two_pixels(), ImageFormat::Png);
        corrupt.truncate(20);
        corrupt.extend_from_slice(b"not the rest of a png");
        let error = decode_image(&corrupt, Some("png")).unwrap_err();
        assert!(!error.is_empty());

        let image = ViewerResolution {
            kind: ViewerKind::Image,
            fallback: None,
        };
        let resolution = image_decode_fallback(image, &[error]);
        assert_eq!(resolution.kind, ViewerKind::Hex);
        assert!(resolution.fallback.is_some());
    }

    fn png_file(name: &str, image: &RgbaImage) -> LoadedFile {
        LoadedFile::Binary(Arc::new(BinaryFile {
            path: UniversalPath::from(name),
            bytes: encode(image, ImageFormat::Png),
        }))
    }

    /// Decoding and comparing are threaded, so poll like the app's frames do.
    fn poll_until(processor: &mut ImageDiffProcessor, done: impl Fn(&ImageDiffProcessor) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !done(processor) {
            assert!(Instant::now() < deadline, "timed out: {processor:?}");
            processor.poll(usize::MAX);
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    fn changed(processor: &ImageDiffProcessor) -> Option<(u8, u64, u64)> {
        let map = processor.diff_map.as_ref()?;
        Some((map.tolerance, map.stats.changed, map.stats.total))
    }

    #[test]
    fn a_decoded_pair_is_compared_again_when_the_tolerance_changes() {
        let first = RgbaImage::from_raw(2, 1, vec![0, 0, 0, 255, 0, 0, 0, 255]).unwrap();
        let second = RgbaImage::from_raw(2, 1, vec![0, 0, 0, 255, 10, 0, 0, 255]).unwrap();
        let mut processor = ImageDiffProcessor::default();
        processor.request(
            Some(png_file("a.png", &first)),
            Some(png_file("b.png", &second)),
        );
        poll_until(&mut processor, |p| p.diff_map.is_some());
        assert_eq!(changed(&processor), Some((0, 1, 2)));
        assert!(!processor.comparing());
        assert_eq!(processor.compare_status(), "1 changed pixel (50.00%)");

        processor.tolerance = 10;
        assert!(processor.comparing());
        // The last result stays shown while the new tolerance is compared.
        assert!(processor.compare_status().starts_with("1 changed pixel"));
        poll_until(&mut processor, |p| !p.comparing());
        assert_eq!(changed(&processor), Some((10, 0, 2)));
        assert_eq!(processor.compare_status(), "No changed pixels");
    }

    #[test]
    fn a_one_sided_or_changed_pair_has_no_comparison() {
        let image = two_pixels();
        let mut processor = ImageDiffProcessor::default();
        processor.request(Some(png_file("a.png", &image)), None);
        poll_until(&mut processor, |p| p.decoded(0).is_some());
        for _ in 0..20 {
            processor.poll(usize::MAX);
        }
        assert!(processor.diff_map.is_none());
        assert!(!processor.comparing());
        assert_eq!(processor.compare_status(), "");

        // The pair completes, is compared, then a new side drops the old pair's result at once.
        processor.request(
            Some(png_file("a.png", &image)),
            Some(png_file("b.png", &image)),
        );
        poll_until(&mut processor, |p| p.diff_map.is_some());
        assert_eq!(changed(&processor), Some((0, 0, 2)));
        processor.request(
            Some(png_file("a.png", &image)),
            Some(png_file("c.png", &image)),
        );
        assert!(processor.diff_map.is_none());
        poll_until(&mut processor, |p| p.diff_map.is_some());
        assert_eq!(changed(&processor), Some((0, 0, 2)));
    }

    #[test]
    fn stats_never_round_a_changed_pixel_down_to_zero_percent() {
        let stats = |changed, total| PixelStats {
            changed,
            total,
            bounds: None,
        };
        assert_eq!(stats_text(&stats(0, 100)), "No changed pixels");
        assert_eq!(stats_text(&stats(1, 4)), "1 changed pixel (25.00%)");
        assert_eq!(stats_text(&stats(3, 3)), "3 changed pixels (100.00%)");
        // One pixel of a 4K image.
        assert_eq!(
            stats_text(&stats(1, 3840 * 2160)),
            "1 changed pixel (<0.01%)"
        );
    }

    #[test]
    fn the_mask_marks_changed_pixels_on_a_transparent_canvas() {
        let diff = PixelDiff {
            width: 2,
            height: 1,
            mask: vec![false, true],
            stats: PixelStats::default(),
        };
        let mask = mask_image(&diff);
        assert_eq!(mask.size, [2, 1]);
        assert_eq!(mask.pixels, vec![Color32::TRANSPARENT, DIFF_COLOR]);
    }

    #[test]
    fn differently_sized_images_share_their_top_left_corner() {
        let view = ImageView {
            zoom: 2.0,
            pan: vec2(10.0, -4.0),
        };
        let origin = pos2(100.0, 50.0);
        let small = image_screen_rect(view, origin, vec2(3.0, 5.0));
        let large = image_screen_rect(view, origin, vec2(40.0, 2.0));
        assert_eq!(small.min, pos2(110.0, 46.0));
        assert_eq!(large.min, small.min);
        assert_eq!(small.size(), vec2(6.0, 10.0));
        assert_eq!(large.size(), vec2(80.0, 4.0));
    }

    #[test]
    fn swipe_clips_tile_the_view_at_the_divider() {
        let rect = Rect::from_min_max(pos2(100.0, 20.0), pos2(300.0, 120.0));
        let [left, right] = swipe_clips(rect, 0.25);
        assert_eq!(
            left,
            Rect::from_min_max(pos2(100.0, 20.0), pos2(150.0, 120.0))
        );
        assert_eq!(
            right,
            Rect::from_min_max(pos2(150.0, 20.0), pos2(300.0, 120.0))
        );

        // At either end one side takes the whole view and the other nothing.
        let [left, right] = swipe_clips(rect, 0.0);
        assert_eq!((left.width(), right), (0.0, rect));
        let [left, right] = swipe_clips(rect, 1.0);
        assert_eq!((left, right.width()), (rect, 0.0));

        // Out of range is clamped.
        assert_eq!(swipe_clips(rect, -1.0), swipe_clips(rect, 0.0));
        assert_eq!(swipe_clips(rect, 7.0), swipe_clips(rect, 1.0));
    }

    #[test]
    fn the_divider_follows_the_pointer_inside_the_view() {
        let rect = Rect::from_min_max(pos2(100.0, 20.0), pos2(300.0, 120.0));
        assert_eq!(divider_at(rect, 150.0), 0.25);
        assert_eq!(divider_at(rect, 300.0), 1.0);
        assert_eq!(divider_at(rect, 20.0), 0.0);
        assert_eq!(divider_at(rect, 1000.0), 1.0);
        // Round trip: the divider lands where the pointer is.
        let [left, _] = swipe_clips(rect, divider_at(rect, 237.0));
        assert!((left.right() - 237.0).abs() < 1e-3);
        // A view with no width keeps the divider in range.
        let empty = Rect::from_min_max(pos2(5.0, 0.0), pos2(5.0, 10.0));
        assert!((0.0..=1.0).contains(&divider_at(empty, 5.0)));
    }

    #[test]
    fn overlay_opacity_scales_the_second_image() {
        assert_eq!(overlay_tint(1.0), Color32::WHITE);
        assert_eq!(overlay_tint(0.0), Color32::TRANSPARENT);
        let half = overlay_tint(0.5);
        assert!((120..=135).contains(&half.a()), "{half:?}");
        // Premultiplied: every channel scales with alpha.
        assert_eq!([half.r(), half.g(), half.b()], [half.a(); 3]);
        assert_eq!(overlay_tint(2.0), overlay_tint(1.0));
        assert_eq!(overlay_tint(-1.0), overlay_tint(0.0));
    }

    #[test]
    fn unknown_content_with_no_usable_extension_is_an_error() {
        assert!(decode_image(b"plain text", Some("zpng")).is_err());
        assert!(decode_image(b"plain text", None).is_err());
        assert!(decode_image(b"", Some("png")).is_err());
    }
}
