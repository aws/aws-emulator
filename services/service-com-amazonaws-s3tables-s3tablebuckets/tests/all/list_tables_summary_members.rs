// SPDX-License-Identifier: CC0-1.0

//! The MEMBER SET of a `ListTables` summary row.
//!
//! RULE — a `ListTables` row reports the LISTED TABLE's own record and nothing else: the
//! namespace path it sits in, its name, its type, its ARN and its two instants, plus
//! `managedByService` for a table a service manages. The optional members that name the
//! table's CONTAINERS — `namespaceId` and `tableBucketId` — are reported by the reads of
//! the resources they identify (`GetTable`, `GetNamespace`, `GetTableBucket`) and by the
//! summaries of those resources, never by a table summary, so a row carries neither of
//! them. The member list comes from the model
//! (`smithy_query(query="shape", shape="TableSummary")`: six `@required` members —
//! `namespace`, `name`, `type`, `tableARN`, `createdAt`, `modifiedAt` — and the three
//! optional `managedByService`, `namespaceId`, `tableBucketId`); which of the optional
//! ones a listing reports is recorded behaviour: a recorded `ListTables` element carries
//! exactly the six required members, and an optional member the service does not report
//! is ABSENT from the JSON rather than emitted empty.
//!
//! The rule is asserted over a TABLE of fixtures — one, three and five tables, one and
//! two namespaces per bucket, short and long name forms, three regions, with and without
//! table metadata — and over every way a page is asked for (the whole bucket, narrowed by
//! namespace, narrowed by prefix, and walked one or two rows at a time through
//! `continuationToken`), so the row the reported divergence came from is one case among
//! many. Two further suites cover the member set a row carries for a table a SERVICE
//! manages and the row a table carries after it has been renamed.

use std::collections::BTreeSet;
use std::time::SystemTime;

use serde_json::{json, Value};
use service_com_amazonaws_s3tables_s3tablebuckets::state::{service_state, OpenTableFormat};

use crate::harness::{timestamp_seconds, Emulator};

/// The members `TableSummary` declares as `@required`, which every row carries.
const REQUIRED_MEMBERS: &[&str] = &[
    "namespace",
    "name",
    "type",
    "tableARN",
    "createdAt",
    "modifiedAt",
];

/// One namespace of a fixture and the tables created in it, in request order.
#[derive(Debug, Clone, Copy)]
struct Group {
    namespace: &'static str,
    tables: &'static [&'static str],
}

/// One bucket's worth of tables, as a row of the table below.
#[derive(Debug, Clone, Copy)]
struct Fixture {
    label: &'static str,
    region: &'static str,
    bucket: &'static str,
    groups: &'static [Group],
    /// `maxTables` for the paged walk, so a page boundary falls inside the listing.
    page_size: i64,
    /// Whether each create describes the table's metadata, which gives the table a
    /// metadata file — a second record behind the same summary row.
    metadata: bool,
}

/// The fixtures span how many tables a bucket holds (one, three, five), how they are
/// spread over namespaces (one namespace, two), name length and form (the modelled
/// one-character minimum, digits, interior underscores, a long name), the region the ARNs
/// are scoped to, whether the create carries metadata, and where the page boundary of the
/// paged walk falls.
const FIXTURES: &[Fixture] = &[
    Fixture {
        label: "one table, shortest names",
        region: "us-east-1",
        bucket: "summary-members-single",
        groups: &[Group {
            namespace: "n",
            tables: &["t"],
        }],
        page_size: 1,
        metadata: false,
    },
    Fixture {
        label: "three tables in one namespace, paged two at a time",
        region: "us-west-2",
        bucket: "summary-members-paged",
        groups: &[Group {
            namespace: "analytics_ns",
            tables: &["tbl_one", "tbl_two", "tbl_three"],
        }],
        page_size: 2,
        metadata: true,
    },
    Fixture {
        label: "five tables over two namespaces, paged one at a time",
        region: "eu-west-1",
        bucket: "summary-members-spread",
        groups: &[
            Group {
                namespace: "ns_a",
                tables: &["a0", "a_1"],
            },
            Group {
                namespace: "ns_2026_q1",
                tables: &["b0", "sales_2026", "a_very_long_table_name_0123456789"],
            },
        ],
        page_size: 1,
        metadata: false,
    },
];

/// `CreateTable`'s request body.
fn create_table_request(table: &str, metadata: bool) -> Value {
    let mut request = json!({
        "name": table,
        // `format` is required and `ICEBERG` is the only value the modelled
        // `OpenTableFormat` enum declares.
        "format": OpenTableFormat::Iceberg.as_wire(),
    });
    if metadata {
        request["metadata"] = json!({
            "iceberg": {
                "schema": {
                    "fields": [
                        { "name": "id", "type": "long", "required": true },
                        { "name": "label", "type": "string" },
                    ]
                }
            }
        });
    }
    request
}

/// Create one fixture's bucket, namespaces and tables, and answer the bucket's ARN.
async fn create_fixture(emulator: &Emulator, fixture: &Fixture) -> String {
    let case = fixture.label;
    let bucket = emulator
        .create_table_bucket(fixture.region, &json!({ "name": fixture.bucket }))
        .await;
    assert_eq!(
        bucket.status, 200,
        "case {case:?}: CreateTableBucket: {bucket:?}"
    );
    let bucket_arn = bucket.text("arn");
    for group in fixture.groups {
        let namespace = emulator
            .create_namespace(
                fixture.region,
                &bucket_arn,
                &json!({ "namespace": [group.namespace] }),
            )
            .await;
        assert_eq!(
            namespace.status, 200,
            "case {case:?}: CreateNamespace {:?}: {namespace:?}",
            group.namespace
        );
        for table in group.tables {
            let created = emulator
                .create_table(
                    fixture.region,
                    &bucket_arn,
                    group.namespace,
                    &create_table_request(table, fixture.metadata),
                )
                .await;
            assert_eq!(
                created.status, 200,
                "case {case:?}: CreateTable {table:?}: {created:?}"
            );
        }
    }
    bucket_arn
}

/// Assert `row` carries EXACTLY the members the service reports for it, each in the
/// modelled form: the six required ones, plus `managedByService` when and only when a
/// service manages the table, and neither container id.
fn assert_row_members(row: &Value, managed_by_service: Option<&str>, case: &str) {
    let members = row
        .as_object()
        .unwrap_or_else(|| panic!("case {case:?}: a summary row must be an object, got {row}"));
    let actual: BTreeSet<&str> = members.keys().map(String::as_str).collect();
    let mut expected: BTreeSet<&str> = REQUIRED_MEMBERS.iter().copied().collect();
    if managed_by_service.is_some() {
        expected.insert("managedByService");
    }
    assert_eq!(
        actual, expected,
        "case {case:?}: expected the summary row to carry exactly {expected:?}, got \
         {actual:?}: {row}"
    );

    let namespace = row.get("namespace").and_then(Value::as_array);
    assert!(
        namespace.is_some_and(|parts| !parts.is_empty() && parts.iter().all(Value::is_string)),
        "case {case:?}: expected namespace to be a non-empty list of strings, got {row}"
    );
    for member in ["name", "type", "tableARN"] {
        assert!(
            row.get(member).and_then(Value::as_str).is_some(),
            "case {case:?}: expected {member} to be a string, got {row}"
        );
    }
    // Reads the ONE wire form the model declares for an instant, and refuses every
    // other one.
    for member in ["createdAt", "modifiedAt"] {
        timestamp_seconds(row, member, case);
    }
    if let Some(service) = managed_by_service {
        assert_eq!(
            row.get("managedByService").and_then(Value::as_str),
            Some(service),
            "case {case:?}: expected managedByService {service:?}, got {row}"
        );
    }
}

/// The `name` of a summary row.
fn row_name(row: &Value, case: &str) -> String {
    row.get("name")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: a summary row carries no name: {row}"))
        .to_string()
}

/// Walk the whole listing `page_size` rows at a time, asserting the member set of every
/// row of every page, and answer the names in the order they were paged in.
async fn walk_pages(emulator: &Emulator, fixture: &Fixture, bucket_arn: &str) -> Vec<String> {
    let case = fixture.label;
    let total: usize = fixture.groups.iter().map(|group| group.tables.len()).sum();
    let mut names = Vec::new();
    let mut token: Option<String> = None;
    // One page per row is the worst case; the extra pages are headroom for the last,
    // empty page a cursor can end on.
    for page in 0..total + 2 {
        let mut query = vec![("maxTables", fixture.page_size.to_string())];
        if let Some(token) = &token {
            query.push(("continuationToken", token.clone()));
        }
        let reply = emulator
            .list_tables(fixture.region, bucket_arn, &query)
            .await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: ListTables page {page}: {reply:?}"
        );
        let rows = reply.list("tables");
        assert!(
            rows.len() <= fixture.page_size as usize,
            "case {case:?}: page {page} carries {} rows, more than the {} asked for",
            rows.len(),
            fixture.page_size
        );
        for row in &rows {
            assert_row_members(row, None, &format!("{case} / page {page}"));
            names.push(row_name(row, case));
        }
        token = reply.optional_text("continuationToken");
        if token.is_none() {
            return names;
        }
    }
    panic!("case {case:?}: the listing never ran out of pages for {total} tables");
}

/// The RULE, over every fixture and every way a page is asked for.
#[tokio::test]
async fn every_list_tables_row_carries_exactly_the_members_the_service_reports() {
    for fixture in FIXTURES {
        let case = fixture.label;
        let emulator = Emulator::new();
        let bucket_arn = create_fixture(&emulator, fixture).await;

        let first = fixture.groups[0];
        let prefix = &first.tables[0][..1];
        let variants: Vec<(String, Vec<(&str, String)>)> = vec![
            ("the whole bucket".to_string(), Vec::new()),
            (
                format!("namespace {:?}", first.namespace),
                vec![("namespace", first.namespace.to_string())],
            ),
            (
                format!("prefix {prefix:?}"),
                vec![("prefix", prefix.to_string())],
            ),
        ];
        for (what, query) in variants {
            let listed = format!("{case} / {what}");
            let reply = emulator
                .list_tables(fixture.region, &bucket_arn, &query)
                .await;
            assert_eq!(reply.status, 200, "case {listed:?}: ListTables: {reply:?}");
            let rows = reply.list("tables");
            assert!(
                !rows.is_empty(),
                "case {listed:?}: expected at least one row, got {reply:?}"
            );
            for row in &rows {
                assert_row_members(row, None, &listed);
            }
        }

        // The same rule on every page of a walk that ends on the cursor, and the walk
        // reports each table of the bucket exactly once.
        let mut paged = walk_pages(&emulator, fixture, &bucket_arn).await;
        paged.sort();
        let mut expected: Vec<String> = fixture
            .groups
            .iter()
            .flat_map(|group| group.tables.iter().map(|table| table.to_string()))
            .collect();
        expected.sort();
        assert_eq!(
            paged, expected,
            "case {case:?}: the paged walk reported {paged:?}, not the bucket's tables \
             {expected:?}"
        );
    }
}

/// A table a SERVICE manages is the one case whose row carries a seventh member: the
/// service that manages it. It is still the table's own record — no container id joins
/// it.
#[tokio::test]
async fn a_row_for_a_table_a_service_manages_reports_the_managing_service() {
    let case = "a service-managed table beside a caller's";
    let region = "us-west-2";
    let namespace = "managed_ns";
    let managing_service = "kinesis.amazonaws.com";
    let emulator = Emulator::new();

    let fixture = Fixture {
        label: case,
        region,
        bucket: "summary-members-managed",
        groups: &[Group {
            namespace: "managed_ns",
            tables: &["customer_tbl"],
        }],
        page_size: 1,
        metadata: false,
    };
    let bucket_arn = create_fixture(&emulator, &fixture).await;

    // The one place a service-provisioned table is written, so the row below comes from
    // a stored record of the same store `CreateTable` writes to.
    let state = service_state(&*emulator.engine).expect("the S3 Tables store is mounted");
    let managed_arn = state
        .lock()
        .create_service_managed_table(
            &bucket_arn,
            namespace,
            "managed_tbl",
            managing_service,
            SystemTime::now(),
        )
        .expect("the bucket and the namespace were just created");

    let reply = emulator.list_tables(region, &bucket_arn, &[]).await;
    assert_eq!(reply.status, 200, "case {case:?}: ListTables: {reply:?}");
    let rows = reply.list("tables");
    assert_eq!(
        rows.len(),
        2,
        "case {case:?}: expected the caller's table and the service's, got {rows:?}"
    );

    for row in &rows {
        let name = row_name(row, case);
        match name.as_str() {
            "customer_tbl" => {
                assert_row_members(row, None, &format!("{case} / the caller's table"));
                assert_eq!(
                    row.get("type").and_then(Value::as_str),
                    Some("customer"),
                    "case {case:?}: the caller's table must be of type customer: {row}"
                );
            }
            "managed_tbl" => {
                assert_row_members(
                    row,
                    Some(managing_service),
                    &format!("{case} / the service's table"),
                );
                assert_eq!(
                    row.get("type").and_then(Value::as_str),
                    Some("aws"),
                    "case {case:?}: a service-managed table must be of type aws: {row}"
                );
                assert_eq!(
                    row.get("tableARN").and_then(Value::as_str),
                    Some(managed_arn.as_str()),
                    "case {case:?}: the row must carry the ARN the store minted \
                     ({managed_arn}): {row}"
                );
            }
            other => panic!("case {case:?}: the listing carries an unexpected row {other:?}"),
        }
    }
}

/// A rename re-keys a table; the row it is listed in afterwards is still a table summary,
/// so it carries the same member set as before the move.
#[tokio::test]
async fn a_renamed_table_is_listed_with_the_same_member_set() {
    let case = "a renamed table";
    let region = "us-east-1";
    let namespace = "rename_ns";
    let emulator = Emulator::new();

    let fixture = Fixture {
        label: case,
        region,
        bucket: "summary-members-rename",
        groups: &[Group {
            namespace: "rename_ns",
            tables: &["tbl_one", "tbl_two"],
        }],
        page_size: 2,
        metadata: false,
    };
    let bucket_arn = create_fixture(&emulator, &fixture).await;

    let before = emulator
        .get_table(region, &bucket_arn, namespace, "tbl_one")
        .await;
    assert_eq!(before.status, 200, "case {case:?}: GetTable: {before:?}");
    let renamed = emulator
        .rename_table(
            region,
            &bucket_arn,
            namespace,
            "tbl_one",
            &json!({ "newName": "tbl_one_moved", "versionToken": before.text("versionToken") }),
        )
        .await;
    assert_eq!(
        renamed.status, 204,
        "case {case:?}: RenameTable: expected 204, got {renamed:?}"
    );

    let reply = emulator.list_tables(region, &bucket_arn, &[]).await;
    assert_eq!(reply.status, 200, "case {case:?}: ListTables: {reply:?}");
    let rows = reply.list("tables");
    let mut names: Vec<String> = rows.iter().map(|row| row_name(row, case)).collect();
    names.sort();
    assert_eq!(
        names,
        vec!["tbl_one_moved".to_string(), "tbl_two".to_string()],
        "case {case:?}: expected the moved table and its neighbour, got {rows:?}"
    );
    for row in &rows {
        assert_row_members(row, None, &format!("{case} / after the move"));
    }
}
