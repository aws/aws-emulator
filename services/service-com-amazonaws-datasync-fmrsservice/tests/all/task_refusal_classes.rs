// SPDX-License-Identifier: CC0-1.0

//! Which refusal CLASS a `CreateTask` that breaks a cross-member rule reports.
//!
//! RULE: every client failure of this service is the one modelled `InvalidRequestException`
//! (400), so the class of the refusal is carried by that error's two code members —
//! `errorCode` and `datasyncErrorCode`, always the same value — and each broken rule has
//! its OWN published class: a pair of `LocationArn` members naming the same location is
//! `DestLocSameAsSrcLoc`, while a member naming a location the account does not hold is
//! `LocationNotFound`. A caller branches on that value, so one class must never stand in
//! for another, and nothing is stored when either is reported.
//!
//! Every case goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: FmrsService.<Operation>` and an `awsJson1_1` body, dispatched by the
//! multi-service router. Nothing reaches into the state struct.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The region every request in this suite signs for.
const SIGNED_REGION: &str = "us-west-2";

/// The class a refusal of the degenerate source/destination pair carries.
const DEST_LOC_SAME_AS_SRC_LOC: &str = "DestLocSameAsSrcLoc";

/// The class a refusal naming a location the account does not hold carries.
const LOCATION_NOT_FOUND: &str = "LocationNotFound";

/// The whole message that refusal is worded with: fixed text, with no variable slot — the
/// rule it reports is about the two members' relationship, not about either value.
const SAME_LOCATION_MESSAGE: &str = "Destination location cannot be same as source location.";

/// A router with just this service mounted on a fresh engine.
fn service() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_datasync_fmrsservice::EmulatedService::new(engine),
    );
    router
}

/// One `awsJson1_1` call, returning the HTTP status and the parsed response body.
async fn call(router: &MultiServiceRouter, operation: &str, body: Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("FmrsService.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDDATASYNCTEST/20260101/{SIGNED_REGION}\
                 /datasync/aws4_request, SignedHeaders=host;x-amz-target, Signature=00"
            ),
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(&body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = response.into_body();
    let parsed = if payload.is_empty() {
        json!({})
    } else {
        serde_json::from_slice(&payload)
            .unwrap_or_else(|_| json!({ "__unparsed": String::from_utf8_lossy(&payload) }))
    };
    (status, parsed)
}

/// A call that must succeed, returning its body.
async fn ok(router: &MultiServiceRouter, operation: &str, body: Value, case: &str) -> Value {
    let (status, parsed) = call(router, operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected {operation} to answer 200, got {status} with {parsed}"
    );
    parsed
}

/// What a refusal's message must say.
enum Wording {
    /// AWS's whole text for a class whose message has no variable slot.
    Exact(&'static str),
    /// For the not-found class, whose exact wording another open report states and this
    /// suite therefore does not pin: the message identifies the location it could not find
    /// and ends in the sentence that class is worded with.
    NamesLocation(String),
}

/// A call that must be refused with the whole envelope a class carries: HTTP 400, the one
/// modelled client error, that class in BOTH code members, and the message `wording` states.
async fn refused(
    router: &MultiServiceRouter,
    operation: &str,
    body: Value,
    class: &str,
    wording: &Wording,
    case: &str,
) {
    let (status, parsed) = call(router, operation, body).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected {operation} to answer 400, got {status} with {parsed}"
    );
    let wire_type = parsed["__type"].as_str().unwrap_or_default();
    assert!(
        wire_type.contains("InvalidRequestException"),
        "case {case:?}: expected an InvalidRequestException from {operation}, got {parsed}"
    );
    for member in ["errorCode", "datasyncErrorCode"] {
        assert_eq!(
            parsed[member].as_str(),
            Some(class),
            "case {case:?}: expected {member} {class:?}, got {parsed}"
        );
    }
    let message = parsed["message"].as_str().unwrap_or_default();
    match wording {
        Wording::Exact(expected) => assert_eq!(
            message, *expected,
            "case {case:?}: wrong refusal wording from {operation}, got {parsed}"
        ),
        Wording::NamesLocation(location_id) => assert!(
            message.contains(location_id.as_str()) && message.ends_with("is not found."),
            "case {case:?}: expected the refusal to name {location_id} as not found, got \
             {parsed}"
        ),
    }
}

/// An Amazon S3 transfer location for `bucket`, created and returned by ARN.
async fn given_location(router: &MultiServiceRouter, bucket: &str, case: &str) -> String {
    let created = ok(
        router,
        "CreateLocationS3",
        json!({
            "S3BucketArn": format!("arn:aws:s3:::{bucket}"),
            "S3Config": {
                "BucketAccessRoleArn": format!(
                    "arn:aws:iam::{account}:role/datasync-bucket-access",
                    account = engine::emulator_account_id()
                ),
            },
        }),
        case,
    )
    .await;
    created["LocationArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a LocationArn, got {created}"))
        .to_string()
}

/// A well-formed `LocationArn` this service never minted: the modelled pattern is
/// `…:location/loc-[0-9a-z]{17}`, so the id here is 17 of those characters.
fn never_minted(tail: char) -> String {
    format!(
        "arn:aws:datasync:{SIGNED_REGION}:{account}:location/loc-0123456789abcdef{tail}",
        account = engine::emulator_account_id()
    )
}

/// The location id an ARN carries: what a not-found refusal identifies the location by.
fn location_id(location_arn: &str) -> String {
    location_arn
        .rsplit('/')
        .next()
        .unwrap_or(location_arn)
        .to_string()
}

/// RULE: a transfer task copies one location onto ANOTHER, so a `CreateTask` whose two
/// `LocationArn` members name the same location is refused with the
/// `DestLocSameAsSrcLoc` class and its fixed message, while a member naming a location the
/// account does not hold is refused with `LocationNotFound` instead — and neither stores a
/// task.
///
/// The rows differ along what the rule generalises over: which location is named on both
/// sides, both orderings of a distinct pair, and which member carries an absent location,
/// so the two classes are told apart rather than collapsed into one.
#[tokio::test]
async fn create_task_reports_the_class_of_the_cross_member_rule_it_refused() {
    let router = service();
    let first = given_location(&router, "class-first", "setup").await;
    let second = given_location(&router, "class-second", "setup").await;
    let absent = never_minted('0');
    let other_absent = never_minted('1');

    /// What a row expects: an accepted task, or a refusal class with its wording.
    enum Expected {
        Accepted,
        Refused {
            class: &'static str,
            wording: Wording,
        },
    }

    let rows = [
        (
            "the first location onto itself",
            first.clone(),
            first.clone(),
            Expected::Refused {
                class: DEST_LOC_SAME_AS_SRC_LOC,
                wording: Wording::Exact(SAME_LOCATION_MESSAGE),
            },
        ),
        (
            "the second location onto itself",
            second.clone(),
            second.clone(),
            Expected::Refused {
                class: DEST_LOC_SAME_AS_SRC_LOC,
                wording: Wording::Exact(SAME_LOCATION_MESSAGE),
            },
        ),
        (
            "the first onto the second",
            first.clone(),
            second.clone(),
            Expected::Accepted,
        ),
        (
            "the second onto the first",
            second.clone(),
            first.clone(),
            Expected::Accepted,
        ),
        (
            "an absent source",
            absent.clone(),
            first.clone(),
            Expected::Refused {
                class: LOCATION_NOT_FOUND,
                wording: Wording::NamesLocation(location_id(&absent)),
            },
        ),
        (
            "an absent destination",
            first.clone(),
            absent.clone(),
            Expected::Refused {
                class: LOCATION_NOT_FOUND,
                wording: Wording::NamesLocation(location_id(&absent)),
            },
        ),
        (
            // Both members name the SAME absent location: it is still a location the
            // account does not hold, which is the refusal a caller has to see first.
            "the same absent location on both sides",
            other_absent.clone(),
            other_absent.clone(),
            Expected::Refused {
                class: LOCATION_NOT_FOUND,
                wording: Wording::NamesLocation(location_id(&other_absent)),
            },
        ),
    ];

    let mut accepted = 0;
    for (case, source, destination, expected) in rows {
        let request = json!({
            "SourceLocationArn": source,
            "DestinationLocationArn": destination,
            "Name": case,
        });
        match expected {
            Expected::Accepted => {
                ok(&router, "CreateTask", request, case).await;
                accepted += 1;
            }
            Expected::Refused { class, wording } => {
                refused(&router, "CreateTask", request, class, &wording, case).await;
            }
        }
    }

    let listed = ok(&router, "ListTasks", json!({}), "listing").await;
    let held = listed["Tasks"].as_array().map(Vec::len).unwrap_or_default();
    assert_eq!(
        held, accepted,
        "case \"listing\": a refused create must store nothing, got {listed}"
    );
}
