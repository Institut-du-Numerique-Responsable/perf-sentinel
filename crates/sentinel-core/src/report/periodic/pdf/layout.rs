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

/// A line never holds more characters than this, so glyphs without an
/// advance cannot grow one without bound.
const MAX_LINE_CHARS: usize = 200;

/// One face at one size, parsed once for a whole measurement.
struct Measure {
    charmap: skrifa::charmap::Charmap<'static>,
    metrics: skrifa::metrics::GlyphMetrics<'static>,
}

impl Measure {
    fn new(face: Face, size: f32) -> Self {
        let font = face.font();
        Self {
            charmap: font.charmap(),
            metrics: font.glyph_metrics(Size::new(size), LocationRef::default()),
        }
    }

    fn advance(&self, c: char) -> f32 {
        self.charmap
            .map(c)
            .and_then(|glyph| self.metrics.advance_width(glyph))
            .unwrap_or(0.0)
    }

    fn width(&self, text: &str) -> f32 {
        text.chars().map(|c| self.advance(c)).sum()
    }
}

/// Width of `text` in points, from the glyph advances without kerning.
#[cfg(test)]
pub(crate) fn text_width(face: Face, size: f32, text: &str) -> f32 {
    Measure::new(face, size).width(text)
}

/// The line being filled, with its width kept as it grows.
#[derive(Default)]
struct Line {
    text: String,
    width: f32,
    chars: usize,
}

impl Line {
    fn take(&mut self) -> String {
        self.width = 0.0;
        self.chars = 0;
        std::mem::take(&mut self.text)
    }
}

/// Greedy line breaking on spaces, in one pass. A word wider than the line
/// is split between characters.
pub(crate) fn wrap(text: &str, face: Face, size: f32, width: f32) -> Vec<String> {
    let measure = Measure::new(face, size);
    let space = measure.advance(' ');
    let mut lines = Vec::new();
    let mut line = Line::default();
    for word in text.split(' ').filter(|word| !word.is_empty()) {
        let word_width = measure.width(word);
        let word_chars = word.chars().count();
        let joined = line.width + space + word_width;
        if !line.text.is_empty() && joined <= width && line.chars + 1 + word_chars <= MAX_LINE_CHARS
        {
            line.text.push(' ');
            line.text.push_str(word);
            line.width = joined;
            line.chars += 1 + word_chars;
            continue;
        }
        if !line.text.is_empty() {
            lines.push(line.take());
        }
        for c in word.chars() {
            let advance = measure.advance(c);
            if !line.text.is_empty()
                && (line.width + advance > width || line.chars >= MAX_LINE_CHARS)
            {
                lines.push(line.take());
            }
            line.text.push(c);
            line.width += advance;
            line.chars += 1;
        }
    }
    if !line.text.is_empty() {
        lines.push(line.text);
    }
    lines
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
    let charmaps =
        [Face::Regular, Face::Bold, Face::Mono].map(|face| (face, face.font().charmap()));
    let mut missing: Vec<char> = Vec::new();
    for (face, text) in texts {
        let (_, charmap) = charmaps
            .iter()
            .find(|(candidate, _)| *candidate == face)
            .expect("every face has a charmap");
        missing.extend(
            text.chars()
                .filter(|c| !c.is_whitespace() && charmap.map(*c).is_none()),
        );
    }
    missing.sort_unstable();
    missing.dedup();
    missing
}
