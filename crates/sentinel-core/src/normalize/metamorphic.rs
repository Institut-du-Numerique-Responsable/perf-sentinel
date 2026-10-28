//! Metamorphic property tests for SQL normalization.
//!
//! Same approach as `detect::metamorphic`: assert *relations* between
//! normalization runs on transformed inputs instead of expected outputs,
//! so the tokenizer is exercised on thousands of generated statements
//! without a hand-labeled corpus:
//!
//! - **idempotence**: a produced template is a fixed point,
//!   `normalize(normalize(x).template)` changes nothing and extracts
//!   no params
//! - **literal invariance**: two statements differing only in literal
//!   values (including IN-list arity) share one template
//! - **containment**: params are extracted exactly, in encounter order,
//!   and no literal content survives in the template
//! - **round trip**: putting each literal back in place of its `?`
//!   rebuilds the original statement byte for byte, so the tokenizer
//!   never drops or alters the text between literals

use proptest::prelude::*;

use super::sql::{normalize_sql, normalize_sql_for};

/// Marker stamped on every generated string literal so a leak into the
/// template is detectable by substring search (generated skeletons
/// never contain it, and no literal is empty).
const MARKER: &str = "zzq";

/// One SQL literal: its source text and the param `normalize_sql` must
/// extract for it.
#[derive(Debug, Clone)]
struct Lit {
    sql: String,
    param: String,
}

fn int_lit() -> impl Strategy<Value = Lit> {
    (0u64..=99_999_999).prop_map(|n| Lit {
        sql: n.to_string(),
        param: n.to_string(),
    })
}

fn float_lit() -> impl Strategy<Value = Lit> {
    (0u32..=9999, 0u32..=99).prop_map(|(int_part, frac_part)| {
        let s = format!("{int_part}.{frac_part}");
        Lit {
            sql: s.clone(),
            param: s,
        }
    })
}

/// Quoted string literal. Content mixes letters, digits, spaces,
/// backslashes, and single quotes. Quotes are escaped as `''` in the
/// SQL text and must come back unescaped in the param. Backslashes are
/// doubled, read as one escape or as two plain characters depending on
/// the dialect, and kept as written in the param either way.
fn str_lit() -> impl Strategy<Value = Lit> {
    raw_str().prop_map(|raw| {
        let written = raw.replace('\\', "\\\\");
        Lit {
            sql: format!("'{}'", written.replace('\'', "''")),
            param: written,
        }
    })
}

/// `PostgreSQL` standard string: a backslash is an ordinary character, so
/// a lone one (even right before the closing quote) is carried as is.
fn pg_str_lit() -> impl Strategy<Value = Lit> {
    raw_str().prop_map(|raw| Lit {
        sql: format!("'{}'", raw.replace('\'', "''")),
        param: raw,
    })
}

/// String whose quotes are backslash-escaped (`\'`), the `MySQL` and
/// unknown-dialect spelling. Only an escaping reading closes it in the
/// right place.
fn bs_str_lit() -> impl Strategy<Value = Lit> {
    raw_str().prop_map(|raw| {
        let written = raw.replace('\\', "\\\\").replace('\'', "\\'");
        Lit {
            sql: format!("'{written}'"),
            param: written,
        }
    })
}

fn raw_str() -> impl Strategy<Value = String> {
    proptest::collection::vec(
        prop_oneof![
            Just('a'),
            Just('B'),
            Just('7'),
            Just(' '),
            Just('\\'),
            Just('\''),
            Just('-'),
        ],
        0..8,
    )
    .prop_map(|chars| format!("{MARKER}{}", chars.into_iter().collect::<String>()))
}

/// `MySQL` double-quoted string literal. A `"` is escaped as `""` in the SQL
/// text, a single quote is carried as is. A string only in `MySQL` mode.
fn dq_str_lit() -> impl Strategy<Value = Lit> {
    proptest::collection::vec(
        prop_oneof![Just('a'), Just('7'), Just(' '), Just('\''), Just('"')],
        0..8,
    )
    .prop_map(|chars| {
        let raw = format!("{MARKER}{}", chars.into_iter().collect::<String>());
        Lit {
            sql: format!("\"{}\"", raw.replace('"', "\"\"")),
            param: raw,
        }
    })
}

/// Hex (`0x1F`) or exponent (`2.5e-3`) literal: one param, never split.
fn prefixed_num_lit() -> impl Strategy<Value = Lit> {
    prop_oneof![
        any::<u32>().prop_map(|n| format!("0x{n:X}")),
        (
            0u32..=99,
            prop_oneof![Just("e"), Just("E-"), Just("e+")],
            0u32..=99
        )
            .prop_map(|(m, e, x)| format!("{m}.5{e}{x}")),
    ]
    .prop_map(|s| Lit {
        sql: s.clone(),
        param: s,
    })
}

fn any_lit() -> impl Strategy<Value = Lit> {
    prop_oneof![int_lit(), float_lit(), prefixed_num_lit(), str_lit()]
}

/// Cosmetic dimensions of a statement (keyword casing, whitespace,
/// table name). Held constant when comparing two literal sets: the
/// tokenizer preserves casing and whitespace verbatim, so only the
/// literals may vary between the compared statements.
#[derive(Debug, Clone)]
struct Shape {
    select: &'static str,
    r#in: &'static str,
    ws: &'static str,
    table: &'static str,
}

fn shape() -> impl Strategy<Value = Shape> {
    (
        prop_oneof![Just("SELECT"), Just("select"), Just("Select")],
        prop_oneof![Just("IN"), Just("in")],
        prop_oneof![Just(" "), Just("  "), Just("\t")],
        prop_oneof![Just("users"), Just("order_items")],
    )
        .prop_map(|(select, r#in, ws, table)| Shape {
            select,
            r#in,
            ws,
            table,
        })
}

/// Render one statement:
/// `SELECT * FROM t WHERE id = <lit> AND name IN (<list>) AND note = <str>`.
fn render(shape: &Shape, eq_lit: &Lit, in_list: &[Lit], note: &Lit) -> String {
    let list = in_list
        .iter()
        .map(|l| l.sql.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let w = shape.ws;
    format!(
        "{sel}{w}*{w}FROM{w}{table}{w}WHERE{w}id{w}={w}{eq}{w}AND{w}name{w}{inkw}{w}({list}){w}AND{w}note = {note}",
        sel = shape.select,
        table = shape.table,
        eq = eq_lit.sql,
        inkw = shape.r#in,
        note = note.sql,
    )
}

/// Render a flat statement with one column per literal and no `IN`
/// list: `SELECT * FROM t WHERE (ca = <lit>) AND (cb = <lit>) ...`. The
/// `IN (?)` collapse rewrites whitespace and list arity, so the round
/// trip only holds on statements it leaves alone. The `)` puts a
/// non-space byte right after each literal, the last one included.
fn render_flat(shape: &Shape, lits: &[Lit]) -> String {
    let w = shape.ws;
    let predicates = lits
        .iter()
        .zip(b'a'..)
        .map(|(lit, col)| format!("(c{col}{w}={w}{sql})", col = char::from(col), sql = lit.sql))
        .collect::<Vec<_>>()
        .join(&format!("{w}AND{w}"));
    format!(
        "{sel}{w}*{w}FROM{w}{table}{w}WHERE{w}{predicates}",
        sel = shape.select,
        table = shape.table,
    )
}

/// Literals paired with the `db.system` that reads each of them as one
/// literal: backslash-escaped strings on an unnamed engine and on
/// `MySQL`, double-quoted strings on `MySQL` only, standard strings with a
/// lone backslash on `PostgreSQL` only. Doubled backslashes read on all three.
fn dialect_lits() -> impl Strategy<Value = (&'static str, Vec<Lit>)> {
    prop_oneof![
        prop::collection::vec(prop_oneof![any_lit(), bs_str_lit()], 1..6)
            .prop_map(|lits| ("", lits)),
        prop::collection::vec(prop_oneof![any_lit(), bs_str_lit(), dq_str_lit()], 1..6)
            .prop_map(|lits| ("mysql", lits)),
        prop::collection::vec(prop_oneof![any_lit(), pg_str_lit()], 1..6)
            .prop_map(|lits| ("postgresql", lits)),
    ]
}

type Statement = (Shape, Lit, Vec<Lit>, Lit);

fn statement() -> impl Strategy<Value = Statement> {
    (
        shape(),
        any_lit(),
        prop::collection::vec(any_lit(), 1..5),
        str_lit(),
    )
}

proptest! {
    /// Idempotence: normalizing a template is a no-op. Every literal was
    /// already collapsed to `?` (and `IN` lists to `IN (?)`), so a second
    /// pass must return the same template and extract nothing.
    #[test]
    fn normalization_is_idempotent((shape, eq, list, note) in statement()) {
        let first = normalize_sql(&render(&shape, &eq, &list, &note));
        let second = normalize_sql(&first.template);
        prop_assert_eq!(&second.template, &first.template);
        prop_assert!(
            second.params.is_empty(),
            "template re-extracted params: {:?}",
            second.params
        );
    }

    /// Literal invariance: two statements sharing a skeleton but with
    /// different literal values, types, and IN-list arities produce the
    /// same template. N+1 grouping by template depends on this property.
    #[test]
    fn literals_never_change_the_template(
        (shape, eq_a, list_a, note_a) in statement(),
        (eq_b, list_b, note_b) in (any_lit(), prop::collection::vec(any_lit(), 1..5), str_lit()),
    ) {
        let a = normalize_sql(&render(&shape, &eq_a, &list_a, &note_a));
        let b = normalize_sql(&render(&shape, &eq_b, &list_b, &note_b));
        prop_assert_eq!(a.template, b.template);
    }

    /// Containment: params come back exactly and in encounter order, and
    /// the template retains no literal content: no string marker, no
    /// quote, no digit (the skeleton is digit-free, so any digit in the
    /// template is a leaked literal).
    #[test]
    fn params_never_leak_into_the_template((shape, eq, list, note) in statement()) {
        let normalized = normalize_sql(&render(&shape, &eq, &list, &note));

        let expected: Vec<&str> = std::iter::once(&eq)
            .chain(&list)
            .chain(std::iter::once(&note))
            .map(|l| l.param.as_str())
            .collect();
        prop_assert_eq!(&normalized.params, &expected);

        prop_assert!(!normalized.template.contains(MARKER), "{}", normalized.template);
        prop_assert!(!normalized.template.contains('\''), "{}", normalized.template);
        prop_assert!(
            !normalized.template.bytes().any(|b| b.is_ascii_digit()),
            "digit leaked into template: {}",
            normalized.template
        );
    }

    /// `MySQL` mode: a double-quoted string is a literal like a quoted one.
    /// It comes back exactly, leaves nothing in the template, and the
    /// template stays a fixed point.
    #[test]
    fn mysql_double_quoted_strings_are_literals(
        (shape, eq, list, _) in statement(),
        note in dq_str_lit(),
    ) {
        let first = normalize_sql_for(&render(&shape, &eq, &list, &note), "mysql");

        let expected: Vec<&str> = std::iter::once(&eq)
            .chain(&list)
            .chain(std::iter::once(&note))
            .map(|l| l.param.as_str())
            .collect();
        prop_assert_eq!(&first.params, &expected);
        prop_assert!(!first.template.contains(MARKER), "{}", first.template);
        prop_assert!(!first.template.contains('"'), "{}", first.template);

        let second = normalize_sql_for(&first.template, "mysql");
        prop_assert_eq!(&second.template, &first.template);
        prop_assert!(second.params.is_empty(), "{:?}", second.params);
    }

    /// Unknown dialect and `MySQL` mode: a backslash-escaped quote stays
    /// inside its string, so nothing after it reaches the template.
    #[test]
    fn backslash_escaped_quotes_never_leak(
        (shape, eq, list, _) in statement(),
        note in bs_str_lit(),
        // `select`, not `prop_oneof!`: RustRover splits a nested macro's commas here (RsTypeCheck).
        db_system in prop::sample::select(vec!["", "mysql"]),
    ) {
        let first = normalize_sql_for(&render(&shape, &eq, &list, &note), db_system);

        let expected: Vec<&str> = std::iter::once(&eq)
            .chain(&list)
            .chain(std::iter::once(&note))
            .map(|l| l.param.as_str())
            .collect();
        prop_assert_eq!(&first.params, &expected);
        prop_assert!(!first.template.contains(MARKER), "{}", first.template);
        prop_assert!(!first.template.contains('\''), "{}", first.template);

        let second = normalize_sql_for(&first.template, db_system);
        prop_assert_eq!(&second.template, &first.template);
        prop_assert!(second.params.is_empty(), "{:?}", second.params);
    }

    /// `PostgreSQL` mode: a lone backslash does not escape the closing
    /// quote, so the string ends where the server ends it.
    #[test]
    fn postgres_backslash_is_an_ordinary_character(
        (shape, eq, list, _) in statement(),
        note in pg_str_lit(),
    ) {
        let first = normalize_sql_for(&render(&shape, &eq, &list, &note), "postgresql");

        let expected: Vec<&str> = std::iter::once(&eq)
            .chain(&list)
            .chain(std::iter::once(&note))
            .map(|l| l.param.as_str())
            .collect();
        prop_assert_eq!(&first.params, &expected);
        prop_assert!(!first.template.contains(MARKER), "{}", first.template);

        let second = normalize_sql_for(&first.template, "postgresql");
        prop_assert_eq!(&second.template, &first.template);
        prop_assert!(second.params.is_empty(), "{:?}", second.params);
    }

    /// Round trip: the template plus the literals rebuild the original
    /// statement exactly. Containment checks what comes out as params;
    /// this checks that everything else is copied through unchanged.
    #[test]
    fn template_and_literals_rebuild_the_statement(
        shape in shape(),
        (db_system, lits) in dialect_lits(),
    ) {
        let original = render_flat(&shape, &lits);
        let normalized = normalize_sql_for(&original, db_system);

        let expected: Vec<&str> = lits.iter().map(|l| l.param.as_str()).collect();
        prop_assert_eq!(&normalized.params, &expected);

        let parts: Vec<&str> = normalized.template.split('?').collect();
        prop_assert_eq!(parts.len(), lits.len() + 1, "{}", normalized.template);
        let mut rebuilt = String::from(parts[0]);
        for (lit, part) in lits.iter().zip(&parts[1..]) {
            rebuilt.push_str(&lit.sql);
            rebuilt.push_str(part);
        }
        prop_assert_eq!(rebuilt, original);
    }
}
