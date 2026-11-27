// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the `SchemaDefinition` a schema-registry write admits, and for the
//! refusal one it does not earns.
//!
//! The rules under test:
//!
//! 1. A `SchemaDefinition` is "the schema definition using the DataFormat setting for the
//!    SchemaName" (`smithy_query operation=RegisterSchemaVersion`), and the documents of the
//!    `AVRO` and `JSON` formats are written AS JSON — so text that is not JSON at all is no
//!    definition of either, and the write (`RegisterSchemaVersion`, `CreateSchema`) is
//!    refused with HTTP 400 `InvalidInputException` carrying the registry's JSON parser's own
//!    diagnostic: what the parse stopped on, what could have stood there instead, and the
//!    line and column of the submitted definition it stopped at — the position just PAST an
//!    unrecognised bare word, and none of the definition itself. A refused write admits no
//!    version and creates no schema set.
//! 2. `GetSchemaVersion` refuses a version it cannot resolve with HTTP 400
//!    `EntityNotFoundException`, and WHICH entity it reports missing follows what the request
//!    addressed: a bare `SchemaVersionId` names no schema set, so the schema is what could
//!    not be found, while a `SchemaVersionNumber` of a schema set this endpoint holds names a
//!    version of a schema it did find — and that refusal echoes the address the request asked
//!    for, the `SchemaId`'s three members as sent and the version number it named.
//!
//! Each suite is a TABLE whose rows differ along what the rule generalises over: the class of
//! JSON fault (a bare word, a character no value begins, an unquoted member name, a
//! collection left open), how far into the definition the fault sits, the `DataFormat` the
//! schema set is written in, the write the definition was submitted to, and the way the read
//! addressed the version. Every call goes over the wire through the shared [`crate::wire`]
//! harness.

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::state::DEFAULT_REGISTRY_NAME;
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{call, emulator, error_code, error_message, ok, ACCOUNT_ID, REGION};

/// A valid Avro record document, the first version of the `AVRO` schema set below.
const AVRO: &str = r#"{"type":"record","name":"User","fields":[{"name":"id","type":"string"}]}"#;

/// An evolution of it, for the row that shows a definition the parser DOES accept is still
/// admitted.
const AVRO_EVOLVED: &str = concat!(
    r#"{"type":"record","name":"User","fields":[{"name":"id","type":"string"},"#,
    r#"{"name":"email","type":["null","string"],"default":null}]}"#
);

/// A valid JSON Schema document, the first version of the `JSON` schema set below.
const JSON_SCHEMA: &str = r#"{"type":"object","properties":{"id":{"type":"string"}}}"#;

/// The source clause a diagnostic carries where the parse's own source is WITHHELD: the
/// registry does not echo a submitted definition back, so it reports the source as redacted
/// and names the parser feature that would have put it there.
///
/// One copy, shared with [`crate::schema_definition_between_entries`], so the two suites
/// that pin a parser diagnostic cannot drift about this clause.
pub(crate) const REDACTED: &str =
    "REDACTED (`StreamReadFeature.INCLUDE_SOURCE_IN_LOCATION` disabled)";

/// The classes a JSON value may be written as, as the parser's diagnostic lists them.
const CLASSES: &str = "JSON String, Number, Array, Object or token 'null', 'true' or 'false'";

/// The schema set written in `AVRO`, and the one written in `JSON`.
const AVRO_SCHEMA: &str = "parse-refusal-avro";
const JSON_FORMAT_SCHEMA: &str = "parse-refusal-json";

/// The whole `Message` a definition that is not JSON is refused with: the frame the schema
/// registry states the fault in, and the parser's `diagnostic` of where the definition
/// stopped being JSON.
///
/// One copy, shared with [`crate::schema_definition_between_entries`].
pub(crate) fn refusal(data_format: &str, diagnostic: &str) -> String {
    format!("Schema definition of {data_format} data format is invalid: {diagnostic}")
}

/// The location clause a diagnostic ends with, for a parse that stopped at `line`:`column`.
///
/// One copy, shared with [`crate::schema_definition_between_entries`].
pub(crate) fn at(line: usize, column: usize) -> String {
    format!("\n at [Source: {REDACTED}; line: {line}, column: {column}]")
}

/// Stand up an emulator holding one schema set per `DataFormat`, each with a valid first
/// version and `Compatibility` `NONE`, so what a registration is refused for is the
/// definition's own form and never a compatibility verdict.
async fn given_schemas() -> MultiServiceRouter {
    let router = emulator();
    for (schema, data_format, definition) in [
        (AVRO_SCHEMA, "AVRO", AVRO),
        (JSON_FORMAT_SCHEMA, "JSON", JSON_SCHEMA),
    ] {
        ok(
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
    }
    router
}

/// The `VersionNumber`s `ListSchemaVersions` reports for `schema`, in the order it reports
/// them.
async fn version_numbers(router: &MultiServiceRouter, schema: &str) -> Vec<i64> {
    let listed = ok(
        router,
        "ListSchemaVersions",
        &json!({ "SchemaId": { "SchemaName": schema } }),
    )
    .await;
    listed
        .get("Schemas")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("the listing must report Schemas, got {listed}"))
        .iter()
        .map(|entry| {
            entry
                .get("VersionNumber")
                .and_then(Value::as_i64)
                .unwrap_or_else(|| panic!("every entry carries a VersionNumber, got {entry}"))
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Rule 1: a definition that is not JSON
// ---------------------------------------------------------------------------

/// One row: what a failing assertion calls it, the schema set the definition is registered
/// against, that set's `DataFormat`, the definition, and the diagnostic the refusal carries.
type Row = (&'static str, &'static str, &'static str, String);

/// The rows of rule 1, spanning the fault classes a JSON parse reports and the two formats
/// whose documents are JSON.
fn unreadable_definitions() -> Vec<Row> {
    vec![
        (
            "a bare word a literal starts like",
            AVRO_SCHEMA,
            "not a schema",
            // The word is named WHOLE and the position is the one just past its last
            // character: the reader matched every character of it before it could say the
            // word is no value.
            format!("Unrecognized token 'not': was expecting ({CLASSES}){}", at(1, 4)),
        ),
        (
            "a longer bare word",
            AVRO_SCHEMA,
            "schema-registry",
            format!(
                "Unrecognized token 'schema': was expecting ({CLASSES}){}",
                at(1, 7)
            ),
        ),
        (
            "a bare word where a member's value belongs",
            AVRO_SCHEMA,
            r#"{"type": record}"#,
            format!(
                "Unrecognized token 'record': was expecting ({CLASSES}){}",
                at(1, 16)
            ),
        ),
        (
            "a character no value begins",
            AVRO_SCHEMA,
            "*",
            format!(
                "Unexpected character ('*' (code 42)): was expecting a valid value \
                 ({CLASSES}){}",
                at(1, 1)
            ),
        ),
        (
            "an unquoted member name",
            AVRO_SCHEMA,
            r#"{type: "record"}"#,
            format!(
                "Unexpected character ('t' (code 116)): was expecting double-quote to start \
                 field name{}",
                at(1, 2)
            ),
        ),
        (
            "an object left open",
            AVRO_SCHEMA,
            r#"{"type":"record""#,
            format!(
                "Unexpected end-of-input: expected close marker for Object (start marker at \
                 [Source: {REDACTED}; line: 1, column: 1]){}",
                at(1, 17)
            ),
        ),
        (
            "a bare word in a JSON-format schema set",
            JSON_FORMAT_SCHEMA,
            "nope",
            format!(
                "Unrecognized token 'nope': was expecting ({CLASSES}){}",
                at(1, 5)
            ),
        ),
        (
            "a member left dangling in a JSON-format schema set",
            JSON_FORMAT_SCHEMA,
            r#"{"type":"#,
            format!(
                "Unexpected end-of-input: expected a value{}",
                at(1, 9)
            ),
        ),
    ]
}

/// Rule 1 through `RegisterSchemaVersion`: the registration is refused with the parser's
/// diagnostic, and the schema set is left holding exactly the versions it already had.
#[tokio::test]
async fn a_registration_whose_definition_is_not_json_is_refused_with_the_parsers_diagnostic() {
    let router = given_schemas().await;

    for (case, schema, definition, diagnostic) in unreadable_definitions() {
        let data_format = if schema == AVRO_SCHEMA { "AVRO" } else { "JSON" };
        let (status, refused) = call(
            &router,
            "RegisterSchemaVersion",
            &json!({
                "SchemaId": { "SchemaName": schema },
                "SchemaDefinition": definition,
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} with {refused}"
        );
        assert!(
            error_code(&refused).contains("InvalidInputException"),
            "case {case:?}: expected InvalidInputException, got {refused}"
        );
        let expected = refusal(data_format, &diagnostic);
        let message = error_message(&refused);
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
    }

    // A refused registration admits no version: each set still holds the one its create
    // stored.
    for schema in [AVRO_SCHEMA, JSON_FORMAT_SCHEMA] {
        let held = version_numbers(&router, schema).await;
        assert_eq!(
            held,
            vec![1],
            "schema {schema:?}: a refused registration must admit no version, got {held:?}"
        );
    }

    // A definition the parser DOES accept is still admitted, and reads back.
    let registered = ok(
        &router,
        "RegisterSchemaVersion",
        &json!({
            "SchemaId": { "SchemaName": AVRO_SCHEMA },
            "SchemaDefinition": AVRO_EVOLVED,
        }),
    )
    .await;
    assert_eq!(
        registered.get("VersionNumber").and_then(Value::as_i64),
        Some(2),
        "a parseable definition must take the set's next version, got {registered}"
    );
    let version_id = registered
        .get("SchemaVersionId")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("the registration must report a SchemaVersionId, got {registered}"))
        .to_string();
    let read = ok(
        &router,
        "GetSchemaVersion",
        &json!({ "SchemaVersionId": version_id }),
    )
    .await;
    assert_eq!(
        read.get("SchemaDefinition").and_then(Value::as_str),
        Some(AVRO_EVOLVED),
        "the admitted version must read back the definition registered, got {read}"
    );
}

/// Rule 1 through `CreateSchema`: the same reading refuses the create, which then holds no
/// schema set at all under the name it was refused for.
#[tokio::test]
async fn a_create_whose_definition_is_not_json_is_refused_and_stores_nothing() {
    let router = emulator();

    // (case, the DataFormat the create names, the definition, the diagnostic)
    let rows: Vec<(&str, &str, &str, String)> = vec![
        (
            "a bare word",
            "AVRO",
            "not a schema",
            format!("Unrecognized token 'not': was expecting ({CLASSES}){}", at(1, 4)),
        ),
        (
            "an array left open",
            "JSON",
            r#"{"enum":[1,2"#,
            format!(
                "Unexpected end-of-input: expected close marker for Array (start marker at \
                 [Source: {REDACTED}; line: 1, column: 9]){}",
                at(1, 13)
            ),
        ),
        (
            "a bare word on the second line",
            "AVRO",
            "{\n  oops\n}",
            format!(
                "Unexpected character ('o' (code 111)): was expecting double-quote to start \
                 field name{}",
                at(2, 3)
            ),
        ),
    ];

    for (index, (case, data_format, definition, diagnostic)) in rows.into_iter().enumerate() {
        let schema = format!("refused-create-{index}");
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
            "case {case:?}: expected 400, got {status} with {refused}"
        );
        assert!(
            error_code(&refused).contains("InvalidInputException"),
            "case {case:?}: expected InvalidInputException, got {refused}"
        );
        let expected = refusal(data_format, &diagnostic);
        let message = error_message(&refused);
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );

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
}

// ---------------------------------------------------------------------------
// Rule 2: which entity a version read reports missing
// ---------------------------------------------------------------------------

/// Rule 2: the refusal names the entity the request's own address could not be resolved to —
/// the schema for a bare `SchemaVersionId`, the version for a number of a schema set this
/// endpoint holds, whose address the refusal echoes back member for member.
#[tokio::test]
async fn a_version_read_reports_the_entity_its_address_could_not_resolve() {
    let router = given_schemas().await;
    let avro_arn = format!(
        "arn:aws:glue:{REGION}:{ACCOUNT_ID}:schema/{DEFAULT_REGISTRY_NAME}/{AVRO_SCHEMA}"
    );

    // (case, the GetSchemaVersion body, the whole Message the refusal carries)
    let rows: Vec<(&str, Value, String)> = vec![
        (
            "an id no version carries",
            json!({ "SchemaVersionId": "00000000-0000-0000-0000-000000000000" }),
            "Schema is not found".to_string(),
        ),
        (
            "another well-formed id no version carries",
            json!({ "SchemaVersionId": "9f8e7d6c-5b4a-3928-1706-fedcba987654" }),
            "Schema is not found".to_string(),
        ),
        (
            "a version number the set does not hold",
            json!({
                "SchemaId": { "SchemaName": AVRO_SCHEMA },
                "SchemaVersionNumber": { "VersionNumber": 7 },
            }),
            format!(
                "Schema version is not found. RegistryName: null, SchemaName: {AVRO_SCHEMA}, \
                 SchemaArn: null, VersionNumber: 7, isLatestVersion: false"
            ),
        ),
        (
            "a version number of a set addressed by registry and name",
            json!({
                "SchemaId": {
                    "RegistryName": DEFAULT_REGISTRY_NAME,
                    "SchemaName": JSON_FORMAT_SCHEMA,
                },
                "SchemaVersionNumber": { "VersionNumber": 2 },
            }),
            format!(
                "Schema version is not found. RegistryName: {DEFAULT_REGISTRY_NAME}, \
                 SchemaName: {JSON_FORMAT_SCHEMA}, SchemaArn: null, VersionNumber: 2, \
                 isLatestVersion: false"
            ),
        ),
        (
            "a version number of a set addressed by ARN",
            json!({
                "SchemaId": { "SchemaArn": avro_arn.clone() },
                "SchemaVersionNumber": { "VersionNumber": 4 },
            }),
            format!(
                "Schema version is not found. RegistryName: null, SchemaName: null, \
                 SchemaArn: {avro_arn}, VersionNumber: 4, isLatestVersion: false"
            ),
        ),
        (
            "a schema set this endpoint holds none of",
            json!({
                "SchemaId": { "SchemaName": "no-such-schema" },
                "SchemaVersionNumber": { "LatestVersion": true },
            }),
            "Schema is not found.".to_string(),
        ),
    ];

    for (case, body, expected) in rows {
        let (status, refused) = call(&router, "GetSchemaVersion", &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} with {refused}"
        );
        assert!(
            error_code(&refused).contains("EntityNotFoundException"),
            "case {case:?}: expected EntityNotFoundException, got {refused}"
        );
        let message = error_message(&refused);
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
    }
    // The same read resolves the version the create stored, so the refusals above are the
    // address failing and not the read.
    let read = ok(
        &router,
        "GetSchemaVersion",
        &json!({
            "SchemaId": { "SchemaName": AVRO_SCHEMA },
            "SchemaVersionNumber": { "LatestVersion": true },
        }),
    )
    .await;
    assert_eq!(
        read.get("SchemaDefinition").and_then(Value::as_str),
        Some(AVRO),
        "the set's latest version must read back, got {read}"
    );
}
