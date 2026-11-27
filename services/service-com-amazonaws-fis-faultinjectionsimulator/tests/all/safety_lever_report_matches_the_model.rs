// SPDX-License-Identifier: CC0-1.0

//! The `SafetyLever` every safety-lever operation reports carries EXACTLY the members the
//! service's model declares for it — no more, no fewer — whichever operation reports it,
//! whichever Region it is signed for and whatever state the lever is in.
//!
//! A reported structure is the model's structure: a member the model does not declare is
//! not something a client can read, and a member it does declare has to be filled.
//! `SafetyLever` declares `id`, `arn` and `state` only, so the account the lever belongs to
//! is named by its ARN and nowhere else.
//!
//! Both operations that report a lever — `GetSafetyLever` and `UpdateSafetyLeverState` —
//! are driven over HTTP through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization.
//!
//! The HTTP harness below is local to this suite, as each sibling suite's is.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_fis_faultinjectionsimulator::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The account id every ARN this emulator mints carries
/// (`::engine::EMULATOR_ACCOUNT_ID`), spelled out so a changed account fails here.
const ACCOUNT_ID: &str = "000000000000";

/// The id the account's own safety lever is named by: FIS gives an account one lever and
/// publishes no API that creates or enumerates one.
const LEVER_ID: &str = "default";

/// The member names `SafetyLever` declares, and nothing else.
///
/// SOURCE: the model's `com.amazonaws.fis#SafetyLever` structure
/// (`smithy_query(query="shape",shape="SafetyLever")`), whose members are `id`, `arn` and
/// `state`.
const SAFETY_LEVER_MEMBERS: [&str; 3] = ["id", "arn", "state"];

/// The member names `SafetyLeverState` declares, and nothing else.
///
/// SOURCE: the model's `com.amazonaws.fis#SafetyLeverState` structure.
const SAFETY_LEVER_STATE_MEMBERS: [&str; 2] = ["status", "reason"];

/// The values `SafetyLeverStatus` admits — what a lever can REPORT.
///
/// SOURCE: the model's `com.amazonaws.fis#SafetyLeverStatus` enum.
const REPORTABLE_STATUSES: [&str; 3] = ["disengaged", "engaged", "engaging"];

/// How long the transition the engage owes the clock may take before a report must show
/// it. The emulator's span is deliberately non-zero and well under this.
const TRANSITION_DEADLINE: Duration = Duration::from_secs(30);

/// How long to wait between two polls of a lever whose transition is still due.
const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// A SigV4 credential scope naming this service, for `region`. The service field is what
/// routes the request to fis; the region field is what a minted ARN names.
fn signed_for(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/fis/aws4_request, \
         SignedHeaders=host, Signature=abc"
    )
}

/// One emulator: a router carrying only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// What the emulator answered: the HTTP status and the JSON body.
struct Answer {
    status: u16,
    body: Value,
}

async fn send(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
    region: &str,
) -> Answer {
    let payload = body
        .map(|value| Bytes::from(serde_json::to_vec(&value).expect("the test body serializes")))
        .unwrap_or_default();
    let request = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "fis.us-east-1.amazonaws.com")
        .header("authorization", signed_for(region))
        .header("content-type", "application/json")
        .body(payload)
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "{method} {uri} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    Answer { status, body }
}

async fn get_lever(router: &MultiServiceRouter, region: &str) -> Answer {
    send(
        router,
        "GET",
        &format!("/safetyLevers/{LEVER_ID}"),
        None,
        region,
    )
    .await
}

async fn update_lever(
    router: &MultiServiceRouter,
    status: &str,
    reason: &str,
    region: &str,
) -> Answer {
    send(
        router,
        "PATCH",
        &format!("/safetyLevers/{LEVER_ID}/state"),
        Some(json!({ "state": { "status": status, "reason": reason } })),
        region,
    )
    .await
}

/// The keys of a JSON object, sorted, or a panic naming `case` when it is not one.
fn keys(case: &str, what: &str, value: &Value) -> Vec<String> {
    let object = value
        .as_object()
        .unwrap_or_else(|| panic!("case {case:?}: expected {what} to be an object, got {value:?}"));
    let mut names: Vec<String> = object.keys().cloned().collect();
    names.sort();
    names
}

/// The member names expected of a reported structure, sorted, as `keys` reports them.
fn expected(members: &[&str]) -> Vec<String> {
    let mut names: Vec<String> = members.iter().map(|name| (*name).to_string()).collect();
    names.sort();
    names
}

/// Assert that the `safetyLever` of a 200 answer is the model's structure: exactly the
/// declared members, each filled, the state exactly the declared state members, a status
/// the enum admits, and the account named only by the ARN.
fn assert_is_the_modelled_lever(case: &str, answer: &Answer, region: &str) -> Value {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected HTTP 200, got {} with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        keys(case, "the answer body", &answer.body),
        vec!["safetyLever".to_string()],
        "case {case:?}: expected only the modelled output member, got {:?}",
        answer.body
    );

    let lever = answer.body["safetyLever"].clone();
    assert_eq!(
        keys(case, "the safetyLever", &lever),
        expected(&SAFETY_LEVER_MEMBERS),
        "case {case:?}: expected exactly the members SafetyLever declares, got {lever:?}"
    );
    assert_eq!(
        lever["id"].as_str().unwrap_or_default(),
        LEVER_ID,
        "case {case:?}: expected the lever to name the account's one lever, got {lever:?}"
    );
    // The account the lever belongs to is named by its ARN, which is the only member that
    // carries it.
    assert_eq!(
        lever["arn"].as_str().unwrap_or_default(),
        format!("arn:aws:fis:{region}:{ACCOUNT_ID}:safety-lever/{LEVER_ID}"),
        "case {case:?}: expected the lever arn to name this account and region, got {lever:?}"
    );

    let state = lever["state"].clone();
    assert_eq!(
        keys(case, "the lever state", &state),
        expected(&SAFETY_LEVER_STATE_MEMBERS),
        "case {case:?}: expected exactly the members SafetyLeverState declares, got {state:?}"
    );
    let status = state["status"].as_str().unwrap_or_default();
    assert!(
        REPORTABLE_STATUSES.contains(&status),
        "case {case:?}: expected a modelled SafetyLeverStatus, got {status:?} in {state:?}"
    );
    lever
}

/// Read the lever until it reports `wanted`, or panic naming `case` once the deadline has
/// passed. Each read is asserted to be the model's structure too.
async fn poll_until_status(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    wanted: &str,
) -> Value {
    let deadline = std::time::Instant::now() + TRANSITION_DEADLINE;
    loop {
        let answer = get_lever(router, region).await;
        let lever = assert_is_the_modelled_lever(case, &answer, region);
        if lever["state"]["status"].as_str().unwrap_or_default() == wanted {
            return lever;
        }
        if std::time::Instant::now() >= deadline {
            panic!("case {case:?}: the lever never reported {wanted:?}, last report {lever:?}");
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

#[tokio::test]
async fn every_report_of_the_lever_carries_exactly_the_modelled_members() {
    // The rows differ along the Region the calls are signed for, which is what the
    // reported ARN names; within each Region the lever is reported by BOTH operations and
    // in each state it can be in — untouched, the engage still taking effect, engaged, and
    // disengaged again.
    for region in ["us-east-1", "eu-west-1", "ap-southeast-3", "sa-east-1"] {
        let router = emulator();

        let untouched = format!("untouched-{region}");
        let answer = get_lever(&router, region).await;
        let lever = assert_is_the_modelled_lever(&untouched, &answer, region);
        assert_eq!(
            lever["state"]["status"].as_str().unwrap_or_default(),
            "disengaged",
            "case {untouched:?}: an untouched lever is disengaged, got {lever:?}"
        );

        let engaging = format!("engage-{region}");
        let answer = update_lever(&router, "engaged", "model members", region).await;
        let lever = assert_is_the_modelled_lever(&engaging, &answer, region);
        assert_eq!(
            lever["state"]["status"].as_str().unwrap_or_default(),
            "engaging",
            "case {engaging:?}: an engage takes effect rather than landing at once, got {lever:?}"
        );

        let engaged = format!("engaged-{region}");
        poll_until_status(&router, &engaged, region, "engaged").await;

        let disengaged = format!("disengage-{region}");
        let answer = update_lever(&router, "disengaged", "model members", region).await;
        let lever = assert_is_the_modelled_lever(&disengaged, &answer, region);
        assert_eq!(
            lever["state"]["status"].as_str().unwrap_or_default(),
            "disengaged",
            "case {disengaged:?}: a disengage lands immediately, got {lever:?}"
        );
    }
}
