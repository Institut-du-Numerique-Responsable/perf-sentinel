//! The card as pixels: the drawing operations rasterized with tiny-skia,
//! written as an opaque 8-bit RGB PNG whose `iTXt` chunk carries the
//! signed envelope, and that envelope read back without decoding a pixel.

use std::ops::Range;

use skrifa::instance::{LocationRef, Size};
use skrifa::outline::{DrawSettings, OutlineGlyph, OutlinePen};
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
/// The largest envelope a PNG chunk or a PDF attachment may hold.
pub(crate) const MAX_ENVELOPE_BYTES: usize = 64 * 1024;

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

/// The pixmap's pixels as 8-bit RGB rows, the bytes the PNG and the JPEG
/// XL both encode.
pub(crate) fn opaque_rgb(pixmap: &Pixmap) -> Vec<u8> {
    pixmap
        .pixels()
        .iter()
        .flat_map(|pixel| {
            let color = pixel.demultiply();
            [color.red(), color.green(), color.blue()]
        })
        .collect()
}

/// `pixmap` as an 8-bit RGB PNG, the envelope in an `iTXt` chunk ahead of
/// the pixels.
///
/// # Errors
///
/// `png_encode_failed` when the encoder refuses the chunk or the pixels.
pub(crate) fn encode_png(pixmap: &Pixmap, envelope: &str) -> Result<Vec<u8>, CardError> {
    let rgb = opaque_rgb(pixmap);
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

/// The envelope a card PNG carries, read from its `iTXt` chunk through the
/// PNG card walk, without decoding a pixel.
///
/// # Errors
///
/// `file_too_large`, then as [`png_envelope_chunk`].
pub fn read_png_envelope(png: &[u8]) -> Result<String, CardError> {
    if png.len() > MAX_CARD_PNG_BYTES {
        return Err(input_error(
            "file_too_large",
            format!("the PNG is over {MAX_CARD_PNG_BYTES} bytes"),
        ));
    }
    let chunk = png_envelope_chunk(png)?;
    let text = &png[chunk.start + 8 + ITXT_TEXT_AT..chunk.end - 4];
    // png_envelope_chunk checked the text is UTF-8: nothing is replaced.
    Ok(String::from_utf8_lossy(text).into_owned())
}

const PNG_SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";
/// The text of the card's `iTXt` chunk starts after the keyword, its NUL,
/// and four zero bytes: no compression, method 0, empty language tag and
/// translated keyword.
const ITXT_TEXT_AT: usize = CARD_PAYLOAD_KEYWORD.len() + 5;

/// CRC-32 with the PNG polynomial, over a chunk's type and data.
pub(crate) fn png_crc(bytes: &[u8]) -> u32 {
    !bytes.iter().fold(u32::MAX, |crc, &byte| {
        (0..8).fold(crc ^ u32::from(byte), |crc, _| {
            if crc & 1 == 1 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            }
        })
    })
}

/// The chunk at `at`: its type and its whole range, length field through
/// CRC, checked against the file before any slicing.
fn png_chunk_at(png: &[u8], at: usize) -> Result<([u8; 4], Range<usize>), CardError> {
    let malformed = |detail: &str| input_error("malformed_input", detail);
    let header = png
        .get(at..at + 8)
        .ok_or_else(|| malformed("the PNG ends inside a chunk header"))?;
    let len = u32::from_be_bytes([header[0], header[1], header[2], header[3]]);
    if len > 0x7FFF_FFFF {
        return Err(malformed("a PNG chunk length is over 2^31 - 1"));
    }
    let end = (len as usize)
        .checked_add(at + 12)
        .filter(|&end| end <= png.len())
        .ok_or_else(|| malformed("a PNG chunk runs past the end of the file"))?;
    Ok(([header[4], header[5], header[6], header[7]], at..end))
}

/// Whether the chunk at `range` is an `iTXt` of the card keyword.
fn is_card_chunk(png: &[u8], kind: [u8; 4], range: &Range<usize>) -> bool {
    let data = &png[range.start + 8..range.end - 4];
    &kind == b"iTXt"
        && data.starts_with(CARD_PAYLOAD_KEYWORD.as_bytes())
        && data.get(CARD_PAYLOAD_KEYWORD.len()) == Some(&0)
}

/// The card's `iTXt` chunk, length field through CRC, by the PNG card walk
/// (`docs/SHARE-CARD.md`): the signature, `IHDR` of 13 bytes first, chunks
/// walked by their lengths up to the first `IEND`, exactly one card chunk,
/// ahead of the first `IDAT`, its CRC right, its header four zero bytes and
/// its text UTF-8 of at most 64 KiB.
///
/// # Errors
///
/// `malformed_input` for a broken walk or card chunk, `no_card_payload`
/// with no card chunk ahead of `IDAT`, `duplicate_card_payload` with two.
pub(crate) fn png_envelope_chunk(png: &[u8]) -> Result<Range<usize>, CardError> {
    let malformed = |detail: &str| input_error("malformed_input", detail);
    if !png.starts_with(PNG_SIGNATURE) {
        return Err(malformed("not a PNG"));
    }
    let (kind, ihdr) = png_chunk_at(png, PNG_SIGNATURE.len())?;
    if &kind != b"IHDR" || ihdr.len() != 13 + 12 {
        return Err(malformed("the PNG does not start with its IHDR chunk"));
    }
    let (mut at, mut seen_idat) = (ihdr.end, false);
    let (mut card, mut count) = (None, 0);
    loop {
        let (kind, range) = png_chunk_at(png, at)?;
        match &kind {
            b"IEND" => break,
            b"IDAT" => seen_idat = true,
            _ if is_card_chunk(png, kind, &range) => {
                count += 1;
                card = (!seen_idat).then(|| range.clone());
            }
            _ => {}
        }
        at = range.end;
    }
    match (count, card) {
        (0, _) | (1, None) => Err(input_error(
            "no_card_payload",
            "the PNG carries no card payload ahead of its pixels",
        )),
        (1, Some(range)) => check_card_chunk(png, range),
        _ => Err(input_error(
            "duplicate_card_payload",
            "the PNG carries more than one card payload",
        )),
    }
}

/// Rule 6 of the walk: the card chunk's CRC, its four zero header bytes,
/// and its text.
fn check_card_chunk(png: &[u8], range: Range<usize>) -> Result<Range<usize>, CardError> {
    let malformed = |detail: &str| input_error("malformed_input", detail);
    let body = &png[range.start + 4..range.end - 4];
    let crc = u32::from_be_bytes(
        png[range.end - 4..range.end]
            .try_into()
            .expect("a chunk ends with 4 CRC bytes"),
    );
    if png_crc(body) != crc {
        return Err(malformed("the card payload chunk's CRC is wrong"));
    }
    let data = &body[4..];
    if data.get(CARD_PAYLOAD_KEYWORD.len() + 1..ITXT_TEXT_AT) != Some(&[0; 4][..]) {
        return Err(malformed(
            "the card payload is compressed or carries a language tag",
        ));
    }
    let text = &data[ITXT_TEXT_AT..];
    if text.len() > MAX_ENVELOPE_BYTES {
        return Err(malformed(&format!(
            "the card payload is over {MAX_ENVELOPE_BYTES} bytes"
        )));
    }
    std::str::from_utf8(text).map_err(|_| malformed("the card payload is not UTF-8"))?;
    Ok(range)
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

/// Each glyph of `text` from the face's charmap with its pen x, starting
/// at `x`, each advance the one the line breaking measured (no kerning).
pub(crate) fn for_each_glyph(
    face: Face,
    size: f32,
    text: &str,
    x: f32,
    mut each: impl FnMut(&OutlineGlyph<'_>, f32),
) {
    let font = FontRef::new(face.bytes()).expect("bundled font parses");
    let charmap = font.charmap();
    let outlines = font.outline_glyphs();
    let mut pen_x = x;
    let mut utf8 = [0u8; 4];
    for c in text.chars() {
        if let Some(glyph) = charmap.map(c).and_then(|id| outlines.get(id)) {
            each(&glyph, pen_x);
        }
        pen_x += text_width(face, size, c.encode_utf8(&mut utf8));
    }
}

/// `glyph` drawn unhinted at `size` into `pen`. The layout checked every
/// glyph exists; a bundled outline that fails to draw leaves a gap rather
/// than a broken card.
pub(crate) fn draw_glyph(glyph: &OutlineGlyph<'_>, size: f32, pen: &mut impl OutlinePen) {
    let settings = DrawSettings::unhinted(Size::new(size), LocationRef::default());
    let _ = glyph.draw(settings, pen);
}

fn draw_text(pixmap: &mut Pixmap, (x, y): (f32, f32), face: Face, size: f32, rgb: Rgb, text: &str) {
    let mut pen = Pen {
        builder: PathBuilder::new(),
        x,
        baseline: y,
    };
    for_each_glyph(face, size, text, x, |glyph, pen_x| {
        pen.x = pen_x;
        draw_glyph(glyph, size, &mut pen);
    });
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
    use crate::report::card::content::CONTENT_SIG_PLACEHOLDER;
    use crate::report::card::copy::story_sentence;
    use crate::report::card::keys::{b64url_encode, file_envelope, generate_key};
    use crate::report::card::layout::{UNDERLINE_PX, layout_image};
    use crate::report::card::payload::canonical_bytes;
    use crate::report::card::test_fixtures::{
        compact_url_of, full_payload, png_chunk, png_chunks, splice, story_input,
    };
    use crate::report::card::verify::{CardSource, ContentVerdict, check_integrity};
    use crate::report::card::{CardFormat, render_card};
    use crate::report::periodic::pdf::pages::{BRAND_TEXT, Op, SURFACE};
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
            CONTENT_SIG_PLACEHOLDER,
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
    fn opaque_rgb_is_what_the_png_holds() {
        let pixmap = rasterize(&ops(&story())).expect("the image rasterizes");
        let (_, pixels) = decode(&encode_png(&pixmap, "{}").expect("the image encodes"));
        assert!(
            opaque_rgb(&pixmap) == pixels,
            "opaque_rgb is the PNG's pixels"
        );
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

    #[test]
    fn png_crc_matches_the_encoders() {
        let png = png_of(&story());
        let chunks = png_chunks(&png);
        assert!(chunks.len() >= 4);
        for (_, range) in chunks {
            let crc = u32::from_be_bytes(png[range.end - 4..range.end].try_into().expect("4"));
            assert_eq!(png_crc(&png[range.start + 4..range.end - 4]), crc);
        }
    }

    /// An `iTXt` chunk of the card keyword with the given header fields.
    fn itxt(flag: u8, method: u8, language: &[u8], translated: &[u8], text: &str) -> Vec<u8> {
        let mut data = b"perf-sentinel-card\0".to_vec();
        data.extend_from_slice(&[flag, method]);
        data.extend_from_slice(language);
        data.push(0);
        data.extend_from_slice(translated);
        data.push(0);
        data.extend_from_slice(text.as_bytes());
        png_chunk(*b"iTXt", &data)
    }

    #[test]
    fn the_png_walk_follows_the_shared_byte_rules() {
        let key = generate_key().expect("key");
        let input = serde_json::to_vec(&story()).expect("json");
        let png = render_card(&input, &key.pkcs8, CardFormat::Png)
            .expect("renders")
            .bytes;
        let chunks = png_chunks(&png);
        let range_of = |kind: &[u8; 4]| {
            chunks
                .iter()
                .find(|(k, _)| k == kind)
                .expect("the chunk")
                .1
                .clone()
        };
        let (ihdr, card, idat, iend) = (
            range_of(b"IHDR"),
            range_of(b"iTXt"),
            range_of(b"IDAT"),
            range_of(b"IEND"),
        );
        let text = read_png_envelope(&png).expect("reads");
        let one_walk = |bytes: &[u8]| {
            let walk = png_envelope_chunk(bytes).map(|_| ()).map_err(|e| e.reason);
            let read = read_png_envelope(bytes).map(|_| ()).map_err(|e| e.reason);
            assert_eq!(walk, read);
            walk
        };

        let mut first = png.clone();
        first[0] = b'X';
        let before_ihdr = splice(
            &splice(&png, card.clone(), b""),
            ihdr.start..ihdr.start,
            &png[card.clone()],
        );
        let no_iend = splice(&png, iend.clone(), b"");
        let cut_after_idat = png[..idat.end].to_vec();
        let mut crc = png.clone();
        crc[card.end - 1] ^= 1;
        let headers = [
            itxt(1, 0, b"", b"", &text),
            itxt(0, 1, b"", b"", &text),
            itxt(0, 0, b"en", b"", &text),
            itxt(0, 0, b"", b"x", &text),
            itxt(0, 0, &[b'a'; 1024], b"", &text),
        ];
        let mut malformed = vec![first, before_ihdr, no_iend, cut_after_idat, crc];
        malformed.extend(
            headers
                .iter()
                .map(|chunk| splice(&png, card.clone(), chunk)),
        );
        for bytes in &malformed {
            assert_eq!(one_walk(bytes), Err("malformed_input"));
            let error = check_integrity(CardSource::Png(bytes)).expect_err("refused");
            assert_eq!(error.reason, "malformed_input");
        }

        let mut appended = png.clone();
        appended.extend_from_slice(b"retouched");
        let mut idat_crc = png.clone();
        idat_crc[idat.end - 1] ^= 1;
        for bytes in [appended, idat_crc] {
            assert_eq!(one_walk(&bytes), Ok(()));
            let integrity = check_integrity(CardSource::Png(&bytes)).expect("checks");
            assert_eq!(integrity.content, ContentVerdict::Modified);
            assert_eq!(integrity.reason, "content_modified");
        }
    }

    #[test]
    fn the_names_are_underlined_in_the_png() {
        let ops = ops(&story());
        let pixmap = rasterize(&ops).expect("the image rasterizes");
        let underlines: Vec<(f32, f32, f32, f32)> = ops
            .iter()
            .filter_map(|op| match op {
                Op::Box {
                    x,
                    y,
                    w,
                    h,
                    fill: Some(fill),
                    ..
                } if *fill == BRAND_TEXT && (h - UNDERLINE_PX).abs() < 1e-3 => {
                    Some((*x, *y, *w, *h))
                }
                _ => None,
            })
            .collect();
        assert_eq!(underlines.len(), 2, "one underline per brand name");
        let near = |a: (u8, u8, u8), b: Rgb| {
            a.0.abs_diff(b.0) <= 8 && a.1.abs_diff(b.1) <= 8 && a.2.abs_diff(b.2) <= 8
        };
        for (x, y, w, h) in underlines {
            let middle = pixel(&pixmap, x + w / 2.0, y + h / 2.0);
            assert!(near(middle, BRAND_TEXT), "{middle:?} at the underline");
            let below = pixel(&pixmap, x + w / 2.0, y + h + 6.0);
            assert!(near(below, SURFACE), "{below:?} under the underline");
        }
    }
}
