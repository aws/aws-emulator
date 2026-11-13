// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for WHICH sentence AppFabric refuses a call that would
//! leave one app bundle holding a SECOND app authorization of one application
//! for one tenant, driven over the wire (restJson1) through the same router the
//! emulator boots.
//!
//! The RULE every test here states: such a call is refused with the modelled
//! `ValidationException` (HTTP 400), and the refusal names what the REQUEST
//! repeats — the OAuth2 client id it supplies, when the authorization standing
//! in its way is registered with that very client id (an OAuth2 client id names
//! the caller's registration of the application, and the sentence states the
//! scope that registration is held in: the app bundle and the application) —
//! and the tenant it names otherwise. A credential that registers no client id
//! (an API key) repeats no registration, and a client id is a registration
//! rather than a secret, so rotating the secret beside it repeats the same one.
//!
//! The rule selects the SENTENCE of a refusal; it does not refuse a call that
//! would otherwise succeed, which `a_registration_may_back_several_tenants`
//! pins.
//!
//! The harness (the router, `Answer`, `encode`, `arn_of`) is the one
//! `app_bundle_ingestion_lifecycle` already builds.

use std::sync::Arc;

use serde_json::{json, Value};

use crate::app_bundle_ingestion_lifecycle::{arn_of, encode, Answer, Emulator};

/// The sentence a call that repeats a held registration's OAuth2 client id is
/// refused with. Spelled out here, so these tests assert the wording itself
/// rather than whatever constant the handler reaches for.
const CLIENT_ID_MESSAGE: &str = "ClientId is already being used for this AppBundle and App.";

/// The sentence a call that repeats only the tenant is refused with — the one
/// `app_authorization_refusal_error_class` pins for the create path.
const ALREADY_EXISTS_MESSAGE: &str =
    "An AppAuthorization already exists for this tenant and this authType.";

/// Application names these tables carry, from the set the model's own
/// documentation of `app` gives
/// (`smithy_query(query="doc",shape="CreateAppAuthorizationRequest",member="app")`:
/// "Valid values are: SLACK ASANA JIRA M365 M365AUDITLOGS ZOOM ZENDESK OKTA
/// GOOGLE DROPBOX SMARTSHEET CISCO").
const APPS: &[&str] = &["ZOOM", "SMARTSHEET", "M365", "GOOGLE", "ASANA", "CISCO"];

/// One OAuth2 registration of an application: the client id that names it, and
/// the secret beside it.
#[derive(Debug, Clone, Copy)]
struct Oauth2 {
    client_id: &'static str,
    client_secret: &'static str,
}

/// The `credential` body for one OAuth2 registration.
fn oauth2_credential(registration: Oauth2) -> Value {
    json!({"oauth2Credential": {
        "clientId": registration.client_id,
        "clientSecret": registration.client_secret,
    }})
}

/// The `credential` body for an API key — the credential kind that registers no
/// client id at all (`AuthType` is `apiKey` | `oauth2`, from
/// `smithy_query(query="shape",shape="AuthType")`).
fn api_key_credential(api_key: &str) -> Value {
    json!({"apiKeyCredential": {"apiKey": api_key}})
}

/// `CreateAppAuthorization` with no `clientToken`, so every call is a request of
/// its own rather than a retry of an earlier one.
async fn create_authorization(
    emulator: &Emulator,
    bundle: &str,
    app: &str,
    auth_type: &str,
    tenant: &str,
    credential: &Value,
) -> Answer {
    emulator
        .call(
            "POST",
            &format!("/appbundles/{}/appauthorizations", encode(bundle)),
            Some(json!({
                "app": app,
                "authType": auth_type,
                "credential": credential,
                "tenant": {
                    "tenantIdentifier": tenant,
                    "tenantDisplayName": format!("{tenant} display"),
                },
            })),
        )
        .await
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
fn authorization_member(answer: &Answer, name: &str) -> Value {
    answer
        .body
        .get("appAuthorization")
        .and_then(|authorization| authorization.get(name))
        .cloned()
        .unwrap_or_else(|| panic!("expected appAuthorization.{name} in {}", answer.body))
}

/// The `appAuthorizationArn` of a create that succeeded.
fn created_arn(answer: &Answer) -> String {
    authorization_member(answer, "appAuthorizationArn")
        .as_str()
        .unwrap_or_default()
        .to_string()
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

/// The stored `tenantIdentifier` of one authorization, read back through
/// `GetAppAuthorization`.
async fn stored_tenant(emulator: &Emulator, bundle: &str, authorization: &str) -> String {
    let read = emulator
        .call(
            "GET",
            &format!(
                "/appbundles/{}/appauthorizations/{}",
                encode(bundle),
                encode(authorization)
            ),
            None,
        )
        .await;
    assert_eq!(
        read.status, 200,
        "GetAppAuthorization must answer HTTP 200, got {} with body {}",
        read.status, read.body
    );
    authorization_member(&read, "tenant")
        .get("tenantIdentifier")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// A refusal of the shape the client-id sentence takes: the state the store
/// holds, not a member of the request, is what forbids the call, so `reason` is
/// `other` and no `fieldList` names a member.
fn assert_state_refusal(answer: &Answer, case: &str, expected_message: &str) {
    assert_eq!(
        answer.status, 400,
        "case {case:?}: expected HTTP 400, got {} with body {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code(),
        "ValidationException",
        "case {case:?}: expected ValidationException, got {} with body {}",
        answer.error_code(),
        answer.body
    );
    assert_eq!(
        member(answer, "message"),
        expected_message,
        "case {case:?}: expected the service's fixed sentence, got {:?}",
        member(answer, "message")
    );
    assert_eq!(
        member(answer, "reason"),
        "other",
        "case {case:?}: the store, not a request member, forbids the call, got {}",
        answer.body
    );
    assert!(
        answer.body.get("fieldList").is_none(),
        "case {case:?}: no request member is at fault, got {}",
        answer.body
    );
}

/// One row of the create table: the credential the held authorization was
/// registered with, the credential the second create supplies, and the sentence
/// that second create must be refused with.
#[derive(Debug, Clone, Copy)]
struct Row {
    case: &'static str,
    auth_type: &'static str,
    held: Credential,
    supplied: Credential,
    expected: &'static str,
}

/// The credential a row supplies, in the two kinds the model's `Credential`
/// union carries.
#[derive(Debug, Clone, Copy)]
enum Credential {
    Oauth2(Oauth2),
    ApiKey(&'static str),
}

impl Credential {
    fn body(self) -> Value {
        match self {
            Self::Oauth2(registration) => oauth2_credential(registration),
            Self::ApiKey(api_key) => api_key_credential(api_key),
        }
    }

    /// The values a refusal's sentence must not echo: a fixed sentence carries
    /// no part of the credential the caller supplied.
    fn values(self) -> Vec<&'static str> {
        match self {
            Self::Oauth2(registration) => {
                vec![registration.client_id, registration.client_secret]
            }
            Self::ApiKey(api_key) => vec![api_key],
        }
    }
}

/// The registrations the rows below are built from.
const FIRST: Oauth2 = Oauth2 {
    client_id: "registration-one",
    client_secret: "secret-one",
};
const SECOND: Oauth2 = Oauth2 {
    client_id: "registration-two",
    client_secret: "secret-two",
};

/// The create table: the rule has to select the same way whichever credential
/// kind, client id, secret, application and tenant form a row carries.
const ROWS: &[Row] = &[
    Row {
        case: "the very same registration",
        auth_type: "oauth2",
        held: Credential::Oauth2(FIRST),
        supplied: Credential::Oauth2(FIRST),
        expected: CLIENT_ID_MESSAGE,
    },
    Row {
        case: "the same client id, another secret beside it",
        auth_type: "oauth2",
        held: Credential::Oauth2(FIRST),
        supplied: Credential::Oauth2(Oauth2 {
            client_id: FIRST.client_id,
            client_secret: "rotated-secret",
        }),
        expected: CLIENT_ID_MESSAGE,
    },
    Row {
        case: "another client id, the same secret beside it",
        auth_type: "oauth2",
        held: Credential::Oauth2(FIRST),
        supplied: Credential::Oauth2(Oauth2 {
            client_id: SECOND.client_id,
            client_secret: FIRST.client_secret,
        }),
        expected: ALREADY_EXISTS_MESSAGE,
    },
    Row {
        case: "another registration altogether",
        auth_type: "oauth2",
        held: Credential::Oauth2(FIRST),
        supplied: Credential::Oauth2(SECOND),
        expected: ALREADY_EXISTS_MESSAGE,
    },
    Row {
        case: "the very same api key, which registers no client id",
        auth_type: "apiKey",
        held: Credential::ApiKey("api-key-one"),
        supplied: Credential::ApiKey("api-key-one"),
        expected: ALREADY_EXISTS_MESSAGE,
    },
    Row {
        case: "another api key",
        auth_type: "apiKey",
        held: Credential::ApiKey("api-key-one"),
        supplied: Credential::ApiKey("api-key-two"),
        expected: ALREADY_EXISTS_MESSAGE,
    },
];

/// Tenant identifiers of the forms an application tenant takes, so no row's
/// outcome depends on the shape of the tenant it names.
const TENANTS: &[&str] = &[
    "tenant-word",
    "fabric.example.com",
    "11111111-2222-4333-8444-555555555555",
];

/// RULE: a second create for an (application, tenant) pair the bundle already
/// holds an authorization for is refused with the modelled `ValidationException`
/// (HTTP 400), and the sentence names what the request repeats: the OAuth2
/// client id it supplies when the held authorization is registered with that
/// very client id, and the tenant otherwise. Either sentence is wholly fixed
/// text, `reason` is `other`, no member is faulted, and the refused create
/// writes nothing.
#[tokio::test]
async fn a_duplicate_create_is_refused_for_what_the_request_repeats() {
    for (index, row) in ROWS.iter().enumerate() {
        let app = APPS[index % APPS.len()];
        let tenant = TENANTS[index % TENANTS.len()];
        let case = format!("{} (app {app}, tenant {tenant})", row.case);
        let emulator = Emulator::new();
        let bundle = arn_of(
            &emulator.create_app_bundle(json!({})).await.body,
            "appBundle",
        );

        let created = create_authorization(
            &emulator,
            &bundle,
            app,
            row.auth_type,
            tenant,
            &row.held.body(),
        )
        .await;
        assert_eq!(
            created.status, 201,
            "case {case:?}: expected HTTP 201 from the first create, got {} with body {}",
            created.status, created.body
        );
        let arn = created_arn(&created);

        let refused = create_authorization(
            &emulator,
            &bundle,
            app,
            row.auth_type,
            tenant,
            &row.supplied.body(),
        )
        .await;
        assert_state_refusal(&refused, &case, row.expected);
        for value in row
            .supplied
            .values()
            .into_iter()
            .chain([app, tenant, row.auth_type, arn.as_str()])
        {
            assert!(
                !member(&refused, "message").contains(value),
                "case {case:?}: the sentence must not carry {value:?}, got {:?}",
                member(&refused, "message")
            );
        }

        // The refused create wrote nothing: the first authorization is still
        // the only one the bundle holds, and still reports its own tenant.
        assert_eq!(
            held_authorizations(&emulator, &bundle).await,
            vec![arn.clone()],
            "case {case:?}: expected the bundle to still hold exactly the first authorization"
        );
        assert_eq!(
            stored_tenant(&emulator, &bundle, &arn).await,
            tenant,
            "case {case:?}: a refused create must leave the stored authorization alone"
        );
    }
}

/// RULE (the boundary of the one above): the client id selects the SENTENCE of a
/// refusal, it does not make a call impossible. One registration backs as many
/// tenants of an application as a caller creates authorizations for, and an
/// authorization of ANOTHER application may be registered with it too, so every
/// such create succeeds and each authorization is persisted on its own.
///
/// The table spans the number of tenants one registration backs and the
/// applications it is used for.
#[tokio::test]
async fn a_registration_may_back_several_tenants() {
    for tenants in [2_usize, 3, 5] {
        let case = format!("{tenants} tenants of one registration");
        let emulator = Emulator::new();
        let bundle = arn_of(
            &emulator.create_app_bundle(json!({})).await.body,
            "appBundle",
        );

        let mut created = Vec::with_capacity(tenants * APPS.len());
        for app in APPS {
            for index in 0..tenants {
                let tenant = format!("tenant-{index}.example.com");
                let answer = create_authorization(
                    &emulator,
                    &bundle,
                    app,
                    "oauth2",
                    &tenant,
                    &oauth2_credential(FIRST),
                )
                .await;
                assert_eq!(
                    answer.status, 201,
                    "case {case:?}: app {app}, tenant {tenant}: one registration backs every \
                     tenant, expected HTTP 201, got {} with body {}",
                    answer.status, answer.body
                );
                let arn = created_arn(&answer);
                assert_eq!(
                    stored_tenant(&emulator, &bundle, &arn).await,
                    tenant,
                    "case {case:?}: app {app}: expected the created authorization to report its \
                     own tenant"
                );
                created.push(arn);
            }
        }

        let mut held = held_authorizations(&emulator, &bundle).await;
        held.sort();
        let mut expected = created;
        expected.sort();
        assert_eq!(
            held, expected,
            "case {case:?}: expected the bundle to hold every created authorization"
        );
    }
}

/// RULE: however many callers race to create an authorization for one
/// (application, tenant) pair with the SAME registration, exactly one succeeds
/// and every loser is refused for the client id it sent — the one the winner
/// registered — with the modelled `ValidationException` (HTTP 400). The bundle
/// afterwards holds exactly one authorization for the pair: the winner's, which
/// reads back under its own ARN.
///
/// The table spans the number of racers; the runtime is multi-threaded, so the
/// calls genuinely overlap.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_loser_of_a_race_for_one_registration_is_refused_for_its_client_id() {
    for racers in [2_usize, 5, 9] {
        let case = format!("{racers} racers");
        let emulator = Arc::new(Emulator::new());
        let bundle = arn_of(
            &emulator.create_app_bundle(json!({})).await.body,
            "appBundle",
        );

        let mut tasks = Vec::with_capacity(racers);
        for _ in 0..racers {
            let emulator = Arc::clone(&emulator);
            let bundle = bundle.clone();
            tasks.push(tokio::spawn(async move {
                create_authorization(
                    &emulator,
                    &bundle,
                    "DROPBOX",
                    "oauth2",
                    "race-tenant.example.com",
                    &oauth2_credential(FIRST),
                )
                .await
            }));
        }

        let mut winners: Vec<String> = Vec::new();
        for task in tasks {
            let answer = task.await.expect("a racer task never panics");
            if answer.status == 201 {
                winners.push(created_arn(&answer));
            } else {
                assert_state_refusal(&answer, &case, CLIENT_ID_MESSAGE);
            }
        }

        assert_eq!(
            winners.len(),
            1,
            "case {case:?}: expected exactly one winner, got {winners:?}"
        );
        assert_eq!(
            held_authorizations(&emulator, &bundle).await,
            winners,
            "case {case:?}: expected the bundle to hold exactly the winner's authorization"
        );
        assert_eq!(
            stored_tenant(&emulator, &bundle, &winners[0]).await,
            "race-tenant.example.com",
            "case {case:?}: expected the winner's authorization to report the raced tenant"
        );
    }
}

/// RULE (the same selection on the update path): an update that would move an
/// authorization onto an (application, tenant) pair ANOTHER authorization of the
/// bundle holds is refused with the modelled `ValidationException` (HTTP 400),
/// and it too names what the request repeats — the client id it supplies when
/// that other authorization is registered with it, and the `tenant` member it
/// supplied otherwise. Nothing is written either way.
///
/// The table spans the credential the update carries: the held registration's
/// own, another registration, an api-key authorization's key, and no credential
/// at all.
#[tokio::test]
async fn an_update_onto_a_held_pair_is_refused_for_what_the_request_repeats() {
    // (case, authType of both authorizations, the credential the update
    // supplies, whether the client-id sentence is the one that answers)
    let rows: &[(&str, &str, Option<Credential>, bool)] = &[
        (
            "the held registration's own client id",
            "oauth2",
            Some(Credential::Oauth2(SECOND)),
            true,
        ),
        (
            "the same client id, another secret beside it",
            "oauth2",
            Some(Credential::Oauth2(Oauth2 {
                client_id: SECOND.client_id,
                client_secret: "rotated-secret",
            })),
            true,
        ),
        (
            "a registration of its own",
            "oauth2",
            Some(Credential::Oauth2(FIRST)),
            false,
        ),
        ("no credential at all", "oauth2", None, false),
        (
            "an api key, which registers no client id",
            "apiKey",
            Some(Credential::ApiKey("api-key-two")),
            false,
        ),
    ];

    for (index, (row, auth_type, supplied, by_client_id)) in rows.iter().enumerate() {
        let app = APPS[index % APPS.len()];
        let case = format!("{row} (app {app}, authType {auth_type})");
        let emulator = Emulator::new();
        let bundle = arn_of(
            &emulator.create_app_bundle(json!({})).await.body,
            "appBundle",
        );
        let (moved_credential, held_credential) = if *auth_type == "apiKey" {
            (
                api_key_credential("api-key-one"),
                api_key_credential("api-key-two"),
            )
        } else {
            (oauth2_credential(FIRST), oauth2_credential(SECOND))
        };

        // The authorization the update moves, and the one already standing at
        // the tenant it is moved onto.
        let moved = create_authorization(
            &emulator,
            &bundle,
            app,
            auth_type,
            "tenant-moved",
            &moved_credential,
        )
        .await;
        assert_eq!(
            moved.status, 201,
            "case {case:?}: expected HTTP 201 for the authorization to move, got {} with body {}",
            moved.status, moved.body
        );
        let moved_arn = created_arn(&moved);
        let held = create_authorization(
            &emulator,
            &bundle,
            app,
            auth_type,
            "tenant-held",
            &held_credential,
        )
        .await;
        assert_eq!(
            held.status, 201,
            "case {case:?}: expected HTTP 201 for the authorization in the way, got {} with \
             body {}",
            held.status, held.body
        );

        let mut body = json!({"tenant": {
            "tenantIdentifier": "tenant-held",
            "tenantDisplayName": "tenant-held display",
        }});
        if let Some(credential) = supplied {
            body.as_object_mut()
                .expect("a JSON object request body")
                .insert("credential".to_string(), credential.body());
        }
        let refused = emulator
            .call(
                "PATCH",
                &format!(
                    "/appbundles/{}/appauthorizations/{}",
                    encode(&bundle),
                    encode(&moved_arn)
                ),
                Some(body),
            )
            .await;

        if *by_client_id {
            assert_state_refusal(&refused, &case, CLIENT_ID_MESSAGE);
        } else {
            assert_eq!(
                refused.status, 400,
                "case {case:?}: expected HTTP 400, got {} with body {}",
                refused.status, refused.body
            );
            assert_eq!(
                refused.error_code(),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {} with body {}",
                refused.error_code(),
                refused.body
            );
            assert_ne!(
                member(&refused, "message"),
                CLIENT_ID_MESSAGE,
                "case {case:?}: a request repeating no registration must not be refused for a \
                 client id, got {}",
                refused.body
            );
        }

        // Neither authorization moved: the refused update wrote nothing.
        assert_eq!(
            stored_tenant(&emulator, &bundle, &moved_arn).await,
            "tenant-moved",
            "case {case:?}: a refused update must leave the stored tenant alone"
        );
        assert_eq!(
            held_authorizations(&emulator, &bundle).await.len(),
            2,
            "case {case:?}: expected the bundle to still hold both authorizations"
        );
    }
}
