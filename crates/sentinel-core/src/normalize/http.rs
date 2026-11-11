//! HTTP URL normalizer.
//!
//! Replaces numeric path segments and upper-case alphanumeric codes with
//! `{id}`, UUID segments with `{uuid}`, strips query parameters, and prepends
//! the HTTP method.

use std::borrow::Cow;

/// Check if a string is a UUID (8-4-4-4-12 hex with dashes).
/// Hand-coded for performance, avoids regex engine overhead on the hot path.
fn is_uuid(s: &str) -> bool {
    if s.len() != 36 {
        return false;
    }
    let b = s.as_bytes();
    b[8] == b'-'
        && b[13] == b'-'
        && b[18] == b'-'
        && b[23] == b'-'
        && b.iter()
            .enumerate()
            .all(|(i, &c)| matches!(i, 8 | 13 | 18 | 23) || c.is_ascii_hexdigit())
}

/// Result of HTTP URL normalization.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpNormalized {
    pub template: String,
    pub params: Vec<String>,
}

/// The placeholder an identifier-like path segment becomes, or `None` for a
/// path word kept as is.
///
/// - `{uuid}`: a dashed UUID.
/// - `{id}`: all digits, or a hex string of at least 16 characters with a
///   digit (dashless UUID, Mongo `ObjectId`, SHA digest), in any case.
/// - `{id}`, with `mask_codes` only: an upper-case alphanumeric code of at
///   least 4 characters (`K7QX2P`, `INV2024001`). Letters and digits must
///   alternate at least twice, or the digits run 6 long, so fixed words made
///   of a prefix and a short number (`UTF8`, `H264`, `X509`, `SHA256`,
///   `ISO8601`) stay. RPC `service/method` targets turn it off: an IDL name
///   such as `WAFV2` is never a per-call value.
///
/// One pass over the bytes, and the first byte that is neither a letter nor
/// a digit ends it: most path words (`api`, `v2`, `my-orders`) cost a few
/// bytes.
fn segment_placeholder(seg: &str, mask_codes: bool) -> Option<&'static str> {
    if is_uuid(seg) {
        return Some("{uuid}");
    }
    let shape = SegmentShape::scan(seg)?;
    let len = seg.len();
    if shape.digits == 0 {
        return None;
    }
    let numeric = shape.digits == len;
    let hex_id = shape.hex && len >= 16;
    let code = mask_codes
        && len >= 4
        && shape.digits + shape.upper == len
        && (shape.runs >= 3 || shape.longest_digit_run >= 6);
    (numeric || hex_id || code).then_some("{id}")
}

/// Byte counts of an all-alphanumeric segment, read by `segment_placeholder`.
struct SegmentShape {
    digits: usize,
    upper: usize,
    /// Every letter is a hex digit.
    hex: bool,
    /// Maximal runs of letters or of digits: `K7QX2P` has 5, `UTF8` 2.
    runs: usize,
    longest_digit_run: usize,
}

impl SegmentShape {
    /// `None` at the first byte that is neither an ASCII letter nor a digit.
    fn scan(seg: &str) -> Option<Self> {
        let mut shape = Self {
            digits: 0,
            upper: 0,
            hex: true,
            runs: 0,
            longest_digit_run: 0,
        };
        let mut digit_run = 0usize;
        let mut prev_digit = None;
        for &c in seg.as_bytes() {
            let is_digit = c.is_ascii_digit();
            if is_digit {
                shape.digits += 1;
                digit_run += 1;
                shape.longest_digit_run = shape.longest_digit_run.max(digit_run);
            } else if c.is_ascii_alphabetic() {
                digit_run = 0;
                shape.upper += usize::from(c.is_ascii_uppercase());
                shape.hex &= c.is_ascii_hexdigit();
            } else {
                return None;
            }
            if prev_digit != Some(is_digit) {
                shape.runs += 1;
                prev_digit = Some(is_digit);
            }
        }
        Some(shape)
    }
}

/// Count occurrences of `target` in `s`.
fn bytecount(s: &str, target: u8) -> usize {
    s.bytes().filter(|&b| b == target).count()
}

/// Normalize an HTTP target URL.
///
/// Replaces numeric segments and upper-case alphanumeric codes with `{id}`,
/// UUID segments with `{uuid}`, strips query params, and prepends the
/// method. The callee host is kept in the template for DNS-addressed calls
/// (`GET user-svc/api/x`) so two calls to the same path on different
/// backends do not merge into one group and raise a false redundant/N+1
/// finding. IP-literal authorities are dropped, so load-balanced replicas
/// (pods behind one service) still group together.
#[must_use]
pub fn normalize_http(method: &str, target: &str) -> HttpNormalized {
    // Split scheme + authority from the path, keeping the authority so a
    // DNS host can stay in the grouping template.
    let (authority, path_and_query) = split_origin(target);

    // Strip query params
    let (path, query_params) = match path_and_query.split_once('?') {
        Some((p, q)) => (p, Some(q)),
        None => (path_and_query, None),
    };

    // Collect query params as extracted values (capped to prevent unbounded allocation).
    // Each pair is heap-allocated via to_string() because NormalizedEvent.params
    // is Vec<String> throughout the pipeline (a Cow<str> backed by the source
    // would avoid that, but needs a larger refactor). Pre-size the Vec from the
    // ampersand count to avoid the doubling growth on the hot path.
    let mut params = match query_params {
        Some(q) => {
            let cap = (bytecount(q, b'&') + 1).min(100);
            let mut out = Vec::with_capacity(cap);
            for pair in q.split('&').take(100) {
                out.push(pair.to_string());
            }
            out
        }
        None => Vec::new(),
    };

    let mask_codes = is_url_like(target);
    let normalized_path = normalize_path_segments(path, &mut params, mask_codes);

    // `normalized_path` starts with `/` whenever an authority was present
    // (it is sliced from the authority's trailing `/`), so a DNS host slots
    // in as `GET host/path` with no extra separator handling.
    let template = match authority.and_then(host_group_prefix) {
        Some(host) => format!("{method} {host}{normalized_path}"),
        None => format!("{method} {normalized_path}"),
    };
    HttpNormalized { template, params }
}

/// Whether `target` is a URL or a URL path rather than an RPC
/// `service/method` (see `segment_placeholder`): a leading `/`, a
/// `scheme://` in any case, or a `host:port` first segment.
fn is_url_like(target: &str) -> bool {
    let first = target.split('/').next().unwrap_or_default();
    first.is_empty()
        || first.ends_with(':')
        || first.rsplit_once(':').is_some_and(|(host, port)| {
            !host.is_empty() && !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit())
        })
}

/// Re-apply the current path masking to the template of an HTTP finding
/// read from an older report (`GET svc/api/dossiers/K7QX2P`). `None` when
/// nothing changes, or for an RPC template, whose first token is a
/// lower-case `rpc.system` rather than an HTTP method, or for a SQL template
/// (a fan-out's), which has more than one word after its verb or no `/`.
///
/// Only the segments past the first `/` are read. What precedes it, a kept
/// DNS host or a scheme `split_origin` leaves in the path (`ws:`), stays as
/// written.
pub(crate) fn remask_http_template(template: &str) -> Option<String> {
    let (method, rest) = template.split_once(' ')?;
    if method.is_empty()
        || !method.bytes().all(|b| b.is_ascii_uppercase() || b == b'_')
        || rest.contains(char::is_whitespace)
    {
        return None;
    }
    let path_start = rest.find('/')?;
    let masked = mask_path(&rest[path_start..])?;
    Some(format!("{method} {}{masked}", &rest[..path_start]))
}

/// Normalize path segments: replace each identifier-like segment with the
/// placeholder [`segment_placeholder`] gives it.
fn normalize_path_segments(path: &str, params: &mut Vec<String>, mask_codes: bool) -> String {
    if path.is_empty() || path == "/" {
        return "/".to_string();
    }
    let mut result = String::with_capacity(path.len() + 8);
    for (idx, seg) in path.split('/').enumerate() {
        if idx > 0 {
            result.push('/');
        }
        if let Some(placeholder) = segment_placeholder(seg, mask_codes) {
            params.push(seg.to_string());
            result.push_str(placeholder);
        } else {
            result.push_str(seg);
        }
    }
    result
}

/// Mask identifier-like path segments of a `source.endpoint` in place.
///
/// Without `http.route` the endpoint is the raw request path, and each
/// `/dossiers/42` would mint its own ack signature and ledger entry. Only the
/// path is touched: after a URL authority, after a `METHOD ` prefix, or the
/// whole endpoint when it starts with `/`. Called on the raw URL fallback
/// (`event::raw_url_endpoint`) and by the readers of an older report or ack
/// entry, never on an authoritative endpoint such as an `http.route`.
pub(crate) fn mask_endpoint_ids(endpoint: &mut String) {
    // Past a `METHOD ` prefix, so `GET //host/x` reads like `//host/x`. A
    // space further in (`http://svc/a b /42`) belongs to the URL.
    let url_start = endpoint
        .find(" /")
        .filter(|&i| !endpoint[..i].contains('/'))
        .map_or(0, |i| i + 1);
    let url = &endpoint[url_start..];
    let path_start = match crate::event::endpoint_authority_start(url) {
        Some(a) => url[a..].find('/').map(|j| a + j),
        None => url.starts_with('/').then_some(0),
    };
    let Some(start) = path_start.map(|p| url_start + p) else {
        return;
    };
    if let Some(masked) = mask_path(&endpoint[start..]) {
        endpoint.replace_range(start.., &masked);
    }
}

/// An owned copy of `endpoint` with [`mask_endpoint_ids`] applied, for the
/// readers of an older report, archive or acknowledgment entry.
pub(crate) fn masked_endpoint(endpoint: &str) -> String {
    let mut masked = endpoint.to_string();
    mask_endpoint_ids(&mut masked);
    masked
}

/// `path` with its identifier-like segments masked, or `None` when it has
/// none. One walk, and no allocation until the first match.
fn mask_path(path: &str) -> Option<String> {
    let mut out: Option<String> = None;
    let mut seg_start = 0;
    for seg in path.split('/') {
        let placeholder = segment_placeholder(seg, true);
        if let Some(out) = out.as_mut() {
            out.push('/');
            out.push_str(placeholder.unwrap_or(seg));
        } else if let Some(placeholder) = placeholder {
            let mut masked = String::with_capacity(path.len() + 8);
            masked.push_str(&path[..seg_start]);
            masked.push_str(placeholder);
            out = Some(masked);
        }
        seg_start += seg.len() + 1;
    }
    out
}

/// Split scheme + authority from an `http(s)` URL. Returns
/// `(authority, path_and_query)`: `authority` is `None` for a relative URL
/// (no scheme), and `path_and_query` defaults to `/` when the URL has an
/// authority but no path (`http://host`).
fn split_origin(target: &str) -> (Option<&str>, &str) {
    match target
        .strip_prefix("http://")
        .or_else(|| target.strip_prefix("https://"))
    {
        // RFC 3986: the authority ends at the first '/', '?' or '#'.
        // Terminating only on '/' would fold a query or fragment into the
        // authority, leaking it (verbatim, secrets included) into the
        // grouping template on a URL with no path (`http://host?token=...`).
        // A '#' terminator means there is no path (fragments never carry a
        // path), and the fragment is never sent to the server, so the path
        // is just `/`. A fragment that follows a path is left in the path
        // unchanged (it is handled by `normalize_path_segments`).
        Some(rest) => match rest.find(['/', '?', '#']) {
            Some(idx) if rest.as_bytes()[idx] == b'#' => (Some(&rest[..idx]), "/"),
            Some(idx) => (Some(&rest[..idx]), &rest[idx..]),
            None => (Some(rest), "/"),
        },
        None => (None, target),
    }
}

/// The DNS host to keep in the grouping template, or `None` when the
/// authority is an IP literal (kept anonymous so load-balanced replicas
/// still dedup) or empty. Strips RFC 3986 userinfo and the port, drops a
/// single trailing DNS root dot (`svc.` == `svc`), and lowercases the host
/// (DNS is case-insensitive) so casing variants group.
fn host_group_prefix(authority: &str) -> Option<Cow<'_, str>> {
    // Strip userinfo: "user:pass@host:port" -> "host:port". Safe because
    // split_origin already trimmed any query/fragment (which may contain
    // '@'), so the remaining '@' can only be the userinfo delimiter.
    let host_port = authority.rsplit('@').next().unwrap_or(authority);
    // IPv6 literal ("[::1]:8080"): always an address, drop it.
    if host_port.starts_with('[') {
        return None;
    }
    // Strip the port: "host:port" -> "host", then the DNS root dot.
    let host = host_port.split(':').next().unwrap_or(host_port);
    let host = host.strip_suffix('.').unwrap_or(host);
    if host.is_empty() || is_ipv4_literal(host) {
        return None;
    }
    if host.bytes().any(|b| b.is_ascii_uppercase()) {
        Some(Cow::Owned(host.to_ascii_lowercase()))
    } else {
        Some(Cow::Borrowed(host))
    }
}

/// Whether `host` is a dotted-decimal IPv4 literal (exactly four all-digit
/// octets). Such authorities are load-balanced replica addresses, so they
/// are dropped from the template while DNS hostnames are kept. Counts in
/// `usize` and bails past four labels so a host with hundreds of numeric
/// dot-labels cannot overflow the counter.
fn is_ipv4_literal(host: &str) -> bool {
    let mut octets = 0usize;
    for part in host.split('.') {
        if part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()) {
            return false;
        }
        octets += 1;
        if octets > 4 {
            return false;
        }
    }
    octets == 4
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simple_path_with_numeric_id() {
        let r = normalize_http("GET", "/api/orders/42/submit");
        assert_eq!(r.template, "GET /api/orders/{id}/submit");
        assert_eq!(r.params, vec!["42"]);
    }

    #[test]
    fn uuid_segment() {
        let r = normalize_http("GET", "/api/users/a1b2c3d4-e5f6-7890-abcd-ef1234567890");
        assert_eq!(r.template, "GET /api/users/{uuid}");
        assert_eq!(r.params, vec!["a1b2c3d4-e5f6-7890-abcd-ef1234567890"]);
    }

    #[test]
    fn full_url_keeps_dns_host() {
        // The DNS host stays in the template (and the port is dropped) so
        // calls to the same path on different backends stay distinct groups.
        let r = normalize_http("GET", "http://user-svc:5000/api/users/user-123");
        assert_eq!(r.template, "GET user-svc/api/users/user-123");
    }

    #[test]
    fn query_params_stripped() {
        let r = normalize_http("GET", "/api/users?page=2&size=10");
        assert_eq!(r.template, "GET /api/users");
        assert_eq!(r.params, vec!["page=2", "size=10"]);
    }

    #[test]
    fn full_url_with_query() {
        let r = normalize_http("POST", "https://svc.internal/api/items/99?expand=true");
        assert_eq!(r.template, "POST svc.internal/api/items/{id}");
        assert_eq!(r.params, vec!["expand=true", "99"]);
    }

    #[test]
    fn multiple_numeric_segments() {
        let r = normalize_http("DELETE", "/api/orders/42/items/7");
        assert_eq!(r.template, "DELETE /api/orders/{id}/items/{id}");
        assert_eq!(r.params, vec!["42", "7"]);
    }

    #[test]
    fn upper_case_alphanumeric_codes_become_ids() {
        // One signature per dossier number flooded a production ledger.
        let a = normalize_http("GET", "https://crm.example.test/api/dossiers/K7QX2P");
        let b = normalize_http("GET", "https://crm.example.test/api/dossiers/R4TN8W");
        assert_eq!(a.template, "GET crm.example.test/api/dossiers/{id}");
        assert_eq!(a.template, b.template);
        assert_eq!(a.params, vec!["K7QX2P"]);
    }

    #[test]
    fn alphanumeric_code_shapes() {
        let code = |s: &str| segment_placeholder(s, true);
        assert_eq!(code("A1B2"), Some("{id}"));
        assert_eq!(code("INV2024001"), Some("{id}"));
        // No upper bound, like a numeric segment.
        assert_eq!(code(&format!("A{}", "1B".repeat(100))), Some("{id}"));
        assert_eq!(code("A1B"), None);
        assert_eq!(code("ORD12345"), None);
        assert_eq!(code("K7qx2P"), None);
        assert_eq!(segment_placeholder("K7QX2P", false), None);
    }

    #[test]
    fn fixed_words_with_a_number_stay() {
        for word in [
            "UTF8", "H264", "H265", "X509", "IPV6", "HTTP2", "SHA256", "ISO8601", "ED25519",
            "ISO27001", "1080P", "WAFV2",
        ] {
            let path = format!("/api/{word}/items");
            let r = normalize_http("GET", &path);
            assert_eq!(r.template, format!("GET {path}"), "{word}");
        }
    }

    #[test]
    fn hex_ids_become_ids_in_any_case() {
        for (seg, kept) in [
            ("507f1f77bcf86cd799439011", false),
            ("507F1F77BCF86CD799439011", false),
            ("a1b2c3d4e5f67890abcdef1234567890", false),
            ("A1B2C3D4E5F67890ABCDEF1234567890", false),
            ("da39a3ee5e6b4b0d3255bfef95601890afd80709", false),
            ("deadbeef1234567", true),
            ("abcdefabcdefabcdef", true),
        ] {
            let r = normalize_http("GET", &format!("/api/items/{seg}"));
            let expected = if kept {
                format!("GET /api/items/{seg}")
            } else {
                "GET /api/items/{id}".to_string()
            };
            assert_eq!(r.template, expected, "{seg}");
        }
    }

    #[test]
    fn url_targets_mask_codes_whatever_their_scheme() {
        for target in [
            "HTTP://svc/api/dossiers/K7QX2P",
            "ws://svc/api/dossiers/K7QX2P",
            "order-svc:8080/api/dossiers/K7QX2P",
        ] {
            let r = normalize_http("GET", target);
            assert!(
                r.template.ends_with("/dossiers/{id}"),
                "{target}: {}",
                r.template
            );
        }
    }

    #[test]
    fn older_http_templates_are_remasked() {
        assert_eq!(
            remask_http_template("GET crm.example.test/api/dossiers/K7QX2P").as_deref(),
            Some("GET crm.example.test/api/dossiers/{id}")
        );
        assert_eq!(
            remask_http_template("GET /api/items/507f1f77bcf86cd799439011").as_deref(),
            Some("GET /api/items/{id}")
        );
        assert_eq!(remask_http_template("GET /api/orders/{id}"), None);
        assert_eq!(remask_http_template("GET svc/encoding/UTF8"), None);
        assert_eq!(remask_http_template("aws-api WAFV2/ListWebACLs"), None);
        assert_eq!(remask_http_template("GET ws://svc/api/dossiers/{id}"), None);
        assert_eq!(remask_http_template("GET HTTP://svc/api/x"), None);
        // Masking only: a first segment is never re-read as a host.
        assert_eq!(remask_http_template("GET 10.0.0.1/api/x"), None);
        assert_eq!(remask_http_template("GET Order-Svc/api/x"), None);
        assert_eq!(remask_http_template("SELECT * FROM t WHERE id = ?"), None);
        assert_eq!(remask_http_template("SHOW TABLES"), None);
    }

    #[test]
    fn rpc_targets_keep_upper_case_names() {
        let r = normalize_http("aws-api", "WAFV2/ListWebACLs");
        assert_eq!(r.template, "aws-api WAFV2/ListWebACLs");
        let r = normalize_http("grpc", "K7QX2PService/Get");
        assert_eq!(r.template, "grpc K7QX2PService/Get");
        // Numeric and UUID masking still apply, as before.
        let r = normalize_http("grpc", "svc/42");
        assert_eq!(r.template, "grpc svc/{id}");
    }

    #[test]
    fn endpoint_ids_are_masked() {
        for (raw, expected) in [
            ("/api/dossiers/K7QX2P", "/api/dossiers/{id}"),
            ("GET /api/orders/42/submit", "GET /api/orders/{id}/submit"),
            (
                "http://svc/api/items/507f1f77bcf86cd799439011",
                "http://svc/api/items/{id}",
            ),
            ("/api/orders/{id}", "/api/orders/{id}"),
            // A `://` inside the path is no scheme, and a `//host` is kept.
            (
                "/api/orders/42/cb/http://x/7",
                "/api/orders/{id}/cb/http://x/{id}",
            ),
            ("//ABC123DEF/api/42", "//ABC123DEF/api/{id}"),
            ("GET //ABC123DEF/api/42", "GET //ABC123DEF/api/{id}"),
            ("GET http://svc/a/42/b", "GET http://svc/a/{id}/b"),
            // A space inside the URL starts no method prefix.
            ("http://svc/a/7/b c /42", "http://svc/a/{id}/b c /{id}"),
            ("/42", "/{id}"),
            ("/a/42/", "/a/{id}/"),
            ("activemq /queue/INV2024001", "activemq /queue/{id}"),
            ("/encoding/UTF8", "/encoding/UTF8"),
            ("com.acme.Job.run", "com.acme.Job.run"),
            ("kafka orders", "kafka orders"),
            ("unknown", "unknown"),
        ] {
            let mut endpoint = raw.to_string();
            mask_endpoint_ids(&mut endpoint);
            assert_eq!(endpoint, expected, "{raw}");
        }
    }

    #[test]
    fn path_words_and_short_or_letter_only_segments_stay() {
        for path in [
            "/api/v2/items",
            "/oauth2/token",
            "/hash/sha256",
            "/export/CSV",
            "/api/A1B",
            "/files/AB12-CD",
        ] {
            let r = normalize_http("GET", path);
            assert_eq!(r.template, format!("GET {path}"), "{path}");
            assert!(r.params.is_empty(), "{path}");
        }
    }

    #[test]
    fn root_path() {
        let r = normalize_http("GET", "/");
        assert_eq!(r.template, "GET /");
        assert_eq!(r.params, Vec::<String>::new());
    }

    #[test]
    fn no_numeric_or_uuid_segments() {
        let r = normalize_http("GET", "/api/health");
        assert_eq!(r.template, "GET /api/health");
        assert_eq!(r.params, Vec::<String>::new());
    }

    #[test]
    fn port_in_url_not_treated_as_id() {
        // Host kept, port dropped, and the port digits are not an {id}.
        let r = normalize_http("GET", "http://localhost:8080/api/items");
        assert_eq!(r.template, "GET localhost/api/items");
    }

    #[test]
    fn url_without_path_keeps_host() {
        let r = normalize_http("GET", "http://example.com");
        assert_eq!(r.template, "GET example.com/");
        assert_eq!(r.params, Vec::<String>::new());
    }

    #[test]
    fn https_url_without_path() {
        let r = normalize_http("GET", "https://example.com");
        assert_eq!(r.template, "GET example.com/");
    }

    #[test]
    fn dns_hosts_disambiguate_same_path() {
        // Same method + path on two DNS backends must NOT collapse into
        // one template (which would raise a false redundant).
        let a = normalize_http("POST", "http://ms-23205/vs2nqhh1hq");
        let b = normalize_http("POST", "http://ms-53745/vs2nqhh1hq");
        assert_eq!(a.template, "POST ms-23205/vs2nqhh1hq");
        assert_eq!(b.template, "POST ms-53745/vs2nqhh1hq");
        assert_ne!(a.template, b.template);
    }

    #[test]
    fn ipv4_hosts_are_dropped_keeping_replica_dedup() {
        // Load-balanced pod replicas share a service. Their IP authorities
        // must collapse to one template so replica calls still group together.
        let a = normalize_http("GET", "http://10.0.0.1:8080/api/x");
        let b = normalize_http("GET", "http://10.0.0.2:8080/api/x");
        assert_eq!(a.template, "GET /api/x");
        assert_eq!(a.template, b.template);
    }

    #[test]
    fn ipv6_host_is_dropped() {
        let r = normalize_http("GET", "http://[2001:db8::1]:8080/api/x");
        assert_eq!(r.template, "GET /api/x");
    }

    #[test]
    fn host_is_lowercased() {
        let r = normalize_http("GET", "http://User-SVC.Example.COM/api/x");
        assert_eq!(r.template, "GET user-svc.example.com/api/x");
    }

    #[test]
    fn userinfo_is_stripped_from_host() {
        let r = normalize_http("GET", "http://user:pass@svc.internal/api/x");
        assert_eq!(r.template, "GET svc.internal/api/x");
    }

    #[test]
    fn relative_url_has_no_host() {
        // No authority to key on, so no host prefix.
        let r = normalize_http("GET", "/api/x");
        assert_eq!(r.template, "GET /api/x");
    }

    #[test]
    fn query_only_url_does_not_leak_into_host() {
        // Regression: a query on a path-less URL must not fold into the
        // authority and leak (e.g. a token) verbatim into the template.
        let r = normalize_http("GET", "http://api.example.com?token=abc123secret");
        assert_eq!(r.template, "GET api.example.com/");
        assert!(!r.template.contains("token"), "{}", r.template);
    }

    #[test]
    fn query_with_userinfo_does_not_leak() {
        let r = normalize_http("GET", "http://user:pass@svc.internal?token=xyz");
        assert_eq!(r.template, "GET svc.internal/");
        assert!(!r.template.contains("token"), "{}", r.template);
    }

    #[test]
    fn fragment_only_url_does_not_pollute_host() {
        // A path-less fragment is never sent to the server, so it is dropped
        // and must not become part of the host token.
        let r = normalize_http("GET", "http://svc.internal#section");
        assert_eq!(r.template, "GET svc.internal/");
    }

    #[test]
    fn trailing_dns_dot_groups_with_bare_host() {
        // `svc.` (DNS root label) and `svc` are the same host.
        let dotted = normalize_http("GET", "http://user-svc./api/x");
        let bare = normalize_http("GET", "http://user-svc/api/x");
        assert_eq!(dotted.template, "GET user-svc/api/x");
        assert_eq!(dotted.template, bare.template);
    }

    #[test]
    fn pathological_numeric_host_does_not_overflow() {
        // 260 all-numeric dot-labels must not overflow the octet counter
        // (debug panic) nor wrap-classify as an IPv4 literal.
        let host = vec!["1"; 260].join(".");
        let r = normalize_http("GET", &format!("http://{host}/x"));
        // Not four octets, so it is treated as a DNS host and kept.
        assert_eq!(r.template, format!("GET {host}/x"));
        assert!(is_ipv4_literal("1.2.3.4"));
        assert!(!is_ipv4_literal(&host));
    }

    #[test]
    fn non_uuid_36_char_segment_not_replaced() {
        // 36 chars but not a valid UUID format
        let r = normalize_http("GET", "/api/users/abcdefghijklmnopqrstuvwxyz1234567890");
        assert_eq!(
            r.template,
            "GET /api/users/abcdefghijklmnopqrstuvwxyz1234567890"
        );
        assert_eq!(r.params, Vec::<String>::new());
    }

    #[test]
    fn empty_path() {
        let r = normalize_http("GET", "");
        assert_eq!(r.template, "GET /");
    }

    #[test]
    fn trailing_slash() {
        let r = normalize_http("GET", "/api/users/");
        assert_eq!(r.template, "GET /api/users/");
        assert_eq!(r.params, Vec::<String>::new());
    }

    #[test]
    fn single_numeric_segment() {
        let r = normalize_http("GET", "/42");
        assert_eq!(r.template, "GET /{id}");
        assert_eq!(r.params, vec!["42"]);
    }

    #[test]
    fn mixed_uuid_and_numeric() {
        let r = normalize_http(
            "PUT",
            "/api/org/a1b2c3d4-e5f6-7890-abcd-ef1234567890/user/99",
        );
        assert_eq!(r.template, "PUT /api/org/{uuid}/user/{id}");
        assert_eq!(r.params, vec!["a1b2c3d4-e5f6-7890-abcd-ef1234567890", "99"]);
    }

    #[test]
    fn is_uuid_valid() {
        assert!(is_uuid("a1b2c3d4-e5f6-7890-abcd-ef1234567890"));
        assert!(is_uuid("00000000-0000-0000-0000-000000000000"));
        assert!(is_uuid("AAAABBBB-CCCC-DDDD-EEEE-FFFFFFFFFFFF"));
    }

    #[test]
    fn is_uuid_invalid() {
        assert!(!is_uuid("not-a-uuid-at-all"));
        assert!(!is_uuid("")); // too short
        assert!(!is_uuid("a1b2c3d4-e5f6-7890-abcd-ef123456789")); // 35 chars
        assert!(!is_uuid("a1b2c3d4-e5f6-7890-abcd-ef12345678901")); // 37 chars
        assert!(!is_uuid("a1b2c3d4xe5f6-7890-abcd-ef1234567890")); // wrong dash pos
        assert!(!is_uuid("g1b2c3d4-e5f6-7890-abcd-ef1234567890")); // 'g' not hex
    }

    #[test]
    fn uppercase_uuid_detected() {
        let r = normalize_http("GET", "/api/item/A1B2C3D4-E5F6-7890-ABCD-EF1234567890");
        assert_eq!(r.template, "GET /api/item/{uuid}");
    }

    // -- Fragment handling --

    #[test]
    fn fragment_not_stripped_from_path() {
        // Fragments are rare in server-side URLs. The segment "42#section" is not
        // purely numeric so it passes through as-is (fragment is not separated)
        let r = normalize_http("GET", "/api/users/42#section");
        assert_eq!(r.template, "GET /api/users/42#section");
    }

    // -- Malformed/edge-case query params --

    #[test]
    fn trailing_question_mark_only() {
        let r = normalize_http("GET", "/api/users?");
        assert_eq!(r.template, "GET /api/users");
        assert_eq!(r.params, vec![""]);
    }

    #[test]
    fn empty_query_param_values() {
        let r = normalize_http("GET", "/api/users?id=&name=");
        assert_eq!(r.template, "GET /api/users");
        assert_eq!(r.params, vec!["id=", "name="]);
    }

    #[test]
    fn double_ampersand_in_query() {
        let r = normalize_http("GET", "/api/users?a=1&&b=2");
        assert_eq!(r.template, "GET /api/users");
        assert_eq!(r.params, vec!["a=1", "", "b=2"]);
    }

    // -- Double slashes --

    #[test]
    fn double_slash_in_path_preserved() {
        let r = normalize_http("GET", "/api//users/42");
        assert_eq!(r.template, "GET /api//users/{id}");
    }

    // -- URL-encoded segments (pass through as-is) --

    #[test]
    fn url_encoded_numeric_not_detected() {
        // %34%32 = "42" but URL-encoded, not decoded before detection
        let r = normalize_http("GET", "/api/users/%34%32");
        assert_eq!(r.template, "GET /api/users/%34%32");
        assert_eq!(r.params, Vec::<String>::new());
    }

    // -- Query params capped at 100 --

    #[test]
    fn query_params_capped_at_100() {
        let params: Vec<String> = (0..200).map(|i| format!("p{i}={i}")).collect();
        let url = format!("/api/test?{}", params.join("&"));
        let r = normalize_http("GET", &url);
        assert_eq!(r.params.len(), 100);
    }
}
