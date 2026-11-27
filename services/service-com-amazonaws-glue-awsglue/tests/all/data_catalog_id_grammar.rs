// SPDX-License-Identifier: CC0-1.0

//! The grammar a `CatalogId` that SELECTS a Data Catalog is measured against, on every
//! surface that resolves one before looking an entity up.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is HTTP behaviour and not a direct call into a handler.
//!
//! RULE under test: a `CatalogId` that selects the Data Catalog an entity lives in is a
//! constrained identifier, not a free-form name — the holding account's own twelve-digit
//! number, alone or followed by the path of a catalog inside it written in the characters a
//! catalog id carries (lowercase letters, digits, underscores, hyphens and the nesting
//! separator) — so an id outside that form selects no catalog any account could hold, now or
//! after any create. That makes it a verdict on the REQUEST: every surface that resolves a
//! `CatalogId` refuses such an id as invalid input (HTTP 400 `InvalidInputException` with the
//! service's fixed grammar sentence), decided BEFORE the cross-account authorization decision
//! and before the entity lookup, and only an operation whose model declares no
//! `InvalidInputException` answers otherwise — it keeps its own catalog miss, an error code
//! outside an operation's modelled set being one no typed client can map. An id that IS of
//! the catalog-id form keeps the answer its target earns: the account's own catalog serves
//! the entity, another account's catalog is the authorization refusal, and a well-formed id
//! this account holds no catalog under is the catalog miss (HTTP 400
//! `EntityNotFoundException` with the catalog-miss sentence).
//!
//! The rows differ along HOW an id leaves the grammar (no account-id prefix, too few or too
//! many digits, upper case, a character outside the id's set, an empty path, a whole ARN)
//! and along the form a WELL-FORMED id takes (the bare account, a named catalog, a nested
//! path, another account's), each crossed with operations from both resolution paths — the
//! ones that tell a missing catalog from a foreign one, the ones that can report only the
//! authorization refusal, and one that models no invalid-input error at all.
//!
//! [`crate::catalog_id_grammar`] pins the OTHER grammar an id is measured against: the
//! catalog-NAME pattern the operations that address a catalog RESOURCE by its name use.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{call, emulator, error_code, error_message, ok, ACCOUNT_ID, REGION};

/// The whole `Message` a request whose `CatalogId` is no catalog id at all earns. Fixed
/// text: it states the rule and interpolates nothing — no id, account, Region or name.
const CATALOG_ID_INVALID: &str = "Catalog id is invalid. The catalog id must be prepended \
     with the account id prefix and can only contain lowercase letters, numbers, \
     underscores, and slashes.";

/// The whole `Message` a WELL-FORMED `CatalogId` that selects no catalog this endpoint
/// serves earns.
const CATALOG_NOT_FOUND: &str = "Catalog not found.";

/// The database every request below addresses, so the only thing a row varies is the
/// `CatalogId`.
const DATABASE: &str = "orders_db";

/// The table inside it.
const TABLE: &str = "orders";

/// An emulator holding one database with one table in the account's own Data Catalog.
async fn emulator_with_entities() -> MultiServiceRouter {
    let router = emulator();
    ok(
        &router,
        "CreateDatabase",
        &json!({ "DatabaseInput": { "Name": DATABASE } }),
    )
    .await;
    ok(
        &router,
        "CreateTable",
        &json!({ "DatabaseName": DATABASE, "TableInput": { "Name": TABLE } }),
    )
    .await;
    router
}

/// The requests a row is asked of: one per operation that resolves a `CatalogId` before it
/// looks anything up, drawn from both resolution paths — reads and writes of databases and
/// tables, the listings, a connection read, the search and the catalog's encryption
/// settings. The flag says whether the operation's modelled error set carries
/// `InvalidInputException` (`smithy_query query="operation" operation="GetDatabase,DeleteConnection"`):
/// every one that does states the grammar verdict, while `DeleteConnection`, which models
/// only the entity miss and the timeout, keeps answering its catalog miss.
fn catalog_addressed_requests() -> Vec<(&'static str, Value, bool)> {
    vec![
        ("GetDatabase", json!({ "Name": DATABASE }), true),
        ("GetDatabases", json!({}), true),
        (
            "GetTable",
            json!({ "DatabaseName": DATABASE, "Name": TABLE }),
            true,
        ),
        ("GetTables", json!({ "DatabaseName": DATABASE }), true),
        (
            "GetTableVersions",
            json!({ "DatabaseName": DATABASE, "TableName": TABLE }),
            true,
        ),
        (
            "UpdateDatabase",
            json!({ "Name": DATABASE, "DatabaseInput": { "Name": DATABASE } }),
            true,
        ),
        ("DeleteDatabase", json!({ "Name": DATABASE }), true),
        (
            "CreateDatabase",
            json!({ "DatabaseInput": { "Name": "written_db" } }),
            true,
        ),
        ("GetConnection", json!({ "Name": "no-such-connection" }), true),
        ("SearchTables", json!({}), true),
        ("GetDataCatalogEncryptionSettings", json!({}), true),
        (
            "DeleteConnection",
            json!({ "ConnectionName": "no-such-connection" }),
            false,
        ),
    ]
}

/// The same request, addressed at `catalog_id`.
fn addressed_at(request: &Value, catalog_id: &str) -> Value {
    let mut body = request.clone();
    body["CatalogId"] = json!(catalog_id);
    body
}

/// Every id that is NO catalog id, and the axis of the grammar each one leaves.
///
/// The character set and the account-id prefix are the ones the sentence such an id is
/// refused with states ([`CATALOG_ID_INVALID`]).
fn malformed_catalog_ids() -> Vec<(&'static str, String)> {
    vec![
        (
            "a bare catalog name, carrying no prefix",
            "analytics".to_string(),
        ),
        ("too few digits for an account number", "12345".to_string()),
        (
            "too many digits for an account number",
            "1234567890123".to_string(),
        ),
        ("a Region-shaped value", REGION.to_string()),
        (
            "an account prefix followed by nothing",
            format!("{ACCOUNT_ID}:"),
        ),
        (
            "a prefixed path in upper case",
            format!("{ACCOUNT_ID}:Analytics"),
        ),
        (
            "a prefixed path carrying a dot",
            format!("{ACCOUNT_ID}:ana.lytics"),
        ),
        (
            "a prefixed path carrying a space",
            format!("{ACCOUNT_ID}:ana lytics"),
        ),
        (
            "a prefixed path carrying a backslash",
            format!("{ACCOUNT_ID}:ana\\lytics"),
        ),
        (
            "a whole catalog ARN",
            format!("arn:aws:glue:{REGION}:{ACCOUNT_ID}:catalog"),
        ),
        (
            "an account-shaped prefix that is not digits",
            "abcdefghijkl:analytics".to_string(),
        ),
    ]
}

/// The grammar verdict is the same on every surface that resolves a `CatalogId`, and it is
/// the WHOLE answer: HTTP 400 with the grammar sentence as the operation's own
/// `InvalidInputException` — except on an operation that models no such error, which keeps
/// answering its catalog miss.
#[tokio::test]
async fn an_id_outside_the_catalog_id_grammar_is_refused() {
    let router = emulator_with_entities().await;

    for (case, catalog_id) in malformed_catalog_ids() {
        for (operation, request, models_invalid_input) in catalog_addressed_requests() {
            let label = format!("{case} ({catalog_id:?}) via {operation}");
            let (expected_code, expected_message) = if models_invalid_input {
                ("InvalidInputException", CATALOG_ID_INVALID)
            } else {
                ("EntityNotFoundException", CATALOG_NOT_FOUND)
            };
            let (status, body) =
                call(&router, operation, &addressed_at(&request, &catalog_id)).await;
            assert_eq!(
                status, 400,
                "case {label}: expected 400, got {status} with {body}"
            );
            assert!(
                error_code(&body).ends_with(expected_code),
                "case {label}: expected {expected_code}, got {} in {body}",
                error_code(&body)
            );
            assert_eq!(
                error_message(&body),
                expected_message,
                "case {label}: expected {expected_message:?}, got {:?}",
                error_message(&body)
            );
        }
    }

    // Every refused call wrote nothing: the database the create named is absent, and the
    // database the update and the delete addressed is still there.
    let (status, body) = call(&router, "GetDatabase", &json!({ "Name": "written_db" })).await;
    assert_eq!(
        status, 400,
        "a refused create must have written nothing, got {status} with {body}"
    );
    let served = ok(&router, "GetDatabase", &json!({ "Name": DATABASE })).await;
    assert_eq!(
        served["Database"]["Name"].as_str(),
        Some(DATABASE),
        "a refused update or delete must leave the database as it was, got {served}"
    );
}

/// The verdict is a constraint the REQUEST alone decides, so it precedes the cross-account
/// authorization decision: a malformed id that carries ANOTHER account's number is still
/// answered on the id, not as the authorization refusal.
#[tokio::test]
async fn the_grammar_precedes_the_cross_account_decision() {
    let router = emulator_with_entities().await;
    let foreign_account = "210987654321";

    for (case, catalog_id, code, message) in [
        (
            "another account's Data Catalog, well formed",
            foreign_account.to_string(),
            "AccessDeniedException",
            None,
        ),
        (
            "a catalog of another account, well formed",
            format!("{foreign_account}:analytics"),
            "AccessDeniedException",
            None,
        ),
        (
            "a catalog of another account in upper case",
            format!("{foreign_account}:Analytics"),
            "InvalidInputException",
            Some(CATALOG_ID_INVALID),
        ),
        (
            "another account's number with a digit too few",
            "21098765432".to_string(),
            "InvalidInputException",
            Some(CATALOG_ID_INVALID),
        ),
    ] {
        let (status, body) = call(
            &router,
            "GetDatabase",
            &json!({ "CatalogId": catalog_id, "Name": DATABASE }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400 for {catalog_id:?}, got {status} with {body}"
        );
        assert!(
            error_code(&body).ends_with(code),
            "case {case}: expected {code} for {catalog_id:?}, got {} in {body}",
            error_code(&body)
        );
        if let Some(message) = message {
            assert_eq!(
                error_message(&body),
                message,
                "case {case}: expected {message:?} for {catalog_id:?}, got {:?}",
                error_message(&body)
            );
        }
    }
}

/// The verdict is decided from the request alone, so it does not depend on what the store
/// holds: the same malformed id earns the same answer before and after a catalog of that
/// name is created, while the WELL-FORMED id of that catalog goes from the catalog miss to
/// the entity lookup inside it.
#[tokio::test]
async fn the_grammar_is_decided_without_reading_the_store() {
    let router = emulator_with_entities().await;
    let catalog_name = "analytics";
    let qualified = format!("{ACCOUNT_ID}:{catalog_name}");

    let before = call(
        &router,
        "GetDatabase",
        &json!({ "CatalogId": catalog_name, "Name": DATABASE }),
    )
    .await;
    assert_eq!(
        error_message(&before.1),
        CATALOG_ID_INVALID,
        "case before the create: a malformed id is refused on its spelling alone, got {:?}",
        before.1
    );

    // A well-formed id nothing has created is the catalog MISS, which IS decided from the
    // store.
    let (status, body) = call(
        &router,
        "GetDatabase",
        &json!({ "CatalogId": &qualified, "Name": DATABASE }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case before the create: expected 400, got {status} with {body}"
    );
    assert_eq!(
        error_message(&body),
        CATALOG_NOT_FOUND,
        "case before the create: expected the catalog miss, got {body}"
    );

    // Register that catalog the only way `CreateCatalog` registers one: a federation at a
    // source one of AWS's own managed integrations registered.
    ok(
        &router,
        "CreateCatalog",
        &json!({
            "Name": catalog_name,
            "CatalogInput": {
                "FederatedCatalog": {
                    "Identifier": format!("arn:aws:s3tables:{REGION}:{ACCOUNT_ID}:bucket/held"),
                    "ConnectionName": "aws:s3tables",
                },
            },
        }),
    )
    .await;

    // The malformed id is refused exactly as before: the store moved, the verdict did not.
    let after = call(
        &router,
        "GetDatabase",
        &json!({ "CatalogId": catalog_name, "Name": DATABASE }),
    )
    .await;
    assert_eq!(
        (after.0, error_message(&after.1)),
        (before.0, error_message(&before.1)),
        "case after the create: the grammar verdict must not depend on the store, got {:?}",
        after.1
    );

    // The well-formed id of that catalog now reaches the entity lookup inside it, which
    // holds no database of that name.
    let (status, body) = call(
        &router,
        "GetDatabase",
        &json!({ "CatalogId": &qualified, "Name": DATABASE }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case after the create: expected 400, got {status} with {body}"
    );
    assert_eq!(
        error_message(&body),
        format!("Database {DATABASE} not found."),
        "case after the create: expected the entity miss inside that catalog, got {body}"
    );
}

/// A well-formed id keeps the answer its target earns, in every form it takes: absent (the
/// documented default) and the bare account number serve the entity, while an account-
/// qualified path this account holds no catalog under is the catalog miss.
#[tokio::test]
async fn a_well_formed_catalog_id_keeps_its_answer() {
    let router = emulator_with_entities().await;

    for (case, catalog_id) in [
        ("no CatalogId at all", None),
        ("the account's own number", Some(ACCOUNT_ID.to_string())),
    ] {
        let mut body = json!({ "Name": DATABASE });
        if let Some(catalog_id) = &catalog_id {
            body["CatalogId"] = json!(catalog_id);
        }
        let served = ok(&router, "GetDatabase", &body).await;
        assert_eq!(
            served["Database"]["Name"].as_str(),
            Some(DATABASE),
            "case {case}: the account's own Data Catalog must serve the database, got {served}"
        );
    }

    for (case, catalog_id) in [
        (
            "a named catalog this account holds nothing under",
            format!("{ACCOUNT_ID}:never-created"),
        ),
        (
            "a nested path this account holds nothing under",
            format!("{ACCOUNT_ID}:s3tablescatalog/orders_bucket"),
        ),
        (
            "a path spelled with an underscore",
            format!("{ACCOUNT_ID}:never_created"),
        ),
    ] {
        let (status, body) = call(
            &router,
            "GetDatabase",
            &json!({ "CatalogId": &catalog_id, "Name": DATABASE }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400 for {catalog_id:?}, got {status} with {body}"
        );
        assert!(
            error_code(&body).ends_with("EntityNotFoundException"),
            "case {case}: expected EntityNotFoundException for {catalog_id:?}, got {} in {body}",
            error_code(&body)
        );
        assert_eq!(
            error_message(&body),
            CATALOG_NOT_FOUND,
            "case {case}: expected the catalog miss for {catalog_id:?}, got {body}"
        );
    }
}

/// The two verdicts stay DISTINCT, and the only thing that decides between them is the
/// spelling of the id: an id outside the grammar is the input refusal stating the grammar,
/// while the well-formed id it would be if it were spelled as one is the catalog miss.
///
/// Each row is a PAIR differing only along one axis of the grammar — the account-id prefix,
/// the case of the path, the characters of the path, an empty path — asked of the operations
/// that resolve the catalog an entity lives in, so neither verdict can drift into the other
/// on any of them. A typed client branches on the code alone, and the two codes mean
/// opposite things: one says the id can never name a catalog, the other that none has been
/// created yet.
#[tokio::test]
async fn the_grammar_refusal_and_the_catalog_miss_stay_distinct() {
    let router = emulator_with_entities().await;

    let operations: [(&str, Value); 5] = [
        ("GetDatabase", json!({ "Name": DATABASE })),
        ("GetDatabases", json!({})),
        (
            "GetTable",
            json!({ "DatabaseName": DATABASE, "Name": TABLE }),
        ),
        ("GetTables", json!({ "DatabaseName": DATABASE })),
        ("DeleteDatabase", json!({ "Name": DATABASE })),
    ];

    for (axis, malformed, well_formed) in [
        (
            "the account-id prefix",
            "analytics".to_string(),
            format!("{ACCOUNT_ID}:analytics"),
        ),
        (
            "the case of the path",
            format!("{ACCOUNT_ID}:Analytics"),
            format!("{ACCOUNT_ID}:analytics"),
        ),
        (
            "the characters of the path",
            format!("{ACCOUNT_ID}:ana.lytics"),
            format!("{ACCOUNT_ID}:ana_lytics"),
        ),
        (
            // A hyphen is in BOTH halves of this pair: a catalog's own name may carry one
            // (the service's own catalog-name grammar sentence: "Only lowercase letters,
            // numbers, underscores and hyphens are allowed for catalog name"), and the path
            // segments of an id are catalog names, so the prefix is what this pair differs
            // along.
            "the account-id prefix on a Region-shaped path",
            REGION.to_string(),
            format!("{ACCOUNT_ID}:{REGION}"),
        ),
        (
            "an empty path",
            format!("{ACCOUNT_ID}:"),
            format!("{ACCOUNT_ID}:inner"),
        ),
    ] {
        for (operation, request) in &operations {
            for (verdict, catalog_id, expected_code, expected_message) in [
                (
                    "outside the grammar",
                    &malformed,
                    "InvalidInputException",
                    CATALOG_ID_INVALID,
                ),
                (
                    "well formed, naming no catalog",
                    &well_formed,
                    "EntityNotFoundException",
                    CATALOG_NOT_FOUND,
                ),
            ] {
                let label = format!("{axis}, {verdict} ({catalog_id:?}) via {operation}");
                let (status, body) =
                    call(&router, operation, &addressed_at(request, catalog_id)).await;
                assert_eq!(
                    status, 400,
                    "case {label}: expected 400, got {status} with {body}"
                );
                assert!(
                    error_code(&body).ends_with(expected_code),
                    "case {label}: expected {expected_code}, got {} in {body}",
                    error_code(&body)
                );
                assert_eq!(
                    error_message(&body),
                    expected_message,
                    "case {label}: expected {expected_message:?}, got {:?}",
                    error_message(&body)
                );
            }
        }
    }

    // Not one of those refused calls touched the store: the database is still served out of
    // the account's own Data Catalog, with the table it was created holding.
    let served = ok(&router, "GetDatabase", &json!({ "Name": DATABASE })).await;
    assert_eq!(
        served["Database"]["Name"].as_str(),
        Some(DATABASE),
        "a refused call must leave the database as it was, got {served}"
    );
    let tables = ok(&router, "GetTables", &json!({ "DatabaseName": DATABASE })).await;
    assert_eq!(
        tables["TableList"][0]["Name"].as_str(),
        Some(TABLE),
        "a refused call must leave the table as it was, got {tables}"
    );
}
