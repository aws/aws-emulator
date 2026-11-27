// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the one DISCRIMINATOR a client has between "the resource is not
//! there" and "your request is wrong", across the two resources an Athena control-plane
//! caller addresses by a single reference: a data catalog by its `Name` and a saved query by
//! its `NamedQueryId`. Driven over the wire (awsJson1_1,
//! `x-amz-target: AmazonAthena.<Operation>`) through the same router the emulator boots.
//!
//! THE RULE UNDER TEST: these operations model ONE client error — `InvalidRequestException`,
//! HTTP 400 — so every condition they refuse arrives in the same envelope and the refusal
//! CLASS, which the wire carries under both `AthenaErrorCode` and the `ErrorCode` alias, is
//! what tells the conditions apart. A reference the model ADMITS that resolves to no
//! resource this account holds is refused under the class of that resource's miss; the class
//! of a request the caller must correct is reserved for a reference the model EXCLUDES. The
//! two classes are therefore never the same value for the same operation, which is what lets
//! an idempotent teardown treat a miss as "already gone" and converge instead of retrying a
//! request it is told to correct.
//!
//! Asserted over a TABLE rather than over one request: the rows span the two resource kinds,
//! every operation that addresses one of them by a single reference, the FORM a reference
//! takes (a UUID and a non-UUID token, one character and the longest the constraint allows,
//! each documented punctuation character, mixed case with digits) and HOW the reference came
//! to resolve to nothing (never created, or taken by a delete).
//!
//! Sources: `smithy_query(query="operation",operation="DeleteDataCatalog,GetNamedQuery,UpdateNamedQuery,DeleteNamedQuery")`
//! for each operation's input and for `InvalidRequestException` (HTTP 400) being the only
//! client error any of them models, and the `@length`/`@pattern` on `CatalogNameString`
//! (1..256, excluding the control characters below `U+0020` other than the tab) and on
//! `NamedQueryId` (1..128, `^\S+$`) for the excluded rows below.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_athena_amazonathena::state::{
    DATA_CATALOG_TYPE_GLUE, ERROR_CODE_DATA_CATALOG_NOT_FOUND, ERROR_CODE_INVALID_INPUT,
    ERROR_CODE_NAMED_QUERY_NOT_FOUND, MAX_DATA_CATALOG_NAME_CHARS,
};

use crate::harness::{assert_refusal, call, error_code, message, router};

/// The upper bound of the modelled `NamedQueryId` `@length` (1 to 128 characters).
const MAX_NAMED_QUERY_ID_CHARS: usize = 128;

/// The upper bound of the modelled `CatalogNameString` `@length` (1 to 256 characters).
const MAX_CATALOG_NAME_CHARS: usize = 256;

/// The `catalog-id` a GLUE catalog is created with, so a row the teardown test creates is
/// never refused for its parameters instead of its name.
const CATALOG_ID: &str = "000000000000";

/// One operation that addresses a resource by a single reference: the request it is handed
/// for that reference, well formed in every other member, so a row exercises the reference
/// resolving to nothing and nothing else.
struct Addressing {
    case: &'static str,
    operation: &'static str,
    body: fn(&str) -> Value,
}

/// One resource kind, with the class and the sentence a miss of ONE of its references is
/// stated in, the operations that address it, and the references each test drives.
struct ResourceKind {
    case: &'static str,
    /// The class a reference this account holds no resource under is refused under.
    not_found_code: &'static str,
    /// Athena's sentence for that miss. Spelled out by the rows below rather than taken
    /// from the service: the fixed text is this function's literal and the reference the
    /// CALLER carried is its one variable part.
    not_found_message: fn(&str) -> String,
    addressing: Vec<Addressing>,
    /// References the model admits that resolve to no resource.
    admitted: Vec<(&'static str, String)>,
    /// References the model excludes, which are the caller's own request to correct.
    excluded: Vec<(&'static str, String)>,
}

/// A reference of `chars` characters of non-whitespace, so a row can sit exactly on a
/// published bound of a reference's `@length` — or one past it — without quoting any
/// identifier a run minted.
fn reference_of_length(chars: usize) -> String {
    "r".repeat(chars)
}

/// The two resource kinds and everything each test drives for them.
fn resource_kinds() -> Vec<ResourceKind> {
    vec![
        ResourceKind {
            case: "a data catalog addressed by its Name",
            not_found_code: ERROR_CODE_DATA_CATALOG_NOT_FOUND,
            not_found_message: |name| format!("DataCatalog {name} was not found"),
            addressing: vec![Addressing {
                case: "DeleteDataCatalog, which removes it",
                operation: "DeleteDataCatalog",
                body: |name| json!({ "Name": name }),
            }],
            // Names inside the documented character set — "alphanumeric, underscore, at
            // sign, or hyphen characters" — that no create registered, differing in length
            // and in which of those characters they use.
            admitted: vec![
                ("a name of one character", "c".to_string()),
                ("a name with hyphens", "orders-eu-west-1".to_string()),
                ("a name with underscores", "orders_catalog_2".to_string()),
                ("a name with at signs", "orders@eu@west@1".to_string()),
                ("a name mixing case and digits", "Orders9Catalog".to_string()),
                (
                    "the longest name the documented bound allows",
                    reference_of_length(MAX_DATA_CATALOG_NAME_CHARS),
                ),
            ],
            // What the modelled `@pattern` on `CatalogNameString` excludes (a control
            // character below `U+0020` other than the tab, and the two non-characters at
            // the end of the basic plane) and what its `@length` excludes.
            excluded: vec![
                ("a name carrying a control character", "orders\u{1}catalog".to_string()),
                ("a name carrying a NUL", "orders\u{0}catalog".to_string()),
                ("a name carrying a newline", "orders\ncatalog".to_string()),
                (
                    "a name carrying the non-character U+FFFF",
                    "orders\u{ffff}catalog".to_string(),
                ),
                (
                    "a name one character past the modelled maximum",
                    reference_of_length(MAX_CATALOG_NAME_CHARS + 1),
                ),
                ("a name of no characters at all", String::new()),
            ],
        },
        ResourceKind {
            case: "a saved query addressed by its NamedQueryId",
            not_found_code: ERROR_CODE_NAMED_QUERY_NOT_FOUND,
            not_found_message: |id| format!("NamedQuery: {id} does not exist"),
            addressing: vec![
                Addressing {
                    case: "GetNamedQuery, which reads it",
                    operation: "GetNamedQuery",
                    body: |id| json!({ "NamedQueryId": id }),
                },
                Addressing {
                    case: "UpdateNamedQuery, which revises it",
                    operation: "UpdateNamedQuery",
                    body: |id| {
                        json!({
                            "NamedQueryId": id,
                            "Name": "revised-by-a-refused-update",
                            "QueryString": "SELECT 1 FROM events",
                        })
                    },
                },
                Addressing {
                    case: "DeleteNamedQuery, which takes it",
                    operation: "DeleteNamedQuery",
                    body: |id| json!({ "NamedQueryId": id }),
                },
            ],
            // Ids inside the modelled `NamedQueryId` that no create minted, differing in
            // form and sitting on each published bound.
            admitted: vec![
                (
                    "a UUID no create minted",
                    "11111111-2222-4333-8444-555555555555".to_string(),
                ),
                ("an id that is no UUID at all", "nq_42".to_string()),
                ("the shortest id the model allows", reference_of_length(1)),
                (
                    "the longest id the model allows",
                    reference_of_length(MAX_NAMED_QUERY_ID_CHARS),
                ),
            ],
            // What the modelled `@pattern` `^\S+$` and `@length` 1..128 exclude.
            excluded: vec![
                ("an id carrying a space", "saved query".to_string()),
                ("an id carrying a tab", "saved\tquery".to_string()),
                (
                    "an id one character past the modelled maximum",
                    reference_of_length(MAX_NAMED_QUERY_ID_CHARS + 1),
                ),
                ("an id of no characters at all", String::new()),
            ],
        },
    ]
}

/// RULE: a reference the model admits that resolves to no resource is refused under the
/// class of THAT resource's miss, in the one sentence Athena states the miss in.
#[tokio::test]
async fn an_admitted_reference_to_nothing_is_refused_under_the_class_of_the_miss() {
    let router = router();

    for kind in resource_kinds() {
        for (reference_case, reference) in &kind.admitted {
            for addressing in &kind.addressing {
                let request = (addressing.body)(reference);
                let (status, body) = call(&router, addressing.operation, &request).await;
                let case = format!("{} / {reference_case} / {}", kind.case, addressing.case);
                assert_refusal(
                    &case,
                    status,
                    &body,
                    kind.not_found_code,
                    &(kind.not_found_message)(reference),
                );
            }
        }
    }
}

/// RULE: the class of a request the caller must correct is reserved for a reference the
/// model EXCLUDES — so for the same operation it is never the class a miss carries, and a
/// client branching on the class can tell the two conditions apart.
#[tokio::test]
async fn an_excluded_reference_is_refused_under_a_different_class_from_a_miss() {
    let router = router();

    for kind in resource_kinds() {
        for (reference_case, reference) in &kind.excluded {
            for addressing in &kind.addressing {
                let request = (addressing.body)(reference);
                let (status, body) = call(&router, addressing.operation, &request).await;
                let case = format!("{} / {reference_case} / {}", kind.case, addressing.case);
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected HTTP 400, got {status} with {body:?}"
                );
                assert_eq!(
                    error_code(&body),
                    "InvalidRequestException",
                    "case {case:?}: expected the one client error the operation models, got \
                     {body:?}"
                );
                for member in ["AthenaErrorCode", "ErrorCode"] {
                    assert_eq!(
                        body[member].as_str(),
                        Some(ERROR_CODE_INVALID_INPUT),
                        "case {case:?}: expected {member} {ERROR_CODE_INVALID_INPUT:?}, got \
                         {body:?}"
                    );
                    assert_ne!(
                        body[member].as_str(),
                        Some(kind.not_found_code),
                        "case {case:?}: {member} must not carry the class of a miss \
                         ({:?}), got {body:?}",
                        kind.not_found_code
                    );
                }
            }
        }
    }
}

/// Register a GLUE data catalog called `name`.
async fn create_catalog(router: &MultiServiceRouter, name: &str) {
    let (status, created) = call(
        router,
        "CreateDataCatalog",
        &json!({
            "Name": name,
            "Type": DATA_CATALOG_TYPE_GLUE,
            "Parameters": { "catalog-id": CATALOG_ID },
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the catalog {name} must be registered, got {status} with {created:?}"
    );
}

/// Save one query and answer with the `NamedQueryId` the create minted.
async fn create_named_query(router: &MultiServiceRouter, name: &str, query_string: &str) -> String {
    let (status, created) = call(
        router,
        "CreateNamedQuery",
        &json!({
            "Name": name,
            "Database": "logs",
            "QueryString": query_string,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the saved query {name} must be created, got {status} with {created:?}"
    );
    created["NamedQueryId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateNamedQuery must answer with an id, got {created:?}"))
        .to_string()
}

/// RULE applied to the loop it exists for: a teardown that deletes a resource twice is told
/// the second time that the resource is not there — the class of the miss rather than the
/// class of a request to correct — so it converges, and the resources it did not name are
/// untouched by either refusal.
#[tokio::test]
async fn a_repeated_delete_reports_the_resource_gone_rather_than_the_request_wrong() {
    let router = router();

    // Catalog names differing in the documented characters they use, each registered, taken
    // once, then named by a delete that finds nothing.
    let taken_catalogs = [
        "teardown-catalog",
        "teardown_catalog_2",
        "Teardown9Catalog",
        "teardown@catalog",
    ];
    let standing_catalog = "left-standing-catalog";
    create_catalog(&router, standing_catalog).await;

    for name in taken_catalogs {
        create_catalog(&router, name).await;
        let (status, removed) = call(&router, "DeleteDataCatalog", &json!({ "Name": name })).await;
        assert_eq!(
            status, 200,
            "case {name:?}: the first delete must be served, got {status} with {removed:?}"
        );

        // The second delete of the same name, and a third: a miss is a standing fact about
        // this account's state, so it is stated the same way every time it is asked.
        for attempt in ["the second delete", "the third delete"] {
            let (status, body) = call(&router, "DeleteDataCatalog", &json!({ "Name": name })).await;
            assert_refusal(
                &format!("{attempt} of {name:?}"),
                status,
                &body,
                ERROR_CODE_DATA_CATALOG_NOT_FOUND,
                &format!("DataCatalog {name} was not found"),
            );
        }
    }

    // The catalog no delete named still reads back: none of the refusals above touched it.
    let (status, read) = call(
        &router,
        "GetDataCatalog",
        &json!({ "Name": standing_catalog }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the catalog {standing_catalog} must still read back, got {status} with {read:?}"
    );

    // The same loop over saved queries: rows differ along the statement each query holds,
    // so what is proved is the delete of a record rather than of one recorded id.
    let standing_query = create_named_query(&router, "left-standing-query", "SELECT 0").await;
    let statements = [
        "SELECT 1 FROM events",
        "SELECT count(*) FROM events WHERE kind = 'click'",
        "SELECT 1 FROM events; SELECT 2 FROM events",
    ];
    for (position, statement) in statements.iter().enumerate() {
        let name = format!("teardown-query-{position}");
        let id = create_named_query(&router, &name, statement).await;
        let (status, removed) =
            call(&router, "DeleteNamedQuery", &json!({ "NamedQueryId": id })).await;
        assert_eq!(
            status, 200,
            "case {statement:?}: the first delete must be served, got {status} with {removed:?}"
        );

        let expected = format!("NamedQuery: {id} does not exist");
        for attempt in ["the second delete", "the third delete"] {
            let (status, body) =
                call(&router, "DeleteNamedQuery", &json!({ "NamedQueryId": id })).await;
            assert_refusal(
                &format!("{attempt} of {id:?}"),
                status,
                &body,
                ERROR_CODE_NAMED_QUERY_NOT_FOUND,
                &expected,
            );
        }

        // And the read of the taken id states the same class and the same sentence: one id
        // resolving to nothing is one fact, however the request asked about it.
        let (status, body) = call(&router, "GetNamedQuery", &json!({ "NamedQueryId": id })).await;
        assert_refusal(
            &format!("the read of the taken id {id:?}"),
            status,
            &body,
            ERROR_CODE_NAMED_QUERY_NOT_FOUND,
            &expected,
        );
        assert_eq!(
            message(&body),
            expected,
            "case {statement:?}: expected the one wording of a saved query's miss, got {body:?}"
        );
    }

    // The saved query no delete named still reads back, with the statement it was created
    // under: the refusals above wrote nothing.
    let (status, body) = call(
        &router,
        "GetNamedQuery",
        &json!({ "NamedQueryId": standing_query }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the saved query {standing_query} must still read back, got {status} with {body:?}"
    );
    assert_eq!(
        body["NamedQuery"]["QueryString"].as_str(),
        Some("SELECT 0"),
        "the standing saved query must still hold the statement it was created under, got \
         {body:?}"
    );
}
