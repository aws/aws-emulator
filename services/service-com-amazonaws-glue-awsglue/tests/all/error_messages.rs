// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the wording of this service's refusals: the `Message` member
//! of the error bodies `GetCrawler`, `CreateCrawler`, `CreateDatabase`, `GetDatabase`,
//! `DeleteDatabase`, `CreateTable`, `GetTable`, `GetTables` and `DeleteTable` answer
//! with.
//!
//! The rule under test: a refusal's `Message` is the wording the service produces FOR
//! THAT OPERATION AND RESOURCE — a fixed constant where the service's message names no
//! resource, and otherwise a template carrying only the request member the refusal is
//! about, never a container the caller also sent and never emulator-side context such
//! as the resolved `CatalogId`. So the same missing table read by `GetTable` and
//! deleted by `DeleteTable` answers two different messages, and a duplicate create
//! never echoes the name it refused.
//!
//! Each suite is a table whose rows differ along what the rule generalises over — the
//! name's value, length, casing and character set, and which operation asks — and the
//! recorded wire case is one row among them. The expected strings are written out here
//! as literals (not read from the service's own constants) so the test pins the wording
//! rather than restating it.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{call, emulator, error_code, error_message, ok, ACCOUNT_ID};

/// The whole `Message` a job-run miss carries — fixed text with no variable part, so a
/// client that matches refusals on their wording recognises the miss whatever it asked
/// for. Written out here, beside this module's other expected wordings, so every suite
/// that drives a job-run lookup pins the same spelling: the read, the retry a start names
/// and each entry of a stop ([`crate::job_run_miss_message`],
/// [`crate::batch_stop_job_run`]).
pub(crate) const JOB_RUN_NOT_FOUND_MESSAGE: &str =
    "Job run not found for the given job name and runId.";

/// Names that span the axis the templates generalise over: the recorded fixture's own
/// shape, a short name, a mixed-case name, one with the punctuation `NameString`'s
/// pattern admits, and one at the member's documented 255-character ceiling
/// (`NameString`, length 1..255, from `smithy_query` on `GetCrawler`/`GetTable`).
fn absent_names() -> Vec<(String, String)> {
    vec![
        (
            "recorded fixture shape".to_string(),
            "sim2real-probe-glue-gluecrawler-missing-49390596".to_string(),
        ),
        ("single character".to_string(), "x".to_string()),
        ("mixed case".to_string(), "MissingThing".to_string()),
        (
            "punctuation and digits".to_string(),
            "team.data_lake-2026/raw".to_string(),
        ),
        ("at the 255 ceiling".to_string(), "n".repeat(255)),
    ]
}

/// Create `name` as a database and assert it succeeded.
async fn create_database(router: &MultiServiceRouter, name: &str) {
    ok(router, "CreateDatabase", &json!({ "DatabaseInput": { "Name": name } })).await;
}

/// Create `table` inside `database` and assert it succeeded.
async fn create_table(router: &MultiServiceRouter, database: &str, table: &str) {
    ok(
        router,
        "CreateTable",
        &json!({ "DatabaseName": database, "TableInput": { "Name": table } }),
    )
    .await;
}

/// A minimal `CreateCrawler` body naming one S3 crawl target.
fn crawler_body(name: &str) -> Value {
    json!({
        "Name": name,
        "Role": "arn:aws:iam::000000000000:role/service-role/AWSGlueServiceRole-test",
        "Targets": { "S3Targets": [{ "Path": "s3://crawler-bucket/data/" }] },
    })
}

/// Assert one refusal: HTTP 400, the expected error code, and the expected `Message`
/// verbatim.
pub(crate) async fn assert_refusal(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    request: &Value,
    expected_code: &str,
    expected_message: &str,
) -> Value {
    let (status, body) = call(router, operation, request).await;
    assert_eq!(
        status, 400,
        "case {case}: expected {operation} to answer 400, got {status} with {body}"
    );
    assert!(
        error_code(&body).contains(expected_code),
        "case {case}: expected {expected_code}, got {body}"
    );
    assert_eq!(
        error_message(&body),
        expected_message,
        "case {case}: expected Message {expected_message:?}, got {:?}",
        error_message(&body)
    );
    body
}

/// Assert a refusal's `Message` does not disclose `name`, in either the casing sent or
/// the folded casing the catalog stores.
///
/// A name counts as disclosed only where it appears as a name of its own, not as a run
/// of characters inside a word: without that, a one-character name would be "found"
/// inside ordinary wording ("Database already exists." carries the "d" of "already")
/// and a disclosure would be reported where there is none.
pub(crate) fn assert_withholds(case: &str, message: &str, name: &str) {
    for candidate in [name.to_string(), name.to_ascii_lowercase()] {
        assert!(
            !names_token(message, &candidate),
            "case {case}: the refusal must not disclose {name:?}, got Message {message:?}"
        );
    }
}

/// Whether `message` carries `token` with a non-name character (or nothing) on each
/// side. The name characters are the ones a Glue `NameString` carries in practice:
/// alphanumerics and the `_`, `-`, `.` punctuation the test names use.
fn names_token(message: &str, token: &str) -> bool {
    let is_name_character =
        |character: Option<char>| character.is_some_and(|c| c.is_alphanumeric() || "_-.".contains(c));
    !token.is_empty()
        && message.match_indices(token).any(|(at, _)| {
            let before = message[..at].chars().next_back();
            let after = message[at + token.len()..].chars().next();
            !is_name_character(before) && !is_name_character(after)
        })
}

// ---------------------------------------------------------------------------
// Crawlers
// ---------------------------------------------------------------------------

/// A crawler lookup for a name no crawler has is refused with the crawler-specific
/// template, built from the request's `Name`: the phrasing AWS uses only for crawlers.
///
/// The rows span the name axis of [`absent_names`], plus a name that differs from a
/// stored crawler only in case (crawler names are not folded, so that is a miss too).
#[tokio::test]
async fn a_crawler_miss_names_the_requested_crawler() {
    let router = emulator();
    ok(&router, "CreateCrawler", &crawler_body("present-crawler")).await;

    let mut cases = absent_names();
    cases.push((
        "differs only in case from a stored crawler".to_string(),
        "PRESENT-CRAWLER".to_string(),
    ));

    for (case, name) in &cases {
        assert_refusal(
            &router,
            case,
            "GetCrawler",
            &json!({ "Name": name }),
            "EntityNotFoundException",
            &format!("Crawler entry with name {name} does not exist"),
        )
        .await;
    }
}

/// A duplicate `CreateCrawler` is refused with a constant that states the resource kind
/// and withholds the requested name, so the error body discloses nothing the caller
/// sent.
///
/// The rows differ along the name's value, casing and length.
#[tokio::test]
async fn a_duplicate_crawler_create_withholds_the_name() {
    let router = emulator();

    let cases: Vec<(&str, String)> = vec![
        ("recorded shape", "sim2real-crawler".to_string()),
        ("mixed case", "Curated-Crawler".to_string()),
        ("at the 255 ceiling", "c".repeat(255)),
    ];

    for (case, name) in &cases {
        let name = name.as_str();
        ok(&router, "CreateCrawler", &crawler_body(name)).await;
        let body = assert_refusal(
            &router,
            case,
            "CreateCrawler",
            &crawler_body(name),
            "AlreadyExistsException",
            "Crawler already exists.",
        )
        .await;
        assert_withholds(case, &error_message(&body), name);
    }
}

// ---------------------------------------------------------------------------
// Databases
// ---------------------------------------------------------------------------

/// A duplicate `CreateDatabase` is refused with a constant that withholds the requested
/// database name — the recorded wording.
///
/// The rows differ along the name's value, casing and length; the mixed-case row also
/// covers the fold the catalog applies on store, which must not reach the message.
#[tokio::test]
async fn a_duplicate_database_create_withholds_the_name() {
    let router = emulator();

    let cases: Vec<(&str, String)> = vec![
        (
            "recorded shape",
            "sim2real_probe_glue_sim2real_probe_db_00614834".to_string(),
        ),
        ("single character", "d".to_string()),
        ("mixed case", "Warehouse_DB".to_string()),
        ("at the 255 ceiling", "e".repeat(255)),
    ];

    for (case, name) in &cases {
        let name = name.as_str();
        create_database(&router, name).await;
        let body = assert_refusal(
            &router,
            case,
            "CreateDatabase",
            &json!({ "DatabaseInput": { "Name": name } }),
            "AlreadyExistsException",
            "Database already exists.",
        )
        .await;
        assert_withholds(case, &error_message(&body), name);
    }

    // `UpdateDatabase` collides the same way when its rename lands on a taken name, and
    // answers the same constant.
    create_database(&router, "rename_source").await;
    create_database(&router, "rename_destination").await;
    let body = assert_refusal(
        &router,
        "rename onto a taken name",
        "UpdateDatabase",
        &json!({
            "Name": "rename_source",
            "DatabaseInput": { "Name": "rename_destination" },
        }),
        "AlreadyExistsException",
        "Database already exists.",
    )
    .await;
    assert_withholds(
        "rename onto a taken name",
        &error_message(&body),
        "rename_destination",
    );
}

/// Every operation that NAMES the database it could not resolve answers the same database
/// template, built from the requested name alone — rendered in the Hive-compatible fold the
/// catalog keys a name under, with no catalog id, no account id and no other member of the
/// request.
///
/// The rows are the cross product of the name axis of [`absent_names`] with every
/// operation that resolves a database and names it: the three that address it by `Name`
/// (`GetDatabase`, `DeleteDatabase`, `UpdateDatabase`) and the five that address it by
/// `DatabaseName` while naming tables (`GetTables`, `CreateTable`, `UpdateTable`,
/// `DeleteTable`, `BatchDeleteTable`). `GetTable` is NOT one of them: it words both of its
/// misses in the fixed sentence naming no entity, which
/// [`the_table_read_and_the_table_writes_refuse_with_different_wording`] pins.
#[tokio::test]
async fn a_database_miss_names_the_requested_database_and_nothing_else() {
    let router = emulator();
    // A database that DOES exist, so the refusals below are about the named one and not
    // about an empty catalog.
    create_database(&router, "present_db").await;

    for (case, name) in &absent_names() {
        let expected = format!("Database {} not found.", name.to_ascii_lowercase());
        let by_name: [(&str, Value); 3] = [
            ("GetDatabase", json!({ "Name": name })),
            ("DeleteDatabase", json!({ "Name": name })),
            (
                "UpdateDatabase",
                json!({ "Name": name, "DatabaseInput": { "Name": name } }),
            ),
        ];
        let by_database_name: [(&str, Value); 5] = [
            ("GetTables", json!({ "DatabaseName": name })),
            (
                "CreateTable",
                json!({ "DatabaseName": name, "TableInput": { "Name": "any_table" } }),
            ),
            (
                "UpdateTable",
                json!({ "DatabaseName": name, "TableInput": { "Name": "any_table" } }),
            ),
            (
                "DeleteTable",
                json!({ "DatabaseName": name, "Name": "any_table" }),
            ),
            (
                "BatchDeleteTable",
                json!({ "DatabaseName": name, "TablesToDelete": ["any_table"] }),
            ),
        ];

        for (operation, request) in by_name.iter().chain(by_database_name.iter()) {
            let label = format!("{case} via {operation}");
            let body = assert_refusal(
                &router,
                &label,
                operation,
                request,
                "EntityNotFoundException",
                &expected,
            )
            .await;
            assert_withholds(&label, &error_message(&body), ACCOUNT_ID);
        }
    }
}

// ---------------------------------------------------------------------------
// Tables
// ---------------------------------------------------------------------------

/// `GetTable` on one side and the table writes (`DeleteTable`, `UpdateTable`) on the
/// other refuse a table the database does not hold with their OWN wording, which a
/// client can tell apart: the read answers a constant naming neither table nor
/// database — for a missing DATABASE as much as for a missing table — and a write answers
/// a template built from the request's table name, folded as the catalog keys it, and
/// naming no database.
///
/// The rows span the name axis of [`absent_names`] and reach the miss two ways — a
/// table that never existed, and one created and then deleted.
#[tokio::test]
async fn the_table_read_and_the_table_writes_refuse_with_different_wording() {
    let router = emulator();
    create_database(&router, "present_db").await;

    let mut cases: Vec<(String, String)> = absent_names();
    // A table that existed and was deleted is just as absent as one that never was.
    create_table(&router, "present_db", "deleted_table").await;
    ok(
        &router,
        "DeleteTable",
        &json!({ "DatabaseName": "present_db", "Name": "deleted_table" }),
    )
    .await;
    cases.push((
        "created then deleted".to_string(),
        "deleted_table".to_string(),
    ));

    for (case, name) in &cases {
        let folded = name.to_ascii_lowercase();
        let read = assert_refusal(
            &router,
            &format!("{case} via GetTable"),
            "GetTable",
            &json!({ "DatabaseName": "present_db", "Name": name }),
            "EntityNotFoundException",
            "Entity Not Found",
        )
        .await;
        // The read's refusal names neither the table it could not find nor its parent.
        assert_withholds(case, &error_message(&read), name);
        assert_withholds(case, &error_message(&read), "present_db");

        let deleted = assert_refusal(
            &router,
            &format!("{case} via DeleteTable"),
            "DeleteTable",
            &json!({ "DatabaseName": "present_db", "Name": name }),
            "EntityNotFoundException",
            &format!("Table {folded} not found."),
        )
        .await;
        // The delete's refusal names the table, but never its parent database.
        assert_withholds(case, &error_message(&deleted), "present_db");
        assert_ne!(
            error_message(&read),
            error_message(&deleted),
            "case {case}: the read and the delete must be distinguishable by message, \
             both answered {:?}",
            error_message(&read)
        );

        // The other write that addresses one table answers the same write wording.
        let updated = assert_refusal(
            &router,
            &format!("{case} via UpdateTable"),
            "UpdateTable",
            &json!({ "DatabaseName": "present_db", "TableInput": { "Name": name } }),
            "EntityNotFoundException",
            &format!("Table {folded} not found."),
        )
        .await;
        assert_withholds(case, &error_message(&updated), "present_db");
    }
}

/// A duplicate `CreateTable` is refused with a constant that withholds the requested
/// table name, as the duplicate database create does.
///
/// The rows differ along the table name's value, casing and length.
#[tokio::test]
async fn a_duplicate_table_create_withholds_the_name() {
    let router = emulator();
    create_database(&router, "present_db").await;

    let cases: Vec<(&str, String)> = vec![
        (
            "recorded shape",
            "sim2real_probe_glue_sim2real_probe_tbl_00614834".to_string(),
        ),
        ("single character", "t".to_string()),
        ("mixed case", "Sales_By_Day".to_string()),
        ("at the 255 ceiling", "u".repeat(255)),
    ];

    for (case, name) in &cases {
        let name = name.as_str();
        create_table(&router, "present_db", name).await;
        let body = assert_refusal(
            &router,
            case,
            "CreateTable",
            &json!({ "DatabaseName": "present_db", "TableInput": { "Name": name } }),
            "AlreadyExistsException",
            "Table already exists.",
        )
        .await;
        assert_withholds(case, &error_message(&body), name);
    }
}
