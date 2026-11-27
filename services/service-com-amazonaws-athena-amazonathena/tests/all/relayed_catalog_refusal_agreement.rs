// SPDX-License-Identifier: CC0-1.0

//! That EVERY Athena metadata read relays a Data Catalog miss the same way.
//!
//! The rule under test: Athena registers no databases and no tables of its own, so each of
//! its metadata reads resolves the address its input names by CALLING the Data Catalog, and
//! a reference that catalog resolves to nothing comes back as that catalog's own
//! `EntityNotFoundException`. Athena reports it as `MetadataException` (HTTP 400) whose
//! message is the catalog's own sentence verbatim followed by the clause the relaying SDK
//! layer states around a caught service exception — the component that answered, the status
//! it raised the refusal with, the error code it answered under, that call's own request id
//! and the proxy it came through. Because one renderer states that clause, the reads AGREE:
//! the same absent address is reported in the same words whichever read was asked, apart
//! from the per-call request id.
//!
//! The suite beside this one ([`crate::relayed_catalog_refusals`]) drives the two reads the
//! recorded cases exercise, `GetTableMetadata` and `ListTableMetadata`. This one adds the
//! THIRD read that resolves a database through the same call, `GetDatabase`, and asserts the
//! agreement between the three — so a read that drifts back onto wording of its own, or onto
//! a clause of its own, fails here.
//!
//! Rows differ along what the rule generalises over: which read relayed the call, how the
//! catalog was addressed (its own name, that name lowercased, a registered `GLUE` catalog
//! naming the same catalog id) and why no database answered (a name nothing created, a near
//! miss of a stored name, a mixed-case name, a database the catalog dropped again).
//!
//! The wire path is the shared one ([`crate::table_metadata`]): Athena and Glue on ONE
//! engine behind the same `MultiServiceRouter` the production front door uses.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{error_code, message};
use crate::relayed_catalog_refusals::{register_second_name, relayed_request_id};
use crate::table_metadata::{athena, athena_ok, emulator, glue_ok, GLUE_CATALOG};

/// A database the catalog DOES hold, so the rows that address an absent database are read
/// against a catalog that is not empty, and the served read has something to answer with.
const SEEDED_DATABASE: &str = "relay_agreement_db";

/// The catalog's own sentence for a database it holds none of, composed by the crate that
/// OWNS that wording (`glue::state::messages::database_not_found`, which renders the name in
/// the catalog's Hive-compatible fold) rather than re-spelled here.
///
/// `pub(crate)` because `tests/all` is ONE crate: the sibling suite over the recorded reads
/// asserts this same sentence through this one function, so the two cannot drift apart.
pub(crate) fn database_miss(database_name: &str) -> String {
    ::service_com_amazonaws_glue_awsglue::state::messages::database_not_found(database_name)
}

/// Create the seeded database over Glue's own API, as a client creates one.
async fn seed(router: &MultiServiceRouter) {
    glue_ok(
        router,
        "CreateDatabase",
        &json!({ "DatabaseInput": { "Name": SEEDED_DATABASE } }),
    )
    .await;
}

/// The request each read takes for ONE address: every one of them names the catalog and the
/// database, and the single-table read names a table of it as well.
fn request(operation: &str, catalog: &str, database: &str) -> Value {
    match operation {
        "GetTableMetadata" => json!({
            "CatalogName": catalog,
            "DatabaseName": database,
            "TableName": "any_table_of_it"
        }),
        _ => json!({ "CatalogName": catalog, "DatabaseName": database }),
    }
}

/// The reads that resolve a database by calling the Data Catalog: the two the recorded cases
/// exercise and the read of the database record itself.
const RELAYING_READS: [&str; 3] = ["GetDatabase", "GetTableMetadata", "ListTableMetadata"];

/// Drive one read of an absent database and assert the relayed refusal, answering with the
/// message carrying its request id replaced by a fixed token — so two reads of the same
/// address are compared on everything BUT the per-call id.
async fn relayed_miss(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    catalog: &str,
    database: &str,
) -> String {
    let (status, body) = athena(router, operation, &request(operation, catalog, database)).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        "MetadataException",
        "case {case:?}: expected MetadataException, got {body}"
    );
    let refusal = message(&body);
    let request_id = relayed_request_id(case, &refusal, &database_miss(database));
    refusal.replace(&request_id, "<request id>")
}

/// Every read that resolves a database through the catalog relays the catalog's own sentence
/// inside the caught-exception clause, whichever read was asked, however the catalog was
/// addressed and whyever the database was absent.
#[tokio::test]
async fn every_metadata_read_relays_a_database_miss_inside_the_clause() {
    let router = emulator();
    seed(&router).await;
    let second_name = register_second_name(&router, "relay_agreement_second_name").await;
    // A database the catalog held and then dropped: the miss is the catalog's current answer
    // and not a name that never existed.
    glue_ok(
        &router,
        "CreateDatabase",
        &json!({ "DatabaseInput": { "Name": "relay_dropped_db" } }),
    )
    .await;
    glue_ok(
        &router,
        "DeleteDatabase",
        &json!({ "Name": "relay_dropped_db" }),
    )
    .await;

    // (why no database answers, the name asked for)
    let absent: [(&str, &str); 4] = [
        ("a name nothing created", "relay_no_such_db"),
        ("a near miss of a stored name", "relay_agreement_dbx"),
        ("a mixed-case name nothing created", "RelayAgreementAbsent"),
        ("a database the catalog dropped again", "relay_dropped_db"),
    ];
    // How the one Data Catalog is addressed.
    let catalogs: [(&str, &str); 3] = [
        ("the catalog's own name", GLUE_CATALOG),
        ("that name lowercased", "awsdatacatalog"),
        (
            "a registered GLUE catalog of the same catalog id",
            second_name.as_str(),
        ),
    ];

    for (why, database) in absent {
        for (addressed, catalog) in catalogs {
            // Every read of this one address, compared against one another: the clause is
            // one renderer's, so the reads agree apart from each call's own request id.
            let mut reported: Vec<(&str, String)> = Vec::new();
            for operation in RELAYING_READS {
                let case = format!("{operation}, {why}, through {addressed}");
                let refusal = relayed_miss(&router, &case, operation, catalog, database).await;
                reported.push((operation, refusal));
            }
            let (first_read, first) = &reported[0];
            for (operation, refusal) in &reported[1..] {
                assert_eq!(
                    refusal, first,
                    "case {operation:?} vs {first_read:?} ({why}, through {addressed}): the \
                     reads must relay one message, got {refusal:?} and {first:?}"
                );
            }
        }
    }
}

/// An address the catalog DOES resolve is served by the same read, and the clause appears
/// only once the catalog actually refuses: it is the answer to a real catalog refusal rather
/// than something the read states for every address.
#[tokio::test]
async fn an_address_the_catalog_resolves_is_served_by_the_relaying_read() {
    let router = emulator();
    seed(&router).await;

    let served = athena_ok(
        &router,
        "GetDatabase",
        &request("GetDatabase", GLUE_CATALOG, SEEDED_DATABASE),
    )
    .await;
    assert_eq!(
        served["Database"]["Name"],
        json!(SEEDED_DATABASE),
        "case \"a database the catalog holds\": expected the stored database, got {served}"
    );

    glue_ok(
        &router,
        "DeleteDatabase",
        &json!({ "Name": SEEDED_DATABASE }),
    )
    .await;
    let refusal = relayed_miss(
        &router,
        "a database the catalog dropped",
        "GetDatabase",
        GLUE_CATALOG,
        SEEDED_DATABASE,
    )
    .await;
    assert!(
        refusal.contains("EntityNotFoundException"),
        "case \"a database the catalog dropped\": the clause must state the catalog's own \
         error code, got {refusal:?}"
    );
}
