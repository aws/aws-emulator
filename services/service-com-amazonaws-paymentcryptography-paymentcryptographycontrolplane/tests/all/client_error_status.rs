// SPDX-License-Identifier: CC0-1.0

//! The HTTP STATUS this service answers a refused request with, end to end over the
//! wire.
//!
//! The rule under test: this service's wire protocol is `awsJson1_0`, which reports
//! WHICH error a caller hit in the response body's `__type` and not in the status line,
//! so every modelled CLIENT error is answered with one status — HTTP 400 — whatever
//! `@httpError` its own shape declares. A `@httpError` on a client error shape of this
//! model classifies the error; it is not the status line the service writes.
//!
//! Each test below is a table spanning the class the rule generalises over rather than
//! the one request that exposed it: every client-error class this service's operations
//! can reach (its shape declaring 400, 402, 404 and 409 in turn), every identifier form
//! `KeyArnOrKeyAliasType` admits, and several operations per class — the status is a
//! property of the service's error channel, so it cannot be right for one operation and
//! wrong for the next.
//!
//! Every call goes through the real `MultiServiceRouter` (`POST /` with
//! `X-Amz-Target: PaymentCryptographyControlPlane.<Op>`, awsJson1_0) and the
//! modelled-constraint walk, through the same harness `key_lifecycle` uses, so what
//! these tests assert is what a signed SDK request observes.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_paymentcryptography_paymentcryptographycontrolplane::state::{
    MAX_ENABLED_REPLICATION_REGIONS, MAX_KEY_REPLICATION_REGIONS,
};

use crate::key_lifecycle::{call, error_code, ok_call, router, string, REGION};

/// The status every client error of this service is answered with.
const CLIENT_ERROR_STATUS: u16 = 400;

/// Each modelled client-error class of this service, with the `@httpError` its own
/// shape declares.
///
/// Source: `smithy_query(query="shape",shape="ValidationException,
/// ServiceQuotaExceededException,AccessDeniedException,ResourceNotFoundException,
/// ConflictException,ThrottlingException")` — every one is `errorKind` `client`, and
/// the declared statuses are 400, 402, 403, 404, 409 and 429 respectively. The classes
/// a request below can reach are the four marked reachable; the two that are not are
/// listed so the catalog is the model's and not just the tests'.
const CLIENT_ERROR_CLASSES: &[(&str, u16, bool)] = &[
    ("ValidationException", 400, true),
    ("ServiceQuotaExceededException", 402, true),
    ("AccessDeniedException", 403, false),
    ("ResourceNotFoundException", 404, true),
    ("ConflictException", 409, true),
    ("ThrottlingException", 429, false),
];

/// The `@httpError` the named class's shape declares, from [`CLIENT_ERROR_CLASSES`].
fn declared_status(class: &str) -> u16 {
    CLIENT_ERROR_CLASSES
        .iter()
        .find(|(name, _, _)| *name == class)
        .map(|(_, declared, _)| *declared)
        .unwrap_or_else(|| panic!("{class} is not a modelled client error of this service"))
}

/// Assert a refusal answered `class` under this service's one client-error status.
///
/// Also asserts the answered status is NOT the status the class's shape declares
/// whenever those differ, which is the whole point of the rule: the shape's
/// `@httpError` classifies the error and the protocol writes the status line.
fn assert_client_refusal(case: &str, status: u16, body: &Value, class: &str) {
    assert_eq!(
        status, CLIENT_ERROR_STATUS,
        "case {case}: a {class} must be answered with HTTP {CLIENT_ERROR_STATUS}, got \
         {status} {body}"
    );
    assert_eq!(
        error_code(body),
        class,
        "case {case}: expected the refusal to name {class} in __type, got {body}"
    );
    let declared = declared_status(class);
    if declared != CLIENT_ERROR_STATUS {
        assert_ne!(
            status, declared,
            "case {case}: the status must be this service's client-error status and not \
             {class}'s own @httpError {declared}, got {status} {body}"
        );
    }
}

/// A `CreateKey` body for one symmetric data-encryption key, the attribute set this
/// service's documentation pairs with `AES_128`.
fn key_body() -> Value {
    json!({
        "KeyAttributes": {
            "KeyUsage": "TR31_D0_SYMMETRIC_DATA_ENCRYPTION_KEY",
            "KeyClass": "SYMMETRIC_KEY",
            "KeyAlgorithm": "AES_128",
            "KeyModesOfUse": {"Encrypt": true, "Decrypt": true},
        },
        "Exportable": true,
    })
}

/// Create one key and answer its `KeyArn`.
async fn fresh_key_arn(router: &MultiServiceRouter) -> String {
    let created = ok_call(router, "CreateKey", &key_body()).await;
    string(created.get("Key").expect("CreateKey answers a Key"), "KeyArn").to_string()
}

/// Every identifier form `KeyArnOrKeyAliasType` admits, each naming nothing.
///
/// Source: `smithy_query(query="operation",operation="GetKey")` — `KeyIdentifier`
/// targets `KeyArnOrKeyAliasType`, whose pattern admits a key ARN, an alias ARN and a
/// bare `alias/<name>`. The names are this test's own, not the issue's.
fn absent_identifiers() -> Vec<(&'static str, String)> {
    let account = engine::emulator_account_id();
    vec![
        (
            "a key ARN",
            format!("arn:aws:payment-cryptography:{REGION}:{account}:key/neverminted00000"),
        ),
        (
            "an alias ARN",
            format!("arn:aws:payment-cryptography:{REGION}:{account}:alias/never-created"),
        ),
        ("a bare alias", "alias/never-created".to_string()),
        ("the longest bare alias", format!("alias/{}", "z".repeat(250))),
    ]
}

/// RULE: a request naming a resource the account does not hold is refused with
/// `ResourceNotFoundException`, whose shape declares `@httpError` 404, under this
/// service's one client-error status.
///
/// The table is every operation that resolves a `KeyIdentifier` on its own crossed with
/// every identifier form the member admits, so neither the operation nor the form of
/// the identifier can change the status line.
#[tokio::test]
async fn a_not_found_refusal_answers_the_client_error_status_for_every_operation_and_identifier_form(
) {
    let router = router();
    for operation in [
        "GetKey",
        "DeleteKey",
        "RestoreKey",
        "StartKeyUsage",
        "StopKeyUsage",
        "ListTagsForResource",
    ] {
        for (form, identifier) in absent_identifiers() {
            // `ListTagsForResource` takes a key ARN only, so the two alias forms are
            // not addressable through it.
            if operation == "ListTagsForResource" && !identifier.contains(":key/") {
                continue;
            }
            let case = format!("{operation} on {form}");
            let body = if operation == "ListTagsForResource" {
                json!({"ResourceArn": identifier})
            } else {
                json!({"KeyIdentifier": identifier})
            };
            let (status, parsed) = call(&router, operation, &body).await;
            assert_client_refusal(&case, status, &parsed, "ResourceNotFoundException");
        }
    }

    // An alias is reached by name rather than by identifier, and earns the same status.
    let (status, parsed) = call(
        &router,
        "GetAlias",
        &json!({"AliasName": "alias/never-created"}),
    )
    .await;
    assert_client_refusal(
        "GetAlias on a name never bound",
        status,
        &parsed,
        "ResourceNotFoundException",
    );
}

/// RULE: every client-error class this service reaches is answered with the one
/// client-error status, with the class carried in the body's `__type` — the status line
/// is the protocol's and the `@httpError` on the shape is the classification.
///
/// One row per reachable class of [`CLIENT_ERROR_CLASSES`], each reached through a
/// different operation, so the four distinct `@httpError` values the model declares
/// (400, 402, 404, 409) all arrive as one status.
#[tokio::test]
async fn every_reachable_client_error_class_answers_the_one_client_error_status() {
    let router = router();

    // ValidationException (@httpError 400): `CreateAlias`' own documentation says "You
    // can't create an alias without a key".
    let (status, parsed) = call(
        &router,
        "CreateAlias",
        &json!({"AliasName": "alias/no-key-at-all"}),
    )
    .await;
    assert_client_refusal(
        "CreateAlias without a key",
        status,
        &parsed,
        "ValidationException",
    );

    // ConflictException (@httpError 409): an alias name identifies one key, so binding
    // a taken name to another key is the inconsistent state the class describes.
    let first = fresh_key_arn(&router).await;
    let second = fresh_key_arn(&router).await;
    let taken = "alias/already-taken";
    ok_call(
        &router,
        "CreateAlias",
        &json!({"AliasName": taken, "KeyArn": first}),
    )
    .await;
    let (status, parsed) = call(
        &router,
        "CreateAlias",
        &json!({"AliasName": taken, "KeyArn": second}),
    )
    .await;
    assert_client_refusal(
        "CreateAlias onto a taken name",
        status,
        &parsed,
        "ConflictException",
    );

    // ConflictException again, reached by a second operation: a key already scheduled
    // for deletion cannot be scheduled for a different waiting period.
    ok_call(
        &router,
        "DeleteKey",
        &json!({"KeyIdentifier": second, "DeleteKeyInDays": 7}),
    )
    .await;
    let (status, parsed) = call(
        &router,
        "DeleteKey",
        &json!({"KeyIdentifier": second, "DeleteKeyInDays": 30}),
    )
    .await;
    assert_client_refusal(
        "DeleteKey re-scheduling a pending deletion",
        status,
        &parsed,
        "ConflictException",
    );

    // ServiceQuotaExceededException (@httpError 402): one past the set this emulator
    // holds for the account's default replication regions. Each generated name matches
    // the modelled `Region` pattern.
    let too_many: Vec<String> = (0..=MAX_ENABLED_REPLICATION_REGIONS)
        .map(|n| format!("eu-west-{n}"))
        .collect();
    let (status, parsed) = call(
        &router,
        "EnableDefaultKeyReplicationRegions",
        &json!({"ReplicationRegions": too_many}),
    )
    .await;
    assert_client_refusal(
        "EnableDefaultKeyReplicationRegions past the bound",
        status,
        &parsed,
        "ServiceQuotaExceededException",
    );

    // ServiceQuotaExceededException again, through a second operation: one past the
    // replication configuration this emulator holds for a single key.
    let bounded = fresh_key_arn(&router).await;
    let at_bound: Vec<String> = (0..MAX_KEY_REPLICATION_REGIONS)
        .map(|n| format!("ap-south-{n}"))
        .collect();
    ok_call(
        &router,
        "AddKeyReplicationRegions",
        &json!({"KeyIdentifier": bounded, "ReplicationRegions": at_bound}),
    )
    .await;
    let (status, parsed) = call(
        &router,
        "AddKeyReplicationRegions",
        &json!({"KeyIdentifier": bounded, "ReplicationRegions": ["eu-central-1"]}),
    )
    .await;
    assert_client_refusal(
        "AddKeyReplicationRegions past the bound",
        status,
        &parsed,
        "ServiceQuotaExceededException",
    );

    // ResourceNotFoundException (@httpError 404), for completeness of the class table.
    let (status, parsed) = call(
        &router,
        "GetKey",
        &json!({"KeyIdentifier": "alias/never-created"}),
    )
    .await;
    assert_client_refusal(
        "GetKey on an alias never bound",
        status,
        &parsed,
        "ResourceNotFoundException",
    );
}

/// RULE: a refusal the centralized constraint walk raises before any handler runs — a
/// value outside a member's declared bound — is answered under the same client-error
/// status, so a caller sees one status line for the whole class however early the
/// request was refused.
///
/// The rows are values one past each published bound of two different members:
/// `DeleteKeyInDays` (range 3..180) and `ListKeys`' `MaxResults` (range 1..100).
/// Source: `smithy_query(query="operation",operation="DeleteKey,ListKeys")`.
#[tokio::test]
async fn a_constraint_refusal_answers_the_same_client_error_status() {
    let router = router();
    let arn = fresh_key_arn(&router).await;

    for days in [2, 181] {
        let (status, parsed) = call(
            &router,
            "DeleteKey",
            &json!({"KeyIdentifier": arn, "DeleteKeyInDays": days}),
        )
        .await;
        assert_eq!(
            status, CLIENT_ERROR_STATUS,
            "case DeleteKeyInDays {days}: outside the modelled 3..180 range, expected \
             HTTP {CLIENT_ERROR_STATUS}, got {status} {parsed}"
        );
    }

    for max_results in [0, 101] {
        let (status, parsed) = call(&router, "ListKeys", &json!({"MaxResults": max_results})).await;
        assert_eq!(
            status, CLIENT_ERROR_STATUS,
            "case MaxResults {max_results}: outside the modelled 1..100 range, expected \
             HTTP {CLIENT_ERROR_STATUS}, got {status} {parsed}"
        );
    }
}
