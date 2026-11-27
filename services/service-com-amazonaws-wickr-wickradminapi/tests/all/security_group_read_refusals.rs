// SPDX-License-Identifier: CC0-1.0

//! How a read of ONE security group answers a group it cannot report, driven over HTTP
//! against one running emulator.
//!
//! The rules under test:
//!
//! * `GetSecurityGroup` is `GET /networks/{networkId}/security-groups/{groupId}` and the
//!   resource it reports on is the security GROUP the request named, so a `groupId` this store
//!   resolves to no group under that `networkId` is refused as that group — whether no group
//!   of the network ever carried the id, the id names a group of a DIFFERENT network, or the
//!   network that held it has been deleted and its groups went with it. The parent network
//!   never stands in for it.
//! * That refusal is an authorization-class one, not a not-found: HTTP 401, the status the
//!   model declares for `UnauthorizedError`
//!   (`smithy_query(query="shape",shape="UnauthorizedError")`), with this API's fixed
//!   sentence for such a read.
//! * It names its error SHAPE nowhere: no `x-amzn-errortype` header and no discriminator
//!   member in the body, so a client classifies it by status. Everything else this service
//!   refuses still names its shape — a network read of a deleted network is the typed
//!   `ForbiddenError` this API states for a network that is gone, and a `networkId` it cannot
//!   place at all is a typed `ForbiddenError` carrying this API's denial sentence.
//! * The sentence is entirely fixed text: it names neither the `groupId` nor the `networkId`
//!   the request carried, so the refusal discloses nothing about what the account holds or
//!   used to hold.
//! * A group that IS there is still reported, and deleting one network leaves another
//!   network's groups readable, so the refusal follows the state of the group that was asked
//!   for rather than the state of the store.
//!
//! Every case goes through the real multi-service router — route resolution, restJson1
//! deserialization, modelled-constraint validation, the handler and response serialization —
//! so what it asserts is the wire behaviour a client sees.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_wickr_wickradminapi::state::{
    ACCESS_DENIED_MESSAGE, DEFAULT_SECURITY_GROUP_ID, NETWORK_GONE_MESSAGE,
    SECURITY_GROUP_READ_REFUSED_MESSAGE,
};
use smithy_server_runtime::MultiServiceRouter;

/// The status the model declares for `UnauthorizedError` (`@httpError(401)`), which a read of
/// a security group this store cannot resolve is refused with.
const REFUSED_READ_STATUS: u16 = 401;

/// The status the model declares for `ForbiddenError` (`@httpError(403)`), which is what a
/// read of a DELETED NETWORK is refused with — the control showing this service does name the
/// shape of its other refusals.
const FORBIDDEN_STATUS: u16 = 403;

/// The body members a JSON failure can name its error shape in, in the order a client
/// consults them: `__type` is what this emulator's restJson1 renderer writes, and
/// `code` / `Code` are the other spellings the same parsers accept.
const DISCRIMINATOR_MEMBERS: [&str; 3] = ["__type", "code", "Code"];

/// One answer as a client sees it: the status, the `x-amzn-errortype` header restJson1
/// resolves an error shape from, and the parsed body.
struct Answer {
    status: u16,
    error_type: String,
    body: Value,
}

impl Answer {
    /// The `message` member of the body, or `""` when there is none.
    fn message(&self) -> &str {
        self.body["message"].as_str().unwrap_or_default()
    }

    /// The error shape the body names, or `""` when it names none.
    fn body_shape(&self) -> String {
        DISCRIMINATOR_MEMBERS
            .iter()
            .find_map(|member| self.body[*member].as_str())
            .unwrap_or_default()
            .to_string()
    }
}

/// A fresh emulator holding only this service, over its own engine, so one test's networks
/// are invisible to another's.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // The signature is not the subject here; the refusal path is.
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_wickr_wickradminapi::EmulatedService::new(
        engine,
    ));
    router
}

/// One request/response round trip.
async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: Option<Value>) -> Answer {
    let payload = body
        .map(|value| serde_json::to_vec(&value).expect("serialize the request body"))
        .unwrap_or_default();
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "admin.wickr.us-east-1.amazonaws.com")
        // The SigV4 credential scope is how the router picks the service.
        .header(
            "authorization",
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260102/us-east-1/wickr/aws4_request, \
             SignedHeaders=host, Signature=00",
        )
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(payload))
        .expect("build the request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let raw = response.into_body();
    let body = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&raw).into_owned()))
    };
    Answer {
        status,
        error_type,
        body,
    }
}

/// Create a network and return its minted id, failing the test if it was refused.
async fn create_network(router: &MultiServiceRouter, name: &str) -> String {
    let answer = call(
        router,
        "POST",
        "/networks",
        Some(json!({ "networkName": name, "accessLevel": "STANDARD" })),
    )
    .await;
    assert!(
        (200..300).contains(&answer.status),
        "CreateNetwork {name:?}: expected a success, got {} with {}",
        answer.status,
        answer.body
    );
    answer.body["networkId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateNetwork {name:?}: no networkId in {}", answer.body))
        .to_string()
}

/// Create one security group of `network_id` and return its minted id.
async fn create_group(router: &MultiServiceRouter, network_id: &str, name: &str) -> String {
    let answer = call(
        router,
        "POST",
        &format!("/networks/{network_id}/security-groups"),
        Some(json!({ "name": name, "securityGroupSettings": {} })),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "CreateSecurityGroup {name:?}: expected 200, got {} with {}",
        answer.status, answer.body
    );
    answer.body["securityGroup"]["id"]
        .as_str()
        .unwrap_or_else(|| {
            panic!("CreateSecurityGroup {name:?}: no group id in {}", answer.body)
        })
        .to_string()
}

/// Read one security group back.
async fn get_group(router: &MultiServiceRouter, network_id: &str, group_id: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/networks/{network_id}/security-groups/{group_id}"),
        None,
    )
    .await
}

/// Delete one network and wait out the deletion the way a client does: by asking for the
/// network until the read is refused.
///
/// The deadline bounds the loop and is not what the test measures; the assertion after it is
/// the check.
async fn delete_network(router: &MultiServiceRouter, network_id: &str) {
    let answer = call(router, "DELETE", &format!("/networks/{network_id}"), None).await;
    assert!(
        (200..300).contains(&answer.status),
        "DeleteNetwork: expected a success, got {} with {}",
        answer.status,
        answer.body
    );
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    let mut answer = call(router, "GET", &format!("/networks/{network_id}"), None).await;
    while answer.status < 400 && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(50)).await;
        answer = call(router, "GET", &format!("/networks/{network_id}"), None).await;
    }
    assert_eq!(
        answer.status, FORBIDDEN_STATUS,
        "DeleteNetwork: expected the network read to end up refused {FORBIDDEN_STATUS}, got {} \
         with {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.message(),
        NETWORK_GONE_MESSAGE,
        "DeleteNetwork: expected the gone-network sentence, got {}",
        answer.body
    );
}

/// Assert `answer` is the refusal this API states for a security group a read cannot resolve:
/// the status, the fixed sentence, no error shape in either place, and nothing of the ids
/// that were asked about.
fn assert_refused_read(case: &str, answer: &Answer, network_id: &str, group_id: &str) {
    assert_eq!(
        answer.status, REFUSED_READ_STATUS,
        "case {case:?}: expected status {REFUSED_READ_STATUS}, got {} with {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.message(),
        SECURITY_GROUP_READ_REFUSED_MESSAGE,
        "case {case:?}: expected the fixed sentence {SECURITY_GROUP_READ_REFUSED_MESSAGE:?}, got \
         {:?}",
        answer.message()
    );
    assert_eq!(
        answer.error_type, "",
        "case {case:?}: the refusal must name no error shape in the header, got {:?}",
        answer.error_type
    );
    assert_eq!(
        answer.body_shape(),
        "",
        "case {case:?}: the refusal must name no error shape in the body, got {}",
        answer.body
    );
    let rendered = answer.body.to_string();
    assert!(
        !rendered.contains(group_id),
        "case {case:?}: the refusal must not disclose the groupId it was asked about, got {}",
        answer.body
    );
    assert!(
        !rendered.contains(network_id),
        "case {case:?}: the refusal must not disclose the networkId it was asked about, got {}",
        answer.body
    );
}

/// Every security group a read cannot resolve is refused as that GROUP, in the one untyped
/// form.
///
/// The rows differ along what the rule generalises over: whether the network behind the label
/// exists, was deleted or was never minted, and whether the `groupId` was never minted,
/// belongs to another network, is the network's seeded default group, or is a form no id this
/// store mints takes at all (`GenericString`'s `^[\S\s]*$` constrains nothing, so the label
/// reaches the handler whatever it carries).
#[tokio::test]
async fn a_security_group_a_read_cannot_resolve_is_refused_as_that_group_and_names_no_error_shape()
{
    let router = emulator();
    let live = create_network(&router, "live-sg-net").await;
    let live_group = create_group(&router, &live, "live group").await;
    let other = create_network(&router, "other-sg-net").await;
    let other_group = create_group(&router, &other, "other group").await;
    // A SECOND and THIRD group of the neighbour, so the id used below is one neither the live
    // network nor the doomed one has minted: group ids are scoped to their own network and
    // minted from the same start, so each network's first group shares an id with every
    // other's.
    create_group(&router, &other, "other second group").await;
    let other_third_group = create_group(&router, &other, "other third group").await;
    let doomed = create_network(&router, "doomed-sg-net").await;
    let doomed_group = create_group(&router, &doomed, "doomed group").await;
    // A second group of the doomed network, for the same reason as the neighbour's: its id
    // is one the live network has not minted.
    let doomed_second_group = create_group(&router, &doomed, "doomed second group").await;
    delete_network(&router, &doomed).await;

    // A well-formed `NetworkId` (`^[0-9]{8}$`) this store never minted: ids are minted from
    // the low end of the 8-digit range upwards, so the top of it is no network here.
    let unminted_network = "99999999";
    assert!(
        ![live.as_str(), other.as_str(), doomed.as_str()].contains(&unminted_network),
        "the unminted network id must not be one this store minted"
    );

    // (case, the networkId asked under, the groupId asked for)
    let rows: Vec<(&str, String, String)> = vec![
        (
            "a groupId no group of the network ever carried",
            live.clone(),
            "4242".to_string(),
        ),
        (
            "a groupId that is not a number at all",
            live.clone(),
            "engineering".to_string(),
        ),
        (
            "a groupId of one character",
            live.clone(),
            "7".to_string(),
        ),
        (
            "a groupId that names a group of another network",
            live.clone(),
            other_third_group.clone(),
        ),
        (
            "the group of a network that has been deleted",
            doomed.clone(),
            doomed_group.clone(),
        ),
        (
            "the default group of a network that has been deleted",
            doomed.clone(),
            DEFAULT_SECURITY_GROUP_ID.to_string(),
        ),
        (
            "a group of a deleted network read under a live one",
            live.clone(),
            doomed_second_group.clone(),
        ),
        (
            "a networkId this store never minted",
            unminted_network.to_string(),
            live_group.clone(),
        ),
        (
            "the default group of a network that never existed",
            unminted_network.to_string(),
            DEFAULT_SECURITY_GROUP_ID.to_string(),
        ),
    ];

    for (case, network_id, group_id) in rows {
        let answer = get_group(&router, &network_id, &group_id).await;
        assert_refused_read(case, &answer, &network_id, &group_id);
    }

    // The groups that ARE there are still reported, so the refusals above follow the group the
    // request named rather than the state of the store.
    for (case, network_id, group_id, name) in [
        ("the live network's group", &live, &live_group, "live group"),
        (
            "the other network's group",
            &other,
            &other_group,
            "other group",
        ),
    ] {
        let answer = get_group(&router, network_id, group_id).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected 200, got {} with {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.body["securityGroup"]["id"].as_str(),
            Some(group_id.as_str()),
            "case {case:?}: expected the group that was asked for, got {}",
            answer.body
        );
        assert_eq!(
            answer.body["securityGroup"]["name"].as_str(),
            Some(name),
            "case {case:?}: expected the stored name, got {}",
            answer.body
        );
    }
}

/// A read inside a deleted network reports the GROUP, while a read OF that network reports
/// the network — and the two refusals cannot be confused for each other: each states its own
/// status and sentence, and the network's names its shape where the group's names none.
#[tokio::test]
async fn a_cascade_is_reported_at_the_resource_the_request_named() {
    let router = emulator();
    let kept = create_network(&router, "kept-sg-net").await;
    let kept_group = create_group(&router, &kept, "kept group").await;
    let gone = create_network(&router, "gone-sg-net").await;
    let first_group = create_group(&router, &gone, "first gone group").await;
    let second_group = create_group(&router, &gone, "second gone group").await;
    delete_network(&router, &gone).await;

    for (case, group_id) in [
        ("the seeded default group", DEFAULT_SECURITY_GROUP_ID),
        ("the first group the network held", first_group.as_str()),
        ("the second group the network held", second_group.as_str()),
    ] {
        let answer = get_group(&router, &gone, group_id).await;
        assert_refused_read(case, &answer, &gone, group_id);
    }

    // The network itself is the resource a network read names, and that refusal keeps the
    // shape this API states it in.
    let answer = call(&router, "GET", &format!("/networks/{gone}"), None).await;
    assert_eq!(
        answer.status, FORBIDDEN_STATUS,
        "case \"the deleted network itself\": expected {FORBIDDEN_STATUS}, got {} with {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.message(),
        NETWORK_GONE_MESSAGE,
        "case \"the deleted network itself\": expected the gone-network sentence, got {}",
        answer.body
    );
    assert_eq!(
        answer.body_shape(),
        "ForbiddenError",
        "case \"the deleted network itself\": expected the typed shape, got {}",
        answer.body
    );

    // A neighbour's group is untouched by the cascade.
    let answer = get_group(&router, &kept, &kept_group).await;
    assert_eq!(
        answer.status, 200,
        "case \"a neighbour network's group\": expected 200, got {} with {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.body["securityGroup"]["name"].as_str(),
        Some("kept group"),
        "case \"a neighbour network's group\": expected its own record, got {}",
        answer.body
    );
}

/// The refusal body carries the sentence and nothing else, and another refusal of this API
/// still names its shape: a `networkId` this store cannot place at all is denied with the
/// typed `ForbiddenError` this API states for it, so dropping the shape reaches the group
/// read alone.
#[tokio::test]
async fn the_refusal_body_carries_the_sentence_alone_and_no_other_refusal_loses_its_shape() {
    let router = emulator();
    let network_id = create_network(&router, "body-sg-net").await;
    let answer = get_group(&router, &network_id, "4242").await;
    assert_eq!(
        answer.status, REFUSED_READ_STATUS,
        "expected {REFUSED_READ_STATUS}, got {} with {}",
        answer.status, answer.body
    );
    let object = answer
        .body
        .as_object()
        .unwrap_or_else(|| panic!("the refusal body must be a JSON object, got {}", answer.body));
    let members: Vec<&str> = object.keys().map(String::as_str).collect();
    assert_eq!(
        members,
        vec!["message"],
        "expected the sentence alone, got {}",
        answer.body
    );

    // The network read's own denial: this one a client resolves to a modelled shape, in both
    // places the shape travels, and it is the permission class rather than this status.
    let answer = call(&router, "GET", "/networks/99999999", None).await;
    assert_eq!(
        answer.status, FORBIDDEN_STATUS,
        "case \"an unplaceable networkId\": expected {FORBIDDEN_STATUS}, got {} with {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.message(),
        ACCESS_DENIED_MESSAGE,
        "case \"an unplaceable networkId\": expected the denial sentence, got {}",
        answer.body
    );
    assert_eq!(
        answer.body_shape(),
        "ForbiddenError",
        "case \"an unplaceable networkId\": expected the typed shape in the body, got {}",
        answer.body
    );
    assert_eq!(
        answer.error_type, "ForbiddenError",
        "case \"an unplaceable networkId\": expected x-amzn-errortype ForbiddenError, got {:?}",
        answer.error_type
    );
}
