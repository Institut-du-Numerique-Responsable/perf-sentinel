//! The card as drawing operations: the single image and the four pages of
//! the carousel, laid out after the approved mock-up of 2026-10-30. Text
//! steps down in size when it does not fit and is never cut.

use fast_qr::{ECL, QRBuilder};

use super::copy::{
    RENDERED_LANGS, co2_parts, collective_captions, collective_lines, confirmation,
    curve_bar_labels, curve_comparison, curve_lines, equivalence_lines, family_label,
    format_permille, generated_line, header_period, org_line, scope_line, story_fact,
    story_sentence, why_it_matters, words,
};
use super::payload::{Angle, Figures, FullPayload, Lang};
use super::{CardError, CardResult};
use crate::report::periodic::pdf::layout::{Face, missing_glyphs, text_width, wrap};
use crate::report::periodic::pdf::pages::{
    ACCENT_BD, ACCENT_BG, BORDER, BORDER_2, BRAND, BRAND_TEXT, Op, Outline, Rgb, SURFACE, TEXT,
    TEXT_2,
};

pub(crate) const CARD_WIDTH: f32 = 1080.0;
pub(crate) const CARD_HEIGHT: f32 = 1350.0;
pub(crate) const QR_ECL: ECL = ECL::L;
pub(crate) const QR_MAX_VERSION: usize = 12;
/// Whole pixels per module on the image.
pub(crate) const QR_MODULE_PX: f32 = 4.0;
/// Whole pixels per module on the carousel's proof page.
pub(crate) const QR_PAGE_MODULE_PX: f32 = 8.0;
/// The deep green of DESIGN.md, for the hero figures.
pub(crate) const BRAND_DEEP: Rgb = (0x0b, 0x52, 0x3a);
pub(crate) const PROOF_BAND_PX: f32 = 312.0;

const WHITE: Rgb = (0xff, 0xff, 0xff);
const MARGIN: f32 = 80.0;
const RIGHT: f32 = CARD_WIDTH - MARGIN;
const CONTENT: f32 = RIGHT - MARGIN;
const HEADER_TOP: f32 = 64.0;
const HEADER_WIDTH: f32 = 600.0;
const LOGO_HEIGHT: f32 = 52.0;
/// Space between the header and the main block.
const MAIN_GAP: f32 = 28.0;
const IMAGE_MAIN_BOTTOM: f32 = CARD_HEIGHT - 336.0;
const CAROUSEL_MAIN_BOTTOM: f32 = CARD_HEIGHT - 170.0;
const FOOTER_BOTTOM: f32 = CARD_HEIGHT - 48.0;
const PROOF_PAGE_BOTTOM: f32 = CARD_HEIGHT - 56.0;
const QR_TILE_BOTTOM: f32 = 18.0;
/// Quiet zone around a QR code, in modules.
const QUIET_MODULES: f32 = 4.0;
/// A hero figure's line box, as a share of its size.
const HERO_LINE: f32 = 0.86;
/// Cap height of DM Sans, as a share of the size, to centre a line.
const CAP: f32 = 0.7;

const IMAGE_HEROES: &[f32] = &[170.0, 150.0, 130.0, 112.0, 96.0];
const IMAGE_SENTENCES: &[f32] = &[42.0, 38.0, 34.0];
const PAGE_HEROES: &[f32] = &[340.0, 300.0, 260.0, 220.0, 180.0, 150.0];
const PAGE_SENTENCES: &[f32] = &[56.0, 48.0, 42.0];
/// The curve's share, which can read "moins de 0,1 %": it steps down further.
const PAGE_CURVE_HEROES: &[f32] = &[300.0, 260.0, 220.0, 180.0, 150.0, 120.0, 96.0];

/// Build the QR code of the compact URL, at error correction L.
///
/// # Errors
///
/// `compact_too_large` when the URL needs more than version
/// [`QR_MAX_VERSION`].
pub(crate) fn compact_qr(url: &str) -> Result<Vec<Vec<bool>>, CardError> {
    let too_large =
        |detail: String| CardError::new(CardResult::Unrenderable, "compact_too_large", detail);
    let code = QRBuilder::new(url)
        .ecl(QR_ECL)
        .build()
        .map_err(|e| too_large(format!("{e:?}")))?;
    let max_side = 21 + 4 * (QR_MAX_VERSION - 1);
    if code.size > max_side {
        return Err(too_large(format!(
            "the compact URL needs {} modules a side, the cap is {max_side}",
            code.size
        )));
    }
    Ok((0..code.size)
        .map(|row| code[row].iter().map(|module| module.value()).collect())
        .collect())
}

/// `unsupported_language` unless the card can be drawn in `lang`.
///
/// # Errors
///
/// When `rendered` leaves `lang` out.
pub(crate) fn ensure_rendered(lang: Lang, rendered: &[Lang]) -> Result<(), CardError> {
    if rendered.contains(&lang) {
        Ok(())
    } else {
        Err(CardError::new(
            CardResult::Refused,
            "unsupported_language",
            format!("cards are not drawn in {lang:?}"),
        ))
    }
}

/// The single 1080 x 1350 image.
///
/// # Errors
///
/// `unsupported_language`, `compact_too_large` or `unsupported_characters`.
pub(crate) fn layout_image(payload: &FullPayload, compact_url: &str) -> Result<Vec<Op>, CardError> {
    let modules = check(payload, compact_url)?;
    let mut ops = page();
    let top = header(
        &mut ops,
        payload.free.team.as_deref(),
        &header_period(payload),
    );
    let room = (top + MAIN_GAP, IMAGE_MAIN_BOTTOM);
    let block = match payload.angle {
        Angle::Story => fit(room, IMAGE_HEROES, IMAGE_SENTENCES, |hero, sentence| {
            story_image(payload, hero, sentence)
        }),
        Angle::Curve => fit(room, IMAGE_HEROES, &[0.0], |hero, _| {
            curve_image(payload, hero)
        }),
        Angle::Collective => fit(room, IMAGE_HEROES, &[0.0], |hero, _| {
            collective_image(payload, hero)
        }),
    };
    place(&mut ops, block, room);
    proof_band(&mut ops, payload, &modules);
    check_glyphs(&ops)?;
    Ok(ops)
}

/// The four carousel pages: the hook, why it matters, the figures with the
/// equivalence, and the proof with the method.
///
/// # Errors
///
/// As [`layout_image`].
pub(crate) fn layout_carousel(
    payload: &FullPayload,
    compact_url: &str,
) -> Result<Vec<Vec<Op>>, CardError> {
    let modules = check(payload, compact_url)?;
    let pages = vec![
        footed_page(payload, |room| hook_page(payload, room)),
        footed_page(payload, |_| why_page(payload.lang)),
        footed_page(payload, |room| figures_page(payload, room)),
        proof_page(payload, &modules),
    ];
    for page in &pages {
        check_glyphs(page)?;
    }
    Ok(pages)
}

/// The checks both layouts run before drawing: the language, the free
/// texts the fonts must draw, and the QR code.
fn check(payload: &FullPayload, compact_url: &str) -> Result<Vec<Vec<bool>>, CardError> {
    ensure_rendered(payload.lang, RENDERED_LANGS)?;
    let free = &payload.free;
    for (field, text) in [
        ("free.title", free.title.as_deref()),
        ("free.team", free.team.as_deref()),
        ("free.label", free.label.as_deref()),
        ("org.name", Some(payload.org.name.as_str())),
    ] {
        let Some(text) = text else { continue };
        let missing = missing_glyphs([(Face::Regular, text), (Face::Bold, text)].into_iter());
        if !missing.is_empty() {
            let mut error = unsupported(&missing);
            error.field = Some(field);
            return Err(error);
        }
    }
    compact_qr(compact_url)
}

fn unsupported(missing: &[char]) -> CardError {
    CardError::new(
        CardResult::Unrenderable,
        "unsupported_characters",
        format!("the fonts cannot draw {missing:?}"),
    )
}

/// Every text of a page against its face, for the texts no field names.
fn check_glyphs(ops: &[Op]) -> Result<(), CardError> {
    let missing = missing_glyphs(ops.iter().filter_map(|op| match op {
        Op::Text { face, text, .. } => Some((*face, text.as_str())),
        _ => None,
    }));
    if missing.is_empty() {
        Ok(())
    } else {
        Err(unsupported(&missing))
    }
}

/// The first of `sizes` at which `text` wraps at `width` into at most
/// `max_lines` lines that each fit, else the smallest.
fn step_size(text: &str, face: Face, sizes: &[f32], width: f32, max_lines: usize) -> f32 {
    sizes
        .iter()
        .copied()
        .find(|&size| {
            let lines = wrap(text, face, size, width);
            lines.len() <= max_lines
                && lines
                    .iter()
                    .all(|line| text_width(face, size, line) <= width)
        })
        .or_else(|| sizes.last().copied())
        .unwrap_or(0.0)
}

/// The first block that fits `room`: the hero steps down first, then the
/// sentence.
fn fit(
    (top, bottom): (f32, f32),
    heroes: &[f32],
    sentences: &[f32],
    build: impl Fn(f32, f32) -> Block,
) -> Block {
    let smallest = heroes.last().copied().unwrap_or(0.0);
    let first_sentence = sentences.first().copied().unwrap_or(0.0);
    let candidates = heroes.iter().map(|&hero| (hero, first_sentence)).chain(
        sentences
            .iter()
            .skip(1)
            .map(|&sentence| (smallest, sentence)),
    );
    let mut block = Block::new(MARGIN, CONTENT);
    for (hero, sentence) in candidates {
        block = build(hero, sentence);
        if block.y <= bottom - top {
            break;
        }
    }
    block
}

// ------------------------------------------------------------ the block

/// Ops laid out downwards from y = 0, moved into place once their height
/// is known.
struct Block {
    ops: Vec<Op>,
    y: f32,
    x: f32,
    width: f32,
}

/// The baseline of a line of `size` centred in a line box of `line`.
fn baseline(top: f32, line: f32, size: f32) -> f32 {
    top + f32::midpoint(line, CAP * size)
}

fn text_op(x: f32, y: f32, face: Face, size: f32, color: Rgb, text: &str) -> Op {
    Op::Text {
        x,
        y,
        face,
        size,
        color,
        text: text.to_owned(),
    }
}

fn filled(x: f32, y: f32, w: f32, h: f32, radius: f32, color: Rgb) -> Op {
    Op::Box {
        x,
        y,
        w,
        h,
        radius,
        fill: Some(color),
        outline: None,
    }
}

/// One run of a paragraph: its face, colour and words.
type Run<'a> = (Face, Rgb, &'a str);

impl Block {
    fn new(x: f32, width: f32) -> Self {
        Self {
            ops: Vec::new(),
            y: 0.0,
            x,
            width,
        }
    }

    fn gap(&mut self, height: f32) {
        self.y += height;
    }

    fn para(&mut self, face: Face, size: f32, line: f32, color: Rgb, text: &str) {
        self.runs(&[(face, color, text)], size, line);
    }

    /// A paragraph of runs wrapped greedily on spaces; runs not separated
    /// by a space stay glued.
    fn runs(&mut self, runs: &[Run<'_>], size: f32, line: f32) {
        for row in wrap_runs(runs, size, self.width) {
            let y = baseline(self.y, line, size);
            for (x, face, color, text) in row {
                self.ops
                    .push(text_op(self.x + x, y, face, size, color, &text));
            }
            self.y += line;
        }
    }

    fn rule(&mut self, color: Rgb) {
        self.ops
            .push(filled(self.x, self.y, self.width, 2.0, 0.0, color));
        self.y += 2.0;
    }

    /// `inner`, laid out at `padding` inside a rounded box of `color`.
    fn panel(&mut self, color: Rgb, padding: (f32, f32, f32), fill_inner: impl FnOnce(&mut Self)) {
        let (top, side, bottom) = padding;
        let mut inner = Self::new(self.x + side, self.width - 2.0 * side);
        inner.y = top;
        fill_inner(&mut inner);
        let height = inner.y + bottom;
        self.ops
            .push(filled(self.x, self.y, self.width, height, 20.0, color));
        self.ops.extend(shifted(inner.ops, self.y));
        self.y += height;
    }
}

/// Lines of `(x, face, colour, text)` segments.
type Row = Vec<(f32, Face, Rgb, String)>;

/// A word: the styled pieces between two spaces.
type Word<'a> = Vec<Run<'a>>;

fn wrap_runs(runs: &[Run<'_>], size: f32, width: f32) -> Vec<Row> {
    place_words(&glue_words(runs), size, width)
}

/// Splits the runs on spaces; pieces not separated by a space, even across
/// two runs, stay in one word.
fn glue_words<'a>(runs: &[Run<'a>]) -> Vec<Word<'a>> {
    let mut words: Vec<Word<'a>> = Vec::new();
    let mut open = false;
    for &(face, color, text) in runs {
        for (i, part) in text.split(' ').enumerate() {
            if i > 0 {
                open = false;
            }
            if part.is_empty() {
                continue;
            }
            match words.last_mut() {
                Some(word) if open => word.push((face, color, part)),
                _ => words.push(vec![(face, color, part)]),
            }
            open = true;
        }
    }
    words
}

/// Fills rows greedily with `words`.
fn place_words(words: &[Word<'_>], size: f32, width: f32) -> Vec<Row> {
    let space = text_width(Face::Regular, size, " ");
    let mut rows: Vec<Row> = Vec::new();
    let mut row: Row = Vec::new();
    let mut x = 0.0;
    for word in words {
        let w: f32 = word
            .iter()
            .map(|&(face, _, text)| text_width(face, size, text))
            .sum();
        if !row.is_empty() && x + space + w > width {
            rows.push(std::mem::take(&mut row));
            x = 0.0;
        }
        for (i, &piece) in word.iter().enumerate() {
            let gap = (i == 0 && !row.is_empty()).then_some(space);
            push_piece(&mut row, x, gap, piece);
            x += gap.unwrap_or(0.0) + text_width(piece.0, size, piece.2);
        }
    }
    if !row.is_empty() {
        rows.push(row);
    }
    rows
}

/// Appends `piece` to the row, merged into the last segment when both share
/// their style; `space` is the gap before it, if any.
fn push_piece(row: &mut Row, x: f32, space: Option<f32>, (face, color, text): Run<'_>) {
    match row.last_mut() {
        Some((_, last_face, last_color, last)) if *last_face == face && *last_color == color => {
            if space.is_some() {
                last.push(' ');
            }
            last.push_str(text);
        }
        _ => row.push((x + space.unwrap_or(0.0), face, color, text.to_owned())),
    }
}

fn shifted(ops: Vec<Op>, dy: f32) -> impl Iterator<Item = Op> {
    ops.into_iter().map(move |mut op| {
        match &mut op {
            Op::Text { y, .. } | Op::Box { y, .. } | Op::Qr { y, .. } | Op::Logo { y, .. } => {
                *y += dy;
            }
            Op::Circle { cy, .. } => *cy += dy,
        }
        op
    })
}

/// `block` centred in `room`, or at its top when it does not fit.
fn place(ops: &mut Vec<Op>, block: Block, (top, bottom): (f32, f32)) {
    let dy = top + ((bottom - top - block.y) / 2.0).max(0.0);
    ops.extend(shifted(block.ops, dy));
}

// ------------------------------------------------------- shared pieces

fn page() -> Vec<Op> {
    vec![filled(0.0, 0.0, CARD_WIDTH, CARD_HEIGHT, 0.0, WHITE)]
}

/// The logo on the left, the team then the period on the right; returns
/// the header's bottom.
fn header(ops: &mut Vec<Op>, team: Option<&str>, period: &str) -> f32 {
    ops.push(Op::Logo {
        x: MARGIN,
        y: HEADER_TOP + 2.0,
        height: LOGO_HEIGHT,
    });
    let size = 28.0;
    let line = 38.0;
    let mut lines: Vec<(Face, Rgb, String)> = Vec::new();
    if let Some(team) = team {
        lines.extend(
            wrap(team, Face::Bold, size, HEADER_WIDTH)
                .into_iter()
                .map(|text| (Face::Bold, TEXT, text)),
        );
    }
    let (face, color) = if team.is_some() {
        (Face::Regular, TEXT_2)
    } else {
        (Face::Bold, TEXT)
    };
    lines.extend(
        wrap(period, face, size, HEADER_WIDTH)
            .into_iter()
            .map(|text| (face, color, text)),
    );
    let mut top = HEADER_TOP;
    for (face, color, text) in lines {
        let x = RIGHT - text_width(face, size, &text);
        ops.push(text_op(
            x,
            baseline(top, line, size),
            face,
            size,
            color,
            &text,
        ));
        top += line;
    }
    top.max(HEADER_TOP + 2.0 + LOGO_HEIGHT)
}

/// The figure and, for a duration or a large count, its unit word.
fn hero_parts(figure: &str) -> (&str, Option<&str>) {
    let Some(letter) = figure.find(char::is_alphabetic) else {
        return (figure, None);
    };
    match figure[..letter]
        .char_indices()
        .rev()
        .find(|(_, c)| matches!(c, ' ' | '\u{a0}'))
    {
        Some((at, c)) => (&figure[..at], Some(&figure[at + c.len_utf8()..])),
        None => (figure, None),
    }
}

/// The hero figure in deep green, its unit word smaller beside it.
fn hero(b: &mut Block, figure: &str, size: f32) {
    let (number, unit) = hero_parts(figure);
    let y = b.y + 0.78 * size;
    b.ops
        .push(text_op(b.x, y, Face::Bold, size, BRAND_DEEP, number));
    if let Some(unit) = unit {
        let x = b.x + text_width(Face::Bold, size, number) + 16.0 * size / 170.0;
        let unit_size = (size * 64.0 / 170.0).round();
        b.ops
            .push(text_op(x, y, Face::Bold, unit_size, BRAND_DEEP, unit));
    }
    b.y += HERO_LINE * size;
}

/// `wanted`, stepped down through `sizes` until the hero and its unit fit
/// the width.
fn fitted_hero(figure: &str, sizes: &[f32], wanted: f32) -> f32 {
    hero_sizes(figure, sizes)
        .first()
        .copied()
        .unwrap_or(wanted)
        .min(wanted)
}

/// The largest of `sizes` at which the hero and its unit fit the width.
fn hero_sizes<'a>(figure: &str, sizes: &'a [f32]) -> &'a [f32] {
    let (number, unit) = hero_parts(figure);
    let room = unit.map_or(CONTENT, |unit| {
        CONTENT - text_width(Face::Bold, 64.0, unit) - 16.0
    });
    let widest = step_size(number, Face::Bold, sizes, room, 1);
    let start = sizes.iter().position(|&s| s <= widest).unwrap_or(0);
    &sizes[start..]
}

/// Bold confirmation, then the dates in the secondary colour.
fn facts(b: &mut Block, payload: &FullPayload, size: f32, line: f32) {
    b.para(
        Face::Bold,
        size,
        line,
        TEXT,
        words(payload.lang).fix_confirmed,
    );
    b.para(Face::Regular, size, line, TEXT_2, &story_fact(payload));
}

/// The scope line under a rule, at 30 px.
fn scope(b: &mut Block, payload: &FullPayload) {
    b.gap(22.0);
    b.rule(BORDER);
    b.gap(16.0);
    b.para(Face::Regular, 30.0, 38.0, TEXT, &scope_line(payload));
}

/// Sizes of a CO2 block: text, its line, the equivalence and its line,
/// the source and its line.
struct Co2Sizes {
    text: f32,
    line: f32,
    count: f32,
    count_line: f32,
    source: f32,
    source_line: f32,
}

/// The CO2 line with its figure in bold, then the equivalence.
fn co2_block(b: &mut Block, payload: &FullPayload, s: &Co2Sizes) {
    let Some((lead, figure, rest)) = co2_parts(payload) else {
        return;
    };
    let lead = format!("{lead} ");
    let rest = format!(" {rest}");
    b.runs(
        &[
            (Face::Regular, TEXT, &lead),
            (Face::Bold, TEXT, &figure),
            (Face::Regular, TEXT, &rest),
        ],
        s.text,
        s.line,
    );
    equivalence(b, payload, s);
}

fn equivalence(b: &mut Block, payload: &FullPayload, s: &Co2Sizes) {
    if let Some((count, factor)) = equivalence_lines(payload) {
        b.gap(10.0);
        b.para(Face::Bold, s.count, s.count_line, TEXT, &count);
        b.para(Face::Regular, s.source, s.source_line, TEXT_2, &factor);
    }
}

const IMAGE_CO2: Co2Sizes = Co2Sizes {
    text: 26.0,
    line: 35.0,
    count: 28.0,
    count_line: 36.0,
    source: 22.0,
    source_line: 30.0,
};
const PAGE_CO2: Co2Sizes = Co2Sizes {
    text: 36.0,
    line: 49.0,
    count: 38.0,
    count_line: 50.0,
    source: 26.0,
    source_line: 35.0,
};

/// One measured bar and the alert threshold as an empty dashed outline,
/// never a measured duration.
fn slow_comparison(b: &mut Block, payload: &FullPayload, gap: f32, label: f32, bar: f32) {
    let Figures::Story(f) = &payload.figures else {
        return;
    };
    let (Some(cited), Some(threshold)) = (f.cited_ms, f.threshold_ms) else {
        return;
    };
    b.gap(gap);
    let w = words(payload.lang);
    let line = (label * 1.3).round();
    b.para(Face::Regular, label, line, TEXT_2, w.slowest_before);
    b.gap(8.0);
    b.ops.push(filled(b.x, b.y, b.width, bar, 8.0, TEXT_2));
    b.gap(bar + 20.0);
    b.para(Face::Regular, label, line, TEXT_2, w.alert_threshold);
    b.gap(8.0);
    #[allow(clippy::cast_precision_loss)]
    let share = threshold as f32 / cited.max(1) as f32;
    b.ops.push(Op::Box {
        x: b.x,
        y: b.y,
        w: (b.width * share).clamp(24.0, b.width),
        h: bar,
        radius: 8.0,
        fill: None,
        outline: Some(Outline {
            color: BRAND_TEXT,
            width: 3.0,
            dashed: true,
        }),
    });
    b.gap(bar);
}

/// Sizes of a bar chart: its height, value labels and period labels.
struct ChartSizes {
    height: f32,
    value: f32,
    label: f32,
}

/// Where one bar stands: its left edge, top, size and the shared base.
struct Bar {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    base: f32,
}

/// Left of a label `width` wide: centred on the bar, or on the outer edges
/// when only the ends are named (`Some(true)` for the last bar).
fn bar_anchor(bar: &Bar, width: f32, ends: Option<bool>) -> f32 {
    match ends {
        None => bar.x + (bar.w - width) / 2.0,
        Some(false) => bar.x,
        Some(true) => bar.x + bar.w - width,
    }
}

/// Rounded top, square base: a square box covers the bottom corners.
fn draw_bar(b: &mut Block, bar: &Bar, color: Rgb) {
    if bar.h <= 0.0 {
        return;
    }
    let radius = 8.0_f32.min(bar.h / 2.0);
    b.ops
        .push(filled(bar.x, bar.y, bar.w, bar.h, radius, color));
    b.ops
        .push(filled(bar.x, bar.base - radius, bar.w, radius, 0.0, color));
}

fn bar_value(b: &mut Block, bar: &Bar, text: &str, size: f32, ends: Option<bool>) {
    let width = text_width(Face::Bold, size, text);
    let x = bar_anchor(bar, width, ends);
    b.ops
        .push(text_op(x, bar.y - 0.4 * size, Face::Bold, size, TEXT, text));
}

fn bar_label(b: &mut Block, bar: &Bar, label: &str, size: f32, room: f32, ends: Option<bool>) {
    let steps = [size, size * 0.85, size * 0.7];
    let fitted = step_size(label, Face::Regular, &steps, room, 1);
    let x = bar_anchor(bar, text_width(Face::Regular, fitted, label), ends);
    let y = baseline(bar.base + 14.0, size * 1.3, size);
    b.ops
        .push(text_op(x, y, Face::Regular, fitted, TEXT_2, label));
}

/// One bar per period, the card's in deep green; values on the two
/// compared bars only.
fn chart(b: &mut Block, payload: &FullPayload, sizes: &ChartSizes) {
    let Figures::Curve(curve) = &payload.figures else {
        return;
    };
    let values = &curve.waste_permille;
    let labels = curve_bar_labels(payload);
    let n = values.len();
    let many = n > super::copy::NAMED_BARS;
    let gap = if many { 14.0 } else { 28.0 };
    #[allow(clippy::cast_precision_loss)]
    let count = n as f32;
    let bar_w = ((b.width - gap * (count - 1.0)) / count).min(220.0);
    let x0 = b.x + (b.width - (bar_w * count + gap * (count - 1.0))) / 2.0;
    let max = values.iter().copied().max().unwrap_or(0).max(1);
    let top = b.y + sizes.value * 1.6;
    let plot = sizes.height - sizes.value * 1.6 - sizes.label * 1.8;
    let base = top + plot;
    let label_room = if many { b.width / 2.0 } else { bar_w + gap };
    for (i, (&v, label)) in values.iter().zip(&labels).enumerate() {
        #[allow(clippy::cast_precision_loss)]
        let (x, h) = (x0 + i as f32 * (bar_w + gap), plot * v as f32 / max as f32);
        let bar = Bar {
            x,
            y: base - h,
            w: bar_w,
            h,
            base,
        };
        let color = if i + 1 == n { BRAND_DEEP } else { BRAND };
        draw_bar(b, &bar, color);
        let ends = many.then_some(i > 0);
        if i + 2 >= n {
            let text = format_permille(v, payload.lang);
            bar_value(b, &bar, &text, sizes.value, ends);
        }
        if !label.is_empty() {
            bar_label(b, &bar, label, sizes.label, label_room, ends);
        }
    }
    b.ops.push(filled(b.x, base, b.width, 2.0, 0.0, BORDER_2));
    b.y += sizes.height;
}

/// The families of a collective, each with one square per fix.
fn families(b: &mut Block, payload: &FullPayload, size: f32, unit: f32) {
    let Figures::Collective(collective) = &payload.figures else {
        return;
    };
    let lang = payload.lang;
    let line = (size * 1.36).round();
    let counts = &collective.fixes_by_family_count;
    let rows: Vec<(u64, &str)> = [
        (counts.extra_calls, "extra_calls"),
        (counts.slow_calls, "slow_calls"),
        (counts.call_architecture, "call_architecture"),
    ]
    .into_iter()
    .filter(|(n, _)| *n > 0)
    .map(|(n, family)| (n, family_label(family, lang)))
    .collect();
    b.para(
        Face::Regular,
        size,
        line,
        TEXT_2,
        words(lang).what_was_fixed,
    );
    let most = rows.iter().map(|(n, _)| *n).max().unwrap_or(1);
    // shortcut: beyond 12 fixes in a family the squares become one bar; a
    // per-fix mark stops being countable there anyway.
    let squares = most <= 12;
    let step = unit + 8.0;
    #[allow(clippy::cast_precision_loss)]
    let units_w = if squares {
        most as f32 * step - 8.0
    } else {
        12.0 * step - 8.0
    };
    let number_w = text_width(Face::Bold, size, &most.to_string()).max(34.0 * size / 28.0);
    for (n, label) in rows {
        b.gap(10.0);
        let y = baseline(b.y, line, size);
        let digits = n.to_string();
        let x = b.x + number_w - text_width(Face::Bold, size, &digits);
        b.ops.push(text_op(x, y, Face::Bold, size, TEXT, &digits));
        let units_x = b.x + number_w + 18.0;
        let unit_y = b.y + (line - unit) / 2.0;
        if squares {
            for k in 0..n {
                #[allow(clippy::cast_precision_loss)]
                let at = units_x + k as f32 * step;
                b.ops.push(filled(at, unit_y, unit, unit, 6.0, BRAND));
            }
        } else {
            #[allow(clippy::cast_precision_loss)]
            let w = units_w * n as f32 / most as f32;
            b.ops
                .push(filled(units_x, unit_y, w.max(unit), unit, 6.0, BRAND));
        }
        b.ops.push(text_op(
            units_x + units_w + 18.0,
            y,
            Face::Regular,
            size,
            TEXT,
            label,
        ));
        b.y += line;
    }
}

/// The two collective figures, each with its caption glued to it, on one
/// line or stacked.
fn collective_heroes(b: &mut Block, payload: &FullPayload, size: f32, caption: f32, stack: bool) {
    let Some(pairs) = collective_captions(payload) else {
        return;
    };
    let widths: Vec<f32> = pairs
        .iter()
        .map(|(n, c)| text_width(Face::Bold, size, n) + 18.0 + text_width(Face::Bold, caption, c))
        .collect();
    let stack = stack || widths.iter().sum::<f32>() + 72.0 > b.width;
    let mut x = b.x;
    for (i, (n, c)) in pairs.iter().enumerate() {
        if stack && i > 0 {
            b.y += HERO_LINE * size + 28.0;
            x = b.x;
        }
        let y = b.y + 0.78 * size;
        b.ops.push(text_op(x, y, Face::Bold, size, BRAND_DEEP, n));
        let cx = x + text_width(Face::Bold, size, n) + 18.0;
        b.ops.push(text_op(cx, y, Face::Bold, caption, TEXT, c));
        x += widths[i] + 72.0;
    }
    b.y += HERO_LINE * size;
}

fn quoted_title(payload: &FullPayload) -> Option<String> {
    payload.free.title.as_ref().map(|t| format!("\"{t}\""))
}

// ------------------------------------------------------- the image

fn story_image(payload: &FullPayload, hero_size: f32, sentence: f32) -> Block {
    let mut b = Block::new(MARGIN, CONTENT);
    if let Some(title) = quoted_title(payload) {
        b.para(Face::Regular, 34.0, 42.0, TEXT_2, &title);
        b.gap(10.0);
    }
    let figure = super::copy::cited_figure(payload);
    hero(
        &mut b,
        &figure,
        fitted_hero(&figure, IMAGE_HEROES, hero_size),
    );
    b.gap(18.0);
    b.para(
        Face::Regular,
        sentence,
        (sentence * 1.24).round(),
        TEXT,
        &story_sentence(payload),
    );
    b.gap(18.0);
    facts(&mut b, payload, 28.0, 38.0);
    scope(&mut b, payload);
    if let Some((lead, figure, rest)) = co2_parts(payload) {
        b.gap(24.0);
        b.panel(ACCENT_BG, (20.0, 32.0, 22.0), |p| {
            p.para(Face::Regular, 28.0, 36.0, TEXT_2, &lead);
            p.para(Face::Bold, 72.0, 78.0, TEXT, &figure);
            p.para(Face::Regular, 28.0, 36.0, TEXT, &rest);
            if let Some((count, factor)) = equivalence_lines(payload) {
                p.gap(12.0);
                p.rule(ACCENT_BD);
                p.gap(12.0);
                p.para(Face::Bold, 30.0, 38.0, TEXT, &count);
                p.para(Face::Regular, 22.0, 30.0, TEXT_2, &factor);
            }
        });
    } else {
        slow_comparison(&mut b, payload, 26.0, 28.0, 52.0);
    }
    b
}

fn curve_image(payload: &FullPayload, hero_size: f32) -> Block {
    let mut b = Block::new(MARGIN, CONTENT);
    let w = words(payload.lang);
    let (current, previous) = curve_comparison(payload);
    let share = curve_share(payload);
    b.para(Face::Bold, 36.0, 44.0, TEXT, w.curve_what);
    b.gap(8.0);
    hero(&mut b, &share, fitted_hero(&share, IMAGE_HEROES, hero_size));
    b.gap(10.0);
    b.runs(
        &[
            (Face::Bold, TEXT, &current),
            (Face::Regular, TEXT_2, &previous),
        ],
        34.0,
        44.0,
    );
    b.gap(6.0);
    b.para(Face::Bold, 30.0, 40.0, BRAND_TEXT, w.lower_is_better);
    b.gap(26.0);
    chart(
        &mut b,
        payload,
        &ChartSizes {
            height: 250.0,
            value: 30.0,
            label: 26.0,
        },
    );
    scope(&mut b, payload);
    b.gap(18.0);
    co2_block(&mut b, payload, &IMAGE_CO2);
    b
}

fn curve_share(payload: &FullPayload) -> String {
    match &payload.figures {
        Figures::Curve(f) => {
            format_permille(f.waste_permille.last().copied().unwrap_or(0), payload.lang)
        }
        _ => String::new(),
    }
}

fn collective_image(payload: &FullPayload, hero_size: f32) -> Block {
    let mut b = Block::new(MARGIN, CONTENT);
    collective_heroes(&mut b, payload, hero_size, 44.0, false);
    b.gap(22.0);
    collective_sentence(&mut b, payload, 38.0, 48.0, true);
    b.gap(28.0);
    families(&mut b, payload, 28.0, 30.0);
    scope(&mut b, payload);
    if co2_parts(payload).is_some() {
        b.gap(18.0);
        co2_block(&mut b, payload, &IMAGE_CO2);
    }
    b
}

/// The collective sentence, its comparison in the secondary colour on the
/// same paragraph or on its own line.
fn collective_sentence(b: &mut Block, payload: &FullPayload, size: f32, line: f32, inline: bool) {
    let lines = collective_lines(payload);
    let sentence = lines.first().map_or("", String::as_str);
    match lines.get(1) {
        Some(against) if inline => {
            let against = format!(" {against}");
            b.runs(
                &[
                    (Face::Regular, TEXT, sentence),
                    (Face::Regular, TEXT_2, &against),
                ],
                size,
                line,
            );
        }
        Some(against) => {
            b.para(Face::Regular, size, line, TEXT, sentence);
            b.para(Face::Regular, size, line, TEXT_2, against);
        }
        None => b.para(Face::Regular, size, line, TEXT, sentence),
    }
}

/// The band at the foot of the image: who declared what, when it was
/// generated, where to check it, and the QR code.
fn proof_band(ops: &mut Vec<Op>, payload: &FullPayload, modules: &[Vec<bool>]) {
    let band_top = CARD_HEIGHT - PROOF_BAND_PX;
    ops.push(filled(
        0.0,
        band_top,
        CARD_WIDTH,
        PROOF_BAND_PX,
        0.0,
        SURFACE,
    ));
    let tile = qr_tile(modules, QR_MODULE_PX);
    let tile_x = RIGHT - tile;
    let tile_y = CARD_HEIGHT - QR_TILE_BOTTOM - tile;
    push_qr(ops, modules, QR_MODULE_PX, tile_x, tile_y);
    let w = words(payload.lang);
    let mut b = Block::new(MARGIN, HEADER_WIDTH.min(tile_x - 24.0 - MARGIN));
    b.para(Face::Bold, 21.0, 29.0, TEXT, &org_line(payload));
    b.para(Face::Regular, 21.0, 29.0, TEXT_2, &generated_line(payload));
    b.para(Face::Regular, 21.0, 29.0, TEXT_2, w.measured_with);
    b.gap(18.0);
    b.para(Face::Regular, 24.0, 32.0, TEXT, w.verify);
    b.para(Face::Bold, 32.0, 40.0, BRAND_TEXT, w.verify_url);
    place(ops, b, (band_top, band_top + PROOF_BAND_PX));
}

/// Side of a QR tile: the code and its quiet zone.
#[allow(clippy::cast_precision_loss)]
fn qr_tile(modules: &[Vec<bool>], module_px: f32) -> f32 {
    (modules.len() as f32 + 2.0 * QUIET_MODULES) * module_px
}

/// A white tile at `(x, y)` and the code inside its quiet zone.
fn push_qr(ops: &mut Vec<Op>, modules: &[Vec<bool>], module_px: f32, x: f32, y: f32) {
    let tile = qr_tile(modules, module_px);
    let quiet = QUIET_MODULES * module_px;
    ops.push(filled(x, y, tile, tile, 0.0, WHITE));
    ops.push(Op::Qr {
        x: x + quiet,
        y: y + quiet,
        side: tile - 2.0 * quiet,
        modules: modules.to_vec(),
    });
}

// ---------------------------------------------------- the carousel

/// A page of the carousel with the header, the footer, and the block
/// `body` lays out in the room between them.
fn footed_page(payload: &FullPayload, body: impl FnOnce(f32) -> Block) -> Vec<Op> {
    let mut ops = page();
    let top = header(
        &mut ops,
        payload.free.team.as_deref(),
        &header_period(payload),
    );
    let footer_top = footer(&mut ops, payload);
    let room = (top + MAIN_GAP, CAROUSEL_MAIN_BOTTOM.min(footer_top - 16.0));
    let block = body(room.1 - room.0);
    place(&mut ops, block, room);
    ops
}

/// The scope on the left, the verify address on the right, under a rule;
/// returns the rule's top.
fn footer(ops: &mut Vec<Op>, payload: &FullPayload) -> f32 {
    let size = 30.0;
    let line = 38.0;
    let url = words(payload.lang).verify_url;
    let url_w = text_width(Face::Bold, size, url);
    let lines = wrap(
        &scope_line(payload),
        Face::Regular,
        size,
        CONTENT - url_w - 32.0,
    );
    #[allow(clippy::cast_precision_loss)]
    let top = FOOTER_BOTTOM - lines.len() as f32 * line;
    let rule = top - 22.0 - 2.0;
    ops.push(filled(MARGIN, rule, CONTENT, 2.0, 0.0, BORDER));
    let first = baseline(top, line, size);
    ops.push(text_op(
        RIGHT - url_w,
        first,
        Face::Bold,
        size,
        BRAND_TEXT,
        url,
    ));
    for (i, text) in lines.iter().enumerate() {
        #[allow(clippy::cast_precision_loss)]
        let y = first + i as f32 * line;
        ops.push(text_op(MARGIN, y, Face::Regular, size, TEXT, text));
    }
    rule
}

fn room_fit(
    room: f32,
    heroes: &[f32],
    sentences: &[f32],
    build: impl Fn(f32, f32) -> Block,
) -> Block {
    fit((0.0, room), heroes, sentences, build)
}

fn hook_page(payload: &FullPayload, room: f32) -> Block {
    match payload.angle {
        Angle::Story => room_fit(room, PAGE_HEROES, PAGE_SENTENCES, |hero_size, sentence| {
            let mut b = Block::new(MARGIN, CONTENT);
            if let Some(title) = quoted_title(payload) {
                b.para(Face::Regular, 44.0, 54.0, TEXT_2, &title);
                b.gap(24.0);
            }
            let figure = super::copy::cited_figure(payload);
            hero(
                &mut b,
                &figure,
                fitted_hero(&figure, PAGE_HEROES, hero_size),
            );
            b.gap(30.0);
            b.para(
                Face::Regular,
                sentence,
                (sentence * 1.21).round(),
                TEXT,
                &story_sentence(payload),
            );
            b.gap(34.0);
            facts(&mut b, payload, 28.0, 38.0);
            b
        }),
        Angle::Curve => room_fit(room, PAGE_HEROES, &[0.0], |hero_size, _| {
            let w = words(payload.lang);
            let (current, previous) = curve_comparison(payload);
            let mut b = Block::new(MARGIN, CONTENT);
            b.para(Face::Bold, 52.0, 62.0, TEXT, w.curve_what);
            b.gap(16.0);
            let share = curve_share(payload);
            let size = fitted_hero(&share, PAGE_CURVE_HEROES, hero_size.min(300.0));
            hero(&mut b, &share, size);
            b.gap(30.0);
            let previous = previous.trim_start_matches([',', ' ']);
            let current = if previous.is_empty() {
                current.clone()
            } else {
                format!("{current},")
            };
            b.para(Face::Bold, 52.0, 64.0, TEXT, &current);
            b.para(Face::Regular, 52.0, 64.0, TEXT_2, previous);
            b.gap(36.0);
            b.para(Face::Bold, 44.0, 56.0, BRAND_TEXT, w.lower_is_better);
            b
        }),
        Angle::Collective => room_fit(room, PAGE_HEROES, &[0.0], |hero_size, _| {
            let mut b = Block::new(MARGIN, CONTENT);
            let size = hero_size.min(260.0);
            collective_heroes(&mut b, payload, size, (size * 64.0 / 260.0).round(), true);
            b.gap(56.0);
            collective_sentence(&mut b, payload, 52.0, 64.0, false);
            b
        }),
    }
}

fn why_page(lang: Lang) -> Block {
    let mut b = Block::new(MARGIN, CONTENT);
    let lines = why_it_matters(lang);
    if let [first, second] = lines {
        b.para(Face::Bold, 64.0, 78.0, TEXT, first);
        b.gap(44.0);
        b.para(Face::Regular, 46.0, 60.0, TEXT_2, second);
    }
    b
}

fn figures_page(payload: &FullPayload, room: f32) -> Block {
    let mut b = Block::new(MARGIN, CONTENT);
    match payload.angle {
        Angle::Story => {
            if let Some((lead, figure, rest)) = co2_parts(payload) {
                b.para(Face::Regular, 44.0, 54.0, TEXT_2, &lead);
                b.para(Face::Bold, 150.0, 160.0, TEXT, &figure);
                b.para(Face::Regular, 44.0, 56.0, TEXT, &rest);
                if let Some((count, factor)) = equivalence_lines(payload) {
                    b.gap(56.0);
                    b.panel(ACCENT_BG, (28.0, 32.0, 30.0), |p| {
                        p.para(Face::Bold, 48.0, 58.0, TEXT, &count);
                        p.gap(6.0);
                        p.para(Face::Regular, 26.0, 35.0, TEXT_2, &factor);
                    });
                }
            } else {
                return room_fit(room, PAGE_HEROES, &[0.0], |hero_size, _| {
                    let mut b = Block::new(MARGIN, CONTENT);
                    let figure = super::copy::cited_figure(payload);
                    hero(&mut b, &figure, hero_size.min(260.0));
                    b.gap(34.0);
                    facts(&mut b, payload, 36.0, 48.0);
                    slow_comparison(&mut b, payload, 44.0, 36.0, 72.0);
                    b
                });
            }
        }
        Angle::Curve => {
            let legend = curve_lines(payload).pop().unwrap_or_default();
            b.para(Face::Regular, 40.0, 50.0, TEXT_2, &legend);
            b.gap(26.0);
            chart(
                &mut b,
                payload,
                &ChartSizes {
                    height: 520.0,
                    value: 48.0,
                    label: 36.0,
                },
            );
            b.gap(44.0);
            co2_block(&mut b, payload, &PAGE_CO2);
        }
        Angle::Collective => {
            families(&mut b, payload, 40.0, 46.0);
            if co2_parts(payload).is_some() {
                b.gap(72.0);
                co2_block(&mut b, payload, &PAGE_CO2);
            }
        }
    }
    b
}

/// The last page: the QR code at 8 px a module, the verify address, and
/// everything the proof band of the image says, with the method link.
fn proof_page(payload: &FullPayload, modules: &[Vec<bool>]) -> Vec<Op> {
    let roomy = proof_page_with(payload, modules, 1.0);
    let bottom = roomy
        .iter()
        .filter_map(|op| match op {
            Op::Text { y, size, .. } => Some(y + 0.25 * size),
            _ => None,
        })
        .fold(0.0, f32::max);
    if bottom <= PROOF_PAGE_BOTTOM {
        return roomy;
    }
    // Long texts take their room from the gaps, evenly, never from the QR.
    let spacing = 1.0 - (bottom - PROOF_PAGE_BOTTOM) / PROOF_PAGE_GAPS;
    proof_page_with(payload, modules, spacing.clamp(0.25, 1.0))
}

/// The gaps of the proof page at full spacing, which shrink together when
/// the texts run long.
const PROOF_PAGE_GAPS: f32 = 56.0 + 28.0 + 2.0 * 20.0 + 36.0 + 14.0 + 10.0 + 10.0 + 14.0;

fn proof_page_with(payload: &FullPayload, modules: &[Vec<bool>], spacing: f32) -> Vec<Op> {
    let w = words(payload.lang);
    let mut ops = page();
    let top = header(&mut ops, None, &header_period(payload));
    let mut b = Block::new(MARGIN, CONTENT);
    b.gap(56.0 * spacing);
    b.para(Face::Bold, 56.0, 64.0, TEXT, w.check_title);
    b.gap(28.0 * spacing);
    let tile = qr_tile(modules, QR_PAGE_MODULE_PX);
    let padding = (20.0 * spacing).round();
    let band_top = (top + b.y).round();
    let band_h = tile + 2.0 * padding;
    let mut qr_ops = vec![filled(0.0, band_top, CARD_WIDTH, band_h, 0.0, SURFACE)];
    push_qr(
        &mut qr_ops,
        modules,
        QR_PAGE_MODULE_PX,
        ((CARD_WIDTH - tile) / 2.0).round(),
        band_top + padding,
    );
    b.gap(band_h + 36.0 * spacing);
    verify_line(&mut b, payload);
    b.gap(14.0 * spacing);
    b.para(Face::Regular, 30.0, 38.0, TEXT, &scope_line(payload));
    if let Some(confirmed) = confirmation(payload) {
        b.para(Face::Regular, 28.0, 38.0, TEXT, confirmed);
    }
    b.gap(10.0 * spacing);
    b.para(Face::Regular, 24.0, 32.0, TEXT, &org_line(payload));
    b.gap(10.0 * spacing);
    b.para(Face::Regular, 24.0, 33.0, TEXT_2, &generated_line(payload));
    b.para(Face::Regular, 24.0, 33.0, TEXT_2, w.measured_with);
    b.gap(14.0 * spacing);
    method_link(&mut b, payload);
    ops.extend(shifted(b.ops, top));
    ops.extend(qr_ops);
    ops
}

/// "Check it and read the method:" then the address in bold, on one line
/// when it fits.
fn verify_line(b: &mut Block, payload: &FullPayload) {
    let w = words(payload.lang);
    let lead_w = text_width(Face::Regular, 28.0, w.verify);
    let space = text_width(Face::Regular, 28.0, " ");
    let url_w = text_width(Face::Bold, 32.0, w.verify_url);
    let y = baseline(b.y, 38.0, 28.0);
    b.ops
        .push(text_op(b.x, y, Face::Regular, 28.0, TEXT, w.verify));
    if lead_w + space + url_w <= b.width {
        b.ops.push(text_op(
            b.x + lead_w + space,
            y,
            Face::Bold,
            32.0,
            BRAND_TEXT,
            w.verify_url,
        ));
        b.y += 38.0;
    } else {
        b.y += 38.0;
        b.para(Face::Bold, 32.0, 40.0, BRAND_TEXT, w.verify_url);
    }
}

/// "Method:" then the link in mono, broken only after a `/`.
fn method_link(b: &mut Block, payload: &FullPayload) {
    let w = words(payload.lang);
    let (size, mono, line) = (24.0, 21.0, 33.0);
    let lead = text_width(Face::Regular, size, w.method) + text_width(Face::Regular, size, " ");
    let y = baseline(b.y, line, size);
    b.ops
        .push(text_op(b.x, y, Face::Regular, size, TEXT_2, w.method));
    let parts = slash_wrap(w.method_url, mono, b.width - lead, b.width);
    for (i, part) in parts.iter().enumerate() {
        let x = if i == 0 { b.x + lead } else { b.x };
        #[allow(clippy::cast_precision_loss)]
        let y = y + i as f32 * line;
        b.ops.push(text_op(x, y, Face::Mono, mono, TEXT, part));
    }
    #[allow(clippy::cast_precision_loss)]
    let rows = parts.len().max(1) as f32;
    b.y += line * rows;
}

/// `url` in lines of at most `first` then `rest` wide, broken only after
/// a `/`; a segment wider than a line stays whole.
fn slash_wrap(url: &str, size: f32, first: f32, rest: f32) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut line = String::new();
    for segment in url.split_inclusive('/') {
        let room = if lines.is_empty() { first } else { rest };
        let joined = format!("{line}{segment}");
        if !line.is_empty() && text_width(Face::Mono, size, &joined) > room {
            lines.push(std::mem::replace(&mut line, segment.to_owned()));
        } else {
            line = joined;
        }
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detect::FindingType;
    use crate::report::card::copy::{
        RENDERED_LANGS, co2_line, collective_lines, equivalence_lines, org_line, scope_line,
        story_fact, why_it_matters,
    };
    use crate::report::card::test_fixtures::{
        collective_input, compact_url_of, curve_input, full_payload, story_input,
        story_input_with_threshold,
    };
    use crate::report::periodic::pdf::layout::{Face, text_width};
    use crate::report::periodic::pdf::pages::{TEXT, TEXT_2};
    use serde_json::{Value, json};

    const LANGS: [Lang; 2] = [Lang::En, Lang::Fr];
    const BAND_TOP: f32 = CARD_HEIGHT - PROOF_BAND_PX;
    const METHOD_EN: &str =
        "github.com/Institut-du-Numerique-Responsable/perf-sentinel/blob/main/docs/SHARE-CARD.md";
    const METHOD_FR: &str = "github.com/Institut-du-Numerique-Responsable/perf-sentinel/blob/main/docs/FR/SHARE-CARD-FR.md";

    fn in_lang(mut input: Value, lang: Lang) -> Value {
        input["lang"] = serde_json::to_value(lang).expect("a language serializes");
        input
    }

    fn story() -> Value {
        story_input(FindingType::NPlusOneSql)
    }

    fn angles() -> [Value; 3] {
        [story(), curve_input(), collective_input()]
    }

    fn image(input: &Value) -> Vec<Op> {
        layout_image(&full_payload(input), &compact_url_of(input)).expect("the image lays out")
    }

    fn carousel(input: &Value) -> Vec<Vec<Op>> {
        layout_carousel(&full_payload(input), &compact_url_of(input))
            .expect("the carousel lays out")
    }

    fn image_error(input: &Value) -> CardError {
        layout_image(&full_payload(input), &compact_url_of(input)).expect_err("refused")
    }

    fn same(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-3
    }

    /// A drawn text, read back from its op.
    struct Drawn<'a> {
        y: f32,
        face: Face,
        size: f32,
        color: Rgb,
        content: &'a str,
    }

    fn texts(ops: &[Op]) -> Vec<Drawn<'_>> {
        ops.iter()
            .filter_map(|op| match op {
                Op::Text {
                    y,
                    face,
                    size,
                    color,
                    text,
                    ..
                } => Some(Drawn {
                    y: *y,
                    face: *face,
                    size: *size,
                    color: *color,
                    content: text,
                }),
                _ => None,
            })
            .collect()
    }

    /// Every drawn text joined with a space, runs of spaces collapsed:
    /// a line wrapped or split into runs reads back whole.
    fn joined(ops: &[Op]) -> String {
        texts(ops)
            .iter()
            .map(|t| t.content)
            .collect::<Vec<_>>()
            .join(" ")
            .split(' ')
            .filter(|word| !word.is_empty())
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// `(left, top, right, bottom)` of an op.
    fn bounds(op: &Op) -> (f32, f32, f32, f32) {
        match op {
            Op::Text {
                x,
                y,
                face,
                size,
                text,
                ..
            } => (
                *x,
                y - size,
                x + text_width(*face, *size, text),
                y + 0.25 * size,
            ),
            Op::Box { x, y, w, h, .. } => (*x, *y, x + w, y + h),
            Op::Circle { cx, cy, r, .. } => (cx - r, cy - r, cx + r, cy + r),
            Op::Qr { x, y, side, .. } => (*x, *y, x + side, y + side),
            Op::Logo { x, y, height } => (*x, *y, x + height * 296.0 / 88.0, y + height),
        }
    }

    fn assert_inside_page(ops: &[Op]) {
        for op in ops {
            let (left, top, right, bottom) = bounds(op);
            assert!(
                left >= 0.0 && top >= 0.0 && right <= CARD_WIDTH && bottom <= CARD_HEIGHT,
                "outside the page: {op:?}"
            );
        }
    }

    fn assert_opaque_page(ops: &[Op]) {
        match ops.first() {
            Some(Op::Box {
                x,
                y,
                w,
                h,
                fill: Some(_),
                ..
            }) => assert_eq!((*x, *y, *w, *h), (0.0, 0.0, CARD_WIDTH, CARD_HEIGHT)),
            other => panic!("the page starts with {other:?}"),
        }
    }

    /// The ops drawn between the opaque page and the proof band.
    fn above_band(ops: &[Op]) -> &[Op] {
        let band = ops
            .iter()
            .position(
                |op| matches!(op, Op::Box { y, h, .. } if *y == BAND_TOP && *h == PROOF_BAND_PX),
            )
            .expect("a proof band");
        &ops[1..band]
    }

    fn heroes(ops: &[Op]) -> Vec<Drawn<'_>> {
        texts(ops)
            .into_iter()
            .filter(|t| t.color == BRAND_DEEP)
            .collect()
    }

    fn qrs(ops: &[Op]) -> Vec<(f32, f32, f32, &Vec<Vec<bool>>)> {
        ops.iter()
            .filter_map(|op| match op {
                Op::Qr {
                    x,
                    y,
                    side,
                    modules,
                } => Some((*x, *y, *side, modules)),
                _ => None,
            })
            .collect()
    }

    fn header_period(lang: Lang, input: &Value) -> &'static str {
        match (lang, input["angle"].as_str()) {
            (Lang::En, Some("curve")) => "July to September 2026",
            (Lang::Fr, Some("curve")) => "juillet à septembre 2026",
            (Lang::En, _) => "September 2026",
            (Lang::Fr, _) => "septembre 2026",
        }
    }

    #[test]
    fn the_image_starts_with_an_opaque_page() {
        for input in angles() {
            let ops = image(&input);
            assert_opaque_page(&ops);
            assert_inside_page(&ops);
        }
    }

    #[test]
    fn the_image_prints_the_locked_lines() {
        for lang in LANGS {
            for input in angles() {
                let input = in_lang(input, lang);
                let payload = full_payload(&input);
                let text = joined(&image(&input));
                let (generated, verify) = match lang {
                    Lang::En => (
                        "Generated on 1 October 2026",
                        "Check it and read the method: perf-sentinel.dev/verifier",
                    ),
                    Lang::Fr => (
                        "Généré le 1er octobre 2026",
                        "Vérifiez la carte et lisez la méthode\u{a0}: perf-sentinel.dev/verifier",
                    ),
                };
                let (count, factor) = equivalence_lines(&payload).expect("an equivalence");
                for line in [
                    header_period(lang, &input).to_owned(),
                    scope_line(&payload),
                    co2_line(&payload).expect("a CO2 line"),
                    count,
                    factor,
                    generated.to_owned(),
                    verify.to_owned(),
                    org_line(&payload),
                ] {
                    assert!(text.contains(&line), "{line:?} missing from {text}");
                }
                assert!(!text.contains('₂'), "{text}");
                match input["angle"].as_str() {
                    Some("story") => {
                        let (fix, seen) = match lang {
                            Lang::En => ("Fix confirmed by measurement", "Seen on 20 August 2026"),
                            Lang::Fr => (
                                "Correction confirmée par la mesure",
                                "Observé le 20 août 2026",
                            ),
                        };
                        assert!(text.contains(fix), "{text}");
                        assert!(text.contains(seen), "{text}");
                        assert!(text.contains(&story_fact(&payload)), "{text}");
                    }
                    Some("collective") => {
                        let (fixes, against) = match lang {
                            Lang::En => (
                                "fixes confirmed by measurement",
                                "against 5 the month before",
                            ),
                            Lang::Fr => (
                                "corrections confirmées par la mesure",
                                "contre 5 le mois précédent",
                            ),
                        };
                        assert!(text.to_lowercase().contains(fixes), "{text}");
                        assert!(text.contains(against), "{text}");
                        for line in collective_lines(&payload) {
                            assert!(text.contains(&line), "{line:?} missing from {text}");
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    #[test]
    fn the_slow_story_draws_the_alert_threshold_as_an_empty_outline() {
        let ops = image(&story_input_with_threshold(FindingType::SlowSql));
        let dashed: Vec<&Op> = ops
            .iter()
            .filter(|op| matches!(op, Op::Box { outline: Some(o), .. } if o.dashed))
            .collect();
        assert_eq!(dashed.len(), 1);
        assert!(matches!(dashed[0], Op::Box { fill: None, .. }));
        let text = joined(&ops);
        assert!(text.contains("Slowest runs, before the fix"), "{text}");
        assert!(text.contains("Alert threshold"), "{text}");
        assert!(co2_line(&full_payload(&story_input(FindingType::SlowSql))).is_none());
        let fallback = image(&story_input(FindingType::SlowSql));
        assert!(
            !fallback
                .iter()
                .any(|op| matches!(op, Op::Box { outline: Some(o), .. } if o.dashed))
        );
    }

    #[test]
    fn the_scope_sits_under_the_figures_at_30_px() {
        for lang in LANGS {
            for input in angles() {
                let input = in_lang(input, lang);
                let payload = full_payload(&input);
                let scope = scope_line(&payload);
                let ops = image(&input);
                let first_word = scope.split(' ').next().expect("a word");
                let line = texts(&ops)
                    .into_iter()
                    .find(|t| t.content.starts_with(first_word) && same(t.size, 30.0))
                    .expect("the scope line");
                assert_eq!(line.color, TEXT);
                assert!(line.y < BAND_TOP);
                for hero in heroes(&ops) {
                    assert!(line.y > hero.y, "{} under the scope", hero.content);
                }
                for page in carousel(&input) {
                    assert!(joined(&page).contains(&scope), "{scope:?} missing");
                }
            }
        }
    }

    #[test]
    fn the_organisation_is_in_the_footer_and_the_title_is_quoted() {
        let mut input = story();
        input["free"]["title"] = json!("Lighter cart pages");
        input["free"]["team"] = json!("Shop team");
        let ops = image(&input);
        let org = texts(&ops)
            .into_iter()
            .find(|t| t.content.starts_with("Organisation"))
            .expect("the organisation line");
        assert!(org.y > BAND_TOP, "the organisation sits at {}", org.y);
        assert_eq!(
            texts(&ops)
                .iter()
                .filter(|t| t.content.contains("Acme"))
                .count(),
            1
        );
        let title = texts(&ops)
            .into_iter()
            .find(|t| t.content == "\"Lighter cart pages\"")
            .expect("the quoted title");
        assert_eq!(
            (title.size, title.color, title.face),
            (34.0, TEXT_2, Face::Regular)
        );
        let pages = carousel(&input);
        for (index, page) in pages.iter().enumerate() {
            let has_title = texts(page)
                .iter()
                .any(|t| t.content == "\"Lighter cart pages\"");
            assert_eq!(has_title, index == 0, "page {}", index + 1);
        }
    }

    fn longest(lang: Lang) -> Value {
        let mut input = in_lang(story(), lang);
        input["org"]["name"] =
            json!("Regional Transport Cooperative of the Greater Western Region");
        input["free"]["team"] = json!("Payments and International Billing platform team");
        input["free"]["title"] = json!("Order history pages, so much lighter than before");
        input["free"]["label"] = json!(
            "Looking through the complete history of all their orders, with every filter used"
        );
        input["figures"]["cited_ops"] = json!(1284);
        assert_eq!(
            input["org"]["name"].as_str().map(|s| s.chars().count()),
            Some(60)
        );
        assert_eq!(
            input["free"]["team"].as_str().map(|s| s.chars().count()),
            Some(48)
        );
        assert_eq!(
            input["free"]["title"].as_str().map(|s| s.chars().count()),
            Some(48)
        );
        assert_eq!(
            input["free"]["label"].as_str().map(|s| s.chars().count()),
            Some(80)
        );
        input
    }

    #[test]
    fn the_longest_strings_fit() {
        for lang in LANGS {
            let input = longest(lang);
            let ops = image(&input);
            assert_inside_page(&ops);
            for op in above_band(&ops) {
                let (_, _, _, bottom) = bounds(op);
                assert!(bottom <= BAND_TOP, "over the proof band: {op:?}");
            }
            let hero = heroes(&ops).first().map(|t| t.size).expect("a hero");
            let sentence = texts(&ops)
                .into_iter()
                .find(|t| t.content.starts_with("Looking"))
                .map(|t| t.size)
                .expect("the sentence");
            assert!(hero < 170.0, "the hero did not step down");
            assert!(
                same(sentence, 42.0) || same(hero, 96.0),
                "the sentence stepped first"
            );
            for page in carousel(&input) {
                assert_inside_page(&page);
                assert!(texts(&page).iter().all(|t| !t.content.ends_with('…')));
            }
            assert!(texts(&ops).iter().all(|t| !t.content.ends_with('…')));
            let text = joined(&ops);
            for word in ["Regional", "Payments", "history", "lighter"] {
                assert!(text.contains(word), "{word} cut from {text}");
            }
        }
    }

    #[test]
    fn the_qr_encodes_the_compact_url_on_whole_pixels() {
        let input = story();
        let url = compact_url_of(&input);
        let ops = image(&input);
        let found = qrs(&ops);
        assert_eq!(found.len(), 1);
        let (x, y, side, modules) = found[0];
        assert_eq!(*modules, compact_qr(&url).expect("fits"));
        #[allow(clippy::cast_precision_loss)]
        let expected = modules.len() as f32 * QR_MODULE_PX;
        assert!((side - expected).abs() < f32::EPSILON);
        assert!(QR_MODULE_PX >= 4.0 && QR_MODULE_PX.fract() == 0.0);
        assert!(side >= 155.0, "{side}");
        assert!(x.fract() == 0.0 && y.fract() == 0.0, "({x}, {y})");
        let quiet = 4.0 * QR_MODULE_PX;
        assert!(ops.iter().any(|op| matches!(op,
            Op::Box { x: bx, y: by, w, h, fill: Some((255, 255, 255)), .. }
                if same(*bx, x - quiet) && same(*by, y - quiet) && same(*w, side + 2.0 * quiet) && same(*h, side + 2.0 * quiet))));
        let pages = carousel(&input);
        let found = qrs(&pages[3]);
        assert_eq!(found.len(), 1);
        let (x, y, side, modules) = found[0];
        #[allow(clippy::cast_precision_loss)]
        let expected = modules.len() as f32 * QR_PAGE_MODULE_PX;
        assert!((side - expected).abs() < f32::EPSILON);
        assert!(x.fract() == 0.0 && y.fract() == 0.0, "({x}, {y})");
    }

    #[test]
    fn a_domain_too_long_for_the_qr_is_refused() {
        let mut input = story();
        input["org"]["domain"] =
            json!("cooperative-des-transports-regionaux-du-grand-ouest.acme.com");
        assert_eq!(input["org"]["domain"].as_str().map(str::len), Some(60));
        let error = image_error(&input);
        assert_eq!(error.reason, "compact_too_large");
        assert_eq!(error.result, crate::report::card::CardResult::Unrenderable);
        image(&story());
    }

    #[test]
    fn a_title_the_fonts_cannot_draw_is_unrenderable() {
        let mut input = story();
        input["free"]["title"] = json!("Fixed 🚀");
        let error = image_error(&input);
        assert_eq!(
            (error.reason, error.field),
            ("unsupported_characters", Some("free.title"))
        );
        let mut input = story();
        input["free"]["team"] = json!("Shop 🚀");
        assert_eq!(image_error(&input).field, Some("free.team"));
        let mut input = story();
        input["org"]["name"] = json!("Acme 🚀");
        assert_eq!(image_error(&input).field, Some("org.name"));
        let mut input = story();
        input["free"]["label"] = json!("Cart 🚀");
        assert_eq!(image_error(&input).field, Some("free.label"));
        let mut input = story();
        input["scope"]["sources"] = json!(["prod 🚀"]);
        let error = image_error(&input);
        assert_eq!(
            (error.reason, error.field),
            ("unsupported_characters", None)
        );
    }

    #[test]
    fn an_unrendered_language_is_refused() {
        let error = ensure_rendered(Lang::En, &[]).expect_err("refused");
        assert_eq!(error.reason, "unsupported_language");
        assert_eq!(error.result, crate::report::card::CardResult::Refused);
        for lang in LANGS {
            // A new language fails to compile here until it is handled.
            match lang {
                Lang::En | Lang::Fr => assert!(RENDERED_LANGS.contains(&lang)),
            }
            assert!(ensure_rendered(lang, RENDERED_LANGS).is_ok());
        }
    }

    #[test]
    fn the_carousel_has_four_pages_and_ends_with_the_proof() {
        for lang in LANGS {
            for input in angles() {
                let input = in_lang(input, lang);
                let payload = full_payload(&input);
                let pages = carousel(&input);
                assert_eq!(pages.len(), 4);
                let method = match lang {
                    Lang::En => METHOD_EN,
                    Lang::Fr => METHOD_FR,
                };
                for (index, page) in pages.iter().enumerate() {
                    assert_opaque_page(page);
                    assert_inside_page(page);
                    assert!(page.iter().any(|op| matches!(op, Op::Logo { .. })));
                    assert!(joined(page).contains(&scope_line(&payload)));
                    assert_eq!(
                        qrs(page).len(),
                        usize::from(index == 3),
                        "page {}",
                        index + 1
                    );
                    let link: String = texts(page)
                        .iter()
                        .filter(|t| t.face == Face::Mono)
                        .map(|t| t.content)
                        .collect();
                    if index == 3 {
                        assert_eq!(link, method);
                        for line in texts(page).iter().filter(|t| t.face == Face::Mono) {
                            assert!(
                                line.content == method.rsplit('/').next().unwrap_or_default()
                                    || line.content.ends_with('/')
                                    || method.ends_with(line.content),
                                "the link breaks inside a word: {}",
                                line.content
                            );
                        }
                    } else {
                        assert!(link.is_empty(), "page {}", index + 1);
                    }
                }
                let why = joined(&pages[1]);
                for line in why_it_matters(lang) {
                    assert!(why.contains(line), "{line:?} missing");
                }
                let figures = joined(&pages[2]);
                assert!(figures.contains(&co2_line(&payload).expect("a CO2 line")));
                let (count, factor) = equivalence_lines(&payload).expect("an equivalence");
                assert!(figures.contains(&count) && figures.contains(&factor));
            }
        }
    }

    #[test]
    fn a_curve_ending_under_one_permille_keeps_its_hero_on_the_page() {
        for lang in LANGS {
            let mut input = in_lang(curve_input(), lang);
            input["figures"]["waste_permille"] = json!([62, 48, 0]);
            let share = match lang {
                Lang::En => "under 0.1%",
                Lang::Fr => "moins de 0,1\u{a0}%",
            };
            let mut pages = carousel(&input);
            pages.push(image(&input));
            for page in &pages {
                assert_inside_page(page);
            }
            // The hook page and the image both print the share as their hero.
            for page in [&pages[0], &pages[4]] {
                assert!(
                    heroes(page).iter().any(|t| t.content == share),
                    "{lang:?} lacks {share:?}"
                );
            }
        }
    }

    #[test]
    fn layout_is_deterministic() {
        for input in angles() {
            assert_eq!(
                format!("{:?}", image(&input)),
                format!("{:?}", image(&input))
            );
        }
    }
}
