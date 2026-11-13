// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for ApiGatewayV2 `DeletePortal`.
//!
//! Every test drives the real `restJson1` request path — `MultiServiceRouter::dispatch`
//! over a signed `http::Request` — so what is asserted is the wire behaviour a client
//! sees, not a handler called directly.
//!
//! The rules these cases assert:
//!
//! * **`DeletePortal` takes the portal its URI addresses away and answers the modelled 204
//!   with no body**, whatever publication state that portal was in: afterwards
//!   `ListPortals` no longer lists it, the account no longer holds it, every writer that
//!   addresses it is a 404, and the domains it reported are served by this service no
//!   longer — a deleted portal's endpoint is claimed by nothing.
//! * **A deleted portal stays READABLE in its terminal status**: `GetPortal` answers 200
//!   with the portal's body and `publishStatus` `DELETED`, which is how a client confirms
//!   a deletion it asked for — and once the removal the service still owes comes due the id
//!   names nothing at all, so a client polling the portal until it is gone converges.
//! * **The delete is IDEMPOTENT**: a `PortalId` the account holds no portal under — one
//!   nothing minted, and one an earlier delete already took away — is answered with the
//!   same 204 and no body, and removes nothing else.
//! * **Deleting one portal touches no other**: every sibling portal of the account is
//!   still readable, listed and unchanged, and the slot the deleted portal occupied in the
//!   account's portals quota is free as soon as it is gone.
//! * **Every other operation addressed as `/v2/portals/{PortalId}…` refuses an unresolved
//!   id** with 404 `NotFoundException` and the one sentence this API's portal plane names a
//!   missing portal with.
//!
//! The rows vary what the rules generalise over: which members the portal was created with
//! (the required ones only, an empty inclusion list, one included product, and every
//! optional member including a Cognito authorization, an ACM-managed custom domain, a
//! logo, a RUM monitor and tags), the publication state the delete meets (never published,
//! a publish still in flight, published, published and then disabled, a preview ready),
//! and how many sibling portals the account holds beside the one deleted (none, one, two).

use std::sync::Arc;
use std::thread::sleep;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_apigatewayv2_apigatewayv2::state::{
    MAX_PORTALS, MAX_PORTAL_TOMBSTONES, PORTAL_BUILD, PORTAL_DELETION, PORTAL_NOT_FOUND_MESSAGE,
    PUBLISH_STATUS_DELETED,
};
use service_com_amazonaws_apigatewayv2_apigatewayv2::ComAmazonawsApigatewayv2Apigatewayv2State;

use crate::api_lifecycle::{as_str, call, error_code};
use crate::portal_lifecycle::{custom_colors, hold_included_products, portal_content};
use crate::websocket_handshake_data_plane::send;

/// The portals collection URI (`CreatePortal`'s modelled `POST /v2/portals`); one portal
/// hangs off it as `{PORTALS}/{PortalId}`, the URI `GetPortal`, `UpdatePortal` and
/// `DeletePortal` are all bound to.
const PORTALS: &str = "/v2/portals";

/// The region [`call`] signs for, mirrored here because an ARN a request carries has to
/// name the same region the request is signed for to be accepted.
const REGION: &str = "eu-west-1";

/// The emulator's canonical account id, the account segment of every ARN it emits.
const ACCOUNT_ID: &str = "000000000000";

/// The sentence this API's portal plane answers a `PortalId` that resolves to nothing
/// with, spelled out here rather than taken from the crate's constant: a test that asserts
/// a handler's own constant back at it cannot catch the wording drifting away from the
/// service's.
const PORTAL_NOT_FOUND_SENTENCE: &str = "Portal not found";

/// The error code the front door answers when NO service claims a request: what a deleted
/// portal's domain must answer, because the portal that made this service claim that host
/// is gone.
const UNKNOWN_SERVICE: &str = "UnknownService";

/// How long to wait so a build the service owes has certainly come due: its span plus
/// slack. A BLOCKING wait on purpose — this workspace's `tokio` carries no `time` feature.
fn past_the_build() -> Duration {
    PORTAL_BUILD + Duration::from_millis(400)
}

/// A fresh emulator holding only this service, plus the engine it was built from, so a
/// test can assert on the store itself — what a delete removed, and what a refused delete
/// must NOT have removed.
fn emulator_with_store() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_apigatewayv2_apigatewayv2::EmulatedService::new(Arc::clone(&engine)),
    );
    (router, engine)
}

/// The `PortalId`s the store itself holds as portals of the account — the live records,
/// not the tombstone a delete leaves behind for `GetPortal` to report.
fn stored_portals(engine: &engine::InMemoryEngine) -> Vec<String> {
    engine
        .state::<ComAmazonawsApigatewayv2Apigatewayv2State>()
        .lock()
        .live_portals()
        .map(|portal| portal.portal_id.clone())
        .collect()
}

/// A portal-product ARN of the form `CreatePortalProduct` mints: this service's ARN
/// namespace and the `/portalproducts/{id}` resource path.
fn product_arn(id: &str) -> String {
    format!("arn:aws:apigateway:{REGION}:{ACCOUNT_ID}:/portalproducts/{id}")
}

/// A minimal create request: a portal nobody authorizes, on API Gateway's own domain.
fn minimal(display_name: &str) -> Value {
    json!({
        "authorization": { "none": {} },
        "endpointConfiguration": { "none": {} },
        "portalContent": portal_content(display_name)
    })
}

/// What a client needs to address one created portal: its id and the domains it reports.
struct Created {
    portal_id: String,
    default_domain: String,
    custom_domain: Option<String>,
}

/// Create one portal, assert the create was accepted, and answer what addresses it.
async fn create_portal(emulator: &MultiServiceRouter, case: &str, request: Value) -> Created {
    let (status, created) = call(emulator, "POST", PORTALS, Some(request)).await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected 201 from CreatePortal, got {status} {created}"
    );
    let endpoint = created
        .get("endpointConfiguration")
        .cloned()
        .unwrap_or(Value::Null);
    Created {
        portal_id: as_str(&created, "portalId"),
        default_domain: as_str(&endpoint, "portalDefaultDomainName"),
        custom_domain: endpoint
            .get("domainName")
            .and_then(Value::as_str)
            .map(str::to_string),
    }
}

/// The `PortalId`s `ListPortals` reports, walked to the end of its pages.
async fn listed_portals(emulator: &MultiServiceRouter, case: &str) -> Vec<String> {
    let mut ids = Vec::new();
    let mut token: Option<String> = Option::None;
    loop {
        let uri = match &token {
            Some(next) => format!("{PORTALS}?nextToken={next}"),
            Option::None => PORTALS.to_string(),
        };
        let (status, body) = call(emulator, "GET", &uri, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from ListPortals, got {status} {body}"
        );
        for item in body["items"].as_array().cloned().unwrap_or_default() {
            ids.push(as_str(&item, "portalId"));
        }
        token = body
            .get("nextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        if token.is_none() {
            return ids;
        }
    }
}

/// The publication state the delete under test meets.
#[derive(Debug, Clone, Copy)]
enum At {
    /// Created and left alone: a portal nothing has published.
    NeverPublished,
    /// `PublishPortal` called and its build still in flight.
    PublishInFlight,
    /// Published, with the build the publish started already settled.
    Published,
    /// Published and then taken down again by `DisablePortal`.
    Disabled,
    /// Previewed, with the preview build already settled.
    PreviewReady,
}

impl At {
    /// Drive the portal into this state.
    async fn reach(self, emulator: &MultiServiceRouter, case: &str, portal_id: &str) {
        let publish = format!("{PORTALS}/{portal_id}/publish");
        let preview = format!("{PORTALS}/{portal_id}/preview");
        match self {
            At::NeverPublished => {}
            At::PublishInFlight => {
                accepted(emulator, case, "POST", &publish, Some(json!({})), 202).await;
            }
            At::Published => {
                accepted(emulator, case, "POST", &publish, Some(json!({})), 202).await;
                sleep(past_the_build());
            }
            At::Disabled => {
                accepted(emulator, case, "POST", &publish, Some(json!({})), 202).await;
                sleep(past_the_build());
                accepted(emulator, case, "DELETE", &publish, Option::None, 204).await;
                sleep(past_the_build());
            }
            At::PreviewReady => {
                accepted(emulator, case, "POST", &preview, Option::None, 202).await;
                sleep(past_the_build());
            }
        }
    }
}

/// Send one call that is expected to be accepted with `status`.
async fn accepted(
    emulator: &MultiServiceRouter,
    case: &str,
    method: &str,
    uri: &str,
    body: Option<Value>,
    status: u16,
) {
    let (answered, answer) = call(emulator, method, uri, body).await;
    assert_eq!(
        answered, status,
        "case {case:?}: expected {status} from {method} {uri}, got {answered} {answer}"
    );
}

/// One delete row: the portal to delete, the state the delete meets it in, and the sibling
/// portals the account holds beside it.
struct Row {
    case: &'static str,
    create: Value,
    at: At,
    siblings: Vec<Value>,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the only portal of the account, with only the required members, \
                   never published",
            create: minimal("The Only Portal"),
            at: At::NeverPublished,
            siblings: vec![],
        },
        Row {
            case: "a Cognito-authorized portal on its own ACM-managed domain with every \
                   optional member set, published",
            create: json!({
                "authorization": {
                    "cognitoConfig": {
                        "appClientId": "1h57kf5cpq17m0eml12eqemjq1",
                        "userPoolArn": format!("arn:aws:cognito-idp:{REGION}:{ACCOUNT_ID}:userpool/{REGION}_pooL1"),
                        "userPoolDomain": "https://portal-users.auth.eu-west-1.amazoncognito.com"
                    }
                },
                "endpointConfiguration": {
                    "acmManaged": {
                        "certificateArn": format!("arn:aws:acm:{REGION}:{ACCOUNT_ID}:certificate/8f4c1b1e-2a1d-4a57-9c0e-1f7b2a3d4e5f"),
                        "domainName": "developer.example.com"
                    }
                },
                "portalContent": {
                    "displayName": "Example Developer Portal",
                    "description": "every optional member set",
                    "theme": { "customColors": custom_colors() }
                },
                "includedPortalProductArns": [product_arn("prodaaaaaa"), product_arn("prodbbbbbb")],
                "logoUri": "s3://example-portal-assets/logo.png",
                "rumAppMonitorName": "example-portal-monitor",
                "tags": { "team": "developer-experience", "env": "test" }
            }),
            at: At::Published,
            siblings: vec![minimal("Sibling One"), minimal("Sibling Two")],
        },
        Row {
            case: "a portal including one product, deleted while its publish is still in \
                   flight",
            create: json!({
                "authorization": { "none": {} },
                "endpointConfiguration": { "none": {} },
                "portalContent": portal_content("One Product"),
                "includedPortalProductArns": [product_arn("prodcccccc")]
            }),
            at: At::PublishInFlight,
            siblings: vec![minimal("Sibling One")],
        },
        Row {
            case: "a portal whose publication was taken down again, with an empty \
                   inclusion list",
            create: json!({
                "authorization": { "none": {} },
                "endpointConfiguration": { "none": {} },
                "portalContent": portal_content("Empty Inclusions"),
                "includedPortalProductArns": [],
                "tags": {}
            }),
            at: At::Disabled,
            siblings: vec![minimal("Sibling One")],
        },
        Row {
            case: "a portal whose preview is ready",
            create: json!({
                "authorization": { "none": {} },
                "endpointConfiguration": { "none": {} },
                "portalContent": portal_content("Previewed Portal"),
                "logoUri": "https://assets.example.com/logo.svg"
            }),
            at: At::PreviewReady,
            siblings: vec![minimal("Sibling One"), minimal("Sibling Two")],
        },
    ]
}

#[tokio::test]
async fn delete_portal_takes_the_portal_away_and_leaves_it_readable_as_deleted() {
    // The sentence the handlers answer with is built from the service's own constant, so
    // it is asserted to BE the literal this suite expects and cannot drift from it.
    assert_eq!(
        PORTAL_NOT_FOUND_MESSAGE, PORTAL_NOT_FOUND_SENTENCE,
        "expected the service's missing-portal sentence to be {PORTAL_NOT_FOUND_SENTENCE:?}, \
         got {PORTAL_NOT_FOUND_MESSAGE:?}"
    );

    for mut row in rows() {
        let case = row.case;
        let (emulator, engine) = emulator_with_store();

        // The siblings first, so the portal under test is neither the first nor the last
        // key of the store in every row.
        let mut sibling_ids = Vec::new();
        for sibling in &row.siblings {
            sibling_ids.push(
                create_portal(&emulator, case, sibling.clone())
                    .await
                    .portal_id,
            );
        }
        // A portal may only include products the account HOLDS, so this row's products are
        // created before its portal is.
        let _held = hold_included_products(&emulator, case, &mut row.create).await;
        let created = create_portal(&emulator, case, row.create.clone()).await;
        let portal_id = created.portal_id.clone();
        let uri = format!("{PORTALS}/{portal_id}");
        row.at.reach(&emulator, case, &portal_id).await;

        // Before the delete this service answers for the portal's domains: it is a host
        // this service claims BECAUSE a portal records it.
        for domain in [Some(created.default_domain.clone()), created.custom_domain.clone()]
            .into_iter()
            .flatten()
        {
            let answer = send(&emulator, &domain, "GET", "/", &[]).await;
            assert_ne!(
                answer.error_type.as_deref(),
                Some(UNKNOWN_SERVICE),
                "case {case:?}: before the delete, {domain:?} must be served by this service, \
                 got {}",
                answer.summary()
            );
        }

        // The delete is accepted with the modelled 204 and reports nothing: the response
        // structure the model declares for it carries no members at all.
        let (status, body) = call(&emulator, "DELETE", &uri, None).await;
        assert_eq!(
            status, 204,
            "case {case:?}: expected 204 from DeletePortal, got {status} {body}"
        );
        assert!(
            body.is_null() || body.as_object().is_some_and(serde_json::Map::is_empty),
            "case {case:?}: expected DeletePortal to report no members, got {body}"
        );

        // The portal is gone as a portal of the account: nothing lists it and no writer
        // resolves it any more.
        let listed = listed_portals(&emulator, case).await;
        assert!(
            !listed.contains(&portal_id),
            "case {case:?}: expected ListPortals not to list the deleted {portal_id:?}, \
             got {listed:?}"
        );
        let stored = stored_portals(&engine);
        assert!(
            !stored.contains(&portal_id),
            "case {case:?}: expected the account not to hold the deleted {portal_id:?}, \
             got {stored:?}"
        );
        for (operation, method, addressed, request) in [
            (
                "UpdatePortal",
                "PATCH",
                uri.clone(),
                Some(json!({ "portalContent": portal_content("Renamed") })),
            ),
            (
                "PublishPortal",
                "POST",
                format!("{uri}/publish"),
                Some(json!({})),
            ),
            ("DisablePortal", "DELETE", format!("{uri}/publish"), None),
            ("PreviewPortal", "POST", format!("{uri}/preview"), None),
        ] {
            let (status, answer) = call(&emulator, method, &addressed, request).await;
            assert_eq!(
                status, 404,
                "case {case:?}: expected 404 from {operation} on the deleted portal, got \
                 {status} {answer}"
            );
            assert_eq!(
                as_str(&answer, "message"),
                PORTAL_NOT_FOUND_SENTENCE,
                "case {case:?}: expected the sentence this API answers a missing portal with \
                 from {operation}, got {answer}"
            );
        }

        // But the portal is still READABLE, in the terminal status a deleted portal
        // carries: that is how a client confirms the deletion it asked for. Its body is
        // the one it had, and the preview a deleted portal has is none.
        let (status, read) = call(&emulator, "GET", &uri, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from GetPortal after the delete, got {status} {read}"
        );
        assert_eq!(
            as_str(&read, "publishStatus"),
            PUBLISH_STATUS_DELETED,
            "case {case:?}: expected the deleted portal to report {PUBLISH_STATUS_DELETED}, \
             got {read}"
        );
        assert_eq!(
            as_str(&read, "portalId"),
            portal_id,
            "case {case:?}: expected the deleted portal's own record, got {read}"
        );
        assert_eq!(
            read.get("preview"),
            Some(&json!({})),
            "case {case:?}: expected a deleted portal to report the empty preview, got {read}"
        );
        // And reading it again answers the same: a deleted portal is a terminal state,
        // not a countdown.
        let (status, again) = call(&emulator, "GET", &uri, None).await;
        assert_eq!(
            (status, &again),
            (200, &read),
            "case {case:?}: expected a second read of the deleted portal to answer the \
             same, got {status} {again}"
        );

        // The domains the portal reported are claimed by nothing now: the record that made
        // this service answer for them is gone, whatever it was serving before.
        for domain in [Some(created.default_domain.clone()), created.custom_domain.clone()]
            .into_iter()
            .flatten()
        {
            let answer = send(&emulator, &domain, "GET", "/", &[]).await;
            assert_eq!(
                answer.error_type.as_deref(),
                Some(UNKNOWN_SERVICE),
                "case {case:?}: after the delete, {domain:?} must be served by nothing, got {}",
                answer.summary()
            );
        }

        // Deleting one portal touched no other: every sibling is readable and listed.
        for sibling_id in &sibling_ids {
            let (status, read) =
                call(&emulator, "GET", &format!("{PORTALS}/{sibling_id}"), None).await;
            assert_eq!(
                status, 200,
                "case {case:?} (sibling {sibling_id:?}): expected 200 from GetPortal after the \
                 delete, got {status} {read}"
            );
            assert_eq!(
                as_str(&read, "portalId"),
                *sibling_id,
                "case {case:?} (sibling {sibling_id:?}): expected the sibling's own record, \
                 got {read}"
            );
            assert!(
                listed.contains(sibling_id),
                "case {case:?}: expected ListPortals to still list the sibling {sibling_id:?}, \
                 got {listed:?}"
            );
        }
        assert_eq!(
            stored.len(),
            sibling_ids.len(),
            "case {case:?}: expected only the {} sibling portal(s) left in the store, got {stored:?}",
            sibling_ids.len()
        );
    }
}

#[tokio::test]
async fn deleting_a_portal_the_account_does_not_hold_is_the_same_204() {
    let (emulator, engine) = emulator_with_store();

    // One portal survives, so a miss is a miss on the identifier and not on an empty
    // store.
    let known = create_portal(&emulator, "the surviving portal", minimal("Surviving"))
        .await
        .portal_id;

    // And one portal that has already been deleted: its id names nothing the account
    // holds any more, so a second delete of it asks for the state that already holds.
    let gone = create_portal(&emulator, "the portal deleted first", minimal("Gone"))
        .await
        .portal_id;
    accepted(
        &emulator,
        "the first delete of the portal deleted first",
        "DELETE",
        &format!("{PORTALS}/{gone}"),
        Option::None,
        204,
    )
    .await;

    for (case, portal_id) in [
        ("an id nothing minted", "nosuchprtl".to_string()),
        (
            "an id that is not of the minted form",
            "no-such-portal".to_string(),
        ),
        (
            "the surviving id with its last character changed",
            // Map the last character onto one it is not, so the row is a miss whatever
            // the id bijection mints.
            format!(
                "{}{}",
                &known[..known.len() - 1],
                if known.ends_with('z') { 'a' } else { 'z' }
            ),
        ),
        ("the id of a portal already deleted", gone.clone()),
        // And again: repeating the call cannot make it answer differently.
        ("the id of a portal already deleted, once more", gone.clone()),
    ] {
        let (status, body) = call(
            &emulator,
            "DELETE",
            &format!("{PORTALS}/{portal_id}"),
            None,
        )
        .await;
        assert_eq!(
            status, 204,
            "case {case:?}: expected 204 from DeletePortal, got {status} {body}"
        );
        assert!(
            body.is_null() || body.as_object().is_some_and(serde_json::Map::is_empty),
            "case {case:?}: expected DeletePortal to report no members, got {body}"
        );
        // A delete of a portal the account does not hold removes nothing: the surviving
        // portal is still the one portal the account holds.
        assert_eq!(
            stored_portals(&engine),
            vec![known.clone()],
            "case {case:?}: a DeletePortal answering for a portal the account does not \
             hold must remove nothing"
        );
    }

    // The portal deleted first is still readable in its terminal status, however many
    // deletes were addressed at it.
    let (status, read) = call(&emulator, "GET", &format!("{PORTALS}/{gone}"), None).await;
    assert_eq!(
        status, 200,
        "expected the deleted portal to stay readable, got {status} {read}"
    );
    assert_eq!(
        as_str(&read, "publishStatus"),
        PUBLISH_STATUS_DELETED,
        "expected the deleted portal to report {PUBLISH_STATUS_DELETED}, got {read}"
    );
}

#[tokio::test]
async fn every_operation_addressing_a_portal_refuses_an_unresolved_id_the_same_way() {
    let (emulator, _engine) = emulator_with_store();

    // One portal exists, so a miss is a miss on the identifier and not on an empty store.
    let known = create_portal(&emulator, "the surviving portal", minimal("Surviving"))
        .await
        .portal_id;

    // Every operation that resolves a `PortalId` out of its URI: the reader, the patch,
    // and the three calls that start a server-managed build. `DeletePortal` is not among
    // them — it answers 204 for an id the account holds no portal under, which
    // `deleting_a_portal_the_account_does_not_hold_is_the_same_204` asserts.
    for (operation, method, uri, body) in [
        ("GetPortal", "GET", format!("{PORTALS}/{{id}}"), None),
        (
            "UpdatePortal",
            "PATCH",
            format!("{PORTALS}/{{id}}"),
            Some(json!({ "portalContent": portal_content("Renamed") })),
        ),
        (
            "PublishPortal",
            "POST",
            format!("{PORTALS}/{{id}}/publish"),
            Some(json!({})),
        ),
        (
            "DisablePortal",
            "DELETE",
            format!("{PORTALS}/{{id}}/publish"),
            None,
        ),
        (
            "PreviewPortal",
            "POST",
            format!("{PORTALS}/{{id}}/preview"),
            None,
        ),
    ] {
        for (case, portal_id) in [
            ("an id nothing minted", "nosuchprtl".to_string()),
            (
                "an id that is not of the minted form",
                "no-such-portal".to_string(),
            ),
            (
                "the surviving id with its last character changed",
                format!(
                    "{}{}",
                    &known[..known.len() - 1],
                    if known.ends_with('z') { 'a' } else { 'z' }
                ),
            ),
        ] {
            let addressed = uri.replace("{id}", &portal_id);
            let (status, answer) = call(&emulator, method, &addressed, body.clone()).await;
            assert_eq!(
                status, 404,
                "case {operation} with {case:?}: expected 404, got {status} {answer}"
            );
            assert_eq!(
                error_code(&answer),
                "NotFoundException",
                "case {operation} with {case:?}: expected NotFoundException, got {answer}"
            );
            assert_eq!(
                as_str(&answer, "message"),
                PORTAL_NOT_FOUND_SENTENCE,
                "case {operation} with {case:?}: expected the sentence this API answers a \
                 missing portal with, got {answer}"
            );
        }
    }

    // The surviving portal is untouched by every refusal above.
    let (status, read) = call(&emulator, "GET", &format!("{PORTALS}/{known}"), None).await;
    assert_eq!(
        status, 200,
        "expected the surviving portal to still read back, got {status} {read}"
    );
}

#[tokio::test]
async fn a_deleted_portals_quota_slot_is_free_at_once() {
    let (emulator, engine) = emulator_with_store();

    let mut ids = Vec::new();
    for index in 0..MAX_PORTALS {
        ids.push(
            create_portal(
                &emulator,
                "filling the account to its quota",
                minimal(&format!("Portal {index}")),
            )
            .await
            .portal_id,
        );
    }

    // At the quota a create is refused, and the refusal is the modelled "a limit has been
    // exceeded".
    let (status, body) = call(
        &emulator,
        "POST",
        PORTALS,
        Some(minimal("One Too Many")),
    )
    .await;
    assert_eq!(
        status, 429,
        "case \"at the quota\": expected 429 from CreatePortal, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "TooManyRequestsException",
        "case \"at the quota\": expected TooManyRequestsException, got {body}"
    );

    // A delete frees the slot it occupied straight away — the portal is gone when the
    // delete answers, not after a span — so the next create is accepted, with an id of
    // its own rather than the one that was deleted.
    let deleted = ids.first().expect("the quota is at least one portal").clone();
    accepted(
        &emulator,
        "deleting one portal of a full account",
        "DELETE",
        &format!("{PORTALS}/{deleted}"),
        Option::None,
        204,
    )
    .await;
    let replacement = create_portal(
        &emulator,
        "a create once the deleted portal has gone",
        minimal("One Too Many"),
    )
    .await
    .portal_id;
    assert!(
        !ids.contains(&replacement),
        "case \"a create once the deleted portal has gone\": expected a PortalId no earlier \
         portal carried, got {replacement:?}"
    );
    let stored = stored_portals(&engine);
    assert_eq!(
        stored.len(),
        MAX_PORTALS,
        "case \"a create once the deleted portal has gone\": expected the account to hold \
         {MAX_PORTALS} portals, got {}",
        stored.len()
    );
    assert!(
        !stored.contains(&deleted),
        "case \"a create once the deleted portal has gone\": expected the deleted \
         {deleted:?} to stay gone, got {stored:?}"
    );
}

/// The store keeps the tombstone of a deleted portal so `GetPortal` can report it, and it
/// keeps at most [`MAX_PORTAL_TOMBSTONES`] of them: a loop of create-and-delete cannot grow
/// the store without end, whatever the removals the clock has not finished yet.
#[tokio::test]
async fn the_store_keeps_a_bounded_number_of_deleted_portals() {
    let (emulator, engine) = emulator_with_store();

    // More create-and-delete pairs than the store keeps tombstones for, so the bound has
    // to be enforced however fast the loop runs.
    for index in 0..=MAX_PORTAL_TOMBSTONES {
        let portal_id = create_portal(
            &emulator,
            "a portal created and deleted",
            minimal(&format!("Portal {index}")),
        )
        .await
        .portal_id;
        accepted(
            &emulator,
            "a portal created and deleted",
            "DELETE",
            &format!("{PORTALS}/{portal_id}"),
            Option::None,
            204,
        )
        .await;
        let held = engine
            .state::<ComAmazonawsApigatewayv2Apigatewayv2State>()
            .lock()
            .portals
            .len();
        assert!(
            held <= MAX_PORTAL_TOMBSTONES,
            "case \"delete {index}\": expected the store to keep at most \
             {MAX_PORTAL_TOMBSTONES} records, got {held}"
        );
    }

    assert!(
        stored_portals(&engine).is_empty(),
        "expected the account to hold no portal at all, got {:?}",
        stored_portals(&engine)
    );
}

/// Removing a portal is work the SERVICE finishes after the delete has answered: the portal
/// reads back in its terminal `DELETED` status until that removal comes due, and the id
/// names nothing afterwards — so a client polling the portal until it is gone converges.
///
/// The rows vary the state the delete met the portal in, because the removal is owed
/// whatever the portal was doing.
#[tokio::test]
async fn a_deleted_portal_stops_being_readable_once_its_removal_comes_due() {
    for (case, at) in [
        ("never published", At::NeverPublished),
        ("published", At::Published),
        ("a preview ready", At::PreviewReady),
    ] {
        let (emulator, engine) = emulator_with_store();
        let portal_id = create_portal(&emulator, case, minimal("Removed Portal"))
            .await
            .portal_id;
        at.reach(&emulator, case, &portal_id).await;
        let uri = format!("{PORTALS}/{portal_id}");

        accepted(&emulator, case, "DELETE", &uri, Option::None, 204).await;

        // Straight after the delete the portal is readable in its terminal status: the
        // removal is not finished yet.
        let (status, read) = call(&emulator, "GET", &uri, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the deleted portal to still read back, got {status} \
             {read}"
        );
        assert_eq!(
            as_str(&read, "publishStatus"),
            PUBLISH_STATUS_DELETED,
            "case {case:?}: expected {PUBLISH_STATUS_DELETED}, got {read}"
        );

        // Once the removal has come due the id names nothing, with nothing but the clock
        // having happened — and the record is not kept either.
        sleep(PORTAL_DELETION + Duration::from_millis(400));
        let (status, gone) = call(&emulator, "GET", &uri, None).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404 once the removal had come due, got {status} {gone}"
        );
        assert_eq!(
            error_code(&gone),
            "NotFoundException",
            "case {case:?}: expected NotFoundException, got {gone}"
        );
        assert_eq!(
            as_str(&gone, "message"),
            PORTAL_NOT_FOUND_SENTENCE,
            "case {case:?}: expected the sentence this API answers a missing portal with, \
             got {gone}"
        );

        // And the delete still answers the same 204 for it.
        accepted(&emulator, case, "DELETE", &uri, Option::None, 204).await;
        assert!(
            stored_portals(&engine).is_empty(),
            "case {case:?}: expected the account to hold no portal, got {:?}",
            stored_portals(&engine)
        );
    }
}
