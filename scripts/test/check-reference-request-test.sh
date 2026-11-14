#!/usr/bin/env bash
#
# Self-test harness for scripts/check-reference-request.sh.
#
# Usage:
#   scripts/test/check-reference-request-test.sh
#   VERBOSE=1 scripts/test/check-reference-request-test.sh   # also print each verdict
#
# For each scenario: builds a throwaway directory under $(mktemp -d) with a
# fake `perf-sentinel` and a fake `curl` first in PATH, driven by response
# files, renders an issue-form body, runs the script under test and asserts
# the exit code plus optional checks on the verdict and on the recorded
# calls. Nothing touches the network and the real binary is never called.
# Prints one PASS/FAIL line per scenario and a summary. Exit 0 iff every
# scenario matches.
#
# Response files, in $FAKE_DIR (a missing file means "nothing there"):
#   card.out, card.exit          verify-card stdout and exit code
#   hash-url.out, .exit          verify-hash --url
#   hash-report.out, .exit       verify-hash --report
#   report.body, .code, .fail    curl download of the report
#   consent.body, .code, .fail   curl of /.well-known/perf-sentinel-listing
#   doh.body, .code, .fail       curl of the DNS-over-HTTPS service
# A `.fail` file holds a curl exit code: the fake prints nothing and exits
# with it, as curl does on a transport error. Each call is appended to
# $FAKE_DIR/calls.log.

# The single quotes below hold backticks and $( on purpose: hostile test data.
# shellcheck disable=SC2016
set -u

SCRIPT_UNDER_TEST="$(cd "$(dirname "$0")/../.." && pwd)/scripts/check-reference-request.sh"

PASS=0
FAIL=0

DOMAIN_OK="exemple-energie.fr"
ISSUE_OK="https://github.com/Institut-du-Numerique-Responsable/perf-sentinel/issues/123"
CARD_OK="https://perf-sentinel.dev/verifier#c=AAAA.BBBB"
REPORT_OK="https://www.exemple-energie.fr/perf-sentinel/rapport.json"
IDENTITY_OK="https://github.com/exemple/depot/.github/workflows/disclose.yml@refs/heads/main"
ISSUER_OK="https://token.actions.githubusercontent.com"
HOSTILE='$(touch pwned)"; touch pwned; "'

# --- Fixtures ---------------------------------------------------------------

# Edit a JSON response file in place with a jq filter.
edit() {
  local file="$1"
  shift
  jq "$@" "${file}" > "${file}.tmp" && mv "${file}.tmp" "${file}"
}

install_fakes() {
  mkdir -p bin fake
  : > fake/calls.log
  cat > bin/perf-sentinel <<'EOF'
#!/usr/bin/env bash
printf 'perf-sentinel %s\n' "$*" >> "$FAKE_DIR/calls.log"
case "${1-}" in
  verify-card) name=card ;;
  verify-hash) case " $* " in *" --url "*) name=hash-url ;; *) name=hash-report ;; esac ;;
  *) exit 99 ;;
esac
[ -f "$FAKE_DIR/$name.out" ] && cat "$FAKE_DIR/$name.out"
exit "$(cat "$FAKE_DIR/$name.exit" 2>/dev/null || echo 0)"
EOF
  cat > bin/curl <<'EOF'
#!/usr/bin/env bash
printf 'curl %s\n' "$*" >> "$FAKE_DIR/calls.log"
out=/dev/null
url=""
while [ $# -gt 0 ]; do
  case "$1" in
    --output|-o) out="$2"; shift ;;
    --url) url="$2"; shift ;;
    --max-redirs|--max-time|--max-filesize|--proto|--write-out|-w|--header|-H) shift ;;
  esac
  shift
done
case "$url" in
  https://cloudflare-dns.com/dns-query*) kind=doh ;;
  */.well-known/perf-sentinel-listing) kind=consent ;;
  *) kind=report ;;
esac
if [ -f "$FAKE_DIR/$kind.fail" ]; then
  printf '000'
  exit "$(cat "$FAKE_DIR/$kind.fail")"
fi
if [ -f "$FAKE_DIR/$kind.body" ]; then
  cat "$FAKE_DIR/$kind.body" > "$out"
  code=200
else
  : > "$out"
  code=404
fi
[ -f "$FAKE_DIR/$kind.code" ] && code="$(cat "$FAKE_DIR/$kind.code")"
printf '%s' "$code"
EOF
  chmod +x bin/perf-sentinel bin/curl
}

# Issue-form body, one `### <label>` block per field, `_No response_` when empty.
field_block() { printf '### %s\n\n%s\n\n' "$1" "${2:-_No response_}"; }

render_body() {
  field_block "Organisation name" "${F_NAME}"
  field_block "Domain" "${F_DOMAIN}"
  field_block "Card verify URL" "${F_CARD}"
  field_block "Published report URL" "${F_REPORT}"
  field_block "Expected signer identity" "${F_IDENTITY}"
  field_block "Expected signer issuer" "${F_ISSUER}"
  field_block "Stack" "${F_STACK}"
}

base_setup() {
  install_fakes
  F_NAME="Exemple Énergie"
  F_DOMAIN="${DOMAIN_OK}"
  F_CARD=""
  F_REPORT=""
  F_IDENTITY=""
  F_ISSUER=""
  F_STACK="Java, Postgres"
  RUN_ISSUE_URL="${ISSUE_OK}"
}

# A level 2 card for the domain, period September 2026.
with_card() {
  F_CARD="${CARD_OK}"
  cat > fake/card.out <<EOF
{"integrity":"ok","content":"not_checked","level":2,"domain":"${DOMAIN_OK}","key":"current","reason":"key_published","figures":{"angle":"story","period":{"kind":"month","from":"2026-09-01","to":"2026-09-30"},"cited_ops":41}}
EOF
}

# An unsigned official report for the domain, July to December 2026. The
# verifications object has every key the real `verify-hash --format json`
# prints (content_hash, core_patterns, attestation_binding, signature,
# binary_attestation), with the statuses a real run gives an unsigned report:
# nothing to bind, nothing signed, so PARTIAL (exit 2).
with_report() {
  F_REPORT="${REPORT_OK}"
  cat > fake/hash-url.out <<'EOF'
{"report_path":"x","report_metadata":{"intent":"official","integrity_level":"hash-only","perf_sentinel_version":"0.45.0","report_uuid":"u"},"period":{"from_date":"2026-07-01","to_date":"2026-12-31"},"verifications":{"content_hash":{"status":"ok","detail":"matches integrity.content_hash (sha256:abc)"},"core_patterns":{"status":"ok","detail":"matches canonical core set for local perf-sentinel (4 patterns)"},"attestation_binding":{"status":"not_provided"},"signature":{"status":"not_provided"},"binary_attestation":{"status":"not_provided"}},"overall":"PARTIAL"}
EOF
  echo 2 > fake/hash-url.exit
  cp fake/hash-url.out fake/hash-report.out
  cp fake/hash-url.exit fake/hash-report.exit
  cat > fake/report.body <<EOF
{"report_metadata":{"intent":"official"},"organisation":{"name":"Exemple Énergie","country":"FR","identifiers":{"domain":"${DOMAIN_OK}"}},"period":{"from_date":"2026-07-01","to_date":"2026-12-31"},"scope_manifest":{"environments_measured":["production"]},"integrity":{"content_hash":"sha256:abc","signature":null}}
EOF
}

# The same report, signed, with the signer fields filled. The run on the
# published URL finds the sidecar attestation and bundle: signature and
# attestation_binding are ok, TRUSTED (exit 0). The run on the downloaded file
# has no sidecar: the signature is skipped, PARTIAL (exit 2).
with_signed_report() {
  with_report
  F_IDENTITY="${IDENTITY_OK}"
  F_ISSUER="${ISSUER_OK}"
  edit fake/report.body '.integrity.signature = {"format":"sigstore-cosign-intoto-v1","signer_identity":"x","signer_issuer":"y"}'
  edit fake/hash-url.out '.verifications.attestation_binding = {"status":"ok","detail":"the attestation names this content hash"} | .verifications.signature = {"status":"ok","detail":"valid (signed by x via y)"} | .overall = "TRUSTED"'
  echo 0 > fake/hash-url.exit
  edit fake/hash-report.out '.verifications.signature = {"status":"skip","detail":"signature metadata present, pass --attestation <path> to verify"}'
}

with_consent_file() { printf '%s\n' "${ISSUE_OK}" > fake/consent.body; }

# --- Scenario setups --------------------------------------------------------

s_accepts_card() { with_card; with_consent_file; }
s_card_level_1() { with_card; edit fake/card.out '.level = 1 | .reason = "key_not_published"'; with_consent_file; }
s_card_other_domain() { with_card; edit fake/card.out '.domain = "autre.fr"'; with_consent_file; }
s_card_tampered() { with_card; echo '{"integrity":"tampered","level":1,"domain":null,"reason":"signature_invalid","figures":null}' > fake/card.out; echo 1 > fake/card.exit; with_consent_file; }
s_card_other_host() { with_card; F_CARD='https://evil.example/verifier#c=AAAA.BBBB'; with_consent_file; }
s_card_http() { with_card; F_CARD='http://perf-sentinel.dev/verifier#c=AAAA.BBBB'; with_consent_file; }
s_card_host_suffix() { with_card; F_CARD='https://perf-sentinel.dev.evil.example/verifier#c=AAAA.BBBB'; with_consent_file; }
s_card_hostile() { with_card; F_CARD="https://perf-sentinel.dev/verifier#c=AAAA${HOSTILE}"; with_consent_file; }
s_card_french_verifier() { with_card; F_CARD='https://perf-sentinel.dev/fr/verifier#c=AAAA.BBBB'; with_consent_file; }

s_report_http() { with_report; F_REPORT='http://www.exemple-energie.fr/rapport.json'; with_consent_file; }
s_report_other_host() { with_report; F_REPORT='https://exemple-energie.fr.evil.example/rapport.json'; with_consent_file; }
s_report_lookalike() { with_report; F_REPORT='https://evil-exemple-energie.fr/rapport.json'; with_consent_file; }
s_report_userinfo() { with_report; F_REPORT='https://exemple-energie.fr@evil.example/rapport.json'; with_consent_file; }
s_report_port() { with_report; F_REPORT='https://www.exemple-energie.fr:8443/rapport.json'; with_consent_file; }
s_report_hostile() { with_report; F_REPORT="https://www.exemple-energie.fr/r${HOSTILE}"; with_consent_file; }
s_report_internal() { with_report; edit fake/report.body '.report_metadata.intent = "internal"'; with_consent_file; }
s_report_other_name() { with_report; edit fake/report.body '.organisation.name = "Autre Société"'; with_consent_file; }
s_report_name_missing() { with_report; edit fake/report.body 'del(.organisation.name)'; with_consent_file; }
s_report_other_domain() { with_report; edit fake/report.body '.organisation.identifiers.domain = "autre.fr"'; with_consent_file; }
s_report_redirects() { with_report; echo 301 > fake/report.code; with_consent_file; }
s_report_hash_fails() {
  with_report
  edit fake/hash-report.out '.verifications.content_hash = {"status":"fail","detail":"mismatch"} | .overall = "UNTRUSTED"'
  echo 1 > fake/hash-report.exit
  with_consent_file
}
s_report_swapped() {
  with_report
  edit fake/hash-url.out '.verifications.content_hash = {"status":"fail","detail":"mismatch"} | .overall = "UNTRUSTED"'
  echo 1 > fake/hash-url.exit
  with_consent_file
}
s_report_other_hash() { with_report; edit fake/hash-url.out '.verifications.content_hash.detail = "matches integrity.content_hash (sha256:other)"'; with_consent_file; }
s_report_not_found() { with_report; echo 404 > fake/report.code; with_consent_file; }
s_report_server_error() { with_report; echo 503 > fake/report.code; with_consent_file; }
s_report_not_json() { with_report; echo '%PDF-1.7' > fake/report.body; with_consent_file; }
s_report_hostile_environment() { with_report; edit fake/report.body '.scope_manifest.environments_measured = ["pr`od"]'; with_consent_file; }
s_report_too_large() { with_report; echo 63 > fake/report.fail; with_consent_file; }
s_report_download_fails() { with_report; echo 28 > fake/report.fail; with_consent_file; }
s_hash_network_error() { with_report; echo 4 > fake/hash-url.exit; : > fake/hash-url.out; with_consent_file; }
s_unsigned_report() { with_report; with_consent_file; }
s_signed_no_fields() { with_signed_report; F_IDENTITY=""; F_ISSUER=""; with_consent_file; }
s_signed_one_field() { with_signed_report; F_ISSUER=""; with_consent_file; }
s_signed_ok() { with_signed_report; with_consent_file; }
s_signed_cosign_rejects() {
  with_signed_report
  edit fake/hash-url.out '.verifications.signature = {"status":"fail","detail":"cosign rejected"} | .overall = "UNTRUSTED"'
  echo 1 > fake/hash-url.exit
  with_consent_file
}
# A third party's attestation and bundle beside the report: cosign verifies
# them, but they name other figures.
s_signed_binding_fails() {
  with_signed_report
  edit fake/hash-url.out '.verifications.attestation_binding = {"status":"fail","detail":"the attestation names another content hash"} | .overall = "UNTRUSTED"'
  echo 1 > fake/hash-url.exit
  with_consent_file
}
s_signed_binding_skipped() {
  with_signed_report
  edit fake/hash-url.out '.verifications.attestation_binding = {"status":"skip","detail":"nothing ties it to these figures"} | .overall = "PARTIAL"'
  echo 2 > fake/hash-url.exit
  with_consent_file
}
s_signed_signature_skipped() {
  with_signed_report
  edit fake/hash-url.out '.verifications.signature = {"status":"skip","detail":"cosign not found in PATH"} | .overall = "PARTIAL"'
  echo 2 > fake/hash-url.exit
  with_consent_file
}
s_untrusted_first_run() {
  with_report
  edit fake/hash-report.out '.overall = "UNTRUSTED"'
  echo 1 > fake/hash-report.exit
  with_consent_file
}
s_untrusted_second_run() {
  with_report
  edit fake/hash-url.out '.overall = "UNTRUSTED"'
  echo 1 > fake/hash-url.exit
  with_consent_file
}
s_core_patterns_fail_first_run() {
  with_report
  edit fake/hash-report.out '.verifications.core_patterns = {"status":"fail","detail":"core_patterns_required diverges"}'
  echo 1 > fake/hash-report.exit
  with_consent_file
}
s_core_patterns_fail_second_run() {
  with_report
  edit fake/hash-url.out '.verifications.core_patterns = {"status":"fail","detail":"core_patterns_required diverges"}'
  echo 1 > fake/hash-url.exit
  with_consent_file
}
# The exit code and the JSON must agree: either one saying "not trusted" refuses.
s_card_integrity_not_ok() { with_card; edit fake/card.out '.integrity = "tampered"'; with_consent_file; }
s_exit_1_first_run() { with_report; echo 1 > fake/hash-report.exit; with_consent_file; }
s_exit_1_second_run() { with_report; echo 1 > fake/hash-url.exit; with_consent_file; }
# Equal details prove the two runs saw one content, so empty ones prove nothing.
s_hash_detail_missing() {
  with_report
  edit fake/hash-url.out 'del(.verifications.content_hash.detail)'
  edit fake/hash-report.out 'del(.verifications.content_hash.detail)'
  with_consent_file
}
s_signed_hostile_fields() { with_signed_report; F_IDENTITY="${HOSTILE}"; F_ISSUER="https://x${HOSTILE}"; with_consent_file; }
s_both_proofs() { with_card; with_report; with_consent_file; }

s_no_consent() { with_card; echo '{"Status":3}' > fake/doh.body; }
s_consent_by_dns() {
  with_card
  printf '{"Status":0,"Answer":[{"name":"_perf-sentinel-listing.%s","type":16,"TTL":300,"data":"\\"%s\\""}]}\n' "${DOMAIN_OK}" "${ISSUE_OK}" > fake/doh.body
}
s_dns_other_issue() {
  with_card
  printf '{"Status":0,"Answer":[{"type":16,"data":"\\"%s4\\""}]}\n' "${ISSUE_OK}" > fake/doh.body
}
s_consent_other_issue() { with_card; printf '%s4\n' "${ISSUE_OK}" > fake/consent.body; echo '{"Status":3}' > fake/doh.body; }
s_consent_padded() { with_card; printf '  \r\n %s \r\n\n' "${ISSUE_OK}" > fake/consent.body; }

s_network_failure() { with_card; echo 28 > fake/consent.fail; echo 28 > fake/doh.fail; }
s_network_failure_file_only() { with_card; echo 28 > fake/consent.fail; echo '{"Status":3}' > fake/doh.body; }
s_card_network_error() { with_card; echo 4 > fake/card.exit; echo '{"level":1,"reason":"binding_check_failed"}' > fake/card.out; with_consent_file; }
s_dns_not_json() { with_card; echo '<html>bad gateway</html>' > fake/doh.body; }
s_consent_server_error() { with_card; echo 503 > fake/consent.code; echo x > fake/consent.body; echo '{"Status":3}' > fake/doh.body; }

s_domain_uppercase() { with_card; F_DOMAIN="Exemple-Energie.fr"; with_consent_file; }
s_domain_one_label() { with_card; F_DOMAIN="localhost"; with_consent_file; }
s_domain_numeric_tld() { with_card; F_DOMAIN="1.2.3.4"; with_consent_file; }
s_domain_hostile() { with_card; F_DOMAIN="${HOSTILE}"; with_consent_file; }
s_domain_markdown() { with_card; F_DOMAIN='`@robin`.fr```'; with_consent_file; }
s_name_hostile() { with_card; F_NAME="${HOSTILE}"; with_consent_file; }
s_name_markup() { with_card; F_NAME='Exemple <b>Énergie</b>'; with_consent_file; }
s_name_invisible() { with_card; F_NAME=$'Exemple \xe2\x80\xaeÉnergie'; with_consent_file; }
s_name_zero_width() { with_card; F_NAME=$'Exemple\xe2\x80\x8bÉnergie'; with_consent_file; }
s_name_bom() { with_card; F_NAME=$'\xef\xbb\xbfExemple Énergie'; with_consent_file; }
s_name_joiner() { with_card; F_NAME=$'Exemple\xe2\x81\xa0Énergie'; with_consent_file; }
s_name_accented() { with_card; F_NAME="Écoute à l’œuvre ‑ 日本語 — Ñandú"; with_consent_file; }
s_name_missing() { with_card; F_NAME=""; with_consent_file; }
s_name_multiline() { with_card; F_NAME=$'Exemple\nÉnergie'; with_consent_file; }
s_repeated_heading() { with_card; F_STACK=$'### Domain\n\nother.fr'; with_consent_file; }
s_stack_multiline() { with_card; F_STACK=$'Java\nPostgres\n\nKafka'; with_consent_file; }
s_no_proof() { with_consent_file; }
s_bad_issue_url() { with_card; with_consent_file; RUN_ISSUE_URL='https://github.com/exemple/autre/issues/1'; }

# --- Harness ----------------------------------------------------------------

# The JSON entry block of the verdict, between the ```json and ``` lines.
entry_of() { awk '/^```json$/{on=1; next} /^```$/{on=0} on' "$1"; }

# run_scenario NAME EXPECTED_EXIT SETUP [CHECK ARG]...
# Checks: --has/--lacks S (stdout), --called/--never S (calls.log),
# --curl-all S (every curl call), --no-file F, --entry JQ_FILTER.
run_scenario() {
  local name="$1" expected_exit="$2" setup_func="$3"
  shift 3

  local tmpdir
  tmpdir=$(mktemp -d) || { echo "harness error: mktemp -d failed" >&2; exit 1; }
  local actual_exit
  actual_exit=$(
    cd "${tmpdir}" || exit 1
    base_setup
    "${setup_func}" > /dev/null
    export FAKE_DIR="${PWD}/fake"
    PATH="${PWD}/bin:${PATH}" ISSUE_URL="${RUN_ISSUE_URL}" ISSUE_BODY="$(render_body)" \
      "${BASH}" "${SCRIPT_UNDER_TEST}" > stdout.txt 2> stderr.txt
    echo "$?"
  )

  local problems=""
  [ "${actual_exit}" = "${expected_exit}" ] || problems="expected exit ${expected_exit}, got ${actual_exit}; "
  while [ $# -gt 0 ]; do
    local kind="$1" arg="$2"
    shift 2
    case "${kind}" in
      --has)   grep -qF -- "${arg}" "${tmpdir}/stdout.txt" 2>/dev/null || problems="${problems}stdout lacks '${arg}'; " ;;
      --lacks) ! grep -qF -- "${arg}" "${tmpdir}/stdout.txt" 2>/dev/null || problems="${problems}stdout has '${arg}'; " ;;
      --called) grep -qF -- "${arg}" "${tmpdir}/fake/calls.log" 2>/dev/null || problems="${problems}no call with '${arg}'; " ;;
      --never) ! grep -qF -- "${arg}" "${tmpdir}/fake/calls.log" 2>/dev/null || problems="${problems}a call has '${arg}'; " ;;
      --curl-all)
        ! grep '^curl ' "${tmpdir}/fake/calls.log" 2>/dev/null | grep -qvF -- "${arg}" || problems="${problems}a curl call lacks '${arg}'; " ;;
      --no-file) [ ! -e "${tmpdir}/${arg}" ] || problems="${problems}file '${arg}' exists; " ;;
      --entry)
        entry_of "${tmpdir}/stdout.txt" | jq -e "${arg}" > /dev/null 2>&1 || problems="${problems}entry fails '${arg}'; " ;;
      *) problems="${problems}unknown check '${kind}'; " ;;
    esac
  done

  if [ -z "${problems}" ]; then
    echo "PASS: ${name} (exit ${actual_exit})"
    PASS=$((PASS + 1))
  else
    echo "FAIL: ${name} (${problems%; })"
    FAIL=$((FAIL + 1))
  fi
  if [ -n "${VERBOSE-}" ]; then
    sed 's/^/    | /' "${tmpdir}/stdout.txt"
  fi
  rm -rf "${tmpdir}"
}

# --- Drive ------------------------------------------------------------------

run_scenario "accepts_a_level_2_card_with_consent" 0 s_accepts_card \
  --has '"since": "2026-09"' \
  --has '- [ ] Organisation name: not proven by a card, check that it belongs to `exemple-energie.fr`' \
  --lacks '- [x] Organisation name' --has 'needs a manual check' \
  --entry '.domain == "exemple-energie.fr" and .name == "Exemple Énergie" and .proofs.card.verify_url == "https://perf-sentinel.dev/verifier#c=AAAA.BBBB" and .proofs.card.period_from == "2026-09-01" and .proofs.card.period_to == "2026-09-30" and (.proofs | has("report") | not) and (.request | endswith("/issues/123"))' \
  --called 'verify-card https://perf-sentinel.dev/verifier#c=AAAA.BBBB --online' \
  --curl-all '--max-time 10' \
  --lacks 'Java'
run_scenario "accepts_a_card_from_the_french_verifier" 0 s_card_french_verifier
run_scenario "refuses_a_level_1_card" 1 s_card_level_1
run_scenario "refuses_a_card_whose_domain_differs" 1 s_card_other_domain
run_scenario "refuses_a_tampered_card" 1 s_card_tampered
run_scenario "refuses_a_card_url_on_another_host" 1 s_card_other_host --never 'perf-sentinel verify-' --never curl
run_scenario "refuses_a_card_url_in_http" 1 s_card_http --never 'perf-sentinel verify-' --never curl
run_scenario "refuses_a_card_url_on_a_host_that_extends_the_site" 1 s_card_host_suffix --never 'perf-sentinel verify-'
run_scenario "refuses_a_card_url_with_extra_characters" 1 s_card_hostile --never 'perf-sentinel verify-' --no-file pwned

run_scenario "refuses_an_http_report_url" 1 s_report_http --never 'perf-sentinel verify-' --never curl
run_scenario "refuses_a_report_served_outside_the_domain" 1 s_report_other_host --never 'perf-sentinel verify-'
run_scenario "refuses_a_report_on_a_lookalike_host" 1 s_report_lookalike --never 'perf-sentinel verify-'
run_scenario "refuses_a_report_url_with_a_user_part" 1 s_report_userinfo --never 'perf-sentinel verify-'
run_scenario "refuses_a_report_url_with_a_port" 1 s_report_port --never 'perf-sentinel verify-'
run_scenario "refuses_a_report_url_with_extra_characters" 1 s_report_hostile --never 'perf-sentinel verify-' --no-file pwned
run_scenario "refuses_an_internal_report" 1 s_report_internal
run_scenario "refuses_a_report_of_another_domain" 1 s_report_other_domain
run_scenario "refuses_a_report_that_names_another_organisation" 1 s_report_other_name \
  --has '`Autre Société`' --has '`Exemple Énergie`' --lacks '- [x] Organisation name' --lacks '```json'
run_scenario "refuses_a_report_without_an_organisation_name" 1 s_report_name_missing --lacks '```json'
run_scenario "refuses_a_report_that_redirects" 1 s_report_redirects \
  --called '--max-redirs 0' --called "--proto =https"
run_scenario "refuses_a_report_larger_than_the_engine_cap" 1 s_report_too_large \
  --has '10 MiB' --never 'perf-sentinel verify-'
run_scenario "refuses_a_report_that_answers_404" 1 s_report_not_found --never 'perf-sentinel verify-'
run_scenario "refuses_a_report_that_is_not_json" 1 s_report_not_json --never 'perf-sentinel verify-'
run_scenario "refuses_a_report_whose_content_hash_fails" 1 s_report_hash_fails
run_scenario "refuses_a_report_that_changed_between_two_fetches" 1 s_report_swapped
run_scenario "refuses_a_report_with_another_hash_on_the_second_fetch" 1 s_report_other_hash
run_scenario "refuses_an_environment_name_with_markup_characters" 1 s_report_hostile_environment
run_scenario "refuses_a_signed_report_without_identity_fields" 1 s_signed_no_fields \
  --never 'perf-sentinel verify-' --never '--no-identity-check' --never '--expected-identity'
run_scenario "refuses_a_signed_report_with_one_identity_field" 1 s_signed_one_field --never 'perf-sentinel verify-'
run_scenario "refuses_a_signed_report_the_signature_check_rejects" 1 s_signed_cosign_rejects
run_scenario "refuses_a_signed_report_whose_attestation_names_other_figures" 1 s_signed_binding_fails \
  --lacks '"signed": true' --lacks '```json' --has 'attestation_binding'
run_scenario "refuses_a_signed_report_whose_attestation_binding_is_skipped" 1 s_signed_binding_skipped \
  --lacks '"signed": true' --lacks '```json'
run_scenario "refuses_a_signed_report_whose_signature_was_skipped" 1 s_signed_signature_skipped \
  --lacks '"signed": true' --lacks '```json'
run_scenario "refuses_a_report_the_engine_rates_untrusted_on_the_first_run" 1 s_untrusted_first_run --lacks '```json'
run_scenario "refuses_a_report_the_engine_rates_untrusted_on_the_second_run" 1 s_untrusted_second_run --lacks '```json'
run_scenario "refuses_a_failed_secondary_check_on_the_first_run" 1 s_core_patterns_fail_first_run \
  --has 'core_patterns' --lacks '```json'
run_scenario "refuses_a_failed_secondary_check_on_the_second_run" 1 s_core_patterns_fail_second_run \
  --has 'core_patterns' --lacks '```json'
run_scenario "refuses_a_card_whose_signature_check_did_not_pass" 1 s_card_integrity_not_ok --lacks '```json'
run_scenario "refuses_a_report_when_the_first_run_exits_untrusted" 1 s_exit_1_first_run --lacks '```json'
run_scenario "refuses_a_report_when_the_second_run_exits_untrusted" 1 s_exit_1_second_run --lacks '```json'
run_scenario "refuses_a_report_without_a_content_hash_detail_to_compare" 1 s_hash_detail_missing --lacks '```json'
run_scenario "accepts_a_signed_report_with_identity_fields" 0 s_signed_ok \
  --has '"signed": true' \
  --called "--expected-identity=${IDENTITY_OK}" --called "--expected-issuer=${ISSUER_OK}" \
  --never '--no-identity-check' \
  --has '- [x] Organisation name: `Exemple Énergie` is the name in the report' \
  --entry '.proofs.report.signed == true and .proofs.report.environments == ["production"]'
run_scenario "accepts_an_unsigned_report_on_its_content_hash" 0 s_unsigned_report \
  --has '"signed": false' \
  --never '--expected-identity' --never '--no-identity-check' \
  --called '--max-redirs 0' --called '--max-filesize 10485760' --curl-all '--max-time 10' \
  --has '- [x] Organisation name: `Exemple Énergie` is the name in the report' --lacks 'manual check' \
  --entry '.since == "2026-07" and .proofs.report.url == "https://www.exemple-energie.fr/perf-sentinel/rapport.json" and .proofs.report.period_from == "2026-07-01" and .proofs.report.period_to == "2026-12-31" and (.proofs | has("card") | not)'
run_scenario "dates_the_listing_from_the_earliest_period" 0 s_both_proofs \
  --has '"since": "2026-07"' --has '- [x] Organisation name' --lacks 'manual check' \
  --entry '.proofs.card.period_from == "2026-09-01" and .proofs.report.period_from == "2026-07-01"'

run_scenario "refuses_without_domain_consent" 1 s_no_consent
run_scenario "accepts_consent_published_in_a_dns_txt_record" 0 s_consent_by_dns \
  --called 'cloudflare-dns.com/dns-query?name=_perf-sentinel-listing.exemple-energie.fr&type=TXT' \
  --called 'accept: application/dns-json'
run_scenario "refuses_a_txt_record_for_a_longer_issue_number" 1 s_dns_other_issue
run_scenario "refuses_a_consent_file_for_another_issue" 1 s_consent_other_issue
run_scenario "accepts_a_consent_file_with_padding" 0 s_consent_padded

run_scenario "keeps_a_hostile_field_as_data" 1 s_domain_hostile --no-file pwned --never 'perf-sentinel verify-' --never curl
run_scenario "keeps_a_hostile_name_as_data" 0 s_name_hostile --no-file pwned \
  --entry '.name | startswith("$(touch pwned)")'
run_scenario "keeps_hostile_signer_fields_as_data" 0 s_signed_hostile_fields --no-file pwned
run_scenario "wraps_a_rejected_value_so_it_cannot_inject_markdown" 1 s_domain_markdown \
  --lacks '```' --has 'Domain: `@robin.fr` is not'
run_scenario "refuses_a_name_with_markup" 1 s_name_markup --never 'perf-sentinel verify-'
run_scenario "refuses_a_name_with_an_invisible_direction_character" 1 s_name_invisible --never 'perf-sentinel verify-'
run_scenario "refuses_a_name_with_a_zero_width_space" 1 s_name_zero_width --never 'perf-sentinel verify-'
run_scenario "refuses_a_name_with_a_byte_order_mark" 1 s_name_bom --never 'perf-sentinel verify-'
run_scenario "refuses_a_name_with_a_word_joiner" 1 s_name_joiner --never 'perf-sentinel verify-'
run_scenario "accepts_a_name_with_accents_and_other_scripts" 0 s_name_accented
run_scenario "refuses_a_missing_name" 1 s_name_missing --never 'perf-sentinel verify-'
run_scenario "refuses_a_multi_line_field" 1 s_name_multiline --never 'perf-sentinel verify-'
run_scenario "refuses_a_repeated_field_heading" 1 s_repeated_heading --never 'perf-sentinel verify-'
run_scenario "accepts_a_multi_line_stack" 0 s_stack_multiline --lacks 'Kafka'
run_scenario "refuses_an_uppercase_domain" 1 s_domain_uppercase --never 'perf-sentinel verify-'
run_scenario "refuses_a_one_label_domain" 1 s_domain_one_label --never 'perf-sentinel verify-'
run_scenario "refuses_a_domain_whose_last_label_is_numeric" 1 s_domain_numeric_tld --never 'perf-sentinel verify-'
run_scenario "refuses_a_request_without_proof" 1 s_no_proof --never 'perf-sentinel verify-' --never curl
run_scenario "stops_on_an_issue_url_of_another_repository" 3 s_bad_issue_url --never 'perf-sentinel verify-'

run_scenario "reports_a_network_failure_as_could_not_check" 2 s_network_failure --has 'Could not check'
run_scenario "reports_a_failed_consent_lookup_as_could_not_check" 2 s_network_failure_file_only
run_scenario "reports_a_card_network_error_as_could_not_check" 2 s_card_network_error
run_scenario "reports_a_dns_answer_that_is_not_json_as_could_not_check" 2 s_dns_not_json
run_scenario "reports_a_consent_server_error_as_could_not_check" 2 s_consent_server_error
run_scenario "reports_a_report_network_error_as_could_not_check" 2 s_hash_network_error
run_scenario "reports_a_failed_report_download_as_could_not_check" 2 s_report_download_fails --never 'perf-sentinel verify-'
run_scenario "reports_a_report_server_error_as_could_not_check" 2 s_report_server_error --never 'perf-sentinel verify-'

echo ""
echo "Summary: ${PASS} passed, ${FAIL} failed"
[ "${FAIL}" -eq 0 ]
