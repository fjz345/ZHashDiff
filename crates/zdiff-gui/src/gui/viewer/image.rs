//! Image viewer: both sides decoded off the UI thread, uploaded once as textures and drawn side
//! by side with one zoom and pan shared by both.

use std::{
    io::Cursor,
    sync::{Arc, mpsc},
};

use eframe::egui::{
    self, Color32, ColorImage, Rect, Sense, Stroke, StrokeKind, TextureFilter, TextureHandle,
    TextureOptions, Vec2, pos2, vec2,
};

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

type DecodeResult = (usize, u64, Result<DecodedImage, String>);

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
        }
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
}

/// Both sides, left one `left_width` wide like the table's left column, in the rest of `ui`.
/// Wheel zooms about the cursor, drag pans, double-click fits; both sides share the view.
pub fn show(ui: &mut egui::Ui, processor: &mut ImageDiffProcessor, left_width: f32) {
    let mut fit = false;
    let mut actual_size = false;
    ui.horizontal(|ui| {
        fit = ui.button("Fit").clicked();
        actual_size = ui.button("1:1").clicked();
        ui.label(format!("{:.0}%", processor.view.zoom * 100.0));
    });

    let area = ui.available_rect_before_wrap();
    let info_height = ui.text_style_height(&egui::TextStyle::Monospace) + 4.0;
    let gap = 12.0 + 2.0 * ui.spacing().item_spacing.x;
    let left_right = (area.left() + left_width).min(area.right());
    let columns = [
        Rect::from_min_max(area.min, pos2(left_right, area.bottom())),
        Rect::from_min_max(
            pos2((left_right + gap).min(area.right()), area.top()),
            area.max,
        ),
    ];
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

    let view = processor.view;
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
                painter.text(
                    columns[index].min,
                    egui::Align2::LEFT_TOP,
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
                let drawn = Rect::from_min_size(
                    image_rect.min + view.pan,
                    vec2(image.width as f32, image.height as f32) * view.zoom,
                );
                let painter = ui.painter_at(image_rect);
                painter.image(
                    texture.id(),
                    drawn,
                    Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                    Color32::WHITE,
                );
                painter.rect_stroke(
                    drawn,
                    0.0,
                    Stroke::new(1.0, Color32::from_gray(80)),
                    StrokeKind::Outside,
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use image::{ImageFormat, RgbaImage};

    use super::*;
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

    #[test]
    fn unknown_content_with_no_usable_extension_is_an_error() {
        assert!(decode_image(b"plain text", Some("zpng")).is_err());
        assert!(decode_image(b"plain text", None).is_err());
        assert!(decode_image(b"", Some("png")).is_err());
    }
}
