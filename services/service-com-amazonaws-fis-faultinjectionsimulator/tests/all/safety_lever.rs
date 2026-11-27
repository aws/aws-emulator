// SPDX-License-Identifier: CC0-1.0

//! AWS FIS's safety lever — `GetSafetyLever` and `UpdateSafetyLeverState` — end to end
//! over HTTP.
//!
//! Every assertion here goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization — the same path a
//! client takes. The rules under test are the ones FIS states for the lever:
//!
//! * an account HAS one safety lever, which no API creates, lists or deletes, so it is
//!   named by the id FIS gives it and reports `disengaged` under its initial reason until
//!   an update stores something, under the lever's ARN in the Region the request is signed
//!   for;
//! * the state a request asks for is PERSISTED: `GetSafetyLever` reports the status and
//!   the reason the last `UpdateSafetyLeverState` stored, whatever order the two
//!   admissible states are asked for in and however often;
//! * engaging the lever TAKES EFFECT rather than landing at once — the lever reports the
//!   modelled `engaging`, which no `SafetyLeverStatusInput` can ask for, until the clock
//!   says it has — while disengaging it lands immediately;
//! * engaging it stops the experiments that are running, while a start made after it is
//!   engaged is ACCEPTED and the run it creates is cancelled instead of refused, and
//!   disengaging it lets a run go ahead again;
//! * an `id` that names no lever of this account answers `ResourceNotFoundException`
//!   (HTTP 404), and a `status` outside the input enum `ValidationException` (HTTP 400) —
//!   neither persisting anything;
//! * reading the lever changes nothing a later caller sees.
//!
//! The HTTP harness below is local to this suite, as each sibling suite's is.

use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_fis_faultinjectionsimulator::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

/// The account id every ARN this emulator mints carries
/// (`::engine::EMULATOR_ACCOUNT_ID`), spelled out so a changed account fails here.
const ACCOUNT_ID: &str = "000000000000";

/// The id the account's own safety lever is named by: FIS gives an account one lever and
/// publishes no API that creates or enumerates one, so the lever already exists under this
/// fixed id.
const LEVER_ID: &str = "default";

/// The `state.reason` the lever carries before any update has stored one: the lever exists
/// already, so FIS seeded its state.
const INITIAL_REASON: &str = "Initial default status";

/// The region most cases here sign for.
const REGION: &str = "us-east-1";

/// The values `SafetyLeverStatusInput` admits — the only states a caller can ask for.
///
/// SOURCE: the model's `com.amazonaws.fis#SafetyLeverStatusInput` enum
/// (`smithy_query(query="shape",shape="SafetyLeverStatusInput")`), written out here rather
/// than imported from the crate so a wrong value in the handler fails this suite.
const REQUESTABLE_STATUSES: [&str; 2] = ["disengaged", "engaged"];

/// The values `SafetyLeverStatus` admits — what a lever can REPORT. The one value that is
/// not requestable (`engaging`) is the status the service itself reaches.
///
/// SOURCE: the model's `com.amazonaws.fis#SafetyLeverStatus` enum.
const REPORTABLE_STATUSES: [&str; 3] = ["disengaged", "engaged", "engaging"];

/// How long to wait before a transition this service owes the clock must have been
/// materialised. The emulator's span is deliberately non-zero and well under this.
const PAST_TRANSITION: Duration = Duration::from_millis(900);

/// A wait inside the 1..720 minutes FIS documents for `aws:fis:wait`'s `duration`, long
/// enough that an experiment running it is still running while a case engages the lever.
const WAIT_DURATION: &str = "PT5M";

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

    /// The member `name` of this answer's body.
    fn member(&self, name: &str) -> &Value {
        &self.body[name]
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
    send(
        router,
        "GET",
        &format!("/safetyLevers/{id}"),
        None,
        region,
    )
    .await
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

/// The `safetyLever` of a 200 answer, with the identity rules every report of it
/// satisfies asserted: the lever names itself by the account's one lever id, its ARN names
/// the account in the Region the request was signed for, and its status is one
/// `SafetyLeverStatus` admits.
fn lever(case: &str, answer: &Answer, member: &str, region: &str) -> Value {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected HTTP 200, got {} with {:?}",
        answer.status, answer.body
    );
    let lever = answer.member(member).clone();
    assert_eq!(
        lever["id"].as_str().unwrap_or_default(),
        LEVER_ID,
        "case {case:?}: expected the lever to name the account's one lever, got {lever:?}"
    );
    assert_eq!(
        lever["arn"].as_str().unwrap_or_default(),
        format!("arn:aws:fis:{region}:{ACCOUNT_ID}:safety-lever/{LEVER_ID}"),
        "case {case:?}: expected the lever arn to name this account and region, got {lever:?}"
    );
    let status = lever["state"]["status"].as_str().unwrap_or_default();
    assert!(
        REPORTABLE_STATUSES.contains(&status),
        "case {case:?}: expected a modelled SafetyLeverStatus, got {status:?}"
    );
    lever
}

/// The status a report carries.
fn status_of(lever: &Value) -> String {
    lever["state"]["status"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

#[tokio::test]
async fn an_untouched_lever_is_disengaged_and_names_itself() {
    let router = emulator();

    // The rows differ along the Region the request is signed for: the lever is the
    // account's one lever, and its ARN names the Region it was read in.
    for region in ["us-east-1", "eu-west-1", "ap-southeast-3"] {
        let case = format!("untouched-{region}");
        let answer = get_lever(&router, LEVER_ID, region).await;
        let reported = lever(&case, &answer, "safetyLever", region);
        assert_eq!(
            status_of(&reported),
            "disengaged",
            "case {case:?}: an untouched lever is disengaged, got {reported:?}"
        );
        assert_eq!(
            reported["state"]["reason"].as_str().unwrap_or_default(),
            INITIAL_REASON,
            "case {case:?}: an untouched lever carries its initial reason, got {reported:?}"
        );

        // Reading it changed nothing a later caller sees.
        let again = get_lever(&router, LEVER_ID, region).await;
        assert_eq!(
            again.member("safetyLever"),
            &reported,
            "case {case:?}: a second read reports the same lever, got {:?}",
            again.body
        );
    }
}

/// The member names `SafetyLever` declares, and nothing else.
///
/// SOURCE: the model's `com.amazonaws.fis#SafetyLever` structure
/// (`smithy_query(query="shape",shape="SafetyLever")`), whose members are `id`, `arn` and
/// `state`.
const SAFETY_LEVER_MEMBERS: [&str; 3] = ["id", "arn", "state"];

/// The member names `SafetyLeverState` declares.
///
/// SOURCE: the model's `com.amazonaws.fis#SafetyLeverState` structure.
const SAFETY_LEVER_STATE_MEMBERS: [&str; 2] = ["status", "reason"];

/// The keys of a JSON object, or a panic naming `case` when it is not one.
fn keys(case: &str, what: &str, value: &Value) -> Vec<String> {
    let object = value
        .as_object()
        .unwrap_or_else(|| panic!("case {case:?}: expected {what} to be an object, got {value:?}"));
    object.keys().cloned().collect()
}

#[tokio::test]
async fn the_reported_lever_carries_only_the_members_the_model_declares() {
    // The rows differ along what the lever has been asked for, which is what decides
    // whether `reason` is carried and which status is reported: an untouched lever, one
    // whose engage is still taking effect, and a disengaged one. The disengage is asked of
    // a lever that was engaged first, because a disengage of a lever that already holds
    // nothing stores no reason of its own
    // (`tests/all/safety_lever_status_change.rs`).
    let cases: [(&str, &[&str]); 3] = [
        ("untouched", &[]),
        ("engaged", &["engaged"]),
        ("disengaged", &["engaged", "disengaged"]),
    ];

    for (case, requests) in cases {
        let router = emulator();
        for requested in requests {
            let updated = update_lever(&router, LEVER_ID, requested, case, REGION).await;
            assert_eq!(
                updated.status, 200,
                "case {case:?}: expected HTTP 200 from UpdateSafetyLeverState asking for \
                 {requested:?}, got {} with {:?}",
                updated.status, updated.body
            );
        }

        let answer = get_lever(&router, LEVER_ID, REGION).await;
        assert_eq!(
            keys(case, "the GetSafetyLever body", &answer.body),
            vec!["safetyLever".to_string()],
            "case {case:?}: expected only the modelled output member, got {:?}",
            answer.body
        );

        let reported = lever(case, &answer, "safetyLever", REGION);
        for member in keys(case, "the safetyLever", &reported) {
            assert!(
                SAFETY_LEVER_MEMBERS.contains(&member.as_str()),
                "case {case:?}: {member:?} is not a member SafetyLever declares, got {reported:?}"
            );
        }
        for member in SAFETY_LEVER_MEMBERS {
            assert!(
                !reported[member].is_null(),
                "case {case:?}: expected the lever to carry {member:?}, got {reported:?}"
            );
        }

        let state = reported["state"].clone();
        for member in keys(case, "the lever state", &state) {
            assert!(
                SAFETY_LEVER_STATE_MEMBERS.contains(&member.as_str()),
                "case {case:?}: {member:?} is not a member SafetyLeverState declares, got {state:?}"
            );
        }
        let status = status_of(&reported);
        assert!(
            REPORTABLE_STATUSES.contains(&status.as_str()),
            "case {case:?}: expected a modelled SafetyLeverStatus, got {state:?}"
        );
        if requests.is_empty() {
            // Nothing has stored one, so the lever still carries the reason it was seeded
            // with.
            assert_eq!(
                state["reason"].as_str().unwrap_or_default(),
                INITIAL_REASON,
                "case {case:?}: expected the initial reason on an untouched lever, got {state:?}"
            );
        } else {
            // An update stores the reason it was given, so the lever carries it.
            assert_eq!(
                state["reason"].as_str().unwrap_or_default(),
                case,
                "case {case:?}: expected the stored reason, got {state:?}"
            );
        }
    }
}

#[tokio::test]
async fn reading_the_lever_never_disturbs_the_transition_the_clock_owes() {
    // The rows differ along HOW OFTEN the lever is read inside the window the engage takes
    // to come into effect: reads are reporting, so the count must make no difference to
    // what the lever settles at, and no read may walk the status backwards.
    for reads in [1_usize, 4, 16, 64] {
        let case = format!("reads-{reads}");
        let router = emulator();
        let reason = format!("polled {reads} times");

        let engaged = update_lever(&router, LEVER_ID, "engaged", &reason, REGION).await;
        let reported = lever(&case, &engaged, "safetyLever", REGION);
        assert_eq!(
            status_of(&reported),
            "engaging",
            "case {case:?}: the engage is taking effect, got {reported:?}"
        );

        let mut settled_already = false;
        for index in 0..reads {
            let read = get_lever(&router, LEVER_ID, REGION).await;
            let read_lever = lever(&case, &read, "safetyLever", REGION);
            let status = status_of(&read_lever);
            assert!(
                status == "engaging" || status == "engaged",
                "case {case:?}: read {index} expected engaging or engaged, got {read_lever:?}"
            );
            // Once the clock has moved the lever on, it stays on: a read never takes it
            // back to the transient status.
            if settled_already {
                assert_eq!(
                    status, "engaged",
                    "case {case:?}: read {index} went back to {status:?}, got {read_lever:?}"
                );
            }
            settled_already |= status == "engaged";
            assert_eq!(
                read_lever["state"]["reason"].as_str().unwrap_or_default(),
                reason,
                "case {case:?}: read {index} changed the stored reason, got {read_lever:?}"
            );
        }

        // However often it was read, the transition the engage owed falls due on the
        // clock, not on the reads.
        tokio::time::sleep(PAST_TRANSITION).await;
        let after = get_lever(&router, LEVER_ID, REGION).await;
        let after_lever = lever(&case, &after, "safetyLever", REGION);
        assert_eq!(
            status_of(&after_lever),
            "engaged",
            "case {case:?}: expected the engage to have taken effect, got {after_lever:?}"
        );
        assert_eq!(
            after_lever["state"]["reason"].as_str().unwrap_or_default(),
            reason,
            "case {case:?}: expected the stored reason to survive the reads, got {after_lever:?}"
        );

        // And a read after it settled still changes nothing a later caller sees.
        let again = get_lever(&router, LEVER_ID, REGION).await;
        assert_eq!(
            again.member("safetyLever"),
            &after_lever,
            "case {case:?}: a later read reports the same lever, got {:?}",
            again.body
        );
    }
}

/// One step of a case: the state the request asks for, the status the update itself
/// reports back, and the status a read reports once every transition the clock owes the
/// lever has fallen due.
struct Step {
    requested: &'static str,
    immediate: &'static str,
    settled: &'static str,
}

#[tokio::test]
async fn the_state_a_request_asks_for_is_persisted_and_taken_up_on_the_clock() {
    // The rows differ along the state asked for, the state the lever is already in, and
    // how many updates it takes: each requestable state asked for from the other one, and a
    // lever taken through both and back again. No row repeats the state the lever is already
    // held at: an engage of a holding lever is refused, and a disengage of a lever that
    // holds nothing stores no reason (`tests/all/safety_lever_status_change.rs`).
    let cases: [(&str, Vec<Step>); 3] = [
        (
            "engage-an-untouched-lever",
            vec![Step {
                requested: "engaged",
                immediate: "engaging",
                settled: "engaged",
            }],
        ),
        (
            "disengage-an-engaged-lever",
            vec![
                Step {
                    requested: "engaged",
                    immediate: "engaging",
                    settled: "engaged",
                },
                Step {
                    requested: "disengaged",
                    immediate: "disengaged",
                    settled: "disengaged",
                },
            ],
        ),
        (
            "engage-a-disengaged-lever",
            vec![
                Step {
                    requested: "engaged",
                    immediate: "engaging",
                    settled: "engaged",
                },
                Step {
                    requested: "disengaged",
                    immediate: "disengaged",
                    settled: "disengaged",
                },
                Step {
                    requested: "engaged",
                    immediate: "engaging",
                    settled: "engaged",
                },
            ],
        ),
    ];

    for (case, steps) in cases {
        let router = emulator();
        for (index, step) in steps.iter().enumerate() {
            assert!(
                REQUESTABLE_STATUSES.contains(&step.requested),
                "case {case:?}: step {index} asks for {:?}, which is not a requestable status",
                step.requested
            );
            let reason = format!("{case} step {index}");
            let updated = update_lever(&router, LEVER_ID, step.requested, &reason, REGION).await;
            let reported = lever(case, &updated, "safetyLever", REGION);
            assert_eq!(
                status_of(&reported),
                step.immediate,
                "case {case:?}: step {index} asking for {:?} reports {:?}, got {reported:?}",
                step.requested,
                step.immediate
            );
            assert_eq!(
                reported["state"]["reason"].as_str().unwrap_or_default(),
                reason,
                "case {case:?}: step {index} reports the reason it was given, got {reported:?}"
            );

            // The update is PERSISTED: a read taken straight after it reports the same
            // state, transient status included.
            let read = get_lever(&router, LEVER_ID, REGION).await;
            let read_lever = lever(case, &read, "safetyLever", REGION);
            assert_eq!(
                read_lever, reported,
                "case {case:?}: step {index} expected the read to report what the update \
                 stored ({reported:?}), got {read_lever:?}"
            );

            // And once the clock has caught up, the state the lever settles in — looked for
            // until it holds rather than waited for by the clock of whatever host runs this.
            let settled_lever = poll_lever_until(&router, step.settled).await;
            assert_eq!(
                status_of(&settled_lever),
                step.settled,
                "case {case:?}: step {index} settles at {:?}, got {settled_lever:?}",
                step.settled
            );
            assert_eq!(
                settled_lever["state"]["reason"].as_str().unwrap_or_default(),
                reason,
                "case {case:?}: step {index} keeps the reason it was given, got {settled_lever:?}"
            );
        }
    }
}

#[tokio::test]
async fn the_lever_is_the_accounts_own_whichever_region_reads_it() {
    let router = emulator();
    let engaged_in = "us-east-1";
    let elsewhere_region = "eu-west-1";

    let updated = update_lever(
        &router,
        LEVER_ID,
        "engaged",
        "one lever for the account",
        engaged_in,
    )
    .await;
    let reported = lever("one-lever", &updated, "safetyLever", engaged_in);
    assert_eq!(
        status_of(&reported),
        "engaging",
        "case \"one-lever\": the engage is taking effect, got {reported:?}"
    );

    // The same lever, read through another Region: the state is the account's one state,
    // and only the ARN follows the Region the request was signed for — the same way a
    // catalogued action's ARN does.
    let elsewhere = get_lever(&router, LEVER_ID, elsewhere_region).await;
    let elsewhere_lever = lever("one-lever", &elsewhere, "safetyLever", elsewhere_region);
    assert_eq!(
        elsewhere_lever["state"],
        reported["state"],
        "case \"one-lever\": expected the account's one lever state {:?}, got {:?}",
        reported["state"],
        elsewhere_lever["state"]
    );
    assert_ne!(
        elsewhere_lever["arn"], reported["arn"],
        "case \"one-lever\": expected the arn to name the region it was read in, got {:?}",
        elsewhere_lever["arn"]
    );
}

#[tokio::test]
async fn an_id_that_is_not_this_accounts_names_no_lever() {
    let router = emulator();

    // The rows differ along the SHAPE of the id: an account id, the longest id
    // `SafetyLeverId` admits (`@length(max: 64)`), the lever's id in the wrong case, and
    // ids that differ from it by one character either way.
    for id in [
        ACCOUNT_ID,
        &"a".repeat(64),
        "DEFAULT",
        "defaults",
        "defaul",
    ] {
        let case = format!("unknown-lever-{id}");
        let read = get_lever(&router, id, REGION).await;
        assert_eq!(
            read.status, 404,
            "case {case:?}: expected HTTP 404 from GetSafetyLever, got {} with {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.error_code(),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {:?}",
            read.body
        );

        let updated = update_lever(&router, id, "engaged", "no such lever", REGION).await;
        assert_eq!(
            updated.status, 404,
            "case {case:?}: expected HTTP 404 from UpdateSafetyLeverState, got {} with {:?}",
            updated.status, updated.body
        );
        assert_eq!(
            updated.error_code(),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {:?}",
            updated.body
        );
    }

    // None of those refusals persisted anything: the account's own lever is as it was.
    let own = get_lever(&router, LEVER_ID, REGION).await;
    let own_lever = lever("unknown-lever-aftermath", &own, "safetyLever", REGION);
    assert_eq!(
        status_of(&own_lever),
        "disengaged",
        "case \"unknown-lever-aftermath\": a refused update persists nothing, got {own_lever:?}"
    );
    assert_eq!(
        own_lever["state"]["reason"].as_str().unwrap_or_default(),
        INITIAL_REASON,
        "case \"unknown-lever-aftermath\": a refused update stores no reason, got {own_lever:?}"
    );
}

#[tokio::test]
async fn a_status_outside_the_input_enum_is_refused_and_persists_nothing() {
    let router = emulator();

    // The rows differ along HOW the value misses the enum: the status only the service
    // reaches, the right value in the wrong case, a near miss, and nothing at all.
    for status in ["engaging", "ENGAGED", "disengage", ""] {
        let case = format!("bad-status-{status:?}");
        let answer = update_lever(&router, LEVER_ID, status, "not a modelled state", REGION).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400 from a status outside the input enum, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {:?}",
            answer.body
        );

        let read = get_lever(&router, LEVER_ID, REGION).await;
        let read_lever = lever(&case, &read, "safetyLever", REGION);
        assert_eq!(
            status_of(&read_lever),
            "disengaged",
            "case {case:?}: a refused update leaves the lever as it was, got {read_lever:?}"
        );
        assert_eq!(
            read_lever["state"]["reason"].as_str().unwrap_or_default(),
            INITIAL_REASON,
            "case {case:?}: a refused update stores no reason, got {read_lever:?}"
        );
    }
}

/// Create a template whose single action waits, start an experiment from it, and report
/// the pair of ids (template, experiment).
async fn running_experiment(
    case: &str,
    router: &MultiServiceRouter,
    label: &str,
) -> (String, String) {
    let created = send(
        router,
        "POST",
        "/experimentTemplates",
        Some(json!({
            "clientToken": format!("token-lever-template-{label}"),
            "description": format!("safety lever: {label}"),
            "roleArn": "arn:aws:iam::000000000000:role/fis-safety-lever",
            "stopConditions": [{ "source": "none" }],
            "targets": {},
            "actions": {
                "wait": { "actionId": "aws:fis:wait", "parameters": { "duration": WAIT_DURATION } }
            }
        })),
        REGION,
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case {case:?}: expected HTTP 200 from CreateExperimentTemplate, got {} with {:?}",
        created.status, created.body
    );
    let template_id = created.member("experimentTemplate")["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no template id in {:?}", created.body))
        .to_string();

    let started = start_experiment(router, label, &template_id).await;
    assert_eq!(
        started.status, 200,
        "case {case:?}: expected HTTP 200 from StartExperiment, got {} with {:?}",
        started.status, started.body
    );
    let experiment_id = started.member("experiment")["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no experiment id in {:?}", started.body))
        .to_string();
    (template_id, experiment_id)
}

async fn start_experiment(
    router: &MultiServiceRouter,
    label: &str,
    template_id: &str,
) -> Answer {
    send(
        router,
        "POST",
        "/experiments",
        Some(json!({
            "clientToken": format!("token-lever-experiment-{label}"),
            "experimentTemplateId": template_id
        })),
        REGION,
    )
    .await
}

async fn get_experiment(router: &MultiServiceRouter, id: &str) -> Value {
    send(router, "GET", &format!("/experiments/{id}"), None, REGION)
        .await
        .member("experiment")
        .clone()
}

/// How long a case gives a condition it polls for: generous, because every wait here ends
/// as soon as the condition holds ([`poll_lever_until`], [`poll_experiment_until`]).
const POLL_BUDGET: Duration = Duration::from_secs(60);

/// How often a poll looks again.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// Poll `GetSafetyLever` until the lever reports `status`, or [`POLL_BUDGET`] runs out, and
/// report it as last read.
async fn poll_lever_until(router: &MultiServiceRouter, status: &str) -> Value {
    let deadline = std::time::Instant::now() + POLL_BUDGET;
    loop {
        let answer = get_lever(router, LEVER_ID, REGION).await;
        let reported = answer.member("safetyLever").clone();
        if status_of(&reported) == status || std::time::Instant::now() >= deadline {
            return reported;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Poll `GetExperiment` until the experiment reports `status`, or [`POLL_BUDGET`] runs out,
/// and report it as last read.
async fn poll_experiment_until(router: &MultiServiceRouter, id: &str, status: &str) -> Value {
    let deadline = std::time::Instant::now() + POLL_BUDGET;
    loop {
        let experiment = get_experiment(router, id).await;
        let reported = experiment["state"]["status"].as_str().unwrap_or_default();
        if reported == status || std::time::Instant::now() >= deadline {
            return experiment;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

#[tokio::test]
async fn engaging_the_lever_stops_running_experiments_and_cancels_the_ones_started_under_it() {
    // The rows differ along the status the experiment is in when the lever is engaged: one
    // engaged while the experiment is still initiating, one after it started running.
    for (case, run_first) in [
        ("engaged-while-initiating", false),
        ("engaged-while-running", true),
    ] {
        let router = emulator();
        let (template_id, experiment_id) = running_experiment(case, &router, case).await;
        if run_first {
            let running = poll_experiment_until(&router, &experiment_id, "running").await;
            assert_eq!(
                running["state"]["status"].as_str().unwrap_or_default(),
                "running",
                "case {case:?}: expected the experiment to reach running, got {running:?}"
            );
        }

        let engaged = update_lever(
            &router,
            LEVER_ID,
            "engaged",
            "stop the experiments",
            REGION,
        )
        .await;
        let reported = lever(case, &engaged, "safetyLever", REGION);
        assert_eq!(
            status_of(&reported),
            "engaging",
            "case {case:?}: the engage is taking effect, got {reported:?}"
        );

        // The experiment the lever stopped is on its way out, under a reason of its own.
        let stopping = get_experiment(&router, &experiment_id).await;
        assert_eq!(
            stopping["state"]["status"].as_str().unwrap_or_default(),
            "stopping",
            "case {case:?}: engaging the lever stops the experiment, got {stopping:?}"
        );
        assert!(
            stopping["state"]["reason"]
                .as_str()
                .is_some_and(|reason| !reason.is_empty()),
            "case {case:?}: an experiment the lever stopped carries a reason, got {stopping:?}"
        );

        // And a start made while the lever holds is ACCEPTED — the lever is enforced on the
        // run, which is cancelled rather than refused. What that cancellation reports is
        // asserted in [`crate::safety_lever_cancels_the_run_it_held`].
        let accepted =
            start_experiment(&router, &format!("{case}-while-held"), &template_id).await;
        assert_eq!(
            accepted.status, 200,
            "case {case:?}: expected HTTP 200 from a start while the lever holds, got {} with {:?}",
            accepted.status, accepted.body
        );
        let held_run = accepted.member("experiment")["id"]
            .as_str()
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: the accepted start reports an experiment id, got {:?}",
                    accepted.body
                )
            })
            .to_string();
        let cancelled = poll_experiment_until(&router, &held_run, "cancelled").await;
        assert_eq!(
            cancelled["state"]["status"].as_str().unwrap_or_default(),
            "cancelled",
            "case {case:?}: the run started under the held lever is cancelled, got {cancelled:?}"
        );

        let settled = poll_lever_until(&router, "engaged").await;
        assert_eq!(
            status_of(&settled),
            "engaged",
            "case {case:?}: the engage has taken effect, got {settled:?}"
        );
        let stopped = poll_experiment_until(&router, &experiment_id, "stopped").await;
        assert_eq!(
            stopped["state"]["status"].as_str().unwrap_or_default(),
            "stopped",
            "case {case:?}: the stopped experiment reached stopped, got {stopped:?}"
        );
        assert!(
            !stopped["endTime"].is_null(),
            "case {case:?}: an experiment the lever stopped carries an endTime, got {stopped:?}"
        );

        // Disengaging it lets experiments start again.
        let disengaged = update_lever(&router, LEVER_ID, "disengaged", "resume", REGION).await;
        let reported = lever(case, &disengaged, "safetyLever", REGION);
        assert_eq!(
            status_of(&reported),
            "disengaged",
            "case {case:?}: a disengage lands at once, got {reported:?}"
        );
        let (_, restarted) = running_experiment(case, &router, &format!("{case}-again")).await;
        let running = get_experiment(&router, &restarted).await;
        assert!(
            running["state"]["status"]
                .as_str()
                .is_some_and(|status| status == "initiating" || status == "running"),
            "case {case:?}: a start after the disengage is accepted, got {running:?}"
        );
    }
}
