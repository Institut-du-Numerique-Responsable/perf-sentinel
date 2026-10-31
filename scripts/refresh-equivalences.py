#!/usr/bin/env python3
"""Regenerate crates/sentinel-core/src/score/equivalence_data.rs from three sources.

- ADEME Base Carbone (France, Licence Ouverte 2.0), read through the
  data.ademe.fr API: the dataset metadata for the vintage, the lines
  endpoint for the factors.
- DESNZ GHG conversion factors (UK, Open Government Licence v3.0), the
  flat-format XLSX at DESNZ_URL. A row is the sum of a tailpipe and a
  well-to-tank factor.
- US EPA GHG Equivalencies Calculator (US federal public domain), the
  "miles driven" factor read from the page at EPA_URL.

The rows and their order are chosen by hand in ROWS (selection rules in
docs/SHARE-CARD.md), the factors come from the sources. DESNZ_URL,
DESNZ_VINTAGE, EPA_URL, EPA_VINTAGE and EPA_SECTION are set by hand too:
change them when DESNZ publishes a new edition (the media URL changes
with each upload) or the EPA revises its calculator, then check that
every ID still exists. Also rewrites the vintages and the table between
the `<!-- equivalences:begin -->` and `<!-- equivalences:end -->` markers
of docs/SHARE-CARD.md and docs/FR/SHARE-CARD-FR.md, nothing outside them.

Usage:
    python3 scripts/refresh-equivalences.py
    python3 scripts/refresh-equivalences.py --self-test   # offline parser checks

Prints an old/new diff table on stdout and errors on stderr. Exits 0
even when values changed (the CI workflow decides via git diff), 1 on
download, parse or acceptance failure. Run `cargo fmt -p
perf-sentinel-core` afterwards (rustfmt owns the layout, and the CI
workflow runs it). Run twice: the second run plus fmt must be a no-op.
"""

import html
import io
import json
import re
import sys
import urllib.parse
import urllib.request
import zipfile
from decimal import Decimal
from pathlib import Path
from xml.etree import ElementTree

API = "https://data.ademe.fr/data-fair/api/v1/datasets/base-carboner"
DESNZ_URL = "https://assets.publishing.service.gov.uk/media/6a6c9748862aaf18d9c62ac9/ghg-conversion-factors-2026-flat-format-revised.xlsx"
DESNZ_VINTAGE = "2026, v1.2"
EPA_URL = "https://www.epa.gov/energy/greenhouse-gas-equivalencies-calculator-calculations-and-references"
EPA_VINTAGE = "2024-10"
EPA_SECTION = "Miles driven by the average gasoline-powered passenger vehicle"

REPO_ROOT = Path(__file__).resolve().parent.parent
TARGET = REPO_ROOT / "crates" / "sentinel-core" / "src" / "score" / "equivalence_data.rs"
DOCS = {
    "en": REPO_ROOT / "docs" / "SHARE-CARD.md",
    "fr": REPO_ROOT / "docs" / "FR" / "SHARE-CARD-FR.md",
}
BEGIN = "<!-- equivalences:begin -->"
END = "<!-- equivalences:end -->"

SRC_ADEME = "ADEME, Base Carbone"
SRC_DESNZ = "DESNZ, GHG conversion factors"
SRC_EPA = "US EPA, GHG Equivalencies Calculator"

COUNTRIES = ("fr", "gb", "us")
# Largest allowed jump between two neighbouring auto rows of one country,
# so a CO2 figure always has a row that gives a readable count.
MAX_LADDER_RATIO = 200

# (id, countries, auto, unit, label, source), where source is
# ("ademe", Base Carbone id, expected `Unité français`),
# ("desnz", (tailpipe ID, well-to-tank ID), expected UOM) or
# ("epa", expected unit). In ladder order (ascending factor). Adding a
# row means adding a line here, by hand, after checking the selection
# rules in docs/SHARE-CARD.md.
ROWS: "list[tuple]" = [
    ("tgv_km", ("fr",), False, "passenger_km", "km by high-speed train in France",
     ("ademe", 43256, "kgCO2e/passager.km")),
    ("gb_rail_km", ("gb",), True, "passenger_km", "km by train in the UK",
     ("desnz", ("25_315_3147_11_1", "26_911_3147_11_1"), "passenger.km")),
    ("gb_car_km", ("gb",), True, "vehicle_km", "km driven by an average car in the UK",
     ("desnz", ("4_301_3074_4_1", "26_904_3074_4_1"), "km")),
    ("car_km", ("fr",), True, "vehicle_km", "km driven by an average car in France",
     ("ademe", 43791, "kgCO2e/km")),
    ("us_car_mile", ("us",), True, "vehicle_mile", "miles driven by an average US car, tailpipe only",
     ("epa", "mile")),
    ("meal", ("fr",), True, "meal", "average meals in France",
     ("ademe", 20682, "kgCO2e/repas")),
    ("smartphone", ("fr", "gb", "us"), True, "unit", "smartphones",
     ("ademe", 27013, "kgCO2e/unité")),
]

# Acceptance rule 2: one everyday countable thing per unit, never per
# euro, per kg or per kWh. Maps each unit to the source units it may use.
COUNTABLE_UNITS = {
    "passenger_km": {"kgCO2e/passager.km", "passenger.km"},
    "vehicle_km": {"kgCO2e/km", "km"},
    "vehicle_mile": {"mile"},
    "meal": {"kgCO2e/repas"},
    "unit": {"kgCO2e/unité"},
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
// from the sources.
//
// Sources, selection rules in docs/SHARE-CARD.md:
// - ADEME, Base Carbone (Licence Ouverte 2.0),
//   https://data.ademe.fr/datasets/base-carboner
// - DESNZ, GHG conversion factors for company reporting (Open
//   Government Licence v3.0),
//   https://www.gov.uk/government/collections/government-conversion-factors-for-company-reporting
// - US EPA, GHG Equivalencies Calculator (US federal public domain),
//   {epa_url}

use super::equivalence::Equivalence;
use super::equivalence::EquivalenceCountry::{{Fr, Gb, Us}};

/// Grep-audited by release procedure step 2.5, like `PUE_VINTAGE`. The
/// date is the Licence Ouverte "date de dernière mise à jour".
pub(crate) const BASE_CARBONE_VINTAGE: &str = "{ademe}";
/// The DESNZ publication year and version.
pub(crate) const DESNZ_VINTAGE: &str = "{desnz}";
/// The EPA calculator revision, year and month.
pub(crate) const EPA_VINTAGE: &str = "{epa}";

pub(super) static GENERATED_EQUIVALENCES: &[Equivalence] = &[
"""

VINTAGE_CONSTS = {"ademe": "BASE_CARBONE_VINTAGE", "desnz": "DESNZ_VINTAGE", "epa": "EPA_VINTAGE"}

DOC_TEXT = {
    "en": {
        "vintage": "Embedded vintages: Base Carbone `{ademe}`, DESNZ `{desnz}`, US EPA `{epa}`.",
        "headers": ("`id`", "Countries", "Auto", "Source", "Source reference", "Factor",
                    "`per_unit_ug`", "`unit`", "`label`", "Validity"),
        "auto": ("yes", "no"),
    },
    "fr": {
        "vintage": "Millésimes embarqués : Base Carbone `{ademe}`, DESNZ `{desnz}`, US EPA `{epa}`.",
        "headers": ("`id`", "Pays", "Auto", "Source", "Référence de la source", "Facteur",
                    "`per_unit_ug`", "`unit`", "`label`", "Validité"),
        "auto": ("oui", "non"),
    },
}


class RefreshError(Exception):
    pass


def fetch(url: str) -> bytes:
    # A browser-like agent: some of these hosts refuse the urllib default.
    request = urllib.request.Request(url, headers={"User-Agent": "Mozilla/5.0 perf-sentinel-refresh"})
    with urllib.request.urlopen(request, timeout=120) as response:
        return response.read()


def get_json(url: str) -> dict:
    return json.loads(fetch(url))


def whole_ug(rid: str, value: Decimal) -> int:
    """`value` is already in µg. Must be a positive whole number."""
    if value <= 0 or value != value.to_integral_value():
        raise RefreshError(f"{rid}: {value} µg is not a positive whole number")
    return int(value)


def fetch_vintage() -> str:
    meta = get_json(API)
    name = meta.get("file", {}).get("name", "")
    version = re.search(r"V\d+\.\d+", name)
    updated = meta.get("dataUpdatedAt", "")[:10]
    if not version or not re.fullmatch(r"\d{4}-\d{2}-\d{2}", updated):
        raise RefreshError(f"cannot read the vintage from {name!r} and {updated!r}")
    return f"{version.group(0)} ({updated})"


def fetch_lines() -> "dict[int, dict]":
    ids = " OR ".join(str(row[5][1]) for row in ROWS if row[5][0] == "ademe")
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


def accept_ademe(row: tuple, line: "dict | None") -> dict:
    """Acceptance rules 1 and 2 for a Base Carbone row."""
    rid, *_, (_, bc_id, unit_fr) = row
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
    total = Decimal(str(line[FIELDS["total"]]))
    element = ", ".join(p for p in (line.get(FIELDS["name"]), line.get(FIELDS["attribute"])) if p)
    return {
        "ug": whole_ug(rid, total * 10**9), "source": SRC_ADEME, "ref": str(bc_id),
        "factor": f"{total} {unit_fr}", "validity": str(line.get(FIELDS["validity"], "")),
        "comment": element,
    }


def column(cell_ref: str) -> str:
    return re.match(r"[A-Z]+", cell_ref).group(0)


def sheet_rows(archive: zipfile.ZipFile, name: str, strings: "list[str]"):
    """Yield each row of a worksheet as {column letter: text}."""
    ns = "{http://schemas.openxmlformats.org/spreadsheetml/2006/main}"
    for row in ElementTree.fromstring(archive.read(name)).iter(f"{ns}row"):
        cells = {}
        for cell in row.iter(f"{ns}c"):
            value = cell.find(f"{ns}v")
            if cell.get("t") == "inlineStr":
                cells[column(cell.get("r"))] = "".join(t.text or "" for t in cell.iter(f"{ns}t"))
            elif value is not None and value.text is not None:
                cells[column(cell.get("r"))] = strings[int(value.text)] if cell.get("t") == "s" else value.text
        yield cells


def desnz_lines(data: bytes, wanted: "set[str]") -> "tuple[str, dict[str, list[tuple]]]":
    """Read the DESNZ flat file. Returns (factor year, {ID: [(UOM, GHG/Unit, kg per UOM, name)]})."""
    ns = "{http://schemas.openxmlformats.org/spreadsheetml/2006/main}"
    archive = zipfile.ZipFile(io.BytesIO(data))
    strings = ["".join(t.text or "" for t in si.iter(f"{ns}t"))
               for si in ElementTree.fromstring(archive.read("xl/sharedStrings.xml")).iter(f"{ns}si")]
    # The real file opens with a front page: use the first sheet that has the header row.
    sheets = sorted((n for n in archive.namelist() if re.fullmatch(r"xl/worksheets/sheet\d+\.xml", n)),
                    key=lambda n: int(re.search(r"\d+", n.rsplit("/", 1)[1]).group(0)))
    for sheet in sheets:
        header = None
        found: "dict[str, list[tuple]]" = {}
        for cells in sheet_rows(archive, sheet, strings):
            if header is None:
                by_name = {v: k for k, v in cells.items()}
                factor_col = next((k for k, v in cells.items() if v.startswith("GHG Conversion Factor")), None)
                if {"ID", "UOM", "GHG/Unit"} <= by_name.keys() and factor_col:
                    header = (by_name["ID"], by_name["UOM"], by_name["GHG/Unit"], factor_col,
                              by_name.get("Level 3"), by_name.get("Column Text"))
                    year = cells[factor_col].removeprefix("GHG Conversion Factor").strip()
                continue
            rid = cells.get(header[0])
            if rid not in wanted:
                continue
            try:
                # The shortest round-trip repr drops the float noise of the file (0.0089700000000000005).
                factor = Decimal(repr(float(cells[header[3]])))
            except (KeyError, ValueError):
                raise RefreshError(f"DESNZ {rid}: no numeric factor ({cells.get(header[3])!r})")
            name = ", ".join(cells[c] for c in header[4:] if c and cells.get(c))
            found.setdefault(rid, []).append((cells.get(header[1]), cells.get(header[2]), factor, name))
        if header is not None:
            return year, found
    raise RefreshError("DESNZ file: no worksheet with an ID / UOM / GHG/Unit header row")


def accept_desnz(row: tuple, year: str, found: dict) -> dict:
    rid, *_, (_, ids, uom) = row
    factors, names = [], []
    for desnz_id in ids:
        lines = found.get(desnz_id, [])
        if len(lines) != 1:
            raise RefreshError(f"{rid}: DESNZ ID {desnz_id} found {len(lines)} times, expected once")
        line_uom, ghg_unit, factor, name = lines[0]
        if line_uom != uom or ghg_unit != "kg CO2e":
            raise RefreshError(f"{rid}: DESNZ {desnz_id} is {ghg_unit!r} per {line_uom!r}, expected 'kg CO2e' per {uom!r}")
        factors.append(factor)
        names.append(name)
    if year != DESNZ_VINTAGE.split(",")[0]:
        raise RefreshError(f"{rid}: DESNZ factor column is for {year!r}, DESNZ_VINTAGE says {DESNZ_VINTAGE!r}")
    return {
        "ug": whole_ug(rid, sum(factors) * 10**9), "source": SRC_DESNZ, "ref": " + ".join(ids),
        "factor": " + ".join(format(f, "f") for f in factors) + f" kgCO2e/{uom}",
        "validity": year, "comment": f"{names[0]}, tailpipe + WTT",
    }


def epa_factor(page: str) -> "tuple[str, str]":
    """The (mantissa, exponent) of the EPA factor, in metric tons CO2e per mile, inside EPA_SECTION."""
    headings = list(re.finditer(rf"<h([1-6])[^>]*>\s*{re.escape(EPA_SECTION)}\s*</h\1>", page, re.I))
    if len(headings) != 1:
        raise RefreshError(f"EPA: section {EPA_SECTION!r} found {len(headings)} times, expected once")
    rest = page[headings[0].end():]
    # The section ends at the next heading of the same or a higher level.
    end = re.search(rf"<h[1-{headings[0].group(1)}][\s>]", rest)
    text = html.unescape(re.sub(r"<[^>]+>", "", rest[: end.start()] if end else rest))
    hits = re.findall(r"(\d+(?:\.\d+)?)\s*[x×]\s*10\s*[-−–]\s*(\d+)\s*metric tons CO2E/mile", text, re.I)
    if len(hits) != 1:
        raise RefreshError(f"EPA: the per-mile factor found {len(hits)} times in its section, expected once")
    return hits[0]


def accept_epa(row: tuple, page: str) -> dict:
    mantissa, exponent = epa_factor(page)
    return {
        "ug": whole_ug(row[0], Decimal(mantissa) * Decimal(10) ** (12 - int(exponent))), "source": SRC_EPA,
        "ref": EPA_SECTION, "factor": f"{mantissa} x 10-{exponent} metric tons CO2e/mile",
        "validity": EPA_VINTAGE, "comment": "EPA calculator",
    }


def check_ladders(ugs: "dict[str, list[int]]") -> None:
    """Acceptance rule 3: no jump over MAX_LADDER_RATIO between neighbouring auto rows of a country."""
    for country, values in ugs.items():
        values = sorted(values)
        for low, high in zip(values, values[1:]):
            if high > MAX_LADDER_RATIO * low:
                raise RefreshError(f"{country}: auto rows {low} and {high} µg are more than {MAX_LADDER_RATIO}x apart")


def accept_all(lines: dict, desnz: bytes, page: str) -> "list[dict]":
    year, found = desnz_lines(desnz, {i for row in ROWS if row[5][0] == "desnz" for i in row[5][1]})
    accepted = []
    for row in ROWS:
        rid, countries, _, unit, _, source = row
        if not set(countries) <= set(COUNTRIES):
            raise RefreshError(f"{rid}: unknown country in {countries}")
        if source[-1] not in COUNTABLE_UNITS.get(unit, ()):
            raise RefreshError(f"{rid}: unit {unit!r} is not an everyday countable thing ({source[-1]})")
        if source[0] == "ademe":
            accepted.append(accept_ademe(row, lines.get(source[1])))
        elif source[0] == "desnz":
            accepted.append(accept_desnz(row, year, found))
        else:
            accepted.append(accept_epa(row, page))
    check_ladders({c: [a["ug"] for row, a in zip(ROWS, accepted) if row[2] and c in row[1]] for c in COUNTRIES})
    return accepted


def parse_existing(text: str) -> "tuple[dict[str, str], dict[str, int]]":
    vintages = dict(re.findall(r'(\w+_VINTAGE): &str = "([^"]+)"', text))
    rows = {
        rid: int(ug.replace("_", ""))
        for rid, ug in re.findall(r'\bid: "([^"]+)",.*?per_unit_ug: ([0-9_]+)', text, re.S)
    }
    return vintages, rows


def render_rust(vintages: "dict[str, str]", accepted: list) -> str:
    out = []
    for (rid, countries, auto, unit, label, source), a in zip(ROWS, accepted):
        variants = ", ".join(c.capitalize() for c in countries)
        out.append(
            f'    Equivalence {{ id: "{rid}", countries: &[{variants}], auto: {str(auto).lower()}, '
            f'source: "{a["source"]}", source_ref: "{a["ref"]}", vintage: {VINTAGE_CONSTS[source[0]]}, '
            f'per_unit_ug: {a["ug"]:_}, unit: "{unit}", label: "{label}" }}, // {a["comment"]}'
        )
    return HEADER.format(epa_url=EPA_URL, **vintages) + "\n".join(out) + "\n];\n"


def render_doc_block(lang: str, vintages: "dict[str, str]", accepted: list) -> str:
    text = DOC_TEXT[lang]
    out = [text["vintage"].format(**vintages), "",
           "| " + " | ".join(text["headers"]) + " |",
           "|---|---|---|---|---|---|---:|---|---|---|"]
    for (rid, countries, auto, unit, label, _), a in zip(ROWS, accepted):
        out.append(
            f"| `{rid}` | {', '.join(countries)} | {text['auto'][not auto]} | {a['source']} | {a['ref']} | "
            f"{a['factor']} | {a['ug']:_} | `{unit}` | {label} | {a['validity']} |"
        )
    return "\n".join(out)


def rewrite_doc(path: Path, block: str) -> str:
    text = path.read_text(encoding="utf-8")
    start, end = text.find(BEGIN), text.find(END)
    if start < 0 or end < start:
        raise RefreshError(f"{path.relative_to(REPO_ROOT)}: missing {BEGIN} / {END} markers")
    return text[: start + len(BEGIN)] + "\n" + block + "\n" + text[end:]


def main() -> int:
    try:
        vintages = {"ademe": fetch_vintage(), "desnz": DESNZ_VINTAGE, "epa": EPA_VINTAGE}
        accepted = accept_all(fetch_lines(), fetch(DESNZ_URL), fetch(EPA_URL).decode("utf-8"))
        docs = {path: rewrite_doc(path, render_doc_block(lang, vintages, accepted)) for lang, path in DOCS.items()}
    except (OSError, ValueError, KeyError, zipfile.BadZipFile, ElementTree.ParseError, RefreshError) as err:
        print(f"error: {err}", file=sys.stderr)
        return 1

    old_vintages, old = parse_existing(TARGET.read_text(encoding="utf-8")) if TARGET.exists() else ({}, {})
    for key, const in VINTAGE_CONSTS.items():
        print(f"vintage {key}: {old_vintages.get(const, '-')} -> {vintages[key]}")
    print(f"{'id':<12} {'old µg':>16} {'new µg':>16}")
    for (rid, *_), a in zip(ROWS, accepted):
        before = old.get(rid)
        print(f"{rid:<12} {before if before is not None else '-':>16} {a['ug']:>16}{'' if before == a['ug'] else ' <'}")
    for rid in old:
        if rid not in {row[0] for row in ROWS}:
            print(f"warning: {rid} removed from generated rows", file=sys.stderr)

    TARGET.write_text(render_rust(vintages, accepted), encoding="utf-8", newline="\n")
    print(f"wrote {TARGET.relative_to(REPO_ROOT)}")
    for path, text in docs.items():
        path.write_text(text, encoding="utf-8", newline="\n")
        print(f"wrote {path.relative_to(REPO_ROOT)}")
    return 0


def expect_error(call) -> None:
    try:
        call()
    except RefreshError:
        return
    raise AssertionError("expected a RefreshError")


def tiny_xlsx(rows: "list[list[str]]") -> bytes:
    """A minimal workbook: one sheet, every cell an inline string."""
    ns = "http://schemas.openxmlformats.org/spreadsheetml/2006/main"
    cells = "".join(
        "<row>" + "".join(
            f'<c r="{chr(65 + col)}{n}" t="inlineStr"><is><t>{value}</t></is></c>' for col, value in enumerate(row)
        ) + "</row>"
        for n, row in enumerate(rows, 1)
    )
    buffer = io.BytesIO()
    with zipfile.ZipFile(buffer, "w") as archive:
        archive.writestr("xl/sharedStrings.xml", f'<sst xmlns="{ns}"/>')
        archive.writestr("xl/worksheets/sheet1.xml", f'<worksheet xmlns="{ns}"><sheetData>{cells}</sheetData></worksheet>')
    return buffer.getvalue()


def self_test() -> None:
    rows = [["ID", "UOM", "GHG/Unit", "GHG Conversion Factor 2026"]] + [[i, u, "kg CO2e", f] for i, u, f in [
        ("25_315_3147_11_1", "passenger.km", "0.03092"),
        ("26_911_3147_11_1", "passenger.km", "8.9700000000000005E-3"),
        ("4_301_3074_4_1", "km", "0.16591"),
        ("26_904_3074_4_1", "km", "4.3990000000000001E-2"),
    ]]
    desnz = [r for r in ROWS if r[5][0] == "desnz"]
    year, found = desnz_lines(tiny_xlsx(rows), {i for r in desnz for i in r[5][1]})
    assert [accept_desnz(r, year, found)["ug"] for r in desnz] == [39_890_000, 209_900_000]
    # A changed UOM, a missing ID and a duplicated ID are refused.
    expect_error(lambda: accept_desnz(desnz[0], year, {**found, "25_315_3147_11_1": [("km", "kg CO2e", Decimal(1), "")]}))
    expect_error(lambda: accept_desnz(desnz[0], year, {k: v for k, v in found.items() if k != "25_315_3147_11_1"}))
    expect_error(lambda: accept_desnz(desnz[0], year, {**found, "25_315_3147_11_1": found["25_315_3147_11_1"] * 2}))

    section = f"<h3>{EPA_SECTION}</h3><p>{{}}</p><h3>Next</h3>"
    factor = "3.93 x 10<sup>-4</sup> metric tons CO2E/mile"
    assert accept_epa(("us_car_mile",), section.format(factor))["ug"] == 393_000_000
    expect_error(lambda: epa_factor(section.format(factor + " and " + factor)))
    expect_error(lambda: epa_factor(section.format("no factor")))

    check_ladders({"gb": [39_890_000, 209_900_000, 39_100_000_000]})
    expect_error(lambda: check_ladders({"gb": [1, 201]}))
    print("self-test ok")


if __name__ == "__main__":
    if sys.argv[1:] == ["--self-test"]:
        self_test()
        sys.exit(0)
    sys.exit(main())
