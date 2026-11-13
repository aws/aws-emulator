// SPDX-License-Identifier: CC0-1.0

//! How a CLIENT FAULT about a RESOURCE this control plane does not hold reaches the
//! caller, and the Multi-Party Approval (MPA) team association behind three of those
//! operations — end to end over the wire.
//!
//! Two rules, each asserted over a table that spans the class it generalises over rather
//! than the one request a report happened to cite:
//!
//! 1. the not-found class names the identifier the request asked for in its `ResourceId`
//!    member, whichever operation raised it and whichever resource kind that identifier
//!    names — a key by ARN or by alias, an alias by name, a protected operation, or an MPA
//!    approval team by ARN. (The STATUS every client fault of this service is answered at
//!    is its own rule, tested over every class in `client_error_status.rs`.)
//! 2. an MPA team association names a team AWS Multi-Party Approval really holds, is
//!    reported from the one stored record, and settles on the clock: a fresh association
//!    is `UPDATE_PENDING` and becomes `ACTIVE`, and a disassociated one is
//!    `DELETE_PENDING` until the protected operation carries no association at all.
//!
//! Every call goes through the real `MultiServiceRouter` (`POST /` with `X-Amz-Target:
//! PaymentCryptographyControlPlane.<Op>`, awsJson1_0) and the modelled-constraint walk,
//! through the same harness `key_lifecycle` uses, so what these tests assert is what a
//! signed SDK request observes.

use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::key_lifecycle::{call, error_code, ok_call, router, string, REGION};

/// The one value `MpaOperation` declares, from
/// `smithy_query(query="shape",shape="MpaOperation")`.
const PROTECTED_OPERATION: &str = "IMPORT_ROOT_PUBLIC_KEY_CERTIFICATE";

/// The canonical emulator account every ARN carries.
fn account() -> String {
    engine::emulator_account_id().to_string()
}

/// A key ARN of the form the modelled `KeyArn` pattern admits, naming no stored key.
fn absent_key_arn(id: &str) -> String {
    format!(
        "arn:aws:payment-cryptography:{REGION}:{}:key/{id}",
        account()
    )
}

/// An alias ARN of the form `KeyArnOrKeyAliasType` admits, naming no stored alias.
fn absent_alias_arn(name: &str) -> String {
    format!(
        "arn:aws:payment-cryptography:{REGION}:{}:alias/{name}",
        account()
    )
}

/// An approval-team ARN of the form the modelled `MpaTeamArn` pattern admits
/// (`^arn:aws(-[^:]+)?:mpa:[a-z0-9-]{1,20}:[0-9]{12}:approval-team/[a-zA-Z0-9._-]+$`).
fn team_arn(name: &str) -> String {
    format!("arn:aws:mpa:{REGION}:{}:approval-team/{name}", account())
}

/// Assert `(status, body)` is the not-found class naming `resource_id`.
fn assert_not_found(case: &str, status: u16, body: &Value, resource_id: &str) {
    assert_eq!(
        status, 400,
        "case {case}: the not-found class is answered at this service's client-error \
         status, got {status} {body}"
    );
    assert_eq!(
        error_code(body),
        "ResourceNotFoundException",
        "case {case}: expected ResourceNotFoundException, got {body}"
    );
    assert_eq!(
        body.get("ResourceId").and_then(Value::as_str),
        Some(resource_id),
        "case {case}: expected ResourceId {resource_id:?}, got {body}"
    );
}

/// RULE 1: every operation that reports a resource it does not hold names the identifier
/// the request asked for.
///
/// The rows differ along the operation, the resource kind the identifier names (a key by
/// ARN, a key by alias ARN, a key by bare alias name, an alias by name, a protected
/// operation's MPA association) and the form of the identifier — not one recorded request.
#[tokio::test]
async fn the_not_found_class_names_the_requested_resource() {
    let router = router();
    // (operation, the member the identifier sits in, the identifier)
    let rows: Vec<(&str, &str, String)> = vec![
        ("GetKey", "KeyIdentifier", absent_key_arn("aaaaaaaaaaaaaaaa")),
        ("GetKey", "KeyIdentifier", absent_key_arn("ZZZZ999999999999")),
        ("GetKey", "KeyIdentifier", "alias/nothing-bound".to_string()),
        (
            "GetKey",
            "KeyIdentifier",
            absent_alias_arn("nothing/bound_here-1"),
        ),
        (
            "DeleteKey",
            "KeyIdentifier",
            absent_key_arn("bbbbbbbbbbbbbbbb"),
        ),
        ("DeleteKey", "KeyIdentifier", "alias/also-nothing".to_string()),
        ("RestoreKey", "KeyIdentifier", absent_key_arn("cccccccccccccccc")),
        ("GetAlias", "AliasName", "alias/absent".to_string()),
        ("GetAlias", "AliasName", format!("alias/{}", "z".repeat(250))),
        ("DeleteAlias", "AliasName", "alias/absent-too".to_string()),
        (
            "GetMpaTeamAssociation",
            "Action",
            PROTECTED_OPERATION.to_string(),
        ),
        (
            "DisassociateMpaTeam",
            "Action",
            PROTECTED_OPERATION.to_string(),
        ),
    ];

    for (operation, member, identifier) in rows {
        let case = format!("{operation} on {identifier}");
        let (status, body) = call(&router, operation, &json!({member: identifier})).await;
        assert_not_found(&case, status, &body, &identifier);
    }

    // `AssociateMpaTeam` resolves its team in AWS Multi-Party Approval's own store, so an
    // ARN that service holds no team under is the same not-found class, naming the ARN.
    for name in ["no-such-team", "Team.Beta_2", "t"] {
        let arn = team_arn(name);
        let (status, body) = call(
            &router,
            "AssociateMpaTeam",
            &json!({"Action": PROTECTED_OPERATION, "MpaTeamArn": arn}),
        )
        .await;
        assert_not_found(&format!("AssociateMpaTeam on {arn}"), status, &body, &arn);
    }
}

/// How long a poll waits for a transition the clock owes before giving up. Generous: it
/// bounds a transition whose own span is a few seconds, and a loaded host must not fail a
/// test that is about the transition and not about the machine's speed.
const POLL_DEADLINE: Duration = Duration::from_secs(60);

/// How long a poll sleeps between looks. Short against the span it is waiting on, so the
/// loop observes the transition rather than a time of its own choosing.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// A router serving this service beside AWS Multi-Party Approval on ONE engine, with the
/// caller's account recorded as the organization's management account — the state in which
/// MPA's own `CreateApprovalTeam` is permitted.
///
/// A case that needs an approval team mints it through MPA's own API, so the team
/// `AssociateMpaTeam` resolves is one that service really holds, exactly as the assembled
/// emulator does.
fn router_with_mpa() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_paymentcryptography_paymentcryptographycontrolplane::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_mpa_awsfluffycoreservice::EmulatedService::new(Arc::clone(&engine)),
    );
    let mpa_store = service_com_amazonaws_mpa_awsfluffycoreservice::state::service_state(&*engine)
        .expect("the multi-party approval store");
    mpa_store.lock().organization_management_account_id = Some(account());
    router
}

/// Mint an approval team through MPA's own `CreateApprovalTeam` and answer its ARN.
async fn mpa_approval_team(router: &MultiServiceRouter, name: &str) -> String {
    let body = json!({
        "Name": name,
        "Description": "the payment cryptography approval team",
        "ApprovalStrategy": {"MofN": {"MinApprovalsRequired": 1}},
        "Approvers": [{
            "PrimaryIdentityId": "identity-0001",
            "PrimaryIdentitySourceArn":
                format!("arn:aws:mpa:{REGION}:{}:identity-source/ssoins-0001", account()),
        }],
        // The one policy a fresh MPA store's published catalog holds.
        "Policies": [{
            "PolicyArn": "arn:aws:mpa:::aws:policy/IdentityCenter/MPAApprovalTeamPolicy/$DEFAULT",
        }],
    });
    let request = http::Request::builder()
        .method("POST")
        .uri("/approval-teams")
        .header("host", format!("mpa.{REGION}.amazonaws.com"))
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/mpa/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        )
        .body(Bytes::from(
            serde_json::to_vec(&body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    assert_eq!(
        status, 200,
        "CreateApprovalTeam {name}: expected 200, got {status} {parsed}"
    );
    string(&parsed, "Arn").to_string()
}

/// The MPA association the protected operation carries, as `GetMpaTeamAssociation`
/// reports it.
async fn association(router: &MultiServiceRouter) -> (u16, Value) {
    call(
        router,
        "GetMpaTeamAssociation",
        &json!({"Action": PROTECTED_OPERATION}),
    )
    .await
}

/// RULE 2: an association names a team AWS Multi-Party Approval really holds, settles into
/// `ACTIVE` on the clock, and once its teardown settles the protected operation is
/// reported as carrying no association at all.
///
/// The rows differ along the team the association names, so the lifecycle is asserted over
/// the class rather than over one team: the second row re-associates the protected
/// operation with another team, which is the state the model's `UPDATE_PENDING` describes.
#[tokio::test]
async fn an_mpa_association_names_a_real_team_and_settles_on_the_clock() {
    let router = router_with_mpa();
    // The protected operation carries nothing until something associates a team.
    let (status, body) = association(&router).await;
    assert_not_found(
        "before any association",
        status,
        &body,
        PROTECTED_OPERATION,
    );

    for name in ["pcc-approvals", "Second.Team_2"] {
        let case = format!("associated with {name}");
        let arn = mpa_approval_team(&router, name).await;
        let associated = ok_call(
            &router,
            "AssociateMpaTeam",
            &json!({
                "Action": PROTECTED_OPERATION,
                "MpaTeamArn": arn,
                "RequesterComment": "please approve",
            }),
        )
        .await;
        let reported = associated
            .get("MpaTeamAssociation")
            .unwrap_or_else(|| panic!("case {case}: AssociateMpaTeam answered no association"));
        assert_eq!(
            (string(reported, "Action"), string(reported, "MpaTeamArn")),
            (PROTECTED_OPERATION, arn.as_str()),
            "case {case}: the association must name what the request asked for"
        );
        assert_eq!(
            string(reported, "AssociationState"),
            "UPDATE_PENDING",
            "case {case}: a fresh association is still settling"
        );

        // The clock owes the move to ACTIVE; it is polled for, never slept for.
        let settled = poll_for_state(&router, "ACTIVE", &case).await;
        assert_eq!(
            string(&settled, "MpaTeamArn"),
            arn,
            "case {case}: the settled association must still name the same team"
        );
    }

    let answered = ok_call(
        &router,
        "DisassociateMpaTeam",
        &json!({"Action": PROTECTED_OPERATION, "RequesterComment": "done with it"}),
    )
    .await;
    let torn_down = answered
        .get("MpaTeamAssociation")
        .expect("DisassociateMpaTeam answers the association");
    assert_eq!(
        string(torn_down, "AssociationState"),
        "DELETE_PENDING",
        "the teardown is still settling, got {torn_down}"
    );

    // Once the teardown settles the protected operation carries no association, which is
    // the not-found class naming the protected operation.
    let deadline = Instant::now() + POLL_DEADLINE;
    loop {
        let (status, body) = association(&router).await;
        if status != 200 {
            assert_not_found("after the teardown settles", status, &body, PROTECTED_OPERATION);
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the teardown never settled; the association still reads {body}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Poll `GetMpaTeamAssociation` until the association reports `state`, and answer it.
async fn poll_for_state(router: &MultiServiceRouter, state: &str, case: &str) -> Value {
    let deadline = Instant::now() + POLL_DEADLINE;
    loop {
        let (status, body) = association(router).await;
        assert_eq!(
            status, 200,
            "case {case}: the association must be readable while it settles, got {body}"
        );
        let reported = body
            .get("MpaTeamAssociation")
            .unwrap_or_else(|| panic!("case {case}: no association in {body}"))
            .clone();
        if string(&reported, "AssociationState") == state {
            return reported;
        }
        assert!(
            Instant::now() < deadline,
            "case {case}: the association never reached {state}; it reads {reported}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}
