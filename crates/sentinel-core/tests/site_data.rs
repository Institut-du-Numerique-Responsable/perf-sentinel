//! Shape checks on the hand-written data under `site-data/`, which the
//! technologies page renders. Every failure message names the faulty `id`.

use std::collections::BTreeSet;
use std::path::PathBuf;

use serde_json::{Map, Value};

const FAMILY_IDS: [&str; 8] = [
    "frameworks",
    "databases",
    "brokers",
    "traces",
    "clouds",
    "energy",
    "grids",
    "ci",
];

const ROW_KEYS: [&str; 15] = [
    "id",
    "name",
    "detail",
    "group",
    "aliases",
    "engine",
    "fallback",
    "unsupported",
    "dropped",
    "real_answer",
    "template",
    "note",
    "criteria",
    "lab",
    "production",
];

const REQUIRED_ROW_KEYS: [&str; 5] = ["id", "name", "aliases", "criteria", "lab"];

const FR_FAMILY_KEYS: [&str; 7] = [
    "title",
    "row_header",
    "extra_column",
    "criteria",
    "after",
    "add",
    "groups",
];

const FR_ROW_KEYS: [&str; 3] = ["name", "detail", "note"];

/// The grid criterion "Real answer": true for a recorded operator answer,
/// or for a lab that receives real answers from the service.
const GRID_REAL_ANSWER: usize = 1;

fn workspace_path(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join(rel)
}

fn load(name: &str) -> Value {
    let path = workspace_path(&format!("site-data/{name}"));
    let raw = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()));
    serde_json::from_str(&raw)
        .unwrap_or_else(|e| panic!("{} is not valid JSON: {e}", path.display()))
}

fn id_of(v: &Value) -> &str {
    v["id"].as_str().unwrap_or("<no id>")
}

fn families(doc: &Value) -> &[Value] {
    doc["families"].as_array().expect("`families` is an array")
}

fn rows_of(family: &Value) -> &[Value] {
    family["rows"]
        .as_array()
        .unwrap_or_else(|| panic!("family `{}` has no `rows` array", id_of(family)))
}

/// Every row with the family that holds it.
fn all_rows(doc: &Value) -> Vec<(&Value, &Value)> {
    families(doc)
        .iter()
        .flat_map(|f| rows_of(f).iter().map(move |r| (f, r)))
        .collect()
}

fn criteria_of(row: &Value) -> Vec<bool> {
    row["criteria"]
        .as_array()
        .unwrap_or_else(|| panic!("row `{}`: `criteria` is not an array", id_of(row)))
        .iter()
        .map(|c| {
            c.as_bool()
                .unwrap_or_else(|| panic!("row `{}`: criterion {c} is not a boolean", id_of(row)))
        })
        .collect()
}

fn lab_of(row: &Value) -> &[Value] {
    row["lab"]
        .as_array()
        .unwrap_or_else(|| panic!("row `{}`: `lab` is not an array", id_of(row)))
}

fn flag(row: &Value, key: &str) -> bool {
    match row.get(key) {
        None => false,
        Some(Value::Bool(b)) => *b,
        Some(other) => panic!(
            "row `{}`: `{key}` must be a boolean, got {other}",
            id_of(row)
        ),
    }
}

fn has_text(row: &Value, key: &str) -> bool {
    row[key].as_str().is_some_and(|s| !s.trim().is_empty())
}

/// Digits only, split on `-`, each part as wide as the matching width.
fn date_parts(s: &str, widths: &[usize]) -> Option<Vec<u32>> {
    let parts: Vec<&str> = s.split('-').collect();
    if parts.len() != widths.len() {
        return None;
    }
    parts
        .iter()
        .zip(widths)
        .map(|(p, w)| {
            if p.len() == *w && p.bytes().all(|b| b.is_ascii_digit()) {
                p.parse().ok()
            } else {
                None
            }
        })
        .collect()
}

fn is_month(s: &str) -> bool {
    date_parts(s, &[4, 2]).is_some_and(|p| (1..=12).contains(&p[1]))
}

fn is_date(s: &str) -> bool {
    date_parts(s, &[4, 2, 2]).is_some_and(|p| (1..=12).contains(&p[1]) && (1..=31).contains(&p[2]))
}

fn keys_of(obj: &Map<String, Value>) -> BTreeSet<&str> {
    obj.keys().map(String::as_str).collect()
}

fn assert_keys_within(what: &str, obj: &Map<String, Value>, allowed: &[&str]) {
    let unknown: Vec<&str> = keys_of(obj)
        .into_iter()
        .filter(|k| !allowed.contains(k))
        .collect();
    assert!(unknown.is_empty(), "{what}: unknown keys {unknown:?}");
}

fn assert_same_ids(what: &str, expected: &BTreeSet<&str>, actual: &BTreeSet<&str>) {
    let missing: Vec<_> = expected.difference(actual).collect();
    let extra: Vec<_> = actual.difference(expected).collect();
    assert!(
        missing.is_empty() && extra.is_empty(),
        "{what}: missing {missing:?}, unexpected {extra:?}"
    );
}

#[test]
fn family_ids_are_the_eight_expected_in_order() {
    let doc = load("technologies.json");
    let ids: Vec<&str> = families(&doc).iter().map(id_of).collect();
    assert_eq!(ids, FAMILY_IDS);
}

#[test]
fn row_ids_are_unique_across_families() {
    let doc = load("technologies.json");
    let mut seen = BTreeSet::new();
    for (family, row) in all_rows(&doc) {
        let id = id_of(row);
        assert!(
            seen.insert(id),
            "row id `{id}` (family `{}`) is used twice",
            id_of(family)
        );
    }
}

#[test]
fn every_row_has_one_criterion_per_family_criterion() {
    let doc = load("technologies.json");
    for (family, row) in all_rows(&doc) {
        let expected = family["criteria"]
            .as_array()
            .unwrap_or_else(|| panic!("family `{}`: `criteria` is not an array", id_of(family)))
            .len();
        let got = criteria_of(row).len();
        assert_eq!(
            got,
            expected,
            "row `{}`: {got} criteria, family `{}` declares {expected}",
            id_of(row),
            id_of(family)
        );
    }
}

#[test]
fn level_flags_are_exclusive_and_consistent() {
    let doc = load("technologies.json");
    for (_, row) in all_rows(&doc) {
        let id = id_of(row);
        let (fallback, unsupported) = (flag(row, "fallback"), flag(row, "unsupported"));
        assert!(
            !(fallback && unsupported),
            "row `{id}`: `fallback` and `unsupported` are both true"
        );
        assert!(
            !unsupported || criteria_of(row).iter().all(|c| !c),
            "row `{id}`: `unsupported` needs every criterion false"
        );
    }
}

/// The single level rule: "full" is the only level that needs no note.
fn is_full(row: &Value) -> bool {
    !flag(row, "unsupported") && !flag(row, "fallback") && criteria_of(row).iter().all(|c| *c)
}

#[test]
fn a_note_explains_every_row_that_is_not_full() {
    let doc = load("technologies.json");
    for (_, row) in all_rows(&doc) {
        assert!(
            is_full(row) || has_text(row, "note"),
            "row `{}` is not full and has no note",
            id_of(row)
        );
    }
}

fn assert_lab_matches(family: &Value, row: &Value) {
    let (id, lab) = (id_of(row), lab_of(row));
    assert!(
        lab.iter()
            .all(|p| p.as_str().is_some_and(|s| !s.is_empty())),
        "row `{id}`: `lab` holds a path that is not a non-empty string"
    );
    match family["lab_criterion"].as_u64() {
        None => assert!(
            lab.is_empty(),
            "row `{id}`: family `{}` has no lab criterion, so `lab` must be empty",
            id_of(family)
        ),
        Some(i) => {
            let criteria = criteria_of(row);
            let proven = *criteria
                .get(usize::try_from(i).unwrap())
                .unwrap_or_else(|| panic!("row `{id}`: lab criterion {i} is out of range"));
            assert_eq!(
                proven,
                !lab.is_empty(),
                "row `{id}`: lab criterion is {proven} but `lab` has {} paths",
                lab.len()
            );
        }
    }
}

#[test]
fn lab_list_matches_the_lab_criterion() {
    let doc = load("technologies.json");
    for (family, row) in all_rows(&doc) {
        assert_lab_matches(family, row);
    }
}

fn assert_real_answer(family: &Value, row: &Value) {
    let id = id_of(row);
    let answer = row.get("real_answer").filter(|v| !v.is_null());
    if id_of(family) != "grids" {
        assert!(
            answer.is_none(),
            "row `{id}`: `real_answer` belongs to grids"
        );
        return;
    }
    if let Some(date) = answer {
        assert!(
            date.as_str().is_some_and(is_date),
            "row `{id}`: `real_answer` {date} is not a YYYY-MM-DD date"
        );
    }
    let expected = answer.is_some() || !lab_of(row).is_empty();
    assert_eq!(
        criteria_of(row)[GRID_REAL_ANSWER],
        expected,
        "row `{id}`: the Real answer criterion must equal (`real_answer` set or `lab` not empty)"
    );
}

#[test]
fn real_answer_matches_the_grid_criterion() {
    let doc = load("technologies.json");
    for (family, row) in all_rows(&doc) {
        assert_real_answer(family, row);
    }
}

#[test]
fn production_refers_to_declared_teams() {
    let doc = load("technologies.json");
    let teams = doc["teams"].as_array().expect("`teams` is an array");
    for team in teams {
        assert!(
            team["since"].as_str().is_some_and(is_month),
            "team `{}`: `since` is not a YYYY-MM month",
            id_of(team)
        );
    }
    let declared: BTreeSet<&str> = teams.iter().map(id_of).collect();
    for (_, row) in all_rows(&doc) {
        let Some(production) = row.get("production") else {
            continue;
        };
        for team in production.as_array().expect("`production` is an array") {
            assert!(
                team.as_str().is_some_and(|t| declared.contains(t)),
                "row `{}`: production names the undeclared team {team}",
                id_of(row)
            );
        }
    }
}

#[test]
fn ci_rows_name_an_existing_template() {
    let doc = load("technologies.json");
    for (family, row) in all_rows(&doc) {
        let id = id_of(row);
        if id_of(family) != "ci" {
            assert!(
                row.get("template").is_none(),
                "row `{id}`: `template` belongs to ci"
            );
            continue;
        }
        let template = row["template"]
            .as_str()
            .unwrap_or_else(|| panic!("ci row `{id}` has no `template`"));
        let path = workspace_path(&format!("docs/ci-templates/{template}"));
        assert!(
            path.is_file(),
            "ci row `{id}`: {} does not exist",
            path.display()
        );
    }
}

/// Collects every string holding `<` or `>`, labelled with the nearest `id`
/// or map key above it.
fn markup_offenders(v: &Value, ctx: &str, out: &mut Vec<String>) {
    match v {
        Value::String(s) if s.contains(['<', '>']) => out.push(format!("{ctx}: {s}")),
        Value::Array(items) => items.iter().for_each(|i| markup_offenders(i, ctx, out)),
        Value::Object(map) => {
            let ctx = map.get("id").and_then(Value::as_str).unwrap_or(ctx);
            for (key, child) in map {
                markup_offenders(child, &format!("{ctx}/{key}"), out);
            }
        }
        _ => {}
    }
}

#[test]
fn no_text_field_carries_markup() {
    for name in ["technologies.json", "technologies-fr.json"] {
        let mut offenders = Vec::new();
        markup_offenders(&load(name), name, &mut offenders);
        assert!(offenders.is_empty(), "markup in {name}: {offenders:#?}");
    }
}

#[test]
fn rows_carry_no_unknown_key() {
    let doc = load("technologies.json");
    for (_, row) in all_rows(&doc) {
        let obj = row.as_object().expect("a row is an object");
        let what = format!("row `{}`", id_of(row));
        assert_keys_within(&what, obj, &ROW_KEYS);
        for key in REQUIRED_ROW_KEYS {
            assert!(
                obj.contains_key(key),
                "{what}: missing required key `{key}`"
            );
        }
        assert!(
            row["aliases"].is_array(),
            "{what}: `aliases` is not an array"
        );
    }
}

fn group_ids(family: &Value) -> BTreeSet<&str> {
    family["groups"]
        .as_array()
        .map(|g| g.iter().map(id_of).collect())
        .unwrap_or_default()
}

fn assert_french_family(en: &Value, fr: &Value) {
    let id = id_of(en);
    let what = format!("french family `{id}`");
    assert_keys_within(
        &what,
        fr.as_object().expect("a family is an object"),
        &FR_FAMILY_KEYS,
    );
    assert!(has_text(fr, "title"), "{what}: no `title`");
    for key in ["criteria", "add"] {
        let (n_en, n_fr) = (
            en[key].as_array().map(Vec::len),
            fr[key].as_array().map(Vec::len),
        );
        assert_eq!(
            n_fr, n_en,
            "{what}: `{key}` has {n_fr:?} entries, english has {n_en:?}"
        );
    }
    let fr_groups: BTreeSet<&str> = fr["groups"]
        .as_object()
        .map(|g| g.keys().map(String::as_str).collect())
        .unwrap_or_default();
    assert_same_ids(&format!("{what} groups"), &group_ids(en), &fr_groups);
}

fn assert_french_rows(en: &Value, fr: &Value) {
    let ids: BTreeSet<&str> = all_rows(en).iter().map(|(_, r)| id_of(r)).collect();
    let fr_rows = fr["rows"].as_object().expect("french `rows` is an object");
    assert_same_ids("french rows", &ids, &keys_of(fr_rows));
    for (id, row) in fr_rows {
        let obj = row.as_object().expect("a french row is an object");
        assert_keys_within(&format!("french row `{id}`"), obj, &FR_ROW_KEYS);
    }
}

#[test]
fn french_file_covers_every_id_and_nothing_else() {
    let (en, fr) = (load("technologies.json"), load("technologies-fr.json"));
    let fr_families = fr["families"]
        .as_object()
        .expect("french `families` is an object");
    let en_ids: BTreeSet<&str> = families(&en).iter().map(id_of).collect();
    assert_same_ids("french families", &en_ids, &keys_of(fr_families));
    for family in families(&en) {
        assert_french_family(family, &fr_families[id_of(family)]);
    }
    assert_french_rows(&en, &fr);
}
