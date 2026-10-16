//! Line breaking and glyph coverage over the bundled fonts. krilla shapes
//! and draws, skrifa measures: both read the same font files.

use skrifa::instance::{LocationRef, Size};
use skrifa::{FontRef, MetadataProvider};

pub(crate) const DM_SANS_REGULAR: &[u8] = include_bytes!("fonts/DMSans-Regular.ttf");
pub(crate) const DM_SANS_BOLD: &[u8] = include_bytes!("fonts/DMSans-Bold.ttf");
pub(crate) const JETBRAINS_MONO: &[u8] = include_bytes!("fonts/JetBrainsMono-Regular.ttf");

/// The three faces a page draws with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Face {
    Regular,
    Bold,
    Mono,
}

impl Face {
    pub(crate) fn bytes(self) -> &'static [u8] {
        match self {
            Self::Regular => DM_SANS_REGULAR,
            Self::Bold => DM_SANS_BOLD,
            Self::Mono => JETBRAINS_MONO,
        }
    }

    fn font(self) -> FontRef<'static> {
        FontRef::new(self.bytes()).expect("bundled font parses")
    }
}

/// Width of `text` in points, from the glyph advances without kerning.
pub(crate) fn text_width(face: Face, size: f32, text: &str) -> f32 {
    let font = face.font();
    let charmap = font.charmap();
    let metrics = font.glyph_metrics(Size::new(size), LocationRef::default());
    text.chars()
        .filter_map(|c| charmap.map(c))
        .filter_map(|glyph| metrics.advance_width(glyph))
        .sum()
}

/// Greedy line breaking on spaces. A word wider than the line is split
/// between characters.
pub(crate) fn wrap(text: &str, face: Face, size: f32, width: f32) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in text.split(' ').filter(|word| !word.is_empty()) {
        let candidate = if line.is_empty() {
            word.to_string()
        } else {
            format!("{line} {word}")
        };
        if text_width(face, size, &candidate) <= width {
            line = candidate;
            continue;
        }
        if !line.is_empty() {
            lines.push(std::mem::take(&mut line));
        }
        line = split_long_word(word, face, size, width, &mut lines);
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

/// Push the full-width pieces of `word` and return what is left of it.
fn split_long_word(
    word: &str,
    face: Face,
    size: f32,
    width: f32,
    lines: &mut Vec<String>,
) -> String {
    let mut piece = String::new();
    for c in word.chars() {
        piece.push(c);
        if piece.chars().count() > 1 && text_width(face, size, &piece) > width {
            piece.pop();
            lines.push(std::mem::take(&mut piece));
            piece.push(c);
        }
    }
    piece
}

/// `sha256:abcd…` as `sha256: abcd efgh …`, to read aloud and compare.
pub(crate) fn group4(hash: &str) -> String {
    let (prefix, hex) = hash.split_once(':').unwrap_or(("", hash));
    let groups = hex
        .as_bytes()
        .chunks(4)
        .map(|chunk| String::from_utf8_lossy(chunk).into_owned())
        .collect::<Vec<_>>()
        .join(" ");
    if prefix.is_empty() {
        groups
    } else {
        format!("{prefix}: {groups}")
    }
}

/// Characters some drawn text needs that its face cannot draw, sorted and
/// deduplicated. Whitespace is never drawn as a glyph.
pub(crate) fn missing_glyphs<'a>(texts: impl Iterator<Item = (Face, &'a str)>) -> Vec<char> {
    let mut missing: Vec<char> = texts
        .flat_map(|(face, text)| {
            let charmap = face.font().charmap();
            text.chars()
                .filter(move |c| !c.is_whitespace() && charmap.map(*c).is_none())
                .collect::<Vec<_>>()
        })
        .collect();
    missing.sort_unstable();
    missing.dedup();
    missing
}
