// SPDX-License-Identifier: CC0-1.0

//! What `ResultSize` COUNTS: the bytes the warehouse returned for a statement's
//! result, driven over the wire — every call below is a real `POST /` with
//! `X-Amz-Target: RedshiftData.<Operation>` dispatched through the router.
//!
//! The rule under test: a result is returned to the client as one data row per
//! row of the result, each carrying the number of fields it holds (a two-byte
//! count) and then each value preceded by its own four-byte length, so the size a
//! statement reports is
//!
//! ```text
//! sum over returned rows of (2 + sum over that row's values of (4 + bytes of the value's text))
//! ```
//!
//! and the column names — described once, ahead of the rows, rather than in a
//! row — are no part of it. Every case below is one row of a table that varies
//! what the rule generalises over: how many columns a row carries, how many rows
//! the result has, which TYPE each value is and how many bytes its text takes,
//! and a result that returned no row at all. Each case is checked twice: against
//! the number counted by hand from the rule, and against the same number derived
//! from the rows the result reader actually serves, so the size reported and the
//! bytes a caller reads back cannot disagree.

use serde_json::{json, Value};

use super::emulator::{id_of, list, number, wait_for_terminal, Emulator};

/// The relation the sizing cases read: three columns of three different types,
/// so a case's size depends on the TEXT each value carries and not on the
/// storage class the engine kept it in.
const PARTS_DDL: &str = "CREATE TABLE parts (id INTEGER NOT NULL, label VARCHAR(16) NOT NULL, \
                         amount DECIMAL(8,2) NOT NULL)";
const PARTS_ROWS: &str = "INSERT INTO parts VALUES (1, 'bolt', 2.50), (22, 'washer', 10.75)";

/// What one row of a result costs beyond its values: the count of the fields it
/// carries.
const ROW_FRAMING_BYTES: usize = 2;

/// What one value costs beyond its text: the length that precedes it.
const FIELD_LENGTH_BYTES: usize = 4;

/// One row of the sizing table: the statements that prepare the relation, the
/// statement whose result is sized, and the size its record must report —
/// counted by hand from the rule.
struct SizeRow {
    label: &'static str,
    setup: &'static [&'static str],
    sql: &'static str,
    expected: i64,
}

/// The table: one row per shape of result the rule generalises over.
const ROWS: &[SizeRow] = &[
    SizeRow {
        label: "two columns of two rows",
        // `1`, `bolt` (1 + 4) and `22`, `washer` (2 + 6): two rows of two
        // values, so 2 * 2 bytes of row framing and 4 * 4 bytes of lengths.
        setup: &[PARTS_DDL, PARTS_ROWS],
        sql: "SELECT id, label FROM parts ORDER BY id",
        expected: 33,
    },
    SizeRow {
        label: "three columns of two rows, one of them an exact-numeric column",
        // `1`, `bolt`, `2.50` (1 + 4 + 4) and `22`, `washer`, `10.75`
        // (2 + 6 + 5): 2 * 2 bytes of framing and 6 * 4 bytes of lengths.
        setup: &[PARTS_DDL, PARTS_ROWS],
        sql: "SELECT id, label, amount FROM parts ORDER BY id",
        expected: 50,
    },
    SizeRow {
        label: "one column of one row",
        setup: &[],
        sql: "SELECT 1",
        expected: 7,
    },
    SizeRow {
        label: "five columns of one row, which costs four bytes more per column",
        setup: &[],
        sql: "SELECT 1 AS a, 2 AS b, 3 AS c, 4 AS d, 5 AS e",
        expected: 27,
    },
    SizeRow {
        label: "three rows of one column, which costs six bytes more per row",
        // `1`, `22` and `333`: 3 * 2 bytes of framing, 3 * 4 bytes of lengths
        // and 6 bytes of digits.
        setup: &[],
        sql: "SELECT 1 AS n UNION ALL SELECT 22 UNION ALL SELECT 333",
        expected: 24,
    },
    SizeRow {
        label: "a NULL, which carries its length and no text at all",
        setup: &[],
        sql: "SELECT 'x' AS a, NULL AS b",
        expected: 11,
    },
    SizeRow {
        label: "text counted in the bytes it takes, not the characters it spells",
        // `é` is one character of two bytes, so this value's text is 3 bytes.
        setup: &[],
        sql: "SELECT 'hé' AS t",
        expected: 9,
    },
    SizeRow {
        label: "an approximate-numeric value, which carries the digits it prints",
        setup: &[],
        sql: "SELECT 1.5 AS ratio",
        expected: 9,
    },
    SizeRow {
        label: "booleans, which carry the text they print",
        // `true` (4) and `false` (5), one row each.
        setup: &[
            "CREATE TABLE flags (flag BOOLEAN)",
            "INSERT INTO flags (flag) VALUES (1), (0)",
        ],
        sql: "SELECT flag FROM flags ORDER BY flag DESC",
        expected: 21,
    },
    SizeRow {
        label: "binary, which carries its bytes as hexadecimal",
        // Two bytes print as the four characters `6162`.
        setup: &[],
        sql: "SELECT x'6162' AS raw",
        expected: 10,
    },
    SizeRow {
        label: "an empty result set, which returned no row and so no byte",
        setup: &[PARTS_DDL, PARTS_ROWS],
        sql: "SELECT id FROM parts WHERE id < 0",
        expected: 0,
    },
];

/// `ResultSize` is the bytes the warehouse returned for the result: the row
/// framing and the value lengths of every row it sent, and the text each value
/// carries.
#[tokio::test]
async fn result_size_is_the_bytes_the_rows_were_returned_in() {
    for case in ROWS {
        // One emulator per case, so a case's relation is the only one its
        // statement can read.
        let emulator = Emulator::new();
        for statement in case.setup {
            emulator.run(statement).await;
        }
        let id = emulator.run(case.sql).await;
        let described = wait_for_terminal(&emulator, case.label, &id).await;
        assert_eq!(
            described["Status"], "FINISHED",
            "case {}: expected a FINISHED statement, got {described}",
            case.label
        );
        let reported = number(&described, "ResultSize");
        assert_eq!(
            reported, case.expected,
            "case {}: expected ResultSize {}, got {reported} in {described}",
            case.label, case.expected
        );

        // The same number, derived from the rows the reader serves: the size
        // reported and the result read back cannot disagree.
        let fetched = emulator
            .ok("GetStatementResult", &json!({"Id": id}))
            .await;
        let derived = returned_bytes(&list(&fetched, "Records"));
        assert_eq!(
            reported, derived,
            "case {}: ResultSize {reported} must be the {derived} bytes the {} row(s) the reader \
             serves were returned in, got {fetched}",
            case.label,
            list(&fetched, "Records").len()
        );
    }
}

/// Every reader of a statement's record reports that one number: a batch's
/// statements each report the size of what THEY returned, and the record read
/// back for one position of the batch reports the same.
#[tokio::test]
async fn a_batch_statement_reports_the_bytes_its_own_rows_were_returned_in() {
    let emulator = Emulator::new();
    let batched = emulator
        .batch_and_wait(&json!({
            "ClusterIdentifier": "my-cluster",
            "Database": "dev",
            "DbUser": "awsuser",
            "Sqls": [PARTS_DDL, PARTS_ROWS, "SELECT id, label FROM parts ORDER BY id"],
        }))
        .await;
    let id = id_of(&batched);
    let described = wait_for_terminal(&emulator, "a batch", &id).await;

    // The statements that returned no result set returned no bytes; the third
    // returned two rows of two values — 2 * 2 bytes of framing, 4 * 4 bytes of
    // lengths and the 13 bytes of `1`, `bolt`, `22` and `washer`.
    let expected = [0, 0, 33];
    let subs = list(&described, "SubStatements");
    assert_eq!(
        subs.len(),
        expected.len(),
        "one SubStatements entry per submitted statement, got {described}"
    );
    for (index, sub) in subs.iter().enumerate() {
        assert_eq!(
            number(sub, "ResultSize"),
            expected[index],
            "case statement {}: expected ResultSize {}, got {sub}",
            index + 1,
            expected[index]
        );
    }

    // The record read back for one POSITION of the batch reports the same size
    // for that statement as the batch's own list does. A position is addressed
    // by the 1-based suffix `SubStatementData.Id` documents.
    for (index, expected) in expected.iter().enumerate() {
        let addressed = emulator
            .ok(
                "DescribeStatement",
                &json!({"Id": format!("{id}:{}", index + 1)}),
            )
            .await;
        let narrowed = list(&addressed, "SubStatements");
        assert_eq!(
            narrowed.len(),
            1,
            "case statement {}: addressing one position reports that statement alone, got \
             {addressed}",
            index + 1
        );
        assert_eq!(
            number(&narrowed[0], "ResultSize"),
            *expected,
            "case statement {}: the addressed record must report ResultSize {expected}, got \
             {addressed}",
            index + 1
        );
    }
}

/// The rule, applied to the rows a reader served: per row its framing, per value
/// its length and the bytes of its text.
fn returned_bytes(records: &[Value]) -> i64 {
    let mut bytes: usize = 0;
    for row in records {
        bytes += ROW_FRAMING_BYTES;
        for field in row.as_array().map(Vec::as_slice).unwrap_or_default() {
            bytes += FIELD_LENGTH_BYTES + text_bytes(field);
        }
    }
    i64::try_from(bytes).unwrap_or(i64::MAX)
}

/// How many bytes one served value's text takes: the characters the warehouse
/// prints for it, as the member the value arrived under spells them. A NULL
/// prints nothing; binary arrives base64-encoded on the wire and prints as two
/// hexadecimal characters per byte, so its text is twice the bytes that encoding
/// carries (three per four characters, less its padding).
fn text_bytes(field: &Value) -> usize {
    if field.get("isNull").is_some() {
        return 0;
    }
    if let Some(value) = field.get("booleanValue").and_then(Value::as_bool) {
        return value.to_string().len();
    }
    if let Some(value) = field.get("longValue").and_then(Value::as_i64) {
        return value.to_string().len();
    }
    if let Some(value) = field.get("doubleValue").and_then(Value::as_f64) {
        return value.to_string().len();
    }
    if let Some(value) = field.get("stringValue").and_then(Value::as_str) {
        return value.len();
    }
    let encoded = field
        .get("blobValue")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("a served value carries none of the modelled members: {field}"));
    let padding = encoded.chars().filter(|character| *character == '=').count();
    (encoded.len() / 4 * 3 - padding) * 2
}
