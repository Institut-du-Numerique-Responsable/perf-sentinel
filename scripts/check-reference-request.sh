#!/usr/bin/env bash
#
# Verifies a listing request: the signed proofs an organisation gave in the
# issue form .github/ISSUE_TEMPLATE/reference.yml. Backs the workflow
# .github/workflows/reference-request.yml and changes no file.
#
# Reads from the environment:
#   ISSUE_BODY  the issue body (text written by a stranger)
#   ISSUE_URL   the issue's html_url
# and finds perf-sentinel, curl and jq in PATH. Writes a Markdown verdict on
# stdout. Exit codes:
#   0  accepted
#   1  refused
#   2  could not check (network): re-run later
#   3  the script itself is misused (bad ISSUE_URL, missing tool)
#
# Every form value is data. It is only expanded inside double quotes, and it
# never reaches eval, a shell -c, a jq program or a printf format: jq gets
# values through --arg, printf through %s, the engine and curl as arguments.
# Rules run in order and the first that fails ends the run.

# The single quotes below hold Markdown backticks, which are literal.
# shellcheck disable=SC2016
set -euo pipefail
export LC_ALL=C

readonly CARD_PREFIX_EN='https://perf-sentinel.dev/verifier#c='
readonly CARD_PREFIX_FR='https://perf-sentinel.dev/fr/verifier#c='
readonly ISSUE_URL_RE='^https://github\.com/Institut-du-Numerique-Responsable/perf-sentinel/issues/[0-9]+$'
# RFC 3986 characters minus quotes, backtick, backslash, angle brackets, braces.
readonly URL_CHARS_RE='^[A-Za-z0-9._~:/?#%&=+@,;!*()-]+$'
readonly MAX_URL_LENGTH=4096
readonly MAX_REPORT_BYTES=16777216
readonly CONSENT_PATH='/.well-known/perf-sentinel-listing'
readonly DOH_URL='https://cloudflare-dns.com/dns-query'

NAME='' DOMAIN='' CARD_URL='' REPORT_URL='' IDENTITY='' ISSUER=''
CARD_FROM='' CARD_TO='' REPORT_FROM='' REPORT_TO=''
REPORT_ENVIRONMENTS='[]' REPORT_SIGNED=false
TMP=''

# --- Verdict ----------------------------------------------------------------

# A form value as inline code: backticks and control characters removed, cut
# at 100 characters, so it cannot open a code block, a link or a mention.
code() {
  local bt='`' v="$1"
  v="${v//$bt/}"
  v="${v//[[:cntrl:]]/ }"
  if [ -z "$v" ]; then
    printf '(empty)'
  else
    printf '`%s`' "${v:0:100}"
  fi
}

pass() { printf -- '- [x] %s\n' "$1"; }

refuse() {
  printf -- '- [ ] %s\n\n**Refused.** Fix the point above, then edit this issue to run the check again.\n' "$1"
  exit 1
}

could_not_check() {
  printf -- '- [ ] %s\n\n**Could not check.** This is not a verdict on the request. The check will run again when the issue is edited, or a maintainer can re-run the workflow.\n' "$1"
  exit 2
}

# --- Helpers ----------------------------------------------------------------

trim() {
  local s="$1"
  s="${s#"${s%%[![:space:]]*}"}"
  s="${s%"${s##*[![:space:]]}"}"
  printf '%s' "$s"
}

# read_field VARIABLE "Label": the text of a form field, between its
# `### <Label>` line and the next `### ` line, trimmed. GitHub renders an empty
# optional field as `_No response_`. A value on several lines is refused.
read_field() {
  local var="$1" label="$2" v
  v=$(printf '%s\n' "$ISSUE_BODY" | tr -d '\r' | awk -v want="### $label" '
    function trim(s) { sub(/^[ \t]+/, "", s); sub(/[ \t]+$/, "", s); return s }
    $0 == want { on = 1; next }
    /^### / { on = 0 }
    on { lines[++n] = trim($0) }
    END {
      first = 1
      while (first <= n && lines[first] == "") first++
      last = n
      while (last >= first && lines[last] == "") last--
      for (i = first; i <= last; i++) print lines[i]
    }')
  if [ "$v" = "_No response_" ]; then
    v=''
  fi
  case "$v" in
    *$'\n'*) refuse "$label: the value must fit on one line" ;;
  esac
  if [ "${#v}" -gt "$MAX_URL_LENGTH" ]; then
    refuse "$label: the value is too long"
  fi
  printf -v "$var" '%s' "$v"
}

# http_get URL FILE MAX_BYTES [CURL_ARG]...: prints the HTTP status, writes the
# body to FILE. No redirect is followed. Returns curl's exit code.
http_get() {
  local url="$1" out="$2" max="$3"
  shift 3
  curl --silent --show-error --proto '=https' --max-redirs 0 --max-time 10 \
    --max-filesize "$max" --output "$out" --write-out '%{http_code}' "$@" --url "$url" 2>/dev/null
}

# Engine exit code of a command whose output goes to FILE. Stderr is dropped:
# it can carry text from the report and it would reach a public log.
run_engine() {
  local out="$1" rc=0
  shift
  perf-sentinel "$@" > "$out" 2> /dev/null || rc=$?
  return "$rc"
}

is_json_object() { jq -e 'type == "object"' "$1" > /dev/null 2>&1; }

is_date() {
  [[ $1 =~ ^[0-9]{4}-(0[1-9]|1[0-2])-(0[1-9]|[12][0-9]|3[01])$ ]]
}

# A period read from a proof: two valid dates, the first not after the second.
check_period() {
  local what="$1" from="$2" to="$3"
  if ! is_date "$from" || ! is_date "$to" || [[ $from > $to ]]; then
    refuse "$what: the period $(code "$from") to $(code "$to") is not a valid range of dates"
  fi
}

# --- Rules ------------------------------------------------------------------

load_form() {
  read_field NAME "Organisation name"
  read_field DOMAIN "Domain"
  read_field CARD_URL "Card verify URL"
  read_field REPORT_URL "Published report URL"
  read_field IDENTITY "Expected signer identity"
  read_field ISSUER "Expected signer issuer"
}

# Rule 1, the share card's domain rule: lowercase ASCII, 253 characters at
# most, two labels or more of 1 to 63 [a-z0-9-] with no edge hyphen, the last
# label not all digits.
check_domain() {
  local label='[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?'
  local re="^(${label}\\.)+${label}\$"
  if [ "${#DOMAIN}" -le 253 ] && [[ $DOMAIN =~ $re ]] && ! [[ ${DOMAIN##*.} =~ ^[0-9]+$ ]]; then
    pass "Domain: $(code "$DOMAIN") is a valid public domain name"
  else
    refuse "Domain: $(code "$DOMAIN") is not a valid domain name (lowercase ASCII, two labels or more, the last one not all digits)"
  fi
}

check_name() {
  local bad='[[:cntrl:]<>`]'
  if [ -z "$NAME" ]; then
    refuse "Organisation name: required"
  fi
  if [ "${#NAME}" -gt 120 ] || [[ $NAME =~ $bad ]]; then
    refuse "Organisation name: $(code "$NAME") is too long or holds markup characters"
  fi
  pass "Organisation name: $(code "$NAME")"
}

# Rule 2.
check_has_a_proof() {
  if [ -z "$CARD_URL" ] && [ -z "$REPORT_URL" ]; then
    refuse "Proofs: give a card verify URL, a published report URL, or both"
  fi
}

# A URL that may be echoed, passed to a command and pasted in a JSON block.
check_url_shape() {
  local what="$1" url="$2"
  if [ "${#url}" -gt "$MAX_URL_LENGTH" ] || ! [[ $url =~ $URL_CHARS_RE ]]; then
    refuse "$what: $(code "$url") holds characters a link never needs"
  fi
}

# Rule 3.
check_card() {
  local rest rc=0 reason found
  check_url_shape "Card" "$CARD_URL"
  case "$CARD_URL" in
    "$CARD_PREFIX_EN"*) rest="${CARD_URL#"$CARD_PREFIX_EN"}" ;;
    "$CARD_PREFIX_FR"*) rest="${CARD_URL#"$CARD_PREFIX_FR"}" ;;
    *) refuse "Card: $(code "$CARD_URL") is not a link to the verifier on perf-sentinel.dev" ;;
  esac
  if ! [[ $rest =~ ^[A-Za-z0-9._-]+$ ]]; then
    refuse "Card: the link does not end with a card token"
  fi

  run_engine "$TMP/card.json" verify-card "$CARD_URL" --online || rc=$?
  case "$rc" in
    0 | 1 | 3) ;;
    *) could_not_check "Card: the check ended with exit code $rc before giving a verdict" ;;
  esac
  if ! is_json_object "$TMP/card.json"; then
    could_not_check "Card: the check printed no verdict"
  fi
  if [ "$rc" -ne 0 ]; then
    reason=$(jq -r '.reason // "unknown"' "$TMP/card.json" 2> /dev/null || true)
    refuse "Card: the card is not valid, reason $(code "$reason")"
  fi
  if ! jq -e '.level == 2' "$TMP/card.json" > /dev/null 2>&1; then
    found=$(jq -r '.level // "none"' "$TMP/card.json" 2> /dev/null || true)
    refuse "Card: level $(code "$found") found, level 2 is needed (the domain must publish the signing key)"
  fi
  if ! jq -e --arg d "$DOMAIN" '.domain == $d' "$TMP/card.json" > /dev/null 2>&1; then
    found=$(jq -r '.domain // "none"' "$TMP/card.json" 2> /dev/null || true)
    refuse "Card: it was signed for $(code "$found"), not for $(code "$DOMAIN")"
  fi
  CARD_FROM=$(jq -r '.figures.period.from // ""' "$TMP/card.json" 2> /dev/null || true)
  CARD_TO=$(jq -r '.figures.period.to // ""' "$TMP/card.json" 2> /dev/null || true)
  check_period "Card" "$CARD_FROM" "$CARD_TO"
  pass "Card: level 2 for $(code "$DOMAIN"), period $(code "$CARD_FROM") to $(code "$CARD_TO")"
}

# The host is the domain or one of its subdomains.
host_is_under_domain() {
  local host="$1" domain="$2" prefix
  [ "$host" = "$domain" ] && return 0
  prefix="${host%".$domain"}"
  [ "$prefix" != "$host" ] && [[ $prefix =~ ^[A-Za-z0-9]([A-Za-z0-9.-]*[A-Za-z0-9])?$ ]]
}

# Rule 4, first half: the address, before any network call.
check_report_url() {
  local rest host
  check_url_shape "Report" "$REPORT_URL"
  case "$REPORT_URL" in
    https://*) ;;
    *) refuse "Report: $(code "$REPORT_URL") must start with https://" ;;
  esac
  rest="${REPORT_URL#https://}"
  host="${rest%%[/?#]*}"
  if ! [[ $host =~ ^[A-Za-z0-9.-]+$ ]]; then
    refuse "Report: the host of $(code "$REPORT_URL") must be a plain name, with no user part and no port"
  fi
  if ! host_is_under_domain "$host" "$DOMAIN"; then
    refuse "Report: it is served from $(code "$host"), which is neither $(code "$DOMAIN") nor one of its subdomains"
  fi
}

# status of a check in a verify-hash JSON file, empty when absent.
hash_status() { jq -r ".verifications.$2.status // \"\"" "$1" 2> /dev/null || true; }
hash_detail() { jq -r ".verifications.$2.detail // \"\"" "$1" 2> /dev/null || true; }

# Runs verify-hash with the given arguments, output in FILE, and sorts its
# exit codes: 0, 1 and 2 print a verdict, 3 means the report is unreadable,
# the others (4 network, 5 newer schema, a crash) are not a verdict on it.
run_verify_hash() {
  local out="$1" rc=0
  shift
  run_engine "$out" verify-hash "$@" --format json || rc=$?
  case "$rc" in
    0 | 1 | 2) ;;
    3) refuse "Report: the report could not be read as a Perf Sentinel disclosure report" ;;
    *) could_not_check "Report: the check ended with exit code $rc before giving a verdict" ;;
  esac
  if ! is_json_object "$out"; then
    could_not_check "Report: the check printed no verdict"
  fi
}

# Downloads the report to FILE, no redirect followed, 16 MiB at most.
download_report() {
  local file="$1" code rc=0
  code=$(http_get "$REPORT_URL" "$file" "$MAX_REPORT_BYTES") || rc=$?
  case "$rc" in
    0) ;;
    63) refuse "Report: it is larger than 16 MiB" ;;
    *) could_not_check "Report: the download failed (curl exit code $rc)" ;;
  esac
  case "$code" in
    200) ;;
    429 | 5??) could_not_check "Report: the server answered HTTP $code" ;;
    *) refuse "Report: the server answered HTTP $code (a redirect is not followed, give the final address)" ;;
  esac
  if [ "$(wc -c < "$file")" -gt "$MAX_REPORT_BYTES" ]; then
    refuse "Report: it is larger than 16 MiB"
  fi
  if ! is_json_object "$file"; then
    refuse "Report: the address must serve the JSON report"
  fi
}

# Rule 4.
check_report() {
  local intent
  check_report_url
  if [ -n "$IDENTITY$ISSUER" ] && { [ -z "$IDENTITY" ] || [ -z "$ISSUER" ]; }; then
    refuse "Report: fill both signer fields, or neither"
  fi
  local args=(--url "$REPORT_URL") signer
  if [ -n "$IDENTITY" ]; then
    for signer in "$IDENTITY" "$ISSUER"; do
      case "$signer" in
        -* | *[[:cntrl:]]*) refuse "Report: a signer field must not start with a dash or hold control characters" ;;
      esac
    done
    args+=("--expected-identity=$IDENTITY" "--expected-issuer=$ISSUER")
  fi

  # Download first: a 404 or a redirect is a plain refusal, whereas the engine
  # reports any failed fetch as a network error. Then check the hash of the
  # bytes read below, and check that the engine, which fetches the report
  # itself, saw the same content.
  download_report "$TMP/report.json"
  run_verify_hash "$TMP/hash-file.json" --report "$TMP/report.json"
  if [ "$(hash_status "$TMP/hash-file.json" content_hash)" != ok ]; then
    refuse "Report: its content hash does not match the report"
  fi
  run_verify_hash "$TMP/hash-url.json" "${args[@]}"
  if [ "$(hash_status "$TMP/hash-url.json" content_hash)" != ok ] ||
    [ "$(hash_detail "$TMP/hash-url.json" content_hash)" != "$(hash_detail "$TMP/hash-file.json" content_hash)" ]; then
    refuse "Report: the file changed between two downloads, or its content hash does not match"
  fi

  intent=$(jq -r '.report_metadata.intent // ""' "$TMP/report.json" 2> /dev/null || true)
  if [ "$intent" != official ]; then
    refuse "Report: its intent is $(code "$intent"), a listing needs an official report"
  fi
  if ! jq -e --arg d "$DOMAIN" '.organisation.identifiers.domain == $d' "$TMP/report.json" > /dev/null 2>&1; then
    refuse "Report: it does not name $(code "$DOMAIN") as the organisation domain"
  fi

  if jq -e '.integrity.signature != null' "$TMP/report.json" > /dev/null 2>&1; then
    if [ -z "$IDENTITY" ] || [ -z "$ISSUER" ]; then
      refuse "Report: it is signed, so both signer fields (identity and issuer) are required"
    fi
    if [ "$(hash_status "$TMP/hash-url.json" signature)" != ok ]; then
      refuse "Report: the signature check gave $(code "$(hash_status "$TMP/hash-url.json" signature)"), $(code "$(hash_detail "$TMP/hash-url.json" signature)")"
    fi
    REPORT_SIGNED=true
  fi

  REPORT_FROM=$(jq -r '.period.from_date // ""' "$TMP/report.json" 2> /dev/null || true)
  REPORT_TO=$(jq -r '.period.to_date // ""' "$TMP/report.json" 2> /dev/null || true)
  check_period "Report" "$REPORT_FROM" "$REPORT_TO"
  if ! jq -e '(.scope_manifest.environments_measured // []) | type == "array" and length <= 20 and all(.[]; type == "string" and test("\\A[A-Za-z0-9._ :/+-]{1,64}\\z"))' \
    "$TMP/report.json" > /dev/null 2>&1; then
    refuse "Report: the measured environment names must be plain words (letters, digits, . _ - : / +)"
  fi
  REPORT_ENVIRONMENTS=$(jq -c '.scope_manifest.environments_measured // []' "$TMP/report.json")
  if [ "$REPORT_SIGNED" = true ]; then
    pass "Report: official, content hash and signature verified, period $(code "$REPORT_FROM") to $(code "$REPORT_TO")"
  else
    pass "Report: official, content hash verified, unsigned, period $(code "$REPORT_FROM") to $(code "$REPORT_TO")"
  fi
}

# Consent lookups return 0 when they name this issue, 1 when they definitely
# do not, 2 when they could not be checked.
consent_from_file() {
  local code rc=0 body
  code=$(http_get "https://${DOMAIN}${CONSENT_PATH}" "$TMP/consent" 65536) || rc=$?
  if [ "$rc" -eq 63 ]; then
    return 1
  elif [ "$rc" -ne 0 ]; then
    return 2
  fi
  case "$code" in
    200) ;;
    429 | 5??) return 2 ;;
    *) return 1 ;;
  esac
  body=$(trim "$(cat "$TMP/consent")")
  [ "$body" = "$ISSUE_URL" ]
}

consent_from_dns() {
  local code rc=0 status
  code=$(http_get "${DOH_URL}?name=_perf-sentinel-listing.${DOMAIN}&type=TXT" "$TMP/doh.json" 65536 \
    --header 'accept: application/dns-json') || rc=$?
  if [ "$rc" -ne 0 ] || [ "$code" != 200 ]; then
    return 2
  fi
  if ! jq -e 'type == "object" and (.Status | type == "number")' "$TMP/doh.json" > /dev/null 2>&1; then
    return 2
  fi
  status=$(jq -r '.Status' "$TMP/doh.json")
  case "$status" in
    0 | 3) ;;
    *) return 2 ;;
  esac
  # The chunks of a TXT record join without a space. A record names the issue
  # when one of its words is the issue address.
  jq -e --arg want "$ISSUE_URL" '
    [.Answer[]? | select(.type == 16) | .data | strings
      | gsub("\"\\s+\""; "") | ltrimstr("\"") | rtrimstr("\"")
      | split("\\s+"; null)[]]
    | any(. == $want)' "$TMP/doh.json" > /dev/null 2>&1
}

# Rule 5.
check_consent() {
  local file_rc=0 dns_rc=0
  consent_from_file || file_rc=$?
  if [ "$file_rc" -eq 0 ]; then
    pass "Consent: $(code "https://$DOMAIN$CONSENT_PATH") names this issue"
    return 0
  fi
  consent_from_dns || dns_rc=$?
  if [ "$dns_rc" -eq 0 ]; then
    pass "Consent: a TXT record of $(code "_perf-sentinel-listing.$DOMAIN") names this issue"
    return 0
  fi
  if [ "$file_rc" -eq 2 ] || [ "$dns_rc" -eq 2 ]; then
    could_not_check "Consent: the lookup of $(code "$DOMAIN") failed, so the domain could not be asked"
  fi
  refuse "Consent: $(code "$DOMAIN") does not agree to the listing. Publish the address of this issue, $(code "$ISSUE_URL"), as the whole content of $(code "https://$DOMAIN$CONSENT_PATH") or in a TXT record of $(code "_perf-sentinel-listing.$DOMAIN")"
}

# --- Entry ------------------------------------------------------------------

emit_entry() {
  local earliest='' since entry d
  for d in "$CARD_FROM" "$REPORT_FROM"; do
    if [ -n "$d" ] && { [ -z "$earliest" ] || [[ $d < $earliest ]]; }; then
      earliest="$d"
    fi
  done
  since="${earliest:0:7}"
  entry=$(jq -n \
    --arg id "${DOMAIN//./-}" --arg name "$NAME" --arg domain "$DOMAIN" --arg since "$since" \
    --arg request "$ISSUE_URL" \
    --arg card_url "$CARD_URL" --arg card_from "$CARD_FROM" --arg card_to "$CARD_TO" \
    --arg report_url "$REPORT_URL" --arg report_from "$REPORT_FROM" --arg report_to "$REPORT_TO" \
    --argjson environments "$REPORT_ENVIRONMENTS" --argjson signed "$REPORT_SIGNED" '
    {
      id: $id, name: $name, domain: $domain, since: $since, request: $request,
      proofs: (
        (if $card_url != "" then
          {card: {verify_url: $card_url, period_from: $card_from, period_to: $card_to}}
        else {} end)
        + (if $report_url != "" then
          {report: {url: $report_url, period_from: $report_from, period_to: $report_to,
                    environments: $environments, signed: $signed}}
        else {} end))
    }')
  printf '\n**Accepted.** Add this entry to `site-data/references.json`, then run `cargo test -p perf-sentinel-core --test site_data`.\n\n'
  printf '```json\n%s\n```\n' "$entry"
}

main() {
  local tool
  for tool in perf-sentinel curl jq; do
    if ! command -v "$tool" > /dev/null 2>&1; then
      echo "error: $tool not found in PATH" >&2
      exit 3
    fi
  done
  if ! [[ ${ISSUE_URL-} =~ $ISSUE_URL_RE ]]; then
    echo "error: ISSUE_URL must be the address of an issue of this repository" >&2
    exit 3
  fi
  ISSUE_BODY="${ISSUE_BODY-}"
  TMP=$(mktemp -d)
  trap 'rm -rf "$TMP"' EXIT

  printf '### Listing request check\n\n'
  load_form
  check_domain
  check_name
  check_has_a_proof
  if [ -n "$CARD_URL" ]; then
    check_card
  fi
  if [ -n "$REPORT_URL" ]; then
    check_report
  fi
  check_consent
  emit_entry
}

main "$@"
