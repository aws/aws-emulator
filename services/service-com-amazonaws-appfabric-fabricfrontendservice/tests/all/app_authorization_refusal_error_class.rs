// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for WHICH modelled error AppFabric's app-authorization
//! paths answer with, and with which sentence, driven over the wire (restJson1)
//! through the same router the emulator boots.
//!
//! Each test states a RULE over a TABLE of inputs rather than one recorded call:
//!
//! * a create that a record the store ALREADY holds makes impossible is the
//!   VALIDATION refusal (HTTP 400 `ValidationException`) of the two client
//!   errors `CreateAppAuthorization` models — the request can never succeed as
//!   written, which is not a conflict that settles on its own — carrying the
//!   service's fixed sentence and faulting no request member;
//! * an app-authorization identifier the named bundle holds no record for
//!   resolves to no resource, so every operation of that chain — the
//!   `@idempotent` delete included — refuses it with the modelled
//!   `ResourceNotFoundException` (HTTP 404), one fixed sentence, and the
//!   supplied identifier in `resourceId`;
//! * `app` is typed as an open `String255`, so the wire schema bounds its length
//!   only, while the set of applications the service serves is the one the
//!   member's documentation gives: a name in that set runs the whole authorize →
//!   connect → ingest chain (a name outside it is refused, which
//!   `supported_application_names` pins).
//!
//! The harness (the router, `Answer`, `encode`, `arn_of`, `uuid_of`) is the one
//! `app_bundle_ingestion_lifecycle` already builds.

use serde_json::{json, Value};

use crate::app_bundle_ingestion_lifecycle::{
    arn_of, encode, unknown_bundle_arn, unknown_uuid, uuid_of, Answer, Emulator,
};

/// The sentence a second app authorization for one tenant and authorization type
/// is refused with. Spelled out here, so these tests assert the wording itself
/// rather than whatever constant the handler happens to reach for.
const ALREADY_EXISTS_MESSAGE: &str =
    "An AppAuthorization already exists for this tenant and this authType.";

/// The sentence every operation of the app-authorization chain refuses an
/// identifier it holds no record for with.
const NOT_FOUND_MESSAGE: &str = "The app authorization was not found.";

/// The sentence `CreateIngestion` refuses a pair whose app authorization has not
/// completed the connect handshake with — see `ingestion_authorization_refusal_wording`.
const NOT_CONNECTED_MESSAGE: &str =
    "The app authorization for the specified application and tenant was not connected. \
     Connect the app authorization for the application and tenant before creating an ingestion.";

/// The two `AuthType` values the model's enum carries
/// (`smithy_query(query="shape",shape="AuthType")`: `apiKey` | `oauth2`).
const AUTH_TYPES: &[&str] = &["apiKey", "oauth2"];

/// Application names the tables below carry, from the set the model's own
/// documentation of `app` gives
/// (`smithy_query(query="doc",shape="CreateAppAuthorizationRequest",member="app")`:
/// "Valid values are: SLACK ASANA JIRA M365 M365AUDITLOGS ZOOM ZENDESK OKTA
/// GOOGLE DROPBOX SMARTSHEET CISCO") — entries the issue did not cite rather
/// than one repeated value.
const APPS: &[&str] = &[
    "ZENDESK",
    "DROPBOX",
    "M365AUDITLOGS",
    "CISCO",
    "ZOOM",
    "GOOGLE",
    "JIRA",
];

/// The `credential` body one `authType` takes, as the model's documentation of
/// the member requires ("Specify credentials that match the authorization type
/// for your request").
fn credential_for(auth_type: &str) -> Value {
    credential_for_registration(auth_type, "placeholder")
}

/// The same body, for one named REGISTRATION of the application: an OAuth2
/// client id names the caller's registration, and a duplicate create that
/// repeats a held registration's client id is refused for that client id rather
/// than for the tenant (`oauth2_client_id_registration` pins which sentence
/// answers when), so a row that means to pin the per-tenant refusal supplies a
/// registration of its own.
fn credential_for_registration(auth_type: &str, registration: &str) -> Value {
    if auth_type == "apiKey" {
        json!({"apiKeyCredential": {"apiKey": format!("{registration}-api-key")}})
    } else {
        json!({"oauth2Credential": {
            "clientId": format!("{registration}-client-id"),
            "clientSecret": "placeholder-client-secret",
        }})
    }
}

/// A string member of a response body, or `""`.
fn member(answer: &Answer, name: &str) -> String {
    answer
        .body
        .get(name)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// One member of the `appAuthorization` a success body carries.
fn authorization_member<'a>(answer: &'a Answer, name: &str) -> &'a Value {
    answer
        .body
        .get("appAuthorization")
        .and_then(|authorization| authorization.get(name))
        .unwrap_or_else(|| panic!("expected appAuthorization.{name} in {}", answer.body))
}

/// `CreateAppAuthorization` with no `clientToken`, so every call is a request of
/// its own rather than a retry.
async fn create_authorization(
    emulator: &Emulator,
    bundle: &str,
    app: &str,
    auth_type: &str,
    tenant: &str,
) -> Answer {
    create_authorization_registered_with(emulator, bundle, app, auth_type, tenant, "placeholder")
        .await
}

/// The same call, for one named registration of the application — see
/// [`credential_for_registration`].
async fn create_authorization_registered_with(
    emulator: &Emulator,
    bundle: &str,
    app: &str,
    auth_type: &str,
    tenant: &str,
    registration: &str,
) -> Answer {
    emulator
        .call(
            "POST",
            &format!("/appbundles/{}/appauthorizations", encode(bundle)),
            Some(json!({
                "app": app,
                "authType": auth_type,
                "credential": credential_for_registration(auth_type, registration),
                "tenant": {
                    "tenantIdentifier": tenant,
                    "tenantDisplayName": format!("{tenant} display"),
                },
            })),
        )
        .await
}

/// `ConnectAppAuthorization`, carrying the `authRequest` an `oauth2`
/// authorization requires and nothing for an `apiKey` one.
async fn connect_authorization(
    emulator: &Emulator,
    bundle: &str,
    authorization: &str,
    auth_type: &str,
) -> Answer {
    let body = if auth_type == "apiKey" {
        json!({})
    } else {
        json!({"authRequest": {
            "redirectUri": "https://example.com/oauth/callback",
            "code": "placeholder-authorization-code",
        }})
    };
    emulator
        .call(
            "POST",
            &format!(
                "/appbundles/{}/appauthorizations/{}/connect",
                encode(bundle),
                encode(authorization)
            ),
            Some(body),
        )
        .await
}

/// The ARNs the bundle's `ListAppAuthorizations` reports, in the order it
/// reports them.
async fn held_authorizations(emulator: &Emulator, bundle: &str) -> Vec<String> {
    let listed = emulator
        .call(
            "GET",
            &format!("/appbundles/{}/appauthorizations", encode(bundle)),
            None,
        )
        .await;
    assert_eq!(
        listed.status, 200,
        "ListAppAuthorizations must answer HTTP 200, got {} with body {}",
        listed.status, listed.body
    );
    listed
        .body
        .get("appAuthorizationSummaryList")
        .and_then(Value::as_array)
        .map(|summaries| {
            summaries
                .iter()
                .filter_map(|summary| summary.get("appAuthorizationArn"))
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// RULE: a bundle holds at most one app authorization per (application, tenant,
/// authorization type), and a create for one it already holds is refused with
/// the modelled `ValidationException` (HTTP 400) carrying the service's fixed
/// sentence, `reason` `other` and no `fieldList` — the sentence has no variable
/// part, and the stored authorization is left exactly as it was.
///
/// The table spans the application (names the model documents), both `AuthType`
/// values and tenant identifiers of several forms, because none of them may
/// change which error class answers. Every second create supplies a
/// REGISTRATION of its own, so what it repeats is the tenant alone: a create
/// that repeats the held registration's own client id is refused for that
/// instead, which `oauth2_client_id_registration` pins.
#[tokio::test]
async fn a_second_authorization_for_a_held_pair_is_the_modelled_validation_refusal() {
    let tenants = [
        "tenant-word",
        "fabric.example.com",
        "11111111-2222-4333-8444-555555555555",
    ];

    for (index, app) in APPS.iter().enumerate() {
        for auth_type in AUTH_TYPES {
            let tenant = tenants[index % tenants.len()];
            let case = format!("app {app}, authType {auth_type}, tenant {tenant}");
            let emulator = Emulator::new();
            let bundle = arn_of(
                &emulator.create_app_bundle(json!({})).await.body,
                "appBundle",
            );

            let created = create_authorization(&emulator, &bundle, app, auth_type, tenant).await;
            assert_eq!(
                created.status, 201,
                "case {case:?}: expected HTTP 201 from the first create, got {} with body {}",
                created.status, created.body
            );
            let arn = authorization_member(&created, "appAuthorizationArn")
                .as_str()
                .unwrap_or_default()
                .to_string();

            let refused = create_authorization_registered_with(
                &emulator, &bundle, app, auth_type, tenant, "another",
            )
            .await;
            assert_eq!(
                refused.status, 400,
                "case {case:?}: expected HTTP 400 from the second create, got {} with body {}",
                refused.status, refused.body
            );
            assert_eq!(
                refused.error_code(),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {} with body {}",
                refused.error_code(),
                refused.body
            );
            assert_eq!(
                member(&refused, "message"),
                ALREADY_EXISTS_MESSAGE,
                "case {case:?}: expected the service's fixed sentence, got {:?}",
                member(&refused, "message")
            );
            assert_eq!(
                member(&refused, "reason"),
                "other",
                "case {case:?}: the store, not a request member, forbids the call, got {}",
                refused.body
            );
            assert!(
                refused.body.get("fieldList").is_none(),
                "case {case:?}: no request member is at fault, got {}",
                refused.body
            );
            for (what, value) in [
                ("the application", (*app).to_string()),
                ("the tenant", tenant.to_string()),
                ("the authorization type", (*auth_type).to_string()),
                ("the stored ARN", arn.clone()),
            ] {
                assert!(
                    !member(&refused, "message").contains(&value),
                    "case {case:?}: the sentence must not carry {what} ({value:?}), got {:?}",
                    member(&refused, "message")
                );
            }

            // The refused create wrote nothing: the first authorization is
            // still the only one the bundle holds.
            assert_eq!(
                held_authorizations(&emulator, &bundle).await,
                vec![arn.clone()],
                "case {case:?}: expected the bundle to still hold exactly the first authorization"
            );
        }
    }
}

/// Which operation of the app-authorization chain a row drives.
#[derive(Debug, Clone, Copy)]
enum Chain {
    Get,
    Update,
    Delete,
}

impl Chain {
    fn method(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Update => "PATCH",
            Self::Delete => "DELETE",
        }
    }

    fn body(self) -> Option<Value> {
        match self {
            Self::Update => Some(json!({"credential": credential_for("apiKey")})),
            _ => None,
        }
    }
}

/// RULE: an app-authorization identifier the named bundle holds no record for
/// resolves to no resource, so the call is refused with the modelled
/// `ResourceNotFoundException` (HTTP 404) — the chain's ONE fixed sentence, the
/// `AppAuthorization` as `resourceType`, the identifier the caller supplied as
/// `resourceId` — and nothing is written. That holds for the `@idempotent`
/// delete too: `@idempotent` bounds the EFFECT of a repeated call, it does not
/// make a target the bundle never held a deletion.
///
/// The table spans the three operations of the chain crossed with every way an
/// identifier comes to name no record of this bundle: never minted (in either
/// `Identifier` form), minted under ANOTHER bundle, and deleted by an earlier
/// call of this very test.
#[tokio::test]
async fn an_app_authorization_identifier_the_bundle_does_not_hold_is_not_found() {
    for chain in [Chain::Get, Chain::Update, Chain::Delete] {
        let emulator = Emulator::new();
        let bundle = emulator.fixture_bundle(0).await;
        let other_bundle = emulator.fixture_bundle(1).await;

        // The record this bundle keeps throughout, so a row can prove the
        // refusal wrote nothing.
        let kept = create_authorization(&emulator, &bundle, "OKTA", "apiKey", "tenant-kept").await;
        assert_eq!(
            kept.status, 201,
            "{chain:?}: the set-up authorization must be created, got {} with body {}",
            kept.status, kept.body
        );
        let kept_arn = authorization_member(&kept, "appAuthorizationArn")
            .as_str()
            .unwrap_or_default()
            .to_string();

        // One under the OTHER bundle: a real authorization, but not this
        // bundle's.
        let foreign = create_authorization(
            &emulator,
            &other_bundle,
            "SMARTSHEET",
            "apiKey",
            "fabric.smartsheet.com",
        )
        .await;
        assert_eq!(
            foreign.status, 201,
            "{chain:?}: the other bundle's authorization must be created, got {} with body {}",
            foreign.status, foreign.body
        );
        let foreign_arn = authorization_member(&foreign, "appAuthorizationArn")
            .as_str()
            .unwrap_or_default()
            .to_string();

        // One this bundle DID hold and no longer does, which is the
        // double-delete direction.
        let gone =
            create_authorization(&emulator, &bundle, "ASANA", "oauth2", "tenant-gone").await;
        assert_eq!(
            gone.status, 201,
            "{chain:?}: the doomed authorization must be created, got {} with body {}",
            gone.status, gone.body
        );
        let gone_arn = authorization_member(&gone, "appAuthorizationArn")
            .as_str()
            .unwrap_or_default()
            .to_string();
        let removed = emulator
            .call(
                "DELETE",
                &format!(
                    "/appbundles/{}/appauthorizations/{}",
                    encode(&bundle),
                    encode(&gone_arn)
                ),
                None,
            )
            .await;
        assert_eq!(
            removed.status, 204,
            "{chain:?}: the delete of a held authorization must answer HTTP 204, got {} with \
             body {}",
            removed.status, removed.body
        );

        // (case, the identifier the bundle holds no record for)
        let identifiers = [
            ("a uuid no create minted", unknown_uuid()),
            (
                "an authorization arn no create minted",
                format!(
                    "{}/appauthorization/{}",
                    unknown_bundle_arn(&uuid_of(&bundle)),
                    unknown_uuid()
                ),
            ),
            ("another bundle's authorization, by arn", foreign_arn.clone()),
            (
                "another bundle's authorization, by uuid",
                uuid_of(&foreign_arn),
            ),
            ("one this bundle held and deleted, by arn", gone_arn.clone()),
            (
                "one this bundle held and deleted, by uuid",
                uuid_of(&gone_arn),
            ),
        ];

        for (identifier_case, identifier) in &identifiers {
            let case = format!("{chain:?} on {identifier_case}");
            let answer = emulator
                .call(
                    chain.method(),
                    &format!(
                        "/appbundles/{}/appauthorizations/{}",
                        encode(&bundle),
                        encode(identifier)
                    ),
                    chain.body(),
                )
                .await;
            assert_eq!(
                answer.status, 404,
                "case {case:?}: expected HTTP 404, got {} with body {}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code(),
                "ResourceNotFoundException",
                "case {case:?}: expected ResourceNotFoundException, got {} with body {}",
                answer.error_code(),
                answer.body
            );
            assert_eq!(
                member(&answer, "message"),
                NOT_FOUND_MESSAGE,
                "case {case:?}: expected the chain's fixed sentence, got {:?}",
                member(&answer, "message")
            );
            assert_eq!(
                member(&answer, "resourceType"),
                "AppAuthorization",
                "case {case:?}: expected resourceType AppAuthorization, got {}",
                answer.body
            );
            assert_eq!(
                member(&answer, "resourceId"),
                identifier.as_str(),
                "case {case:?}: expected the supplied identifier in resourceId, got {}",
                answer.body
            );
            assert!(
                !member(&answer, "message").contains(identifier.as_str()),
                "case {case:?}: the sentence must not echo the identifier, got {:?}",
                member(&answer, "message")
            );
        }

        // Nothing any of those refusals touched: this bundle still holds the
        // record it kept, and the other bundle still holds its own.
        assert_eq!(
            held_authorizations(&emulator, &bundle).await,
            vec![kept_arn.clone()],
            "{chain:?}: a refused call must leave this bundle's authorizations alone"
        );
        assert_eq!(
            held_authorizations(&emulator, &other_bundle).await,
            vec![foreign_arn.clone()],
            "{chain:?}: a refused call must leave the other bundle's authorizations alone"
        );
    }
}

/// RULE: an application name the service serves runs the whole chain — create
/// (`PendingConnect`, `persona` `admin`, the tenant echoed), connect
/// (`Connected`), then the ingestion of that pair — and the name the request
/// carried is what every read of those records reports. Before the connect call
/// the ingestion is refused for the handshake the pair has not completed, which
/// names neither the application nor the tenant.
///
/// The table spans names from the served set that the issue did not cite, with
/// tenant identifiers of the forms an application tenant takes. The refusal of a
/// name outside that set is pinned in `supported_application_names`.
#[tokio::test]
async fn a_served_application_name_runs_the_whole_chain() {
    // (application, tenant identifier)
    let cases = [
        ("ASANA", "fabric.asana.com"),
        ("OKTA", "fabric-okta"),
        ("M365", "99999999-8888-4777-8666-555555555555"),
        ("SLACK", "fabric.slack.com"),
    ];

    for (app, tenant) in cases {
        let case = format!("app {app}, tenant {tenant}");
        let emulator = Emulator::new();
        let bundle = arn_of(
            &emulator.create_app_bundle(json!({})).await.body,
            "appBundle",
        );

        let created = create_authorization(&emulator, &bundle, app, "apiKey", tenant).await;
        assert_eq!(
            created.status, 201,
            "case {case:?}: expected HTTP 201 from CreateAppAuthorization, got {} with body {}",
            created.status, created.body
        );
        for (name, expected) in [
            ("app", json!(app)),
            ("authType", json!("apiKey")),
            ("status", json!("PendingConnect")),
            ("persona", json!("admin")),
            ("appBundleArn", json!(bundle.clone())),
            (
                "tenant",
                json!({
                    "tenantIdentifier": tenant,
                    "tenantDisplayName": format!("{tenant} display"),
                }),
            ),
        ] {
            assert_eq!(
                authorization_member(&created, name),
                &expected,
                "case {case:?}: expected appAuthorization.{name} to be {expected}, got {}",
                created.body
            );
        }
        for stamp in ["createdAt", "updatedAt"] {
            assert!(
                authorization_member(&created, stamp).is_number(),
                "case {case:?}: expected a timestamp in appAuthorization.{stamp}, got {}",
                created.body
            );
        }
        let arn = authorization_member(&created, "appAuthorizationArn")
            .as_str()
            .unwrap_or_default()
            .to_string();

        // Before the handshake: the ingestion of that pair is refused for the
        // connect step the caller still owes.
        let refused = emulator
            .call(
                "POST",
                &format!("/appbundles/{}/ingestions", encode(&bundle)),
                Some(json!({"app": app, "tenantId": tenant, "ingestionType": "auditLog"})),
            )
            .await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected HTTP 400 from CreateIngestion before the connect, got {} \
             with body {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {} with body {}",
            refused.error_code(),
            refused.body
        );
        assert_eq!(
            member(&refused, "message"),
            NOT_CONNECTED_MESSAGE,
            "case {case:?}: expected the connect-the-authorization sentence, got {:?}",
            member(&refused, "message")
        );

        let connected = connect_authorization(&emulator, &bundle, &arn, "apiKey").await;
        assert_eq!(
            connected.status, 200,
            "case {case:?}: expected HTTP 200 from ConnectAppAuthorization, got {} with body {}",
            connected.status, connected.body
        );

        let read = emulator
            .call(
                "GET",
                &format!(
                    "/appbundles/{}/appauthorizations/{}",
                    encode(&bundle),
                    encode(&arn)
                ),
                None,
            )
            .await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected the authorization to be readable, got {} with body {}",
            read.status, read.body
        );
        assert_eq!(
            authorization_member(&read, "status"),
            &json!("Connected"),
            "case {case:?}: expected the connected status on the read, got {}",
            read.body
        );
        assert_eq!(
            authorization_member(&read, "app"),
            &json!(app),
            "case {case:?}: expected the stored application name on the read, got {}",
            read.body
        );

        // And the ingestion that depends on that pair now exists.
        let ingestion = emulator
            .call(
                "POST",
                &format!("/appbundles/{}/ingestions", encode(&bundle)),
                Some(json!({"app": app, "tenantId": tenant, "ingestionType": "auditLog"})),
            )
            .await;
        assert_eq!(
            ingestion.status, 201,
            "case {case:?}: expected HTTP 201 from CreateIngestion once connected, got {} with \
             body {}",
            ingestion.status, ingestion.body
        );
        let ingestion_arn = arn_of(&ingestion.body, "ingestion");
        let ingestion_read = emulator.get_ingestion(&bundle, &ingestion_arn).await;
        assert_eq!(
            ingestion_read.status, 200,
            "case {case:?}: expected the ingestion to be readable, got {} with body {}",
            ingestion_read.status, ingestion_read.body
        );
        assert_eq!(
            ingestion_read
                .body
                .get("ingestion")
                .and_then(|record| record.get("app")),
            Some(&json!(app)),
            "case {case:?}: expected the ingestion to report the application name, got {}",
            ingestion_read.body
        );
    }
}
