// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of `ListTableMigrations`.
//!
//! The rule under test: the listing reports one summary per migration of the
//! account — the same facts `GetTableMigration` reports about each — newest
//! first, narrowed to the migrations whose destination the request's members
//! name, at most `maxResults` of them (a hundred when the request asks for none),
//! with a `nextToken` back only when a migration the caller has not seen
//! remains. A member that narrows without its own coordinates narrows nothing and
//! is refused; a table bucket or namespace the store does not hold is the modelled
//! not-found; and the listing writes nothing.
//!
//! The table spans what the rule generalises over: which members narrow the
//! listing, the filters (`state`, `hasValidPlan`), the page size — including one
//! past the number held and each bound the model publishes — and a resumed page.

use serde_json::{json, Value};

use super::harness::{assert_served, served, start_migration_request, Emulator};

/// The fixed sentences this service states the classes below with.
const NO_BUCKET: &str = "The specified bucket does not exist.";
const NO_NAMESPACE: &str = "The specified namespace does not exist.";
const LISTING_ADDRESSING: &str = "Invalid input: Must provide either tableARN alone OR a table \
                                  bucket arn, optionally narrowed by namespace and then table \
                                  name";

/// The bounds the model publishes for the page size: `ListTableMigrationsLimit`
/// is `@range(min: 1, max: 1000)` (`smithy_query(query="shape",
/// shape="ListTableMigrationsLimit")`).
const MAX_RESULTS_MIN: i64 = 1;
const MAX_RESULTS_MAX: i64 = 1000;

/// Create a table bucket holding `namespaces`, and answer its ARN.
async fn create_bucket(
    emulator: &Emulator,
    region: &str,
    bucket: &str,
    namespaces: &[&str],
) -> String {
    let created = served(
        emulator
            .create_table_bucket(region, &json!({ "name": bucket }))
            .await,
        "CreateTableBucket",
        bucket,
    );
    let bucket_arn = created.text("arn");
    for namespace in namespaces {
        served(
            emulator
                .create_namespace(region, &bucket_arn, &json!({ "namespace": [namespace] }))
                .await,
            "CreateNamespace",
            namespace,
        );
    }
    bucket_arn
}

/// Start a migration into `namespace`/`name` of `bucket_arn` and answer its id.
async fn start(
    emulator: &Emulator,
    region: &str,
    bucket_arn: &str,
    namespace: &str,
    name: &str,
) -> String {
    let started = emulator
        .start_table_migration(region, &start_migration_request(bucket_arn, namespace, name))
        .await;
    assert_eq!(
        started.status, 202,
        "starting the migration into {namespace}/{name} must be accepted, got {status} {body}",
        status = started.status,
        body = started.body
    );
    started.text("migrationId")
}

/// One row of the listing table: the case, the query members the listing is
/// addressed with, and the migration ids it must report in order.
type ListCase = (&'static str, Vec<(&'static str, String)>, Vec<String>);

/// The migration ids `reply` lists, in the order it listed them.
fn listed_ids(reply: &super::harness::Reply, case: &str) -> Vec<String> {
    reply
        .list("migrations")
        .iter()
        .map(|row| {
            row.get("migrationId")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {case:?}: every row names its migration: {row}"))
                .to_string()
        })
        .collect()
}

/// RULE: the listing reports the account's migrations newest first, narrowed by
/// whichever coordinates the request names.
#[tokio::test]
async fn the_listing_reports_migrations_newest_first_narrowed_by_the_members_named() {
    let region = "us-east-1";
    let emulator = Emulator::new();
    let one = create_bucket(&emulator, region, "migration-list-one", &["sales", "stock"]).await;
    let two = create_bucket(&emulator, region, "migration-list-two", &["sales"]).await;

    // Five migrations, started in this order, so the newest-first order is the
    // reverse of it.
    let first = start(&emulator, region, &one, "sales", "orders").await;
    let second = start(&emulator, region, &one, "sales", "invoices").await;
    let third = start(&emulator, region, &one, "stock", "levels").await;
    let fourth = start(&emulator, region, &two, "sales", "orders").await;
    let fifth = start(&emulator, region, &two, "sales", "returns").await;
    let newest_first = vec![
        fifth.clone(),
        fourth.clone(),
        third.clone(),
        second.clone(),
        first.clone(),
    ];
    let orders_table = served(
        emulator.get_table(region, &one, "sales", "orders").await,
        "GetTable",
        "the first destination",
    )
    .text("tableARN");

    // The rows differ along WHICH members narrow the listing.
    let cases: Vec<ListCase> = vec![
        ("every migration", vec![], newest_first.clone()),
        (
            "one table bucket",
            vec![("tableBucketARN", one.clone())],
            vec![third.clone(), second.clone(), first.clone()],
        ),
        (
            "one namespace of one table bucket",
            vec![
                ("tableBucketARN", one.clone()),
                ("namespace", "sales".to_string()),
            ],
            vec![second.clone(), first.clone()],
        ),
        (
            "one table of one namespace",
            vec![
                ("tableBucketARN", one.clone()),
                ("namespace", "sales".to_string()),
                ("name", "orders".to_string()),
            ],
            vec![first.clone()],
        ),
        (
            "one destination table by ARN",
            vec![("tableARN", orders_table.clone())],
            vec![first.clone()],
        ),
        (
            "a namespace no migration went into",
            vec![
                ("tableBucketARN", two.clone()),
                ("namespace", "sales".to_string()),
                ("name", "nothing_here".to_string()),
            ],
            vec![],
        ),
    ];

    for (case, query, expected) in cases {
        let listed = served(
            emulator.list_table_migrations(region, &query).await,
            "ListTableMigrations",
            case,
        );
        assert_eq!(
            listed_ids(&listed, case),
            expected,
            "case {case:?}: the listing reports the migrations the request narrows to, newest \
             first, got {body}",
            body = listed.body
        );
        assert_eq!(
            listed.optional_text("nextToken"),
            None,
            "case {case:?}: a whole listing carries no nextToken, got {body}",
            body = listed.body
        );
    }

    // Every row carries the facts a read of that migration reports.
    let listed = served(
        emulator.list_table_migrations(region, &[]).await,
        "ListTableMigrations",
        "rows",
    );
    for row in listed.list("migrations") {
        let id = row
            .get("migrationId")
            .and_then(Value::as_str)
            .expect("every row names its migration")
            .to_string();
        let read = served(
            emulator
                .get_table_migration(region, &[("migrationId", id.clone())])
                .await,
            "GetTableMigration",
            "rows",
        );
        for member in [
            "migrationId",
            "tableBucketARN",
            "namespace",
            "name",
            "tableARN",
            "type",
            "createdAt",
        ] {
            assert_eq!(
                row.get(member),
                read.body.get(member),
                "case \"rows\": the row's {member} is what a read of {id} reports, got {row}"
            );
        }
    }
}

/// RULE: the listing is filtered by the state and by whether planning produced a
/// runnable plan, and a migration that is still planning is listed under no
/// `hasValidPlan` filter at all.
#[tokio::test]
async fn the_listing_is_filtered_by_state_and_by_whether_a_plan_is_valid() {
    let region = "eu-west-1";
    let emulator = Emulator::new();
    let bucket_arn = create_bucket(&emulator, region, "migration-list-filters", &["sales"]).await;
    let migration = start(&emulator, region, &bucket_arn, "sales", "orders").await;

    // Freshly started, the migration is in the state the request was accepted in
    // and planning has not finished, so it carries no hasValidPlan value.
    let cases: Vec<ListCase> = vec![
        (
            "the state it was accepted in",
            vec![("state", "new".to_string())],
            vec![migration.clone()],
        ),
        (
            "a state it is not in",
            vec![("state", "succeeded".to_string())],
            vec![],
        ),
        (
            "a valid plan is asked for",
            vec![("hasValidPlan", "true".to_string())],
            vec![],
        ),
        (
            "no valid plan is asked for",
            vec![("hasValidPlan", "false".to_string())],
            vec![],
        ),
    ];
    for (case, query, expected) in cases {
        let listed = served(
            emulator.list_table_migrations(region, &query).await,
            "ListTableMigrations",
            case,
        );
        assert_eq!(
            listed_ids(&listed, case),
            expected,
            "case {case:?}: a migration that is still planning is listed under no hasValidPlan \
             filter, and only under the state it is in, got {body}",
            body = listed.body
        );
    }
}

/// RULE: the listing hands back at most `maxResults` rows, with a `nextToken`
/// only when a migration the caller has not seen remains, and a resumed page
/// continues where the last one stopped.
#[tokio::test]
async fn the_listing_pages_over_the_migrations_it_covers() {
    let region = "ap-south-1";
    let emulator = Emulator::new();
    let bucket_arn = create_bucket(&emulator, region, "migration-list-pages", &["sales"]).await;
    let mut started = Vec::new();
    for index in 0..4 {
        started.push(start(&emulator, region, &bucket_arn, "sales", &format!("t{index}")).await);
    }
    let newest_first: Vec<String> = started.iter().rev().cloned().collect();

    // The rows differ along the page size, including each bound the model
    // publishes and a size past the number of migrations held.
    let cases: Vec<(&str, Option<i64>, usize)> = vec![
        ("no page size at all", None, 4),
        ("the smallest page the model admits", Some(MAX_RESULTS_MIN), 1),
        ("a page smaller than the number held", Some(2), 2),
        ("a page past the number held", Some(9), 4),
        ("the largest page the model admits", Some(MAX_RESULTS_MAX), 4),
    ];
    for (case, max_results, expected_first_page) in cases {
        let query: Vec<(&str, String)> = max_results
            .map(|size| vec![("maxResults", size.to_string())])
            .unwrap_or_default();
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut page = query.clone();
            if let Some(resume) = token.as_ref() {
                page.push(("nextToken", resume.clone()));
            }
            let listed = served(
                emulator.list_table_migrations(region, &page).await,
                "ListTableMigrations",
                case,
            );
            let ids = listed_ids(&listed, case);
            if token.is_none() {
                assert_eq!(
                    ids.len(),
                    expected_first_page,
                    "case {case:?}: the first page carries at most the page size asked for, got \
                     {body}",
                    body = listed.body
                );
            }
            seen.extend(ids);
            token = listed.optional_text("nextToken");
            if token.is_none() {
                break;
            }
            assert!(
                seen.len() <= newest_first.len(),
                "case {case:?}: paging must end, got {seen:?}"
            );
        }
        assert_eq!(
            seen, newest_first,
            "case {case:?}: walking every page reports each migration once, newest first"
        );
    }

    // A token this service never minted names no place in the listing.
    let refused = emulator
        .list_table_migrations(region, &[("nextToken", "not-a-token".to_string())])
        .await;
    assert_eq!(
        refused.status, 400,
        "case \"a token this service never minted\": the listing is refused, got {status} {body}",
        status = refused.status,
        body = refused.body
    );
}

/// RULE: a member that narrows the listing without its own coordinates narrows
/// nothing and is refused; a table bucket or namespace the store does not hold is
/// the modelled not-found.
#[tokio::test]
async fn a_listing_that_narrows_by_half_a_form_is_refused() {
    let region = "us-west-1";
    let emulator = Emulator::new();
    let bucket_arn = create_bucket(&emulator, region, "migration-list-refusals", &["sales"]).await;
    let table_arn = format!("{bucket_arn}/table/00000000-0000-4000-8000-000000000000");

    // The rows differ along WHICH half of a form the request leaves out.
    let cases: Vec<(&str, Vec<(&str, String)>)> = vec![
        (
            "a namespace with no table bucket",
            vec![("namespace", "sales".to_string())],
        ),
        (
            "a name with no namespace",
            vec![
                ("tableBucketARN", bucket_arn.clone()),
                ("name", "orders".to_string()),
            ],
        ),
        (
            "both whole forms at once",
            vec![
                ("tableBucketARN", bucket_arn.clone()),
                ("tableARN", table_arn.clone()),
            ],
        ),
    ];
    for (case, query) in cases {
        let refused = emulator.list_table_migrations(region, &query).await;
        assert_eq!(
            (refused.status, refused.message()),
            (400, LISTING_ADDRESSING.to_string()),
            "case {case:?}: a listing that narrows by half a form is refused, got {status} {body}",
            status = refused.status,
            body = refused.body
        );
    }

    let absent_bucket = format!("arn:aws:s3tables:{region}:000000000000:bucket/migration-absent");
    let unknown = emulator
        .list_table_migrations(region, &[("tableBucketARN", absent_bucket)])
        .await;
    assert_eq!(
        (unknown.status, unknown.message()),
        (404, NO_BUCKET.to_string()),
        "case \"a table bucket the store does not hold\": the listing is the modelled \
         not-found, got {status} {body}",
        status = unknown.status,
        body = unknown.body
    );
    let unknown_namespace = emulator
        .list_table_migrations(
            region,
            &[
                ("tableBucketARN", bucket_arn.clone()),
                ("namespace", "absent".to_string()),
            ],
        )
        .await;
    assert_eq!(
        (unknown_namespace.status, unknown_namespace.message()),
        (404, NO_NAMESPACE.to_string()),
        "case \"a namespace the bucket does not hold\": the listing is the modelled not-found, \
         got {status} {body}",
        status = unknown_namespace.status,
        body = unknown_namespace.body
    );

    // A refused listing reports nothing and changes nothing: the whole listing of
    // a bucket no migration went into is still empty.
    let listed = emulator
        .list_table_migrations(region, &[("tableBucketARN", bucket_arn)])
        .await;
    assert_served(&listed, "ListTableMigrations", "after refusals");
    assert!(
        listed.list("migrations").is_empty(),
        "case \"after refusals\": a bucket no migration went into lists none, got {body}",
        body = listed.body
    );
}
