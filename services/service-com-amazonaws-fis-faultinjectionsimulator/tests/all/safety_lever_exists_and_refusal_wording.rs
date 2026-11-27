// SPDX-License-Identifier: CC0-1.0

//! The safety lever an account HAS, and the words FIS refuses a safety-lever call with.
//!
//! Every assertion goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization — the same path a
//! client takes. The rules under test:
//!
//! * a safety lever is not a record a caller mints — FIS publishes no operation that
//!   creates, lists or deletes one — so the account's lever EXISTS under the id FIS names
//!   it by: the FIRST call a fresh account makes resolves it, reporting `disengaged` under
//!   the reason FIS seeded its state with, and an update of it needs no create;
//! * an `id` that names no lever of the account is refused `ResourceNotFoundException`
//!   (HTTP 404) with a FIXED sentence: it states the miss without echoing what was looked
//!   up, whatever shape that id had;
//! * a state naming a status `SafetyLeverStatusInput` does not admit is refused
//!   `ValidationException` (HTTP 400) naming the rejected value, whatever value it was,
//!   and persists nothing.
//!
//! The HTTP harness below is local to this suite, as each sibling suite's is.

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_fis_faultinjectionsimulator::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

/// The account id every ARN this emulator mints carries (`::engine::EMULATOR_ACCOUNT_ID`),
/// spelled out so a changed account fails here.
const ACCOUNT_ID: &str = "000000000000";

/// The id the account's one safety lever is named by: no API creates or enumerates a
/// lever, so it already exists under this id.
const LEVER_ID: &str = "default";

/// The `state.reason` the lever carries before any update stored one.
const INITIAL_REASON: &str = "Initial default status";

/// The status an untouched lever reports: nothing is holding experiments back.
const INITIAL_STATUS: &str = "disengaged";

/// How FIS states an id that names no safety lever — fixed text, naming nothing from the
/// request.
const NOT_FOUND_MESSAGE: &str = "Safety lever not found";

/// The prefix FIS states a rejected safety-lever status with; the value follows it.
const INVALID_STATUS_PREFIX: &str = "Invalid status: ";

/// The region most cases here sign for.
const REGION: &str = "us-east-1";

/// The values `SafetyLeverStatusInput` admits — the only states a caller can ask for.
///
/// SOURCE: the model's `com.amazonaws.fis#SafetyLeverStatusInput` enum
/// (`smithy_query(query="shape",shape="SafetyLeverStatusInput")`), written out here rather
/// than imported from the crate so a wrong value in the handler fails this suite.
const REQUESTABLE_STATUSES: [&str; 2] = ["disengaged", "engaged"];

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

impl Answer {
    /// The wire error code (`__type`), or the empty string on a success.
    fn error_code(&self) -> &str {
        self.body["__type"].as_str().unwrap_or_default()
    }

    /// The error message, or the empty string when there is none.
    fn message(&self) -> &str {
        self.body["message"].as_str().unwrap_or_default()
    }

    /// The `safetyLever` this answer reports.
    fn lever(&self) -> &Value {
        &self.body["safetyLever"]
    }
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

async fn get_lever(router: &MultiServiceRouter, id: &str, region: &str) -> Answer {
    send(router, "GET", &format!("/safetyLevers/{id}"), None, region).await
}

async fn update_lever(
    router: &MultiServiceRouter,
    id: &str,
    status: &str,
    reason: &str,
    region: &str,
) -> Answer {
    send(
        router,
        "PATCH",
        &format!("/safetyLevers/{id}/state"),
        Some(json!({ "state": { "status": status, "reason": reason } })),
        region,
    )
    .await
}

/// The status and the reason a reported lever carries.
fn state_of(lever: &Value) -> (String, String) {
    (
        lever["state"]["status"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        lever["state"]["reason"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
    )
}

#[tokio::test]
async fn the_account_has_its_lever_before_any_call_creates_one() {
    // The rows differ along the Region the FIRST call of a fresh account signs for: the
    // lever exists whichever Region asks for it, and its ARN names that Region.
    for region in ["us-east-1", "eu-west-1", "ap-southeast-3", "sa-east-1"] {
        let case = format!("first-call-{region}");
        // A FRESH emulator per row: nothing has created, updated or read a lever.
        let router = emulator();

        let answer = get_lever(&router, LEVER_ID, region).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected HTTP 200 from the account's lever, got {} with {:?}",
            answer.status, answer.body
        );
        let reported = answer.lever().clone();
        assert_eq!(
            reported["id"].as_str().unwrap_or_default(),
            LEVER_ID,
            "case {case:?}: expected the lever to name itself {LEVER_ID:?}, got {reported:?}"
        );
        assert_eq!(
            reported["arn"].as_str().unwrap_or_default(),
            format!("arn:aws:fis:{region}:{ACCOUNT_ID}:safety-lever/{LEVER_ID}"),
            "case {case:?}: expected the lever arn to name this account and region, got \
             {reported:?}"
        );
        assert_eq!(
            state_of(&reported),
            (INITIAL_STATUS.to_string(), INITIAL_REASON.to_string()),
            "case {case:?}: expected the lever FIS seeded, got {reported:?}"
        );

        // Reading it changed nothing a later caller sees.
        let again = get_lever(&router, LEVER_ID, region).await;
        assert_eq!(
            again.lever(),
            &reported,
            "case {case:?}: a second read reports the same lever, got {:?}",
            again.body
        );
    }
}

#[tokio::test]
async fn the_lever_that_already_exists_takes_an_update_without_a_create() {
    // The rows differ along the state asked for of a lever NOTHING created: each
    // requestable status, with the status the update itself reports back (an engage takes
    // effect, so it reports the transient status the service reaches). The disengage is
    // asked of a lever engaged first, because a disengage of a lever that already holds
    // nothing stores no reason of its own
    // (`tests/all/safety_lever_status_change.rs`).
    let cases: [(&str, &str, bool); 2] = [
        ("engaged", "engaging", false),
        ("disengaged", "disengaged", true),
    ];

    for (requested, immediate, engage_first) in cases {
        let case = format!("update-without-create-{requested}");
        assert!(
            REQUESTABLE_STATUSES.contains(&requested),
            "case {case:?}: {requested:?} is not a requestable status"
        );
        let router = emulator();
        let reason = format!("asked for {requested}");

        if engage_first {
            let engaged = update_lever(&router, LEVER_ID, "engaged", "engaged first", REGION).await;
            assert_eq!(
                engaged.status, 200,
                "case {case:?}: expected the lever to take the engage this row starts from, \
                 got {} with {:?}",
                engaged.status, engaged.body
            );
        }

        let updated = update_lever(&router, LEVER_ID, requested, &reason, REGION).await;
        assert_eq!(
            updated.status, 200,
            "case {case:?}: expected HTTP 200 from UpdateSafetyLeverState, got {} with {:?}",
            updated.status, updated.body
        );
        assert_eq!(
            state_of(updated.lever()),
            (immediate.to_string(), reason.clone()),
            "case {case:?}: expected the update to report {immediate:?} under its reason, got {:?}",
            updated.body
        );

        // Round trip: the read reports what the update stored.
        let read = get_lever(&router, LEVER_ID, REGION).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected HTTP 200 from GetSafetyLever, got {} with {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.lever(),
            updated.lever(),
            "case {case:?}: expected the read to report what the update stored ({:?}), got {:?}",
            updated.lever(),
            read.body
        );
    }
}

#[tokio::test]
async fn an_id_that_names_no_lever_is_refused_in_fixed_words() {
    let router = emulator();

    // The rows differ along the SHAPE of the id: the emulator account's own id, another
    // account's, the lever id in the wrong case, a near miss either side of it, a word,
    // and the longest id `SafetyLeverId` admits (`@length(max: 64)`).
    let ids: [String; 7] = [
        ACCOUNT_ID.to_string(),
        "123456789012".to_string(),
        "DEFAULT".to_string(),
        "defaults".to_string(),
        "defaul".to_string(),
        "kill-switch".to_string(),
        "a".repeat(64),
    ];

    for id in &ids {
        for (method, answer) in [
            ("GetSafetyLever", get_lever(&router, id, REGION).await),
            (
                "UpdateSafetyLeverState",
                update_lever(&router, id, "engaged", "no such lever", REGION).await,
            ),
        ] {
            let case = format!("{method}-unknown-{id}");
            assert_eq!(
                answer.status, 404,
                "case {case:?}: expected HTTP 404, got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code(),
                "ResourceNotFoundException",
                "case {case:?}: expected ResourceNotFoundException, got {:?}",
                answer.body
            );
            assert_eq!(
                answer.message(),
                NOT_FOUND_MESSAGE,
                "case {case:?}: expected the fixed refusal wording, got {:?}",
                answer.message()
            );
            assert!(
                !answer.message().contains(id.as_str()),
                "case {case:?}: the refusal echoes the id that was looked up, got {:?}",
                answer.message()
            );
        }
    }

    // None of those refusals touched the lever the account does have.
    let own = get_lever(&router, LEVER_ID, REGION).await;
    assert_eq!(
        state_of(own.lever()),
        (INITIAL_STATUS.to_string(), INITIAL_REASON.to_string()),
        "case \"unknown-lever-aftermath\": a refused call persists nothing, got {:?}",
        own.body
    );
}

#[tokio::test]
async fn a_status_outside_the_input_enum_is_refused_naming_the_value_it_rejected() {
    // The rows differ along HOW the value misses the enum: the status only the service
    // reaches, an admitted value in the wrong case, a near miss, a word of another API's
    // dialect, a number, nothing at all, and a value long enough to be worth echoing.
    let statuses: [String; 8] = [
        "engaging".to_string(),
        "ENGAGED".to_string(),
        "disengage".to_string(),
        "enabled".to_string(),
        "0".to_string(),
        String::new(),
        " engaged".to_string(),
        "z".repeat(100),
    ];

    for status in &statuses {
        let case = format!("bad-status-{status:?}");
        assert!(
            !REQUESTABLE_STATUSES.contains(&status.as_str()),
            "case {case:?}: {status:?} is a requestable status, so it belongs in another row"
        );
        let router = emulator();

        let answer = update_lever(&router, LEVER_ID, status, "not a modelled state", REGION).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {:?}",
            answer.body
        );
        assert_eq!(
            answer.message(),
            format!("{INVALID_STATUS_PREFIX}{status}"),
            "case {case:?}: expected the rejected status to be named, got {:?}",
            answer.message()
        );

        // The refusal persisted nothing: the lever is the one the account started with.
        let read = get_lever(&router, LEVER_ID, REGION).await;
        assert_eq!(
            state_of(read.lever()),
            (INITIAL_STATUS.to_string(), INITIAL_REASON.to_string()),
            "case {case:?}: a refused update leaves the lever as it was, got {:?}",
            read.body
        );
    }
}
