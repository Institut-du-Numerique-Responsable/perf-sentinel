//! The card as a JPEG XL: the PNG's pixels, lossless, in a container the
//! engine writes by hand so a private `pscd` box can carry the signed
//! envelope ahead of the codestream, and that envelope read back by
//! walking the top-level boxes, never decoding a pixel.

use tiny_skia::Pixmap;

use super::raster::{MAX_CARD_PNG_BYTES, MAX_ENVELOPE_BYTES, opaque_rgb};
use super::{CardError, CardResult};

/// The private top-level box holding the signed envelope.
pub const JXL_ENVELOPE_BOX: [u8; 4] = *b"pscd";
/// The 12-byte box every JPEG XL container starts with.
pub const JXL_SIGNATURE_BOX: [u8; 12] = [
    0, 0, 0, 0x0C, b'J', b'X', b'L', b' ', 0x0D, 0x0A, 0x87, 0x0A,
];
/// The largest JPEG XL `verify-card` reads.
pub const MAX_CARD_JXL_BYTES: usize = MAX_CARD_PNG_BYTES;

/// A bare codestream starts with these two bytes.
const CODESTREAM_SIGNATURE: [u8; 2] = [0xFF, 0x0A];
/// `ftyp`: brand `jxl `, minor version 0, compatible with `jxl `.
const FTYP_PAYLOAD: &[u8; 12] = b"jxl \0\0\0\0jxl ";

fn encode_failed(detail: impl Into<String>) -> CardError {
    CardError::new(CardResult::Unrenderable, "jxl_encode_failed", detail)
}

fn input_error(reason: &'static str, detail: impl Into<String>) -> CardError {
    CardError::new(CardResult::InputError, reason, detail)
}

fn malformed(detail: impl Into<String>) -> CardError {
    input_error("malformed_input", detail)
}

/// `rgb` as a lossless JPEG XL codestream. One thread, so the bytes do not
/// depend on the host's core count.
fn encode_codestream(rgb: &[u8], width: u32, height: u32) -> Result<Vec<u8>, CardError> {
    let (width, height) = (width as usize, height as usize);
    let expected = width.checked_mul(height).and_then(|n| n.checked_mul(3));
    if width == 0 || height == 0 || expected != Some(rgb.len()) {
        return Err(encode_failed(format!(
            "{} RGB bytes for a {width} x {height} image",
            rgb.len()
        )));
    }
    let config = jixel::EncodeConfig::default()
        .with_lossless(true)
        .with_speed(jixel::Speed::Medium)
        .with_num_threads(1);
    let codestream = jixel::encode_image(rgb, width, height, &config)
        .map_err(|e| encode_failed(e.to_string()))?;
    if !codestream.starts_with(&CODESTREAM_SIGNATURE) {
        return Err(encode_failed("the encoder wrote no JPEG XL codestream"));
    }
    Ok(codestream)
}

fn push_box(out: &mut Vec<u8>, kind: [u8; 4], payload: &[u8]) -> Result<(), CardError> {
    let size = payload
        .len()
        .checked_add(8)
        .and_then(|size| u32::try_from(size).ok())
        .ok_or_else(|| encode_failed("a JPEG XL box is over 4 GiB"))?;
    out.extend_from_slice(&size.to_be_bytes());
    out.extend_from_slice(&kind);
    out.extend_from_slice(payload);
    Ok(())
}

/// `pixmap` as a JPEG XL container: the signature box, `ftyp`, the
/// envelope in a `pscd` box, then the codestream in a `jxlc` box.
///
/// # Errors
///
/// `jxl_encode_failed` when the encoder refuses the pixels or a box is
/// over 4 GiB.
pub(crate) fn encode_jxl(pixmap: &Pixmap, envelope: &str) -> Result<Vec<u8>, CardError> {
    let codestream = encode_codestream(&opaque_rgb(pixmap), pixmap.width(), pixmap.height())?;
    let mut out = Vec::with_capacity(64 + envelope.len() + codestream.len());
    out.extend_from_slice(&JXL_SIGNATURE_BOX);
    push_box(&mut out, *b"ftyp", FTYP_PAYLOAD)?;
    push_box(&mut out, JXL_ENVELOPE_BOX, envelope.as_bytes())?;
    push_box(&mut out, *b"jxlc", &codestream)?;
    Ok(out)
}

/// A box header at the start of `rest`: its type, its size if not 0 (the
/// box then runs to the end of the file), and the header's length.
fn box_header(rest: &[u8]) -> Result<([u8; 4], Option<u64>, usize), CardError> {
    let truncated = || malformed("the JPEG XL ends inside a box header");
    let [s0, s1, s2, s3, k0, k1, k2, k3, ..] = *rest else {
        return Err(truncated());
    };
    let kind = [k0, k1, k2, k3];
    match u32::from_be_bytes([s0, s1, s2, s3]) {
        0 => Ok((kind, None, 8)),
        1 => {
            let large: [u8; 8] = rest
                .get(8..16)
                .and_then(|bytes| bytes.try_into().ok())
                .ok_or_else(truncated)?;
            Ok((kind, Some(u64::from_be_bytes(large)), 16))
        }
        size => Ok((kind, Some(u64::from(size)), 8)),
    }
}

/// A box's type, its payload, and the bytes after it.
type SplitBox<'a> = ([u8; 4], &'a [u8], &'a [u8]);

/// The box at the start of `rest`: its type, its payload and what follows
/// it. The size is checked against what is left before any slicing.
fn split_box(rest: &[u8]) -> Result<SplitBox<'_>, CardError> {
    let (kind, size, header) = box_header(rest)?;
    let end = match size {
        None => rest.len(),
        Some(size) => {
            if size < header as u64 {
                return Err(malformed("a JPEG XL box is smaller than its header"));
            }
            usize::try_from(size)
                .ok()
                .filter(|&end| end <= rest.len())
                .ok_or_else(|| malformed("a JPEG XL box runs past the end of the file"))?
        }
    };
    Ok((kind, &rest[header..end], &rest[end..]))
}

/// The payload of the one top-level `pscd` box among `boxes`. Every other
/// box is skipped by its length and never read.
fn envelope_box(mut boxes: &[u8]) -> Result<&[u8], CardError> {
    let mut found = None;
    while !boxes.is_empty() {
        let (kind, payload, rest) = split_box(boxes)?;
        if kind == JXL_ENVELOPE_BOX && found.replace(payload).is_some() {
            return Err(input_error(
                "duplicate_card_payload",
                "the JPEG XL carries more than one card payload",
            ));
        }
        boxes = rest;
    }
    found.ok_or_else(|| input_error("no_card_payload", "the JPEG XL carries no card payload"))
}

/// The envelope a card JPEG XL carries, read from its top-level boxes
/// without decoding the codestream.
///
/// # Errors
///
/// `file_too_large`, `no_card_payload` (a bare codestream, or no `pscd`
/// box), `duplicate_card_payload`, or `malformed_input`: not a JPEG XL
/// container, a box whose size does not fit the file, an envelope over
/// 64 KiB or not UTF-8.
pub fn read_jxl_envelope(jxl: &[u8]) -> Result<String, CardError> {
    if jxl.len() > MAX_CARD_JXL_BYTES {
        return Err(input_error(
            "file_too_large",
            format!("the JPEG XL is over {MAX_CARD_JXL_BYTES} bytes"),
        ));
    }
    if jxl.starts_with(&CODESTREAM_SIGNATURE) {
        return Err(input_error(
            "no_card_payload",
            "a bare JPEG XL codestream carries no card payload",
        ));
    }
    let boxes = jxl
        .strip_prefix(&JXL_SIGNATURE_BOX)
        .ok_or_else(|| malformed("not a JPEG XL container"))?;
    let envelope = envelope_box(boxes)?;
    if envelope.len() > MAX_ENVELOPE_BYTES {
        return Err(malformed(format!(
            "the card payload is over {MAX_ENVELOPE_BYTES} bytes"
        )));
    }
    std::str::from_utf8(envelope)
        .map(str::to_owned)
        .map_err(|_| malformed("the card payload is not UTF-8"))
}

#[cfg(test)]
mod tests {
    use std::sync::OnceLock;

    use super::*;
    use crate::detect::FindingType;
    use crate::report::card::keys::{b64url_encode, file_envelope};
    use crate::report::card::layout::layout_image;
    use crate::report::card::payload::canonical_bytes;
    use crate::report::card::raster::{encode_png, rasterize};
    use crate::report::card::test_fixtures::{
        collective_input, compact_url_of, curve_input, full_payload, story_input,
    };
    use serde_json::Value;

    const CODESTREAM: [u8; 2] = [0xFF, 0x0A];

    fn pixmap_of(input: &Value) -> Pixmap {
        let ops = layout_image(&full_payload(input), &compact_url_of(input)).expect("lays out");
        rasterize(&ops).expect("the image rasterizes")
    }

    fn envelope(input: &Value) -> String {
        file_envelope(
            &canonical_bytes(&full_payload(input)),
            &b64url_encode(&[9u8; 64]),
        )
    }

    /// The story card's pixmap, envelope and JPEG XL, encoded once for
    /// every test that only reads them.
    fn story() -> &'static (Pixmap, String, Vec<u8>) {
        static STORY: OnceLock<(Pixmap, String, Vec<u8>)> = OnceLock::new();
        STORY.get_or_init(|| {
            let input = story_input(FindingType::NPlusOneSql);
            let pixmap = pixmap_of(&input);
            let envelope = envelope(&input);
            let jxl = encode_jxl(&pixmap, &envelope).expect("the story encodes");
            (pixmap, envelope, jxl)
        })
    }

    /// The top-level boxes after the signature box, as (type, payload).
    /// Test-side walker: 32-bit sizes only, as the engine writes them.
    fn boxes(jxl: &[u8]) -> Vec<([u8; 4], Vec<u8>)> {
        let mut rest = &jxl[12..];
        let mut out = Vec::new();
        while !rest.is_empty() {
            let size = u32::from_be_bytes(rest[..4].try_into().expect("size")) as usize;
            let kind: [u8; 4] = rest[4..8].try_into().expect("type");
            out.push((kind, rest[8..size].to_vec()));
            rest = &rest[size..];
        }
        out
    }

    fn boxed(kind: [u8; 4], payload: &[u8]) -> Vec<u8> {
        let size = u32::try_from(8 + payload.len()).expect("small box");
        let mut out = size.to_be_bytes().to_vec();
        out.extend_from_slice(&kind);
        out.extend_from_slice(payload);
        out
    }

    /// A container holding `boxes`, in order, after the signature box.
    fn container(boxes: &[([u8; 4], &[u8])]) -> Vec<u8> {
        let mut out = JXL_SIGNATURE_BOX.to_vec();
        for (kind, payload) in boxes {
            out.extend(boxed(*kind, payload));
        }
        out
    }

    fn reason(jxl: &[u8]) -> &'static str {
        read_jxl_envelope(jxl).expect_err("refused").reason
    }

    fn codestream() -> Vec<u8> {
        boxes(&story().2)
            .into_iter()
            .find(|(kind, _)| kind == b"jxlc")
            .expect("a jxlc box")
            .1
    }

    const FTYP: &[u8] = b"jxl \0\0\0\0jxl ";

    fn decode_oxide(jxl: &[u8], w: usize, h: usize) -> Vec<u8> {
        let image = jxl_oxide::JxlImage::builder()
            .read(jxl)
            .expect("jxl-oxide reads the card");
        let frame = image.render_frame(0).expect("jxl-oxide renders frame 0");
        let mut out = vec![0u8; w * h * 3];
        frame.stream().write_to_buffer::<u8>(&mut out);
        out
    }

    fn decode_jxl_rs(jxl: &[u8], w: usize, h: usize) -> Vec<u8> {
        use jxl::api::{Event, JxlDecoder, JxlDecoderOptions, JxlOutputBuffer, JxlPixelFormat};
        let mut dec = JxlDecoder::new(JxlDecoderOptions::default());
        let mut input: &[u8] = jxl;
        let mut out = vec![0u8; w * h * 3];
        let (mut closed, mut have_info) = (false, false);
        loop {
            // The output buffer may only be passed once the basic info has set the pixel format.
            let event = if have_info {
                let mut bufs = [JxlOutputBuffer::new(&mut out, h, w * 3)];
                dec.process(&mut input, Some(&mut bufs), None)
            } else {
                dec.process(&mut input, None, None)
            }
            .expect("jxl-rs decodes the card");
            match event {
                Event::BasicInfo => {
                    dec.set_pixel_format(JxlPixelFormat::rgb8(0))
                        .expect("rgb8 output");
                    have_info = true;
                }
                Event::Complete => break,
                // The whole file is in `input`: say so once, then a second request is a truncated file.
                Event::NeedMoreInput { .. } if !closed => {
                    dec.close_input();
                    closed = true;
                }
                Event::NeedMoreInput { .. } => panic!("jxl-rs wants more than the whole file"),
                _ => {}
            }
        }
        out
    }

    #[test]
    fn same_card_renders_the_same_jxl_bytes() {
        let (pixmap, envelope, jxl) = story();
        assert_eq!(&encode_jxl(pixmap, envelope).expect("encodes"), jxl);
    }

    #[test]
    fn the_jxl_is_a_container_with_the_envelope_before_the_codestream() {
        let (_, envelope, jxl) = story();
        assert!(jxl.starts_with(&JXL_SIGNATURE_BOX));
        let boxes = boxes(jxl);
        let kinds: Vec<[u8; 4]> = boxes.iter().map(|(kind, _)| *kind).collect();
        assert_eq!(kinds, [*b"ftyp", JXL_ENVELOPE_BOX, *b"jxlc"]);
        assert_eq!(boxes[0].1, FTYP);
        assert_eq!(boxes[1].1, envelope.as_bytes());
        assert!(boxes[2].1.starts_with(&CODESTREAM));
    }

    #[test]
    fn the_jxl_carries_the_envelope_once() {
        let (_, envelope, jxl) = story();
        assert_eq!(
            &read_jxl_envelope(jxl).expect("the envelope reads"),
            envelope
        );
    }

    #[test]
    fn the_jxl_holds_the_png_pixels() {
        let (pixmap, envelope, jxl) = story();
        let (w, h) = (pixmap.width() as usize, pixmap.height() as usize);
        assert_eq!((w, h), (1080, 1350));
        let rgb = opaque_rgb(pixmap);
        assert_eq!(rgb.len(), w * h * 3);

        let png = encode_png(pixmap, envelope).expect("the PNG encodes");
        let mut reader = png::Decoder::new(std::io::Cursor::new(png))
            .read_info()
            .expect("the PNG decodes");
        let mut png_pixels = vec![0; reader.output_buffer_size().expect("the size fits")];
        let info = reader
            .next_frame(&mut png_pixels)
            .expect("the frame decodes");
        png_pixels.truncate(info.buffer_size());
        assert!(rgb == png_pixels, "opaque_rgb is the PNG's pixels");

        assert!(
            decode_oxide(jxl, w, h) == rgb,
            "jxl-oxide decodes the PNG's pixels"
        );
        assert!(
            decode_jxl_rs(jxl, w, h) == rgb,
            "jxl-rs decodes the PNG's pixels"
        );
    }

    #[test]
    fn the_jxl_is_smaller_than_the_png() {
        let inputs = [
            ("story", story_input(FindingType::NPlusOneSql)),
            ("curve", curve_input()),
            ("collective", collective_input()),
        ];
        for (name, input) in inputs {
            let pixmap = pixmap_of(&input);
            let envelope = envelope(&input);
            let jxl = encode_jxl(&pixmap, &envelope).expect("encodes").len();
            let png = encode_png(&pixmap, &envelope).expect("encodes").len();
            eprintln!("{name}: jxl {jxl} B, png {png} B");
            assert!(jxl * 10 < png * 6, "{name}: {jxl} B against {png} B");
        }
    }

    #[test]
    fn a_jxl_without_its_box_has_no_card_payload() {
        let (_, envelope, _) = story();
        let codestream = codestream();
        let ftyp = (*b"ftyp", FTYP);
        let jxlc = (*b"jxlc", codestream.as_slice());
        let pscd = (JXL_ENVELOPE_BOX, envelope.as_bytes());

        assert_eq!(reason(&container(&[ftyp, jxlc])), "no_card_payload");
        assert_eq!(
            reason(&container(&[ftyp, pscd, pscd, jxlc])),
            "duplicate_card_payload"
        );
        assert_eq!(reason(&codestream), "no_card_payload");
        assert_eq!(
            &read_jxl_envelope(&container(&[ftyp, jxlc, pscd])).expect("reads"),
            envelope
        );

        let huge = vec![b'x'; MAX_ENVELOPE_BYTES + 1];
        let big = (JXL_ENVELOPE_BOX, huge.as_slice());
        assert_eq!(reason(&container(&[ftyp, big, jxlc])), "malformed_input");
        let invalid = (JXL_ENVELOPE_BOX, &[0xC3, 0x28][..]);
        assert_eq!(
            reason(&container(&[ftyp, invalid, jxlc])),
            "malformed_input"
        );

        assert_eq!(reason(b"GIF89a"), "malformed_input");
        assert_eq!(reason(&vec![0; MAX_CARD_JXL_BYTES + 1]), "file_too_large");
    }

    #[test]
    fn a_pscd_inside_another_box_is_not_seen() {
        let (_, envelope, _) = story();
        let codestream = codestream();
        let inner = boxed(JXL_ENVELOPE_BOX, envelope.as_bytes());

        let mut hidden = inner.clone();
        hidden.extend_from_slice(&codestream);
        let file = container(&[(*b"ftyp", FTYP), (*b"jxlc", &hidden)]);
        assert_eq!(reason(&file), "no_card_payload");

        let mut brob = JXL_ENVELOPE_BOX.to_vec();
        brob.extend_from_slice(&inner);
        let file = container(&[(*b"ftyp", FTYP), (*b"brob", &brob), (*b"jxlc", &codestream)]);
        assert_eq!(reason(&file), "no_card_payload");
    }

    #[test]
    fn a_zero_size_image_is_jxl_encode_failed() {
        for (rgb, width, height) in [(&[][..], 0, 0), (&[0; 3][..], 1, 0), (&[0; 5][..], 1, 1)] {
            let error = encode_codestream(rgb, width, height).expect_err("refused");
            assert_eq!(error.reason, "jxl_encode_failed");
            assert_eq!(error.result, CardResult::Unrenderable);
            assert_eq!(error.result.exit_code(), 5);
        }
    }

    #[test]
    fn box_sizes_are_bounds_checked() {
        let with = |tail: &[u8]| {
            let mut out = JXL_SIGNATURE_BOX.to_vec();
            out.extend_from_slice(tail);
            out
        };
        let cases: [&[u8]; 5] = [
            b"\xFF\xFF\xFF\xFFjxlc\0\0",
            b"\0\0\0\x04jxlc",
            b"\0\0\0\x01jxlc\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF",
            b"\0\0\0\x01jxlc\0\0\0\0\0\0\0\x08",
            b"\0\0\0\x14ft",
        ];
        for tail in cases {
            assert_eq!(reason(&with(tail)), "malformed_input", "{tail:?}");
        }

        let (_, envelope, _) = story();
        let mut file = container(&[(*b"ftyp", FTYP)]);
        file.extend_from_slice(&[0, 0, 0, 0]);
        file.extend_from_slice(&JXL_ENVELOPE_BOX);
        file.extend_from_slice(envelope.as_bytes());
        assert_eq!(&read_jxl_envelope(&file).expect("reads"), envelope);
    }
}
