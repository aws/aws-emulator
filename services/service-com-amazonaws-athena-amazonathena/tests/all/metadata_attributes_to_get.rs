// SPDX-License-Identifier: CC0-1.0

//! `AttributesToGet` on the two metadata listings: which properties a page reports.
//!
//! The rule, from the member's own documentation — `ListTableMetadataInput$AttributesToGet`
//! is "A list of table properties to return. The list must contain `NAME` and can contain at
//! most two values", and `ListDatabasesInput$AttributesToGet` is "A list of database
//! properties to return. The list can contain at most one value" — is that a listing which
//! names the member reports THOSE properties and no others, the name always among them
//! (`TableMetadata$Name` and `Database$Name` are `@required`), while a listing that names no
//! attributes reports the whole entry. Nothing a listing reports is invented by the
//! narrowing: it only drops what the catalog holds.
//!
//! Asserted over a TABLE whose rows differ along what the rule generalises over — which
//! attributes are asked for, in which order, the empty list, and the member left out
//! entirely — and over entries the catalog holds different property sets for, so a page
//! narrowed to one attribute is seen to drop properties that are really there. Every table
//! is created over GLUE's own wire and every Athena call goes through the same router the
//! emulator boots.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::table_metadata::{athena_ok, emulator, glue_ok, GLUE_CATALOG};

/// The database these suites read.
const DATABASE: &str = "attributes_db";

/// The table the catalog holds a full property set, a type and columns for.
const RICH_TABLE: &str = "events";

/// The table the catalog holds nothing beyond a name for, so narrowing it has nothing to
/// drop but the empty property set every table has.
const BARE_TABLE: &str = "bare";

/// The type the catalog holds for [`RICH_TABLE`].
const RICH_TABLE_TYPE: &str = "EXTERNAL_TABLE";

/// Create the database and the two tables over Glue's API.
async fn seed(router: &MultiServiceRouter) {
    glue_ok(
        router,
        "CreateDatabase",
        &json!({
            "DatabaseInput": {
                "Name": DATABASE,
                "Description": "the attributes suite",
                "Parameters": { "owner": "data-eng" },
            },
        }),
    )
    .await;
    glue_ok(
        router,
        "CreateTable",
        &json!({
            "DatabaseName": DATABASE,
            "TableInput": {
                "Name": RICH_TABLE,
                "TableType": RICH_TABLE_TYPE,
                "StorageDescriptor": {
                    "Columns": [ { "Name": "id", "Type": "string" } ],
                    "Location": "s3://bucket/events/",
                },
                "PartitionKeys": [ { "Name": "dt", "Type": "string" } ],
                "Parameters": { "classification": "json" },
            },
        }),
    )
    .await;
    glue_ok(
        router,
        "CreateTable",
        &json!({ "DatabaseName": DATABASE, "TableInput": { "Name": BARE_TABLE } }),
    )
    .await;
}

/// The member names of one reported entry, sorted, so a row states the member SET it
/// expects rather than one member at a time.
fn members(case: &str, entry: &Value) -> Vec<String> {
    let mut names: Vec<String> = entry
        .as_object()
        .unwrap_or_else(|| panic!("case {case:?}: expected an entry object, got {entry}"))
        .keys()
        .cloned()
        .collect();
    names.sort();
    names
}

/// The entries of one `ListTableMetadata` page, keyed by the table's name.
async fn table_page(router: &MultiServiceRouter, attributes: Option<Value>) -> Vec<Value> {
    let mut body = json!({
        "CatalogName": GLUE_CATALOG,
        "DatabaseName": DATABASE,
    });
    if let Some(attributes) = attributes {
        body["AttributesToGet"] = attributes;
    }
    let page = athena_ok(router, "ListTableMetadata", &body).await;
    page["TableMetadataList"]
        .as_array()
        .unwrap_or_else(|| panic!("expected a TableMetadataList, got {page}"))
        .clone()
}

/// The entries of one `ListDatabases` page.
async fn database_page(router: &MultiServiceRouter, attributes: Option<Value>) -> Vec<Value> {
    let mut body = json!({ "CatalogName": GLUE_CATALOG });
    if let Some(attributes) = attributes {
        body["AttributesToGet"] = attributes;
    }
    let page = athena_ok(router, "ListDatabases", &body).await;
    page["DatabaseList"]
        .as_array()
        .unwrap_or_else(|| panic!("expected a DatabaseList, got {page}"))
        .clone()
}

/// One row of the table-listing table: the attributes the request names, and the member set
/// each of the two tables must then report.
struct TableCase {
    /// What this row is about.
    case: &'static str,
    /// `AttributesToGet` as the request carries it, or `None` for a request that names none.
    attributes: Option<Value>,
    /// The members [`RICH_TABLE`] must report, sorted.
    rich_members: Vec<&'static str>,
    /// The members [`BARE_TABLE`] must report, sorted.
    bare_members: Vec<&'static str>,
}

/// A table listing reports exactly the properties the request asked for: the name alone, the
/// name and the type, in whichever order they were asked for — and the whole metadata for a
/// listing that asked for no attributes at all.
#[tokio::test]
async fn a_table_listing_reports_the_attributes_the_request_asked_for() {
    let router = emulator();
    seed(&router).await;

    // `TableAttributes` has two values, NAME and TABLE_TYPE, so these are every list a
    // request can send, plus the member left out.
    let cases = vec![
        TableCase {
            case: "the name alone",
            attributes: Some(json!(["NAME"])),
            rich_members: vec!["Name"],
            bare_members: vec!["Name"],
        },
        TableCase {
            case: "the name and the table type",
            attributes: Some(json!(["NAME", "TABLE_TYPE"])),
            rich_members: vec!["Name", "TableType"],
            // The catalog holds no type for the bare table, so there is none to report:
            // narrowing drops properties, it never invents one.
            bare_members: vec!["Name"],
        },
        TableCase {
            case: "the same two attributes in the other order",
            attributes: Some(json!(["TABLE_TYPE", "NAME"])),
            rich_members: vec!["Name", "TableType"],
            bare_members: vec!["Name"],
        },
        TableCase {
            case: "no attributes named at all",
            attributes: None,
            rich_members: vec![
                "Columns",
                "CreateTime",
                "Name",
                "Parameters",
                "PartitionKeys",
                "TableType",
            ],
            bare_members: vec!["CreateTime", "Name", "Parameters"],
        },
    ];

    for case in cases {
        let entries = table_page(&router, case.attributes).await;
        for (expected, table) in [
            (case.rich_members, RICH_TABLE),
            (case.bare_members, BARE_TABLE),
        ] {
            let entry = entries
                .iter()
                .find(|entry| entry["Name"] == json!(table))
                .unwrap_or_else(|| {
                    panic!(
                        "case {:?}: expected the table {table} on the page, got {entries:?}",
                        case.case
                    )
                });
            assert_eq!(
                members(case.case, entry),
                expected,
                "case {:?}: expected the table {table} to report {expected:?}, got {entry}",
                case.case
            );
        }
    }

    // The values a narrowed page reports are the catalog's own, not placeholders.
    let narrowed = table_page(&router, Some(json!(["NAME", "TABLE_TYPE"]))).await;
    let rich = narrowed
        .iter()
        .find(|entry| entry["Name"] == json!(RICH_TABLE))
        .unwrap_or_else(|| panic!("expected the table {RICH_TABLE} on the page, got {narrowed:?}"));
    assert_eq!(
        rich["TableType"],
        json!(RICH_TABLE_TYPE),
        "expected the type the catalog holds, got {rich}"
    );
}

/// A database listing reports the name alone wherever the request named the attributes, the
/// one value `DatabaseAttributes` has being the name — and the whole database where it named
/// none.
#[tokio::test]
async fn a_database_listing_reports_the_attributes_the_request_asked_for() {
    let router = emulator();
    seed(&router).await;

    // (case, AttributesToGet, the members the database must report). `DatabaseAttributes`
    // has one value, and the list may also be sent empty (its length is 0 to 1), so these
    // are every list a request can send, plus the member left out.
    let cases: Vec<(&str, Option<Value>, Vec<&str>)> = vec![
        ("the name", Some(json!(["NAME"])), vec!["Name"]),
        ("an empty list", Some(json!([])), vec!["Name"]),
        (
            "no attributes named at all",
            None,
            vec!["Description", "Name", "Parameters"],
        ),
    ];

    for (case, attributes, expected) in cases {
        let entries = database_page(&router, attributes).await;
        let entry = entries
            .iter()
            .find(|entry| entry["Name"] == json!(DATABASE))
            .unwrap_or_else(|| {
                panic!("case {case:?}: expected the database {DATABASE} on the page, got {entries:?}")
            });
        assert_eq!(
            members(case, entry),
            expected,
            "case {case:?}: expected the database to report {expected:?}, got {entry}"
        );
    }
}
