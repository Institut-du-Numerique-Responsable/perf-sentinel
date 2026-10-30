#!/usr/bin/env python3
"""Regenerate crates/sentinel-core/src/score/equivalence_data.rs from Base Carbone.

Reads the ADEME Base Carbone dataset (Licence Ouverte 2.0) through the
data.ademe.fr API: the dataset metadata for the vintage, the lines
endpoint for the factors of the rows listed in ROWS. The rows and their
order are chosen by hand (selection rules in docs/SHARE-CARD.md), the
factors come from the source. Also rewrites the vintage and the table
between the `<!-- equivalences:begin -->` and `<!-- equivalences:end -->`
markers of docs/SHARE-CARD.md and docs/FR/SHARE-CARD-FR.md, nothing
outside them.

Usage:
    python3 scripts/refresh-equivalences.py

Prints an old/new diff table on stdout and errors on stderr. Exits 0
even when values changed (the CI workflow decides via git diff), 1 on
download, parse or acceptance failure. Run `cargo fmt -p
perf-sentinel-core` afterwards (rustfmt owns the layout, and the CI
workflow runs it). Run twice: the second run plus fmt must be a no-op.
"""

import json
import re
import sys
import urllib.parse
import urllib.request
from decimal import Decimal
from pathlib import Path

API = "https://data.ademe.fr/data-fair/api/v1/datasets/base-carboner"

REPO_ROOT = Path(__file__).resolve().parent.parent
TARGET = REPO_ROOT / "crates" / "sentinel-core" / "src" / "score" / "equivalence_data.rs"
DOCS = {
    "en": REPO_ROOT / "docs" / "SHARE-CARD.md",
    "fr": REPO_ROOT / "docs" / "FR" / "SHARE-CARD-FR.md",
}
BEGIN = "<!-- equivalences:begin -->"
END = "<!-- equivalences:end -->"

# (id, Base Carbone id, expected `Unité français`, unit, label), in
# ladder order (ascending factor). Adding a row means adding a line here,
# by hand, after checking the selection rules in docs/SHARE-CARD.md.
ROWS: "list[tuple[str, int, str, str, str]]" = [
    ("tgv_km", 43256, "kgCO2e/passager.km", "passenger_km", "km by high-speed train"),
    ("car_km", 43791, "kgCO2e/km", "vehicle_km", "km driven by an average car"),
    ("meal", 20682, "kgCO2e/repas", "meal", "average meals"),
    ("smartphone", 27013, "kgCO2e/unité", "unit", "smartphones"),
]

# Acceptance rule 2: one everyday countable thing per unit, never per
# euro, per kg or per kWh.
COUNTABLE_UNITS = {
    "passenger_km": "kgCO2e/passager.km",
    "vehicle_km": "kgCO2e/km",
    "meal": "kgCO2e/repas",
    "unit": "kgCO2e/unité",
}

FIELDS = {
    "id": "Identifiant_de_l'élément",
    "type": "Type_Ligne",
    "status": "Statut_de_l'élément",
    "name": "Nom_base_français",
    "attribute": "Nom_attribut_français",
    "unit": "Unité_français",
    "total": "Total_poste_non_décomposé",
    "location": "Localisation_géographique",
    "validity": "Période_de_validité",
}

HEADER = """\
// GENERATED FILE - DO NOT EDIT BY HAND.
// Regenerate with: python3 scripts/refresh-equivalences.py
//
// Everyday equivalences for share cards, in µg CO2e per unit. The rows
// and their order are chosen by hand in the script, the factors come
// from the source.
//
// Source: ADEME, Base Carbone (Licence Ouverte 2.0),
// https://data.ademe.fr/datasets/base-carboner - selection rules in
// docs/SHARE-CARD.md.

use super::equivalence::Equivalence;

/// Grep-audited by release procedure step 2.5, like `PUE_VINTAGE`. The
/// date is the Licence Ouverte "date de dernière mise à jour".
pub(crate) const EQUIVALENCE_TABLE_VINTAGE: &str = "{vintage}";

pub(super) static GENERATED_EQUIVALENCES: &[Equivalence] = &[
"""

DOC_TEXT = {
    "en": {
        "vintage": "Embedded vintage: Base Carbone `{vintage}`.",
        "headers": ("`id`", "Base Carbone id", "Base Carbone element", "Factor",
                    "`per_unit_ug`", "`unit`", "`label`", "Validity"),
    },
    "fr": {
        "vintage": "Millésime embarqué : Base Carbone `{vintage}`.",
        "headers": ("`id`", "Identifiant Base Carbone", "Élément Base Carbone", "Facteur",
                    "`per_unit_ug`", "`unit`", "`label`", "Validité"),
    },
}


class RefreshError(Exception):
    pass


def get_json(url: str) -> dict:
    with urllib.request.urlopen(url, timeout=60) as response:
        return json.load(response)


def fetch_vintage() -> str:
    meta = get_json(API)
    name = meta.get("file", {}).get("name", "")
    version = re.search(r"V\d+\.\d+", name)
    updated = meta.get("dataUpdatedAt", "")[:10]
    if not version or not re.fullmatch(r"\d{4}-\d{2}-\d{2}", updated):
        raise RefreshError(f"cannot read the vintage from {name!r} and {updated!r}")
    return f"{version.group(0)} ({updated})"


def fetch_lines() -> "dict[int, dict]":
    ids = " OR ".join(str(row[1]) for row in ROWS)
    query = urllib.parse.urlencode({
        "qs": f"{FIELDS['id']}:({ids}) AND {FIELDS['type']}:Elément",
        "select": ",".join(FIELDS.values()),
        "size": 100,
    })
    lines: "dict[int, dict]" = {}
    for line in get_json(f"{API}/lines?{query}").get("results", []):
        key = int(line[FIELDS["id"]])
        if key in lines:
            raise RefreshError(f"Base Carbone id {key} has more than one Elément line")
        lines[key] = line
    return lines


def accept(row: tuple, line: "dict | None") -> "tuple[int, str, str, str]":
    """Acceptance rules 1 and 2. Returns (µg, element, factor, validity)."""
    rid, bc_id, unit_fr, unit, _ = row
    if line is None:
        raise RefreshError(f"{rid}: Base Carbone id {bc_id} not found")
    checks = [
        ("type", "Elément"),
        ("status", "Valide générique"),
        ("location", "France continentale"),
        ("unit", unit_fr),
    ]
    for field, expected in checks:
        if line.get(FIELDS[field]) != expected:
            raise RefreshError(f"{rid}: {FIELDS[field]} is {line.get(FIELDS[field])!r}, expected {expected!r}")
    if COUNTABLE_UNITS.get(unit) != unit_fr:
        raise RefreshError(f"{rid}: unit {unit!r} is not an everyday countable thing ({unit_fr})")
    total = Decimal(str(line[FIELDS["total"]]))
    ug = total * 10**9
    if total <= 0 or ug != ug.to_integral_value():
        raise RefreshError(f"{rid}: {total} kgCO2e is not a positive whole number of µg")
    element = ", ".join(p for p in (line.get(FIELDS["name"]), line.get(FIELDS["attribute"])) if p)
    factor = f"{total} {unit_fr}"
    return int(ug), element, factor, str(line.get(FIELDS["validity"], ""))


def parse_existing(text: str) -> "tuple[str | None, dict[str, int]]":
    vintage = re.search(r'EQUIVALENCE_TABLE_VINTAGE: &str = "([^"]+)"', text)
    rows = {
        rid: int(ug.replace("_", ""))
        for rid, ug in re.findall(r'id: "([^"]+)",\s*base_carbone_id: \d+,\s*per_unit_ug: ([0-9_]+)', text)
    }
    return (vintage.group(1) if vintage else None), rows


def render_rust(vintage: str, accepted: list) -> str:
    lines = []
    for (rid, bc_id, _, unit, label), (ug, element, _, _) in zip(ROWS, accepted):
        lines.append(
            f'    Equivalence {{ id: "{rid}", base_carbone_id: {bc_id}, per_unit_ug: {ug:_}, '
            f'unit: "{unit}", label: "{label}" }}, // {element}'
        )
    return HEADER.replace("{vintage}", vintage) + "\n".join(lines) + "\n];\n"


def render_doc_block(lang: str, vintage: str, accepted: list) -> str:
    text = DOC_TEXT[lang]
    headers = text["headers"]
    out = [text["vintage"].format(vintage=vintage), "",
           "| " + " | ".join(headers) + " |",
           "|---|---:|---|---|---:|---|---|---|"]
    for (rid, bc_id, _, unit, label), (ug, element, factor, validity) in zip(ROWS, accepted):
        out.append(f"| `{rid}` | {bc_id} | {element} | {factor} | {ug:_} | `{unit}` | {label} | {validity} |")
    return "\n".join(out)


def rewrite_doc(path: Path, block: str) -> str:
    text = path.read_text(encoding="utf-8")
    start, end = text.find(BEGIN), text.find(END)
    if start < 0 or end < start:
        raise RefreshError(f"{path.relative_to(REPO_ROOT)}: missing {BEGIN} / {END} markers")
    return text[: start + len(BEGIN)] + "\n" + block + "\n" + text[end:]


def main() -> int:
    try:
        vintage = fetch_vintage()
        lines = fetch_lines()
        accepted = [accept(row, lines.get(row[1])) for row in ROWS]
        docs = {path: rewrite_doc(path, render_doc_block(lang, vintage, accepted)) for lang, path in DOCS.items()}
    except (OSError, ValueError, KeyError, RefreshError) as err:
        print(f"error: {err}", file=sys.stderr)
        return 1

    old_vintage, old = parse_existing(TARGET.read_text(encoding="utf-8")) if TARGET.exists() else (None, {})
    print(f"vintage: {old_vintage or '-'} -> {vintage}")
    print(f"{'id':<12} {'old µg':>16} {'new µg':>16}")
    for (rid, *_), (ug, *_) in zip(ROWS, accepted):
        before = old.get(rid)
        print(f"{rid:<12} {before if before is not None else '-':>16} {ug:>16}{'' if before == ug else ' <'}")
    for rid in old:
        if rid not in {row[0] for row in ROWS}:
            print(f"warning: {rid} removed from generated rows", file=sys.stderr)

    TARGET.write_text(render_rust(vintage, accepted), encoding="utf-8", newline="\n")
    print(f"wrote {TARGET.relative_to(REPO_ROOT)}")
    for path, text in docs.items():
        path.write_text(text, encoding="utf-8", newline="\n")
        print(f"wrote {path.relative_to(REPO_ROOT)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
