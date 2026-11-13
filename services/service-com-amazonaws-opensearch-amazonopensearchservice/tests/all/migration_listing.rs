// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListMigrations`: the listing of the saved-object migration
//! jobs one OpenSearch application holds.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. The harness comes from [`super::domain_lifecycle`]
//! and the application a migration migrates into from
//! [`super::data_source_attachments`]; this suite adds no second copy of either, and
//! leaves [`super::saved_object_migrations`] to assert the start and the single read.
//!
//! The tests assert RULES over sets of inputs, not one recorded call:
//!
//! * the listing reports every job the named application holds, each from the SAME
//!   stored record its own `GetMigration` read answers with, over a table of listings
//!   differing in how many jobs the application holds (none, one, several) and in the
//!   export scope each job names;
//! * it is scoped to the application it names: a job of another application is never in
//!   it;
//! * `maxResults` bounds the page and `nextToken` names the job the next page starts at,
//!   so the pages of any page size concatenate to exactly the whole listing, in one
//!   order, with no job reported twice and no token on the last page;
//! * `status` selects the jobs in one published status, and the clock moving a job moves
//!   which filter selects it — the listing materialises what the clock owes a job just as
//!   a single read does;
//! * a negative `maxResults`, a `status` outside the published set and a `nextToken` the
//!   listing never issued are each refused with the modelled `ValidationException`, and an
//!   `applicationId` naming no application of this account with the modelled
//!   missing-resource refusal.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_opensearch_amazonopensearchservice::state::{
    MIGRATION_STATUS_FAILED, MIGRATION_STATUS_IN_PROGRESS, MIGRATION_STATUS_PENDING,
    MIGRATION_STATUS_SUCCEEDED, MIGRATION_STATUS_VALUES, PROVISIONING,
};

use super::data_source_attachments::application;
use super::domain_lifecycle::{
    error_code, string_member, Answer, Emulator, PREFIX, RECORDED_NOT_FOUND_STATUS,
};

/// The ARN of the data source every job below migrates saved objects out of: the search
/// COLLECTION shape `StartMigration` validates a source against.
const SOURCE: &str = "arn:aws:aoss:us-east-1:123456789012:collection/listedobjects";

/// `ListMigrations`: `GET /2021-01-01/opensearch/app-migrations?applicationId=…`.
pub(crate) async fn list_migrations(
    emulator: &Emulator,
    application_id: &str,
    query: &str,
) -> Answer {
    emulator
        .call(
            "GET",
            &format!("{PREFIX}/opensearch/app-migrations?applicationId={application_id}{query}"),
            None,
        )
        .await
}

/// `GetMigration`: `GET /2021-01-01/opensearch/app-migrations/{migrationId}`.
async fn get_migration(emulator: &Emulator, migration_id: &str) -> Answer {
    emulator
        .call(
            "GET",
            &format!("{PREFIX}/opensearch/app-migrations/{migration_id}"),
            None,
        )
        .await
}

/// Start one job of `application_id` whose export names `objects` distinct saved
/// objects, answering with the `migrationId` it was minted under.
async fn start(emulator: &Emulator, application_id: &str, objects: usize) -> String {
    let scope: Vec<Value> = (0..objects)
        .map(|index| json!({ "type": "dashboard", "id": format!("board-{index}") }))
        .collect();
    let mut options = json!({
        "source": { "datasourceArn": SOURCE },
        "workspace": { "workspaceId": "ws-listing" },
    });
    if !scope.is_empty() {
        options["exportOptions"] = json!({ "objects": scope });
    }
    let answer = emulator
        .call(
            "POST",
            &format!("{PREFIX}/opensearch/app-migrations"),
            Some(json!({ "applicationId": application_id, "migrationOptions": options })),
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "case StartMigration({application_id}, {objects} object(s)): expected 200, got {} \
         with {:?}",
        answer.status, answer.body
    );
    string_member(&answer.body, "migrationId", "a start this suite made")
}

/// How long to wait for every step the clock owes a job to have fallen due.
///
/// A job takes one provisioning span per step — the export, then the import that follows
/// it — so the wait is both spans plus a margin, taken from the span itself rather than
/// from a second copy of its length.
fn whole_job() -> Duration {
    PROVISIONING * 3
}

/// The `migrationId`s a listing answer reports, in the order it reported them.
fn listed_ids(answer: &Answer, case: &str) -> Vec<String> {
    answer.body["migrations"]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case {case}: expected a migrations list, got {:?}",
                answer.body
            )
        })
        .iter()
        .map(|row| string_member(row, "migrationId", case))
        .collect()
}

/// RULE: the listing reports every job the named application holds, each from the SAME
/// stored record a single read of that job answers with.
#[tokio::test]
async fn the_listing_reports_every_job_of_the_application_from_the_stored_record() {
    // (case, the number of jobs the application holds, the export scope of each)
    let rows: Vec<(&str, &str, Vec<usize>)> = vec![
        ("an application holding no job", "listing-none", vec![]),
        ("one job, nothing filtered out", "listing-one", vec![0]),
        ("two jobs of different scopes", "listing-two", vec![2, 0]),
        (
            "several jobs of different scopes",
            "listing-several",
            vec![1, 4, 0, 7, 2],
        ),
    ];

    for (case, name, scopes) in rows {
        let emulator = Emulator::new();
        let id = application(&emulator, name).await;
        let mut started: Vec<String> = Vec::new();
        for objects in &scopes {
            started.push(start(&emulator, &id, *objects).await);
        }

        let answer = list_migrations(&emulator, &id, "").await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected 200 from ListMigrations, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            listed_ids(&answer, case),
            started,
            "case {case:?}: expected the jobs the application holds, in the order they were \
             started, got {:?}",
            answer.body
        );
        assert_eq!(
            answer.body["nextToken"],
            Value::Null,
            "case {case:?}: a listing that ended carries no token, got {:?}",
            answer.body
        );

        // Both paths answer from the one stored record, so every member the summary and
        // the single read share has to agree.
        let listed = answer.body["migrations"].as_array().cloned().unwrap_or_default();
        for row in &listed {
            let migration_id = string_member(row, "migrationId", case);
            let read = get_migration(&emulator, &migration_id).await;
            assert_eq!(
                read.status, 200,
                "case {case:?}: expected 200 from GetMigration({migration_id}), got {} with \
                 {:?}",
                read.status, read.body
            );
            for member in [
                "migrationId",
                "applicationId",
                "status",
                "source",
                "exportedCount",
                "importedCount",
                "createdAt",
            ] {
                assert_eq!(
                    row[member], read.body[member],
                    "case {case:?}: expected the listing's {member} to be the stored record's, \
                     got {:?} in the listing and {:?} from the read",
                    row[member], read.body[member]
                );
            }
            assert_eq!(
                row["applicationId"],
                json!(id),
                "case {case:?}: expected every row to belong to the application the listing \
                 names, got {row:?}"
            );
            assert!(
                MIGRATION_STATUS_VALUES
                    .contains(&string_member(row, "status", case).as_str()),
                "case {case:?}: expected one of the published statuses \
                 {MIGRATION_STATUS_VALUES:?}, got {row:?}"
            );
        }
    }
}

/// RULE: the listing is the named application's, so a job of another application is
/// never in it.
#[tokio::test]
async fn the_listing_is_scoped_to_the_application_it_names() {
    let emulator = Emulator::new();
    let one = application(&emulator, "listing-scope-one").await;
    let other = application(&emulator, "listing-scope-other").await;

    let mut of_one = Vec::new();
    let mut of_other = Vec::new();
    // Interleaved, so neither listing could be right by the order the store happens to
    // hold the records in.
    for round in 0..3 {
        of_one.push(start(&emulator, &one, round).await);
        of_other.push(start(&emulator, &other, round + 1).await);
    }

    for (case, id, expected) in [
        ("the first application", &one, &of_one),
        ("the second application", &other, &of_other),
    ] {
        let answer = list_migrations(&emulator, id, "").await;
        assert_eq!(
            listed_ids(&answer, case),
            *expected,
            "case {case:?}: expected only the jobs of {id}, got {:?}",
            answer.body
        );
    }
}

/// RULE: `maxResults` bounds the page and `nextToken` names the job the next page starts
/// at, so the pages of any page size concatenate to exactly the whole listing, in one
/// order, with no job reported twice and no token on the last page.
#[tokio::test]
async fn any_page_size_walks_the_whole_listing_once() {
    let emulator = Emulator::new();
    let id = application(&emulator, "listing-pages").await;
    let mut started = Vec::new();
    for objects in 0..5 {
        started.push(start(&emulator, &id, objects).await);
    }

    // Every page size from one row to one past the whole listing, plus the modelled
    // default of zero — the request that asks for no particular size.
    for page_size in [0usize, 1, 2, 3, 4, 5, 6] {
        let case = format!("page size {page_size}");
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        for page in 0..started.len() + 2 {
            let query = match (&token, page_size) {
                (None, 0) => String::new(),
                (None, size) => format!("&maxResults={size}"),
                (Some(next), 0) => format!("&nextToken={next}"),
                (Some(next), size) => format!("&maxResults={size}&nextToken={next}"),
            };
            let answer = list_migrations(&emulator, &id, &query).await;
            assert_eq!(
                answer.status, 200,
                "case {case} page {page}: expected 200, got {} with {:?}",
                answer.status, answer.body
            );
            let ids = listed_ids(&answer, &case);
            let bound = if page_size == 0 {
                started.len()
            } else {
                page_size
            };
            assert!(
                ids.len() <= bound,
                "case {case} page {page}: expected at most {bound} row(s), got {}",
                ids.len()
            );
            walked.extend(ids);
            token = answer.body["nextToken"].as_str().map(str::to_owned);
            if token.is_none() {
                break;
            }
            assert_eq!(
                answer.body["migrations"]
                    .as_array()
                    .map(|rows| rows.len())
                    .unwrap_or_default(),
                bound,
                "case {case} page {page}: a page that left more to report is a FULL page, got \
                 {:?}",
                answer.body
            );
        }
        assert_eq!(
            token, None,
            "case {case}: expected the walk to end with no token, got {token:?}"
        );
        assert_eq!(
            walked, started,
            "case {case}: expected the pages to concatenate to the whole listing in one \
             order, got {walked:?}"
        );
    }
}

/// RULE: `status` selects the jobs in one published status, and the clock moving a job
/// moves which filter selects it — the listing materialises what the clock owes a job
/// just as a single read does.
#[tokio::test]
async fn a_status_filter_selects_the_jobs_in_that_status() {
    let emulator = Emulator::new();
    let id = application(&emulator, "listing-status-filter").await;
    let mut started = Vec::new();
    for objects in 0..3 {
        started.push(start(&emulator, &id, objects).await);
    }

    // The instant the starts returned, every job is in the status a start leaves it in,
    // and no other filter selects any of them.
    for (case, status, expected) in [
        ("pending straight after the starts", MIGRATION_STATUS_PENDING, started.clone()),
        ("in progress before the export ran", MIGRATION_STATUS_IN_PROGRESS, vec![]),
        ("succeeded before the import ran", MIGRATION_STATUS_SUCCEEDED, vec![]),
        ("failed, which nothing here is", MIGRATION_STATUS_FAILED, vec![]),
    ] {
        let answer = list_migrations(&emulator, &id, &format!("&status={status}")).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected 200, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            listed_ids(&answer, case),
            expected,
            "case {case:?}: expected the jobs in {status}, got {:?}",
            answer.body
        );
    }

    // Nothing asked for a move: the clock owes every job its export and its import, and
    // this listing is a call that materialises them.
    tokio::time::sleep(whole_job()).await;

    for (case, status, expected) in [
        ("succeeded once both steps fell due", MIGRATION_STATUS_SUCCEEDED, started.clone()),
        ("pending once the export ran", MIGRATION_STATUS_PENDING, vec![]),
        ("in progress once the import ran", MIGRATION_STATUS_IN_PROGRESS, vec![]),
    ] {
        let answer = list_migrations(&emulator, &id, &format!("&status={status}")).await;
        assert_eq!(
            listed_ids(&answer, case),
            expected,
            "case {case:?}: expected the jobs in {status}, got {:?}",
            answer.body
        );
    }

    // The settled record is what the unfiltered listing reports too, with the progress
    // the completed job made.
    let whole = list_migrations(&emulator, &id, "").await;
    let rows = whole.body["migrations"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        rows.len(),
        started.len(),
        "case the settled listing: expected every job the application holds, got {:?}",
        whole.body
    );
    for (index, row) in rows.iter().enumerate() {
        let case = format!("the settled job of {index} object(s)");
        assert_eq!(
            row["status"],
            json!(MIGRATION_STATUS_SUCCEEDED),
            "case {case}: a job both of whose steps have fallen due has completed, got {row:?}"
        );
        assert_eq!(
            (row["exportedCount"].as_i64(), row["importedCount"].as_i64()),
            (Some(index as i64), Some(index as i64)),
            "case {case}: expected the {index} selected object(s) exported and imported, got \
             {row:?}"
        );
    }
}

/// RULE: a request the listing cannot run — a page size that is no size, a status
/// outside the published set or a token it never issued — is refused with the modelled
/// `ValidationException`, an application this account does not hold with the modelled
/// missing-resource refusal, and the listing itself is unchanged.
#[tokio::test]
async fn a_request_the_listing_cannot_run_is_refused() {
    let emulator = Emulator::new();
    let id = application(&emulator, "listing-refusals").await;
    let other = application(&emulator, "listing-refusals-other").await;
    let mut started = Vec::new();
    for objects in 0..2 {
        started.push(start(&emulator, &id, objects).await);
    }
    let of_other = start(&emulator, &other, 1).await;

    let rows: Vec<(&str, String, String)> = vec![
        (
            "a negative page size",
            id.clone(),
            "&maxResults=-1".to_string(),
        ),
        (
            "a page size far below zero",
            id.clone(),
            format!("&maxResults={}", i32::MIN),
        ),
        (
            "a status outside the published set",
            id.clone(),
            "&status=CANCELLED".to_string(),
        ),
        (
            "a published status in another case",
            id.clone(),
            "&status=pending".to_string(),
        ),
        (
            "a token nobody minted",
            id.clone(),
            "&nextToken=mig-notatoken".to_string(),
        ),
        (
            "a token naming a job of another application",
            id.clone(),
            format!("&nextToken={of_other}"),
        ),
        (
            // A token is scoped to the listing it was issued under, and no job of this
            // control plane ever fails: a token naming a job the filter left out names
            // no place in the filtered listing.
            "a token naming a job the filter left out",
            id.clone(),
            format!("&status={MIGRATION_STATUS_FAILED}&nextToken={}", started[0]),
        ),
    ];

    for (case, application_id, query) in rows {
        let answer = list_migrations(&emulator, &application_id, &query).await;
        assert_eq!(
            (answer.status, error_code(&answer).as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected 400 ValidationException, got {} with {:?}",
            answer.status,
            answer.body
        );
    }

    // An application this account does not hold is a MISSING RESOURCE rather than a
    // malformed value: the listing states it with the shape every call addressed by an
    // application id states it with, naming the id the request carried. The rows differ
    // in the length and the alphabet of the id.
    for absent in ["nosuchapplication", "abc", "a1b2c3d4e5f6g7h8i9j0"] {
        let answer = list_migrations(&emulator, absent, "").await;
        assert_eq!(
            (answer.status, error_code(&answer).as_str()),
            (RECORDED_NOT_FOUND_STATUS, "ResourceNotFoundException"),
            "case {absent:?}: expected the modelled not-found, got {} with {:?}",
            answer.status,
            answer.body
        );
        let expected = format!("Application {absent} not found");
        assert_eq!(
            answer.body["message"].as_str().unwrap_or_default(),
            expected,
            "case {absent:?}: expected {expected:?}, got {:?}",
            answer.body
        );
    }

    let answer = list_migrations(&emulator, &id, "").await;
    assert_eq!(
        listed_ids(&answer, "after the refusals"),
        started,
        "case after the refusals: expected the listing a refused request left alone, got {:?}",
        answer.body
    );
}
