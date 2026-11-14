// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for WHICH refusal a Glue entity lookup answers, and in whose words:
//! the Data Catalog's database, table and catalog misses, the job-run miss, and the schema
//! read's reference rules.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is HTTP behaviour and not a direct call into a handler.
//!
//! The rules under test, each a table whose rows differ along what the rule generalises
//! over — a recorded case is ONE row of each:
//!
//! 1. A refusal that NAMES a Data Catalog database or table renders the name as the
//!    catalog KEYS it: the Hive-compatible fold the model states for the member ("For Hive
//!    compatibility, this is folded to lowercase when it is stored", `DatabaseInput$Name`
//!    and `TableInput$Name`), not the casing the request sent.
//! 2. Each operation words its own miss: the table READ answers one fixed sentence that
//!    names no entity — for an absent DATABASE as much as for an absent table — while the
//!    version listing names the table it could not resolve.
//! 3. A run read addresses a run by the (job name, run id) PAIR, so either half being
//!    unknown is the one fixed RUN miss and never the job-definition miss.
//! 4. A `CatalogId` that names no catalog this endpoint serves is a MISS, decided before
//!    any authorization decision; only a catalog id of ANOTHER account is the
//!    authorization refusal.
//! 5. The schema read takes a COMPLETE reference — a schema ARN, or a registry and a
//!    schema name together — and resolves the registry before the schema set, so an
//!    absent registry is reported as the registry.
//!
//! The expected sentences are written out here as literals rather than read from the
//! service's own constants, so this suite pins the wording instead of restating it.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{call, emulator, error_code, error_message, ok, ACCOUNT_ID, REGION};

/// The code every miss here is modelled with: "A specified entity does not exist"
/// (`EntityNotFoundException`, HTTP 400, from
/// `smithy_query(query="operation",operation="GetDatabase,GetTable,GetTableVersions,GetJobRun,GetSchema")`).
const ENTITY_NOT_FOUND: &str = "EntityNotFoundException";

/// The code a bad parameter is modelled with: "The input provided was not valid"
/// (`InvalidInputException`, HTTP 400, same `smithy_query` call).
const INVALID_INPUT: &str = "InvalidInputException";

/// The whole `Message` the table READ answers for either of its misses.
const TABLE_ENTITY_NOT_FOUND: &str = "Entity Not Found";

/// The whole `Message` a run read answers when the (job name, run id) pair resolves to no
/// run.
const JOB_RUN_NOT_FOUND: &str = "Job run not found for the given job name and runId.";

/// The whole `Message` a request addressed to a catalog that does not exist earns.
const CATALOG_NOT_FOUND: &str = "Catalog not found.";

/// The whole `Message` the schema read answers for a `SchemaId` that addresses no schema
/// set completely.
const SCHEMA_ID_INCOMPLETE: &str =
    "At least one of (registryName and schemaName) or schemaArn has to be provided.";

/// The whole `Message` the schema read answers for a registry that exists but holds no
/// such schema set.
const SCHEMA_NOT_FOUND: &str = "Schema is not found.";

/// Names spanning the axis the fold generalises over: an all-lowercase name (already its
/// own folded form), mixed case, upper case, mixed case carrying the digits and
/// punctuation `NameString`'s pattern admits, and a mixed-case name at the member's
/// documented 255-character ceiling (`NameString`, length 1..255, from
/// `smithy_query(query="operation",operation="GetDatabase")`).
fn name_casings() -> Vec<(&'static str, String)> {
    vec![
        ("already folded", "absent_entity".to_string()),
        ("mixed case", "Absent_Entity".to_string()),
        ("upper case", "ABSENT_ENTITY".to_string()),
        (
            "mixed case with digits and punctuation",
            "Team.Data_Lake-2026/Raw".to_string(),
        ),
        ("mixed case at the 255 ceiling", "Nn".repeat(127) + "N"),
    ]
}

/// Assert one refusal: HTTP 400, the expected error code, and the expected `Message`
/// verbatim.
async fn assert_refusal(
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
        "case {case}: expected {expected_code}, got {}",
        error_code(&body)
    );
    assert_eq!(
        error_message(&body),
        expected_message,
        "case {case}: expected Message {expected_message:?}, got {:?}",
        error_message(&body)
    );
    body
}

/// Create the database `name`.
async fn create_database(router: &MultiServiceRouter, name: &str) {
    ok(
        router,
        "CreateDatabase",
        &json!({ "DatabaseInput": { "Name": name } }),
    )
    .await;
}

/// Create the table `table` inside `database`.
async fn create_table(router: &MultiServiceRouter, database: &str, table: &str) {
    ok(
        router,
        "CreateTable",
        &json!({ "DatabaseName": database, "TableInput": { "Name": table } }),
    )
    .await;
}

/// Rule 1 for the DATABASE miss: every operation that names the database it could not
/// resolve renders the folded name, whatever casing the request sent.
///
/// The rows are the cross product of [`name_casings`] with the operations that name the
/// database: the three that address it by `Name` and the two table operations that name it
/// by `DatabaseName`.
#[tokio::test]
async fn a_database_miss_names_the_folded_database() {
    let router = emulator();
    // A database that DOES exist, so each refusal is about the name it addressed.
    create_database(&router, "present_db").await;

    for (case, name) in name_casings() {
        let folded = name.to_ascii_lowercase();
        let expected = format!("Database {folded} not found.");
        let requests: [(&str, Value); 5] = [
            ("GetDatabase", json!({ "Name": name })),
            ("DeleteDatabase", json!({ "Name": name })),
            (
                "UpdateDatabase",
                json!({ "Name": name, "DatabaseInput": { "Name": name } }),
            ),
            ("GetTables", json!({ "DatabaseName": name })),
            (
                "DeleteTable",
                json!({ "DatabaseName": name, "Name": "any_table" }),
            ),
        ];
        for (operation, request) in requests {
            let label = format!("{case} via {operation}");
            let body = assert_refusal(
                &router,
                &label,
                operation,
                &request,
                ENTITY_NOT_FOUND,
                &expected,
            )
            .await;
            // A request whose spelling differs from the folded one must not see its own
            // spelling back: the refusal names the catalog's name.
            if name != folded {
                assert!(
                    !error_message(&body).contains(name.as_str()),
                    "case {label}: the refusal must name the folded database {folded:?}, got \
                     {:?}",
                    error_message(&body)
                );
            }
        }
    }
}

/// Rule 1 for the TABLE miss, and rule 2: the version listing NAMES the table it could not
/// resolve, folded, while the table read answers the fixed sentence for the same lookup.
///
/// The rows are [`name_casings`] crossed with how the table misses — a name nothing
/// created, and one created and then deleted.
#[tokio::test]
async fn a_table_miss_names_the_folded_table_where_the_operation_names_it() {
    let router = emulator();
    create_database(&router, "present_db").await;
    create_table(&router, "present_db", "present_table").await;
    create_table(&router, "present_db", "Deleted_Table").await;
    ok(
        &router,
        "DeleteTable",
        &json!({ "DatabaseName": "present_db", "Name": "Deleted_Table" }),
    )
    .await;

    let mut rows = name_casings();
    rows.push(("created then deleted", "Deleted_Table".to_string()));

    for (case, name) in rows {
        let folded = name.to_ascii_lowercase();
        let expected = format!("Table {folded} not found.");
        let listed = assert_refusal(
            &router,
            &format!("{case} via GetTableVersions"),
            "GetTableVersions",
            &json!({ "DatabaseName": "present_db", "TableName": name }),
            ENTITY_NOT_FOUND,
            &expected,
        )
        .await;
        assert!(
            !error_message(&listed).contains("present_db"),
            "case {case}: the version listing's miss names no database, got {:?}",
            error_message(&listed)
        );

        // The writes that address one table word the same miss the same way.
        for (operation, request) in [
            (
                "DeleteTable",
                json!({ "DatabaseName": "present_db", "Name": name }),
            ),
            (
                "UpdateTable",
                json!({ "DatabaseName": "present_db", "TableInput": { "Name": name } }),
            ),
        ] {
            assert_refusal(
                &router,
                &format!("{case} via {operation}"),
                operation,
                &request,
                ENTITY_NOT_FOUND,
                &expected,
            )
            .await;
        }

        // And the READ answers its own fixed sentence for that same table, so the two are
        // distinguishable.
        let read = assert_refusal(
            &router,
            &format!("{case} via GetTable"),
            "GetTable",
            &json!({ "DatabaseName": "present_db", "Name": name }),
            ENTITY_NOT_FOUND,
            TABLE_ENTITY_NOT_FOUND,
        )
        .await;
        assert_ne!(
            error_message(&read),
            expected,
            "case {case}: the read and the writes must be distinguishable by message"
        );
    }
}

/// Rule 2 for the table read's OTHER miss: an absent DATABASE is the same fixed sentence,
/// which names neither the database nor the table — the read never answers the templated
/// database miss its siblings do.
///
/// The rows are [`name_casings`] (the database that does not exist) crossed with whether
/// the table named inside it exists anywhere.
#[tokio::test]
async fn the_table_read_words_an_absent_database_as_its_fixed_miss() {
    let router = emulator();
    create_database(&router, "present_db").await;
    create_table(&router, "present_db", "present_table").await;

    for (case, database) in name_casings() {
        for table in ["present_table", "absent_table"] {
            let label = format!("{case} / {table}");
            let body = assert_refusal(
                &router,
                &label,
                "GetTable",
                &json!({ "DatabaseName": database, "Name": table }),
                ENTITY_NOT_FOUND,
                TABLE_ENTITY_NOT_FOUND,
            )
            .await;
            let message = error_message(&body);
            assert!(
                !message.contains(database.as_str()) && !message.contains(table),
                "case {label}: the read's miss names no entity, got {message:?}"
            );
        }
    }

    // The table that IS stored is still served, so the refusals above are misses and not a
    // broken lookup.
    let served = ok(
        &router,
        "GetTable",
        &json!({ "DatabaseName": "present_db", "Name": "present_table" }),
    )
    .await;
    assert_eq!(
        served["Table"]["Name"].as_str(),
        Some("present_table"),
        "the stored table must still be served, got {served}"
    );
}

/// Rule 3: a run read answers the one fixed RUN miss whenever the (job name, run id) pair
/// resolves to no run — for a job this Region holds no definition of as much as for a run
/// id a stored job never minted.
///
/// The rows differ along which half of the pair misses, the job name's casing and length,
/// and the form of the run id.
#[tokio::test]
async fn a_run_read_answers_the_run_miss_for_either_half_of_the_pair() {
    let router = emulator();
    ok(
        &router,
        "CreateJob",
        &json!({
            "Name": "present-job",
            "Role": format!("arn:aws:iam::{ACCOUNT_ID}:role/glue-job-role"),
            "Command": { "Name": "glueetl", "ScriptLocation": "s3://scripts/etl.py" },
        }),
    )
    .await;

    let rows: Vec<(&str, String, String)> = vec![
        (
            "a job nothing created",
            "absent-job".to_string(),
            "jr_0".to_string(),
        ),
        (
            "a job nothing created, mixed case",
            "Absent_Job".to_string(),
            "jr_0".to_string(),
        ),
        (
            "a job nothing created, at the 255 ceiling",
            "n".repeat(255),
            "jr_0".to_string(),
        ),
        (
            "a stored job and a run it never minted",
            "present-job".to_string(),
            "jr_1".to_string(),
        ),
        (
            "a stored job and a run id at the 255 ceiling",
            "present-job".to_string(),
            "r".repeat(255),
        ),
        (
            "a stored job addressed in another casing",
            "PRESENT-JOB".to_string(),
            "jr_1".to_string(),
        ),
    ];

    for (case, job_name, run_id) in rows {
        let body = assert_refusal(
            &router,
            case,
            "GetJobRun",
            &json!({ "JobName": job_name, "RunId": run_id }),
            ENTITY_NOT_FOUND,
            JOB_RUN_NOT_FOUND,
        )
        .await;
        let message = error_message(&body);
        assert!(
            !message.contains(job_name.as_str()) && !message.contains(run_id.as_str()),
            "case {case}: the run miss carries no part of the request, got {message:?}"
        );
    }
}

/// Rule 4: a `CatalogId` naming no catalog this endpoint serves is the catalog MISS,
/// answered before any authorization decision, while a catalog id of another account stays
/// the authorization refusal.
///
/// The rows differ along the SHAPE of the id — digits too few for an account number, a
/// bare name, a Region-shaped value, a name qualified by this account, and a name in
/// another casing — crossed with the operations that resolve a `CatalogId` before looking
/// an entity up.
#[tokio::test]
async fn a_catalog_id_naming_no_catalog_is_a_miss_before_authorization() {
    let router = emulator();
    create_database(&router, "present_db").await;
    create_table(&router, "present_db", "present_table").await;

    let operations: [(&str, Value); 5] = [
        ("GetDatabase", json!({ "Name": "present_db" })),
        ("GetDatabases", json!({})),
        (
            "GetTable",
            json!({ "DatabaseName": "present_db", "Name": "present_table" }),
        ),
        ("GetTables", json!({ "DatabaseName": "present_db" })),
        (
            "GetTableVersions",
            json!({ "DatabaseName": "present_db", "TableName": "present_table" }),
        ),
    ];

    let misses = [
        ("too few digits for an account number", "12345".to_string()),
        ("a bare name nothing created", "never-created".to_string()),
        ("a Region-shaped value", REGION.to_string()),
        (
            "a name this account holds no catalog under",
            format!("{ACCOUNT_ID}:never-created"),
        ),
        ("a name in another casing", "NEVER-CREATED".to_string()),
    ];
    for (case, catalog_id) in &misses {
        for (operation, request) in &operations {
            let label = format!("{case} via {operation}");
            let mut body = request.clone();
            body["CatalogId"] = json!(catalog_id);
            assert_refusal(
                &router,
                &label,
                operation,
                &body,
                ENTITY_NOT_FOUND,
                CATALOG_NOT_FOUND,
            )
            .await;
        }
    }

    // A catalog id of ANOTHER account is a different fault: that catalog is reachable only
    // through its own resource policy, so it stays the authorization refusal, which names
    // the action the caller may not perform.
    for (case, catalog_id) in [
        ("another account's Data Catalog", "111122223333".to_string()),
        (
            "a catalog of another account",
            "111122223333:shared".to_string(),
        ),
    ] {
        for (operation, request) in &operations {
            let label = format!("{case} via {operation}");
            let mut body = request.clone();
            body["CatalogId"] = json!(catalog_id);
            let (status, answer) = call(&router, operation, &body).await;
            assert_eq!(
                status, 400,
                "case {label}: expected 400, got {status} with {answer}"
            );
            assert!(
                error_code(&answer).contains("AccessDeniedException"),
                "case {label}: expected AccessDeniedException, got {}",
                error_code(&answer)
            );
            assert!(
                error_message(&answer).contains(&format!("glue:{operation}")),
                "case {label}: the authorization refusal names the action, got {:?}",
                error_message(&answer)
            );
        }
    }

    // And a catalog this account DOES hold is resolved, so the miss above is decided from
    // stored state: the request reaches the entity lookup and earns the ENTITY's miss.
    ok(
        &router,
        "CreateCatalog",
        &json!({
            "Name": "stored-catalog",
            "CatalogInput": {
                "FederatedCatalog": {
                    "Identifier": format!("arn:aws:s3tables:{REGION}:{ACCOUNT_ID}:bucket/stored"),
                    "ConnectionName": "aws:s3tables",
                },
            },
        }),
    )
    .await;
    for catalog_id in ["stored-catalog", &format!("{ACCOUNT_ID}:stored-catalog")] {
        assert_refusal(
            &router,
            &format!("a stored catalog addressed as {catalog_id:?}"),
            "GetDatabase",
            &json!({ "CatalogId": catalog_id, "Name": "present_db" }),
            ENTITY_NOT_FOUND,
            "Database present_db not found.",
        )
        .await;
    }

    // The account's own catalog still serves the database, addressed by id or by default.
    for catalog_id in [None, Some(ACCOUNT_ID.to_string())] {
        let mut body = json!({ "Name": "present_db" });
        if let Some(id) = &catalog_id {
            body["CatalogId"] = json!(id);
        }
        let served = ok(&router, "GetDatabase", &body).await;
        assert_eq!(
            served["Database"]["Name"].as_str(),
            Some("present_db"),
            "case {catalog_id:?}: the account's own catalog must serve the database, got \
             {served}"
        );
    }
}

/// Rule 5: the schema read takes a complete reference and resolves the registry first.
///
/// The rows differ along WHICH half of the reference is missing, whether the registry
/// exists, and the form the reference takes (names or ARN).
#[tokio::test]
async fn the_schema_read_requires_a_complete_reference_and_names_an_absent_registry() {
    let router = emulator();
    ok(
        &router,
        "CreateRegistry",
        &json!({ "RegistryName": "payments-registry" }),
    )
    .await;
    ok(
        &router,
        "CreateSchema",
        &json!({
            "RegistryId": { "RegistryName": "payments-registry" },
            "SchemaName": "payments",
            "DataFormat": "AVRO",
            "SchemaDefinition": "{\"type\":\"record\",\"name\":\"r\",\"fields\":[]}",
        }),
    )
    .await;

    // An incomplete reference is a bad parameter, not a miss — whichever half it carries.
    for (case, schema_id) in [
        ("neither half", json!({})),
        ("a schema name alone", json!({ "SchemaName": "payments" })),
        (
            "a registry name alone",
            json!({ "RegistryName": "payments-registry" }),
        ),
    ] {
        assert_refusal(
            &router,
            case,
            "GetSchema",
            &json!({ "SchemaId": schema_id }),
            INVALID_INPUT,
            SCHEMA_ID_INCOMPLETE,
        )
        .await;
    }

    // A complete reference into a registry that does not exist reports the REGISTRY, named
    // as the request spelled it; one into a registry that does exist reports the schema.
    for (case, registry) in [
        ("a registry nothing created", "never-created-registry"),
        ("a registry name in another casing", "Payments-Registry"),
    ] {
        assert_refusal(
            &router,
            case,
            "GetSchema",
            &json!({ "SchemaId": { "RegistryName": registry, "SchemaName": "payments" } }),
            ENTITY_NOT_FOUND,
            &format!("Registry is not found. RegistryName: {registry}"),
        )
        .await;
    }
    for (case, schema) in [
        ("a schema name the registry does not hold", "absent"),
        ("a schema name in another casing", "Payments"),
    ] {
        assert_refusal(
            &router,
            case,
            "GetSchema",
            &json!({
                "SchemaId": { "RegistryName": "payments-registry", "SchemaName": schema },
            }),
            ENTITY_NOT_FOUND,
            SCHEMA_NOT_FOUND,
        )
        .await;
    }

    // And both complete forms of the reference serve the schema that IS stored, so the
    // refusals above are about the reference and not about the read.
    let arn = format!(
        "arn:aws:glue:{REGION}:{ACCOUNT_ID}:schema/payments-registry/payments"
    );
    for (case, schema_id) in [
        (
            "by registry and schema name",
            json!({ "RegistryName": "payments-registry", "SchemaName": "payments" }),
        ),
        ("by schema ARN", json!({ "SchemaArn": arn })),
    ] {
        let served = ok(&router, "GetSchema", &json!({ "SchemaId": schema_id })).await;
        assert_eq!(
            served["SchemaName"].as_str(),
            Some("payments"),
            "case {case}: the stored schema must be served, got {served}"
        );
    }
}
