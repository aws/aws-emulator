// SPDX-License-Identifier: CC0-1.0

//! What a metadata read answers when the DATA CATALOG it called refuses the read.
//!
//! The rule under test: Athena registers no databases and no tables of its own, so a
//! metadata read of `AwsDataCatalog` — or of a registered `GLUE` catalog naming the same
//! catalog id — resolves the address by CALLING the Data Catalog, and a reference that
//! catalog resolves to nothing comes back as that catalog's own exception. Athena reports
//! it as `MetadataException` ("an exception that Athena received when it called a custom
//! metastore") whose message is the catalog's own sentence verbatim followed by the clause
//! the relaying SDK layer states around a caught service exception: the component that
//! answered, the status it raised the refusal with, the error code it answered under, that
//! call's own request id and the proxy it came through. So a client parsing the clause
//! recovers the underlying `EntityNotFoundException` whatever was missing, and an absent
//! entity is distinguishable from any other metastore failure.
//!
//! Each suite is a TABLE whose rows differ along what the rule generalises over: which
//! entity of the address missed (the table, the database), which operation relayed the call
//! (`GetTableMetadata`, `ListTableMetadata`), how the catalog was addressed (its own name,
//! that name lowercased, a registered `GLUE` catalog naming the same catalog id) and how the
//! absent name was spelled (lower case, mixed case, one character). The recorded case — a
//! `GetTableMetadata` for an absent table of an existing database, read through
//! `AwsDataCatalog` — is one row among them.
//!
//! The wire path is the one the metadata suite beside this one already mounts
//! ([`crate::table_metadata`]): Athena and Glue on ONE shared engine, reached through the
//! same `MultiServiceRouter` the production front door uses, so every table here is created
//! over GLUE's real wire and every Athena call goes over Athena's own.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{error_code, message};
use crate::relayed_catalog_refusal_agreement::database_miss;
use crate::table_metadata::{athena, athena_ok, emulator, glue_ok, GLUE_CATALOG};

/// The database the rows that address an EXISTING database use, and the one table it holds.
const SEEDED_DATABASE: &str = "relay_db";
const SEEDED_TABLE: &str = "events";

/// The component the relayed clause names: the Data Catalog as the SDK layer that relayed
/// the call addresses it.
const CATALOG_SERVICE: &str = "AmazonDataCatalog";

/// The status that component raised the miss with, and the code it answered under.
const CATALOG_STATUS: u16 = 400;
const CATALOG_CODE: &str = "EntityNotFoundException";

/// The catalog's own sentence for a TABLE READ that resolved to nothing: the Data Catalog
/// answers one wording for every such miss, and it names no entity of the request (Glue's
/// own `GetTable` refusal, `state::messages::TABLE_ENTITY_NOT_FOUND`).
///
/// `pub(crate)` because `tests/all` is ONE crate: the suite over a drop and a recreate of
/// the same table asserts this same sentence rather than keeping a second copy of it.
pub(crate) const TABLE_MISS: &str = "Entity Not Found";

/// Create the seeded database and its one table over Glue's own API.
async fn seed(router: &MultiServiceRouter) {
    glue_ok(
        router,
        "CreateDatabase",
        &json!({ "DatabaseInput": { "Name": SEEDED_DATABASE } }),
    )
    .await;
    glue_ok(
        router,
        "CreateTable",
        &json!({
            "DatabaseName": SEEDED_DATABASE,
            "TableInput": {
                "Name": SEEDED_TABLE,
                "StorageDescriptor": {
                    "Columns": [{ "Name": "id", "Type": "string" }],
                    "InputFormat": "org.apache.hadoop.mapred.TextInputFormat",
                    "OutputFormat":
                        "org.apache.hadoop.hive.ql.io.HiveIgnoreKeyTextOutputFormat",
                    "SerdeInfo": {
                        "SerializationLibrary":
                            "org.apache.hadoop.hive.serde2.lazy.LazySimpleSerDe"
                    }
                }
            }
        }),
    )
    .await;
}

/// Register one `GLUE` data catalog naming THIS account's catalog id, which addresses the
/// same Data Catalog under a second name, and answer with that name.
///
/// `pub(crate)` for the reason [`TABLE_MISS`] is: the sibling suite over a drop and a
/// recreate reads through a second name too, and one registration path keeps the two from
/// drifting.
pub(crate) async fn register_second_name(router: &MultiServiceRouter, name: &str) -> String {
    athena_ok(
        router,
        "CreateDataCatalog",
        &json!({
            "Name": name,
            "Type": "GLUE",
            "Parameters": { "catalog-id": engine::emulator_account_id() }
        }),
    )
    .await;
    name.to_string()
}

/// Assert that `refusal` is `expected_reason` relayed inside the caught-exception clause,
/// and answer with the request id that clause states.
///
/// Every token of the clause is fixed text but the request id, which belongs to the relayed
/// CALL — so it is checked for the FORM the emulator's request ids carry rather than
/// compared to a value.
///
/// `pub(crate)` for the reason [`TABLE_MISS`] is: the sibling suite over a drop and a
/// recreate checks the same clause, and one rendering of it keeps the two from drifting.
pub(crate) fn relayed_request_id(case: &str, refusal: &str, expected_reason: &str) -> String {
    let head = format!(
        "{expected_reason} (Service: {CATALOG_SERVICE}; Status Code: {CATALOG_STATUS}; Error \
         Code: {CATALOG_CODE}; Request ID: "
    );
    let tail = "; Proxy: null)";
    assert!(
        refusal.starts_with(&head),
        "case {case:?}: expected the message to start {head:?}, got {refusal:?}"
    );
    assert!(
        refusal.ends_with(tail),
        "case {case:?}: expected the message to end {tail:?}, got {refusal:?}"
    );
    let request_id = refusal
        .strip_prefix(&head)
        .and_then(|rest| rest.strip_suffix(tail))
        .unwrap_or_default()
        .to_string();
    assert!(
        ::service_common::query_error_envelope::is_request_id(&request_id),
        "case {case:?}: expected the clause to state a request id, got {request_id:?} in \
         {refusal:?}"
    );
    request_id
}

/// The refusal a metadata read answers for an address the Data Catalog resolves to nothing,
/// whatever missed and however the catalog was addressed.
#[tokio::test]
async fn a_catalog_miss_is_relayed_inside_the_caught_exception_clause() {
    let router = emulator();
    seed(&router).await;
    let second_name = register_second_name(&router, "relay_second_name").await;

    // (the case, the operation, the request, the sentence the CATALOG answered)
    let rows: Vec<(&str, &str, Value, String)> = vec![
        (
            "an absent table of an existing database",
            "GetTableMetadata",
            json!({
                "CatalogName": GLUE_CATALOG,
                "DatabaseName": SEEDED_DATABASE,
                "TableName": "sim2real_absent_tbl"
            }),
            TABLE_MISS.to_string(),
        ),
        (
            "an absent table named in mixed case",
            "GetTableMetadata",
            json!({
                "CatalogName": GLUE_CATALOG,
                "DatabaseName": SEEDED_DATABASE,
                "TableName": "AbsentMixedCase"
            }),
            TABLE_MISS.to_string(),
        ),
        (
            "an absent table named with one character",
            "GetTableMetadata",
            json!({
                "CatalogName": GLUE_CATALOG,
                "DatabaseName": SEEDED_DATABASE,
                "TableName": "x"
            }),
            TABLE_MISS.to_string(),
        ),
        (
            "an absent table, through the lowercased catalog name",
            "GetTableMetadata",
            json!({
                "CatalogName": "awsdatacatalog",
                "DatabaseName": SEEDED_DATABASE,
                "TableName": "absent_through_lowercase"
            }),
            TABLE_MISS.to_string(),
        ),
        (
            "an absent table, through a registered GLUE catalog of the same catalog id",
            "GetTableMetadata",
            json!({
                "CatalogName": second_name,
                "DatabaseName": SEEDED_DATABASE,
                "TableName": "absent_through_second_name"
            }),
            TABLE_MISS.to_string(),
        ),
        (
            "an absent database, single-table read",
            "GetTableMetadata",
            json!({
                "CatalogName": GLUE_CATALOG,
                "DatabaseName": "absent_relay_db",
                "TableName": SEEDED_TABLE
            }),
            database_miss("absent_relay_db"),
        ),
        (
            "an absent database named in mixed case, single-table read",
            "GetTableMetadata",
            json!({
                "CatalogName": GLUE_CATALOG,
                "DatabaseName": "AbsentRelayDb",
                "TableName": SEEDED_TABLE
            }),
            database_miss("AbsentRelayDb"),
        ),
        (
            "an absent database, listing",
            "ListTableMetadata",
            json!({ "CatalogName": GLUE_CATALOG, "DatabaseName": "absent_relay_db" }),
            database_miss("absent_relay_db"),
        ),
        (
            "an absent database, listing through a registered GLUE catalog",
            "ListTableMetadata",
            json!({ "CatalogName": second_name, "DatabaseName": "AbsentRelayDb" }),
            database_miss("AbsentRelayDb"),
        ),
    ];

    for (case, operation, request, expected_reason) in rows {
        let (status, body) = athena(&router, operation, &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "MetadataException",
            "case {case:?}: expected MetadataException, got {body}"
        );
        relayed_request_id(case, &message(&body), &expected_reason);
    }
}

/// The request id belongs to the relayed CALL, so two calls that miss the same way are
/// never reported under one id.
#[tokio::test]
async fn each_relayed_refusal_states_the_request_id_of_its_own_call() {
    let router = emulator();
    seed(&router).await;

    let request = json!({
        "CatalogName": GLUE_CATALOG,
        "DatabaseName": SEEDED_DATABASE,
        "TableName": "absent_twice"
    });
    let mut seen: Vec<String> = Vec::new();
    for attempt in 0..3 {
        let case = format!("call {attempt}");
        let (status, body) = athena(&router, "GetTableMetadata", &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {body}"
        );
        seen.push(relayed_request_id(&case, &message(&body), TABLE_MISS));
    }
    let mut unique = seen.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(
        unique.len(),
        seen.len(),
        "case \"three identical reads\": expected each call its own request id, got {seen:?}"
    );
}

/// A table the catalog DOES hold is still served, and a table dropped from the catalog is
/// then relayed as the miss: the clause is the answer to a real catalog refusal and not
/// something a read states for every address.
#[tokio::test]
async fn an_address_the_catalog_resolves_is_served_and_a_dropped_one_is_relayed() {
    let router = emulator();
    seed(&router).await;

    let request = json!({
        "CatalogName": GLUE_CATALOG,
        "DatabaseName": SEEDED_DATABASE,
        "TableName": SEEDED_TABLE
    });
    let served = athena_ok(&router, "GetTableMetadata", &request).await;
    assert_eq!(
        served["TableMetadata"]["Name"],
        json!(SEEDED_TABLE),
        "case \"a table the catalog holds\": expected the stored table, got {served}"
    );

    glue_ok(
        &router,
        "DeleteTable",
        &json!({ "DatabaseName": SEEDED_DATABASE, "Name": SEEDED_TABLE }),
    )
    .await;

    let (status, body) = athena(&router, "GetTableMetadata", &request).await;
    assert_eq!(
        status, 400,
        "case \"a table the catalog dropped\": expected HTTP 400, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        "MetadataException",
        "case \"a table the catalog dropped\": expected MetadataException, got {body}"
    );
    relayed_request_id("a table the catalog dropped", &message(&body), TABLE_MISS);
}
