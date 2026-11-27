// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the reading of a `SchemaDefinition` whose JSON text runs out of
//! input ON an entry separator.
//!
//! The rule under test: a definition that stops on an entry separator has promised its
//! container a further entry and then ended, so the parser reports the STATE it ran out in —
//! `Unexpected end-of-input within/between <Object|Array> entries`, naming the container the
//! separator belongs to and nothing else — followed by the one location clause of the
//! position just past the definition's last character; a definition that runs out anywhere
//! else keeps the reading of what the parse was waiting for (a value, a close marker for the
//! container it was inside, a close marker for a string). `CheckSchemaVersionValidity`
//! reports that reading in `Error` on a 200 with `Valid` false
//! (`smithy_query operation=CheckSchemaVersionValidity`), and a registry WRITE of the same
//! definition is refused with the same reading inside the registry's refusal frame.
//!
//! The rows differ along what the rule generalises over: which container the separator
//! belongs to (an object's members, an array's elements), how deeply that container nests,
//! whether the separator is the innermost container's or an outer one's, how much whitespace
//! follows it, which LINE the definition runs out on, and which of the two `DataFormat`s
//! whose documents are written as JSON the definition was submitted for — beside control
//! rows that run out of input in every other way the parser words. Every call goes over the
//! wire through the shared [`crate::wire`] harness.

use serde_json::{json, Value};

use service_com_amazonaws_glue_awsglue::state::DEFAULT_REGISTRY_NAME;

// The clause a diagnostic carries where the parse's own source is withheld, the location
// clause it ends with, and the frame a registry WRITE states a reading in, all as the suite
// that pins the parse refusals of this registry writes them — one copy, so the two suites
// cannot drift about the same clause.
use crate::schema_definition_parse_refusal::{at, refusal, REDACTED};
use crate::wire::{call, emulator, error_code, error_message, ok};

/// One row: what a failing assertion calls it, the `DataFormat` the definition is submitted
/// for, the definition, and the whole `Error` the check reports for it.
type Row = (&'static str, &'static str, &'static str, String);

/// The rows of the rule: definitions that run out of input on an entry separator, and the
/// control definitions that run out of input in every other way the parser words.
fn definitions_that_run_out_of_input() -> Vec<Row> {
    vec![
        // The recorded shape: an object truncated right after a member separator.
        (
            "an object that ends on its member separator",
            "JSON",
            r#"{"type": "object","#,
            format!(
                "Unexpected end-of-input within/between Object entries{}",
                at(1, 19)
            ),
        ),
        (
            "an array that ends on its element separator",
            "JSON",
            "[1,",
            format!(
                "Unexpected end-of-input within/between Array entries{}",
                at(1, 4)
            ),
        ),
        (
            "an array inside an object that ends on its element separator",
            "JSON",
            r#"{"enum":[1,"#,
            format!(
                "Unexpected end-of-input within/between Array entries{}",
                at(1, 12)
            ),
        ),
        // The separator is the OUTER container's: the inner one closed again, so the
        // container the parse stood between the entries of is the object.
        (
            "an object that ends on its separator past a closed inner object",
            "JSON",
            r#"{"a":{"b":1},"#,
            format!(
                "Unexpected end-of-input within/between Object entries{}",
                at(1, 14)
            ),
        ),
        // Whitespace after the separator is not an entry: the definition still ends between
        // entries, at the position just past its last character.
        (
            "a separator followed by a newline and indentation",
            "JSON",
            "{\n  \"type\": \"object\",\n  ",
            format!(
                "Unexpected end-of-input within/between Object entries{}",
                at(3, 3)
            ),
        ),
        // The same reading for the other format whose documents are written as JSON.
        (
            "an Avro document that ends on its member separator",
            "AVRO",
            r#"{"type":"record","name":"User","#,
            format!(
                "Unexpected end-of-input within/between Object entries{}",
                at(1, 32)
            ),
        ),
        // Controls: every other way of running out of input keeps its own reading.
        (
            "an object left open after a complete member",
            "JSON",
            r#"{"type":"record""#,
            format!(
                "Unexpected end-of-input: expected close marker for Object (start marker at \
                 [Source: {REDACTED}; line: 1, column: 1]){}",
                at(1, 17)
            ),
        ),
        (
            "a member whose value never came",
            "JSON",
            r#"{"type":"#,
            format!("Unexpected end-of-input: expected a value{}", at(1, 9)),
        ),
        // A separator INSIDE a string separates nothing: the parse ran out while still
        // reading the string.
        (
            "a string left open on a comma",
            "JSON",
            r#"{"type":"obj,"#,
            format!(
                "Unexpected end-of-input: expected close marker for String{}",
                at(1, 14)
            ),
        ),
    ]
}

/// The whole `Error` `CheckSchemaVersionValidity` reports for a definition that runs out of
/// input: the state the parse ran out in, and the ONE location clause of where that was.
#[tokio::test]
async fn a_definition_that_runs_out_of_input_is_read_by_the_state_the_parse_ran_out_in() {
    let router = emulator();

    for (case, data_format, definition, expected) in definitions_that_run_out_of_input() {
        let answer = ok(
            &router,
            "CheckSchemaVersionValidity",
            &json!({ "DataFormat": data_format, "SchemaDefinition": definition }),
        )
        .await;
        assert_eq!(
            answer.get("Valid").and_then(Value::as_bool),
            Some(false),
            "case {case:?}: a definition that is not JSON is not valid, got {answer}"
        );
        let reported = answer
            .get("Error")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case:?}: the answer must carry an Error, got {answer}"));
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
        // A definition that ran out between a container's entries names that state and no
        // container start marker, so its reading carries exactly one location clause: the
        // position the definition ended at.
        if expected.contains("within/between") {
            let clauses = reported.matches("[Source: ").count();
            assert_eq!(
                clauses, 1,
                "case {case:?}: expected one location clause, got {clauses} in {reported:?}"
            );
            assert!(
                !reported.contains("start marker"),
                "case {case:?}: a reading between entries names no start marker, got \
                 {reported:?}"
            );
        }
        // The definition itself is never echoed into the reading.
        assert!(
            !reported.contains(definition),
            "case {case:?}: the reading must withhold the definition, got {reported:?}"
        );
    }
}

/// The same reading frames the registry WRITE of the same definition: `CreateSchema` is
/// refused with HTTP 400 `InvalidInputException` whose `Message` states the `Error` the check
/// reported, and the refused create stores no schema set at all.
#[tokio::test]
async fn a_write_of_such_a_definition_is_refused_with_the_same_reading() {
    let router = emulator();

    for (index, (case, data_format, definition, expected)) in
        definitions_that_run_out_of_input().into_iter().enumerate()
    {
        let schema = format!("between-entries-{index}");
        let (status, refused) = call(
            &router,
            "CreateSchema",
            &json!({
                "SchemaName": schema,
                "DataFormat": data_format,
                "Compatibility": "NONE",
                "SchemaDefinition": definition,
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a write of such a definition is refused, got {status} with {refused}"
        );
        assert!(
            error_code(&refused).contains("InvalidInputException"),
            "case {case:?}: expected InvalidInputException, got {refused}"
        );
        let message = error_message(&refused);
        let framed = refusal(data_format, &expected);
        assert_eq!(
            message, framed,
            "case {case:?}: expected {framed:?}, got {message:?}"
        );

        // A refused create stores nothing: the name it was refused for holds no schema set.
        let (status, read) = call(
            &router,
            "GetSchema",
            &json!({
                "SchemaId": {
                    "RegistryName": DEFAULT_REGISTRY_NAME,
                    "SchemaName": schema,
                },
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a refused create stores no schema set, got {status} with {read}"
        );
        assert!(
            error_code(&read).contains("EntityNotFoundException"),
            "case {case:?}: expected EntityNotFoundException, got {read}"
        );
    }

    // The bound of the rule: a definition that carries the entry the separator promised IS
    // admitted, and reads back through `GetSchemaVersion`.
    let whole = r#"{"type":"object","properties":{"id":{"type":"integer"}}}"#;
    let created = ok(
        &router,
        "CreateSchema",
        &json!({
            "SchemaName": "between-entries-complete",
            "DataFormat": "JSON",
            "Compatibility": "NONE",
            "SchemaDefinition": whole,
        }),
    )
    .await;
    let version_id = created
        .get("SchemaVersionId")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("the create must report a SchemaVersionId, got {created}"))
        .to_string();
    let read = ok(
        &router,
        "GetSchemaVersion",
        &json!({ "SchemaVersionId": version_id }),
    )
    .await;
    assert_eq!(
        read.get("SchemaDefinition").and_then(Value::as_str),
        Some(whole),
        "the admitted definition must read back, got {read}"
    );
}
