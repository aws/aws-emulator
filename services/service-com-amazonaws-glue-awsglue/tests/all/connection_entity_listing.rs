// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the ENTITY listing of a Glue connection (`ListEntities`): what the
//! data source behind a connection holds, and how a request that names no source, no
//! connection or no position is refused.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, and Amazon
//! DynamoDB is mounted beside Glue on ONE engine, because the entities of a connection of
//! that type ARE the tables that service holds: what a suite here stages with DynamoDB's own
//! `CreateTable` is what the listing under test must report.
//!
//! The rules under test, each a table whose rows differ along what the rule generalises over
//! (the objects the source holds, their number, their names, the connection's type, the
//! position asked for):
//!
//! 1. The entities of a connection are the objects its SOURCE holds, read through that
//!    source's own listing: an object created there appears, one deleted there is gone, and
//!    each is reported with the name the source holds it under, as a leaf entity of its
//!    category.
//! 2. One answer is one SEGMENT of that listing, and its `NextToken` resumes exactly where
//!    the segment stopped — a token this store did not mint for THIS connection's listing
//!    naming no position at all.
//! 3. A `ConnectionName` the addressed Data Catalog holds no connection under is the
//!    connection read's own miss, and a request naming no connection names no data source.
//! 4. A connection whose source is outside this emulator has no connector here, so the read
//!    reports a federation failure rather than an empty page.
//! 5. `ParentEntityName` asks for the children of ONE entity: an entity the source holds has
//!    none (no entity of this source is a parent entity), and a name it does not hold is a
//!    miss.
//! 6. `DataStoreApiVersion` names a version of the API a SaaS connector calls, and no source
//!    served here is reached through a SaaS API, so the member selects between nothing.
//!
//! The expected sentences are written out here as literals rather than read from the
//! service's own constants, so this suite pins the wording instead of restating it.

use std::sync::Arc;

use aws_sdk_dynamodb::types::{
    AttributeDefinition, BillingMode, KeySchemaElement, KeyType, ScalarAttributeType,
};
use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::state::ENTITIES_PER_PAGE;
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{call, error_code, error_message, ok, ACCOUNT_ID, REGION};

/// The connection type whose data source this emulator serves itself: Amazon DynamoDB, one
/// of the modelled `ConnectionType` values.
const DYNAMODB: &str = "DYNAMODB";

/// The `Entity$Category` of an entity that is a table of its source.
const TABLES_CATEGORY: &str = "tables";

/// An emulator with Glue AND Amazon DynamoDB mounted on one shared engine: the entities of a
/// DynamoDB connection are the tables that service holds, so without it mounted there is no
/// source to read.
fn emulator_with_dynamodb() -> MultiServiceRouter {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_glue_awsglue::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_dynamodb_dynamodb_20120810::EmulatedService::new(Arc::clone(&engine)),
    );
    router
}

/// An Amazon DynamoDB client over this emulator's own in-process callback — the seam the
/// entity read takes, so what a suite stages here is what that read reports.
fn dynamodb(router: &MultiServiceRouter) -> aws_sdk_dynamodb::Client {
    let callback = smithy_server_runtime::EmbeddedEmulator::from_router(router.clone());
    let config = aws_sdk_dynamodb::Config::builder()
        .behavior_version(aws_sdk_dynamodb::config::BehaviorVersion::latest())
        .region(aws_sdk_dynamodb::config::Region::new(REGION))
        .endpoint_url(format!("https://dynamodb.{REGION}.amazonaws.com"))
        .credentials_provider(callback.aws_credentials_provider())
        .http_client(callback.http_client())
        .retry_config(aws_sdk_dynamodb::config::retry::RetryConfig::disabled())
        .timeout_config(aws_sdk_dynamodb::config::timeout::TimeoutConfig::disabled())
        .identity_cache(aws_sdk_dynamodb::config::IdentityCache::no_cache())
        .stalled_stream_protection(
            aws_sdk_dynamodb::config::StalledStreamProtectionConfig::disabled(),
        )
        .build();
    aws_sdk_dynamodb::Client::from_conf(config)
}

/// Create the DynamoDB table `name`, with the one key attribute a table needs.
///
/// The create is not waited on: a table the store holds is listed whatever status it is in,
/// and the listing under test is of the names that store answers with.
async fn given_table(client: &aws_sdk_dynamodb::Client, name: &str) {
    client
        .create_table()
        .table_name(name)
        .billing_mode(BillingMode::PayPerRequest)
        .key_schema(
            KeySchemaElement::builder()
                .attribute_name("id")
                .key_type(KeyType::Hash)
                .build()
                .expect("the key schema element is complete"),
        )
        .attribute_definitions(
            AttributeDefinition::builder()
                .attribute_name("id")
                .attribute_type(ScalarAttributeType::S)
                .build()
                .expect("the attribute definition is complete"),
        )
        .send()
        .await
        .unwrap_or_else(|error| panic!("the table {name} must be created, got {error:?}"));
}

/// Create the Glue connection `name` of `connection_type`.
async fn given_connection(router: &MultiServiceRouter, name: &str, connection_type: &str) {
    ok(
        router,
        "CreateConnection",
        &json!({
            "ConnectionInput": {
                "Name": name,
                "ConnectionType": connection_type,
                "ConnectionProperties": {},
            },
        }),
    )
    .await;
}

/// The `EntityName` of every entity of an answer, in the order the service reported them.
fn entity_names(body: &Value) -> Vec<String> {
    body.get("Entities")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("the answer must report an Entities list, got {body}"))
        .iter()
        .map(|entity| {
            entity
                .get("EntityName")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("every entity carries an EntityName, got {entity}"))
                .to_string()
        })
        .collect()
}

/// Rule 1: the entities of a connection are the objects its source holds, each reported with
/// the name that source holds it under, as a leaf entity of its category — and the listing
/// follows the source, so an object created there appears and one deleted there is gone.
///
/// The rows differ along how many objects the source holds and how their names are spelled,
/// including the punctuation the store admits and the order they were created in.
#[tokio::test]
async fn the_entities_of_a_connection_are_the_objects_its_source_holds() {
    let rows: Vec<(&str, Vec<&str>)> = vec![
        ("a source holding nothing", vec![]),
        ("one object", vec!["orders"]),
        (
            "several objects, created out of order",
            vec!["zeta_stream", "alpha_events", "mid.point"],
        ),
        (
            "names carrying the punctuation the store admits",
            vec!["team.lake-2026", "raw_data.v1", "a-b-c"],
        ),
    ];

    for (case, tables) in rows {
        let router = emulator_with_dynamodb();
        let client = dynamodb(&router);
        for table in &tables {
            given_table(&client, table).await;
        }
        given_connection(&router, "store", DYNAMODB).await;

        let mut expected: Vec<String> = tables.iter().map(|name| (*name).to_string()).collect();
        expected.sort();

        let listed = ok(&router, "ListEntities", &json!({ "ConnectionName": "store" })).await;
        assert_eq!(
            entity_names(&listed),
            expected,
            "case {case}: the entities must be the objects the source holds, got {listed}"
        );
        assert!(
            listed.get("NextToken").is_none(),
            "case {case}: a listing that reached the end of the source carries no NextToken, \
             got {listed}"
        );
        for entity in listed["Entities"].as_array().expect("Entities is a list") {
            let name = entity["EntityName"].as_str().expect("EntityName is text");
            assert_eq!(
                entity.get("Label").and_then(Value::as_str),
                Some(name),
                "case {case}: the entity is labelled with the name its source holds it under, \
                 got {entity}"
            );
            assert_eq!(
                entity.get("IsParentEntity").and_then(Value::as_bool),
                Some(false),
                "case {case}: a table of the source has nothing listed beneath it, got {entity}"
            );
            assert_eq!(
                entity.get("Category").and_then(Value::as_str),
                Some(TABLES_CATEGORY),
                "case {case}: the entity's category is the kind of object it is, got {entity}"
            );
        }

        // The listing FOLLOWS the source: an object created there after the first read is
        // reported by the next one, so what is reported is read from the source on each call
        // rather than kept here.
        given_table(&client, "added_later").await;
        expected.push("added_later".to_string());
        expected.sort();
        let after_create = ok(&router, "ListEntities", &json!({ "ConnectionName": "store" })).await;
        assert_eq!(
            entity_names(&after_create),
            expected,
            "case {case}: an object created in the source must be reported, got {after_create}"
        );
    }
}

/// Rule 2: one answer is one segment of the source's listing, and its `NextToken` resumes
/// exactly where that segment stopped — while a token this store did not mint for THIS
/// connection's listing names no position.
///
/// The source is staged with two objects more than one segment carries, so the walk takes
/// more than one page whatever the segment size is.
#[tokio::test]
async fn an_entity_page_resumes_where_the_previous_page_stopped() {
    let page_size = usize::try_from(ENTITIES_PER_PAGE).expect("the segment size is positive");
    let router = emulator_with_dynamodb();
    let client = dynamodb(&router);
    let mut expected: Vec<String> = Vec::new();
    for index in 0..page_size + 2 {
        let name = format!("entity-{index:04}");
        given_table(&client, &name).await;
        expected.push(name);
    }
    expected.sort();
    given_connection(&router, "store", DYNAMODB).await;
    given_connection(&router, "other-store", DYNAMODB).await;

    let first = ok(&router, "ListEntities", &json!({ "ConnectionName": "store" })).await;
    assert_eq!(
        entity_names(&first).len(),
        page_size,
        "the first segment carries one page of the source's listing, got {first}"
    );
    let token = first
        .get("NextToken")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("a segment that is not the last carries a NextToken, got {first}"))
        .to_string();

    // A token minted for ONE connection's listing names no position in another's, even when
    // both read the same source.
    let (status, refused) = call(
        &router,
        "ListEntities",
        &json!({ "ConnectionName": "other-store", "NextToken": token }),
    )
    .await;
    assert_eq!(
        status, 400,
        "a token minted for another connection's listing must be refused, got {refused}"
    );
    assert_eq!(
        error_message(&refused),
        "Invalid pagination token.",
        "expected the pagination refusal, got {refused}"
    );

    let second = ok(
        &router,
        "ListEntities",
        &json!({ "ConnectionName": "store", "NextToken": token }),
    )
    .await;
    assert!(
        second.get("NextToken").is_none(),
        "the last segment carries no NextToken, got {second}"
    );

    let mut walked = entity_names(&first);
    walked.extend(entity_names(&second));
    assert_eq!(
        walked, expected,
        "the segments must walk the source's objects once each, in its own order"
    );
}

/// Rule 2's refusal, on its own: a `NextToken` this store did not mint names no position and
/// is refused rather than resolved against a name that happens to sort somewhere.
///
/// The rows differ along the FORM a caller-assembled token takes, each inside the member's
/// own pattern (`NextToken`, `^[-a-zA-Z0-9+=/:_]*$`).
#[tokio::test]
async fn a_continuation_token_this_store_did_not_mint_names_no_position() {
    let router = emulator_with_dynamodb();
    let client = dynamodb(&router);
    given_table(&client, "orders").await;
    given_connection(&router, "store", DYNAMODB).await;

    for (case, token) in [
        ("a plain object name", "orders"),
        ("not the hexadecimal of a token", "zzzz"),
        ("a token-shaped run of zeroes", "00000000000000000000000000000000"),
        ("a base64-looking value", "b3JkZXJzOm5leHQ="),
    ] {
        let (status, refused) = call(
            &router,
            "ListEntities",
            &json!({ "ConnectionName": "store", "NextToken": token }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400, got {status} with {refused}"
        );
        assert!(
            error_code(&refused).ends_with("InvalidInputException"),
            "case {case}: expected InvalidInputException, got {}",
            error_code(&refused)
        );
        assert_eq!(
            error_message(&refused),
            "Invalid pagination token.",
            "case {case}: expected the pagination refusal, got {refused}"
        );
    }
}

/// Rule 3: a `ConnectionName` the addressed Data Catalog holds no connection under earns the
/// connection read's own miss — the account-scoped sentence, with the name as the request
/// spelled it — and a request naming no connection at all names no data source.
///
/// The rows differ along how the name misses (nothing created, another casing, created then
/// deleted) and how it is spelled, plus a `CatalogId` this account holds no catalog under,
/// which is resolved before any connection is looked up.
#[tokio::test]
async fn a_connection_the_catalog_does_not_hold_is_refused_as_a_missing_entity() {
    let router = emulator_with_dynamodb();
    let client = dynamodb(&router);
    given_table(&client, "orders").await;
    given_connection(&router, "store", DYNAMODB).await;
    given_connection(&router, "deleted-store", DYNAMODB).await;
    ok(
        &router,
        "DeleteConnection",
        &json!({ "ConnectionName": "deleted-store" }),
    )
    .await;

    for (case, name) in [
        ("a name nothing created", "never-created".to_string()),
        ("a stored connection in another casing", "STORE".to_string()),
        ("created then deleted", "deleted-store".to_string()),
        ("at the 255 ceiling", "n".repeat(255)),
    ] {
        let (status, refused) = call(
            &router,
            "ListEntities",
            &json!({ "ConnectionName": name }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400, got {status} with {refused}"
        );
        assert!(
            error_code(&refused).ends_with("EntityNotFoundException"),
            "case {case}: expected EntityNotFoundException, got {}",
            error_code(&refused)
        );
        assert_eq!(
            error_message(&refused),
            format!("Connection with name {name} in account {ACCOUNT_ID} is not found"),
            "case {case}: expected the connection miss, got {refused}"
        );
    }

    // A request that names no connection names no data source.
    let (status, refused) = call(&router, "ListEntities", &json!({})).await;
    assert_eq!(
        status, 400,
        "a request naming no connection must be refused, got {refused}"
    );
    assert_eq!(
        error_message(&refused),
        "ConnectionName must be provided in the request.",
        "expected the missing-connection refusal, got {refused}"
    );

    // The catalog is resolved first, so an id this account holds no catalog under is the
    // catalog's own miss and never the connection's.
    let (status, refused) = call(
        &router,
        "ListEntities",
        &json!({
            "ConnectionName": "store",
            "CatalogId": format!("{ACCOUNT_ID}:never-created"),
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "a catalog nothing created must be refused, got {refused}"
    );
    assert_eq!(
        error_message(&refused),
        "Catalog not found.",
        "expected the catalog miss, got {refused}"
    );

    // And the connection that IS stored is still served, so the refusals above are misses
    // and not a broken lookup.
    let listed = ok(&router, "ListEntities", &json!({ "ConnectionName": "store" })).await;
    assert_eq!(
        entity_names(&listed),
        vec!["orders".to_string()],
        "the stored connection must still be read, got {listed}"
    );
}

/// Rule 4: a connection whose data source is outside this emulator has no connector here, so
/// the read reports the federation failure it is rather than an empty page of entities.
///
/// The rows span the connectivity kinds the modelled `ConnectionType` enum carries whose
/// sources are external: a SaaS source read over its own API, relational engines reached by
/// host, a generic JDBC endpoint, a file transfer endpoint, a record stream and a VPC path
/// that carries no data source at all.
#[tokio::test]
async fn a_connection_whose_source_is_outside_this_emulator_reports_a_federation_failure() {
    let router = emulator_with_dynamodb();
    let client = dynamodb(&router);
    // An object the source this emulator DOES serve holds, so what the rows below report is
    // about their own sources and not about an empty store.
    given_table(&client, "orders").await;

    for connection_type in [
        "SALESFORCE",
        "MYSQL",
        "POSTGRESQL",
        "JDBC",
        "SFTP",
        "KAFKA",
        "NETWORK",
        "BIGQUERY",
    ] {
        let name = format!("conn-{connection_type}");
        given_connection(&router, &name, connection_type).await;
        let (status, refused) = call(
            &router,
            "ListEntities",
            &json!({ "ConnectionName": name }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {connection_type}: expected 400, got {status} with {refused}"
        );
        assert!(
            error_code(&refused).ends_with("FederationSourceException"),
            "case {connection_type}: expected FederationSourceException, got {}",
            error_code(&refused)
        );
        assert_eq!(
            error_message(&refused),
            format!("Unable to connect to the {connection_type} data source."),
            "case {connection_type}: expected the unreached-source report, got {refused}"
        );
        assert_eq!(
            refused
                .get("FederationSourceErrorCode")
                .and_then(Value::as_str),
            Some("OperationNotSupportedException"),
            "case {connection_type}: the federation failure states that there is no such read \
             to make, got {refused}"
        );
    }
}

/// Rule 5: `ParentEntityName` asks for the children of ONE entity. No entity of this source
/// is a parent entity, so an entity it holds has no children to list, and a name it does not
/// hold is a miss naming that name.
///
/// The rows differ along whether the name is an object of the source and how it is spelled.
#[tokio::test]
async fn a_parent_entity_name_lists_the_children_of_one_entity() {
    let router = emulator_with_dynamodb();
    let client = dynamodb(&router);
    for table in ["orders", "team.lake-2026"] {
        given_table(&client, table).await;
    }
    given_connection(&router, "store", DYNAMODB).await;

    for parent in ["orders", "team.lake-2026"] {
        let listed = ok(
            &router,
            "ListEntities",
            &json!({ "ConnectionName": "store", "ParentEntityName": parent }),
        )
        .await;
        assert_eq!(
            entity_names(&listed),
            Vec::<String>::new(),
            "case {parent}: an entity of this source has no child entities, got {listed}"
        );
        assert!(
            listed.get("NextToken").is_none(),
            "case {parent}: there is no further segment of an empty listing, got {listed}"
        );
    }

    for (case, parent) in [
        ("a name nothing created", "never-created"),
        ("an object of the source in another casing", "ORDERS"),
        ("a name close to one the source holds", "orders.items"),
    ] {
        let (status, refused) = call(
            &router,
            "ListEntities",
            &json!({ "ConnectionName": "store", "ParentEntityName": parent }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400, got {status} with {refused}"
        );
        assert!(
            error_code(&refused).ends_with("EntityNotFoundException"),
            "case {case}: expected EntityNotFoundException, got {}",
            error_code(&refused)
        );
        assert_eq!(
            error_message(&refused),
            format!("Entity {parent} not found."),
            "case {case}: expected the entity miss, got {refused}"
        );
    }
}

/// Rule 6: `DataStoreApiVersion` names a version of the API a SaaS connector calls, and no
/// source served here is reached through a SaaS API — so the member selects between nothing
/// and a read of such a source reports the same objects whatever it carries, rather than
/// being refused for a version nothing publishes.
///
/// The rows span the forms the member admits (`ApiVersion`, length 1..256, pattern
/// `^[a-zA-Z0-9.-]*$`): a dated version, a dotted release number, a single digit and a value
/// at the member's documented ceiling.
#[tokio::test]
async fn a_data_store_api_version_selects_nothing_for_a_source_with_no_saas_api() {
    let router = emulator_with_dynamodb();
    let client = dynamodb(&router);
    given_table(&client, "orders").await;
    given_connection(&router, "store", DYNAMODB).await;

    let without = ok(&router, "ListEntities", &json!({ "ConnectionName": "store" })).await;
    assert_eq!(
        entity_names(&without),
        vec!["orders".to_string()],
        "the source's objects must be reported, got {without}"
    );

    for (case, version) in [
        ("a dated version", "2012-08-10".to_string()),
        ("another dated version", "2011-12-05".to_string()),
        ("a dotted release number", "1.0".to_string()),
        ("a single digit", "2".to_string()),
        ("at the 256 ceiling", "v".repeat(256)),
    ] {
        let served = ok(
            &router,
            "ListEntities",
            &json!({ "ConnectionName": "store", "DataStoreApiVersion": version }),
        )
        .await;
        assert_eq!(
            entity_names(&served),
            entity_names(&without),
            "case {case}: the member selects nothing for this source, got {served}"
        );
    }
}
