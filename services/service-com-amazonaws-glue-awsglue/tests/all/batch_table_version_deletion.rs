// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `BatchDeleteTableVersion`: dropping SEVERAL versions out of ONE
//! table's version history in a single call.
//!
//! The rule under test: a batch version delete removes, from the table the request names,
//! the archived version each addressed `VersionId` identifies — by the COUNTER the id
//! spells, so a padded spelling names the same version — and leaves the table, its current
//! version and every version the batch did not address exactly as the writes left them. An
//! item the history cannot give up fails ONLY its own item, reported in `Errors` under the
//! id that item sent: a version the history does not hold (including one an earlier item of
//! the same batch just deleted) as `EntityNotFoundException`, and an id that spells no
//! integer or that addresses the table's own current version — the live definition, which
//! only `DeleteTable` removes — as `InvalidInputException`. The database and the table are
//! named once by the request rather than per item, so an absent one refuses the whole call
//! with `EntityNotFoundException` and deletes nothing.
//!
//! Every call goes over the wire the way a client makes it, through the shared
//! [`crate::wire`] harness, dispatched by the same `MultiServiceRouter` the production
//! front door uses.
//!
//! The suites are tables whose rows differ along what the rule generalises over: how many
//! writes the table has had and which member set each one submitted (the histories of
//! [`crate::table_versions`]), how many versions one batch addresses (none, one, several,
//! the modelled ceiling), the order it addresses them in, the spelling of an id, and the
//! way one item can fail.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::error_messages::assert_refusal;
use crate::table_version_deletion::{
    listed_versions, versions_of, LATEST_REFUSAL, MISSING_VERSION_REFUSAL,
};
use crate::table_versions::{get_table, histories, write_history, TABLE};
use crate::wire::{call, emulator, error_code, error_message, ok, ERROR_DETAIL_TAG};

/// The largest `VersionIds` one batch may carry: `BatchDeleteTableVersionList`, length
/// 0..100 (source: `smithy_query shape=BatchDeleteTableVersionList`).
const MAX_VERSIONS_PER_BATCH: usize = 100;

/// The error code a per-item failure carries for a version the history does not hold.
const MISS_CODE: &str = "EntityNotFoundException";

/// The per-item wording a batch reports for a version the table's history does not hold.
/// A DIFFERENT sentence from the one a READ or a single-version delete of that version
/// answers ([`MISSING_VERSION_REFUSAL`]): the batch answers 200 and reports this failure
/// inside `Errors`, so the message carries the `VersionId` the item sent, in parentheses,
/// which is what tells a caller which of its items failed.
fn missing_item_refusal(version_id: &str) -> String {
    format!("Table version ({version_id}) not found")
}

/// The error code a per-item failure carries for an id that addresses no deletable
/// version: one that spells no integer, and the table's own current version.
const INVALID_CODE: &str = "InvalidInputException";

/// The refusal an id that spells no version counter earns, as the service words it over
/// the value the request sent.
fn malformed_refusal(version_id: &str) -> String {
    format!("VersionId {version_id} is not a string representation of an integer")
}

/// The request body of a batch version delete.
fn request(database: &str, table: &str, version_ids: &[String]) -> Value {
    json!({
        "DatabaseName": database,
        "TableName": table,
        "VersionIds": version_ids,
    })
}

/// Submit one `BatchDeleteTableVersion` that must answer 200, and return its body.
async fn batch_delete(
    router: &MultiServiceRouter,
    database: &str,
    version_ids: &[String],
) -> Value {
    ok(
        router,
        "BatchDeleteTableVersion",
        &request(database, TABLE, version_ids),
    )
    .await
}

/// The `Errors` entries of a batch answer, in the order served.
///
/// The member is reported whether or not anything failed, so its ABSENCE is a defect
/// rather than an empty list: a client checks an empty `Errors` before treating the batch
/// as done.
fn version_errors(case: &str, body: &Value) -> Vec<Value> {
    body["Errors"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: Errors must be a list, got {body}"))
        .clone()
}

/// `ids` as the owned strings a request carries.
fn ids(items: &[&str]) -> Vec<String> {
    items.iter().map(|item| (*item).to_string()).collect()
}

// ---------------------------------------------------------------------------
// Delete -> read round trip
// ---------------------------------------------------------------------------

/// One row of the round-trip table: the ids one batch sends over a history, and the
/// counters it must delete out of it.
struct BatchCase {
    case: String,
    sent: Vec<String>,
    deleted: Vec<usize>,
}

/// The batches run over each history: every archived version at once in both orders, the
/// oldest alone, and the newest archived one addressed by a zero-padded id.
fn batches_over(archived: &[usize]) -> Vec<BatchCase> {
    let newest_archived = *archived.last().expect("a history with an archived version");
    let mut cases = vec![
        BatchCase {
            case: "every archived version, oldest first".to_string(),
            sent: archived.iter().map(usize::to_string).collect(),
            deleted: archived.to_vec(),
        },
        BatchCase {
            case: "every archived version, newest first".to_string(),
            sent: archived.iter().rev().map(usize::to_string).collect(),
            deleted: archived.to_vec(),
        },
        BatchCase {
            case: "the oldest archived version alone".to_string(),
            sent: vec![archived[0].to_string()],
            deleted: vec![archived[0]],
        },
        BatchCase {
            // The version addressed is the counter the id spells, not the string sent.
            case: "the newest archived version, zero-padded".to_string(),
            sent: vec![format!("0{newest_archived}")],
            deleted: vec![newest_archived],
        },
    ];
    cases.dedup_by(|left, right| left.sent == right.sent);
    cases
}

/// A batch delete removes every archived version it addresses and nothing else: each one
/// stops being readable and listable, while the live table, its current version and every
/// surviving version read back exactly as before — and the answer reports no per-item
/// error.
#[tokio::test]
async fn a_batch_removes_every_addressed_archived_version_and_leaves_the_others() {
    for (history_case, database, history) in histories() {
        // The last definition is the live one, so only the earlier ones are archived.
        let archived: Vec<usize> = (0..history.len().saturating_sub(1)).collect();
        if archived.is_empty() {
            continue;
        }
        for row in batches_over(&archived) {
            let case = format!("{history_case}, {}", row.case);
            let router = emulator();
            write_history(&router, database, &history).await;

            let survivors: Vec<usize> = (0..history.len())
                .filter(|counter| !row.deleted.contains(counter))
                .collect();
            let before_table = get_table(&router, database).await;
            let before_versions = versions_of(&router, database, &survivors).await;

            let body = batch_delete(&router, database, &row.sent).await;
            let reported = version_errors(&case, &body);
            assert!(
                reported.is_empty(),
                "case {case}: every addressed version is archived, so the batch must report no \
                 per-item error, got {:?}",
                body["Errors"]
            );

            // Every deleted version is gone from the single-version read.
            for counter in &row.deleted {
                let (status, answer) = call(
                    &router,
                    "GetTableVersion",
                    &json!({
                        "DatabaseName": database,
                        "TableName": TABLE,
                        "VersionId": counter.to_string(),
                    }),
                )
                .await;
                assert_eq!(
                    status, 400,
                    "case {case}: the deleted version {counter} is no longer readable, got \
                     {status} with {answer}"
                );
                assert!(
                    error_code(&answer).ends_with(MISS_CODE),
                    "case {case}: expected {MISS_CODE} for version {counter}, got {}",
                    error_code(&answer)
                );
                assert_eq!(
                    error_message(&answer),
                    MISSING_VERSION_REFUSAL,
                    "case {case}: expected the refusal {MISSING_VERSION_REFUSAL:?} for version \
                     {counter}, got {:?}",
                    error_message(&answer)
                );
            }

            // ... and from the history listing, which still holds every other version.
            let expected: Vec<String> = survivors.iter().map(usize::to_string).collect();
            let listed = listed_versions(&router, database).await;
            assert_eq!(
                listed, expected,
                "case {case}: the listing holds every surviving version and no other; expected \
                 {expected:?}, got {listed:?}"
            );

            // The table itself, and every surviving version, are untouched.
            let after_table = get_table(&router, database).await;
            assert_eq!(
                after_table, before_table,
                "case {case}: the live table must be untouched; expected {before_table}, got \
                 {after_table}"
            );
            let after_versions = versions_of(&router, database, &survivors).await;
            assert_eq!(
                after_versions, before_versions,
                "case {case}: every surviving version must read back the same; expected \
                 {before_versions:?}, got {after_versions:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Per-item failures
// ---------------------------------------------------------------------------

/// The history every per-item row is written over: four writes, so versions 0, 1 and 2 are
/// archived and 3 is the live definition.
fn four_writes() -> Vec<Value> {
    vec![
        json!({ "Name": TABLE, "Description": "as created" }),
        json!({ "Name": TABLE, "Description": "as updated once" }),
        json!({ "Name": TABLE, "Description": "as updated twice" }),
        json!({ "Name": TABLE, "Description": "as updated three times" }),
    ]
}

/// One row of the per-item failure table.
struct FailureCase {
    case: &'static str,
    /// The ids the batch sends, in order.
    sent: Vec<String>,
    /// One expected `Errors` entry per failing item, in order: the `VersionId` it reports,
    /// its `ErrorCode` and its `ErrorMessage`.
    expected: Vec<(&'static str, &'static str, String)>,
    /// The counters the listing must still serve after the batch, in order.
    survivors: Vec<usize>,
}

/// An item the table's history cannot give up fails only its own item: the batch answers
/// 200, names that item in `Errors` under the id it sent, and deletes every other item of
/// the same batch.
///
/// The rows differ along the way one item fails — a counter past the newest, the table's
/// current version (plainly and zero-padded), an id that names no number, no integer and
/// no counter, and the same version addressed twice in one batch — and along how many
/// items of the batch succeed beside it.
#[tokio::test]
async fn an_item_the_history_cannot_give_up_fails_only_itself() {
    let history = four_writes();
    let cases = vec![
        FailureCase {
            case: "a counter past the newest version",
            sent: ids(&["7"]),
            expected: vec![("7", MISS_CODE, missing_item_refusal("7"))],
            survivors: vec![0, 1, 2, 3],
        },
        FailureCase {
            case: "the table's current version",
            sent: ids(&["3"]),
            expected: vec![("3", INVALID_CODE, LATEST_REFUSAL.to_string())],
            survivors: vec![0, 1, 2, 3],
        },
        FailureCase {
            case: "the table's current version, zero-padded",
            sent: ids(&["03"]),
            expected: vec![("03", INVALID_CODE, LATEST_REFUSAL.to_string())],
            survivors: vec![0, 1, 2, 3],
        },
        FailureCase {
            case: "an id that names no number",
            sent: ids(&["oldest"]),
            expected: vec![("oldest", INVALID_CODE, malformed_refusal("oldest"))],
            survivors: vec![0, 1, 2, 3],
        },
        FailureCase {
            case: "an id that names no integer",
            sent: ids(&["1.5"]),
            expected: vec![("1.5", INVALID_CODE, malformed_refusal("1.5"))],
            survivors: vec![0, 1, 2, 3],
        },
        FailureCase {
            case: "an id that names no counter",
            sent: ids(&["-1"]),
            expected: vec![("-1", INVALID_CODE, malformed_refusal("-1"))],
            survivors: vec![0, 1, 2, 3],
        },
        FailureCase {
            case: "the same archived version addressed twice in one batch",
            sent: ids(&["0", "0"]),
            expected: vec![("0", MISS_CODE, missing_item_refusal("0"))],
            survivors: vec![1, 2, 3],
        },
        FailureCase {
            case: "two deletes beside three kinds of failing item",
            sent: ids(&["0", "7", "1", "nope", "3"]),
            expected: vec![
                ("7", MISS_CODE, missing_item_refusal("7")),
                ("nope", INVALID_CODE, malformed_refusal("nope")),
                ("3", INVALID_CODE, LATEST_REFUSAL.to_string()),
            ],
            survivors: vec![2, 3],
        },
        FailureCase {
            case: "every archived version and the current one",
            sent: ids(&["0", "1", "2", "3"]),
            expected: vec![("3", INVALID_CODE, LATEST_REFUSAL.to_string())],
            survivors: vec![3],
        },
        FailureCase {
            case: "an archived version addressed by a padded id, which fails nothing",
            sent: ids(&["01"]),
            expected: vec![],
            survivors: vec![0, 2, 3],
        },
    ];

    for row in &cases {
        let case = row.case;
        let router = emulator();
        write_history(&router, "batch_failing_db", &history).await;

        let body = batch_delete(&router, "batch_failing_db", &row.sent).await;
        let reported = version_errors(case, &body);
        assert_eq!(
            reported.len(),
            row.expected.len(),
            "case {case}: expected {} per-item error(s), got {:?}",
            row.expected.len(),
            body["Errors"]
        );
        for (position, (entry, (version_id, error_code, message))) in
            reported.iter().zip(row.expected.iter()).enumerate()
        {
            assert_eq!(
                entry["TableName"].as_str(),
                Some(TABLE),
                "case {case}: error {position} must name the table its item addressed, got {entry}"
            );
            assert_eq!(
                entry["VersionId"].as_str(),
                Some(*version_id),
                "case {case}: error {position} must name the version id its item sent, got {entry}"
            );
            let detail = &entry["ErrorDetail"];
            assert_eq!(
                detail["ErrorCode"].as_str(),
                Some(*error_code),
                "case {case}: error {position} must carry ErrorCode {error_code}, got {entry}"
            );
            assert_eq!(
                detail["ErrorMessage"].as_str(),
                Some(message.as_str()),
                "case {case}: error {position} must carry the message {message:?}, got {entry}"
            );
            assert_eq!(
                detail["__type"].as_str(),
                Some(ERROR_DETAIL_TAG),
                "case {case}: error {position} must carry the ErrorDetail type tag, got {entry}"
            );
        }

        // A failing item deletes nothing of its own, and never anything of another item's:
        // exactly the versions the batch could delete are gone.
        let expected: Vec<String> = row.survivors.iter().map(usize::to_string).collect();
        let listed = listed_versions(&router, "batch_failing_db").await;
        assert_eq!(
            listed, expected,
            "case {case}: expected the surviving versions {expected:?}, got {listed:?}"
        );
        // The live definition is never one of them, so the table still reads back as the
        // last write left it.
        let table = get_table(&router, "batch_failing_db").await;
        assert_eq!(
            table["Description"].as_str(),
            Some("as updated three times"),
            "case {case}: the live table must be untouched, got {table}"
        );
    }
}

// ---------------------------------------------------------------------------
// The modelled bounds of one batch
// ---------------------------------------------------------------------------

/// An empty `VersionIds` is a valid input (`BatchDeleteTableVersionList`, length 0..100):
/// it deletes nothing and reports no per-item error. A batch at the ceiling is served, and
/// one past it is refused before any version is deleted.
#[tokio::test]
async fn a_batch_is_served_from_no_version_up_to_the_modelled_ceiling() {
    let history = four_writes();
    let whole: Vec<String> = (0..history.len()).map(|c| c.to_string()).collect();

    // The empty batch.
    let router = emulator();
    write_history(&router, "batch_bounds_db", &history).await;
    let body = batch_delete(&router, "batch_bounds_db", &[]).await;
    assert!(
        version_errors("an empty batch", &body).is_empty(),
        "case an empty batch: no item can fail, got {body}"
    );
    let listed = listed_versions(&router, "batch_bounds_db").await;
    assert_eq!(
        listed, whole,
        "case an empty batch: it deletes nothing, so the whole history stands; expected \
         {whole:?}, got {listed:?}"
    );

    // The ceiling: a hundred ids, of which the three archived versions are deletable, the
    // current one is refused and every other counter is a miss.
    let router = emulator();
    write_history(&router, "batch_bounds_db", &history).await;
    let at_ceiling: Vec<String> = (0..MAX_VERSIONS_PER_BATCH)
        .map(|counter| counter.to_string())
        .collect();
    let body = batch_delete(&router, "batch_bounds_db", &at_ceiling).await;
    let reported = version_errors("a batch at the ceiling", &body);
    // Counters 0..2 are archived and deleted; 3 is the live version; 4..99 name nothing.
    let expected_failures = MAX_VERSIONS_PER_BATCH - (history.len() - 1);
    assert_eq!(
        reported.len(),
        expected_failures,
        "case a batch of {MAX_VERSIONS_PER_BATCH} ids: expected {expected_failures} per-item \
         errors, got {}",
        reported.len()
    );
    let listed = listed_versions(&router, "batch_bounds_db").await;
    assert_eq!(
        listed,
        vec![(history.len() - 1).to_string()],
        "case a batch at the ceiling: every archived version is deleted and the current one \
         stands, got {listed:?}"
    );

    // One id past the ceiling: the modelled constraint refuses the request, so nothing it
    // names is deleted.
    let router = emulator();
    write_history(&router, "batch_bounds_db", &history).await;
    let over: Vec<String> = (0..=MAX_VERSIONS_PER_BATCH)
        .map(|counter| counter.to_string())
        .collect();
    let (status, answer) = call(
        &router,
        "BatchDeleteTableVersion",
        &request("batch_bounds_db", TABLE, &over),
    )
    .await;
    assert_eq!(
        status, 400,
        "case a batch of {} ids: it is past the ceiling of {MAX_VERSIONS_PER_BATCH} and must be \
         refused, got {status} with {answer}",
        over.len()
    );
    let listed = listed_versions(&router, "batch_bounds_db").await;
    assert_eq!(
        listed, whole,
        "case a batch past the ceiling: a refused request deletes nothing; expected {whole:?}, \
         got {listed:?}"
    );
}

// ---------------------------------------------------------------------------
// Whole-request refusals
// ---------------------------------------------------------------------------

/// The database and the table are named once by the request rather than per item, so an
/// absent one — and a catalog of another account, which no resource policy grants — fails
/// the WHOLE batch and deletes nothing.
#[tokio::test]
async fn a_batch_naming_no_stored_table_is_refused_whole() {
    let history = four_writes();
    let router = emulator();
    write_history(&router, "batch_present_db", &history).await;
    let whole: Vec<String> = (0..history.len()).map(|c| c.to_string()).collect();
    let sent = ids(&["0", "1"]);

    // (case, request, expected error code, expected message)
    let cases: Vec<(&str, Value, &str, String)> = vec![
        (
            "a database the catalog does not hold",
            request("absent_db", TABLE, &sent),
            MISS_CODE,
            "Database absent_db not found.".to_string(),
        ),
        (
            "a database the catalog does not hold, mixed case",
            request("Absent_DB", TABLE, &sent),
            MISS_CODE,
            "Database absent_db not found.".to_string(),
        ),
        (
            "a table the database does not hold",
            request("batch_present_db", "absent", &sent),
            MISS_CODE,
            // The table miss of a version operation names neither the table nor the
            // database, exactly as the single-version delete and `GetTable` word it.
            "Entity Not Found".to_string(),
        ),
    ];

    for (case, body, expected_code, expected_message) in &cases {
        assert_refusal(
            &router,
            case,
            "BatchDeleteTableVersion",
            body,
            expected_code,
            expected_message,
        )
        .await;
    }

    // A `CatalogId` of another account is refused before any entity is looked up, so the
    // refusal names the IAM action rather than the missing entity.
    let mut foreign = request("batch_present_db", TABLE, &sent);
    foreign["CatalogId"] = json!("999999999999");
    let (status, answer) = call(&router, "BatchDeleteTableVersion", &foreign).await;
    assert_eq!(
        status, 400,
        "case a catalog of another account: expected 400, got {status} with {answer}"
    );
    assert!(
        error_code(&answer).ends_with("AccessDeniedException"),
        "case a catalog of another account: expected AccessDeniedException, got {}",
        error_code(&answer)
    );
    assert!(
        error_message(&answer).contains("glue:BatchDeleteTableVersion"),
        "case a catalog of another account: the refusal names the IAM action this operation \
         performs, got {:?}",
        error_message(&answer)
    );

    // Every refusal wrote nothing: the whole history still stands.
    let listed = listed_versions(&router, "batch_present_db").await;
    assert_eq!(
        listed, whole,
        "case after every whole-request refusal: the history is whole; expected {whole:?}, got \
         {listed:?}"
    );
}
