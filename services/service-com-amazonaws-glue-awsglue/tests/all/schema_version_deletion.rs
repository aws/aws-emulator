// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DeleteSchemaVersions`: which versions a `Versions` string names,
//! which of them the operation removes, and how it refuses a value it cannot act on.
//!
//! The rules under test:
//!
//! 1. `Versions` names "a single version number, 5 a range, 5-8 : deletes versions 5, 6, 7,
//!    8" (`smithy_query operation=DeleteSchemaVersions`), so the operation removes exactly
//!    the versions the inclusive span names and leaves every other one in place.
//! 2. A value that spells no version number or range of them is refused with the
//!    operation's OWN modelled `InvalidInputException` (HTTP 400) and a `Message` naming the
//!    offending parameter — never with the cross-service `ValidationException`, which this
//!    operation does not model, and never with a body carrying no `Message` at all.
//! 3. The two documented prohibitions refuse the whole request and delete nothing: a span
//!    containing the schema set's check-pointed version, and one naming its first version.
//!
//! Each suite is a TABLE whose rows differ along what the rule generalises over: the form
//! and value of `Versions` (one number, a range, a reversed range, a span past the versions
//! held, and every class of malformed value), which version the schema set is
//! check-pointed at, which versions it holds, and the form `SchemaId` addresses it by.
//! Every call goes over the wire through the shared [`crate::wire`] harness.

use std::sync::Arc;
use std::time::SystemTime;

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::state::{
    ComAmazonawsGlueAwsglueState, RegistryEntry, SchemaEntry, SchemaVersionEntry,
    DEFAULT_COMPATIBILITY, DEFAULT_REGISTRY_NAME, NO_SCHEMA_VERSION, SCHEMA_STATUS_AVAILABLE,
    SCHEMA_VERSION_STATUS_AVAILABLE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{call, emulator_on, error_code, error_message, ok, ACCOUNT_ID};

/// The schema set every suite here addresses.
const SCHEMA: &str = "payments";

/// The version numbers the span suite's schema set is seeded holding, and so the numbers it
/// has assigned: every one of them above the first version, so neither documented
/// prohibition is in play for a span inside this history.
const SEEDED_VERSIONS: &[i64] = &[2, 3, 4, 5, 6];

/// The whole `Message` a `Versions` value the operation cannot act on is refused with,
/// written out here rather than read from the crate's own constructor so this suite pins
/// the wording and the member it names.
const VERSIONS_NOT_VALID: &str =
    "The parameter value contains one or more characters that are not valid. \
     Parameter Name: Versions";

/// The whole `Message` a `SchemaId` resolving to no schema set is refused with: fixed text
/// naming neither the schema addressed nor the registry it was addressed in. Written out
/// here so this suite pins the wording.
const SCHEMA_NOT_FOUND: &str = "Schema is not found.";

/// An Avro record definition, which is what each seeded version carries.
fn definition(version_number: i64) -> String {
    format!(
        r#"{{"type":"record","name":"Payment","fields":[{{"name":"v{version_number}","type":"string"}}]}}"#
    )
}

/// A router over a shared engine holding the default registry with the schema set [`SCHEMA`]
/// in it, carrying one version per entry of `version_numbers` and check-pointed at
/// `checkpoint`.
///
/// The versions are seeded straight into the store because this suite is about the DELETE:
/// it arranges the history it deletes from rather than driving the registration of each
/// version, which has its own suite.
fn emulator_holding(
    version_numbers: &[i64],
    checkpoint: i64,
) -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(Arc::clone(&engine));
    {
        let state = engine.state::<ComAmazonawsGlueAwsglueState>();
        let mut store = state.lock();
        let mut registry = RegistryEntry::new(DEFAULT_REGISTRY_NAME, None, None, SystemTime::now());
        registry.schemas.insert(
            SCHEMA.to_string(),
            SchemaEntry {
                name: SCHEMA.to_string(),
                registry_name: DEFAULT_REGISTRY_NAME.to_string(),
                description: None,
                data_format: "AVRO".to_string(),
                compatibility: DEFAULT_COMPATIBILITY.to_string(),
                status: SCHEMA_STATUS_AVAILABLE.to_string(),
                checkpoint,
                created_time: SystemTime::now(),
                updated_time: SystemTime::now(),
                tags: Default::default(),
                versions: version_numbers
                    .iter()
                    .map(|number| {
                        SchemaVersionEntry::new(
                            *number,
                            definition(*number),
                            SCHEMA_VERSION_STATUS_AVAILABLE,
                            SystemTime::now(),
                        )
                    })
                    .collect(),
                // The highest number the seeded history ever assigned, which is what the
                // next registration would count from however many of them a delete removes.
                highest_version_number: version_numbers
                    .iter()
                    .copied()
                    .max()
                    .unwrap_or(NO_SCHEMA_VERSION),
            },
        );
        store
            .registries
            .insert(DEFAULT_REGISTRY_NAME.to_string(), registry);
    }
    (router, engine)
}

/// The version numbers the store still holds for [`SCHEMA`], in the order it holds them.
fn held_versions(engine: &Arc<engine::InMemoryEngine>) -> Vec<i64> {
    let state = engine.state::<ComAmazonawsGlueAwsglueState>();
    let store = state.lock();
    store
        .schema(DEFAULT_REGISTRY_NAME, SCHEMA)
        .expect("the seeded schema set is held")
        .versions
        .iter()
        .map(|version| version.version_number)
        .collect()
}

/// A `DeleteSchemaVersions` body addressing [`SCHEMA`] by name.
fn delete_body(versions: &str) -> Value {
    json!({ "SchemaId": { "SchemaName": SCHEMA }, "Versions": versions })
}

/// A span names exactly the versions it covers: those are the ones the schema set loses and
/// every other one stays, which a `GetSchema` read confirms through `LatestSchemaVersion`.
///
/// The rows differ along the form and reach of the span: one number in the middle, one at
/// the end, a range of two, a range reaching past the versions held, a degenerate range
/// (`3-3`), a reversed range (which covers nothing), and a span naming only versions the
/// set does not hold.
#[tokio::test]
async fn a_version_span_deletes_exactly_the_versions_it_names() {    // (case, the `Versions` value, the version numbers left afterwards)
    let rows: Vec<(&str, &str, Vec<i64>)> = vec![
        ("one version in the middle", "3", vec![2, 4, 5, 6]),
        ("the latest version", "6", vec![2, 3, 4, 5]),
        ("a range of two", "3-4", vec![2, 5, 6]),
        ("a range reaching past the last version", "4-99", vec![2, 3]),
        ("a degenerate range", "3-3", vec![2, 4, 5, 6]),
        (
            // `[1-9][0-9]*-[1-9][0-9]*` admits a descending pair, which covers no version
            // at all, so the set is left as it was.
            "a reversed range, which covers nothing",
            "6-3",
            vec![2, 3, 4, 5, 6],
        ),
        (
            "a version the set does not hold",
            "9",
            vec![2, 3, 4, 5, 6],
        ),
    ];

    for (case, versions, remaining) in rows {
        // Check-pointed below every version held, and holding no version 1, so neither
        // documented prohibition is in play for these rows.
        let (router, engine) = emulator_holding(SEEDED_VERSIONS, NO_SCHEMA_VERSION);
        let answered = ok(&router, "DeleteSchemaVersions", &delete_body(versions)).await;
        assert_eq!(
            answered.get("SchemaVersionErrors"),
            Some(&json!([])),
            "case {case:?}: every named version was deleted or already absent, so the \
             answer must carry an empty list of per-version errors, got {answered}"
        );

        let held = held_versions(&engine);
        assert_eq!(
            held, remaining,
            "case {case:?}: expected the set to hold {remaining:?}, got {held:?}"
        );

        let read = ok(&router, "GetSchema", &json!({ "SchemaId": { "SchemaName": SCHEMA } })).await;
        // A version NUMBER is assigned once and never reused, so deleting versions frees
        // neither their numbers nor the numbers above them: the schema set keeps reporting
        // the highest number it ever assigned, whichever of those versions it still holds.
        let assigned = SEEDED_VERSIONS
            .iter()
            .copied()
            .max()
            .expect("the seeded history holds a version");
        assert_eq!(
            read["LatestSchemaVersion"],
            json!(assigned),
            "case {case:?}: expected LatestSchemaVersion {assigned}, got {read}"
        );
        assert_eq!(
            read["NextSchemaVersion"],
            json!(assigned + 1),
            "case {case:?}: expected NextSchemaVersion {}, got {read}",
            assigned + 1
        );
    }
}

/// A `Versions` value that spells no version number or range of them is refused with the
/// operation's own `InvalidInputException` naming the parameter, and deletes nothing.
///
/// The rows span every class of value outside the form: the empty string, a zero and a
/// leading zero, a negative number, digits with trailing or leading junk, a decimal, a
/// separator the form does not use, a half-written range, a range of three ends, inner
/// whitespace, and a run of digits too long to be a version counter.
#[tokio::test]
async fn a_versions_value_outside_the_form_is_the_services_own_input_error() {
    let rows: Vec<(&str, String)> = vec![
        ("the empty string", String::new()),
        ("a zero", "0".to_string()),
        ("a leading zero", "03".to_string()),
        ("a negative number", "-3".to_string()),
        ("a decimal", "3.5".to_string()),
        ("digits with trailing text", "3abc".to_string()),
        ("text with trailing digits", "abc3".to_string()),
        ("no digits at all", "latest".to_string()),
        ("a comma-separated list", "3,4".to_string()),
        ("a half-written range", "3-".to_string()),
        ("a range with no low end", "-4".to_string()),
        ("a range of three ends", "3-4-5".to_string()),
        ("inner whitespace", "3 - 4".to_string()),
        ("surrounding whitespace", " 3 ".to_string()),
        ("a run of digits no counter can hold", "9".repeat(30)),
    ];

    for (case, versions) in rows {
        let (router, engine) = emulator_holding(&[2, 3, 4, 5, 6], NO_SCHEMA_VERSION);
        let (status, body) = call(&router, "DeleteSchemaVersions", &delete_body(&versions)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the request to be refused, got {status} with {body}"
        );
        assert!(
            error_code(&body).contains("InvalidInputException"),
            "case {case:?}: expected InvalidInputException, got {body}"
        );
        assert!(
            !error_code(&body).contains("ValidationException"),
            "case {case:?}: this operation models no ValidationException, got {body}"
        );
        assert_eq!(
            error_message(&body),
            VERSIONS_NOT_VALID,
            "case {case:?}: expected Message {VERSIONS_NOT_VALID:?}, got {:?}",
            error_message(&body)
        );
        let held = held_versions(&engine);
        assert_eq!(
            held,
            vec![2, 3, 4, 5, 6],
            "case {case:?}: a refused request must delete nothing, got {held:?}"
        );
    }
}

/// A span that reaches the schema set's check-pointed version refuses the WHOLE request and
/// deletes nothing — "you have to remove the checkpoint first using the
/// DeleteSchemaCheckpoint API before using this API".
///
/// The rows differ along where the checkpoint sits relative to the span: at its low end, in
/// its middle, at its high end, named alone, and outside it (which is served).
#[tokio::test]
async fn a_span_containing_the_checkpoint_deletes_nothing() {
    // (case, the checkpoint, the `Versions` value, whether the request is served)
    let rows: Vec<(&str, i64, &str, bool)> = vec![
        ("the checkpoint named alone", 4, "4", false),
        ("the checkpoint at the span's low end", 4, "4-6", false),
        ("the checkpoint inside the span", 4, "3-5", false),
        ("the checkpoint at the span's high end", 4, "2-4", false),
        ("a span below the checkpoint", 6, "3-5", true),
        ("a span above the checkpoint", 2, "3-5", true),
        ("a schema set with no checkpoint", NO_SCHEMA_VERSION, "3-5", true),
    ];

    for (case, checkpoint, versions, served) in rows {
        let (router, engine) = emulator_holding(&[2, 3, 4, 5, 6], checkpoint);
        let (status, body) = call(&router, "DeleteSchemaVersions", &delete_body(versions)).await;
        if served {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the request to be served, got {status} with {body}"
            );
            continue;
        }
        assert_eq!(
            status, 400,
            "case {case:?}: expected the request to be refused, got {status} with {body}"
        );
        assert!(
            error_code(&body).contains("ConcurrentModificationException"),
            "case {case:?}: expected ConcurrentModificationException, got {body}"
        );
        assert!(
            error_message(&body).contains(&checkpoint.to_string()),
            "case {case:?}: expected the refusal to name the check-pointed version, got {:?}",
            error_message(&body)
        );
        let held = held_versions(&engine);
        assert_eq!(
            held,
            vec![2, 3, 4, 5, 6],
            "case {case:?}: a refused request must delete nothing, got {held:?}"
        );
    }
}

/// A span that names the schema set's FIRST version refuses the whole request and deletes
/// nothing — "the first schema version can only be deleted by the DeleteSchema API".
///
/// The rows differ along which version is first (version 1, and a higher one for a set
/// whose earlier versions are already gone) and along how the span reaches it: named alone,
/// at the span's low end, and inside it — beside a span that starts just past it, which is
/// served.
#[tokio::test]
async fn a_span_naming_the_first_version_deletes_nothing() {
    // (case, the versions held, the `Versions` value, whether the request is served)
    let rows: Vec<(&str, Vec<i64>, &str, bool)> = vec![
        ("version 1 named alone", vec![1, 2, 3], "1", false),
        ("version 1 at the span's low end", vec![1, 2, 3], "1-2", false),
        ("version 1 inside a wider span", vec![1, 2, 3], "1-9", false),
        ("the span starting just past version 1", vec![1, 2, 3], "2-3", true),
        (
            "the first version of a set whose earlier ones are gone",
            vec![4, 5, 6],
            "4",
            false,
        ),
        (
            "a span reaching that first version from below",
            vec![4, 5, 6],
            "2-4",
            false,
        ),
        (
            "a span above that first version",
            vec![4, 5, 6],
            "5-6",
            true,
        ),
    ];

    for (case, held_before, versions, served) in rows {
        let (router, engine) = emulator_holding(&held_before, NO_SCHEMA_VERSION);
        let (status, body) = call(&router, "DeleteSchemaVersions", &delete_body(versions)).await;
        if served {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the request to be served, got {status} with {body}"
            );
            continue;
        }
        assert_eq!(
            status, 400,
            "case {case:?}: expected the request to be refused, got {status} with {body}"
        );
        assert!(
            error_code(&body).contains("InvalidInputException"),
            "case {case:?}: expected InvalidInputException, got {body}"
        );
        let first = held_before.first().copied().unwrap_or(NO_SCHEMA_VERSION);
        assert!(
            error_message(&body).contains(&first.to_string()),
            "case {case:?}: expected the refusal to name version {first}, got {:?}",
            error_message(&body)
        );
        let held = held_versions(&engine);
        assert_eq!(
            held, held_before,
            "case {case:?}: a refused request must delete nothing, got {held:?}"
        );
    }
}

/// One row of [`the_schema_id_is_resolved_before_the_span_is_acted_on`]: a `SchemaId` and
/// what the operation answers for it.
struct ReferenceRow {
    /// The case name every assertion of the row reports.
    case: &'static str,
    /// The `SchemaId` member the request carries.
    schema_id: Value,
    /// The error code the refusal carries, or `None` for a reference that resolves.
    code: Option<&'static str>,
    /// The `Message` the refusal carries, for a row that is refused.
    message: String,
}

/// `SchemaId` addresses the schema set the versions belong to, and is resolved before the
/// span is acted on: a reference naming neither of its two members is the operation's input
/// error, and one that resolves to a schema set this endpoint holds none of is its entity
/// miss.
///
/// The rows differ along the form of the reference: by name, by this endpoint's ARN, by both
/// together, by neither, by a name no schema has, by an ARN of another account and by one
/// that is no schema ARN at all.
#[tokio::test]
async fn the_schema_id_is_resolved_before_the_span_is_acted_on() {
    let local_arn = format!("arn:aws:glue:us-east-1:{ACCOUNT_ID}:schema/{DEFAULT_REGISTRY_NAME}/{SCHEMA}");
    let foreign_arn =
        format!("arn:aws:glue:us-east-1:123456789012:schema/{DEFAULT_REGISTRY_NAME}/{SCHEMA}");
    let not_a_schema_arn = format!("arn:aws:glue:us-east-1:{ACCOUNT_ID}:table/db/t");

    // (case, the `SchemaId` member, the expected error code and `Message` — `None` for a
    // reference that resolves and is served)
    let rows: Vec<ReferenceRow> = vec![
        ReferenceRow {
            case: "by name",
            schema_id: json!({ "SchemaName": SCHEMA }),
            code: None,
            message: String::new(),
        },
        ReferenceRow {
            case: "by name and registry name",
            schema_id: json!({ "SchemaName": SCHEMA, "RegistryName": DEFAULT_REGISTRY_NAME }),
            code: None,
            message: String::new(),
        },
        ReferenceRow {
            case: "by this endpoint's ARN",
            schema_id: json!({ "SchemaArn": local_arn }),
            code: None,
            message: String::new(),
        },
        ReferenceRow {
            case: "by both members naming the same schema",
            schema_id: json!({ "SchemaArn": local_arn, "SchemaName": SCHEMA }),
            code: None,
            message: String::new(),
        },
        ReferenceRow {
            case: "naming neither member",
            schema_id: json!({}),
            code: Some("InvalidInputException"),
            message: "One of SchemaArn or SchemaName has to be provided.".to_string(),
        },
        ReferenceRow {
            case: "a name no schema set has",
            schema_id: json!({ "SchemaName": "no-such-schema" }),
            code: Some("EntityNotFoundException"),
            message: SCHEMA_NOT_FOUND.to_string(),
        },
        ReferenceRow {
            case: "an ARN of another account",
            schema_id: json!({ "SchemaArn": foreign_arn }),
            code: Some("EntityNotFoundException"),
            message: SCHEMA_NOT_FOUND.to_string(),
        },
        ReferenceRow {
            case: "an ARN that is no schema ARN",
            schema_id: json!({ "SchemaArn": not_a_schema_arn }),
            code: Some("InvalidInputException"),
            message: format!("SchemaArn {not_a_schema_arn:?} is not the ARN of a Glue schema"),
        },
    ];

    for row in &rows {
        let case = row.case;
        let (router, engine) = emulator_holding(&[2, 3, 4], NO_SCHEMA_VERSION);
        let (status, body) = call(
            &router,
            "DeleteSchemaVersions",
            &json!({ "SchemaId": row.schema_id, "Versions": "3" }),
        )
        .await;
        match row.code {
            None => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected the request to be served, got {status} with {body}"
                );
                let held = held_versions(&engine);
                assert_eq!(
                    held,
                    vec![2, 4],
                    "case {case:?}: expected version 3 to be deleted, got {held:?}"
                );
            }
            Some(code) => {
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected the request to be refused, got {status} with {body}"
                );
                assert!(
                    error_code(&body).contains(code),
                    "case {case:?}: expected {code}, got {body}"
                );
                assert_eq!(
                    error_message(&body),
                    row.message,
                    "case {case:?}: expected Message {:?}, got {:?}",
                    row.message,
                    error_message(&body)
                );
                let held = held_versions(&engine);
                assert_eq!(
                    held,
                    vec![2, 3, 4],
                    "case {case:?}: a refused request must delete nothing, got {held:?}"
                );
            }
        }
    }
}
