// SPDX-License-Identifier: CC0-1.0

//! The two rules that make a directory registration a REAL authorization of an
//! Active Directory: what it settles on, and what is left of it after a delete.
//!
//! 1. A registration reaches `ACTIVE` only for a directory the account HOLDS.
//!    `CreateDirectoryRegistration` answers 202 and the authorization it starts acts
//!    on a directory of AWS Directory Service — a resource this service creates none
//!    of — so a registration naming a directory that plane has no record of
//!    terminates in `FAILED` carrying `DIRECTORY_RESOURCE_NOT_FOUND`, and every
//!    reporting operation shows that same terminal state.
//!    (`DirectoryRegistrationStatus` = [CREATING, ACTIVE, DELETING, FAILED] and
//!    `DirectoryRegistrationStatusReason` carries `DIRECTORY_RESOURCE_NOT_FOUND`:
//!    `smithy_query(query="shape",
//!    shape="DirectoryRegistrationStatus,DirectoryRegistrationStatusReason")`.)
//! 2. `DeleteDirectoryRegistration` (DELETE, 202, `@idempotent`) deauthorizes it: the
//!    registration moves to `DELETING` and then STOPS EXISTING, after which
//!    `GetDirectoryRegistration` refuses the ARN with 404
//!    `ResourceNotFoundException` and the listing no longer carries it.
//!
//! Directories belong to Directory Service, so that service is mounted beside this
//! one on ONE engine and every directory a case needs is created and deleted through
//! its OWN front door. Nothing here reaches into either store except to move a DUE
//! INSTANT the clock is waiting on — never a status, which the handlers still
//! compute from the records themselves.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_pcaconnectorad_pcaconnectorad as pcad;
use pcad::state::{
    DIRECTORY_REGISTRATION_ACTIVATION_DELAY, DIRECTORY_REGISTRATION_DELETION_DELAY,
};

// One wire harness per service: the request builder, the registration helpers and
// the reply reader come from the sibling suites rather than a second copy here.
use super::directory_registration_lifecycle::{
    create_registration, get_registration, list_page, registration_arn_prefix, set_settle_due,
};
use super::harness::{call, encode_label, REGION};

/// The `X-Amz-Target` prefix the Directory Service front door routes on: that
/// service's own shape name.
const DS_TARGET_PREFIX: &str = "DirectoryService_20150416";

/// A password satisfying the Directory Service `Password` pattern (8-64 characters
/// with a digit, upper case, lower case and a special character). Nothing asserts on
/// it — no operation reports it back — and it exists only so a create is well formed.
const DIRECTORY_PASSWORD: &str = "Passw0rd!2345";

/// Directory names a case creates its directories under. Each satisfies the
/// `DirectoryName` pattern `^([a-zA-Z0-9]+[\\.-])+([a-zA-Z0-9])+$`
/// (`smithy_query(query="operation",operation="CreateDirectory")` of the Directory
/// Service model) and each is distinct, so no two cases share a directory.
const DIRECTORY_NAMES: &[&str] = &["corp.example.com", "ad-1.example.org", "team.example.test"];

/// Directory ids NO directory was ever created for, spanning the modelled
/// `DirectoryId` pattern `^d-[0-9a-f]{10}$`
/// (`smithy_query(query="shape",shape="DirectoryId")`): digits only, hex letters
/// only, the two extremes of the value range, and a mixture.
const ABSENT_DIRECTORY_IDS: &[&str] = &[
    "d-0123456789",
    "d-abcdefabcd",
    "d-0000000000",
    "d-ffffffffff",
    "d-1a2b3c4d5e",
];

/// The `StatusReason` a registration of an unheld directory terminates with.
const REASON_NOT_FOUND: &str = "DIRECTORY_RESOURCE_NOT_FOUND";

/// Far enough out that no case outlives it, so a call made while it stands observes
/// the transient status.
const NOT_DUE_FOR: Duration = Duration::from_secs(3600);

/// Any past instant is due, so the next call applies the move the clock owed.
const ALREADY_DUE: Duration = Duration::from_secs(1);

/// How long a case that waits on the REAL clock gives a move past its own span, so a
/// slow machine does not read the status one instant early.
const CLOCK_MARGIN: Duration = Duration::from_millis(750);

/// This service and the owner of directories over ONE shared engine — the same
/// engine the production router builds — plus that engine, for a case that needs a
/// clock-owed instant in a known position instead of racing it.
///
/// `pub(crate)` because the service-principal-name suite needs the same two services
/// for the same reason: whether a name can be written into a directory at all depends
/// on a directory THAT service creates and deletes.
pub(crate) fn fleet() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Signature verification is the emulator's own concern and is exercised
    // elsewhere; these cases are about the two services' behaviour.
    router.set_request_authentication(false);
    router.register_service(pcad::EmulatedService::new(Arc::clone(&engine)));
    router.register_service(
        service_com_amazonaws_directoryservice_directoryservice_20150416::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    (router, engine)
}

/// One `awsJson1_1` call to the Directory Service front door, as a client makes it:
/// the HTTP status and the parsed body.
async fn ds_call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://ds.{REGION}.amazonaws.com/"))
        .header("host", format!("ds.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("{DS_TARGET_PREFIX}.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/ds/aws4_request, \
                 SignedHeaders=host;x-amz-target, Signature=0123456789abcdef"
            ),
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = response.into_body();
    let parsed: Value = if payload.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&payload).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// Create a directory through Directory Service itself and answer the id it minted.
pub(crate) async fn create_directory(router: &MultiServiceRouter, name: &str) -> String {
    let body = json!({ "Name": name, "Password": DIRECTORY_PASSWORD, "Size": "Small" });
    let (status, parsed) = ds_call(router, "CreateDirectory", &body).await;
    assert_eq!(
        status, 200,
        "setup {name}: Directory Service must create the directory, got {status} / {parsed}"
    );
    parsed["DirectoryId"]
        .as_str()
        .unwrap_or_else(|| panic!("setup {name}: the create answered no DirectoryId: {parsed}"))
        .to_string()
}

/// Delete a directory through Directory Service itself.
pub(crate) async fn delete_directory(router: &MultiServiceRouter, directory_id: &str) {
    let (status, parsed) =
        ds_call(router, "DeleteDirectory", &json!({ "DirectoryId": directory_id })).await;
    assert_eq!(
        status, 200,
        "setup {directory_id}: Directory Service must delete the directory, \
         got {status} / {parsed}"
    );
}

/// `DeleteDirectoryRegistration` for `arn`: the HTTP status and the parsed body.
async fn delete_registration(router: &MultiServiceRouter, arn: &str) -> (u16, Value) {
    let (status, _, parsed) = call(
        router,
        "DELETE",
        &format!("/directoryRegistrations/{}", encode_label(arn)),
        None,
    )
    .await;
    (status, parsed)
}

/// The listing's summary of `arn`, or `None` once the listing no longer carries it.
async fn listed_summary(router: &MultiServiceRouter, arn: &str) -> Option<Value> {
    let (status, _, page) = list_page(router, None, None).await;
    assert_eq!(
        status, 200,
        "ListDirectoryRegistrations must answer 200, got {status} / {page}"
    );
    page["DirectoryRegistrations"]
        .as_array()
        .and_then(|entries| {
            entries
                .iter()
                .find(|summary| summary["Arn"].as_str() == Some(arn))
                .cloned()
        })
}

/// Move the instant the clock owes the registration `arn` its REMOVAL to `due_at`,
/// through the service's own state. Strictly a clock move: only the due instant is
/// rewritten.
pub(crate) fn set_deleted_due(engine: &engine::InMemoryEngine, arn: &str, due_at: SystemTime) {
    let shared = pcad::state::service_state(engine).expect("the service's own state");
    let mut store = shared.lock();
    let record = store
        .directory_registrations
        .get_mut(arn)
        .unwrap_or_else(|| panic!("no directory registration is stored under {arn:?}"));
    record.deleted_at = Some(due_at);
}

/// One row of the terminal-state table: how the directory stands with the service
/// that owns directories, and what the registration of it must settle on.
struct Row {
    case: String,
    directory_id: String,
    status: &'static str,
    reason: Option<&'static str>,
}

#[tokio::test]
async fn a_registration_settles_active_only_for_a_directory_the_account_holds() {
    let (router, engine) = fleet();

    // The rows differ along the one thing the rule generalises over: whether the
    // account holds the directory the registration names — held, held and then
    // deleted, and never created at all — across the forms a directory id takes.
    let mut rows: Vec<Row> = Vec::new();
    for name in DIRECTORY_NAMES {
        let directory_id = create_directory(&router, name).await;
        rows.push(Row {
            case: format!("the directory created as {name} ({directory_id})"),
            directory_id,
            status: "ACTIVE",
            reason: None,
        });
    }
    for name in DIRECTORY_NAMES {
        let deleted_name = format!("gone-{name}");
        let directory_id = create_directory(&router, &deleted_name).await;
        delete_directory(&router, &directory_id).await;
        rows.push(Row {
            case: format!("the directory deleted at {deleted_name} ({directory_id})"),
            directory_id,
            status: "FAILED",
            reason: Some(REASON_NOT_FOUND),
        });
    }
    for directory_id in ABSENT_DIRECTORY_IDS {
        rows.push(Row {
            case: format!("the id {directory_id}, which no directory was created for"),
            directory_id: (*directory_id).to_string(),
            status: "FAILED",
            reason: Some(REASON_NOT_FOUND),
        });
    }

    for row in rows {
        let Row {
            case,
            directory_id,
            status,
            reason,
        } = row;
        let arn = create_registration(&router, &directory_id).await;
        assert_eq!(
            arn,
            format!("{}{directory_id}", registration_arn_prefix()),
            "case {case:?}: the create must mint the ARN naming this directory, got {arn:?}"
        );

        // Before the authorization is due every row reads alike: the CREATING the
        // 202 reported, with no StatusReason.
        set_settle_due(&engine, &arn, SystemTime::now() + NOT_DUE_FOR);
        let (held_status, _, held) = get_registration(&router, &arn).await;
        assert_eq!(
            held_status, 200,
            "case {case:?}: GetDirectoryRegistration must answer 200, got {held_status} / {held}"
        );
        assert_eq!(
            held["DirectoryRegistration"]["Status"].as_str(),
            Some("CREATING"),
            "case {case:?}: a registration whose authorization is not due is CREATING, got {held}"
        );
        assert!(
            held["DirectoryRegistration"]["StatusReason"].is_null(),
            "case {case:?}: a registration that has not failed carries no StatusReason, got {held}"
        );

        // Once it is due, the read path reports the state the authorization reached.
        set_settle_due(&engine, &arn, SystemTime::now() - ALREADY_DUE);
        let (settled_status, _, settled) = get_registration(&router, &arn).await;
        assert_eq!(
            settled_status, 200,
            "case {case:?}: GetDirectoryRegistration must answer 200, \
             got {settled_status} / {settled}"
        );
        let registration = &settled["DirectoryRegistration"];
        assert_eq!(
            registration["Status"].as_str(),
            Some(status),
            "case {case:?}: expected Status {status}, got {registration}"
        );
        assert_eq!(
            registration["StatusReason"].as_str(),
            reason,
            "case {case:?}: expected StatusReason {reason:?}, got {registration}"
        );

        // And the listing reports the SAME terminal state for the same resource.
        let summary = listed_summary(&router, &arn)
            .await
            .unwrap_or_else(|| panic!("case {case:?}: the listing must carry {arn}"));
        for member in ["Arn", "DirectoryId", "Status", "StatusReason"] {
            assert_eq!(
                summary[member], registration[member],
                "case {case:?} / {member}: the listing and the read must agree, \
                 got {summary} vs {registration}"
            );
        }
    }
}

#[tokio::test]
async fn deleting_a_registration_takes_it_through_deleting_and_then_out_of_existence() {
    let (router, engine) = fleet();
    let held_directory = create_directory(&router, DIRECTORY_NAMES[0]).await;

    // The rows differ along the status the registration was in when the delete
    // arrived: still CREATING, settled ACTIVE, and settled FAILED.
    let rows: Vec<(String, String, bool, &str)> = vec![
        (
            "a registration still CREATING".to_string(),
            held_directory,
            false,
            "CREATING",
        ),
        (
            format!(
                "a registration settled ACTIVE on {}",
                DIRECTORY_NAMES[1]
            ),
            create_directory(&router, DIRECTORY_NAMES[1]).await,
            true,
            "ACTIVE",
        ),
        (
            format!("a registration settled FAILED on {}", ABSENT_DIRECTORY_IDS[0]),
            ABSENT_DIRECTORY_IDS[0].to_string(),
            true,
            "FAILED",
        ),
    ];

    for (case, directory_id, settle, before) in rows {
        let arn = create_registration(&router, &directory_id).await;
        if settle {
            set_settle_due(&engine, &arn, SystemTime::now() - ALREADY_DUE);
        } else {
            set_settle_due(&engine, &arn, SystemTime::now() + NOT_DUE_FOR);
        }
        let (_, _, read) = get_registration(&router, &arn).await;
        assert_eq!(
            read["DirectoryRegistration"]["Status"].as_str(),
            Some(before),
            "case {case:?}: the registration must be {before} before the delete, got {read}"
        );

        let (status, parsed) = delete_registration(&router, &arn).await;
        assert_eq!(
            status, 202,
            "case {case:?}: DeleteDirectoryRegistration must answer the modelled 202, \
             got {status} / {parsed}"
        );

        // The deauthorization is under way, not done: the resource is still served,
        // in DELETING and with no StatusReason.
        let (deleting_status, _, deleting) = get_registration(&router, &arn).await;
        assert_eq!(
            deleting_status, 200,
            "case {case:?}: a registration being deleted is still served, \
             got {deleting_status} / {deleting}"
        );
        assert_eq!(
            deleting["DirectoryRegistration"]["Status"].as_str(),
            Some("DELETING"),
            "case {case:?}: the accepted delete must move the registration to DELETING, \
             got {deleting}"
        );
        assert!(
            deleting["DirectoryRegistration"]["StatusReason"].is_null(),
            "case {case:?}: a registration being deleted carries no StatusReason, got {deleting}"
        );
        assert_eq!(
            listed_summary(&router, &arn).await.map(|summary| summary["Status"].clone()),
            Some(Value::String("DELETING".to_string())),
            "case {case:?}: the listing must report the same DELETING"
        );

        // Once the removal the delete owed is due, the resource stops existing: the
        // read is refused with the modelled 404 and all three of its members, and
        // the listing no longer carries it.
        set_deleted_due(&engine, &arn, SystemTime::now() - ALREADY_DUE);
        let (gone_status, code, gone) = get_registration(&router, &arn).await;
        assert_eq!(
            gone_status, 404,
            "case {case:?}: a deleted registration must be a 404, got {gone_status} / {gone}"
        );
        assert_eq!(
            code, "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {code:?} / {gone}"
        );
        assert_eq!(
            gone["Message"].as_str(),
            Some(format!("DirectoryRegistration was not found: {arn}.").as_str()),
            "case {case:?}: the refusal must read as this service words a missing resource, \
             got {gone}"
        );
        assert_eq!(
            gone["ResourceId"].as_str(),
            Some(arn.as_str()),
            "case {case:?}: the refusal must name the identifier as supplied, got {gone}"
        );
        assert_eq!(
            gone["ResourceType"].as_str(),
            Some("DirectoryRegistration"),
            "case {case:?}: the refusal must name the resource type, got {gone}"
        );
        assert_eq!(
            listed_summary(&router, &arn).await,
            None,
            "case {case:?}: the listing must not carry a registration that is gone"
        );

        // @idempotent: deleting an ARN that is already gone is a success too.
        let (again, body) = delete_registration(&router, &arn).await;
        assert_eq!(
            again, 202,
            "case {case:?}: deleting an ARN that is already gone must answer 202, \
             got {again} / {body}"
        );

        // And the directory is free again: registering it mints a fresh registration
        // instead of being refused as a conflict.
        let fresh = create_registration(&router, &directory_id).await;
        assert_eq!(
            fresh, arn,
            "case {case:?}: a re-registration of the same directory names the same ARN, \
             got {fresh:?}"
        );
        let (_, _, recreated) = get_registration(&router, &fresh).await;
        assert_eq!(
            recreated["DirectoryRegistration"]["Status"].as_str(),
            Some("CREATING"),
            "case {case:?}: the re-registration must be a fresh CREATING record, got {recreated}"
        );
        delete_registration(&router, &fresh).await;
        set_deleted_due(&engine, &fresh, SystemTime::now() - ALREADY_DUE);
    }
}

#[tokio::test]
async fn an_unheld_directory_never_leaves_the_registration_at_an_unterminated_status() {
    // The clock alone, with nothing reaching into either store: the spans this
    // service publishes are what move the registration, so a client that polls sees
    // the terminal state and then the resource's removal without anybody nudging it.
    let (router, _engine) = fleet();
    let arn = create_registration(&router, ABSENT_DIRECTORY_IDS[1]).await;

    let (_, _, creating) = get_registration(&router, &arn).await;
    assert_eq!(
        creating["DirectoryRegistration"]["Status"].as_str(),
        Some("CREATING"),
        "a registration read straight after its create is the CREATING the 202 reported, \
         got {creating}"
    );

    tokio::time::sleep(DIRECTORY_REGISTRATION_ACTIVATION_DELAY + CLOCK_MARGIN).await;
    let (_, _, settled) = get_registration(&router, &arn).await;
    assert_eq!(
        settled["DirectoryRegistration"]["Status"].as_str(),
        Some("FAILED"),
        "past its activation span a registration of an unheld directory is FAILED, got {settled}"
    );
    assert_eq!(
        settled["DirectoryRegistration"]["StatusReason"].as_str(),
        Some(REASON_NOT_FOUND),
        "the failure must name why the directory could not be authorized, got {settled}"
    );

    let (status, parsed) = delete_registration(&router, &arn).await;
    assert_eq!(
        status, 202,
        "DeleteDirectoryRegistration must answer the modelled 202, got {status} / {parsed}"
    );
    tokio::time::sleep(DIRECTORY_REGISTRATION_DELETION_DELAY + CLOCK_MARGIN).await;
    let (gone_status, code, gone) = get_registration(&router, &arn).await;
    assert_eq!(
        gone_status, 404,
        "past its deletion span the registration must be gone, got {gone_status} / {gone}"
    );
    assert_eq!(
        code, "ResourceNotFoundException",
        "expected ResourceNotFoundException, got {code:?} / {gone}"
    );
}
