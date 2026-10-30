//! The card as pixels: the drawing operations rasterized with tiny-skia,
//! written as an opaque 8-bit RGB PNG whose `iTXt` chunk carries the
//! signed envelope, and that envelope read back without decoding a pixel.

use std::io::Cursor;

use skrifa::instance::{LocationRef, Size};
use skrifa::outline::{DrawSettings, OutlinePen};
use skrifa::{FontRef, MetadataProvider};
use tiny_skia::{
    Color, FillRule, GradientStop, LinearGradient, Paint, Path, PathBuilder, Pixmap, Point, Rect,
    SpreadMode, Stroke, StrokeDash, Transform,
};

use super::layout::{CARD_HEIGHT, CARD_WIDTH};
use super::{CardError, CardResult};
use crate::report::periodic::pdf::layout::{Face, text_width};
use crate::report::periodic::pdf::logo::{self, Segment};
use crate::report::periodic::pdf::pages::{Op, Outline, Rgb};

/// The `iTXt` keyword of the chunk holding the signed envelope.
pub const CARD_PAYLOAD_KEYWORD: &str = "perf-sentinel-card";
/// The largest PNG `verify-card` reads.
pub const MAX_CARD_PNG_BYTES: usize = 16 * 1024 * 1024;
/// The largest envelope a chunk may hold.
const MAX_ENVELOPE_BYTES: usize = 64 * 1024;

fn input_error(reason: &'static str, detail: impl Into<String>) -> CardError {
    CardError::new(CardResult::InputError, reason, detail)
}

fn encode_failed(detail: impl Into<String>) -> CardError {
    CardError::new(CardResult::Unrenderable, "png_encode_failed", detail)
}

/// The card's operations drawn on a white 1080 x 1350 pixmap.
///
/// # Errors
///
/// `png_encode_failed` when the pixmap cannot be allocated.
#[cfg_attr(not(test), expect(dead_code, reason = "called by render_card, Task 8"))]
pub(crate) fn rasterize(ops: &[Op]) -> Result<Pixmap, CardError> {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // 1080 x 1350
    let mut pixmap = Pixmap::new(CARD_WIDTH as u32, CARD_HEIGHT as u32)
        .ok_or_else(|| encode_failed("the card pixmap cannot be allocated"))?;
    pixmap.fill(Color::WHITE);
    for op in ops {
        draw(&mut pixmap, op);
    }
    Ok(pixmap)
}

/// `pixmap` as an 8-bit RGB PNG, the envelope in an `iTXt` chunk ahead of
/// the pixels.
///
/// # Errors
///
/// `png_encode_failed` when the encoder refuses the chunk or the pixels.
#[cfg_attr(not(test), expect(dead_code, reason = "called by render_card, Task 8"))]
pub(crate) fn encode_png(pixmap: &Pixmap, envelope: &str) -> Result<Vec<u8>, CardError> {
    let rgb: Vec<u8> = pixmap
        .pixels()
        .iter()
        .flat_map(|pixel| {
            let color = pixel.demultiply();
            [color.red(), color.green(), color.blue()]
        })
        .collect();
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, pixmap.width(), pixmap.height());
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    // Added before the header is written, so the chunk precedes IDAT.
    encoder
        .add_itxt_chunk(CARD_PAYLOAD_KEYWORD.to_owned(), envelope.to_owned())
        .map_err(|e| encode_failed(e.to_string()))?;
    let mut writer = encoder
        .write_header()
        .map_err(|e| encode_failed(e.to_string()))?;
    writer
        .write_image_data(&rgb)
        .map_err(|e| encode_failed(e.to_string()))?;
    writer.finish().map_err(|e| encode_failed(e.to_string()))?;
    Ok(out)
}

/// The envelope a card PNG carries, read from the chunks ahead of the
/// pixels without decoding them.
///
/// # Errors
///
/// `file_too_large`, `malformed_input` (not a PNG, or an envelope over
/// 64 KiB), `no_card_payload` or `duplicate_card_payload`.
pub fn read_png_envelope(png: &[u8]) -> Result<String, CardError> {
    if png.len() > MAX_CARD_PNG_BYTES {
        return Err(input_error(
            "file_too_large",
            format!("the PNG is over {MAX_CARD_PNG_BYTES} bytes"),
        ));
    }
    let malformed = |detail: String| input_error("malformed_input", detail);
    let reader = png::Decoder::new(Cursor::new(png))
        .read_info()
        .map_err(|e| malformed(format!("not a readable PNG: {e}")))?;
    let mut chunks = reader
        .info()
        .utf8_text
        .iter()
        .filter(|chunk| chunk.keyword == CARD_PAYLOAD_KEYWORD);
    let mut chunk = chunks
        .next()
        .ok_or_else(|| input_error("no_card_payload", "the PNG carries no card payload"))?
        .clone();
    if chunks.next().is_some() {
        return Err(input_error(
            "duplicate_card_payload",
            "the PNG carries more than one card payload",
        ));
    }
    chunk
        .decompress_text_with_limit(MAX_ENVELOPE_BYTES)
        .map_err(|e| malformed(format!("the card payload does not inflate: {e}")))?;
    let text = chunk
        .get_text()
        .map_err(|e| malformed(format!("the card payload is not text: {e}")))?;
    if text.len() > MAX_ENVELOPE_BYTES {
        return Err(malformed(format!(
            "the card payload is over {MAX_ENVELOPE_BYTES} bytes"
        )));
    }
    Ok(text)
}

fn draw(pixmap: &mut Pixmap, op: &Op) {
    match op {
        Op::Text {
            x,
            y,
            face,
            size,
            color,
            text,
        } => draw_text(pixmap, (*x, *y), *face, *size, *color, text),
        Op::Box {
            x,
            y,
            w,
            h,
            radius,
            fill,
            outline,
        } => draw_box(pixmap, (*x, *y, *w, *h), *radius, *fill, *outline),
        Op::Circle { cx, cy, r, fill } => {
            if let Some(path) = PathBuilder::from_circle(*cx, *cy, *r) {
                fill_path(pixmap, &path, &solid(*fill), Transform::identity());
            }
        }
        Op::Qr {
            x,
            y,
            side,
            modules,
        } => draw_qr(pixmap, (*x, *y), *side, modules),
        Op::Logo { x, y, height } => draw_logo(pixmap, (*x, *y), *height),
    }
}

fn solid(rgb: Rgb) -> Paint<'static> {
    let mut paint = Paint::default();
    paint.set_color_rgba8(rgb.0, rgb.1, rgb.2, 255);
    paint
}

fn fill_path(pixmap: &mut Pixmap, path: &Path, paint: &Paint<'_>, transform: Transform) {
    pixmap.fill_path(path, paint, FillRule::Winding, transform, None);
}

/// Glyph outlines into one path, `y` flipped from font units to the page.
struct Pen {
    builder: PathBuilder,
    x: f32,
    baseline: f32,
}

impl OutlinePen for Pen {
    fn move_to(&mut self, x: f32, y: f32) {
        self.builder.move_to(self.x + x, self.baseline - y);
    }

    fn line_to(&mut self, x: f32, y: f32) {
        self.builder.line_to(self.x + x, self.baseline - y);
    }

    fn quad_to(&mut self, cx0: f32, cy0: f32, x: f32, y: f32) {
        self.builder.quad_to(
            self.x + cx0,
            self.baseline - cy0,
            self.x + x,
            self.baseline - y,
        );
    }

    fn curve_to(&mut self, cx0: f32, cy0: f32, cx1: f32, cy1: f32, x: f32, y: f32) {
        self.builder.cubic_to(
            self.x + cx0,
            self.baseline - cy0,
            self.x + cx1,
            self.baseline - cy1,
            self.x + x,
            self.baseline - y,
        );
    }

    fn close(&mut self) {
        self.builder.close();
    }
}

/// Glyph by glyph from the face's charmap, each advance the one the line
/// breaking measured (no kerning).
fn draw_text(pixmap: &mut Pixmap, (x, y): (f32, f32), face: Face, size: f32, rgb: Rgb, text: &str) {
    let font = FontRef::new(face.bytes()).expect("bundled font parses");
    let charmap = font.charmap();
    let outlines = font.outline_glyphs();
    let mut pen = Pen {
        builder: PathBuilder::new(),
        x,
        baseline: y,
    };
    let mut utf8 = [0u8; 4];
    for c in text.chars() {
        if let Some(glyph) = charmap.map(c).and_then(|id| outlines.get(id)) {
            let settings = DrawSettings::unhinted(Size::new(size), LocationRef::default());
            // The layout checked every glyph exists; a bundled outline that
            // fails to draw leaves a gap rather than a broken card.
            let _ = glyph.draw(settings, &mut pen);
        }
        pen.x += text_width(face, size, c.encode_utf8(&mut utf8));
    }
    if let Some(path) = pen.builder.finish() {
        fill_path(pixmap, &path, &solid(rgb), Transform::identity());
    }
}

fn draw_box(
    pixmap: &mut Pixmap,
    (x, y, w, h): (f32, f32, f32, f32),
    radius: f32,
    fill: Option<Rgb>,
    outline: Option<Outline>,
) {
    let Some(path) = rounded_rect(x, y, w, h, radius) else {
        return;
    };
    if let Some(rgb) = fill {
        fill_path(pixmap, &path, &solid(rgb), Transform::identity());
    }
    if let Some(outline) = outline {
        let stroke = Stroke {
            width: outline.width,
            // The PDF's dash, so both renders of a card match.
            dash: outline
                .dashed
                .then(|| StrokeDash::new(vec![2.5, 2.0], 0.0))
                .flatten(),
            ..Stroke::default()
        };
        pixmap.stroke_path(
            &path,
            &solid(outline.color),
            &stroke,
            Transform::identity(),
            None,
        );
    }
}

/// A rectangle with its corners rounded by `radius`, cubic arcs at the
/// 0.5523 handle length the PDF uses.
fn rounded_rect(left: f32, top: f32, width: f32, height: f32, radius: f32) -> Option<Path> {
    let r = radius.min(width / 2.0).min(height / 2.0).max(0.0);
    let rect = Rect::from_xywh(left, top, width, height)?;
    if r == 0.0 {
        return Some(PathBuilder::from_rect(rect));
    }
    let straight = r * (1.0 - 0.552_285);
    let (right, bottom) = (left + width, top + height);
    // Each corner: its point, the direction the edge arrives in, and the
    // direction it leaves in, clockwise from the top edge.
    let corners = [
        ((right, top), (1.0, 0.0), (0.0, 1.0)),
        ((right, bottom), (0.0, 1.0), (-1.0, 0.0)),
        ((left, bottom), (-1.0, 0.0), (0.0, -1.0)),
        ((left, top), (0.0, -1.0), (1.0, 0.0)),
    ];
    let mut builder = PathBuilder::new();
    builder.move_to(left + r, top);
    for ((cx, cy), (ix, iy), (ox, oy)) in corners {
        builder.line_to(cx - ix * r, cy - iy * r);
        builder.cubic_to(
            cx - ix * straight,
            cy - iy * straight,
            cx + ox * straight,
            cy + oy * straight,
            cx + ox * r,
            cy + oy * r,
        );
    }
    builder.close();
    builder.finish()
}

/// Dark modules as rectangles without anti-aliasing, so a module on whole
/// pixels fills exactly its pixels.
fn draw_qr(pixmap: &mut Pixmap, (x, y): (f32, f32), side: f32, modules: &[Vec<bool>]) {
    #[allow(clippy::cast_precision_loss)] // at most 177 modules a side
    let unit = side / modules.len().max(1) as f32;
    let mut builder = PathBuilder::new();
    for (row, line) in modules.iter().enumerate() {
        for (col, dark) in line.iter().enumerate() {
            #[allow(clippy::cast_precision_loss)] // at most 177 modules a side
            let cell = Rect::from_xywh(x + col as f32 * unit, y + row as f32 * unit, unit, unit);
            if let (true, Some(cell)) = (*dark, cell) {
                builder.push_rect(cell);
            }
        }
    }
    if let Some(path) = builder.finish() {
        let mut paint = solid((0, 0, 0));
        paint.anti_alias = false;
        fill_path(pixmap, &path, &paint, Transform::identity());
    }
}

fn draw_logo(pixmap: &mut Pixmap, (x, y): (f32, f32), height: f32) {
    let logo = logo::logo();
    let [min_x, min_y, _, view_height] = logo.view_box;
    let scale = height / view_height;
    let transform =
        Transform::from_row(scale, 0.0, 0.0, scale, x - min_x * scale, y - min_y * scale);
    for path in &logo.paths {
        let Some(shape) = logo_path(&path.segments) else {
            continue;
        };
        let paint = match path.paint {
            logo::Paint::Solid(rgb) => solid(rgb),
            logo::Paint::Gradient { from, to } => {
                let stop = |at: f32, rgb: Rgb| {
                    GradientStop::new(at, Color::from_rgba8(rgb.0, rgb.1, rgb.2, 255))
                };
                let Some(shader) = LinearGradient::new(
                    Point::from_xy(from.0, from.1),
                    Point::from_xy(to.0, to.1),
                    vec![stop(0.0, from.2), stop(1.0, to.2)],
                    SpreadMode::Pad,
                    Transform::identity(),
                ) else {
                    continue;
                };
                Paint {
                    shader,
                    ..Paint::default()
                }
            }
        };
        fill_path(pixmap, &shape, &paint, transform);
    }
}

fn logo_path(segments: &[Segment]) -> Option<Path> {
    let mut builder = PathBuilder::new();
    for segment in segments {
        match *segment {
            Segment::Move(x, y) => builder.move_to(x, y),
            Segment::Line(x, y) => builder.line_to(x, y),
            Segment::Cubic(x1, y1, x2, y2, x, y) => builder.cubic_to(x1, y1, x2, y2, x, y),
            Segment::Close => builder.close(),
        }
    }
    builder.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detect::FindingType;
    use crate::report::card::copy::story_sentence;
    use crate::report::card::keys::{b64url_encode, file_envelope};
    use crate::report::card::layout::layout_image;
    use crate::report::card::payload::canonical_bytes;
    use crate::report::card::test_fixtures::{compact_url_of, full_payload, story_input};
    use crate::report::periodic::pdf::pages::Op;
    use serde_json::Value;

    fn story() -> Value {
        story_input(FindingType::NPlusOneSql)
    }

    fn ops(input: &Value) -> Vec<Op> {
        layout_image(&full_payload(input), &compact_url_of(input)).expect("the image lays out")
    }

    fn envelope(input: &Value) -> String {
        file_envelope(
            &canonical_bytes(&full_payload(input)),
            &b64url_encode(&[9u8; 64]),
        )
    }

    fn png_of(input: &Value) -> Vec<u8> {
        let pixmap = rasterize(&ops(input)).expect("the image rasterizes");
        encode_png(&pixmap, &envelope(input)).expect("the image encodes")
    }

    fn pixel(pixmap: &tiny_skia::Pixmap, x: f32, y: f32) -> (u8, u8, u8) {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // on the card
        let p = pixmap
            .pixel(x as u32, y as u32)
            .expect("the point is on the card")
            .demultiply();
        (p.red(), p.green(), p.blue())
    }

    fn dark(rgb: (u8, u8, u8)) -> bool {
        u16::from(rgb.0) + u16::from(rgb.1) + u16::from(rgb.2) < 3 * 128
    }

    /// RGB pixels and size of a PNG, decoded in full.
    fn decode(png: &[u8]) -> (png::OutputInfo, Vec<u8>) {
        let mut reader = png::Decoder::new(std::io::Cursor::new(png))
            .read_info()
            .expect("the PNG decodes");
        let mut pixels = vec![0; reader.output_buffer_size().expect("the size fits")];
        let info = reader.next_frame(&mut pixels).expect("the frame decodes");
        pixels.truncate(info.buffer_size());
        (info, pixels)
    }

    fn reencode(png: &[u8], chunks: &[&str]) -> Vec<u8> {
        let (info, pixels) = decode(png);
        let mut out = Vec::new();
        let mut encoder = png::Encoder::new(&mut out, info.width, info.height);
        encoder.set_color(info.color_type);
        encoder.set_depth(info.bit_depth);
        for text in chunks {
            encoder
                .add_itxt_chunk(CARD_PAYLOAD_KEYWORD.to_owned(), (*text).to_owned())
                .expect("the chunk is added");
        }
        let mut writer = encoder.write_header().expect("the header writes");
        writer.write_image_data(&pixels).expect("the pixels write");
        writer.finish().expect("the PNG finishes");
        out
    }

    #[test]
    fn same_card_renders_the_same_png_bytes() {
        let input = story();
        assert_eq!(png_of(&input), png_of(&input));
    }

    #[test]
    fn the_png_is_opaque_rgb_at_1080_by_1350() {
        let (info, _) = decode(&png_of(&story()));
        assert_eq!((info.width, info.height), (1080, 1350));
        assert_eq!(info.color_type, png::ColorType::Rgb);
        assert_eq!(info.bit_depth, png::BitDepth::Eight);
    }

    #[test]
    fn the_png_carries_the_envelope_once() {
        let input = story();
        assert_eq!(
            read_png_envelope(&png_of(&input)).expect("the envelope reads"),
            envelope(&input)
        );
    }

    #[test]
    fn qr_modules_land_on_whole_pixels() {
        let ops = ops(&story());
        let pixmap = rasterize(&ops).expect("the image rasterizes");
        let mut sampled = 0;
        for op in &ops {
            let Op::Qr {
                x,
                y,
                side,
                modules,
            } = op
            else {
                continue;
            };
            #[allow(clippy::cast_precision_loss)] // at most 177 modules a side
            let unit = side / modules.len() as f32;
            for (row, line) in modules.iter().enumerate() {
                for (col, is_dark) in line.iter().enumerate() {
                    #[allow(clippy::cast_precision_loss)] // at most 177 modules a side
                    let (cx, cy) = (x + (col as f32 + 0.5) * unit, y + (row as f32 + 0.5) * unit);
                    let expected = if *is_dark { (0, 0, 0) } else { (255, 255, 255) };
                    assert_eq!(pixel(&pixmap, cx, cy), expected, "module {row},{col}");
                    sampled += 1;
                }
            }
        }
        assert!(sampled > 0, "the image draws a QR code");
    }

    #[test]
    fn text_is_drawn() {
        let input = story();
        let ops = ops(&input);
        let pixmap = rasterize(&ops).expect("the image rasterizes");
        let sentence = story_sentence(&full_payload(&input));
        let (x, y, size, text) = ops
            .iter()
            .find_map(|op| match op {
                Op::Text {
                    x,
                    y,
                    face,
                    size,
                    text,
                    ..
                } if !text.trim().is_empty() && sentence.starts_with(text.trim()) => {
                    Some((*x, *y, *size, text_width(*face, *size, text)))
                }
                _ => None,
            })
            .expect("the story sentence is drawn");
        let region_has_dark = |left: f32, top: f32, width: f32, height: f32| {
            let mut found = false;
            let mut py = top;
            while py < top + height {
                let mut px = left;
                while px < left + width {
                    found |= dark(pixel(&pixmap, px, py));
                    px += 1.0;
                }
                py += 1.0;
            }
            found
        };
        assert!(
            region_has_dark(x, y - size, text, size * 1.2),
            "the sentence's first line has ink"
        );
        // The left margin below the header, where nothing is laid out.
        assert!(
            !region_has_dark(10.0, 300.0, 40.0, 300.0),
            "a blank region stays blank"
        );
    }

    #[test]
    fn a_png_without_its_chunk_has_no_card_payload() {
        let input = story();
        let png = png_of(&input);
        let error = read_png_envelope(&reencode(&png, &[])).expect_err("no chunk");
        assert_eq!(error.reason, "no_card_payload");
        let envelope = envelope(&input);
        let error =
            read_png_envelope(&reencode(&png, &[&envelope, &envelope])).expect_err("two chunks");
        assert_eq!(error.reason, "duplicate_card_payload");
        let error = read_png_envelope(b"GIF89a").expect_err("not a PNG");
        assert_eq!(error.reason, "malformed_input");
    }

    #[test]
    fn an_oversized_file_or_envelope_is_refused() {
        let error = read_png_envelope(&vec![0; MAX_CARD_PNG_BYTES + 1]).expect_err("too large");
        assert_eq!(error.reason, "file_too_large");
        let png = png_of(&story());
        let huge = "x".repeat(64 * 1024 + 1);
        let error = read_png_envelope(&reencode(&png, &[&huge])).expect_err("text too large");
        assert_eq!(error.reason, "malformed_input");
    }
}
