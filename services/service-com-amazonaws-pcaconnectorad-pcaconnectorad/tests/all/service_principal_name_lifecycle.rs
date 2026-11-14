// SPDX-License-Identifier: CC0-1.0

//! The service-principal-name slice: `CreateServicePrincipalName` and
//! `GetServicePrincipalName`.
//!
//! A service principal name is the name a connector authenticates to Active Directory
//! with. It has no ARN of its own — every modelled operation addresses it by the PAIR
//! of ARNs in its URI, the directory registration it lives under and the connector it
//! names (`smithy_query(query="operation",operation="CreateServicePrincipalName,\
//! GetServicePrincipalName")`) — so the pair is its identity here too.
//!
//! The rules asserted below:
//!
//! 1. Both parents are resolved before anything is recorded, in the order the URI
//!    names them: a request naming a directory registration this service does not
//!    hold is HTTP 404 `ResourceNotFoundException` whose message is this service's one
//!    not-found sentence for the resource type and the identifier the request supplied
//!    (`DirectoryRegistration was not found: <arn>.`), and one naming an absent
//!    connector is the same refusal about the connector.
//! 2. A create AWS answers 202 for records a name the clock owes a move out of
//!    `CREATING`: it reaches `ACTIVE` only where the registration's own authorization
//!    reached `ACTIVE`, and otherwise `FAILED` carrying `DIRECTORY_RESOURCE_NOT_FOUND`
//!    — the same cause the registration itself reports, because the name is written
//!    into the directory that registration authorizes.
//!    (`ServicePrincipalNameStatus` = [CREATING, ACTIVE, DELETING, FAILED] and
//!    `ServicePrincipalNameStatusReason` carries `DIRECTORY_RESOURCE_NOT_FOUND`:
//!    `smithy_query(query="shape",shape="ServicePrincipalNameStatus,\
//!    ServicePrincipalNameStatusReason")`.)
//! 3. The operation is `@idempotent`: asking again for a name the pair already holds
//!    changes nothing and is answered with the same 202.
//!
//! Every case drives the real wire path through [`super::harness`]. Directories belong
//! to Directory Service, so the rows that need the account to HOLD one create it
//! through that service's own front door on the shared engine
//! ([`super::directory_registration_authorization::fleet`]). Nothing reaches into a
//! store except to move a DUE INSTANT the clock is waiting on — never a status, which
//! the handlers still compute from the records themselves.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_pcaconnectorad_pcaconnectorad as pcad;

use super::connector_lifecycle::create_connector;
use super::directory_registration_authorization::{create_directory, fleet};
use super::harness::{call, encode_label, register_directory, ACCOUNT, REGION};

/// Far enough out that no case outlives it, so a call made while it stands observes
/// the transient status.
const NOT_DUE_FOR: Duration = Duration::from_secs(3600);

/// Any past instant is due, so the next call applies the move the clock owed.
const ALREADY_DUE: Duration = Duration::from_secs(1);

/// The `StatusReason` a name whose directory the account does not hold terminates
/// with.
const REASON_NOT_FOUND: &str = "DIRECTORY_RESOURCE_NOT_FOUND";

/// Directory ids no registration of a case was created for, spanning the modelled
/// `DirectoryId` pattern `^d-[0-9a-f]{10}$`
/// (`smithy_query(query="shape",shape="DirectoryId")`): digits only, hex letters
/// only, and both extremes of the range.
const ABSENT_DIRECTORY_IDS: &[&str] = &[
    "d-1234567890",
    "d-abcdefabcd",
    "d-0000000000",
    "d-ffffffffff",
];

/// A directory-registration ARN of this Region and account, as this service mints
/// one: the resource segment is the directory's own id.
fn registration_arn(directory_id: &str) -> String {
    format!("arn:aws:pca-connector-ad:{REGION}:{ACCOUNT}:directory-registration/{directory_id}")
}

/// A well-formed connector ARN of this Region and account that no call ever minted.
fn absent_connector_arn() -> String {
    format!(
        "arn:aws:pca-connector-ad:{REGION}:{ACCOUNT}:connector/99999999-8888-7777-6666-555555555555"
    )
}

/// The body a `CreateConnector` carries: the smallest one the shape accepts
/// (`smithy_query(query="operation",operation="CreateConnector")`).
fn connector_body(directory_id: &str) -> Value {
    json!({
        "DirectoryId": directory_id,
        "CertificateAuthorityArn": format!(
            "arn:aws:acm-pca:{REGION}:{ACCOUNT}:certificate-authority/\
             11111111-2222-3333-4444-555555555555"
        ),
        "VpcInformation": { "SecurityGroupIds": ["sg-0123456789abcdef0"] },
    })
}

/// The request target both operations address one name by.
fn name_target(registration_arn: &str, connector_arn: &str) -> String {
    format!(
        "/directoryRegistrations/{}/servicePrincipalNames/{}",
        encode_label(registration_arn),
        encode_label(connector_arn)
    )
}

/// `CreateServicePrincipalName`, with the `ClientToken` the row supplies.
async fn create_name(
    router: &MultiServiceRouter,
    registration_arn: &str,
    connector_arn: &str,
    client_token: Option<&str>,
) -> (u16, String, Value) {
    let body = match client_token {
        Some(token) => json!({ "ClientToken": token }),
        None => json!({}),
    };
    call(
        router,
        "POST",
        &name_target(registration_arn, connector_arn),
        Some(&body),
    )
    .await
}

async fn get_name(
    router: &MultiServiceRouter,
    registration_arn: &str,
    connector_arn: &str,
) -> (u16, String, Value) {
    call(
        router,
        "GET",
        &name_target(registration_arn, connector_arn),
        None,
    )
    .await
}

/// A registration and a connector for `directory_id`, both held by this service: the
/// prerequisites a name needs. The registration comes first — a connector's
/// `DirectoryId` is resolved against it.
async fn prerequisites(router: &MultiServiceRouter, directory_id: &str) -> (String, String) {
    let registration = register_directory(router, directory_id).await;
    let connector = create_connector(router, &connector_body(directory_id)).await;
    (registration, connector)
}

/// Move the instant the clock owes the name of (`registration_arn`, `connector_arn`)
/// its move out of `CREATING` to `due_at`, through the service's own state.
///
/// Strictly a clock move: it rewrites only the due instant, never the status, the
/// terminal status it settles on or the timestamps — the handlers still compute those
/// from this same record.
fn set_settle_due(
    engine: &engine::InMemoryEngine,
    registration_arn: &str,
    connector_arn: &str,
    due_at: SystemTime,
) {
    let shared = pcad::state::service_state(engine).expect("the service's own state");
    let mut store = shared.lock();
    let record = store
        .service_principal_names
        .get_mut(&(registration_arn.to_string(), connector_arn.to_string()))
        .unwrap_or_else(|| {
            panic!("no service principal name is stored under {registration_arn:?} / {connector_arn:?}")
        });
    record.settles_at = due_at;
}

/// Assert one not-found refusal: the status, the code, and all three required members
/// of the modelled shape.
fn assert_not_found(case: &str, answered: (u16, String, Value), kind: &str, identifier: &str) {
    let (status, code, parsed) = answered;
    assert_eq!(
        status, 404,
        "case {case:?}: a resource this service does not hold is a 404, \
         got {status} / {code:?} / {parsed}"
    );
    assert_eq!(
        code, "ResourceNotFoundException",
        "case {case:?}: expected ResourceNotFoundException, got {code:?} / {parsed}"
    );
    let expected = format!("{kind} was not found: {identifier}.");
    assert_eq!(
        parsed["Message"].as_str(),
        Some(expected.as_str()),
        "case {case:?}: expected message {expected:?}, got {parsed}"
    );
    assert_eq!(
        parsed["ResourceId"].as_str(),
        Some(identifier),
        "case {case:?}: the refusal must name the identifier as supplied, got {parsed}"
    );
    assert_eq!(
        parsed["ResourceType"].as_str(),
        Some(kind),
        "case {case:?}: the refusal must name the resource type, got {parsed}"
    );
}

#[tokio::test]
async fn creating_a_name_under_a_registration_this_service_does_not_hold_is_resource_not_found() {
    let (router, _engine) = fleet();
    // A registration and a connector this service DOES hold, so each refusal below is
    // a store with resources in it answering about the one it does not hold.
    let (_, held_connector) = prerequisites(&router, "d-0123456789").await;
    let absent_connector = absent_connector_arn();

    for directory_id in ABSENT_DIRECTORY_IDS {
        let absent = registration_arn(directory_id);
        // The registration is the first label of the path and so the first thing
        // resolved: the refusal is about it whether the connector exists or not.
        for (case, connector) in [
            ("beside a connector this service holds", &held_connector),
            ("beside a connector it does not hold", &absent_connector),
        ] {
            let case = format!("CreateServicePrincipalName on {absent}, {case}");
            let answered = create_name(&router, &absent, connector, None).await;
            assert_not_found(&case, answered, "DirectoryRegistration", &absent);

            let case = format!("GetServicePrincipalName on {absent}, {case}");
            let answered = get_name(&router, &absent, connector).await;
            assert_not_found(&case, answered, "DirectoryRegistration", &absent);
        }
    }
}

/// One more Region, so the refusal is not a property of the Region the request was
/// signed for: a registration of another Region is one this store does not hold
/// either.
#[tokio::test]
async fn a_registration_of_another_region_is_refused_the_same_way() {
    let (router, _engine) = fleet();
    let (_, connector) = prerequisites(&router, "d-0123456789").await;
    let elsewhere =
        format!("arn:aws:pca-connector-ad:eu-west-1:{ACCOUNT}:directory-registration/d-0123456789");

    let answered = create_name(&router, &elsewhere, &connector, None).await;
    assert_not_found(
        "CreateServicePrincipalName on a registration of another Region",
        answered,
        "DirectoryRegistration",
        &elsewhere,
    );
}

#[tokio::test]
async fn creating_a_name_for_a_connector_this_service_does_not_hold_is_resource_not_found() {
    let (router, _engine) = fleet();
    let (registration, _) = prerequisites(&router, "d-0123456789").await;

    // Connector ARNs this service never minted, in three different ways: a fresh
    // uuid, the nil uuid, and one of another Region.
    let absent = [
        absent_connector_arn(),
        format!("arn:aws:pca-connector-ad:{REGION}:{ACCOUNT}:connector/00000000-0000-0000-0000-000000000000"),
        format!("arn:aws:pca-connector-ad:eu-west-1:{ACCOUNT}:connector/11111111-2222-3333-4444-555555555555"),
    ];

    for connector in &absent {
        let case = format!("CreateServicePrincipalName for {connector}");
        let answered = create_name(&router, &registration, connector, None).await;
        assert_not_found(&case, answered, "Connector", connector);
    }
}

/// A registration this service holds, with no name recorded for the connector asked
/// about: the refusal is about the NAME, in the same wording.
#[tokio::test]
async fn reading_a_name_that_was_never_created_is_resource_not_found() {
    let (router, _engine) = fleet();
    let (registration, connector) = prerequisites(&router, "d-0123456789").await;

    let answered = get_name(&router, &registration, &connector).await;
    assert_not_found(
        "GetServicePrincipalName before any create",
        answered,
        "ServicePrincipalName",
        &connector,
    );
}

/// One row of the terminal-state table: whether the account holds the directory the
/// name's registration authorizes, and what the name must settle on.
struct Row {
    case: String,
    directory_id: String,
    status: &'static str,
    reason: Option<&'static str>,
}

#[tokio::test]
async fn a_created_name_is_read_back_and_settles_on_what_its_directory_allows() {
    let (router, engine) = fleet();

    // The rows differ along the one thing the outcome generalises over: whether the
    // account holds the directory — created through Directory Service itself, created
    // and then deleted, and never created at all.
    let mut rows: Vec<Row> = Vec::new();
    let held = create_directory(&router, "corp.example.com").await;
    rows.push(Row {
        case: format!("a directory the account holds ({held})"),
        directory_id: held,
        status: "ACTIVE",
        reason: None,
    });
    let deleted = create_directory(&router, "gone.example.com").await;
    super::directory_registration_authorization::delete_directory(&router, &deleted).await;
    rows.push(Row {
        case: format!("a directory that was deleted again ({deleted})"),
        directory_id: deleted,
        status: "FAILED",
        reason: Some(REASON_NOT_FOUND),
    });
    for directory_id in ABSENT_DIRECTORY_IDS {
        rows.push(Row {
            case: format!("a directory that was never created ({directory_id})"),
            directory_id: (*directory_id).to_string(),
            status: "FAILED",
            reason: Some(REASON_NOT_FOUND),
        });
    }

    for row in rows {
        let (registration, connector) = prerequisites(&router, &row.directory_id).await;

        let (status, code, parsed) = create_name(&router, &registration, &connector, None).await;
        assert_eq!(
            status, 202,
            "case {:?}: CreateServicePrincipalName must answer the modelled 202, \
             got {status} / {code:?} / {parsed}",
            row.case
        );
        assert!(
            parsed.is_null(),
            "case {:?}: the modelled output is empty, so the 202 carries no body, got {parsed}",
            row.case
        );

        // Held open, so the read below observes the transient status the 202 started.
        set_settle_due(
            &engine,
            &registration,
            &connector,
            SystemTime::now() + NOT_DUE_FOR,
        );
        let (status, code, creating) = get_name(&router, &registration, &connector).await;
        assert_eq!(
            status, 200,
            "case {:?}: GetServicePrincipalName must read the name back, \
             got {status} / {code:?} / {creating}",
            row.case
        );
        let name = &creating["ServicePrincipalName"];
        assert_eq!(
            name["DirectoryRegistrationArn"].as_str(),
            Some(registration.as_str()),
            "case {:?}: the name must report the registration it was created under, got {name}",
            row.case
        );
        assert_eq!(
            name["ConnectorArn"].as_str(),
            Some(connector.as_str()),
            "case {:?}: the name must report the connector it names, got {name}",
            row.case
        );
        assert_eq!(
            name["Status"].as_str(),
            Some("CREATING"),
            "case {:?}: a name whose move is not due yet is still CREATING, got {name}",
            row.case
        );
        assert!(
            name["StatusReason"].is_null(),
            "case {:?}: a name that has not failed carries no StatusReason, got {name}",
            row.case
        );
        assert!(
            name["CreatedAt"].is_number() && name["UpdatedAt"].is_number(),
            "case {:?}: CreatedAt/UpdatedAt must be set, got {name}",
            row.case
        );

        // The move the clock owes, made due: the status the authorization of this
        // name's directory reaches.
        set_settle_due(
            &engine,
            &registration,
            &connector,
            SystemTime::now() - ALREADY_DUE,
        );
        let (status, code, settled) = get_name(&router, &registration, &connector).await;
        assert_eq!(
            status, 200,
            "case {:?}: the settled name must still be read back, \
             got {status} / {code:?} / {settled}",
            row.case
        );
        let name = &settled["ServicePrincipalName"];
        assert_eq!(
            name["Status"].as_str(),
            Some(row.status),
            "case {:?}: past its span the name must be {}, got {name}",
            row.case,
            row.status
        );
        assert_eq!(
            name["StatusReason"].as_str(),
            row.reason,
            "case {:?}: expected StatusReason {:?}, got {name}",
            row.case,
            row.reason
        );
        assert_eq!(
            name["ConnectorArn"].as_str(),
            Some(connector.as_str()),
            "case {:?}: settling must not change the name's identity, got {name}",
            row.case
        );
    }
}

/// `@idempotent`: the pair identifies at most one name, so a repeat of the call — with
/// the first call's `ClientToken`, with another one, or with none — is answered again
/// and leaves the one record alone.
#[tokio::test]
async fn asking_again_for_a_name_the_pair_already_holds_changes_nothing() {
    let (router, engine) = fleet();
    let (registration, connector) = prerequisites(&router, "d-0123456789").await;

    let (status, code, parsed) =
        create_name(&router, &registration, &connector, Some("token-one")).await;
    assert_eq!(
        status, 202,
        "the first create must answer 202, got {status} / {code:?} / {parsed}"
    );
    let (_, _, first) = get_name(&router, &registration, &connector).await;
    let created_at = first["ServicePrincipalName"]["CreatedAt"].clone();

    for token in [Some("token-one"), Some("token-two"), None] {
        let (status, code, parsed) = create_name(&router, &registration, &connector, token).await;
        assert_eq!(
            status, 202,
            "case {token:?}: a repeat of the call is answered 202, \
             got {status} / {code:?} / {parsed}"
        );
        let (_, _, read) = get_name(&router, &registration, &connector).await;
        assert_eq!(
            read["ServicePrincipalName"]["CreatedAt"], created_at,
            "case {token:?}: a repeat must not mint a second name, got {read}"
        );
    }

    let shared = pcad::state::service_state(&*engine).expect("the service's own state");
    let stored = shared.lock().service_principal_names.len();
    assert_eq!(
        stored, 1,
        "the four calls must leave exactly one name recorded, got {stored}"
    );
}

/// A name exists under its registration and for its connector: once either stops
/// existing, nothing reports the name either.
#[tokio::test]
async fn a_name_does_not_outlive_the_resources_it_was_created_under() {
    let (router, engine) = fleet();
    let (registration, connector) = prerequisites(&router, "d-0123456789").await;
    let (status, code, parsed) = create_name(&router, &registration, &connector, None).await;
    assert_eq!(
        status, 202,
        "the create must answer 202, got {status} / {code:?} / {parsed}"
    );

    let (status, _, _) = call(
        &router,
        "DELETE",
        &format!("/connectors/{}", encode_label(&connector)),
        None,
    )
    .await;
    assert_eq!(status, 202, "DeleteConnector must answer the modelled 202");

    let answered = get_name(&router, &registration, &connector).await;
    assert_not_found(
        "GetServicePrincipalName after its connector was deleted",
        answered,
        "ServicePrincipalName",
        &connector,
    );

    let shared = pcad::state::service_state(&*engine).expect("the service's own state");
    let stored = shared.lock().service_principal_names.len();
    assert_eq!(
        stored, 0,
        "a deleted connector leaves no service principal name of itself, got {stored}"
    );
}
