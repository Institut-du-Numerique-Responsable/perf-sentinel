//! Pre-decode element budget for OTLP trace exports.
//!
//! An empty `Span`, attribute, event or link costs two bytes on the wire and
//! a full struct once prost decodes it, so a byte cap alone lets one request
//! expand up to a hundred times in memory. This walk counts the
//! length-delimited fields of an encoded `ExportTraceServiceRequest` at
//! every depth, without allocating and without a schema, before prost
//! materializes anything.

/// Most spans one request may carry. The per-service span cap of the
/// conversion is the same figure, so a request this admits is never one the
/// conversion would have kept more of.
pub(crate) const MAX_SPANS_PER_REQUEST: usize = super::MAX_SPANS_PER_SERVICE;

/// Wire bytes per element the generic budget allows. Real exports run at
/// seven to ten bytes per length-delimited field (short attribute keys and
/// values), so four leaves them room while holding a flood of two-byte
/// empty messages to half its count.
const BYTES_PER_ELEMENT: usize = 4;

/// Nesting depth past which a request is refused. Real exports nest about
/// eight deep (an attribute holding a list of key-value lists), and the walk
/// recurses once per level.
const MAX_DEPTH: usize = 32;

/// Field number of the span chain at each depth:
/// `ExportTraceServiceRequest.resource_spans = 1`,
/// `ResourceSpans.scope_spans = 2`, `ScopeSpans.spans = 2`.
const SPAN_CHAIN: [u64; 3] = [1, 2, 2];

/// Limits of one request.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Budget {
    pub max_spans: usize,
    /// Length-delimited fields at any depth: messages, strings and bytes.
    pub max_elements: usize,
}

impl Budget {
    /// The budget for a request of at most `max_payload` decompressed bytes.
    pub(crate) fn for_payload(max_payload: usize) -> Self {
        Self {
            max_spans: MAX_SPANS_PER_REQUEST,
            max_elements: (max_payload / BYTES_PER_ELEMENT).max(MAX_SPANS_PER_REQUEST),
        }
    }
}

/// Why an encoded request is refused before decoding.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum BudgetError {
    /// More spans or elements than the budget.
    Exceeded,
    /// The framing does not parse, uses groups (which proto3 OTLP never
    /// sends), or nests past `MAX_DEPTH`. Refused here so trailing garbage
    /// cannot hide elements from the count while prost still decodes
    /// everything before it.
    Malformed,
}

/// Check `buf` against `budget`.
pub(crate) fn check(buf: &[u8], budget: Budget) -> Result<(), BudgetError> {
    let mut walk = Walk {
        budget,
        elements: 0,
        spans: 0,
    };
    walk.message(buf, 0, true).map_err(|stop| match stop {
        Stop::Exceeded => BudgetError::Exceeded,
        Stop::TooDeep | Stop::NotMessage => BudgetError::Malformed,
    })
}

enum Stop {
    Exceeded,
    TooDeep,
    /// The bytes do not parse as a message: a string or bytes field when
    /// met below the root, garbage at the root.
    NotMessage,
}

struct Walk {
    budget: Budget,
    elements: usize,
    spans: usize,
}

impl Walk {
    /// Walk `buf` as a message at `depth`. `on_chain`: the message is the
    /// request itself or one of its `ResourceSpans` or `ScopeSpans`.
    fn message(&mut self, mut buf: &[u8], depth: usize, on_chain: bool) -> Result<(), Stop> {
        if depth > MAX_DEPTH && !buf.is_empty() {
            return Err(Stop::TooDeep);
        }
        while !buf.is_empty() {
            let key = read_varint(&mut buf)?;
            match key & 7 {
                0 => {
                    read_varint(&mut buf)?;
                }
                1 => buf = buf.get(8..).ok_or(Stop::NotMessage)?,
                5 => buf = buf.get(4..).ok_or(Stop::NotMessage)?,
                2 => {
                    let len =
                        usize::try_from(read_varint(&mut buf)?).map_err(|_| Stop::NotMessage)?;
                    let (field, rest) = buf.split_at_checked(len).ok_or(Stop::NotMessage)?;
                    buf = rest;
                    let chain_child = on_chain && SPAN_CHAIN.get(depth) == Some(&(key >> 3));
                    self.length_delimited(field, depth + 1, chain_child)?;
                }
                _ => return Err(Stop::NotMessage),
            }
        }
        Ok(())
    }

    /// Count one length-delimited field, then what it holds when it parses
    /// as a message. A string or bytes field counts once: its trial walk is
    /// rolled back.
    fn length_delimited(
        &mut self,
        field: &[u8],
        depth: usize,
        chain_child: bool,
    ) -> Result<(), Stop> {
        self.elements += 1;
        if chain_child && depth == SPAN_CHAIN.len() {
            self.spans += 1;
        }
        if self.elements > self.budget.max_elements || self.spans > self.budget.max_spans {
            return Err(Stop::Exceeded);
        }
        let saved = (self.elements, self.spans);
        match self.message(field, depth, chain_child && depth < SPAN_CHAIN.len()) {
            Err(Stop::NotMessage) => {
                (self.elements, self.spans) = saved;
                Ok(())
            }
            other => other,
        }
    }
}

fn read_varint(buf: &mut &[u8]) -> Result<u64, Stop> {
    let mut value = 0u64;
    for shift in (0..64).step_by(7) {
        let (&byte, rest) = buf.split_first().ok_or(Stop::NotMessage)?;
        *buf = rest;
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Ok(value);
        }
    }
    Err(Stop::NotMessage)
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
    use opentelemetry_proto::tonic::common::v1::{AnyValue, KeyValue, any_value};
    use opentelemetry_proto::tonic::trace::v1::{ResourceSpans, ScopeSpans, Span, span};
    use prost::Message;

    /// `resource_spans { scope_spans { spans {} x n } }`, each empty span two bytes.
    pub(in crate::ingest::otlp) fn empty_spans_payload(n: usize) -> Vec<u8> {
        let spans: Vec<u8> = std::iter::repeat_n([0x12, 0x00], n).flatten().collect();
        let scope = [&[0x12][..], &varint(spans.len()), &spans].concat();
        [&[0x0a][..], &varint(scope.len()), &scope].concat()
    }

    fn varint(mut n: usize) -> Vec<u8> {
        let mut out = Vec::new();
        loop {
            let byte = (n & 0x7f) as u8;
            n >>= 7;
            if n == 0 {
                out.push(byte);
                return out;
            }
            out.push(byte | 0x80);
        }
    }

    fn real_request(spans_per_scope: usize) -> Vec<u8> {
        let attr = KeyValue {
            key: "db.statement".into(),
            value: Some(AnyValue {
                value: Some(any_value::Value::StringValue("SELECT 1".into())),
            }),
            ..Default::default()
        };
        let span = Span {
            trace_id: vec![1; 16],
            span_id: vec![2; 8],
            name: "q".into(),
            start_time_unix_nano: 1,
            end_time_unix_nano: 2,
            attributes: vec![attr.clone(); 3],
            events: vec![span::Event::default()],
            links: vec![span::Link::default()],
            ..Default::default()
        };
        let scope = ScopeSpans {
            spans: vec![span; spans_per_scope],
            schema_url: "s".into(),
            ..Default::default()
        };
        ExportTraceServiceRequest {
            resource_spans: vec![
                ResourceSpans {
                    scope_spans: vec![scope.clone(), scope],
                    ..Default::default()
                };
                2
            ],
        }
        .encode_to_vec()
    }

    fn wrap(tag: u8, inner: &[u8]) -> Vec<u8> {
        [&[tag][..], &varint(inner.len()), inner].concat()
    }

    #[test]
    fn counts_the_spans_of_a_real_request_and_admits_its_density() {
        // 2 resources, 4 scopes, 4 * 5 spans.
        let buf = real_request(5);
        assert_eq!(
            check(&buf, Budget::for_payload(buf.len())),
            Ok(()),
            "a real export fits the element budget of its own size"
        );
        let spans = |max_spans| Budget {
            max_spans,
            max_elements: usize::MAX,
        };
        assert_eq!(check(&buf, spans(20)), Ok(()));
        assert_eq!(check(&buf, spans(19)), Err(BudgetError::Exceeded));
    }

    #[test]
    fn many_empty_spans_exceed_the_budget_and_few_pass() {
        let budget = Budget::for_payload(16 * 1024 * 1024);
        let flood = empty_spans_payload(MAX_SPANS_PER_REQUEST + 1);
        assert_eq!(check(&flood, budget), Err(BudgetError::Exceeded));
        let small = empty_spans_payload(10);
        assert_eq!(check(&small, budget), Ok(()));
        ExportTraceServiceRequest::decode(small.as_slice()).expect("the fixture is valid OTLP");
    }

    #[test]
    fn one_span_with_many_empty_attributes_exceeds_the_budget() {
        let budget = Budget::for_payload(1024 * 1024);
        // `Span.attributes` is field 9: tag 0x4a, then an empty KeyValue.
        let attributes: Vec<u8> = std::iter::repeat_n([0x4a, 0x00], budget.max_elements)
            .flatten()
            .collect();
        let buf = wrap(0x0a, &wrap(0x12, &wrap(0x12, &attributes)));
        assert!(buf.len() <= 1024 * 1024, "under the byte cap");
        ExportTraceServiceRequest::decode(buf.as_slice()).expect("the fixture is valid OTLP");
        assert_eq!(check(&buf, budget), Err(BudgetError::Exceeded));
    }

    #[test]
    fn nesting_past_the_depth_limit_is_malformed() {
        let mut buf = wrap(0x0a, b"x");
        for _ in 0..=MAX_DEPTH {
            buf = wrap(0x0a, &buf);
        }
        assert_eq!(
            check(&buf, Budget::for_payload(1024)),
            Err(BudgetError::Malformed)
        );
    }

    #[test]
    fn garbage_and_groups_are_malformed() {
        let budget = Budget::for_payload(1024);
        let mut buf = empty_spans_payload(3);
        buf.push(0x0a); // a length-delimited field with no length
        assert_eq!(check(&buf, budget), Err(BudgetError::Malformed));
        assert_eq!(check(&[0x0b], budget), Err(BudgetError::Malformed)); // start group
        assert_eq!(check(&[0xff; 11], budget), Err(BudgetError::Malformed));
        assert_eq!(check(&[], budget), Ok(()));
    }
}
