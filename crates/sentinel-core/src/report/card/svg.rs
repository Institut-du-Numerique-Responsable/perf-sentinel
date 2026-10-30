//! The card as an SVG: the image's operations drawn as paths, its text as
//! glyph outlines from the bundled fonts, the signed envelope in one
//! `<metadata>` element, and that envelope read back without an XML parser.

use std::fmt::Write as _;
use std::ops::Range;

use skrifa::outline::OutlinePen;

use super::layout::{CARD_HEIGHT, CARD_WIDTH};
use super::payload::Lang;
use super::raster::{CARD_PAYLOAD_KEYWORD, MAX_ENVELOPE_BYTES, draw_glyph, for_each_glyph};
use super::{CardError, CardResult};
use crate::report::periodic::pdf::layout::Face;
use crate::report::periodic::pdf::logo::{self, Segment};
use crate::report::periodic::pdf::pages::{Op, Outline, Rgb};

/// The `id` of the `<metadata>` element holding the signed envelope.
pub const SVG_METADATA_ID: &str = CARD_PAYLOAD_KEYWORD;
/// The largest SVG `verify-card` reads.
pub const MAX_CARD_SVG_BYTES: usize = 16 * 1024 * 1024;

const METADATA_CLOSE: &str = "</metadata>";
const TITLE_CLOSE: &str = "</title>";

fn metadata_open() -> String {
    format!("<metadata id=\"{SVG_METADATA_ID}\">")
}

/// A byte the canonical envelope can hold: JSON punctuation and base64url,
/// so it needs no escaping and the reader can refuse anything else.
fn envelope_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b"{}\":,_-".contains(&b)
}

/// Whether `bytes` read as an SVG: `<?xml` or `<svg` after an optional
/// UTF-8 BOM and ASCII whitespace.
#[must_use]
pub fn looks_like_svg(bytes: &[u8]) -> bool {
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    let start = bytes
        .iter()
        .position(|b| !b.is_ascii_whitespace())
        .unwrap_or(bytes.len());
    let bytes = &bytes[start..];
    bytes.starts_with(b"<?xml") || bytes.starts_with(b"<svg")
}

/// The image's operations as an SVG of 1080 x 1350, the envelope in
/// `<metadata id="perf-sentinel-card">`. Same operations, same bytes.
///
/// # Errors
///
/// `svg_encode_failed` when the envelope holds a byte the reader refuses.
pub(crate) fn render_svg(
    ops: &[Op],
    envelope: &str,
    title: &str,
    lang: Lang,
) -> Result<Vec<u8>, CardError> {
    if !envelope.bytes().all(envelope_byte) {
        return Err(CardError::new(
            CardResult::Unrenderable,
            "svg_encode_failed",
            "the envelope holds a byte the SVG reader refuses",
        ));
    }
    let lang = match lang {
        Lang::En => "en",
        Lang::Fr => "fr",
    };
    let (width, height) = (num(CARD_WIDTH), num(CARD_HEIGHT));
    let mut out = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{width}\" height=\"{height}\" viewBox=\"0 0 {width} {height}\" xml:lang=\"{lang}\" role=\"img\">\n<title>{}</title>\n{}{envelope}{METADATA_CLOSE}\n",
        escape(title),
        metadata_open(),
    );
    for (index, op) in ops.iter().enumerate() {
        element(&mut out, index, op);
        out.push('\n');
    }
    out.push_str("</svg>\n");
    Ok(out.into_bytes())
}

/// The envelope a card SVG carries.
///
/// # Errors
///
/// `file_too_large`, then as [`svg_envelope_range`], or `malformed_input`
/// for an envelope over 64 KiB or holding a byte outside its set.
pub fn read_svg_envelope(svg: &[u8]) -> Result<String, CardError> {
    if svg.len() > MAX_CARD_SVG_BYTES {
        return Err(input_error(
            "file_too_large",
            &format!("the SVG is over {MAX_CARD_SVG_BYTES} bytes"),
        ));
    }
    let element = svg_envelope_range(svg)?;
    let envelope = &svg[element.start + metadata_open().len()..element.end - METADATA_CLOSE.len()];
    if envelope.len() > MAX_ENVELOPE_BYTES {
        return Err(malformed(&format!(
            "the card payload is over {MAX_ENVELOPE_BYTES} bytes"
        )));
    }
    if !envelope.iter().copied().all(envelope_byte) {
        return Err(malformed("the card payload holds a byte outside its set"));
    }
    Ok(String::from_utf8_lossy(envelope).into_owned())
}

fn input_error(reason: &'static str, detail: &str) -> CardError {
    CardError::new(CardResult::InputError, reason, detail)
}

fn malformed(detail: &str) -> CardError {
    input_error("malformed_input", detail)
}

/// The card's `<metadata>` element, its opening tag through its closing
/// one, found exactly once and only where the engine writes it: right after
/// the first `</title>` and its newline. Removed from any other place, it
/// would leave the same bytes, so a moved element is refused, not hashed.
///
/// # Errors
///
/// `malformed_input` (not UTF-8, not an SVG, no closing tag, the element
/// elsewhere), `no_card_payload` or `duplicate_card_payload`.
pub(crate) fn svg_envelope_range(svg: &[u8]) -> Result<Range<usize>, CardError> {
    let text = std::str::from_utf8(svg).map_err(|_| malformed("the SVG is not UTF-8"))?;
    if !looks_like_svg(svg) {
        return Err(malformed("not an SVG"));
    }
    let open = metadata_open();
    let mut found = text.match_indices(&open);
    let Some((at, _)) = found.next() else {
        return Err(input_error(
            "no_card_payload",
            "the SVG carries no card payload",
        ));
    };
    if found.next().is_some() {
        return Err(input_error(
            "duplicate_card_payload",
            "the SVG carries more than one card payload",
        ));
    }
    let end = text[at..]
        .find(METADATA_CLOSE)
        .ok_or_else(|| malformed("the card payload has no closing tag"))?;
    let title_end = text.find(TITLE_CLOSE).map(|t| t + TITLE_CLOSE.len() + 1);
    if title_end != Some(at) || !text[..at].ends_with(&format!("{TITLE_CLOSE}\n")) {
        return Err(malformed(
            "the card payload is not right after the SVG's title",
        ));
    }
    Ok(at..at + end + METADATA_CLOSE.len())
}

/// Fixed point, at most two decimals, no trailing zero, no `-0`, never an
/// exponent: the same operations always print the same bytes.
fn num(v: f32) -> String {
    let text = format!("{v:.2}");
    let text = text.trim_end_matches('0').trim_end_matches('.');
    if text == "-0" {
        "0".to_owned()
    } else {
        text.to_owned()
    }
}

fn hex((r, g, b): Rgb) -> String {
    format!("#{r:02x}{g:02x}{b:02x}")
}

fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(c),
        }
    }
    out
}

/// One op on one line, `index` naming the logo's gradient.
fn element(out: &mut String, index: usize, op: &Op) {
    match op {
        Op::Text {
            x,
            y,
            face,
            size,
            color,
            text,
        } => {
            let d = text_path(*face, *size, (*x, *y), text);
            let _ = write!(out, "<path fill=\"{}\" d=\"{d}\"/>", hex(*color));
        }
        Op::Box {
            x,
            y,
            w,
            h,
            radius,
            fill,
            outline,
        } => rect(out, (*x, *y, *w, *h), *radius, *fill, *outline),
        Op::Circle { cx, cy, r, fill } => {
            let _ = write!(
                out,
                "<circle cx=\"{}\" cy=\"{}\" r=\"{}\" fill=\"{}\"/>",
                num(*cx),
                num(*cy),
                num(*r),
                hex(*fill)
            );
        }
        Op::Qr {
            x,
            y,
            side,
            modules,
        } => qr(out, (*x, *y), *side, modules),
        Op::Logo { x, y, height } => draw_logo(out, index, (*x, *y), *height),
    }
}

fn rect(
    out: &mut String,
    (x, y, w, h): (f32, f32, f32, f32),
    radius: f32,
    fill: Option<Rgb>,
    outline: Option<Outline>,
) {
    let _ = write!(
        out,
        "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\"",
        num(x),
        num(y),
        num(w),
        num(h)
    );
    // The raster's clamp, so both renders round the same corners.
    let corner = radius.min(w / 2.0).min(h / 2.0).max(0.0);
    if corner > 0.0 {
        let _ = write!(out, " rx=\"{}\"", num(corner));
    }
    let _ = write!(
        out,
        " fill=\"{}\"",
        fill.map_or_else(|| "none".to_owned(), hex)
    );
    if let Some(outline) = outline {
        let _ = write!(
            out,
            " stroke=\"{}\" stroke-width=\"{}\"",
            hex(outline.color),
            num(outline.width)
        );
        if outline.dashed {
            // The PDF's dash, as the raster draws it.
            out.push_str(" stroke-dasharray=\"2.5 2\"");
        }
    }
    out.push_str("/>");
}

/// One subpath per dark module, without anti-aliasing.
fn qr(out: &mut String, (x, y): (f32, f32), side: f32, modules: &[Vec<bool>]) {
    #[allow(clippy::cast_precision_loss)] // at most 177 modules a side
    let unit = side / modules.len().max(1) as f32;
    let m = num(unit);
    out.push_str("<path fill=\"#000000\" shape-rendering=\"crispEdges\" d=\"");
    for (row, line) in modules.iter().enumerate() {
        for (col, _) in line.iter().enumerate().filter(|(_, dark)| **dark) {
            #[allow(clippy::cast_precision_loss)] // at most 177 modules a side
            let (cx, cy) = (x + col as f32 * unit, y + row as f32 * unit);
            let _ = write!(out, "M{} {}h{m}v{m}h-{m}z", num(cx), num(cy));
        }
    }
    out.push_str("\"/>");
}

/// Path data written as the outlines are walked, `y` flipped from font
/// units to the page as the raster `Pen` does.
struct SvgPen<'a> {
    d: &'a mut String,
    x: f32,
    baseline: f32,
}

impl SvgPen<'_> {
    fn point(&mut self, command: char, points: &[(f32, f32)]) {
        self.d.push(command);
        for (i, (x, y)) in points.iter().enumerate() {
            if i > 0 {
                self.d.push(' ');
            }
            let _ = write!(self.d, "{} {}", num(self.x + x), num(self.baseline - y));
        }
    }
}

impl OutlinePen for SvgPen<'_> {
    fn move_to(&mut self, x: f32, y: f32) {
        self.point('M', &[(x, y)]);
    }

    fn line_to(&mut self, x: f32, y: f32) {
        self.point('L', &[(x, y)]);
    }

    fn quad_to(&mut self, cx0: f32, cy0: f32, x: f32, y: f32) {
        self.point('Q', &[(cx0, cy0), (x, y)]);
    }

    fn curve_to(&mut self, cx0: f32, cy0: f32, cx1: f32, cy1: f32, x: f32, y: f32) {
        self.point('C', &[(cx0, cy0), (cx1, cy1), (x, y)]);
    }

    fn close(&mut self) {
        self.d.push('Z');
    }
}

fn text_path(face: Face, size: f32, (x, y): (f32, f32), text: &str) -> String {
    let mut d = String::new();
    for_each_glyph(face, size, text, x, |glyph, pen_x| {
        let mut pen = SvgPen {
            d: &mut d,
            x: pen_x,
            baseline: y,
        };
        draw_glyph(glyph, size, &mut pen);
    });
    d
}

/// The logo's paths, scaled and moved into place in their coordinates, so
/// no `transform` attribute is needed.
fn draw_logo(out: &mut String, index: usize, (x, y): (f32, f32), height: f32) {
    let logo = logo::logo();
    let [min_x, min_y, _, view_height] = logo.view_box;
    let scale = height / view_height;
    let at = |px: f32, py: f32| (num(x + (px - min_x) * scale), num(y + (py - min_y) * scale));
    let point = |px: f32, py: f32| {
        let (px, py) = at(px, py);
        format!("{px} {py}")
    };
    for path in &logo.paths {
        let fill = match path.paint {
            logo::Paint::Solid(rgb) => hex(rgb),
            logo::Paint::Gradient { from, to } => {
                // shortcut: one id per Logo op, the bundled logo has one
                // gradient; number them per path if it ever gains a second.
                let ((x1, y1), (x2, y2)) = (at(from.0, from.1), at(to.0, to.1));
                let _ = write!(
                    out,
                    "<defs><linearGradient id=\"ps-logo-{index}\" gradientUnits=\"userSpaceOnUse\" x1=\"{x1}\" y1=\"{y1}\" x2=\"{x2}\" y2=\"{y2}\"><stop offset=\"0\" stop-color=\"{}\"/><stop offset=\"1\" stop-color=\"{}\"/></linearGradient></defs>",
                    hex(from.2),
                    hex(to.2)
                );
                format!("url(#ps-logo-{index})")
            }
        };
        let mut d = String::new();
        for segment in &path.segments {
            match *segment {
                Segment::Move(px, py) => {
                    let _ = write!(d, "M{}", point(px, py));
                }
                Segment::Line(px, py) => {
                    let _ = write!(d, "L{}", point(px, py));
                }
                Segment::Cubic(x1, y1, x2, y2, px, py) => {
                    let _ = write!(d, "C{} {} {}", point(x1, y1), point(x2, y2), point(px, py));
                }
                Segment::Close => d.push('Z'),
            }
        }
        let _ = write!(out, "<path fill=\"{fill}\" d=\"{d}\"/>");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detect::FindingType;
    use crate::report::card::content::CONTENT_SIG_PLACEHOLDER;
    use crate::report::card::copy::story_sentence;
    use crate::report::card::keys::{CardSigner, file_envelope, generate_key};
    use crate::report::card::layout::{UNDERLINE_PX, layout_image};
    use crate::report::card::payload::canonical_bytes;
    use crate::report::card::pdf::card_title;
    use crate::report::card::test_fixtures::{
        collective_input, compact_url_of, curve_input, full_payload, story_input,
    };
    use crate::report::periodic::pdf::logo::{self, Paint};
    use crate::report::periodic::pdf::pages::BRAND_TEXT;
    use serde_json::{Value, json};

    fn story() -> Value {
        story_input(FindingType::NPlusOneSql)
    }

    fn in_french(mut input: Value) -> Value {
        input["lang"] = json!("fr");
        input
    }

    fn ops(input: &Value) -> Vec<Op> {
        layout_image(&full_payload(input), &compact_url_of(input)).expect("the image lays out")
    }

    /// The full payload of `input` signed by a fresh key, in its envelope.
    fn envelope(input: &Value) -> String {
        let signer = CardSigner::from_pkcs8(&generate_key().expect("key").pkcs8).expect("signer");
        let bytes = canonical_bytes(&full_payload(input));
        file_envelope(&bytes, &signer.sign_b64(&bytes), CONTENT_SIG_PLACEHOLDER)
    }

    fn svg_with(input: &Value, envelope: &str) -> Vec<u8> {
        let payload = full_payload(input);
        render_svg(&ops(input), envelope, card_title(&payload), payload.lang)
            .expect("the image renders as SVG")
    }

    fn svg_of(input: &Value) -> String {
        String::from_utf8(svg_with(input, &envelope(input))).expect("the SVG is UTF-8")
    }

    /// The SVG with the envelope's text taken out, so random signature
    /// bytes cannot fake or hide a match.
    fn without_envelope(svg: &str) -> String {
        let open = format!("<metadata id=\"{SVG_METADATA_ID}\">");
        let start = svg.find(&open).expect("the metadata") + open.len();
        let end = start + svg[start..].find("</metadata>").expect("its end");
        format!("{}{}", &svg[..start], &svg[end..])
    }

    /// The lines holding one op each, in op order.
    fn op_lines(svg: &str) -> Vec<&str> {
        let lines: Vec<&str> = svg.lines().collect();
        lines[4..lines.len() - 1].to_vec()
    }

    #[test]
    fn same_card_renders_the_same_svg_bytes() {
        let input = story();
        let envelope = envelope(&input);
        assert_eq!(svg_with(&input, &envelope), svg_with(&input, &envelope));
    }

    #[test]
    fn the_svg_starts_with_its_fixed_header() {
        for (input, lang) in [(story(), "en"), (in_french(story()), "fr")] {
            let svg = svg_of(&input);
            let header = format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"1080\" height=\"1350\" viewBox=\"0 0 1080 1350\" xml:lang=\"{lang}\" role=\"img\">\n<title>"
            );
            assert!(svg.starts_with(&header), "{}", &svg[..200]);
            assert!(svg.ends_with("</svg>\n"));
        }
    }

    #[test]
    fn the_svg_carries_the_envelope_once() {
        let input = story();
        let envelope = envelope(&input);
        let svg = svg_with(&input, &envelope);
        assert_eq!(
            read_svg_envelope(&svg).expect("the envelope reads"),
            envelope
        );
        let text = String::from_utf8(svg).expect("utf-8");
        assert_eq!(
            text.matches("<metadata id=\"perf-sentinel-card\">").count(),
            1
        );
    }

    #[test]
    fn the_svg_holds_only_drawing_elements() {
        const ALLOWED: [&str; 9] = [
            "svg",
            "title",
            "metadata",
            "defs",
            "linearGradient",
            "stop",
            "path",
            "rect",
            "circle",
        ];
        let svg = without_envelope(&svg_of(&story()));
        for tag in svg.split('<').skip(1) {
            if tag.starts_with('?') || tag.starts_with('/') {
                continue;
            }
            let name: String = tag
                .chars()
                .take_while(char::is_ascii_alphanumeric)
                .collect();
            assert!(ALLOWED.contains(&name.as_str()), "<{name}");
            let inside = &tag[..tag.find('>').expect("the tag closes")];
            let handler = inside.split_whitespace().any(|attribute| {
                attribute.starts_with("on")
                    && attribute[2..]
                        .split('=')
                        .next()
                        .is_some_and(|rest| rest.chars().all(|c| c.is_ascii_lowercase()))
                    && attribute.contains('=')
            });
            assert!(!handler, "an event handler in <{inside}>");
        }
        for banned in [
            "<text",
            "<style",
            "<script",
            "<a ",
            "<image",
            "<use",
            "<foreignObject",
            "href",
        ] {
            assert!(!svg.contains(banned), "{banned}");
        }
        assert_eq!(svg.matches("http").count(), 1);
        assert!(svg.contains("xmlns=\"http://www.w3.org/2000/svg\""));
        let urls = svg.matches("url(").count();
        assert!(urls > 0, "the logo's gradient");
        assert_eq!(svg.matches("url(#ps-logo-").count(), urls);
    }

    #[test]
    fn every_op_is_drawn_once() {
        let input = story();
        let ops = ops(&input);
        let svg = without_envelope(&svg_of(&input));
        let count = |f: fn(&Op) -> bool| ops.iter().filter(|op| f(op)).count();
        assert_eq!(
            svg.matches("<rect").count(),
            count(|op| matches!(op, Op::Box { .. }))
        );
        assert_eq!(
            svg.matches("<circle").count(),
            count(|op| matches!(op, Op::Circle { .. }))
        );
        let solid_logo_paths = logo::logo()
            .paths
            .iter()
            .filter(|path| matches!(path.paint, Paint::Solid(_)))
            .count();
        let qr = "<path fill=\"#000000\" shape-rendering=\"crispEdges\"";
        assert_eq!(
            svg.matches("<path fill=\"#").count() - svg.matches(qr).count(),
            count(|op| matches!(op, Op::Text { .. }))
                + solid_logo_paths * count(|op| matches!(op, Op::Logo { .. }))
        );
        assert_eq!(svg.matches("shape-rendering=\"crispEdges\"").count(), 1);
        let dark: usize = ops
            .iter()
            .filter_map(|op| match op {
                Op::Qr { modules, .. } => {
                    Some(modules.iter().flatten().filter(|dark| **dark).count())
                }
                _ => None,
            })
            .sum();
        let qr_line = svg
            .lines()
            .find(|line| line.starts_with(qr))
            .expect("the QR");
        assert_eq!(qr_line.matches('M').count(), dark);
    }

    #[test]
    fn text_is_outlined_from_the_bundled_fonts() {
        let input = story();
        let ops = ops(&input);
        let svg = svg_of(&input);
        let sentence = story_sentence(&full_payload(&input));
        let (index, x, size) = ops
            .iter()
            .enumerate()
            .find_map(|(i, op)| match op {
                Op::Text { x, size, text, .. }
                    if !text.trim().is_empty() && sentence.starts_with(text.trim()) =>
                {
                    Some((i, *x, *size))
                }
                _ => None,
            })
            .expect("the story sentence is drawn");
        let line = op_lines(&svg)[index];
        let d = line
            .strip_prefix("<path fill=\"#20261f\" d=\"M")
            .unwrap_or_else(|| panic!("{line}"));
        let first_x: f32 = d
            .split(' ')
            .next()
            .and_then(|n| n.parse().ok())
            .expect("a number");
        assert!(first_x >= x && first_x <= x + size, "{first_x} against {x}");
    }

    #[test]
    fn the_names_are_underlined_in_the_svg() {
        let input = story();
        let svg = svg_of(&input);
        let expected: Vec<String> = ops(&input)
            .iter()
            .filter_map(|op| match op {
                Op::Box {
                    x,
                    y,
                    w,
                    h,
                    fill: Some(fill),
                    ..
                } if *fill == BRAND_TEXT && (h - UNDERLINE_PX).abs() < 1e-3 => Some(format!(
                    "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"2\" fill=\"#107a56\"/>",
                    num(*x),
                    num(*y),
                    num(*w)
                )),
                _ => None,
            })
            .collect();
        assert_eq!(expected.len(), 2);
        let drawn: Vec<&str> = svg
            .lines()
            .filter(|line| {
                line.starts_with("<rect")
                    && line.contains("fill=\"#107a56\"")
                    && line.contains("height=\"2\"")
            })
            .collect();
        assert_eq!(drawn, expected);
    }

    #[test]
    fn the_title_is_escaped() {
        let mut titled = story();
        titled["free"]["title"] = json!("<b>Q3</b> & \"review\" it's");
        assert!(
            svg_of(&titled).contains(
                "<title>&lt;b&gt;Q3&lt;/b&gt; &amp; &quot;review&quot; it&apos;s</title>"
            )
        );
        assert!(svg_of(&story()).contains("<title>Perf Sentinel card</title>"));
    }

    #[test]
    fn numbers_are_fixed_point() {
        assert_eq!(num(0.0), "0");
        assert_eq!(num(-0.001), "0");
        assert_eq!(num(12.5), "12.5");
        assert_eq!(num(3.456), "3.46");
        assert_eq!(num(1080.0), "1080");
        let svg = without_envelope(&svg_of(&story()));
        // Colours are hex, not numbers: blank them before reading numbers.
        let mut text = String::new();
        let mut chars = svg.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '#' && chars.peek().is_some_and(char::is_ascii_hexdigit) {
                while chars.peek().is_some_and(char::is_ascii_hexdigit) {
                    chars.next();
                }
                text.push(' ');
            } else {
                text.push(c);
            }
        }
        // Past the fixed header, whose `w3.org` and `1.0` are not numbers.
        let body = text.splitn(3, '\n').nth(2).expect("a header");
        for (i, c) in body.char_indices() {
            if c == 'e' || c == 'E' {
                let before = body[..i].chars().last();
                assert!(
                    !before.is_some_and(|b| b.is_ascii_digit()),
                    "an exponent at {i}"
                );
            }
        }
        let numbers = body.split(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-'));
        for number in numbers.filter(|n| n.chars().any(|c| c.is_ascii_digit())) {
            assert_ne!(number, "-0");
            if let Some((_, decimals)) = number.split_once('.') {
                assert!(
                    !decimals.is_empty() && decimals.len() <= 2 && !decimals.ends_with('0'),
                    "{number}"
                );
            }
        }
    }

    #[test]
    fn a_svg_without_its_metadata_has_no_card_payload() {
        let input = story();
        let envelope = envelope(&input);
        let svg = String::from_utf8(svg_with(&input, &envelope)).expect("utf-8");
        let element = format!("<metadata id=\"perf-sentinel-card\">{envelope}</metadata>\n");
        let reason = |bytes: &[u8]| read_svg_envelope(bytes).expect_err("refused").reason;
        assert_eq!(
            reason(svg.replace(&element, "").as_bytes()),
            "no_card_payload"
        );
        assert_eq!(
            reason(svg.replace(&element, &element.repeat(2)).as_bytes()),
            "duplicate_card_payload"
        );
        let script = "<metadata id=\"perf-sentinel-card\"><script/></metadata>\n";
        assert_eq!(
            reason(svg.replace(&element, script).as_bytes()),
            "malformed_input"
        );
        assert_eq!(
            reason(svg.replace("</metadata>", "").as_bytes()),
            "malformed_input"
        );
        assert_eq!(reason(b"GIF89a"), "malformed_input");
        let mut huge = svg.clone().into_bytes();
        huge.resize(MAX_CARD_SVG_BYTES + 1, b' ');
        assert_eq!(reason(&huge), "file_too_large");
        let bom = format!("\u{feff}\n{svg}");
        assert_eq!(
            read_svg_envelope(bom.as_bytes()).expect("a BOM and a newline are skipped"),
            envelope
        );
    }

    #[test]
    fn an_envelope_byte_outside_the_set_is_svg_encode_failed() {
        let ops = ops(&story());
        for bad in ["<", " ", "&", "é"] {
            let envelope = format!("{{\"a\":\"{bad}\"}}");
            let error =
                render_svg(&ops, &envelope, "Perf Sentinel card", Lang::En).expect_err("refused");
            assert_eq!(error.reason, "svg_encode_failed", "{bad:?}");
            assert_eq!(error.result, CardResult::Unrenderable);
            assert_eq!(error.result.exit_code(), 5);
        }
    }

    #[test]
    fn the_svg_stays_small() {
        for input in [story(), curve_input(), collective_input()] {
            for input in [input.clone(), in_french(input)] {
                let size = svg_of(&input).len();
                eprintln!(
                    "{} {}: {size} bytes",
                    input["angle"].as_str().unwrap_or_default(),
                    input["lang"].as_str().unwrap_or("en")
                );
                assert!(size < 1024 * 1024, "{size} bytes");
            }
        }
    }
}
