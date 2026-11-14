//! Shape checks on the hand-written data under `site-data/`, which the
//! technologies page renders. Every failure message names the faulty `id`.

use std::collections::BTreeSet;
use std::path::PathBuf;

use serde_json::{Map, Value, json};

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

// ---- CONTRIBUTING.md -------------------------------------------------

/// The level CONTRIBUTING.md prints for a `difficulty` of the data.
fn contributing_level_name(difficulty: &str) -> Option<&'static str> {
    match difficulty {
        "low" => Some("Low"),
        "medium" => Some("Medium"),
        "high" => Some("High"),
        _ => None,
    }
}

/// The last cell of the table row whose first cell is exactly `row`.
fn contributing_level<'a>(contributing: &'a str, row: &str) -> Option<&'a str> {
    let prefix = format!("| {row}");
    contributing.lines().find_map(|line| {
        let cells = line.strip_prefix(&prefix)?;
        // The first cell must end right after `row`, not run on into a longer title.
        cells
            .trim_start_matches(' ')
            .starts_with('|')
            .then_some(())?;
        line.trim_end()
            .strip_suffix('|')?
            .rsplit('|')
            .next()
            .map(str::trim)
    })
}

fn assert_add_link_level(family: &Value, link: &Value, contributing: &str) {
    let (id, label) = (
        id_of(family),
        link["label"].as_str().unwrap_or("<no label>"),
    );
    let what = format!("family `{id}`, link `{label}`");
    let row = link["contributing_row"]
        .as_str()
        .filter(|r| !r.is_empty())
        .unwrap_or_else(|| panic!("{what}: no `contributing_row`"));
    let difficulty = link["difficulty"].as_str().unwrap_or("<none>");
    let expected = contributing_level_name(difficulty)
        .unwrap_or_else(|| panic!("{what}: unknown difficulty `{difficulty}`"));
    let level = contributing_level(contributing, row).unwrap_or_else(|| {
        panic!("{what}: CONTRIBUTING.md has no table row starting with `| {row}`")
    });
    assert_eq!(
        level, expected,
        "{what}: CONTRIBUTING.md rates `{row}` {level}, the data says {expected}"
    );
}

#[test]
fn add_links_show_the_difficulty_contributing_gives() {
    let doc = load("technologies.json");
    let contributing = std::fs::read_to_string(workspace_path("CONTRIBUTING.md"))
        .expect("failed to read CONTRIBUTING.md");
    for family in families(&doc) {
        for link in family["add"].as_array().into_iter().flatten() {
            assert_add_link_level(family, link, &contributing);
        }
    }
}

// ---- references.json -------------------------------------------------

/// A reference that passes every rule. The tests break one rule at a time.
fn valid_reference() -> Value {
    json!({
        "id": "exemple-energie",
        "name": "Exemple Énergie",
        "domain": "exemple-energie.fr",
        "since": "2026-07",
        "request": "https://github.com/Institut-du-Numerique-Responsable/perf-sentinel/issues/123",
        "proofs": {
            "card": {
                "verify_url": "https://perf-sentinel.dev/verifier#c=AAAA",
                "period_from": "2026-09-01",
                "period_to": "2026-09-30"
            },
            "report": {
                "url": "https://www.exemple-energie.fr/perf-sentinel/rapport.json",
                "period_from": "2026-07-01",
                "period_to": "2026-12-31",
                "environments": ["production"],
                "signed": true
            }
        }
    })
}

/// The valid reference with the value at the JSON pointer `path` replaced.
fn changed(path: &str, value: Value) -> Value {
    let mut r = valid_reference();
    *r.pointer_mut(path)
        .unwrap_or_else(|| panic!("no value at {path}")) = value;
    r
}

/// The check must fail, naming the reference `id` and the rule.
fn assert_rejected(r: &Value, rule: &str) {
    let err = check_reference(r).expect_err("the reference should be rejected");
    assert!(err.contains(id_of(r)), "`{err}` does not name the id");
    assert!(
        err.contains(rule),
        "`{err}` does not name the rule `{rule}`"
    );
}

#[test]
fn references_file_parses_and_may_be_empty() {
    assert_eq!(check_reference(&valid_reference()), Ok(()));
    let doc = load("references.json");
    let refs = doc["references"]
        .as_array()
        .expect("`references` is an array");
    let errors: Vec<String> = refs
        .iter()
        .filter_map(|r| check_reference(r).err())
        .collect();
    assert!(errors.is_empty(), "{errors:#?}");
}

#[test]
fn a_reference_needs_its_required_fields() {
    for key in ["id", "name", "domain", "since", "request", "proofs"] {
        let mut r = valid_reference();
        r.as_object_mut().unwrap().remove(key);
        assert_rejected(&r, &format!("`{key}`"));
    }
}

#[test]
fn a_reference_needs_at_least_one_proof() {
    assert_rejected(&changed("/proofs", json!({})), "`proofs`");
    // `since` follows the one proof left: the card starts in 2026-09.
    for (kept, since) in [("card", "2026-09"), ("report", "2026-07")] {
        let mut r = changed("/since", json!(since));
        r["proofs"] = json!({ kept: valid_reference()["proofs"][kept].clone() });
        assert_eq!(check_reference(&r), Ok(()), "`{kept}` alone is a proof");
    }
}

#[test]
fn since_is_the_month_of_the_earliest_period_from() {
    // The proofs start on 2026-09-01 (card) and 2026-07-01 (report).
    assert_eq!(check_reference(&valid_reference()), Ok(()));
    assert_rejected(&changed("/since", json!("2026-09")), "`since`");
    assert_rejected(&changed("/since", json!("2026-7")), "`since`");
}

#[test]
fn periods_are_ordered_dates() {
    for proof in ["card", "report"] {
        let from = format!("/proofs/{proof}/period_from");
        let to = format!("/proofs/{proof}/period_to");
        assert_rejected(
            &changed(&from, json!("2027-01-01")),
            &from[1..].replace('/', "."),
        );
        assert_rejected(
            &changed(&to, json!("01/10/2026")),
            &to[1..].replace('/', "."),
        );
    }
}

#[test]
fn verify_url_points_at_the_site_verifier() {
    for ok in [
        "https://perf-sentinel.dev/verifier#c=AAAA",
        "https://perf-sentinel.dev/fr/verifier#c=AAAA",
    ] {
        let r = changed("/proofs/card/verify_url", json!(ok));
        assert_eq!(check_reference(&r), Ok(()), "{ok}");
    }
    for bad in [
        "https://perf-sentinel.dev/verifier",
        "http://perf-sentinel.dev/verifier#c=AAAA",
        "https://perf-sentinel.dev.evil.com/verifier#c=AAAA",
        "https://exemple-energie.fr/verifier#c=AAAA",
    ] {
        assert_rejected(
            &changed("/proofs/card/verify_url", json!(bad)),
            "verify_url",
        );
    }
}

#[test]
fn request_is_an_issue_of_this_repository() {
    for bad in [
        "https://github.com/exemple/autre-depot/issues/123",
        "https://github.com/Institut-du-Numerique-Responsable/perf-sentinel/pull/123",
        "https://github.com/Institut-du-Numerique-Responsable/perf-sentinel/issues/",
        "https://github.com/Institut-du-Numerique-Responsable/perf-sentinel/issues/12a",
        "https://github.com/Institut-du-Numerique-Responsable/perf-sentinel/issues/12/",
    ] {
        assert_rejected(&changed("/request", json!(bad)), "`request`");
    }
}

#[test]
fn report_url_is_https_under_the_declared_domain() {
    for ok in [
        "https://exemple-energie.fr/rapport.json",
        "https://exemple-energie.fr",
        "https://www.exemple-energie.fr/a/b?x=1#y",
    ] {
        let r = changed("/proofs/report/url", json!(ok));
        assert_eq!(check_reference(&r), Ok(()), "{ok}");
    }
    for bad in [
        "http://exemple-energie.fr/rapport.json",
        "https://evil-exemple-energie.fr/rapport.json",
        "https://exemple-energie.fr.evil.com/rapport.json",
        "https://exemple-energie.fr@evil.com/rapport.json",
        "https://evil.com@exemple-energie.fr/rapport.json",
        "https://exemple-energie.fr:8443/rapport.json",
        "https://evil.com\\.exemple-energie.fr/rapport.json",
        "https://evil.com/?q=exemple-energie.fr",
        "https://evil.com#exemple-energie.fr",
    ] {
        assert_rejected(
            &changed("/proofs/report/url", json!(bad)),
            "`proofs.report.url`",
        );
    }
}

#[test]
fn a_report_declares_environments_and_signature() {
    assert_rejected(
        &changed("/proofs/report/environments", json!("production")),
        "environments",
    );
    assert_rejected(
        &changed("/proofs/report/environments", json!([1])),
        "environments",
    );
    assert_rejected(&changed("/proofs/report/signed", json!("yes")), "signed");
    for key in ["environments", "signed"] {
        let mut r = valid_reference();
        r["proofs"]["report"].as_object_mut().unwrap().remove(key);
        assert_rejected(&r, key);
    }
}

#[test]
fn a_reference_carries_no_markup() {
    assert_rejected(&changed("/name", json!("Exemple <b>Énergie</b>")), "markup");
    assert_rejected(
        &changed("/proofs/report/environments", json!(["a>b"])),
        "markup",
    );
}

const REQUEST_PREFIX: &str =
    "https://github.com/Institut-du-Numerique-Responsable/perf-sentinel/issues/";

const VERIFY_PREFIXES: [&str; 2] = [
    "https://perf-sentinel.dev/verifier#c=",
    "https://perf-sentinel.dev/fr/verifier#c=",
];

/// A non-empty string field. `prefix` is the dotted path shown in errors.
fn text<'a>(v: &'a Value, prefix: &str, key: &str) -> Result<&'a str, String> {
    v[key]
        .as_str()
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| format!("`{prefix}{key}` is missing or not a non-empty string"))
}

/// The host of an `https://` URL, or `None` when the authority is not a plain
/// ASCII host (no `user@`, no `:port`, no backslash a browser reads as `/`).
fn https_host(url: &str) -> Option<&str> {
    let host = url
        .strip_prefix("https://")?
        .split(['/', '?', '#'])
        .next()?;
    host.bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.')
        .then_some(host)
}

fn check_request(request: &str) -> Result<(), String> {
    let number = request.strip_prefix(REQUEST_PREFIX).unwrap_or("");
    if number.is_empty() || !number.bytes().all(|b| b.is_ascii_digit()) {
        return Err(format!(
            "`request` {request} is not an issue of this repository"
        ));
    }
    Ok(())
}

/// Checks `period_from` and `period_to`, returns `period_from`.
fn check_period<'a>(proof: &'a Value, prefix: &str) -> Result<&'a str, String> {
    let from = text(proof, prefix, "period_from")?;
    let to = text(proof, prefix, "period_to")?;
    for (key, date) in [("period_from", from), ("period_to", to)] {
        if !is_date(date) {
            return Err(format!("`{prefix}{key}` {date} is not a YYYY-MM-DD date"));
        }
    }
    if from > to {
        return Err(format!(
            "`{prefix}period_from` {from} is after `{prefix}period_to` {to}"
        ));
    }
    Ok(from)
}

fn check_card(card: &Value) -> Result<&str, String> {
    let url = text(card, "proofs.card.", "verify_url")?;
    if !VERIFY_PREFIXES.iter().any(|p| url.starts_with(p)) {
        return Err(format!(
            "`proofs.card.verify_url` {url} is not a link to the site verifier"
        ));
    }
    check_period(card, "proofs.card.")
}

fn check_report<'a>(report: &'a Value, domain: &str) -> Result<&'a str, String> {
    let url = text(report, "proofs.report.", "url")?;
    let host = https_host(url).ok_or_else(|| {
        format!("`proofs.report.url` {url} is not an https URL with a plain host")
    })?;
    let under_domain =
        host == domain || host.strip_suffix(domain).is_some_and(|p| p.ends_with('.'));
    if !under_domain {
        return Err(format!(
            "`proofs.report.url` host {host} is not under the domain {domain}"
        ));
    }
    let environments = report["environments"].as_array();
    if !environments.is_some_and(|a| a.iter().all(Value::is_string)) {
        return Err("`proofs.report.environments` is not an array of strings".into());
    }
    if !report["signed"].is_boolean() {
        return Err("`proofs.report.signed` is not a boolean".into());
    }
    check_period(report, "proofs.report.")
}

/// `since` is the month of the earliest `period_from` among the proofs.
fn check_since(since: &str, period_starts: &[&str]) -> Result<(), String> {
    let earliest = period_starts.iter().min().map_or("", |d| &d[..7]);
    if !is_month(since) || since != earliest {
        return Err(format!(
            "`since` {since} must be the YYYY-MM of the earliest period_from ({earliest})"
        ));
    }
    Ok(())
}

fn check_no_markup(r: &Value) -> Result<(), String> {
    let mut offenders = Vec::new();
    markup_offenders(r, "reference", &mut offenders);
    if offenders.is_empty() {
        return Ok(());
    }
    Err(format!("markup in text fields: {offenders:?}"))
}

fn check_reference_rules(r: &Value) -> Result<(), String> {
    for key in ["id", "name"] {
        text(r, "", key)?;
    }
    let (domain, since) = (text(r, "", "domain")?, text(r, "", "since")?);
    check_request(text(r, "", "request")?)?;
    let proofs = r["proofs"]
        .as_object()
        .ok_or("`proofs` is missing or not an object")?;
    let (card, report) = (proofs.get("card"), proofs.get("report"));
    if card.is_none() && report.is_none() {
        return Err("`proofs` needs a `card` or a `report`".into());
    }
    let mut period_starts = Vec::new();
    if let Some(card) = card {
        period_starts.push(check_card(card)?);
    }
    if let Some(report) = report {
        period_starts.push(check_report(report, domain)?);
    }
    check_since(since, &period_starts)?;
    check_no_markup(r)
}

/// Every rule a reference must obey. The error names the reference `id`.
fn check_reference(r: &Value) -> Result<(), String> {
    check_reference_rules(r).map_err(|rule| format!("reference `{}`: {rule}", id_of(r)))
}
