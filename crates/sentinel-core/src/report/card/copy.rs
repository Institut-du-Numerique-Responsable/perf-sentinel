//! The words a card prints: one table per language with the same keys,
//! and the formatters that round every figure down, so a card never
//! prints more than was measured. French puts U+00A0 between a figure
//! and its unit and before `:` and `%`.

use super::payload::{Angle, Figures, FullPayload, Lang, PeriodKind, StoryFigures};
use crate::detect::FindingType;
use crate::score::equivalence::printed_co2_ug;
use crate::time::{epoch_day_to_iso_date, parse_iso_date_to_epoch_day};

pub(crate) const RENDERED_LANGS: &[Lang] = &[Lang::En, Lang::Fr];

/// Words a story sentence never holds, as whole words, case-insensitive.
#[cfg(test)]
pub(crate) const TECHNICAL_WORDS: &[&str] = &[
    "query",
    "queries",
    "request",
    "requests",
    "endpoint",
    "database",
    "sql",
    "http",
    "api",
    "span",
    "trace",
    "latency",
    "p99",
    "fanout",
    "pool",
    "thread",
    "service",
    "services",
    "template",
    "requête",
    "requêtes",
    "latence",
    "base de données",
];

const MS_PER_DAY: u64 = 86_400_000;
/// Beyond this many bars, a curve names only its first and last period.
pub(crate) const NAMED_BARS: usize = 6;

/// A singular and a plural.
type Forms = [&'static str; 2];

/// One language. Story sentences are templates with `{lead}`, `{before}`,
/// `{n}` and `{threshold}`, filled by [`fill`].
pub(crate) struct Words {
    decimal: char,
    group: char,
    /// Between a figure and its unit.
    gap: &'static str,
    /// Before a colon that closes a lead-in.
    colon: &'static str,
    percent: &'static str,
    under: &'static str,
    thousand: Forms,
    million: Forms,
    billion: Forms,
    second: Forms,
    millisecond: Forms,
    day: Forms,
    months: [&'static str; 12],
    first_of_month: &'static str,
    quarter: &'static str,
    halves: [&'static str; 2],
    week: &'static str,
    span: &'static str,
    /// `in {period}`, per kind: week, month, quarter, semester, year.
    in_period: [&'static str; 5],
    /// `the {kind} before`, per kind.
    kind_before: [&'static str; 5],
    /// The curve legend, per kind.
    legend: [&'static str; 5],

    lead_none: &'static str,
    /// Sentences without a threshold: N+1, redundant, slow, fanout, chatty,
    /// pool saturation, serialized calls.
    plain: [&'static str; 7],
    /// Sentences with a threshold: slow, fanout, chatty, pool saturation,
    /// serialized calls.
    with_threshold: [&'static str; 5],
    seen: &'static str,
    not_since: &'static str,

    before_fix: &'static str,
    before_fixes: Forms,
    avoidable: &'static str,
    operation: Forms,
    unnecessary: Forms,
    /// Before a noun counted in millions or more ("d'" in French).
    of_many: &'static str,
    at_least: &'static str,
    equal_to: &'static str,
    factor: &'static str,
    source: &'static str,

    collective: &'static str,
    fixes: Forms,
    confirmed: Forms,
    services: Forms,
    against: &'static str,
    curve_against: &'static str,
    families: [&'static str; 3],

    measured_on: &'static str,
    restart: &'static str,
    traffic: &'static str,
    org: &'static str,
    generated: &'static str,

    pub(crate) fix_confirmed: &'static str,
    pub(crate) fixes_confirmed: &'static str,
    pub(crate) what_was_fixed: &'static str,
    pub(crate) curve_what: &'static str,
    pub(crate) lower_is_better: &'static str,
    pub(crate) slowest_before: &'static str,
    pub(crate) alert_threshold: &'static str,
    pub(crate) measured_with: &'static str,
    pub(crate) verify: &'static str,
    pub(crate) verify_url: &'static str,
    pub(crate) check_title: &'static str,
    pub(crate) method: &'static str,
    pub(crate) method_url: &'static str,
    /// The PDF title when the card has no free title.
    pub(crate) pdf_title: &'static str,
    why: [&'static str; 2],
}

const EN: Words = Words {
    decimal: '.',
    group: ',',
    gap: " ",
    colon: ":",
    percent: "{p}%",
    under: "under {p}",
    thousand: ["thousand", "thousand"],
    million: ["million", "million"],
    billion: ["billion", "billion"],
    second: ["second", "seconds"],
    millisecond: ["millisecond", "milliseconds"],
    day: ["day", "days"],
    months: [
        "January",
        "February",
        "March",
        "April",
        "May",
        "June",
        "July",
        "August",
        "September",
        "October",
        "November",
        "December",
    ],
    first_of_month: "1",
    quarter: "Q",
    halves: ["first half of {y}", "second half of {y}"],
    week: "week of {day}",
    span: "{from} to {to}",
    in_period: ["in the {p}", "in {p}", "in {p}", "in the {p}", "in {p}"],
    kind_before: [
        "the week before",
        "the month before",
        "the quarter before",
        "the half-year before",
        "the year before",
    ],
    legend: [
        "Share of the work that was avoidable, week by week",
        "Share of the work that was avoidable, month by month",
        "Share of the work that was avoidable, quarter by quarter",
        "Share of the work that was avoidable, half-year by half-year",
        "Share of the work that was avoidable, year by year",
    ],

    lead_none: "For one action in the application, ",
    plain: [
        "{lead}the application fetched information {n} times instead of once.",
        "{lead}the application asked {n} times for the same information.",
        "{lead}the slowest runs of one step took {n} or more.",
        "{lead}a single step set off {n} operations.",
        "{lead}the application reached out to other parts of the system {n} times.",
        "{lead}{n} operations ran at the same moment, at the peak.",
        "{lead}the application took {n} steps one after the other.",
    ],
    with_threshold: [
        "{before}, before the fix, the slowest runs of one step took {n} or more. It still runs and no longer triggers the slowness alert, set at {threshold}.",
        "{before}, before the fix, a single step set off {n} operations. It still runs and no longer triggers the alert, set at {threshold} operations.",
        "{before}, before the fix, the application reached out to other parts of the system {n} times. The action still runs and no longer triggers the alert, set at {threshold} times.",
        "{before}, before the fix, {n} operations ran at the same moment, at the peak. The action still runs and no longer triggers the alert, set at {threshold} operations.",
        "{before}, before the fix, the application took {n} steps one after the other. The action still runs and no longer triggers the alert, set at {threshold} steps.",
    ],
    seen: "Seen on {date}.",
    not_since: "Not detected since {date}.",

    before_fix: "{before}, before the fix",
    before_fixes: [
        "{before}, before the fix of work done more than once",
        "{before}, before the {n} fixes of work done more than once",
    ],
    avoidable: "Avoidable {in}",
    operation: ["operation", "operations"],
    unnecessary: ["unnecessary operation", "unnecessary operations"],
    of_many: "",
    at_least: ", at least {co2} CO2e (estimate)",
    equal_to: "Equal to at least {n} {unit}",
    factor: "1 {unit} = {factor} CO2e.",
    source: "Source: {source}, {vintage}",

    collective: "{n} {fixes} {confirmed} by measurement, across {s} {services},",
    fixes: ["fix", "fixes"],
    confirmed: ["confirmed", "confirmed"],
    services: ["service", "services"],
    against: "against {p} {before}",
    curve_against: ", against {x} {in}",
    families: [
        "Work done more than once",
        "Slow steps",
        "How the work was ordered",
    ],

    measured_on: "Measured on {names}, {days}",
    restart: ", {days} with a restart",
    traffic: ", on {p} of traffic",
    org: "Organisation, as declared: {name}",
    generated: "Generated on {date}",

    fix_confirmed: "Fix confirmed by measurement",
    fixes_confirmed: "Fixes confirmed by measurement",
    what_was_fixed: "What was fixed",
    curve_what: "Share of the work that was avoidable",
    lower_is_better: "The lower, the better.",
    slowest_before: "Slowest runs, before the fix",
    alert_threshold: "Alert threshold",
    measured_with: "Measured with Perf Sentinel, free software carried by the Institut du Numérique Responsable",
    verify: "Check it and read the method:",
    verify_url: "perf-sentinel.dev/verifier",
    check_title: "Check these figures",
    method: "Method:",
    method_url: "github.com/Institut-du-Numerique-Responsable/perf-sentinel/blob/main/docs/SHARE-CARD.md",
    pdf_title: "Perf Sentinel card",
    why: [
        "Every unnecessary operation costs server time, energy and sometimes waiting.",
        "Measuring them shows where the application can do the same work with less.",
    ],
};

const FR: Words = Words {
    decimal: ',',
    group: '\u{a0}',
    gap: "\u{a0}",
    colon: "\u{a0}:",
    percent: "{p}\u{a0}%",
    under: "moins de {p}",
    thousand: ["mille", "mille"],
    million: ["million", "millions"],
    billion: ["milliard", "milliards"],
    second: ["seconde", "secondes"],
    millisecond: ["milliseconde", "millisecondes"],
    day: ["jour", "jours"],
    months: [
        "janvier",
        "février",
        "mars",
        "avril",
        "mai",
        "juin",
        "juillet",
        "août",
        "septembre",
        "octobre",
        "novembre",
        "décembre",
    ],
    first_of_month: "1er",
    quarter: "T",
    halves: ["premier semestre {y}", "second semestre {y}"],
    week: "semaine du {day}",
    span: "{from} à {to}",
    in_period: ["pendant la {p}", "en {p}", "au {p}", "au {p}", "en {p}"],
    kind_before: [
        "la semaine précédente",
        "le mois précédent",
        "le trimestre précédent",
        "le semestre précédent",
        "l'année précédente",
    ],
    legend: [
        "Part du travail évitable, semaine par semaine",
        "Part du travail évitable, mois par mois",
        "Part du travail évitable, trimestre par trimestre",
        "Part du travail évitable, semestre par semestre",
        "Part du travail évitable, année par année",
    ],

    lead_none: "Pour une action de l'application, ",
    plain: [
        "{lead}l'application allait chercher les informations {n} fois au lieu d'une.",
        "{lead}l'application demandait {n} fois la même information.",
        "{lead}les passages les plus lents d'une étape prenaient {n} ou plus.",
        "{lead}une seule étape lançait {n} opérations.",
        "{lead}l'application sollicitait d'autres parties du système {n} fois.",
        "{lead}{n} opérations tournaient au même moment, au plus fort.",
        "{lead}l'application enchaînait {n} étapes l'une après l'autre.",
    ],
    with_threshold: [
        "{before}, avant la correction, les passages les plus lents d'une étape prenaient {n} ou plus. Elle tourne toujours et ne déclenche plus l'alerte de lenteur, réglée à {threshold}.",
        "{before}, avant la correction, une seule étape lançait {n} opérations. Elle tourne toujours et ne déclenche plus l'alerte, réglée à {threshold} opérations.",
        "{before}, avant la correction, l'application sollicitait d'autres parties du système {n} fois. L'action tourne toujours et ne déclenche plus l'alerte, réglée à {threshold} fois.",
        "{before}, avant la correction, {n} opérations tournaient au même moment, au plus fort. L'action tourne toujours et ne déclenche plus l'alerte, réglée à {threshold} opérations.",
        "{before}, avant la correction, l'application enchaînait {n} étapes l'une après l'autre. L'action tourne toujours et ne déclenche plus l'alerte, réglée à {threshold} étapes.",
    ],
    seen: "Observé le {date}.",
    not_since: "N'est plus détecté depuis le {date}.",

    before_fix: "{before}, avant la correction",
    before_fixes: [
        "{before}, avant la correction de travail fait plusieurs fois",
        "{before}, avant les {n} corrections de travail fait plusieurs fois",
    ],
    avoidable: "Évitable {in}",
    operation: ["opération", "opérations"],
    unnecessary: ["opération inutile", "opérations inutiles"],
    of_many: "d'",
    at_least: ", au moins {co2} CO2e (estimation)",
    equal_to: "L'équivalent d'au moins {n} {unit}",
    factor: "1 {unit} = {factor} CO2e.",
    source: "Source\u{a0}: {source}, {vintage}",

    collective: "{n} {fixes} {confirmed} par la mesure, sur {s} {services},",
    fixes: ["correction", "corrections"],
    confirmed: ["confirmée", "confirmées"],
    services: ["service", "services"],
    against: "contre {p} {before}",
    curve_against: ", contre {x} {in}",
    families: [
        "Travail fait plusieurs fois",
        "Étapes lentes",
        "Ordre dans lequel le travail était fait",
    ],

    measured_on: "Mesuré sur {names}, {days}",
    restart: ", {days} avec redémarrage",
    traffic: ", sur {p} du trafic",
    org: "Organisation déclarée\u{a0}: {name}",
    generated: "Généré le {date}",

    fix_confirmed: "Correction confirmée par la mesure",
    fixes_confirmed: "Corrections confirmées par la mesure",
    what_was_fixed: "Ce qui a été corrigé",
    curve_what: "Part du travail évitable",
    lower_is_better: "Plus c'est bas, mieux c'est.",
    slowest_before: "Passages les plus lents, avant la correction",
    alert_threshold: "Seuil d'alerte",
    measured_with: "Mesuré avec Perf Sentinel, logiciel libre porté par l'Institut du Numérique Responsable",
    verify: "Vérifiez la carte et lisez la méthode\u{a0}:",
    verify_url: "perf-sentinel.dev/verifier",
    check_title: "Vérifiez ces chiffres",
    method: "Méthode\u{a0}:",
    method_url: "github.com/Institut-du-Numerique-Responsable/perf-sentinel/blob/main/docs/FR/SHARE-CARD-FR.md",
    pdf_title: "Carte Perf Sentinel",
    why: [
        "Chaque opération inutile coûte du temps de serveur, de l'énergie et parfois de l'attente.",
        "Les mesurer montre où l'application peut faire le même travail avec moins.",
    ],
};

pub(crate) fn words(lang: Lang) -> &'static Words {
    match lang {
        Lang::En => &EN,
        Lang::Fr => &FR,
    }
}

/// `template` with each `{key}` replaced by its value.
fn fill(template: &str, values: &[(&str, &str)]) -> String {
    values
        .iter()
        .fold(template.to_owned(), |text, (key, value)| {
            text.replace(&format!("{{{key}}}"), value)
        })
}

/// The singular below 2 in French and for exactly one in English.
fn form(forms: Forms, lang: Lang, one: bool, below_two: bool) -> &'static str {
    let singular = match lang {
        Lang::En => one,
        Lang::Fr => below_two,
    };
    forms[usize::from(!singular)]
}

fn count_form(forms: Forms, n: u64, lang: Lang) -> &'static str {
    form(forms, lang, n == 1, n < 2)
}

fn capitalize(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect()
    })
}

fn lower_first(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_lowercase().chain(chars).collect()
    })
}

fn grouped(n: u64, lang: Lang) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(words(lang).group);
        }
        out.push(c);
    }
    out
}

/// `value / unit` written out exactly, trailing zeros dropped. `unit` is a
/// power of ten.
fn exact(value: u64, unit: u64, lang: Lang) -> String {
    let int = grouped(value / unit, lang);
    let width = usize::try_from(unit.ilog10()).unwrap_or(0);
    if width == 0 || value.is_multiple_of(unit) {
        return int;
    }
    let frac = format!("{:0width$}", value % unit);
    format!("{int}{}{}", words(lang).decimal, frac.trim_end_matches('0'))
}

/// `n` rounded down to two significant figures.
fn floor_two(n: u64) -> u64 {
    let mut scale = 1;
    while n / scale >= 100 {
        scale *= 10;
    }
    n / scale * scale
}

/// Exact below 10 000, else two significant figures rounded down in
/// thousands, millions or billions.
pub(crate) fn format_count_floor(n: u64, lang: Lang) -> String {
    if n < 10_000 {
        return grouped(n, lang);
    }
    let w = words(lang);
    let (scale, forms) = match n {
        1_000_000_000.. => (1_000_000_000, w.billion),
        1_000_000.. => (1_000_000, w.million),
        _ => (1_000, w.thousand),
    };
    let floored = floor_two(n);
    let word = form(forms, lang, floored == scale, floored < 2 * scale);
    format!("{}{}{word}", exact(floored, scale, lang), w.gap)
}

/// A mass in µg, written exactly in t, kg or g.
fn format_mass(ug: u64, lang: Lang) -> String {
    let (unit, symbol) = match ug {
        1_000_000_000_000.. => (1_000_000_000_000, "t"),
        1_000_000_000.. => (1_000_000_000, "kg"),
        _ => (1_000_000, "g"),
    };
    format!("{}{}{symbol}", exact(ug, unit, lang), words(lang).gap)
}

/// The CO2 a card prints: [`printed_co2_ug`] with its unit.
pub(crate) fn format_co2_floor(ug: u64, lang: Lang) -> String {
    format_mass(printed_co2_ug(ug), lang)
}

/// Whole milliseconds below 100 ms, else seconds with one decimal
/// truncated.
pub(crate) fn format_seconds_floor(ms: u64, lang: Lang) -> String {
    let w = words(lang);
    if ms < 100 {
        return format!(
            "{}{}{}",
            grouped(ms, lang),
            w.gap,
            count_form(w.millisecond, ms, lang)
        );
    }
    let tenths = ms / 100;
    let unit = form(w.second, lang, tenths == 10, tenths < 20);
    format!("{}{}{unit}", exact(tenths, 10, lang), w.gap)
}

/// A share in permille as a percentage, "under 0.1%" for zero.
pub(crate) fn format_permille(p: u32, lang: Lang) -> String {
    let w = words(lang);
    let percent = |tenths: u32| fill(w.percent, &[("p", &exact(u64::from(tenths), 10, lang))]);
    if p == 0 {
        fill(w.under, &[("p", &percent(1))])
    } else {
        percent(p)
    }
}

/// Year, month and day of a `YYYY-MM-DD` date the payload rules checked.
fn ymd(iso: &str) -> (u32, u32, u32) {
    let part = |range: std::ops::Range<usize>| {
        iso.get(range)
            .and_then(|digits| digits.parse().ok())
            .unwrap_or(1)
    };
    (part(0..4), part(5..7).clamp(1, 12), part(8..10))
}

fn month_name(month: u32, lang: Lang) -> &'static str {
    words(lang).months[(month as usize).clamp(1, 12) - 1]
}

fn day_and_month(iso: &str, lang: Lang) -> String {
    let (_, month, day) = ymd(iso);
    let day = if day == 1 {
        words(lang).first_of_month.to_owned()
    } else {
        day.to_string()
    };
    format!("{day} {}", month_name(month, lang))
}

/// "28 August 2026", "1er octobre 2026".
pub(crate) fn format_day(iso: &str, lang: Lang) -> String {
    format!("{} {}", day_and_month(iso, lang), ymd(iso).0)
}

fn kind_index(kind: PeriodKind) -> usize {
    match kind {
        PeriodKind::Week => 0,
        PeriodKind::Month => 1,
        PeriodKind::Quarter => 2,
        PeriodKind::Semester => 3,
        PeriodKind::Year => 4,
    }
}

fn months_in(kind: PeriodKind) -> u32 {
    match kind {
        PeriodKind::Week => 0,
        PeriodKind::Month => 1,
        PeriodKind::Quarter => 3,
        PeriodKind::Semester => 6,
        PeriodKind::Year => 12,
    }
}

/// The period that starts on `from`, in words.
pub(crate) fn period_name(kind: PeriodKind, from: &str, lang: Lang) -> String {
    let w = words(lang);
    let (year, month, _) = ymd(from);
    let year_text = year.to_string();
    match kind {
        PeriodKind::Week => fill(w.week, &[("day", &format_day(from, lang))]),
        PeriodKind::Month => format!("{} {year}", month_name(month, lang)),
        PeriodKind::Quarter => format!("{}{} {year}", w.quarter, (month - 1) / 3 + 1),
        PeriodKind::Semester => fill(w.halves[usize::from(month > 6)], &[("y", &year_text)]),
        PeriodKind::Year => year_text,
    }
}

/// `in July 2026`, `in the week of …`, `au premier semestre 2026`.
fn in_period(kind: PeriodKind, from: &str, lang: Lang) -> String {
    fill(
        words(lang).in_period[kind_index(kind)],
        &[("p", &period_name(kind, from, lang))],
    )
}

/// The first day of the period `back` periods before the one starting on
/// `from`.
fn shift_back(kind: PeriodKind, from: &str, back: u32) -> String {
    let months = months_in(kind);
    if months == 0 {
        let day = parse_iso_date_to_epoch_day(from).unwrap_or(0);
        return epoch_day_to_iso_date(day.saturating_sub(7 * back));
    }
    let (year, month, _) = ymd(from);
    let index = (year * 12 + month - 1).saturating_sub(back * months);
    format!("{:04}-{:02}-01", index / 12, index % 12 + 1)
}

/// The days from `from` to `to`, both included.
fn days_between(from: &str, to: &str) -> u32 {
    match (
        parse_iso_date_to_epoch_day(from),
        parse_iso_date_to_epoch_day(to),
    ) {
        (Some(from), Some(to)) => to.saturating_sub(from) + 1,
        _ => 0,
    }
}

fn curve_values(payload: &FullPayload) -> Option<&[u32]> {
    match &payload.figures {
        Figures::Curve(f) => Some(&f.waste_permille),
        _ => None,
    }
}

/// The first day of the oldest period a curve shows.
fn curve_start(payload: &FullPayload, bars: usize) -> String {
    let back = u32::try_from(bars.saturating_sub(1)).unwrap_or(u32::MAX);
    shift_back(payload.period.kind, &payload.period.from, back)
}

/// The period the header prints: the card's, or the span a curve shows.
pub(crate) fn header_period(payload: &FullPayload) -> String {
    let lang = payload.lang;
    let period = &payload.period;
    let name = period_name(period.kind, &period.from, lang);
    let Some(values) = curve_values(payload) else {
        return name;
    };
    let start = curve_start(payload, values.len());
    let same_year = start.get(..4) == period.from.get(..4);
    let first = match period.kind {
        PeriodKind::Month if same_year => month_name(ymd(&start).1, lang).to_owned(),
        PeriodKind::Quarter if same_year => {
            format!("{}{}", words(lang).quarter, (ymd(&start).1 - 1) / 3 + 1)
        }
        kind => period_name(kind, &start, lang),
    };
    fill(words(lang).span, &[("from", &first), ("to", &name)])
}

/// The name under each bar of a curve, oldest first: every one up to
/// [`NAMED_BARS`], else only the first and the last, in full.
pub(crate) fn curve_bar_labels(payload: &FullPayload) -> Vec<String> {
    let Some(values) = curve_values(payload) else {
        return Vec::new();
    };
    let lang = payload.lang;
    let kind = payload.period.kind;
    let bars = values.len();
    (0..bars)
        .map(|i| {
            let back = u32::try_from(bars - 1 - i).unwrap_or(u32::MAX);
            let from = shift_back(kind, &payload.period.from, back);
            if bars > NAMED_BARS {
                return if i == 0 || i == bars - 1 {
                    period_name(kind, &from, lang)
                } else {
                    String::new()
                };
            }
            match kind {
                PeriodKind::Week => day_and_month(&from, lang),
                PeriodKind::Month => month_name(ymd(&from).1, lang).to_owned(),
                PeriodKind::Quarter => {
                    format!("{}{}", words(lang).quarter, (ymd(&from).1 - 1) / 3 + 1)
                }
                PeriodKind::Semester | PeriodKind::Year => period_name(kind, &from, lang),
            }
        })
        .collect()
}

fn story_parts(payload: &FullPayload) -> Option<&StoryFigures> {
    match &payload.figures {
        Figures::Story(f) => Some(f),
        _ => None,
    }
}

/// The index of a finding type in `Words::plain`.
fn plain_index(finding_type: &FindingType) -> usize {
    match finding_type {
        FindingType::NPlusOneSql | FindingType::NPlusOneHttp | FindingType::NPlusOneMessaging => 0,
        FindingType::RedundantSql | FindingType::RedundantHttp => 1,
        FindingType::SlowSql | FindingType::SlowHttp | FindingType::SlowMessaging => 2,
        FindingType::ExcessiveFanout => 3,
        FindingType::ChattyService => 4,
        FindingType::PoolSaturation => 5,
        FindingType::SerializedCalls => 6,
    }
}

/// The figure a story cites, with its unit when it is a duration.
pub(crate) fn cited_figure(payload: &FullPayload) -> String {
    let lang = payload.lang;
    story_parts(payload).map_or_else(String::new, |f| match (f.cited_ms, f.cited_ops) {
        (Some(ms), _) => format_seconds_floor(ms, lang),
        (None, ops) => format_count_floor(ops.unwrap_or(0), lang),
    })
}

fn threshold_text(f: &StoryFigures, lang: Lang) -> Option<String> {
    f.threshold_ms
        .map(|ms| format_seconds_floor(ms, lang))
        .or_else(|| f.threshold_ops.map(|ops| format_count_floor(ops, lang)))
}

/// The story in one or two sentences, citing its own figure.
pub(crate) fn story_sentence(payload: &FullPayload) -> String {
    let (Some(story), Some(f)) = (&payload.story, story_parts(payload)) else {
        return String::new();
    };
    let lang = payload.lang;
    let w = words(lang);
    let n = cited_figure(payload);
    let index = plain_index(&story.finding_type);
    let label_lead = payload
        .free
        .label
        .as_ref()
        .map(|label| format!("{label}{} ", w.colon));
    let threshold = threshold_text(f, lang);
    let template = index.checked_sub(2).and_then(|i| w.with_threshold.get(i));
    if let (Some(threshold), Some(range), Some(template)) =
        (threshold, &payload.before_fix_period, template)
    {
        let before = capitalize(&in_period(payload.period.kind, &range.from, lang));
        let sentence = fill(
            template,
            &[("before", &before), ("n", &n), ("threshold", &threshold)],
        );
        return match label_lead {
            Some(lead) => format!("{lead}{}", lower_first(&sentence)),
            None => sentence,
        };
    }
    let lead = label_lead.unwrap_or_else(|| w.lead_none.to_owned());
    fill(w.plain[index], &[("lead", &lead), ("n", &n)])
}

/// When the trace was seen and, without a threshold, since when the
/// finding is no longer detected.
pub(crate) fn story_fact(payload: &FullPayload) -> String {
    let (Some(story), Some(f)) = (&payload.story, story_parts(payload)) else {
        return String::new();
    };
    let lang = payload.lang;
    let w = words(lang);
    let mut facts = Vec::new();
    if let Some(observed) = &story.observed_at {
        facts.push(fill(w.seen, &[("date", &format_day(observed, lang))]));
    }
    if threshold_text(f, lang).is_none() {
        facts.push(fill(
            w.not_since,
            &[("date", &format_day(&story.done_at, lang))],
        ));
    }
    facts.join(" ")
}

/// The CO2 line in three parts: the lead-in, the figure the layout prints
/// large, and the rest.
pub(crate) fn co2_parts(payload: &FullPayload) -> Option<(String, String, String)> {
    let lang = payload.lang;
    let w = words(lang);
    let before = || {
        payload
            .before_fix_period
            .as_ref()
            .map(|range| capitalize(&in_period(payload.period.kind, &range.from, lang)))
    };
    let (lead, ops, unnecessary, co2) = match &payload.figures {
        Figures::Story(f) => {
            let ops = f.before_fix_ops?;
            let lead = fill(w.before_fix, &[("before", &before()?)]);
            (lead, ops, true, f.co2_low_ug)
        }
        Figures::Curve(f) => {
            let lead = fill(
                w.avoidable,
                &[(
                    "in",
                    &in_period(payload.period.kind, &payload.period.from, lang),
                )],
            );
            (lead, f.avoidable_ops, false, f.co2_low_ug)
        }
        Figures::Collective(f) => {
            let ops = f.before_fix_ops?;
            let fixes = f.fixes_by_family_count.extra_calls;
            let template = w.before_fixes[usize::from(fixes != 1)];
            let lead = fill(
                template,
                &[("before", &before()?), ("n", &fixes.to_string())],
            );
            (lead, ops, true, f.co2_low_ug)
        }
    };
    let noun = count_form(
        if unnecessary {
            w.unnecessary
        } else {
            w.operation
        },
        ops,
        lang,
    );
    let of = if ops >= 1_000_000 { w.of_many } else { "" };
    let tail = co2.map_or_else(String::new, |ug| {
        fill(w.at_least, &[("co2", &format_co2_floor(ug, lang))])
    });
    Some((
        format!("{lead}{}", w.colon),
        format_count_floor(ops, lang),
        format!("{of}{noun}{tail}"),
    ))
}

/// What the CO2 figure measures and over which period, `None` when the
/// card prints no volume.
pub(crate) fn co2_line(payload: &FullPayload) -> Option<String> {
    co2_parts(payload).map(|(lead, figure, rest)| format!("{lead} {figure} {rest}"))
}

/// The count line, then the factor and its source, from the signed
/// equivalence on the CO2 as printed.
pub(crate) fn equivalence_lines(payload: &FullPayload) -> Option<(String, String)> {
    co2_line(payload)?;
    let e = payload.equivalence.as_ref()?;
    let co2 = match &payload.figures {
        Figures::Story(f) => f.co2_low_ug,
        Figures::Curve(f) => f.co2_low_ug,
        Figures::Collective(f) => f.co2_low_ug,
    }?;
    let lang = payload.lang;
    let w = words(lang);
    let count = printed_co2_ug(co2) / e.per_unit_ug.max(1);
    let count_line = fill(
        w.equal_to,
        &[
            ("n", &format_count_floor(count, lang)),
            ("unit", equivalence_unit(&e.id, count, lang)),
        ],
    );
    let factor = fill(
        w.factor,
        &[
            ("unit", equivalence_unit(&e.id, 1, lang)),
            ("factor", &format_mass(e.per_unit_ug, lang)),
        ],
    );
    let source = fill(w.source, &[("source", &e.source), ("vintage", &e.vintage)]);
    Some((count_line, format!("{factor} {source}")))
}

/// The collective sentence, then the comparison with the period before
/// when the Hub could count it.
pub(crate) fn collective_lines(payload: &FullPayload) -> Vec<String> {
    let Figures::Collective(f) = &payload.figures else {
        return Vec::new();
    };
    let lang = payload.lang;
    let w = words(lang);
    let n = f.confirmed_fixes_count;
    let s = f.services_count;
    let mut lines = vec![fill(
        w.collective,
        &[
            ("n", &grouped(n, lang)),
            ("fixes", count_form(w.fixes, n, lang)),
            ("confirmed", count_form(w.confirmed, n, lang)),
            ("s", &grouped(s, lang)),
            ("services", count_form(w.services, s, lang)),
        ],
    )];
    if let Some(previous) = f.previous_fixes_count {
        lines.push(fill(
            w.against,
            &[
                ("p", &grouped(previous, lang)),
                ("before", w.kind_before[kind_index(payload.period.kind)]),
            ],
        ));
    }
    lines
}

/// The captions glued to the two collective figures.
pub(crate) fn collective_captions(payload: &FullPayload) -> Option<[(String, &'static str); 2]> {
    let Figures::Collective(f) = &payload.figures else {
        return None;
    };
    let lang = payload.lang;
    let w = words(lang);
    Some([
        (
            grouped(f.confirmed_fixes_count, lang),
            count_form(w.fixes, f.confirmed_fixes_count, lang),
        ),
        (
            grouped(f.services_count, lang),
            count_form(w.services, f.services_count, lang),
        ),
    ])
}

pub(crate) fn family_label(family: &str, lang: Lang) -> &'static str {
    let families = &words(lang).families;
    match family {
        "extra_calls" => families[0],
        "slow_calls" => families[1],
        "call_architecture" => families[2],
        _ => "",
    }
}

/// `("in September 2026", ", against 4.8% in August 2026")`: the card's
/// period, then the one right before it.
pub(crate) fn curve_comparison(payload: &FullPayload) -> (String, String) {
    let lang = payload.lang;
    let period = &payload.period;
    let current = in_period(period.kind, &period.from, lang);
    let previous = curve_values(payload)
        .and_then(|values| values.iter().rev().nth(1))
        .map_or_else(String::new, |&x| {
            let before = shift_back(period.kind, &period.from, 1);
            fill(
                words(lang).curve_against,
                &[
                    ("x", &format_permille(x, lang)),
                    ("in", &in_period(period.kind, &before, lang)),
                ],
            )
        });
    (current, previous)
}

/// What the curve shows, its comparison, how to read it, and the legend.
pub(crate) fn curve_lines(payload: &FullPayload) -> Vec<String> {
    let w = words(payload.lang);
    let (current, previous) = curve_comparison(payload);
    vec![
        w.curve_what.to_owned(),
        format!("{current}{previous}"),
        w.lower_is_better.to_owned(),
        w.legend[kind_index(payload.period.kind)].to_owned(),
    ]
}

pub(crate) fn why_it_matters(lang: Lang) -> &'static [&'static str] {
    &words(lang).why
}

/// Where and how long the figures were measured; a curve counts the days
/// of every period it shows.
pub(crate) fn scope_line(payload: &FullPayload) -> String {
    let lang = payload.lang;
    let w = words(lang);
    let scope = &payload.scope;
    let names: Vec<&str> = scope
        .sources
        .iter()
        .chain(&scope.environment)
        .map(String::as_str)
        .collect();
    let days = match curve_values(payload) {
        Some(values) => days_between(&curve_start(payload, values.len()), &payload.period.to),
        None => scope.covered_days,
    };
    let days_text = |n: u32| {
        format!(
            "{} {}",
            grouped(u64::from(n), lang),
            count_form(w.day, u64::from(n), lang)
        )
    };
    let mut line = fill(
        w.measured_on,
        &[("names", &names.join(", ")), ("days", &days_text(days))],
    );
    if scope.restart_days > 0 {
        line.push_str(&fill(
            w.restart,
            &[("days", &days_text(scope.restart_days))],
        ));
    }
    if scope.sampled_permille < 1000 {
        line.push_str(&fill(
            w.traffic,
            &[("p", &format_permille(scope.sampled_permille, lang))],
        ));
    }
    line
}

pub(crate) fn org_line(payload: &FullPayload) -> String {
    fill(words(payload.lang).org, &[("name", &payload.org.name)])
}

pub(crate) fn generated_line(payload: &FullPayload) -> String {
    let day = u32::try_from(payload.generated_at_ms / MS_PER_DAY).unwrap_or(u32::MAX);
    fill(
        words(payload.lang).generated,
        &[(
            "date",
            &format_day(&epoch_day_to_iso_date(day), payload.lang),
        )],
    )
}

/// The confirmation the proof prints, `None` on a curve, which names no
/// fix.
pub(crate) fn confirmation(payload: &FullPayload) -> Option<&'static str> {
    let w = words(payload.lang);
    match payload.angle {
        Angle::Story => Some(w.fix_confirmed),
        Angle::Collective => Some(w.fixes_confirmed),
        Angle::Curve => None,
    }
}

/// The unit an equivalence counts, agreed with `count`. The table's
/// `label` stays the English docs label.
pub(crate) fn equivalence_unit(id: &str, count: u64, lang: Lang) -> &'static str {
    let forms: Forms = match (id, lang) {
        ("meal", Lang::En) => ["average meal", "average meals"],
        ("meal", Lang::Fr) => ["repas moyen", "repas moyens"],
        ("tgv_km", Lang::En) => ["km by high-speed train"; 2],
        ("tgv_km", Lang::Fr) => ["km en TGV"; 2],
        ("car_km", Lang::En) => ["km driven by an average car"; 2],
        ("car_km", Lang::Fr) => ["km parcourus par une voiture moyenne"; 2],
        ("smartphone", _) => ["smartphone", "smartphones"],
        // shortcut: an id this binary does not know prints the English docs
        // label, or nothing; upgrade when a card renders a payload it did not build.
        _ => return crate::score::equivalence::find_equivalence(id).map_or("", |e| e.label),
    };
    count_form(forms, count, lang)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detect::FindingType;
    use crate::report::card::payload::Figures;
    use crate::report::card::test_fixtures::{
        collective_input, curve_input, full_payload, story_input, story_input_with_threshold,
    };
    use serde_json::{Value, json};

    const LANGS: [Lang; 2] = [Lang::En, Lang::Fr];
    const ALL_TYPES: [FindingType; 12] = [
        FindingType::NPlusOneSql,
        FindingType::NPlusOneHttp,
        FindingType::NPlusOneMessaging,
        FindingType::RedundantSql,
        FindingType::RedundantHttp,
        FindingType::SlowSql,
        FindingType::SlowHttp,
        FindingType::SlowMessaging,
        FindingType::ExcessiveFanout,
        FindingType::ChattyService,
        FindingType::PoolSaturation,
        FindingType::SerializedCalls,
    ];

    fn in_lang(mut input: Value, lang: Lang) -> Value {
        input["lang"] = serde_json::to_value(lang).expect("a language serializes");
        input
    }

    fn payload(input: Value, lang: Lang) -> FullPayload {
        full_payload(&in_lang(input, lang))
    }

    fn takes_a_threshold(finding_type: &FindingType) -> bool {
        let name = serde_json::to_value(finding_type).expect("serializes");
        let name = name.as_str().expect("a string");
        !name.starts_with("n_plus_one_") && !name.starts_with("redundant_")
    }

    /// Whether `word` appears in `text` with no letter or digit glued to it.
    fn has_word(text: &str, word: &str) -> bool {
        let text = text.to_lowercase();
        text.match_indices(word).any(|(at, _)| {
            let before = text[..at].chars().next_back();
            let after = text[at + word.len()..].chars().next();
            !before.is_some_and(char::is_alphanumeric) && !after.is_some_and(char::is_alphanumeric)
        })
    }

    #[test]
    fn counts_round_down_to_two_significant_figures() {
        for (n, text) in [
            (41, "41"),
            (1_234, "1,234"),
            (9_999, "9,999"),
            (12_345, "12 thousand"),
            (1_937_000, "1.9 million"),
            (1_999_999, "1.9 million"),
            (2_300_000_000, "2.3 billion"),
        ] {
            assert_eq!(format_count_floor(n, Lang::En), text, "{n}");
        }
        for (n, text) in [
            (1_937_000, "1,9\u{a0}million"),
            (2_480_000, "2,4\u{a0}millions"),
            (1_284, "1\u{a0}284"),
            (12_345, "12\u{a0}mille"),
            (2_300_000_000, "2,3\u{a0}milliards"),
        ] {
            assert_eq!(format_count_floor(n, Lang::Fr), text, "{n}");
        }
    }

    #[test]
    fn co2_rounds_down_with_its_unit() {
        for (ug, en, fr) in [
            (12_400_000_000, "12 kg", "12\u{a0}kg"),
            (999_999, "0.99 g", "0,99\u{a0}g"),
            (1_500_000_000_000, "1.5 t", "1,5\u{a0}t"),
        ] {
            assert_eq!(format_co2_floor(ug, Lang::En), en, "{ug}");
            assert_eq!(format_co2_floor(ug, Lang::Fr), fr, "{ug}");
        }
    }

    #[test]
    fn durations_round_down() {
        for (ms, text) in [
            (2_349, "2.3 seconds"),
            (640, "0.6 seconds"),
            (500, "0.5 seconds"),
            (100, "0.1 seconds"),
            (1_000, "1 second"),
            (99, "99 milliseconds"),
            (40, "40 milliseconds"),
        ] {
            assert_eq!(format_seconds_floor(ms, Lang::En), text, "{ms}");
        }
        for (ms, text) in [
            (2_349, "2,3\u{a0}secondes"),
            (640, "0,6\u{a0}seconde"),
            (500, "0,5\u{a0}seconde"),
            (1_000, "1\u{a0}seconde"),
            (99, "99\u{a0}millisecondes"),
        ] {
            assert_eq!(format_seconds_floor(ms, Lang::Fr), text, "{ms}");
        }
    }

    #[test]
    fn a_waste_ratio_under_one_permille_reads_under_0_1_percent() {
        assert_eq!(format_permille(0, Lang::En), "under 0.1%");
        assert_eq!(format_permille(45, Lang::En), "4.5%");
        assert_eq!(format_permille(1000, Lang::En), "100%");
        assert_eq!(format_permille(0, Lang::Fr), "moins de 0,1\u{a0}%");
        assert_eq!(format_permille(45, Lang::Fr), "4,5\u{a0}%");
    }

    #[test]
    fn days_and_periods_read_in_words() {
        assert_eq!(format_day("2026-10-01", Lang::Fr), "1er octobre 2026");
        assert_eq!(format_day("2026-08-28", Lang::Fr), "28 août 2026");
        assert_eq!(format_day("2026-08-28", Lang::En), "28 August 2026");
        for (kind, from, en, fr) in [
            (
                PeriodKind::Month,
                "2026-09-01",
                "September 2026",
                "septembre 2026",
            ),
            (PeriodKind::Quarter, "2026-07-01", "Q3 2026", "T3 2026"),
            (
                PeriodKind::Week,
                "2026-10-05",
                "week of 5 October 2026",
                "semaine du 5 octobre 2026",
            ),
            (
                PeriodKind::Semester,
                "2026-01-01",
                "first half of 2026",
                "premier semestre 2026",
            ),
            (PeriodKind::Year, "2026-01-01", "2026", "2026"),
        ] {
            assert_eq!(period_name(kind, from, Lang::En), en);
            assert_eq!(period_name(kind, from, Lang::Fr), fr);
        }
    }

    fn cited_figure(payload: &FullPayload) -> String {
        let Figures::Story(f) = &payload.figures else {
            panic!("a story");
        };
        match (f.cited_ms, f.cited_ops) {
            (Some(ms), _) => format_seconds_floor(ms, payload.lang),
            (None, Some(ops)) => format_count_floor(ops, payload.lang),
            (None, None) => panic!("a story cites a figure"),
        }
    }

    #[test]
    fn every_story_sentence_cites_its_own_figure_and_no_technical_word() {
        for lang in LANGS {
            let label = match lang {
                Lang::En => "Showing a cart",
                Lang::Fr => "Afficher un panier",
            };
            for finding_type in ALL_TYPES {
                let thresholds: &[bool] = if takes_a_threshold(&finding_type) {
                    &[false, true]
                } else {
                    &[false]
                };
                for &threshold in thresholds {
                    for with_label in [false, true] {
                        let mut input = if threshold {
                            story_input_with_threshold(finding_type.clone())
                        } else {
                            story_input(finding_type.clone())
                        };
                        if with_label {
                            input["free"]["label"] = json!(label);
                        }
                        let payload = payload(input, lang);
                        let sentence = story_sentence(&payload);
                        let case = format!("{finding_type:?} {lang:?} {threshold} {with_label}");
                        assert!(
                            sentence.contains(&cited_figure(&payload)),
                            "{case}: {sentence}"
                        );
                        if with_label {
                            assert!(sentence.starts_with(label), "{case}: {sentence}");
                        }
                        for word in TECHNICAL_WORDS {
                            assert!(!has_word(&sentence, word), "{case}: {word} in {sentence}");
                        }
                        for phrase in [
                            "against",
                            "today",
                            "up to",
                            "contre",
                            "aujourd'hui",
                            "jusqu'à",
                        ] {
                            assert!(
                                !sentence.to_lowercase().contains(phrase),
                                "{case}: {phrase} in {sentence}"
                            );
                        }
                        if finding_type == FindingType::ExcessiveFanout {
                            assert!(!has_word(&sentence, "system"), "{case}: {sentence}");
                            assert!(!has_word(&sentence, "système"), "{case}: {sentence}");
                        }
                    }
                }
            }
        }
        let mut input = story_input(FindingType::NPlusOneSql);
        input["free"]["label"] = json!("Showing a cart");
        assert_eq!(
            story_sentence(&payload(input, Lang::En)),
            "Showing a cart: the application fetched information 41 times instead of once."
        );
        let mut input = story_input(FindingType::NPlusOneSql);
        input["free"]["label"] = json!("Afficher un panier");
        assert_eq!(
            story_sentence(&payload(input, Lang::Fr)),
            "Afficher un panier\u{a0}: l'application allait chercher les informations 41 fois au lieu d'une."
        );
    }

    #[test]
    fn the_slow_story_uses_the_prudent_wording() {
        let input = story_input_with_threshold(FindingType::SlowSql);
        assert_eq!(
            story_sentence(&payload(input.clone(), Lang::En)),
            "In August 2026, before the fix, the slowest runs of one step took 2.3 seconds or more. It still runs and no longer triggers the slowness alert, set at 0.5 seconds."
        );
        assert_eq!(
            story_sentence(&payload(input, Lang::Fr)),
            "En août 2026, avant la correction, les passages les plus lents d'une étape prenaient 2,3\u{a0}secondes ou plus. Elle tourne toujours et ne déclenche plus l'alerte de lenteur, réglée à 0,5\u{a0}seconde."
        );
        let plain = payload(story_input(FindingType::SlowSql), Lang::En);
        assert_eq!(
            story_sentence(&plain),
            "For one action in the application, the slowest runs of one step took 2.3 seconds or more."
        );
        assert!(story_fact(&plain).ends_with("Not detected since 28 August 2026."));
        let plain = payload(story_input(FindingType::SlowSql), Lang::Fr);
        assert_eq!(
            story_sentence(&plain),
            "Pour une action de l'application, les passages les plus lents d'une étape prenaient 2,3\u{a0}secondes ou plus."
        );
        let mut labelled = story_input_with_threshold(FindingType::SlowSql);
        labelled["free"]["label"] = json!("Confirming an order");
        assert!(
            story_sentence(&payload(labelled, Lang::En))
                .starts_with("Confirming an order: in August 2026, before the fix, ")
        );
    }

    #[test]
    fn co2_lines_name_what_they_measure() {
        let story = || story_input(FindingType::NPlusOneSql);
        for (input, en, fr) in [
            (
                story(),
                "In July 2026, before the fix: 1.9 million unnecessary operations, at least 12 kg CO2e (estimate)",
                "En juillet 2026, avant la correction\u{a0}: 1,9\u{a0}million d'opérations inutiles, au moins 12\u{a0}kg CO2e (estimation)",
            ),
            (
                curve_input(),
                "Avoidable in September 2026: 2.4 million operations, at least 15 kg CO2e (estimate)",
                "Évitable en septembre 2026\u{a0}: 2,4\u{a0}millions d'opérations, au moins 15\u{a0}kg CO2e (estimation)",
            ),
            (
                collective_input(),
                "In August 2026, before the 4 fixes of work done more than once: 3.1 million unnecessary operations, at least 19 kg CO2e (estimate)",
                "En août 2026, avant les 4 corrections de travail fait plusieurs fois\u{a0}: 3,1\u{a0}millions d'opérations inutiles, au moins 19\u{a0}kg CO2e (estimation)",
            ),
        ] {
            assert_eq!(
                co2_line(&payload(input.clone(), Lang::En)).as_deref(),
                Some(en)
            );
            assert_eq!(co2_line(&payload(input, Lang::Fr)).as_deref(), Some(fr));
            for line in [en, fr] {
                for word in ["saved", "avoided", "₂"] {
                    assert!(!line.contains(word), "{word} in {line}");
                }
            }
        }
        for lang in LANGS {
            let slow = payload(story_input(FindingType::SlowSql), lang);
            assert_eq!(co2_line(&slow), None);
            assert_eq!(equivalence_lines(&slow), None);
        }
    }

    #[test]
    fn the_equivalence_line_prints_its_factor_source_and_vintage() {
        assert_eq!(
            equivalence_lines(&payload(story_input(FindingType::NPlusOneSql), Lang::En)),
            Some((
                "Equal to at least 5 average meals".to_owned(),
                "1 average meal = 2.04 kg CO2e. Source: ADEME, Base Carbone, V23.6 (2025-07-03)"
                    .to_owned()
            ))
        );
        assert_eq!(
            equivalence_lines(&payload(story_input(FindingType::NPlusOneSql), Lang::Fr)),
            Some((
                "L'équivalent d'au moins 5 repas moyens".to_owned(),
                "1 repas moyen = 2,04\u{a0}kg CO2e. Source\u{a0}: ADEME, Base Carbone, V23.6 (2025-07-03)"
                    .to_owned()
            ))
        );
        let count_line = |input: Value| {
            equivalence_lines(&payload(input, Lang::En))
                .expect("an equivalence")
                .0
        };
        assert_eq!(
            count_line(curve_input()),
            "Equal to at least 7 average meals"
        );
        assert_eq!(
            count_line(collective_input()),
            "Equal to at least 9 average meals"
        );
    }

    #[test]
    fn the_story_prints_the_observed_date() {
        let input = story_input(FindingType::NPlusOneSql);
        assert_eq!(
            story_fact(&payload(input.clone(), Lang::En)),
            "Seen on 20 August 2026. Not detected since 28 August 2026."
        );
        assert_eq!(
            story_fact(&payload(input.clone(), Lang::Fr)),
            "Observé le 20 août 2026. N'est plus détecté depuis le 28 août 2026."
        );
        let mut unseen = input;
        unseen["story"]
            .as_object_mut()
            .expect("story")
            .remove("observed_at");
        let fact = story_fact(&payload(unseen, Lang::En));
        assert!(!fact.contains("Seen on"), "{fact}");
        let fact = story_fact(&payload(
            story_input_with_threshold(FindingType::SlowSql),
            Lang::En,
        ));
        assert_eq!(fact, "Seen on 20 August 2026.");
    }

    #[test]
    fn the_collective_line_compares_with_the_period_before() {
        let en = collective_lines(&payload(collective_input(), Lang::En));
        assert!(en.contains(&"7 fixes confirmed by measurement, across 3 services,".to_owned()));
        assert!(en.contains(&"against 5 the month before".to_owned()));
        let fr = collective_lines(&payload(collective_input(), Lang::Fr));
        assert!(fr.contains(&"7 corrections confirmées par la mesure, sur 3 services,".to_owned()));
        assert!(fr.contains(&"contre 5 le mois précédent".to_owned()));
        let mut input = collective_input();
        input["figures"]
            .as_object_mut()
            .expect("figures")
            .remove("previous_fixes_count");
        let lines = collective_lines(&payload(input, Lang::En));
        assert!(
            lines.iter().all(|line| !line.contains("against")),
            "{lines:?}"
        );
        for lang in LANGS {
            for family in ["extra_calls", "slow_calls", "call_architecture"] {
                assert!(!family_label(family, lang).is_empty(), "{family}");
            }
        }
    }

    #[test]
    fn the_curve_compares_with_the_period_before() {
        let en = curve_lines(&payload(curve_input(), Lang::En));
        assert!(en.contains(&"in September 2026, against 4.8% in August 2026".to_owned()));
        assert!(en.contains(&"The lower, the better.".to_owned()));
        let fr = curve_lines(&payload(curve_input(), Lang::Fr));
        assert!(fr.contains(&"en septembre 2026, contre 4,8\u{a0}% en août 2026".to_owned()));
        assert!(fr.contains(&"Plus c'est bas, mieux c'est.".to_owned()));
    }

    #[test]
    fn the_scope_line_names_restarts_and_sampling() {
        let scope = |restarts: u32, permille: u32, lang: Lang| {
            let mut input = story_input(FindingType::NPlusOneSql);
            input["scope"]["restart_days"] = json!(restarts);
            input["scope"]["sampled_permille"] = json!(permille);
            scope_line(&payload(input, lang))
        };
        assert!(scope(2, 1000, Lang::En).contains("2 days with a restart"));
        assert!(scope(1, 1000, Lang::En).contains(", 1 day with a restart"));
        assert!(scope(2, 1000, Lang::Fr).contains("2 jours avec redémarrage"));
        assert!(scope(0, 250, Lang::En).contains("on 25% of traffic"));
        assert!(scope(0, 250, Lang::Fr).contains("sur 25\u{a0}% du trafic"));
        assert!(scope(0, 0, Lang::En).contains("on under 0.1% of traffic"));
        assert!(scope(0, 0, Lang::Fr).contains("sur moins de 0,1\u{a0}% du trafic"));
        for lang in LANGS {
            for line in [scope(2, 250, lang), scope(0, 0, lang)] {
                assert!(
                    !line.contains("(sampled)") && !line.contains("chantillonn"),
                    "{line}"
                );
            }
        }
        assert_eq!(
            scope(0, 1000, Lang::En),
            "Measured on prod-eu, production, 30 days"
        );
        assert_eq!(
            scope(0, 1000, Lang::Fr),
            "Mesuré sur prod-eu, production, 30 jours"
        );
        assert_eq!(
            scope_line(&payload(curve_input(), Lang::En)),
            "Measured on prod-eu, production, 92 days"
        );
        assert_eq!(
            scope_line(&payload(curve_input(), Lang::Fr)),
            "Mesuré sur prod-eu, production, 92 jours"
        );
    }

    #[test]
    fn the_organisation_line_says_it_is_declared() {
        let input = story_input(FindingType::NPlusOneSql);
        assert_eq!(
            org_line(&payload(input.clone(), Lang::En)),
            "Organisation, as declared: Acme"
        );
        assert_eq!(
            org_line(&payload(input, Lang::Fr)),
            "Organisation déclarée\u{a0}: Acme"
        );
    }

    #[test]
    fn equivalence_units_agree_with_their_count() {
        assert_eq!(equivalence_unit("meal", 1, Lang::En), "average meal");
        assert_eq!(equivalence_unit("meal", 5, Lang::En), "average meals");
        assert_eq!(equivalence_unit("meal", 5, Lang::Fr), "repas moyens");
        assert_eq!(equivalence_unit("tgv_km", 5, Lang::Fr), "km en TGV");
        assert_eq!(equivalence_unit("smartphone", 1, Lang::Fr), "smartphone");
        assert_eq!(
            equivalence_unit("car_km", 3, Lang::En),
            "km driven by an average car"
        );
    }

    #[test]
    fn why_it_matters_has_its_two_sentences() {
        assert_eq!(
            why_it_matters(Lang::En),
            [
                "Every unnecessary operation costs server time, energy and sometimes waiting.",
                "Measuring them shows where the application can do the same work with less.",
            ]
        );
        assert_eq!(why_it_matters(Lang::Fr).len(), 2);
    }

    /// The docs quote every story template, placeholders included, so a
    /// reviewed wording cannot change in one place only.
    #[test]
    fn docs_list_every_story_sentence() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs");
        for (lang, relative) in [
            (Lang::En, "SHARE-CARD.md"),
            (Lang::Fr, "FR/SHARE-CARD-FR.md"),
        ] {
            let doc = std::fs::read_to_string(root.join(relative)).expect("read share card docs");
            let w = words(lang);
            assert!(doc.contains(w.lead_none), "{relative}: {}", w.lead_none);
            for finding_type in ALL_TYPES {
                let index = plain_index(&finding_type);
                let plain = w.plain[index];
                assert!(doc.contains(plain), "{relative}: {finding_type:?}: {plain}");
                if let Some(template) = index.checked_sub(2).and_then(|i| w.with_threshold.get(i)) {
                    assert!(
                        doc.contains(template),
                        "{relative}: {finding_type:?}: {template}"
                    );
                }
            }
        }
    }
}
