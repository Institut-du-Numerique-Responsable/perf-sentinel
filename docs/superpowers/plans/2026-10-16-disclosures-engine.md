# Disclosures in the Hub, engine side, Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give the engine a `disclose-pdf` subcommand that renders a signed periodic report as a PDF/A-3b carrying its source files, and let `verify-hash` verify such a PDF, so the Hub (0.7.0) can publish disclosures.

**Architecture:** Rendering and extraction live in `sentinel-core` under a new `pdf` feature (`report/periodic/pdf/`), the CLI gains `disclose-pdf` and a PDF branch in `verify-hash`, both behind a CLI `pdf` feature that is on by default. A schema-version gate shared by both commands turns "newer than this engine" into exit 5 instead of a false hash failure.

**Tech Stack:** Rust 2024, MSRV 1.99.0, krilla 0.8.2 (PDF/A writer, MIT OR Apache-2.0), fast_qr 0.14.0, lopdf 0.45.0 (reader, MIT), DM Sans and JetBrains Mono static TTFs (SIL OFL 1.1).

**Spec:** `docs/superpowers/specs/2026-10-16-disclosures-in-hub-design.md`

## Global Constraints

- Core stays `#![forbid(unsafe_code)]`. All logic in `sentinel-core`, the CLI only parses and dispatches.
- English only in code, comments, errors and CLI output. Docs ship with FR mirrors in the same commit.
- `disclose-pdf` exit codes, a contract the Hub relies on: 0 rendered, 1 refused, 3 input error, 5 cannot render. Exactly one JSON line on stdout: `{"result":"rendered"}`, `{"result":"refused","reason":R}`, `{"result":"input_error","reason":R}`, `{"result":"unrenderable","reason":R}`.
- Reasons, a closed set: `content_hash_mismatch`, `binding_mismatch`, `malformed_report`, `missing_file`, `unsupported_schema_version`, `unsupported_characters`.
- Accepted schema versions: `perf-sentinel-report/v1.0` to `perf-sentinel-report/v1.7`, read from the raw JSON before deserialising.
- PDF: PDF/A-3b, A4, English, deterministic (dates from `report_metadata.generated_at`, never the clock). `report.json` attached with relationship `Source`, attestation and bundle with `Supplement`, byte for byte as given. File names inside the PDF: `report.json`, `attestation.intoto.jsonl`, `bundle.sig`.
- QR code only for `notes.reference_urls["transparency"]`, nothing otherwise, never a Hub or share URL.
- The document code is the full `content_hash`, grouped by four characters, on pages 1 and 5.
- No glyph substitution, no fallback font: text outside the bundled fonts exits 5 `unsupported_characters` and lists the characters on stderr.
- `verify-hash` on a PDF accepts any well-formed PDF (re-saved copies included). The PDF goes through the existing 64 MiB capped read. Each attachment is capped at `MAX_LOCAL_REPORT_BYTES` (report) and `MAX_ATTESTATION_BYTES` (attestation, bundle).
- Exit 5 is new for `verify-hash` too (`EXIT_UNSUPPORTED`), same reason name.
- `cargo clippy --workspace --all-targets -- -D warnings`, `cargo clippy -p perf-sentinel-core --features daemon,pdf --all-targets -- -D warnings`, `cargo check --workspace --no-default-features` and `cargo check -p perf-sentinel-core --no-default-features --all-targets` stay clean. Sonar cognitive complexity 15 per function.
- Commits: signed, subject line only, Conventional Commits, no process words. `cargo fmt --all` before each Rust commit.

## Review Focus

1. A copy of the PDF re-saved by a viewer (object streams, xref streams, recompressed attachments) must still verify through `verify-hash --report`. Pinned in Task 5 by re-saving a rendered PDF through lopdf with compression before extracting.
2. An organisation name or disclaimer with characters outside Latin (Cyrillic, CJK, emoji) must exit 5, never write a PDF with `.notdef`. Pinned in Task 3.
3. A v1.0 report (no canonical tiers, no carbon breakdown, no trace chain) must render with "not reported" sections, not panic. Pinned in Task 3.
4. A hostile PDF (deep nesting, a huge declared stream length, an attachment named `report.json` twice) must fail as an input error under the caps, never allocate past them. Pinned in Task 5 and the fuzz target.
5. Two renders of the same inputs on different days give the same bytes. Pinned in Task 2.

## File Structure

- `crates/sentinel-core/src/report/periodic/version.rs` (new): the schema-version gate.
- `crates/sentinel-core/src/report/periodic/pdf/mod.rs` (new): public API, `render_disclosure_pdf`, `extract_attachments`, error types.
- `crates/sentinel-core/src/report/periodic/pdf/layout.rs` (new): line splitter and page flow over font advances.
- `crates/sentinel-core/src/report/periodic/pdf/pages.rs` (new): the five pages.
- `crates/sentinel-core/src/report/periodic/pdf/extract.rs` (new): attachment extraction with lopdf.
- `crates/sentinel-core/src/report/periodic/pdf/fonts/` (new): `DMSans-Regular.ttf`, `DMSans-SemiBold.ttf`, `JetBrainsMono-Regular.ttf`, `OFL.txt` (licence plus the SHA-256 of each TTF and its upstream release URL).
- `crates/sentinel-core/src/report/periodic/pdf/tests.rs` (new).
- `crates/sentinel-cli/src/disclose_pdf.rs` (new): the subcommand runner.
- `crates/sentinel-cli/src/verify_hash.rs`, `cli.rs`, `main.rs`, both `Cargo.toml`, `fuzz/`, `.github/workflows/`, docs.

---

### Task 1: Schema-version gate

**Files:**
- Create: `crates/sentinel-core/src/report/periodic/version.rs`, declared in `report/periodic/mod.rs`
- Modify: `crates/sentinel-cli/src/verify_hash.rs` (new exit code, gate before deserialising, local and `--url` paths)
- Test: unit tests in `version.rs`, `crates/sentinel-cli/tests/verify_hash_e2e.rs`

**Interfaces:**
- Produces: `pub const SUPPORTED_SCHEMA_VERSIONS: &[&str]` (v1.0 to v1.7, `schema::SCHEMA_VERSION` last), `pub fn check_schema_version(raw: &[u8]) -> Result<(), SchemaVersionError>`, `pub enum SchemaVersionError { Missing, Unsupported(String) }` (thiserror, `Missing` when the field is absent or not a string, the JSON itself unparseable included), `pub const EXIT_UNSUPPORTED: i32 = 5` in `verify_hash.rs`.

- [ ] **Step 1: Write the failing tests**

`accepts_every_published_version` (loop over v1.0 to v1.7), `refuses_a_newer_version` (`v1.8` gives `Unsupported("perf-sentinel-report/v1.8")`), `missing_version_is_missing` (`{}` and `not json`), `the_current_version_is_supported` (`SCHEMA_VERSION` is in the list). E2E `verify_hash_refuses_a_newer_schema_with_exit_5`: the G2 example with `schema_version` set to v1.8 and its hash re-baked, exit code 5, stderr names the version and says to upgrade perf-sentinel.

- [ ] **Step 2: Run them, expect compile failure then the e2e exit 1 or 3**

Run: `cargo test -p perf-sentinel-core --lib -- periodic::version` and `cargo test -p perf-sentinel --test verify_hash_e2e`

- [ ] **Step 3: Implement**

Read only `schema_version` with a `#[derive(Deserialize)] struct Probe { schema_version: Option<String> }`. In `verify_hash.rs` call it on the bytes before `parse_report` in both load paths and return `Err(EXIT_UNSUPPORTED)` with the message. Add exit 5 to the exit-code doc comment block and to `--help`.

- [ ] **Step 4: Run the tests, expect PASS, plus `cargo test -p perf-sentinel --test verify_hash_e2e`**

- [ ] **Step 5: Commit** `feat(verify-hash): refuse a report newer than this engine with exit 5`

---

### Task 2: `pdf` feature, fonts and the document skeleton

**Files:**
- Modify: `crates/sentinel-core/Cargo.toml` (`pdf = ["dep:krilla", "dep:fast_qr", "dep:lopdf"]`, optional deps at the versions above), `crates/sentinel-cli/Cargo.toml` (`pdf = ["perf-sentinel-core/pdf"]`, added to `default`), `deny.toml` only if `cargo deny check` asks (OFL is not a crate licence, the fonts are data)
- Create: `report/periodic/pdf/mod.rs`, `pdf/fonts/*`, `pdf/tests.rs`

**Interfaces:**
- Consumes: `check_schema_version` (Task 1), `compute_content_hash`, `check_attestation_binding(report, report_bytes, &InTotoStatement)` in `attestation.rs`, the statement read with `serde_json::from_slice::<InTotoStatement>` on the first line of the `.jsonl`, as `verify_hash.rs` does today.
- Produces:
  ```rust
  pub struct PdfInputs<'a> {
      pub report_bytes: &'a [u8],
      pub attestation: Option<&'a [u8]>,
      pub bundle: Option<&'a [u8]>,
      pub expected_identity: Option<&'a str>,
      pub expected_issuer: Option<&'a str>,
  }
  pub enum RenderError {
      Refused(RefusalReason),          // exit 1
      Malformed(String),               // exit 3, reason malformed_report
      UnsupportedSchema(String),       // exit 5
      UnsupportedCharacters(Vec<char>),// exit 5, sorted, deduplicated
  }
  pub enum RefusalReason { ContentHashMismatch, BindingMismatch }
  impl RenderError { pub fn reason(&self) -> &'static str; pub fn result(&self) -> &'static str; }
  pub fn render_disclosure_pdf(inputs: &PdfInputs<'_>) -> Result<Vec<u8>, RenderError>;
  ```

- [ ] **Step 1: Fetch the fonts**

Static TTFs from the upstream releases: DM Sans from `github.com/googlefonts/dm-fonts` (Regular and SemiBold, optical size 14), JetBrains Mono from `github.com/JetBrains/JetBrainsMono` releases (`fonts/ttf/JetBrainsMono-Regular.ttf`). Record each URL and SHA-256 in `OFL.txt` under the licence text. Load them with `include_bytes!`.

- [ ] **Step 2: Write the failing tests** (on the G2 example `docs/schemas/examples/example-official-public-G2.json` with its hash baked by `compute_content_hash`, and an attestation built with the existing test helper that names that hash)

- `renders_a_pdf_a_3b_with_the_three_attachments`: output starts with `%PDF-`, `extract_attachments` is not available yet, so assert with lopdf directly that the catalog's `AF` array names `report.json` (`/AFRelationship /Source`), `attestation.intoto.jsonl` and `bundle.sig` (`/Supplement`), and the embedded `report.json` equals the input bytes.
- `same_inputs_render_the_same_bytes`: two renders, equal vectors.
- `a_hash_that_does_not_recompute_is_refused`: one figure edited, `Refused(ContentHashMismatch)`.
- `an_attestation_naming_another_hash_is_refused`: `Refused(BindingMismatch)`.
- `a_newer_schema_is_unrenderable`: `UnsupportedSchema`.
- `garbage_is_malformed`: `Malformed`.
- `exit_contract_reasons`: `reason()` and `result()` for each variant equal the Global Constraints strings.

- [ ] **Step 3: Run them, expect failure**

Run: `cargo test -p perf-sentinel-core --features pdf --lib -- periodic::pdf`

- [ ] **Step 4: Implement the skeleton**

Order inside `render_disclosure_pdf`: schema gate, parse, hash, binding (only when an attestation is given, `Mismatch` refuses, `Unbindable` and `FileBytes` do not), then a krilla document with `Validator::A3_B`, A4 pages, metadata creation date from `generated_at`, a fixed document id derived from `content_hash`, the three embedded files with their association kinds, and one placeholder page carrying the document code. The five real pages come in Task 3.

- [ ] **Step 5: Run the tests, expect PASS**

- [ ] **Step 6: Commit** `feat(core): render a periodic report as a PDF/A-3b carrying its source files`

---

### Task 3: The five pages

**Files:**
- Create: `pdf/layout.rs`, `pdf/pages.rs`
- Modify: `pdf/mod.rs`, `pdf/tests.rs`

**Interfaces:**
- Produces (crate-private): `fn wrap(text: &str, font: &Font, size: f32, width: f32) -> Vec<String>` (greedy, breaks on spaces, a word wider than the line is split by characters, a hash in monospace is split every four characters at a group boundary), `fn missing_glyphs(texts: impl Iterator<Item = &str>, fonts: &[&Font]) -> Vec<char>`.

- [ ] **Step 1: Write the failing tests**

- `wrap_breaks_on_spaces_within_the_width`, `wrap_splits_a_word_longer_than_the_line`, `wrap_keeps_hash_groups_whole`.
- `an_organisation_in_cyrillic_is_unrenderable`: organisation name `Сбербанк`, `UnsupportedCharacters` listing those letters. Same with an emoji in a disclaimer.
- `latin_extended_renders`: `Société Générale`, `Łódź`, `Ærø` render.
- `a_v1_0_report_renders_its_missing_sections_as_not_reported`: the G2 example rewritten to v1.0 shape (no `canonical_waste`, `operational_waste`, `carbon_breakdown`, `trace_integrity_chain` null), PDF text contains `Not reported`.
- `the_cover_carries_the_document_code_and_no_qr_without_a_transparency_url`, and `a_transparency_url_draws_a_qr` (assert an image XObject exists on page 1 only in the second case).
- `page_five_fills_the_verify_command_with_the_expected_identity`.
- `every_disclaimer_is_copied_verbatim` (extract page text with lopdf and look for each one).

- [ ] **Step 2: Run them, expect failure**

- [ ] **Step 3: Implement the pages**

Content per page as the spec lists it (cover, at a glance, applications, method and limits, verify this document). The 14 October mock-up (`pdf-p1` to `pdf-p5` in the "Divulgations dans le Hub" artifact) is the visual reference: DM Sans for text, JetBrains Mono for hashes and commands, the Hub's palette. Check every string drawn with `missing_glyphs` before drawing anything. Page 5's command:
`perf-sentinel verify-hash --report report.pdf --expected-identity <ID> --expected-issuer <URL>`, with `<ID>` and `<URL>` printed as placeholders when not given.

- [ ] **Step 4: Run them, expect PASS. Run the Task 2 tests again (determinism)**

- [ ] **Step 5: Commit** `feat(core): lay out the five pages of a disclosure PDF`

---

### Task 4: `perf-sentinel disclose-pdf`

**Files:**
- Create: `crates/sentinel-cli/src/disclose_pdf.rs`
- Modify: `cli.rs` (`Commands::DisclosePdf`, `#[cfg(feature = "pdf")]`), `main.rs` dispatch
- Test: `crates/sentinel-cli/tests/e2e/disclose_pdf.rs` (new module in the e2e target)

**Interfaces:**
- Consumes: `render_disclosure_pdf`, `PdfInputs`, `RenderError` (Task 2), `read_capped` (`limits.rs`).
- Produces: flags `--report PATH` (required), `--attestation PATH`, `--bundle PATH`, `--expected-identity ID`, `--expected-issuer URL` (both or neither), `--output PATH` (required, written through `write_file_no_follow`, never on refusal). Exit codes and the JSON line as in Global Constraints.

- [ ] **Step 1: Write the failing e2e tests**

`disclose_pdf_renders_and_prints_rendered`, `disclose_pdf_refuses_edited_figures_with_exit_1` (stdout `{"result":"refused","reason":"content_hash_mismatch"}`, no output file), `disclose_pdf_refuses_a_foreign_attestation`, `disclose_pdf_missing_report_is_exit_3_missing_file`, `disclose_pdf_newer_schema_is_exit_5`, `disclose_pdf_cyrillic_is_exit_5_and_lists_the_characters_on_stderr`, `disclose_pdf_help_lists_the_exit_codes`.

- [ ] **Step 2: Run them, expect failure**

Run: `cargo test -p perf-sentinel --test e2e -- disclose_pdf`

- [ ] **Step 3: Implement**, reading inputs through `read_capped` (report at `MAX_LOCAL_REPORT_BYTES`, attestation and bundle at 1 MiB).

- [ ] **Step 4: Run them, expect PASS, plus `cargo check -p perf-sentinel --no-default-features` (the subcommand disappears cleanly)**

- [ ] **Step 5: Commit** `feat(cli): add disclose-pdf with a fixed exit-code contract`

---

### Task 5: Extraction and `verify-hash --report report.pdf`

**Files:**
- Create: `pdf/extract.rs`, `fuzz/fuzz_targets/pdf_extract.rs`
- Modify: `pdf/mod.rs`, `verify_hash.rs`, `fuzz/Cargo.toml` (target `pdf_extract`, core with `features = ["pdf"]`), `fuzz/Cargo.lock`
- Test: `pdf/tests.rs`, `verify_hash_e2e.rs`

**Interfaces:**
- Produces:
  ```rust
  pub struct PdfAttachments { pub report: Vec<u8>, pub attestation: Option<Vec<u8>>, pub bundle: Option<Vec<u8>> }
  pub enum ExtractError { NotAPdf, NoReport, Duplicate(&'static str), TooLarge(&'static str), Malformed(String) }
  pub fn extract_attachments(pdf: &[u8], report_cap: u64, sidecar_cap: u64) -> Result<PdfAttachments, ExtractError>;
  ```
  Attachments are found by file name through the `EmbeddedFiles` name tree and the catalog `AF` array, so a copy that keeps either still works.

- [ ] **Step 1: Write the failing tests**

- `extracts_the_three_files_byte_for_byte` (from a Task 2 render).
- `a_copy_resaved_with_object_streams_still_extracts`: load the render with lopdf, `compress()`, save with object streams, extract, same bytes.
- `a_pdf_without_report_json_is_no_report`, `two_report_json_are_duplicate`, `an_attachment_over_the_cap_is_too_large` (cap 16 bytes), `a_text_file_is_not_a_pdf`.
- E2E: `verify_hash_on_the_pdf_matches_verify_hash_on_the_json` (same `verifications` object with `--format json`). The no-`pdf` path is covered by `cargo check -p perf-sentinel --no-default-features` only.

- [ ] **Step 2: Run them, expect failure**

- [ ] **Step 3: Implement**

In `verify_hash.rs`, detect a PDF by its `%PDF-` prefix after the capped read, extract, then run the existing checks on the extracted report, writing the attestation and bundle to temp files with `create_new` as the `--url` path already does. Explicit `--attestation` or `--bundle` flags win over attached files. Without the `pdf` feature, a PDF exits 3 with "this build cannot read PDF disclosures".

- [ ] **Step 4: Fuzz target**

`fuzz_target!(|data: &[u8]| { let _ = extract_attachments(data, 1 << 20, 1 << 16); });`. Run it locally for one minute: `cargo +nightly fuzz run pdf_extract -- -max_total_time=60`. Expected: no crash.

- [ ] **Step 5: Run the tests, expect PASS**

- [ ] **Step 6: Commit** `feat(verify-hash): verify a disclosure PDF from its attachments`

---

### Task 6: CI

**Files:**
- Create: `.github/workflows/pdf-a.yml`
- Modify: `.github/workflows/disclosure-signing.yml`, `.github/workflows/fuzz.yml` (add `crates/sentinel-core/src/report/periodic/pdf/**` to `paths`, and `pdf_extract` to the target list)

- [ ] **Step 1: `pdf-a.yml`**

On pull requests touching `crates/sentinel-core/src/report/periodic/pdf/**`, `crates/sentinel-cli/src/disclose_pdf.rs` or the workflow itself: build, render the G2 example with `disclose-pdf`, run veraPDF on it with the PDF/A-3B profile, fail on any violation. Pin veraPDF by version and checksum (or image digest) from the official distribution, actions pinned by SHA as the other workflows do.

- [ ] **Step 2: Extend `disclosure-signing.yml`**

After the TRUSTED step: render the signed report with `disclose-pdf --expected-identity <the job identity> --expected-issuer https://token.actions.githubusercontent.com`, then `verify-hash --report report.pdf` with the same identity flags must exit 0 (TRUSTED). After the "edited figures" step: `disclose-pdf` on the edited report must exit 1 with `content_hash_mismatch`.

- [ ] **Step 3: Validate locally** with `actionlint` on the three files. Expected: no error.

- [ ] **Step 4: Commit** `ci: validate disclosure PDFs as PDF/A-3b and verify one signed`

---

### Task 7: Docs

**Files:**
- Modify: `docs/REPORTING.md` + `docs/FR/REPORTING-FR.md` (a "Rendering and publishing" section: `disclose-pdf`, its exit codes and reasons, verifying a PDF, `notes.reference_urls.transparency` in the org config, publishing to the Hub with `curl -F report=@report.json -F attestation=@... -F bundle=@... -H "X-API-Key: ..." https://hub/api/import/disclosures`), `docs/SUPPLY-CHAIN.md` + FR (the PDF carries the files, verification unchanged), `docs/CLI.md` + `docs/FR/CLI-FR.md`, `docs/SECURITY-OVERVIEW.md` + FR (a PDF parser on untrusted input, with its caps and fuzz target), `docs/ARCHITECTURE.md` module table + FR, `CHANGELOG.md` `[Unreleased]` (Added: `disclose-pdf`, `verify-hash` on a PDF, exit 5. Changed: release binary size, with the measured figure)

- [ ] **Step 1: Write them.** Prose rules: no em dash, no semicolon, straight quotes, sentence case, FR accents.

- [ ] **Step 2: Measure the binary size** (`cargo build --release -p perf-sentinel`, before and after the feature, `ls -l`) and put the figures in the CHANGELOG.

- [ ] **Step 3: Commit** `docs: describe disclose-pdf and verifying a disclosure PDF`

---

### Task 8: Release 0.31.0

Only once 0.30.0 (perf-sentinel#200) is merged: merge `main` into this branch first.

- [ ] **Step 1:** Bump as `docs/RELEASE-PROCEDURE.md` says: workspace version, CLI pin, CI templates and snippets, schema examples' `binary_verification_url`, both lockfiles (`cargo update -w`, `fuzz/`: `cargo update -p perf-sentinel-core` and revert any unrelated line), chart `version`, `appVersion`, `artifacthub.io/images` and `artifacthub.io/changes`, chart CHANGELOG, root CHANGELOG `[0.31.0] - <date>`. The bump is minor: new subcommand, new exit code, no public API removed.
- [ ] **Step 2:** `scripts/check-tag-version.sh v0.31.0`, `scripts/check-chart-version-bumped.sh main`, `scripts/check-chart-appversion-annotation.sh`. Expected: all pass.
- [ ] **Step 3:** Full verification: fmt, both clippy lines, both `--no-default-features` checks, `cargo test --workspace`, `cargo audit`, `cargo deny check`.
- [ ] **Step 4: Commit** `chore(release): 0.31.0`

The lab ledger line comes from the Hub plan's lab task, run on images built from the final tree.
