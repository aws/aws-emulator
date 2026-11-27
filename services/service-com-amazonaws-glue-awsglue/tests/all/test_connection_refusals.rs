// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `TestConnection`: which connections it tests, and what it refuses an
//! incomplete one with.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is HTTP behaviour and not a direct call into a handler.
//!
//! RULE under test: the operation tests ONE connection — the definition the request carries
//! inline or the stored one its `ConnectionName` addresses, never both and never neither —
//! and it supports only a UNIFIED connection, so a connection of one of the legacy kinds is
//! refused with a `ValidationException` stating that restriction whichever form the definition
//! arrived in, in the sentence this service words for that form. An inline definition is then
//! read field by field: an `AuthenticationType` whose
//! credentials member is missing is the modelled `InvalidInputException` naming that member
//! path — a DIFFERENT class from the restriction above, which is what lets a caller tell the
//! two apart. A `ConnectionName` the addressed catalog holds nothing under is the same miss a
//! read of that connection answers.
//!
//! The recorded calls — an inline JDBC definition and an inline SALESFORCE definition whose
//! OAuth2 configuration carries no properties — are one row each of the tables below; the rows
//! differ along the connection type (legacy and unified ones the issue did not cite), the
//! authentication type and which of its members is missing, and the form the definition
//! arrived in.

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::state::{messages, LEGACY_CONNECTION_TYPES};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{call, emulator, error_code, error_message, ACCOUNT_ID};

/// Unified connection types, i.e. the per-source connector kinds. Source: the
/// `ConnectionType` enum of this service's model (`smithy_query shape=ConnectionType`), of
/// which these are connectors rather than one of the legacy kinds
/// ([`LEGACY_CONNECTION_TYPES`]) — the recorded case's `SALESFORCE` among them, plus kinds the
/// issue did not cite.
const UNIFIED_CONNECTION_TYPES: [&str; 5] = [
    "SALESFORCE",
    "SERVICENOW",
    "BIGQUERY",
    "POSTGRESQL",
    "ZENDESK",
];

/// The JDBC connection properties a client sends inline, as `TestConnectionInput`'s own
/// documentation lists them for that kind: "Required: All of (`HOST`, `PORT`, `JDBC_ENGINE`)
/// or `JDBC_CONNECTION_URL`" and "All of (`USERNAME`, `PASSWORD`) or `SECRET_ID`".
fn jdbc_properties() -> Value {
    json!({
        "JDBC_CONNECTION_URL": "jdbc:postgresql://warehouse.example.internal:5432/analytics",
        "USERNAME": "analytics",
        "PASSWORD": "a-password",
    })
}

/// Call `TestConnection` with `body`, answering its status and body.
async fn test_connection(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    call(router, "TestConnection", body).await
}

/// Register one connection of `connection_type` under `name`, so a by-name test has a stored
/// definition to read.
async fn given_connection(router: &MultiServiceRouter, name: &str, connection_type: &str) {
    let (status, body) = call(
        router,
        "CreateConnection",
        &json!({
            "ConnectionInput": {
                "Name": name,
                "ConnectionType": connection_type,
                "ConnectionProperties": jdbc_properties(),
            },
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateConnection must answer 200 for {connection_type}, got {status} with {body}"
    );
}

// ---------------------------------------------------------------------------
// The restriction on the kind of connection
// ---------------------------------------------------------------------------

/// A connection of one of the legacy kinds is refused with the restriction on what the
/// operation supports, whichever form the definition arrived in — and the refusal carries a
/// DIFFERENT error code from the field-level one, so the two are distinguishable.
///
/// The rows are every legacy `ConnectionType` the model declares, inline and then — for the
/// kinds a create accepts — through a stored connection.
#[tokio::test]
async fn a_test_of_a_legacy_connection_kind_is_refused_as_unsupported() {
    let router = emulator();

    for connection_type in LEGACY_CONNECTION_TYPES {
        let (status, body) = test_connection(
            &router,
            &json!({
                "TestConnectionInput": {
                    "ConnectionType": connection_type,
                    "ConnectionProperties": jdbc_properties(),
                },
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case inline {connection_type}: expected 400, got {status} with {body}"
        );
        assert!(
            error_code(&body).contains("ValidationException"),
            "case inline {connection_type}: expected ValidationException, got {body}"
        );
        assert_eq!(
            error_message(&body),
            messages::TEST_CONNECTION_INPUT_UNIFIED_ONLY,
            "case inline {connection_type}: expected the unified-only refusal, got {body}"
        );
    }

    // The rule is the CONNECTION's, not the request form's: a stored connection of a legacy
    // kind earns the same refusal when it is addressed by name.
    for connection_type in ["JDBC", "MONGODB", "NETWORK"] {
        let name = format!("legacy-{connection_type}");
        given_connection(&router, &name, connection_type).await;
        let (status, body) =
            test_connection(&router, &json!({ "ConnectionName": &name })).await;
        assert_eq!(
            status, 400,
            "case stored {connection_type}: expected 400, got {status} with {body}"
        );
        assert!(
            error_code(&body).contains("ValidationException"),
            "case stored {connection_type}: expected ValidationException, got {body}"
        );
        assert_eq!(
            error_message(&body),
            messages::TEST_CONNECTION_STORED_UNIFIED_ONLY,
            "case stored {connection_type}: expected the unified-only refusal, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// The field-level reading of an inline definition
// ---------------------------------------------------------------------------

/// An `AuthenticationType` whose credentials member is missing is refused with the modelled
/// input error naming that member path and the type that requires it — and the code DIFFERS
/// from the unified-only refusal above.
///
/// The rows span every `AuthenticationType` the model declares that carries credentials, over
/// several unified connection kinds: the recorded `OAUTH2` with no properties, `BASIC` with
/// neither credentials nor a secret, and `CUSTOM` the same way.
#[tokio::test]
async fn an_inline_definition_missing_its_credentials_is_the_modelled_input_refusal() {
    let router = emulator();

    let cases: Vec<(&str, Value, &str)> = vec![
        (
            "OAUTH2 with no OAuth2Properties",
            json!({ "AuthenticationType": "OAUTH2" }),
            "OAuth2Properties",
        ),
        (
            "OAUTH2 with a secret but no OAuth2Properties",
            json!({
                "AuthenticationType": "OAUTH2",
                "SecretArn": format!(
                    "arn:aws:secretsmanager:us-east-1:{ACCOUNT_ID}:secret:connector-abc123"
                ),
            }),
            "OAuth2Properties",
        ),
        (
            "BASIC with neither credentials nor a secret",
            json!({ "AuthenticationType": "BASIC" }),
            "BasicAuthenticationCredentials",
        ),
        (
            "CUSTOM with neither credentials nor a secret",
            json!({ "AuthenticationType": "CUSTOM" }),
            "CustomAuthenticationCredentials",
        ),
    ];

    for connection_type in UNIFIED_CONNECTION_TYPES {
        for (case, authentication_configuration, expected_member) in &cases {
            let authentication_type = authentication_configuration["AuthenticationType"]
                .as_str()
                .expect("every row names an AuthenticationType");
            let (status, body) = test_connection(
                &router,
                &json!({
                    "TestConnectionInput": {
                        "ConnectionType": connection_type,
                        "ConnectionProperties": {},
                        "AuthenticationConfiguration": authentication_configuration,
                    },
                }),
            )
            .await;
            assert_eq!(
                status, 400,
                "case {connection_type} {case}: expected 400, got {status} with {body}"
            );
            assert!(
                error_code(&body).contains("InvalidInputException"),
                "case {connection_type} {case}: expected InvalidInputException, got {body}"
            );
            assert_eq!(
                error_message(&body),
                messages::authentication_member_required_for_type(
                    expected_member,
                    authentication_type
                ),
                "case {connection_type} {case}: expected the missing-member refusal, got {body}"
            );
            // The two refusal classes are what a caller branches on, so they must differ.
            assert!(
                !error_code(&body).contains("ValidationException"),
                "case {connection_type} {case}: the field-level refusal must not carry the \
                 type-level code, got {body}"
            );
        }
    }
}

/// A definition that carries the credentials its authentication type needs is read past the
/// field-level rules: what it earns then is the report that the data source it points at was
/// not reached, never the missing-member refusal.
///
/// The rows span the authentication configurations that are complete: OAuth2 with properties,
/// basic credentials inline, basic credentials held in a secret, a custom credential map, the
/// `IAM` type that carries no credentials of its own, and no configuration at all.
#[tokio::test]
async fn a_complete_inline_definition_is_read_past_the_field_level_rules() {
    let router = emulator();
    let secret = format!(
        "arn:aws:secretsmanager:us-east-1:{ACCOUNT_ID}:secret:connector-abc123"
    );

    let cases: Vec<(&str, Value)> = vec![
        (
            "OAUTH2 with its properties",
            json!({
                "AuthenticationType": "OAUTH2",
                "OAuth2Properties": {
                    "OAuth2GrantType": "CLIENT_CREDENTIALS",
                    "OAuth2ClientApplication": { "UserManagedClientApplicationClientId": "id" },
                    "TokenUrl": "https://login.example.com/services/oauth2/token",
                },
            }),
        ),
        (
            "BASIC with credentials inline",
            json!({
                "AuthenticationType": "BASIC",
                "BasicAuthenticationCredentials": {
                    "Username": "analytics",
                    "Password": "a-password",
                },
            }),
        ),
        (
            "BASIC with the credentials in a secret",
            json!({ "AuthenticationType": "BASIC", "SecretArn": secret }),
        ),
        (
            "CUSTOM with a credential map",
            json!({
                "AuthenticationType": "CUSTOM",
                "CustomAuthenticationCredentials": { "apiKey": "a-key" },
            }),
        ),
        (
            "IAM, which carries no credentials of its own",
            json!({ "AuthenticationType": "IAM" }),
        ),
    ];

    for connection_type in UNIFIED_CONNECTION_TYPES {
        for (case, authentication_configuration) in &cases {
            let (status, body) = test_connection(
                &router,
                &json!({
                    "TestConnectionInput": {
                        "ConnectionType": connection_type,
                        "ConnectionProperties": {},
                        "AuthenticationConfiguration": authentication_configuration,
                    },
                }),
            )
            .await;
            assert_eq!(
                status, 400,
                "case {connection_type} {case}: expected 400, got {status} with {body}"
            );
            assert_eq!(
                error_message(&body),
                messages::connection_did_not_reach_the_source(connection_type),
                "case {connection_type} {case}: expected the unreached-source report, got {body}"
            );
        }

        // A definition that carries no authentication configuration at all states no
        // requirement to check either.
        let (status, body) = test_connection(
            &router,
            &json!({
                "TestConnectionInput": {
                    "ConnectionType": connection_type,
                    "ConnectionProperties": {},
                },
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {connection_type} with no configuration: expected 400, got {status} with \
             {body}"
        );
        assert_eq!(
            error_message(&body),
            messages::connection_did_not_reach_the_source(connection_type),
            "case {connection_type} with no configuration: expected the unreached-source \
             report, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// Which connection is under test
// ---------------------------------------------------------------------------

/// A request naming neither a connection nor an inline definition, and one naming both, are
/// each refused as the modelled input error: the call tests ONE connection.
#[tokio::test]
async fn a_request_that_does_not_name_one_connection_is_refused() {
    let router = emulator();
    given_connection(&router, "named-connection", "SALESFORCE").await;

    for (case, body, expected_message) in [
        (
            "neither member",
            json!({}),
            messages::TEST_CONNECTION_NAMES_NO_CONNECTION,
        ),
        (
            "only a catalog id",
            json!({ "CatalogId": ACCOUNT_ID }),
            messages::TEST_CONNECTION_NAMES_NO_CONNECTION,
        ),
        (
            "both a name and an inline definition",
            json!({
                "ConnectionName": "named-connection",
                "TestConnectionInput": {
                    "ConnectionType": "SALESFORCE",
                    "ConnectionProperties": {},
                },
            }),
            messages::TEST_CONNECTION_STATES_TWO_CONNECTIONS,
        ),
    ] {
        let (status, answer) = test_connection(&router, &body).await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400, got {status} with {answer}"
        );
        assert!(
            error_code(&answer).contains("InvalidInputException"),
            "case {case}: expected InvalidInputException, got {answer}"
        );
        assert_eq!(
            error_message(&answer),
            expected_message,
            "case {case}: expected the one-connection refusal, got {answer}"
        );
    }
}

/// A `ConnectionName` the addressed catalog holds nothing under is the same miss a read of
/// that connection answers: the rows span a name never created, one differing only in case
/// (the model documents no case fold for a connection name) and a name in a catalog this
/// account does not hold.
#[tokio::test]
async fn a_test_of_an_absent_connection_is_the_modelled_miss() {
    let router = emulator();
    given_connection(&router, "present-connection", "SALESFORCE").await;

    for (case, body, expected_message) in [
        (
            "a name never created",
            json!({ "ConnectionName": "absent-connection" }),
            messages::connection_not_found("absent-connection", ACCOUNT_ID),
        ),
        (
            "a name differing only in case",
            json!({ "ConnectionName": "PRESENT-CONNECTION" }),
            messages::connection_not_found("PRESENT-CONNECTION", ACCOUNT_ID),
        ),
        (
            "the account's own catalog named explicitly",
            json!({ "ConnectionName": "absent-connection", "CatalogId": ACCOUNT_ID }),
            messages::connection_not_found("absent-connection", ACCOUNT_ID),
        ),
    ] {
        let (status, answer) = test_connection(&router, &body).await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400, got {status} with {answer}"
        );
        assert!(
            error_code(&answer).contains("EntityNotFoundException"),
            "case {case}: expected EntityNotFoundException, got {answer}"
        );
        assert_eq!(
            error_message(&answer),
            expected_message,
            "case {case}: expected the connection miss, got {answer}"
        );
    }
}
