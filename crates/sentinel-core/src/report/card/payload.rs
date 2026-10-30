//! The card payloads (contract C3): the `card.json` input the Hub writes,
//! the full payload signed into the PNG or PDF, and the compact payload
//! the QR code carries. Every rule here also runs on a signed payload,
//! since anyone can make a key.

use super::content::CONTENT_SIG_MARKER;
use super::keys::b64url_decode;
use super::{CardError, CardResult};
use crate::detect::FindingType;
use crate::report::periodic::hasher::canonicalize;
use crate::score::equivalence::{
    EQUIVALENCE_SOURCE, Equivalence, MAX_CHOSEN_EQUIVALENCE_COUNT, MIN_EQUIVALENCE_COUNT,
    embedded_equivalence_vintage, equivalence_count, find_equivalence, pick_equivalence,
    printed_co2_ug,
};
use crate::text_safety::is_bidi_or_invisible;
use crate::time::{epoch_day_to_iso_date, parse_iso_date_to_epoch_day};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

pub const CARD_FORMAT_VERSION: u32 = 1;
/// The largest integer a browser reads back exactly (2^53 - 1).
pub const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Angle {
    Story,
    Curve,
    Collective,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Lang {
    En,
    Fr,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PeriodKind {
    Week,
    Month,
    Quarter,
    Semester,
    Year,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Period {
    pub kind: PeriodKind,
    pub from: String,
    pub to: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scope {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment: Option<String>,
    pub sources: Vec<String>,
    pub covered_days: u32,
    pub restart_days: u32,
    pub sampled_permille: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoryFigures {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cited_ops: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cited_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before_fix_ops: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub threshold_ops: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub threshold_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub co2_low_ug: Option<u64>,
}

/// `waste_permille` runs oldest first, the card's period last;
/// `avoidable_ops` and `co2_low_ug` cover the card's period.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CurveFigures {
    pub waste_permille: Vec<u32>,
    pub avoidable_ops: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub co2_low_ug: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FamilyCounts {
    pub extra_calls: u64,
    pub slow_calls: u64,
    pub call_architecture: u64,
}

/// `previous_fixes_count` is absent when the Hub cannot count the previous
/// period honestly; `before_fix_ops` sums the volume before the N+1 and
/// redundant fixes only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CollectiveFigures {
    pub confirmed_fixes_count: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_fixes_count: Option<u64>,
    pub services_count: u64,
    pub fixes_by_family_count: FamilyCounts,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before_fix_ops: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub co2_low_ug: Option<u64>,
}

/// Untagged for serialization only: parsing goes through the angle, since
/// `{}` would match `StoryFigures` whatever the angle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum Figures {
    Story(StoryFigures),
    Curve(CurveFigures),
    Collective(CollectiveFigures),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Story {
    pub finding_type: FindingType,
    pub service: String,
    pub endpoint: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_at: Option<String>,
    pub done_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Free {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub team: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Org {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub domain: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EquivalenceRef {
    pub id: String,
    pub per_unit_ug: u64,
    pub unit: String,
    pub source: String,
    pub vintage: String,
}

/// The before-fix period; its kind is always `period.kind`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DayRange {
    pub from: String,
    pub to: String,
}

/// The C2 `card.json` the Hub writes. `free` is required, `{}` when empty.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CardInput {
    pub angle: Angle,
    pub lang: Lang,
    pub period: Period,
    pub scope: Scope,
    pub figures: serde_json::Value,
    #[serde(default)]
    pub story: Option<Story>,
    #[serde(default)]
    pub before_fix_period: Option<DayRange>,
    #[serde(default)]
    pub equivalence: Option<String>,
    pub free: Free,
    pub org: Org,
    pub generated_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FullPayload {
    pub v: u32,
    pub angle: Angle,
    pub lang: Lang,
    pub period: Period,
    pub scope: Scope,
    pub figures: Figures,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub story: Option<Story>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub before_fix_period: Option<DayRange>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub equivalence: Option<EquivalenceRef>,
    pub free: Free,
    pub org: Org,
    pub key: String,
    pub public_key: String,
    pub generated_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompactPayload {
    pub v: u32,
    pub a: String,
    pub p: (String, u32, u32),
    pub f: BTreeMap<String, u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub r: Option<(u32, u32)>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub d: Option<String>,
    pub k: String,
    pub g: u32,
}

pub(crate) const MS_PER_DAY: u64 = 86_400_000;
const MAX_TITLE_CHARS: usize = 48;
const MAX_TEAM_CHARS: usize = 48;
/// The Hub stores endpoint labels up to 80 (`LabelWrite.MaxLabelChars`).
const MAX_LABEL_CHARS: usize = 80;
const MAX_ORG_NAME_CHARS: usize = 60;
const MAX_ENDPOINT_CHARS: usize = 200;
/// Bounds the envelope under the readers' 64 KiB cap (`raster::MAX_ENVELOPE_BYTES`).
const MAX_SERVICE_CHARS: usize = 200;
/// The scope line prints at a fixed size: environment and sources, joined
/// with ", ", fit in 120 characters.
const MAX_SCOPE_CHARS: usize = 120;
const NO_CAP: usize = usize::MAX;

pub(crate) const ANGLE_LETTERS: [(Angle, &str); 3] = [
    (Angle::Story, "s"),
    (Angle::Curve, "c"),
    (Angle::Collective, "k"),
];
pub(crate) const KIND_LETTERS: [(PeriodKind, &str); 5] = [
    (PeriodKind::Week, "w"),
    (PeriodKind::Month, "m"),
    (PeriodKind::Quarter, "q"),
    (PeriodKind::Semester, "h"),
    (PeriodKind::Year, "y"),
];
/// The compact `f` key sets of a story, sorted: the cited figure, plus the
/// volume (avoidable types) or the threshold it beats.
const STORY_COMPACT_KEYS: [&[&str]; 5] = [&["o"], &["b", "o"], &["o", "t"], &["m"], &["m", "u"]];

impl Figures {
    fn co2_low_ug(&self) -> Option<u64> {
        match self {
            Self::Story(f) => f.co2_low_ug,
            Self::Curve(f) => f.co2_low_ug,
            Self::Collective(f) => f.co2_low_ug,
        }
    }
}

/// The fields every rule reads, shared by the input and the signed payload.
struct Card<'a> {
    angle: Angle,
    period: &'a Period,
    scope: &'a Scope,
    figures: &'a Figures,
    story: Option<&'a Story>,
    before_fix_period: Option<&'a DayRange>,
    free: &'a Free,
    org: &'a Org,
    generated_at_ms: u64,
}

impl FullPayload {
    fn card(&self) -> Card<'_> {
        Card {
            angle: self.angle,
            period: &self.period,
            scope: &self.scope,
            figures: &self.figures,
            story: self.story.as_ref(),
            before_fix_period: self.before_fix_period.as_ref(),
            free: &self.free,
            org: &self.org,
            generated_at_ms: self.generated_at_ms,
        }
    }
}

impl CardInput {
    fn card<'a>(&'a self, figures: &'a Figures) -> Card<'a> {
        Card {
            angle: self.angle,
            period: &self.period,
            scope: &self.scope,
            figures,
            story: self.story.as_ref(),
            before_fix_period: self.before_fix_period.as_ref(),
            free: &self.free,
            org: &self.org,
            generated_at_ms: self.generated_at_ms,
        }
    }
}

/// The full payload as signed, before its figures are read by angle.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SignedFull {
    v: u32,
    angle: Angle,
    lang: Lang,
    period: Period,
    scope: Scope,
    figures: Value,
    story: Option<Story>,
    before_fix_period: Option<DayRange>,
    equivalence: Option<EquivalenceRef>,
    free: Free,
    org: Org,
    key: String,
    public_key: String,
    generated_at_ms: u64,
}

/// Parse a `card.json` and apply every C3 rule.
///
/// # Errors
///
/// `malformed_input` or one of the refusals of C2.
pub fn parse_card_input(bytes: &[u8]) -> Result<CardInput, CardError> {
    let input: CardInput = deserialize(json_value(bytes)?)?;
    let figures = parse_figures(input.angle, &input.figures)?;
    check_card(&input.card(&figures))?;
    resolve_equivalence(input.equivalence.as_deref(), figures.co2_low_ug())?;
    Ok(input)
}

/// Parse the canonical bytes of a signed full payload, with the same rules.
/// The equivalence is checked on its own terms, never against the embedded
/// table, so a card stays verifiable after a Base Carbone refresh.
///
/// # Errors
///
/// `unsupported_version` for `v != 1`, else as [`parse_card_input`].
pub fn parse_full_payload(canonical: &[u8]) -> Result<FullPayload, CardError> {
    let value = json_value(canonical)?;
    check_version(&value)?;
    let signed: SignedFull = deserialize(value)?;
    let full = FullPayload {
        v: signed.v,
        angle: signed.angle,
        lang: signed.lang,
        figures: parse_figures(signed.angle, &signed.figures)?,
        period: signed.period,
        scope: signed.scope,
        story: signed.story,
        before_fix_period: signed.before_fix_period,
        equivalence: signed.equivalence,
        free: signed.free,
        org: signed.org,
        key: signed.key,
        public_key: signed.public_key,
        generated_at_ms: signed.generated_at_ms,
    };
    check_card(&full.card())?;
    check_signed_equivalence(full.equivalence.as_ref(), full.figures.co2_low_ug())?;
    check_public_key(&full.public_key)?;
    Ok(full)
}

/// Parse the canonical bytes of a signed compact payload.
///
/// # Errors
///
/// `unsupported_version` for `v != 1`, `malformed_input` on any shape C3
/// does not allow.
pub fn parse_compact_payload(canonical: &[u8]) -> Result<CompactPayload, CardError> {
    let value = json_value(canonical)?;
    check_version(&value)?;
    let compact: CompactPayload = deserialize(value)?;
    check_compact(&compact)?;
    Ok(compact)
}

/// The full and compact payloads of an input. The rules run again, so an
/// input built by hand cannot skip them.
///
/// # Errors
///
/// As [`parse_card_input`].
pub fn build_payloads(
    input: &CardInput,
    public_key_b64: &str,
    fingerprint: &str,
) -> Result<(FullPayload, CompactPayload), CardError> {
    let figures = parse_figures(input.angle, &input.figures)?;
    let equivalence = resolve_equivalence(input.equivalence.as_deref(), figures.co2_low_ug())?;
    let full = FullPayload {
        v: CARD_FORMAT_VERSION,
        angle: input.angle,
        lang: input.lang,
        period: input.period.clone(),
        scope: input.scope.clone(),
        figures,
        story: input.story.clone(),
        before_fix_period: input.before_fix_period.clone(),
        equivalence: equivalence.map(equivalence_ref),
        free: input.free.clone(),
        org: input.org.clone(),
        key: fingerprint.to_owned(),
        public_key: public_key_b64.to_owned(),
        generated_at_ms: input.generated_at_ms,
    };
    check_card(&full.card())?;
    check_public_key(&full.public_key)?;
    let compact = compact_of(&full)?;
    Ok((full, compact))
}

/// Sorted keys, no whitespace: the bytes a signature covers.
///
/// # Panics
///
/// Never for the card payloads: their map keys are strings.
#[must_use]
pub fn canonical_bytes<T: Serialize>(value: &T) -> Vec<u8> {
    let value = serde_json::to_value(value).expect("card payloads serialize to JSON");
    serde_json::to_vec(&canonicalize(&value)).expect("a JSON value serializes")
}

fn malformed(detail: impl Into<String>) -> CardError {
    CardError::new(CardResult::InputError, "malformed_input", detail)
}

fn refused(reason: &'static str, detail: impl Into<String>) -> CardError {
    CardError::new(CardResult::Refused, reason, detail)
}

/// JSON whose every number is an integer a browser reads back exactly.
fn json_value(bytes: &[u8]) -> Result<Value, CardError> {
    let value: Value = serde_json::from_slice(bytes).map_err(|e| malformed(e.to_string()))?;
    if safe_integers(&value) {
        Ok(value)
    } else {
        Err(malformed(format!(
            "every number must be an integer in 0..={MAX_SAFE_INTEGER}"
        )))
    }
}

fn safe_integers(value: &Value) -> bool {
    match value {
        Value::Number(n) => n.as_u64().is_some_and(|n| n <= MAX_SAFE_INTEGER),
        Value::Array(items) => items.iter().all(safe_integers),
        Value::Object(map) => map.values().all(safe_integers),
        _ => true,
    }
}

fn deserialize<T: DeserializeOwned>(value: Value) -> Result<T, CardError> {
    serde_json::from_value(value).map_err(|e| malformed(e.to_string()))
}

fn check_version(value: &Value) -> Result<(), CardError> {
    match value.get("v").and_then(Value::as_u64) {
        Some(v) if v != u64::from(CARD_FORMAT_VERSION) => Err(CardError::new(
            CardResult::Unrenderable,
            "unsupported_version",
            format!("card format {v}, this binary reads {CARD_FORMAT_VERSION}"),
        )),
        _ => Ok(()),
    }
}

fn parse_figures(angle: Angle, figures: &Value) -> Result<Figures, CardError> {
    let parsed = match angle {
        Angle::Story => StoryFigures::deserialize(figures).map(Figures::Story),
        Angle::Curve => CurveFigures::deserialize(figures).map(Figures::Curve),
        Angle::Collective => CollectiveFigures::deserialize(figures).map(Figures::Collective),
    };
    parsed.map_err(|e| malformed(format!("figures: {e}")))
}

/// Every rule but the equivalence, in the order C3 lists them.
fn check_card(card: &Card<'_>) -> Result<(), CardError> {
    let period_days = check_period(card.period, card.generated_at_ms)?;
    check_scope(card.scope, period_days)?;
    check_figures(card)?;
    check_story(card)?;
    check_before_fix_period(card)?;
    check_texts(card)?;
    card.org.domain.as_deref().map_or(Ok(()), check_domain)
}

fn day(date: &str) -> Result<u32, CardError> {
    parse_iso_date_to_epoch_day(date)
        .ok_or_else(|| malformed(format!("{date:?} is not a YYYY-MM-DD date")))
}

fn months_in(kind: PeriodKind) -> Option<u32> {
    match kind {
        PeriodKind::Week => None,
        PeriodKind::Month => Some(1),
        PeriodKind::Quarter => Some(3),
        PeriodKind::Semester => Some(6),
        PeriodKind::Year => Some(12),
    }
}

fn month_start(year: u32, month: u32) -> Option<u32> {
    parse_iso_date_to_epoch_day(&format!("{year:04}-{month:02}-01"))
}

/// The first and last epoch days of the `kind` period holding `day`.
fn period_holding(kind: PeriodKind, day: u32) -> Option<(u32, u32)> {
    let Some(months) = months_in(kind) else {
        // 1970-01-01 was a Thursday, so a Monday-based weekday is (day + 3) % 7.
        let monday = day.checked_sub((day + 3) % 7)?;
        return Some((monday, monday + 6));
    };
    let iso = epoch_day_to_iso_date(day);
    let year: u32 = iso.get(..4)?.parse().ok()?;
    let month: u32 = iso.get(5..7)?.parse().ok()?;
    let first = (month - 1) / months * months + 1;
    let (next_year, next) = if first + months > 12 {
        (year + 1, first + months - 12)
    } else {
        (year, first + months)
    };
    Some((month_start(year, first)?, month_start(next_year, next)? - 1))
}

fn holding(kind: PeriodKind, day: u32) -> Result<(u32, u32), CardError> {
    period_holding(kind, day).ok_or_else(|| malformed("no calendar period holds this date"))
}

fn previous(kind: PeriodKind, day: u32) -> Result<(u32, u32), CardError> {
    let (start, _) = holding(kind, day)?;
    holding(kind, start.saturating_sub(1))
}

/// The epoch days of `from`..`to`, refused unless they bound one `kind`
/// period of the calendar.
fn calendar_days(kind: PeriodKind, from: &str, to: &str) -> Result<(u32, u32), CardError> {
    let days = (day(from)?, day(to)?);
    if period_holding(kind, days.0) == Some(days) {
        Ok(days)
    } else {
        Err(refused(
            "period_not_calendar",
            format!("{from}..{to} is not one calendar {kind:?}"),
        ))
    }
}

/// The period's day count.
fn check_period(period: &Period, generated_at_ms: u64) -> Result<u32, CardError> {
    let (from, to) = calendar_days(period.kind, &period.from, &period.to)?;
    if u64::from(to) >= generated_at_ms / MS_PER_DAY {
        return Err(refused(
            "period_not_closed",
            format!("the period ends {} and is not over yet", period.to),
        ));
    }
    Ok(to - from + 1)
}

fn check_scope(scope: &Scope, period_days: u32) -> Result<(), CardError> {
    if scope.covered_days != period_days {
        return Err(refused(
            "incomplete_period",
            format!(
                "{} of the period's {period_days} days are covered",
                scope.covered_days
            ),
        ));
    }
    if scope.restart_days > scope.covered_days {
        return Err(malformed("restart_days exceeds covered_days"));
    }
    if scope.sampled_permille > 1000 {
        return Err(malformed("sampled_permille exceeds 1000"));
    }
    if scope.sources.is_empty() {
        return Err(malformed("scope.sources is empty"));
    }
    // Each name is non-empty (check_texts), so this also caps each at 120.
    let names: Vec<&str> = scope
        .environment
        .iter()
        .chain(&scope.sources)
        .map(String::as_str)
        .collect();
    if names.join(", ").chars().count() > MAX_SCOPE_CHARS {
        return Err(malformed(format!(
            "environment and sources take more than {MAX_SCOPE_CHARS} characters"
        )));
    }
    Ok(())
}

fn check_figures(card: &Card<'_>) -> Result<(), CardError> {
    match (card.figures, card.story) {
        (Figures::Story(f), Some(story)) => check_story_figures(f, &story.finding_type),
        (Figures::Curve(f), None) => check_curve(f),
        (Figures::Collective(f), None) => check_collective(f),
        _ => Err(malformed("story is present exactly on a story card")),
    }
}

#[derive(PartialEq)]
enum StoryKind {
    /// N+1 and redundant: the volume before the fix was avoidable.
    Avoidable,
    Slow,
    /// Fanout, chatty, pool saturation, serialized calls.
    Shape,
}

fn story_kind(finding_type: &FindingType) -> StoryKind {
    match finding_type {
        FindingType::NPlusOneSql
        | FindingType::NPlusOneHttp
        | FindingType::NPlusOneMessaging
        | FindingType::RedundantSql
        | FindingType::RedundantHttp => StoryKind::Avoidable,
        FindingType::SlowSql | FindingType::SlowHttp | FindingType::SlowMessaging => {
            StoryKind::Slow
        }
        FindingType::ExcessiveFanout
        | FindingType::ChattyService
        | FindingType::PoolSaturation
        | FindingType::SerializedCalls => StoryKind::Shape,
    }
}

fn check_story_figures(f: &StoryFigures, finding_type: &FindingType) -> Result<(), CardError> {
    let kind = story_kind(finding_type);
    if f.before_fix_ops.is_some() && kind != StoryKind::Avoidable {
        return Err(refused(
            "volume_not_avoidable",
            "only N+1 and redundant findings have an avoidable volume",
        ));
    }
    if f.co2_low_ug.is_some() && f.before_fix_ops.is_none() {
        return Err(refused(
            "volume_not_avoidable",
            "a story's CO2 prices only the volume before the fix",
        ));
    }
    let threshold_not_applicable = match kind {
        StoryKind::Avoidable => f.threshold_ops.is_some() || f.threshold_ms.is_some(),
        StoryKind::Slow => false,
        StoryKind::Shape => f.threshold_ms.is_some(),
    };
    if threshold_not_applicable {
        return Err(refused(
            "threshold_not_applicable",
            "this finding type has no such threshold",
        ));
    }
    let cites_its_figure = if kind == StoryKind::Slow {
        f.cited_ms.is_some() && f.cited_ops.is_none() && f.threshold_ops.is_none()
    } else {
        f.cited_ops.is_some() && f.cited_ms.is_none()
    };
    if cites_its_figure {
        Ok(())
    } else {
        Err(malformed(
            "a slow story cites cited_ms, any other cites cited_ops",
        ))
    }
}

fn check_curve(f: &CurveFigures) -> Result<(), CardError> {
    if (2..=12).contains(&f.waste_permille.len()) && f.waste_permille.iter().all(|&w| w <= 1000) {
        Ok(())
    } else {
        Err(malformed(
            "waste_permille holds 2 to 12 values of at most 1000",
        ))
    }
}

fn check_collective(f: &CollectiveFigures) -> Result<(), CardError> {
    let families = &f.fixes_by_family_count;
    let confirmed = f.confirmed_fixes_count;
    if confirmed == 0 || !(1..=confirmed).contains(&f.services_count) {
        return Err(malformed(
            "a collective counts at least one fix, on 1 to that many services",
        ));
    }
    if families.extra_calls + families.slow_calls + families.call_architecture != confirmed {
        return Err(malformed(
            "the families do not add up to confirmed_fixes_count",
        ));
    }
    if f.co2_low_ug.is_some() && f.before_fix_ops.is_none() {
        return Err(malformed("co2_low_ug comes only with before_fix_ops"));
    }
    if f.before_fix_ops.is_some() && families.extra_calls == 0 {
        return Err(malformed("before_fix_ops needs an extra_calls fix"));
    }
    Ok(())
}

fn check_story(card: &Card<'_>) -> Result<(), CardError> {
    if card.free.label.is_some() && card.angle != Angle::Story {
        return Err(malformed("free.label is only for a story"));
    }
    let Some(story) = card.story else {
        return Ok(());
    };
    let done = day(&story.done_at)?;
    if done > day(&card.period.to)? {
        return Err(malformed("done_at falls after the period"));
    }
    match &story.observed_at {
        Some(observed) if day(observed)? > done => {
            Err(malformed("observed_at falls after done_at"))
        }
        _ => Ok(()),
    }
}

/// The period the "before the fix" line cites, derived from the dates.
fn expected_before_fix_period(card: &Card<'_>) -> Result<Option<(u32, u32)>, CardError> {
    let kind = card.period.kind;
    match (card.figures, card.story) {
        (Figures::Story(f), Some(story)) if f.before_fix_ops.is_some() => {
            previous(kind, day(&story.done_at)?).map(Some)
        }
        (Figures::Story(f), Some(story)) if f.threshold_ops.or(f.threshold_ms).is_some() => {
            let observed = story
                .observed_at
                .as_deref()
                .ok_or_else(|| malformed("a threshold needs observed_at"))?;
            holding(kind, day(observed)?).map(Some)
        }
        (Figures::Collective(f), None) if f.before_fix_ops.is_some() => {
            previous(kind, day(&card.period.from)?).map(Some)
        }
        _ => Ok(None),
    }
}

fn check_before_fix_period(card: &Card<'_>) -> Result<(), CardError> {
    match (expected_before_fix_period(card)?, card.before_fix_period) {
        (None, None) => Ok(()),
        (Some(expected), Some(given)) => {
            if calendar_days(card.period.kind, &given.from, &given.to)? == expected {
                Ok(())
            } else {
                Err(malformed(
                    "before_fix_period is not the period the figures cover",
                ))
            }
        }
        _ => Err(malformed(
            "before_fix_period is present exactly when the card prints one",
        )),
    }
}

/// Trimmed, non-empty, drawable as plain text, and within `max_chars`.
fn check_text(name: &str, text: &str, max_chars: usize) -> Result<(), CardError> {
    if text.is_empty() || text.trim() != text {
        return Err(malformed(format!("{name} is empty or not trimmed")));
    }
    if text
        .chars()
        .any(|c| c.is_control() || is_bidi_or_invisible(c))
    {
        return Err(malformed(format!(
            "{name} holds a control or invisible character"
        )));
    }
    if text.chars().count() > max_chars {
        return Err(malformed(format!(
            "{name} is longer than {max_chars} characters"
        )));
    }
    Ok(())
}

fn check_texts(card: &Card<'_>) -> Result<(), CardError> {
    let free = card.free;
    // The PDF writes the title raw into its metadata: a second marker there
    // would leave the signature slot unfindable.
    if free
        .title
        .as_deref()
        .is_some_and(|title| title.contains(CONTENT_SIG_MARKER))
    {
        let mut error = malformed(format!(
            "free.title holds {CONTENT_SIG_MARKER}, the PDF signature slot marker"
        ));
        error.field = Some("free.title");
        return Err(error);
    }
    let optional = [
        ("free.title", free.title.as_deref(), MAX_TITLE_CHARS),
        ("free.team", free.team.as_deref(), MAX_TEAM_CHARS),
        ("free.label", free.label.as_deref(), MAX_LABEL_CHARS),
        (
            "scope.environment",
            card.scope.environment.as_deref(),
            NO_CAP,
        ),
        (
            "story.service",
            card.story.map(|s| s.service.as_str()),
            MAX_SERVICE_CHARS,
        ),
        (
            "story.endpoint",
            card.story.map(|s| s.endpoint.as_str()),
            MAX_ENDPOINT_CHARS,
        ),
    ];
    let present = optional
        .into_iter()
        .filter_map(|(name, text, max)| Some((name, text?, max)));
    let sources = card
        .scope
        .sources
        .iter()
        .map(|s| ("scope.sources", s.as_str(), NO_CAP));
    std::iter::once(("org.name", card.org.name.as_str(), MAX_ORG_NAME_CHARS))
        .chain(present)
        .chain(sources)
        .try_for_each(|(name, text, max)| check_text(name, text, max))
}

/// A public DNS name: lowercase ASCII, at most 253, at least two labels of 1
/// to 63 `[a-z0-9-]` with no edge hyphen, the last one not all digits (no IP
/// literal, no `localhost`, no user, path, query or port).
#[must_use]
pub fn is_public_host_name(domain: &str) -> bool {
    let labels: Vec<&str> = domain.split('.').collect();
    let label_ok = |label: &&str| {
        (1..=63).contains(&label.len())
            && !label.starts_with('-')
            && !label.ends_with('-')
            && label
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    };
    let numeric_tld = labels
        .last()
        .is_some_and(|tld| tld.bytes().all(|b| b.is_ascii_digit()));
    domain.len() <= 253 && labels.len() >= 2 && labels.iter().all(label_ok) && !numeric_tld
}

fn check_domain(domain: &str) -> Result<(), CardError> {
    if is_public_host_name(domain) {
        Ok(())
    } else {
        Err(malformed(format!("{domain:?} is not a public host name")))
    }
}

/// The equivalence of `card.json`: the named one when it gives a count in
/// range on the printed CO2, else the picker's on that figure.
fn resolve_equivalence(
    id: Option<&str>,
    co2_low_ug: Option<u64>,
) -> Result<Option<&'static Equivalence>, CardError> {
    let printed = co2_low_ug.map(printed_co2_ug);
    let Some(id) = id else {
        return Ok(printed.and_then(pick_equivalence));
    };
    let equivalence = find_equivalence(id)
        .ok_or_else(|| refused("unknown_equivalence", format!("no equivalence {id:?}")))?;
    let count = printed.map_or(0, |co2| equivalence_count(equivalence, co2));
    if (MIN_EQUIVALENCE_COUNT..=MAX_CHOSEN_EQUIVALENCE_COUNT).contains(&count) {
        Ok(Some(equivalence))
    } else {
        Err(refused(
            "equivalence_out_of_scale",
            format!("{count} {id} on the printed CO2"),
        ))
    }
}

fn equivalence_ref(equivalence: &Equivalence) -> EquivalenceRef {
    EquivalenceRef {
        id: equivalence.id.to_owned(),
        per_unit_ug: equivalence.per_unit_ug,
        unit: equivalence.unit.to_owned(),
        source: EQUIVALENCE_SOURCE.to_owned(),
        vintage: embedded_equivalence_vintage().to_owned(),
    }
}

/// A signed equivalence on its own terms: the 1000 cap applies only to an
/// explicit choice in `card.json`, and the factor may be one a later table
/// no longer carries.
fn check_signed_equivalence(
    equivalence: Option<&EquivalenceRef>,
    co2_low_ug: Option<u64>,
) -> Result<(), CardError> {
    let Some(e) = equivalence else {
        return Ok(());
    };
    for (name, text) in [
        ("equivalence.id", &e.id),
        ("equivalence.unit", &e.unit),
        ("equivalence.source", &e.source),
        ("equivalence.vintage", &e.vintage),
    ] {
        check_text(name, text, NO_CAP)?;
    }
    if e.per_unit_ug == 0 {
        return Err(malformed("equivalence.per_unit_ug is 0"));
    }
    // Floor, as `equivalence_count`.
    let count = co2_low_ug.map_or(0, |co2| printed_co2_ug(co2) / e.per_unit_ug);
    if count < MIN_EQUIVALENCE_COUNT {
        return Err(malformed(format!(
            "the equivalence counts {count}, under {MIN_EQUIVALENCE_COUNT}"
        )));
    }
    Ok(())
}

fn check_public_key(key: &str) -> Result<(), CardError> {
    if b64url_decode(key).is_some_and(|bytes| bytes.len() == 32) {
        Ok(())
    } else {
        Err(malformed(
            "the public key is not 32 bytes of canonical base64url",
        ))
    }
}

fn compact_figures(figures: &Figures) -> BTreeMap<String, u64> {
    let pairs = match figures {
        Figures::Story(f) => vec![
            ("o", f.cited_ops),
            ("m", f.cited_ms),
            ("b", f.before_fix_ops),
            ("t", f.threshold_ops),
            ("u", f.threshold_ms),
        ],
        Figures::Curve(f) => {
            // The card's period, then the one it compares with.
            let mut latest = f.waste_permille.iter().rev().map(|&w| u64::from(w));
            vec![("w", latest.next()), ("x", latest.next())]
        }
        Figures::Collective(f) => vec![
            ("n", Some(f.confirmed_fixes_count)),
            ("e", Some(f.services_count)),
        ],
    };
    pairs
        .into_iter()
        .filter_map(|(key, value)| Some((key.to_owned(), value?)))
        .collect()
}

/// The letter of `key` in one of the two letter tables, which list every
/// variant.
fn letter<K: PartialEq>(table: &[(K, &'static str)], key: &K) -> String {
    table
        .iter()
        .find(|(k, _)| k == key)
        .map_or("", |(_, l)| *l)
        .to_owned()
}

pub(crate) fn compact_of(full: &FullPayload) -> Result<CompactPayload, CardError> {
    let r = match &full.before_fix_period {
        Some(range) => Some((day(&range.from)?, day(&range.to)?)),
        None => None,
    };
    Ok(CompactPayload {
        v: CARD_FORMAT_VERSION,
        a: letter(&ANGLE_LETTERS, &full.angle),
        p: (
            letter(&KIND_LETTERS, &full.period.kind),
            day(&full.period.from)?,
            day(&full.period.to)?,
        ),
        f: compact_figures(&full.figures),
        r,
        d: full.org.domain.clone(),
        k: full.public_key.clone(),
        g: u32::try_from(full.generated_at_ms / MS_PER_DAY)
            .map_err(|_| malformed("generated_at_ms is out of range"))?,
    })
}

fn check_compact(compact: &CompactPayload) -> Result<(), CardError> {
    let angle = ANGLE_LETTERS
        .iter()
        .find(|(_, letter)| *letter == compact.a)
        .map(|(angle, _)| *angle)
        .ok_or_else(|| malformed(format!("unknown angle {:?}", compact.a)))?;
    let (kind, from, to) = &compact.p;
    if !KIND_LETTERS.iter().any(|(_, letter)| letter == kind) || from > to {
        return Err(malformed("p is not [kind letter, from day, to day]"));
    }
    if let Some((r_from, r_to)) = compact.r
        && (angle == Angle::Curve || r_from > r_to || r_to > *to)
    {
        return Err(malformed("r is not a before-fix period of this card"));
    }
    let keys: Vec<&str> = compact.f.keys().map(String::as_str).collect();
    let allowed = match angle {
        Angle::Story => STORY_COMPACT_KEYS.contains(&keys.as_slice()),
        Angle::Curve => keys == ["w", "x"],
        Angle::Collective => keys == ["e", "n"],
    };
    if !allowed {
        return Err(malformed(format!("f keys {keys:?} do not fit the angle")));
    }
    compact.d.as_deref().map_or(Ok(()), check_domain)?;
    check_public_key(&compact.k)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::report::card::keys::b64url_encode;
    use crate::report::card::test_fixtures::{
        collective_input, curve_input, story_input, story_input_with_threshold,
    };
    use crate::score::equivalence::embedded_equivalence_vintage;
    use serde_json::{Value, json};

    const FINGERPRINT: &str = "ps1:AAAAAAAAAAAAAAAAAAAAAA";

    fn public_key() -> String {
        b64url_encode(&[7u8; 32])
    }

    fn story() -> Value {
        story_input(FindingType::NPlusOneSql)
    }

    fn parse(input: &Value) -> Result<CardInput, CardError> {
        parse_card_input(&serde_json::to_vec(input).expect("serializes"))
    }

    fn reason(input: &Value) -> &'static str {
        parse(input).expect_err("refused").reason
    }

    fn accepted(input: &Value) {
        if let Err(e) = parse(input) {
            panic!("refused: {e}");
        }
    }

    fn payloads(input: &Value) -> (FullPayload, CompactPayload) {
        let input = parse(input).expect("accepted");
        build_payloads(&input, &public_key(), FINGERPRINT).expect("built")
    }

    fn as_json<T: Serialize>(value: &T) -> Value {
        serde_json::from_slice(&canonical_bytes(value)).expect("canonical JSON")
    }

    fn keys(value: &Value) -> Vec<String> {
        value.as_object().expect("object").keys().cloned().collect()
    }

    fn compact_json(input: &Value) -> Value {
        as_json(&payloads(input).1)
    }

    fn compact_reason(compact: &Value) -> &'static str {
        parse_compact_payload(&canonical_bytes(compact))
            .expect_err("refused")
            .reason
    }

    fn with(mut input: Value, pointer: &str, value: Value) -> Value {
        *input.pointer_mut(pointer).expect("pointer exists") = value;
        input
    }

    fn without(mut input: Value, pointer: &str) -> Value {
        let (parent, key) = pointer.rsplit_once('/').expect("pointer");
        let parent = if parent.is_empty() {
            &mut input
        } else {
            input.pointer_mut(parent).expect("parent exists")
        };
        parent.as_object_mut().expect("object").remove(key);
        input
    }

    #[test]
    fn canonical_bytes_sort_keys_and_carry_no_whitespace() {
        assert_eq!(
            canonical_bytes(&json!({"b": 1, "a": {"d": 2, "c": 3}})),
            br#"{"a":{"c":3,"d":2},"b":1}"#
        );
    }

    #[test]
    fn the_full_payload_has_the_c3_keys() {
        let full = as_json(&payloads(&story()).0);
        assert_eq!(
            keys(&full),
            [
                "angle",
                "before_fix_period",
                "equivalence",
                "figures",
                "free",
                "generated_at_ms",
                "key",
                "lang",
                "org",
                "period",
                "public_key",
                "scope",
                "story",
                "v"
            ]
        );
        assert_eq!(full["v"], 1);
        assert_eq!(
            full["before_fix_period"],
            json!({"from": "2026-07-01", "to": "2026-07-31"})
        );
        let curve = as_json(&payloads(&curve_input()).0);
        assert!(curve.get("story").is_none());
        assert!(curve.get("before_fix_period").is_none());
    }

    #[test]
    fn a_card_without_a_domain_omits_it_everywhere() {
        let input = with(story(), "/org", json!({"name": "Acme"}));
        let (full, compact) = payloads(&input);
        assert!(as_json(&full)["org"].get("domain").is_none());
        let compact_value = as_json(&compact);
        assert!(compact_value.get("d").is_none());
        assert_eq!(
            parse_compact_payload(&canonical_bytes(&compact)).expect("parses"),
            compact
        );
    }

    #[test]
    fn the_compact_payload_has_only_the_c3_keys() {
        let story = compact_json(&story());
        assert_eq!(keys(&story), ["a", "d", "f", "g", "k", "p", "r", "v"]);
        assert_eq!(story["p"], json!(["m", 20697, 20726]));
        assert_eq!(story["r"], json!([20635, 20665]));
        assert_eq!(story["g"], 20727);
        assert_eq!(story["f"], json!({"b": 1_937_000, "o": 41}));

        let curve = compact_json(&curve_input());
        assert_eq!(keys(&curve), ["a", "d", "f", "g", "k", "p", "v"]);
        assert_eq!(curve["f"], json!({"w": 31, "x": 48}));

        let collective = compact_json(&collective_input());
        assert_eq!(keys(&collective), ["a", "d", "f", "g", "k", "p", "r", "v"]);
        assert_eq!(collective["r"], json!([20666, 20696]));
        assert_eq!(collective["f"], json!({"e": 3, "n": 7}));

        let slow = compact_json(&story_input_with_threshold(FindingType::SlowSql));
        assert_eq!(slow["f"], json!({"m": 2349, "u": 500}));
        assert_eq!(slow["r"], json!([20666, 20696]));
    }

    #[test]
    fn a_far_future_period_date_is_malformed_not_a_panic() {
        assert_eq!(
            reason(&with(story(), "/period/from", json!("9999-01-01"))),
            "malformed_input"
        );
    }

    #[test]
    fn the_before_fix_period_is_the_one_the_figures_cover() {
        let august = json!({"from": "2026-08-01", "to": "2026-08-31"});
        let july = json!({"from": "2026-07-01", "to": "2026-07-31"});
        assert_eq!(
            reason(&with(story(), "/before_fix_period", august.clone())),
            "malformed_input"
        );
        assert_eq!(
            reason(&without(story(), "/before_fix_period")),
            "malformed_input"
        );
        let mut curve = curve_input();
        curve["before_fix_period"] = july.clone();
        assert_eq!(reason(&curve), "malformed_input");
        assert_eq!(
            reason(&with(
                story(),
                "/before_fix_period",
                json!({"from": "2026-07-01", "to": "2026-07-30"})
            )),
            "period_not_calendar"
        );

        let slow = story_input_with_threshold(FindingType::SlowSql);
        accepted(&slow);
        assert_eq!(
            reason(&with(slow.clone(), "/before_fix_period", july.clone())),
            "malformed_input"
        );
        assert_eq!(
            reason(&without(slow, "/story/observed_at")),
            "malformed_input"
        );
        let mut slow_without_threshold = story_input(FindingType::SlowSql);
        slow_without_threshold["before_fix_period"] = august;
        assert_eq!(reason(&slow_without_threshold), "malformed_input");

        assert_eq!(
            reason(&with(collective_input(), "/before_fix_period", july)),
            "malformed_input"
        );
        let no_extra_calls = with(
            collective_input(),
            "/figures/fixes_by_family_count",
            json!({"extra_calls": 0, "slow_calls": 6, "call_architecture": 1}),
        );
        assert_eq!(reason(&no_extra_calls), "malformed_input");

        let past_period = with(compact_json(&story()), "/r", json!([20666, 20760]));
        assert_eq!(compact_reason(&past_period), "malformed_input");
        let mut curve = compact_json(&curve_input());
        curve["r"] = json!([20666, 20696]);
        assert_eq!(compact_reason(&curve), "malformed_input");
    }

    #[test]
    fn parsing_the_canonical_bytes_gives_back_the_payload() {
        for input in [story(), curve_input(), collective_input()] {
            let (full, compact) = payloads(&input);
            assert_eq!(
                parse_full_payload(&canonical_bytes(&full)).expect("full parses"),
                full
            );
            assert_eq!(
                parse_compact_payload(&canonical_bytes(&compact)).expect("compact parses"),
                compact
            );
        }
    }

    #[test]
    fn the_equivalence_is_picked_by_scale() {
        let equivalence = payloads(&story()).0.equivalence.expect("picked");
        assert_eq!(equivalence.id, "meal");
        assert_eq!(equivalence.per_unit_ug, 2_040_000_000);
        assert_eq!(equivalence.source, "ADEME, Base Carbone");
        assert_eq!(equivalence.vintage, embedded_equivalence_vintage());

        let mut chosen = story();
        chosen["equivalence"] = json!("tgv_km");
        assert_eq!(reason(&chosen), "equivalence_out_of_scale");
        chosen["equivalence"] = json!("flight");
        assert_eq!(reason(&chosen), "unknown_equivalence");

        let no_co2 = without(story(), "/figures/co2_low_ug");
        assert!(as_json(&payloads(&no_co2).0).get("equivalence").is_none());
    }

    #[test]
    fn story_figures_follow_the_finding_type() {
        let mut slow = story_input(FindingType::SlowSql);
        slow["figures"]["before_fix_ops"] = json!(1000);
        assert_eq!(reason(&slow), "volume_not_avoidable");

        let mut redundant = story_input(FindingType::RedundantHttp);
        redundant["figures"]["threshold_ops"] = json!(50);
        assert_eq!(reason(&redundant), "threshold_not_applicable");

        let mut fanout = story_input(FindingType::ExcessiveFanout);
        fanout["figures"]["threshold_ms"] = json!(500);
        assert_eq!(reason(&fanout), "threshold_not_applicable");

        let mut slow_http = story_input(FindingType::SlowHttp);
        slow_http["figures"]["cited_ops"] = json!(3);
        assert_eq!(reason(&slow_http), "malformed_input");

        accepted(&without(story(), "/story/observed_at"));
    }

    #[test]
    fn co2_needs_the_volume_it_prices() {
        let mut slow = story_input(FindingType::SlowSql);
        slow["figures"]["co2_low_ug"] = json!(12_400_000_000_u64);
        assert_eq!(reason(&slow), "volume_not_avoidable");

        let mut collective = without(collective_input(), "/figures/before_fix_ops");
        collective = without(collective, "/before_fix_period");
        assert_eq!(reason(&collective), "malformed_input");

        accepted(&without(
            collective_input(),
            "/figures/previous_fixes_count",
        ));
    }

    #[test]
    fn the_period_must_be_closed_calendar_and_covered() {
        assert_eq!(
            reason(&with(story(), "/period/to", json!("2026-09-29"))),
            "period_not_calendar"
        );

        let mut week = curve_input();
        week["period"] = json!({"kind": "week", "from": "2026-10-05", "to": "2026-10-11"});
        week["scope"]["covered_days"] = json!(7);
        // 2026-10-11 12:00 UTC.
        week["generated_at_ms"] = json!(20_737_u64 * 86_400_000 + 43_200_000);
        assert_eq!(reason(&week), "period_not_closed");
        week["generated_at_ms"] = json!(20_738_u64 * 86_400_000);
        accepted(&week);

        assert_eq!(
            reason(&with(story(), "/scope/covered_days", json!(29))),
            "incomplete_period"
        );
        accepted(&with(story(), "/scope/restart_days", json!(2)));
        accepted(&with(story(), "/scope/sampled_permille", json!(0)));
        assert_eq!(
            reason(&with(story(), "/scope/sampled_permille", json!(1001))),
            "malformed_input"
        );
    }

    #[test]
    fn an_integer_above_the_browser_safe_range_is_malformed() {
        let input = with(
            story(),
            "/figures/before_fix_ops",
            json!(9_007_199_254_740_992_u64),
        );
        assert_eq!(reason(&input), "malformed_input");
    }

    #[test]
    fn free_text_with_bidi_controls_is_malformed() {
        let free = |free: Value| with(story(), "/free", free);
        assert_eq!(
            reason(&free(json!({"title": "Fixed\u{202E}gnippohs"}))),
            "malformed_input"
        );
        assert_eq!(
            reason(&with(story(), "/scope/environment", json!("prod\u{200B}"))),
            "malformed_input"
        );
        assert_eq!(
            reason(&with(story(), "/scope/sources", json!(["eu\u{202E}"]))),
            "malformed_input"
        );

        let a = |n: usize| "a".repeat(n);
        for (pointer, max) in [
            ("/free/title", 48),
            ("/free/team", 48),
            ("/free/label", 80),
            ("/org/name", 60),
            ("/story/service", 200),
            ("/story/endpoint", 200),
        ] {
            let mut input = story();
            if pointer.starts_with("/free") {
                input["free"] = json!({});
                input["free"][pointer.trim_start_matches("/free/")] = json!(a(max));
            } else {
                input = with(input, pointer, json!(a(max)));
            }
            accepted(&input);
            *input.pointer_mut(pointer).expect("set") = json!(a(max + 1));
            assert_eq!(reason(&input), "malformed_input", "{pointer}");
        }

        let mut curve = curve_input();
        curve["free"] = json!({"label": "Cart"});
        assert_eq!(reason(&curve), "malformed_input");
    }

    #[test]
    fn a_title_holding_the_slot_marker_is_malformed() {
        let marker = "{\"content_sig\":\"";
        let free = |free: Value| with(story(), "/free", free);
        let error =
            parse(&free(json!({"title": format!("Q3 {marker} review")}))).expect_err("refused");
        assert_eq!(error.reason, "malformed_input");
        assert_eq!(error.field, Some("free.title"));
        accepted(&free(json!({"title": "{\"content"})));
        accepted(&free(json!({"team": marker, "label": marker})));
        accepted(&with(story(), "/org/name", json!(marker)));
    }

    #[test]
    fn the_domain_must_be_a_public_host_name() {
        for domain in [
            "localhost",
            "10.0.0.1",
            "Acme.fr",
            "-acme.fr",
            "acme.fr@evil.example",
            "evil.example/x?",
            "acme.fr&name=_perf-sentinel.evil.com",
        ] {
            assert_eq!(
                reason(&with(story(), "/org/domain", json!(domain))),
                "malformed_input",
                "{domain}"
            );
            assert!(!is_public_host_name(domain), "{domain}");
        }
        accepted(&with(story(), "/org/domain", json!("cards.acme.fr")));
        assert!(is_public_host_name("cards.acme.fr"));
    }

    #[test]
    fn a_compact_payload_with_a_bad_domain_is_malformed() {
        let compact = compact_json(&story());
        assert!(parse_compact_payload(&canonical_bytes(&compact)).is_ok());
        for (pointer, value) in [
            ("/d", json!("acme.fr@evil.example")),
            ("/k", json!(b64url_encode(&[7u8; 31]))),
            ("/a", json!("z")),
            ("/p", json!(["x", 1, 2])),
            ("/p", json!(["m", 20, 10])),
            ("/f", json!({"w": 1})),
        ] {
            assert_eq!(
                compact_reason(&with(compact.clone(), pointer, value)),
                "malformed_input",
                "{pointer}"
            );
        }
    }

    #[test]
    fn the_language_is_en_or_fr() {
        accepted(&with(story(), "/lang", json!("fr")));
        assert_eq!(
            reason(&with(story(), "/lang", json!("de"))),
            "malformed_input"
        );
        assert_eq!(reason(&without(story(), "/lang")), "malformed_input");
    }

    #[test]
    fn unknown_keys_are_refused() {
        let mut extra = story();
        extra["colour"] = json!("green");
        assert_eq!(reason(&extra), "malformed_input");
        let mut curve = curve_input();
        curve["figures"]["cited_ops"] = json!(41);
        assert_eq!(reason(&curve), "malformed_input");
    }

    #[test]
    fn a_compact_payload_is_not_a_full_one() {
        let (full, compact) = payloads(&story());
        assert_eq!(
            parse_full_payload(&canonical_bytes(&compact))
                .expect_err("refused")
                .reason,
            "malformed_input"
        );
        assert_eq!(
            parse_compact_payload(&canonical_bytes(&full))
                .expect_err("refused")
                .reason,
            "malformed_input"
        );
    }
}
