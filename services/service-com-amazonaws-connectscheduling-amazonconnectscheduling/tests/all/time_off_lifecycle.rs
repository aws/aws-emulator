// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of the time off request lifecycle, driven over the wire (restJson1)
//! through the same `MultiServiceRouter` the production router serves with.
//!
//! Every assertion is about the RULE each operation follows, exercised over a table of
//! inputs rather than one request:
//!
//! * a create mints a `TimeOffId`, persists the whole request under it, and `GetTimeOff`
//!   answers exactly what was stored — whichever form (id or ARN) the request addressed the
//!   instance, the user and the shift activity by, and whatever subset of the optional
//!   members it carried;
//! * the status is the service's, never the input's: `AWAIT_APPROVAL` waits in
//!   `AWAITING_APPROVAL`, `AUTOMATIC` is decided at once from stored state;
//! * one decision, two callers: the overlap check against the user's `APPROVED` time off and
//!   the balance the request draws on decide an `AUTOMATIC` create and an approval alike;
//! * `PutTimeOffBalance` REPLACES the stored balance, in whatever unit it is sent;
//! * each lifecycle call moves the stored status, is observable in the next `GetTimeOff`, and
//!   is refused with the modelled `ConflictException` where the current status does not admit
//!   it; and `GetTimeOff` itself writes nothing.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use service_com_amazonaws_connectscheduling_amazonconnectscheduling::state::{
    self, TimeOffStatus, MAX_INSTANCES_HELD, MAX_TIME_OFF_BALANCES_PER_INSTANCE,
    MAX_TIME_OFF_REQUESTS_PER_INSTANCE,
};
use smithy_server_runtime::MultiServiceRouter;

/// The region the test signs for.
const REGION: &str = "us-east-1";

/// The canonical emulator account id (`emulator_server::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = "000000000000";

/// An instance id of the form `InstanceIdOrArn` requires.
const INSTANCE: &str = "11111111-2222-3333-4444-555555555555";

/// A second instance, for the rows that show state is per instance.
const OTHER_INSTANCE: &str = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";

const USER: &str = "user-1";
const OTHER_USER: &str = "user-2";
const ACTIVITY: &str = "vacation";

/// A `TimeOffId` of the modelled form (`^[a-z0-9-]+$`, len 1..64) that no create minted.
const UNKNOWN_TIME_OFF_ID: &str = "00000000-0000-4000-8000-000000000000";

/// The ARN form of an instance identifier, which `InstanceIdOrArn` also admits.
fn instance_arn(instance: &str) -> String {
    format!("arn:aws:connect:{REGION}:{ACCOUNT}:instance/{instance}")
}

/// The ARN form of a user identifier: a Connect user lives inside an instance.
fn user_arn(instance: &str, user: &str) -> String {
    format!("arn:aws:connect:{REGION}:{ACCOUNT}:instance/{instance}/agent/{user}")
}

/// The ARN form of a shift activity identifier.
fn activity_arn(instance: &str, activity: &str) -> String {
    format!("arn:aws:connect:{REGION}:{ACCOUNT}:instance/{instance}/shift-activity/{activity}")
}

/// Percent-encode an `@httpLabel` value the way an SDK does: a non-greedy label is ONE path
/// segment, so the `:` and `/` of an ARN travel encoded.
fn label(value: &str) -> String {
    value
        .chars()
        .map(|c| match c {
            ':' => "%3A".to_string(),
            '/' => "%2F".to_string(),
            other => other.to_string(),
        })
        .collect()
}

/// The access key id the suites sign with when a row is not about the caller.
const DEFAULT_AKID: &str = "AKIDEXAMPLE";

/// A SigV4 credential scope naming this service's signing name (`connect`) and `REGION`;
/// `MultiServiceRouter` routes on the scope's service name.
fn authorization(akid: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential={akid}/20260101/{REGION}/connect/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=deadbeef"
    )
}

/// The emulator under test: one shared engine, mounted in the router the production boot
/// path assembles.
struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
    /// The access key id every call of this emulator signs with.
    akid: String,
}

impl Emulator {
    fn new() -> Self {
        Self::signing_as(DEFAULT_AKID)
    }

    /// An emulator whose calls present `akid`, so a suite reading the records its calls
    /// leave can tell them from a sibling suite's.
    fn signing_as(akid: &str) -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_connectscheduling_amazonconnectscheduling::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        Self {
            router,
            engine,
            akid: akid.to_string(),
        }
    }

    /// Call `path` with `method` and an optional JSON body, and answer the status and the
    /// parsed body (`Value::Null` for an empty one).
    async fn call(&self, method: &str, path: &str, body: Option<&Value>) -> (u16, Value) {
        let payload = body.map(Value::to_string).unwrap_or_default();
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://connect.{REGION}.amazonaws.com{path}"))
            .header("host", format!("connect.{REGION}.amazonaws.com"))
            .header("content-type", "application/json")
            .header("authorization", authorization(&self.akid))
            .body(bytes::Bytes::from(payload))
            .expect("build the request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let raw = response.into_body();
        let parsed = if raw.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&raw).unwrap_or_else(|error| {
                panic!(
                    "{method} {path}: response body is not JSON ({error}): {}",
                    String::from_utf8_lossy(&raw)
                )
            })
        };
        (status, parsed)
    }

    async fn put_balance(
        &self,
        instance: &str,
        user: &str,
        activity: &str,
        value: i32,
        unit: &str,
    ) -> (u16, Value) {
        self.call(
            "PUT",
            &format!(
                "/scheduling/instances/{}/users/{}/timeoff-balances/{}",
                label(instance),
                label(user),
                label(activity)
            ),
            Some(&json!({"Balance": {"Value": value, "Unit": unit}})),
        )
        .await
    }

    async fn create(&self, instance: &str, user: &str, request: &Value) -> (u16, Value) {
        self.call(
            "POST",
            &format!(
                "/scheduling/instances/{}/users/{}/timeoffs",
                label(instance),
                label(user)
            ),
            Some(request),
        )
        .await
    }

    async fn get(&self, instance: &str, user: &str, time_off_id: &str) -> (u16, Value) {
        self.call(
            "GET",
            &format!(
                "/scheduling/instances/{}/users/{}/timeoffs/{}",
                label(instance),
                label(user),
                label(time_off_id)
            ),
            None,
        )
        .await
    }

    /// `POST .../timeoffs/{id}/{action}` — the three lifecycle calls share a shape.
    async fn act(
        &self,
        action: &str,
        instance: &str,
        user: &str,
        time_off_id: &str,
        body: &Value,
    ) -> (u16, Value) {
        self.call(
            "POST",
            &format!(
                "/scheduling/instances/{}/users/{}/timeoffs/{}/{action}",
                label(instance),
                label(user),
                label(time_off_id)
            ),
            Some(body),
        )
        .await
    }

    /// The create's `TimeOffId`, asserting the create succeeded.
    async fn create_ok(&self, instance: &str, user: &str, request: &Value) -> String {
        let (status, body) = self.create(instance, user, request).await;
        assert_eq!(
            status, 200,
            "create {request}: expected 200, got {status} with {body}"
        );
        body.get("TimeOffId")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("create {request}: no TimeOffId in {body}"))
            .to_string()
    }

    /// The stored request as `GetTimeOff` reports it, asserting the read succeeded.
    async fn get_ok(&self, instance: &str, user: &str, time_off_id: &str) -> Value {
        let (status, body) = self.get(instance, user, time_off_id).await;
        assert_eq!(
            status, 200,
            "get {time_off_id}: expected 200, got {status} with {body}"
        );
        body
    }

    /// The status `GetTimeOff` reports for a stored request.
    async fn status_of(&self, instance: &str, user: &str, time_off_id: &str) -> String {
        let body = self.get_ok(instance, user, time_off_id).await;
        body.get("Status")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("get {time_off_id}: no Status in {body}"))
            .to_string()
    }

    /// How many requests the instance's store holds — the count a create must not grow when
    /// it is refused or replayed.
    fn stored_count(&self, instance: &str) -> usize {
        state::service_state(&*self.engine)
            .expect("the emulator's own engine exposes this service's store")
            .lock()
            .instance(instance)
            .map(|held| held.time_offs.len())
            .unwrap_or_default()
    }

    /// The balance the store holds for a (user, activity) pair, as sent.
    fn stored_balance(&self, instance: &str, user: &str, activity: &str) -> Option<(i32, String)> {
        state::service_state(&*self.engine)
            .expect("the emulator's own engine exposes this service's store")
            .lock()
            .instance(instance)
            .and_then(|held| {
                held.balances
                    .get(&(user.to_string(), activity.to_string()))
                    .map(|balance| (balance.value, balance.unit.as_wire().to_string()))
            })
    }

    /// How many instances the store files state under, and how many balances one of them
    /// holds — the two counts the store's bounds cap.
    fn stored_instances(&self) -> usize {
        state::service_state(&*self.engine)
            .expect("the emulator's own engine exposes this service's store")
            .lock()
            .instances
            .len()
    }

    fn stored_balances(&self, instance: &str) -> usize {
        state::service_state(&*self.engine)
            .expect("the emulator's own engine exposes this service's store")
            .lock()
            .instance(instance)
            .map(|held| held.balances.len())
            .unwrap_or_default()
    }

    /// Put a stored request into `status` directly.
    ///
    /// `PENDING` is a status no operation of this service produces — the service decides a
    /// request rather than leaving it processing — so it is reached by writing the store the
    /// handlers read, which is how the rows that pin the `PENDING` refusals are set up.
    fn force_status(&self, instance: &str, time_off_id: &str, status: TimeOffStatus) {
        let store = state::service_state(&*self.engine)
            .expect("the emulator's own engine exposes this service's store");
        let mut guard = store.lock();
        let record = guard
            .instance_mut(instance)
            .time_offs
            .get_mut(time_off_id)
            .unwrap_or_else(|| panic!("the store holds no request {time_off_id}"));
        record.status = status;
    }
}

/// The error code (`__type`) a failure response carries.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .to_string()
}

/// Seconds since the epoch, the unit restJson1 encodes a `Timestamp` member in by default.
fn now_secs() -> i64 {
    i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("the clock is past the epoch")
            .as_secs(),
    )
    .expect("a representable instant")
}

/// A timestamp member of a response, in epoch milliseconds, so a window sent with
/// millisecond precision can be compared to the one stored.
fn millis_of(case: &str, body: &Value, member: &str) -> i64 {
    let value = body
        .get(member)
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("case {case:?}: {member} must be an epoch number, got {body:?}"));
    (value * 1000.0).round() as i64
}

/// A create request body, with the window placed relative to now.
fn create_body(activity: &str, start_secs: f64, end_secs: f64, approval_type: &str) -> Value {
    json!({
        "ShiftActivityId": activity,
        "StartAt": start_secs,
        "EndAt": end_secs,
        "ApprovalType": approval_type,
    })
}

/// A window `days` days from now, `hours` long.
fn window(days: i64, hours: i64) -> (f64, f64) {
    let start = (now_secs() + days * 86_400) as f64;
    (start, start + (hours * 3_600) as f64)
}

/// One row of the create table: what the request carries, and what the read back must show.
struct CreateCase {
    label: &'static str,
    /// How the request addresses the instance and the user; the activity it asks for is in
    /// the request body.
    instance: String,
    user: String,
    request: Value,
    expected_status: &'static str,
    expected_description: Option<&'static str>,
}

/// The create cases, differing along what the create rule generalises over: the form of each
/// identifier (id or ARN), which optional members the request carries, the precision of the
/// window, and the `ApprovalType` that decides the status.
fn create_cases() -> Vec<CreateCase> {
    let (start, end) = window(7, 8);
    let (later_start, later_end) = window(30, 24);
    vec![
        CreateCase {
            label: "only the required members, waiting for approval",
            instance: INSTANCE.to_string(),
            user: USER.to_string(),
            request: create_body(ACTIVITY, start, end, "AWAIT_APPROVAL"),
            expected_status: "AWAITING_APPROVAL",
            expected_description: None,
        },
        CreateCase {
            label: "a description",
            instance: INSTANCE.to_string(),
            user: USER.to_string(),
            request: {
                let mut body = create_body(ACTIVITY, later_start, later_end, "AWAIT_APPROVAL");
                body["Description"] = json!("spring break");
                body
            },
            expected_status: "AWAITING_APPROVAL",
            expected_description: Some("spring break"),
        },
        CreateCase {
            label: "the instance addressed by ARN",
            instance: instance_arn(INSTANCE),
            user: USER.to_string(),
            request: create_body(ACTIVITY, start, end, "AWAIT_APPROVAL"),
            expected_status: "AWAITING_APPROVAL",
            expected_description: None,
        },
        CreateCase {
            label: "the user addressed by ARN",
            instance: INSTANCE.to_string(),
            user: user_arn(INSTANCE, USER),
            request: create_body(ACTIVITY, start, end, "AWAIT_APPROVAL"),
            expected_status: "AWAITING_APPROVAL",
            expected_description: None,
        },
        CreateCase {
            label: "the shift activity addressed by ARN",
            instance: INSTANCE.to_string(),
            user: USER.to_string(),
            request: create_body(
                &activity_arn(INSTANCE, ACTIVITY),
                start,
                end,
                "AWAIT_APPROVAL",
            ),
            expected_status: "AWAITING_APPROVAL",
            expected_description: None,
        },
        CreateCase {
            label: "a window carrying milliseconds",
            instance: INSTANCE.to_string(),
            user: USER.to_string(),
            request: create_body(ACTIVITY, start + 0.25, end + 0.75, "AWAIT_APPROVAL"),
            expected_status: "AWAITING_APPROVAL",
            expected_description: None,
        },
        CreateCase {
            label: "decided automatically, with no balance to fail",
            instance: INSTANCE.to_string(),
            user: OTHER_USER.to_string(),
            request: create_body(ACTIVITY, start, end, "AUTOMATIC"),
            expected_status: "APPROVED",
            expected_description: None,
        },
        CreateCase {
            label: "decided automatically, with an override of the rules",
            instance: INSTANCE.to_string(),
            user: OTHER_USER.to_string(),
            request: {
                let mut body = create_body("sick-leave", later_start, later_end, "AUTOMATIC");
                body["RuleFailureAction"] = json!("OVERRIDE");
                body
            },
            expected_status: "APPROVED",
            expected_description: None,
        },
    ]
}

#[tokio::test]
async fn a_created_request_is_read_back_exactly_as_it_was_stored() {
    let emulator = Emulator::new();
    let expected_actor = json!({
        "AWSIdentityArn": service_common::caller_principal::ambient_user_arn(),
    });

    for case in create_cases() {
        let instance = case.instance.clone();
        let time_off_id = emulator
            .create_ok(&instance, &case.user, &case.request)
            .await;
        assert!(
            !time_off_id.is_empty()
                && time_off_id.len() <= 64
                && time_off_id
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'),
            "case {:?}: TimeOffId must satisfy the modelled ^[a-z0-9-]+$ in 1..64, got \
             {time_off_id:?}",
            case.label
        );

        let read = emulator.get_ok(&instance, &case.user, &time_off_id).await;
        assert_eq!(
            read.get("TimeOffId").and_then(Value::as_str),
            Some(time_off_id.as_str()),
            "case {:?}: the read must answer the id the create minted, got {read:?}",
            case.label
        );
        assert_eq!(
            read.get("UserId").and_then(Value::as_str),
            Some(state::resource_id(&case.user)),
            "case {:?}: UserId must be the id of the user addressed, got {read:?}",
            case.label
        );
        assert_eq!(
            read.get("ShiftActivityId").and_then(Value::as_str),
            Some(state::resource_id(
                case.request["ShiftActivityId"].as_str().expect("a string")
            )),
            "case {:?}: ShiftActivityId must be the id of the activity requested, got {read:?}",
            case.label
        );
        assert_eq!(
            millis_of(case.label, &read, "StartAt"),
            (case.request["StartAt"].as_f64().expect("a number") * 1000.0).round() as i64,
            "case {:?}: StartAt must be the instant sent, to the millisecond, got {read:?}",
            case.label
        );
        assert_eq!(
            millis_of(case.label, &read, "EndAt"),
            (case.request["EndAt"].as_f64().expect("a number") * 1000.0).round() as i64,
            "case {:?}: EndAt must be the instant sent, to the millisecond, got {read:?}",
            case.label
        );
        assert_eq!(
            read.get("Status").and_then(Value::as_str),
            Some(case.expected_status),
            "case {:?}: the service assigns the status, got {read:?}",
            case.label
        );
        assert_eq!(
            read.get("Description").and_then(Value::as_str),
            case.expected_description,
            "case {:?}: the description is the one the create carried, got {read:?}",
            case.label
        );
        assert_eq!(
            read.get("CreatedBy"),
            Some(&expected_actor),
            "case {:?}: CreatedBy must name the identity the request speaks for, got {read:?}",
            case.label
        );
        assert_eq!(
            read.get("UpdatedBy"),
            None,
            "case {:?}: UpdatedBy is absent until an approve, reject or cancel call, got \
             {read:?}",
            case.label
        );
        let created_at = millis_of(case.label, &read, "CreatedAt");
        let updated_at = millis_of(case.label, &read, "UpdatedAt");
        assert!(
            created_at > 0 && updated_at >= created_at,
            "case {:?}: CreatedAt and UpdatedAt must be populated, got {created_at} and \
             {updated_at}",
            case.label
        );
        assert_eq!(
            read.get("ValidationResults"),
            None,
            "case {:?}: a request no rule failed for carries no validation results, got \
             {read:?}",
            case.label
        );

        // The id answers only inside the instance and for the user it belongs to.
        let (status, body) = emulator.get(OTHER_INSTANCE, &case.user, &time_off_id).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (404, "ResourceNotFoundException"),
            "case {:?}: another instance holds no such request, got {status} with {body}",
            case.label
        );
    }
}

#[tokio::test]
async fn a_repeated_client_token_answers_the_first_create_and_stores_one_record() {
    let emulator = Emulator::new();
    let (start, end) = window(7, 8);
    let mut request = create_body(ACTIVITY, start, end, "AWAIT_APPROVAL");
    request["ClientToken"] = json!("tok-1");

    let first = emulator.create_ok(INSTANCE, USER, &request).await;
    assert_eq!(
        emulator.stored_count(INSTANCE),
        1,
        "the first create stores one record"
    );

    // The same token with the same body: the first create's id, and no second record.
    let replay = emulator.create_ok(INSTANCE, USER, &request).await;
    assert_eq!(
        replay, first,
        "a retry carrying the minting ClientToken must answer the first create's id"
    );
    assert_eq!(
        emulator.stored_count(INSTANCE),
        1,
        "a retry carrying the minting ClientToken must store no second record"
    );

    // The same token with a different body is refused, and still stores nothing.
    for (case, changed) in [
        ("another activity", {
            let mut body = request.clone();
            body["ShiftActivityId"] = json!("sick-leave");
            body
        }),
        ("another window", {
            let mut body = request.clone();
            body["EndAt"] = json!(end + 3_600.0);
            body
        }),
        ("another description", {
            let mut body = request.clone();
            body["Description"] = json!("changed my mind");
            body
        }),
        ("another approval type", {
            let mut body = request.clone();
            body["ApprovalType"] = json!("AUTOMATIC");
            body
        }),
    ] {
        let (status, body) = emulator.create(INSTANCE, USER, &changed).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {case:?}: reusing a ClientToken with a different body is refused, got \
             {status} with {body}"
        );
        assert_eq!(
            emulator.stored_count(INSTANCE),
            1,
            "case {case:?}: a refused create stores nothing"
        );
        assert_eq!(
            emulator.stored_instances(),
            1,
            "case {case:?}: a refused create files no state under another instance"
        );
    }

    // A different token is a different create.
    let mut second = request.clone();
    second["ClientToken"] = json!("tok-2");
    let other = emulator.create_ok(INSTANCE, USER, &second).await;
    assert_ne!(
        other, first,
        "a create carrying another ClientToken mints its own request"
    );
    assert_eq!(
        emulator.stored_count(INSTANCE),
        2,
        "a create carrying another ClientToken stores its own record"
    );

    // A create carrying no token is never a replay of one that did.
    let untokened = emulator
        .create_ok(INSTANCE, USER, &create_body(ACTIVITY, start, end, "AWAIT_APPROVAL"))
        .await;
    assert!(
        untokened != first && untokened != other,
        "a create carrying no ClientToken mints its own request, got {untokened}"
    );
    assert_eq!(emulator.stored_count(INSTANCE), 3);

    // The token is the instance's: the same token names no create in another instance.
    let elsewhere = emulator.create_ok(OTHER_INSTANCE, USER, &request).await;
    assert_ne!(
        elsewhere, first,
        "a ClientToken recorded for one instance replays no create in another"
    );
}

#[tokio::test]
async fn a_put_balance_replaces_the_stored_balance_in_whatever_unit_it_is_sent() {
    let emulator = Emulator::new();

    // Every unit the model declares (`DurationUnit`: MINUTES, HOURS, DAYS), each stored as
    // sent and each REPLACING what the previous call left.
    for (case, value, unit) in [
        ("minutes", 2_400, "MINUTES"),
        ("hours", 40, "HOURS"),
        ("days", 3, "DAYS"),
        ("a negative balance", -60, "MINUTES"),
        ("the modelled maximum", 52_560_000, "MINUTES"),
        ("the modelled minimum", -52_560_000, "MINUTES"),
        ("zero", 0, "HOURS"),
    ] {
        let (status, body) = emulator
            .put_balance(INSTANCE, USER, ACTIVITY, value, unit)
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: PutTimeOffBalance succeeds, got {status} with {body}"
        );
        assert!(
            body.is_null() || body == json!({}),
            "case {case:?}: PutTimeOffBalance has no output members, got {body}"
        );
        assert_eq!(
            emulator.stored_balance(INSTANCE, USER, ACTIVITY),
            Some((value, unit.to_string())),
            "case {case:?}: the stored balance is the value and unit as sent, replacing the \
             previous one"
        );
    }

    // The balance is per (user, activity) pair: neither of these sees the other's.
    emulator
        .put_balance(INSTANCE, USER, ACTIVITY, 90, "MINUTES")
        .await;
    emulator
        .put_balance(INSTANCE, OTHER_USER, ACTIVITY, 10, "HOURS")
        .await;
    emulator
        .put_balance(INSTANCE, USER, "sick-leave", 2, "DAYS")
        .await;
    for (case, user, activity, expected) in [
        ("the first user", USER, ACTIVITY, (90, "MINUTES")),
        ("the second user", OTHER_USER, ACTIVITY, (10, "HOURS")),
        ("another activity", USER, "sick-leave", (2, "DAYS")),
    ] {
        assert_eq!(
            emulator.stored_balance(INSTANCE, user, activity),
            Some((expected.0, expected.1.to_string())),
            "case {case:?}: one pair's balance never answers for another's"
        );
    }
    assert_eq!(
        emulator.stored_balance(OTHER_INSTANCE, USER, ACTIVITY),
        None,
        "a balance is held by the instance it was written for"
    );

    // And the replacement is observable through the decision the balance drives: a balance
    // replaced by a smaller one rejects a request the larger one would have approved.
    let (start, end) = window(7, 8);
    emulator
        .put_balance(OTHER_INSTANCE, USER, ACTIVITY, 40, "HOURS")
        .await;
    emulator
        .put_balance(OTHER_INSTANCE, USER, ACTIVITY, 1, "MINUTES")
        .await;
    let decided = emulator
        .create_ok(
            OTHER_INSTANCE,
            USER,
            &create_body(ACTIVITY, start, end, "AUTOMATIC"),
        )
        .await;
    assert_eq!(
        emulator.status_of(OTHER_INSTANCE, USER, &decided).await,
        "REJECTED",
        "the balance a later put stored replaces the earlier one rather than adding to it"
    );
}

/// One row of the lifecycle table: the status a request is put in, the call made on it, and
/// what that call answers and leaves.
struct LifecycleCase {
    status: TimeOffStatus,
    action: &'static str,
    expected_code: u16,
    expected_status: &'static str,
    /// Whether the call must leave the stored request untouched.
    unchanged: bool,
}

/// Every (status, call) pair of the lifecycle, including the three refusals the service
/// documents (`PENDING` for each call, and `CANCELLED` for approve and reject) and the three
/// repeats that "succeed without a change".
fn lifecycle_cases() -> Vec<LifecycleCase> {
    vec![
        LifecycleCase { status: TimeOffStatus::AwaitingApproval, action: "approve", expected_code: 200, expected_status: "APPROVED", unchanged: false },
        LifecycleCase { status: TimeOffStatus::AwaitingApproval, action: "reject", expected_code: 200, expected_status: "REJECTED", unchanged: false },
        LifecycleCase { status: TimeOffStatus::AwaitingApproval, action: "cancel", expected_code: 200, expected_status: "CANCELLED", unchanged: false },
        LifecycleCase { status: TimeOffStatus::Approved, action: "approve", expected_code: 200, expected_status: "APPROVED", unchanged: true },
        LifecycleCase { status: TimeOffStatus::Approved, action: "reject", expected_code: 200, expected_status: "REJECTED", unchanged: false },
        LifecycleCase { status: TimeOffStatus::Approved, action: "cancel", expected_code: 200, expected_status: "CANCELLED", unchanged: false },
        LifecycleCase { status: TimeOffStatus::Rejected, action: "approve", expected_code: 200, expected_status: "APPROVED", unchanged: false },
        LifecycleCase { status: TimeOffStatus::Rejected, action: "reject", expected_code: 200, expected_status: "REJECTED", unchanged: true },
        LifecycleCase { status: TimeOffStatus::Rejected, action: "cancel", expected_code: 200, expected_status: "CANCELLED", unchanged: false },
        LifecycleCase { status: TimeOffStatus::Cancelled, action: "approve", expected_code: 409, expected_status: "CANCELLED", unchanged: true },
        LifecycleCase { status: TimeOffStatus::Cancelled, action: "reject", expected_code: 409, expected_status: "CANCELLED", unchanged: true },
        LifecycleCase { status: TimeOffStatus::Cancelled, action: "cancel", expected_code: 200, expected_status: "CANCELLED", unchanged: true },
        LifecycleCase { status: TimeOffStatus::Pending, action: "approve", expected_code: 409, expected_status: "PENDING", unchanged: true },
        LifecycleCase { status: TimeOffStatus::Pending, action: "reject", expected_code: 409, expected_status: "PENDING", unchanged: true },
        LifecycleCase { status: TimeOffStatus::Pending, action: "cancel", expected_code: 409, expected_status: "PENDING", unchanged: true },
    ]
}

#[tokio::test]
async fn every_lifecycle_call_moves_the_stored_status_or_is_refused_for_it() {
    let (start, end) = window(7, 8);

    for case in lifecycle_cases() {
        let label = format!("{:?} + {}", case.status, case.action);
        // A fresh emulator per row: the row is about ONE request's status.
        let emulator = Emulator::new();
        let time_off_id = emulator
            .create_ok(
                INSTANCE,
                USER,
                &create_body(ACTIVITY, start, end, "AWAIT_APPROVAL"),
            )
            .await;
        // Reach the row's starting status through the service's own calls where one leads
        // there, and through the store for `PENDING`, which no call produces.
        match case.status {
            TimeOffStatus::AwaitingApproval => {}
            TimeOffStatus::Approved => {
                emulator.act("approve", INSTANCE, USER, &time_off_id, &json!({})).await;
            }
            TimeOffStatus::Rejected => {
                emulator.act("reject", INSTANCE, USER, &time_off_id, &json!({})).await;
            }
            TimeOffStatus::Cancelled => {
                emulator.act("cancel", INSTANCE, USER, &time_off_id, &json!({})).await;
            }
            TimeOffStatus::Pending => emulator.force_status(INSTANCE, &time_off_id, TimeOffStatus::Pending),
        }
        let before = emulator.get_ok(INSTANCE, USER, &time_off_id).await;

        let (status, body) = emulator
            .act(case.action, INSTANCE, USER, &time_off_id, &json!({}))
            .await;
        assert_eq!(
            status, case.expected_code,
            "case {label:?}: expected HTTP {}, got {status} with {body}",
            case.expected_code
        );
        if case.expected_code == 409 {
            assert_eq!(
                error_code(&body),
                "ConflictException",
                "case {label:?}: the refusal is the modelled conflict, got {body}"
            );
        }

        let after = emulator.get_ok(INSTANCE, USER, &time_off_id).await;
        assert_eq!(
            after.get("Status").and_then(Value::as_str),
            Some(case.expected_status),
            "case {label:?}: expected the stored status {}, got {after:?}",
            case.expected_status
        );
        if case.unchanged {
            assert_eq!(
                after, before,
                "case {label:?}: the call succeeds or is refused without changing the request"
            );
        } else {
            let expected_actor = json!({
                "AWSIdentityArn": service_common::caller_principal::ambient_user_arn(),
            });
            assert_eq!(
                after.get("UpdatedBy"),
                Some(&expected_actor),
                "case {label:?}: the call that changed the request names its identity in \
                 UpdatedBy, got {after:?}"
            );
            assert!(
                millis_of(&label, &after, "UpdatedAt")
                    >= millis_of(&label, &after, "CreatedAt"),
                "case {label:?}: UpdatedAt moves to the instant of the change, got {after:?}"
            );
            assert_eq!(
                after.get("CreatedBy"),
                before.get("CreatedBy"),
                "case {label:?}: CreatedBy is immutable for the life of the request"
            );
        }
    }
}

#[tokio::test]
async fn a_cancellation_replaces_the_requests_description_and_removes_it_when_omitted() {
    let (start, end) = window(7, 8);

    for (case, cancellation, expected) in [
        (
            "a description recorded with the cancellation",
            json!({"Description": "trip called off"}),
            Some("trip called off"),
        ),
        ("no description, which removes it", json!({}), None),
    ] {
        let emulator = Emulator::new();
        let mut request = create_body(ACTIVITY, start, end, "AWAIT_APPROVAL");
        request["Description"] = json!("spring break");
        let time_off_id = emulator.create_ok(INSTANCE, USER, &request).await;

        let (status, body) = emulator
            .act("cancel", INSTANCE, USER, &time_off_id, &cancellation)
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the cancellation succeeds, got {status} with {body}"
        );
        let read = emulator.get_ok(INSTANCE, USER, &time_off_id).await;
        assert_eq!(
            read.get("Description").and_then(Value::as_str),
            expected,
            "case {case:?}: the cancellation's description replaces the request's, got {read:?}"
        );
    }

    // An approval's or a rejection's description is not returned by the read.
    for action in ["approve", "reject"] {
        let emulator = Emulator::new();
        let mut request = create_body(ACTIVITY, start, end, "AWAIT_APPROVAL");
        request["Description"] = json!("spring break");
        let time_off_id = emulator.create_ok(INSTANCE, USER, &request).await;
        emulator
            .act(
                action,
                INSTANCE,
                USER,
                &time_off_id,
                &json!({"Description": "a note on the decision"}),
            )
            .await;
        let read = emulator.get_ok(INSTANCE, USER, &time_off_id).await;
        assert_eq!(
            read.get("Description").and_then(Value::as_str),
            Some("spring break"),
            "the description of a {action} call is not returned by the read, got {read:?}"
        );
    }
}

/// One row of the overlap table: where the second window sits relative to an approved one,
/// and whether that is an overlap.
struct OverlapCase {
    label: &'static str,
    /// Hours from the approved window's start, and the length in hours.
    offset_hours: i64,
    length_hours: i64,
    overlaps: bool,
}

/// The second window's position, over every relation two windows can be in. `StartAt` is
/// inclusive and `EndAt` exclusive, so two windows that merely touch do not overlap.
fn overlap_cases() -> Vec<OverlapCase> {
    vec![
        OverlapCase { label: "the same window", offset_hours: 0, length_hours: 8, overlaps: true },
        OverlapCase { label: "starting inside it", offset_hours: 4, length_hours: 8, overlaps: true },
        OverlapCase { label: "ending inside it", offset_hours: -4, length_hours: 8, overlaps: true },
        OverlapCase { label: "contained in it", offset_hours: 2, length_hours: 2, overlaps: true },
        OverlapCase { label: "containing it", offset_hours: -2, length_hours: 12, overlaps: true },
        OverlapCase { label: "ending where it starts", offset_hours: -8, length_hours: 8, overlaps: false },
        OverlapCase { label: "starting where it ends", offset_hours: 8, length_hours: 8, overlaps: false },
        OverlapCase { label: "a day earlier", offset_hours: -24, length_hours: 8, overlaps: false },
        OverlapCase { label: "a day later", offset_hours: 24, length_hours: 8, overlaps: false },
    ]
}

#[tokio::test]
async fn an_overlap_with_approved_time_off_rejects_the_request_with_the_conflicting_code() {
    let (approved_start, approved_end) = window(14, 8);

    for case in overlap_cases() {
        for rule_failure_action in [None, Some("OVERRIDE")] {
            let label = format!("{} under {rule_failure_action:?}", case.label);
            let emulator = Emulator::new();

            // The window this instance already holds, approved.
            let held = emulator
                .create_ok(
                    INSTANCE,
                    USER,
                    &create_body(ACTIVITY, approved_start, approved_end, "AWAIT_APPROVAL"),
                )
                .await;
            emulator.act("approve", INSTANCE, USER, &held, &json!({})).await;
            assert_eq!(
                emulator.status_of(INSTANCE, USER, &held).await,
                "APPROVED",
                "case {label:?}: the held window must be approved for the row to mean anything"
            );

            let start = approved_start + (case.offset_hours * 3_600) as f64;
            let mut request = create_body(
                ACTIVITY,
                start,
                start + (case.length_hours * 3_600) as f64,
                "AUTOMATIC",
            );
            if let Some(action) = rule_failure_action {
                request["RuleFailureAction"] = json!(action);
            }
            let candidate = emulator.create_ok(INSTANCE, USER, &request).await;
            let read = emulator.get_ok(INSTANCE, USER, &candidate).await;

            if case.overlaps {
                assert_eq!(
                    read.get("Status").and_then(Value::as_str),
                    Some("REJECTED"),
                    "case {label:?}: an overlap with approved time off is rejected, got {read:?}"
                );
                assert_eq!(
                    read.get("ValidationResults"),
                    Some(&json!([
                        {"Severity": "ERROR", "Code": "CONFLICTING_TIME_OFF_REQUEST"}
                    ])),
                    "case {label:?}: the rejection records the conflict as an ERROR, and \
                     OVERRIDE never waives it, got {read:?}"
                );
            } else {
                assert_eq!(
                    read.get("Status").and_then(Value::as_str),
                    Some("APPROVED"),
                    "case {label:?}: a window that does not overlap is approved, got {read:?}"
                );
                assert_eq!(
                    read.get("ValidationResults"),
                    None,
                    "case {label:?}: no rule failed, so there is nothing to report, got {read:?}"
                );
            }

            // Another user's approved window is not this user's conflict.
            let other_user = emulator
                .create_ok(
                    INSTANCE,
                    OTHER_USER,
                    &create_body(ACTIVITY, approved_start, approved_end, "AUTOMATIC"),
                )
                .await;
            assert_eq!(
                emulator.status_of(INSTANCE, OTHER_USER, &other_user).await,
                "APPROVED",
                "case {label:?}: the overlap check is per user"
            );
        }
    }
}

#[tokio::test]
async fn a_cancelled_window_is_released_for_the_next_request() {
    let emulator = Emulator::new();
    let (start, end) = window(21, 8);

    let held = emulator
        .create_ok(INSTANCE, USER, &create_body(ACTIVITY, start, end, "AUTOMATIC"))
        .await;
    assert_eq!(emulator.status_of(INSTANCE, USER, &held).await, "APPROVED");

    // While it is approved, the same window conflicts.
    let blocked = emulator
        .create_ok(INSTANCE, USER, &create_body(ACTIVITY, start, end, "AUTOMATIC"))
        .await;
    assert_eq!(
        emulator.status_of(INSTANCE, USER, &blocked).await,
        "REJECTED",
        "an approved request holds its window"
    );

    emulator.act("cancel", INSTANCE, USER, &held, &json!({})).await;
    let after_cancel = emulator
        .create_ok(INSTANCE, USER, &create_body(ACTIVITY, start, end, "AUTOMATIC"))
        .await;
    assert_eq!(
        emulator.status_of(INSTANCE, USER, &after_cancel).await,
        "APPROVED",
        "a cancelled request releases the window it reserved"
    );

    // So does one the caller rejected, and one that is still waiting.
    for (case, action) in [("rejected", "reject"), ("waiting", "")] {
        let emulator = Emulator::new();
        let first = emulator
            .create_ok(
                INSTANCE,
                USER,
                &create_body(ACTIVITY, start, end, "AWAIT_APPROVAL"),
            )
            .await;
        if !action.is_empty() {
            emulator.act(action, INSTANCE, USER, &first, &json!({})).await;
        }
        let second = emulator
            .create_ok(INSTANCE, USER, &create_body(ACTIVITY, start, end, "AUTOMATIC"))
            .await;
        assert_eq!(
            emulator.status_of(INSTANCE, USER, &second).await,
            "APPROVED",
            "case {case:?}: only an approved request reserves its window"
        );
    }
}

/// One row of the balance table: the stored balance, the window asked for, what the caller
/// asked to happen on a failed rule, and the decision that follows.
struct BalanceCase {
    label: &'static str,
    /// The balance to store, or `None` to store none at all.
    balance_minutes: Option<i32>,
    /// Days from now the window starts — negative for a window wholly in the past.
    days_from_now: i64,
    hours: i64,
    rule_failure_action: Option<&'static str>,
    expected_status: &'static str,
    expected_results: Value,
}

/// The balance cases: a request that fits, one that does not under each `RuleFailureAction`,
/// the exact bound and one minute past it, a user with no balance stored at all, and a window
/// wholly in the past, whose failed rules are recorded as a `WARNING` whatever the action.
fn balance_cases() -> Vec<BalanceCase> {
    let insufficient = json!([
        {"Severity": "ERROR", "Code": "INSUFFICIENT_USER_TIME_OFF_BALANCE"}
    ]);
    let waived = json!([
        {"Severity": "WARNING", "Code": "INSUFFICIENT_USER_TIME_OFF_BALANCE"}
    ]);
    vec![
        BalanceCase { label: "well inside the balance", balance_minutes: Some(2_400), days_from_now: 7, hours: 8, rule_failure_action: None, expected_status: "APPROVED", expected_results: Value::Null },
        BalanceCase { label: "exactly the balance", balance_minutes: Some(480), days_from_now: 7, hours: 8, rule_failure_action: None, expected_status: "APPROVED", expected_results: Value::Null },
        BalanceCase { label: "one minute past the balance", balance_minutes: Some(479), days_from_now: 7, hours: 8, rule_failure_action: None, expected_status: "REJECTED", expected_results: insufficient.clone() },
        BalanceCase { label: "past the balance, rejecting explicitly", balance_minutes: Some(60), days_from_now: 7, hours: 8, rule_failure_action: Some("REJECT"), expected_status: "REJECTED", expected_results: insufficient.clone() },
        BalanceCase { label: "past the balance, overriding the rules", balance_minutes: Some(60), days_from_now: 7, hours: 8, rule_failure_action: Some("OVERRIDE"), expected_status: "APPROVED", expected_results: waived.clone() },
        BalanceCase { label: "a zero balance, which nothing fits in", balance_minutes: Some(0), days_from_now: 7, hours: 8, rule_failure_action: None, expected_status: "REJECTED", expected_results: insufficient },
        BalanceCase { label: "no balance stored at all, so no balance rule to fail", balance_minutes: None, days_from_now: 7, hours: 8, rule_failure_action: None, expected_status: "APPROVED", expected_results: Value::Null },
        BalanceCase { label: "a window wholly in the past", balance_minutes: Some(60), days_from_now: -30, hours: 8, rule_failure_action: Some("REJECT"), expected_status: "APPROVED", expected_results: waived },
    ]
}

#[tokio::test]
async fn an_insufficient_balance_rejects_or_warns_as_the_rule_failure_action_asks() {
    for case in balance_cases() {
        let emulator = Emulator::new();
        if let Some(minutes) = case.balance_minutes {
            emulator
                .put_balance(INSTANCE, USER, ACTIVITY, minutes, "MINUTES")
                .await;
        }

        let (start, end) = window(case.days_from_now, case.hours);
        let mut request = create_body(ACTIVITY, start, end, "AUTOMATIC");
        if let Some(action) = case.rule_failure_action {
            request["RuleFailureAction"] = json!(action);
        }
        let time_off_id = emulator.create_ok(INSTANCE, USER, &request).await;
        let read = emulator.get_ok(INSTANCE, USER, &time_off_id).await;

        assert_eq!(
            read.get("Status").and_then(Value::as_str),
            Some(case.expected_status),
            "case {:?}: expected the status {}, got {read:?}",
            case.label,
            case.expected_status
        );
        let reported = read.get("ValidationResults").cloned().unwrap_or(Value::Null);
        assert_eq!(
            reported, case.expected_results,
            "case {:?}: expected the validation results {}, got {reported}",
            case.label, case.expected_results
        );
    }
}

#[tokio::test]
async fn an_approval_evaluates_the_same_balance_rule_the_automatic_decision_does() {
    let (start, end) = window(7, 8);

    for (case, rule_failure_action, expected_status, expected_results) in [
        (
            "the approval rejects on the failed rule",
            json!({"RuleFailureAction": "REJECT"}),
            "REJECTED",
            json!([{"Severity": "ERROR", "Code": "INSUFFICIENT_USER_TIME_OFF_BALANCE"}]),
        ),
        (
            "the approval overrides the failed rule",
            json!({"RuleFailureAction": "OVERRIDE"}),
            "APPROVED",
            json!([{"Severity": "WARNING", "Code": "INSUFFICIENT_USER_TIME_OFF_BALANCE"}]),
        ),
        (
            "the approval says nothing, so the rule rejects",
            json!({}),
            "REJECTED",
            json!([{"Severity": "ERROR", "Code": "INSUFFICIENT_USER_TIME_OFF_BALANCE"}]),
        ),
    ] {
        let emulator = Emulator::new();
        emulator
            .put_balance(INSTANCE, USER, ACTIVITY, 60, "MINUTES")
            .await;
        let time_off_id = emulator
            .create_ok(
                INSTANCE,
                USER,
                &create_body(ACTIVITY, start, end, "AWAIT_APPROVAL"),
            )
            .await;
        assert_eq!(
            emulator.status_of(INSTANCE, USER, &time_off_id).await,
            "AWAITING_APPROVAL",
            "case {case:?}: a waiting request is not decided at creation"
        );

        let (status, body) = emulator
            .act("approve", INSTANCE, USER, &time_off_id, &rule_failure_action)
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the approval is accepted, got {status} with {body}"
        );
        let read = emulator.get_ok(INSTANCE, USER, &time_off_id).await;
        assert_eq!(
            read.get("Status").and_then(Value::as_str),
            Some(expected_status),
            "case {case:?}: expected the status {expected_status}, got {read:?}"
        );
        assert_eq!(
            read.get("ValidationResults"),
            Some(&expected_results),
            "case {case:?}: expected the validation results {expected_results}, got {read:?}"
        );
    }
}

#[tokio::test]
async fn the_balance_is_drawn_down_by_the_users_approved_time_off_for_that_activity() {
    let emulator = Emulator::new();
    // 12 hours of balance, asked for in two 8-hour windows that do not overlap.
    emulator
        .put_balance(INSTANCE, USER, ACTIVITY, 12, "HOURS")
        .await;
    let (first_start, first_end) = window(7, 8);
    let (second_start, second_end) = window(14, 8);

    let first = emulator
        .create_ok(
            INSTANCE,
            USER,
            &create_body(ACTIVITY, first_start, first_end, "AUTOMATIC"),
        )
        .await;
    assert_eq!(
        emulator.status_of(INSTANCE, USER, &first).await,
        "APPROVED",
        "the first window fits in the balance"
    );

    let second = emulator
        .create_ok(
            INSTANCE,
            USER,
            &create_body(ACTIVITY, second_start, second_end, "AUTOMATIC"),
        )
        .await;
    assert_eq!(
        emulator.status_of(INSTANCE, USER, &second).await,
        "REJECTED",
        "the approved window already draws on the balance, so the second does not fit"
    );

    // Another activity has its own balance, so this one's is untouched by it. Its window is
    // its own too, so the overlap check \u2014 which is per user, whatever the activity \u2014 is not
    // what decides this row.
    let (other_start, other_end) = window(21, 8);
    let other_activity = emulator
        .create_ok(
            INSTANCE,
            USER,
            &create_body("sick-leave", other_start, other_end, "AUTOMATIC"),
        )
        .await;
    assert_eq!(
        emulator.status_of(INSTANCE, USER, &other_activity).await,
        "APPROVED",
        "a balance is per (user, activity) pair, and this activity has none to fail"
    );

    // Cancelling the first request gives the balance back.
    emulator.act("cancel", INSTANCE, USER, &first, &json!({})).await;
    let third = emulator
        .create_ok(
            INSTANCE,
            USER,
            &create_body(ACTIVITY, second_start, second_end, "AUTOMATIC"),
        )
        .await;
    assert_eq!(
        emulator.status_of(INSTANCE, USER, &third).await,
        "APPROVED",
        "a cancelled request draws on no balance"
    );
}

#[tokio::test]
async fn a_request_no_create_minted_for_this_user_is_the_modelled_404() {
    let emulator = Emulator::new();
    let (start, end) = window(7, 8);
    let mine = emulator
        .create_ok(
            INSTANCE,
            USER,
            &create_body(ACTIVITY, start, end, "AWAIT_APPROVAL"),
        )
        .await;

    for (case, instance, user, time_off_id) in [
        ("an id no create minted", INSTANCE, USER, UNKNOWN_TIME_OFF_ID.to_string()),
        ("an instance holding nothing", OTHER_INSTANCE, USER, mine.clone()),
        ("another user's request", INSTANCE, OTHER_USER, mine.clone()),
    ] {
        for action in ["get", "approve", "reject", "cancel"] {
            let (status, body) = match action {
                "get" => emulator.get(instance, user, &time_off_id).await,
                _ => {
                    emulator
                        .act(action, instance, user, &time_off_id, &json!({}))
                        .await
                }
            };
            assert_eq!(
                (status, error_code(&body).as_str()),
                (404, "ResourceNotFoundException"),
                "case {case:?} on {action}: expected the modelled 404, got {status} with {body}"
            );
            assert_eq!(
                body.get("ResourceId").and_then(Value::as_str),
                Some(time_off_id.as_str()),
                "case {case:?} on {action}: the refusal names the request it did not find, got \
                 {body}"
            );
        }
    }

    // The request the rows missed is still there, untouched.
    assert_eq!(
        emulator.status_of(INSTANCE, USER, &mine).await,
        "AWAITING_APPROVAL",
        "a refused call changes nothing"
    );
}

#[tokio::test]
async fn a_create_the_model_rules_out_is_refused_and_stores_nothing() {
    let (start, end) = window(7, 8);

    for (case, request) in [
        (
            "OVERRIDE on a request that waits for approval",
            {
                let mut body = create_body(ACTIVITY, start, end, "AWAIT_APPROVAL");
                body["RuleFailureAction"] = json!("OVERRIDE");
                body
            },
        ),
        (
            "a window that ends where it starts",
            create_body(ACTIVITY, start, start, "AUTOMATIC"),
        ),
        (
            "a window that ends before it starts",
            create_body(ACTIVITY, end, start, "AUTOMATIC"),
        ),
    ] {
        let emulator = Emulator::new();
        let (status, body) = emulator.create(INSTANCE, USER, &request).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected the modelled 400, got {status} with {body}"
        );
        assert_eq!(
            emulator.stored_count(INSTANCE),
            0,
            "case {case:?}: a refused create stores nothing"
        );
        assert_eq!(
            emulator.stored_instances(),
            0,
            "case {case:?}: a refused create files no state under the instance it named"
        );
    }

    // REJECT, unlike OVERRIDE, is accepted beside AWAIT_APPROVAL.
    let emulator = Emulator::new();
    let mut accepted = create_body(ACTIVITY, start, end, "AWAIT_APPROVAL");
    accepted["RuleFailureAction"] = json!("REJECT");
    let time_off_id = emulator.create_ok(INSTANCE, USER, &accepted).await;
    assert_eq!(
        emulator.status_of(INSTANCE, USER, &time_off_id).await,
        "AWAITING_APPROVAL"
    );
}

#[tokio::test]
async fn reading_a_request_changes_nothing_a_later_caller_sees() {
    let emulator = Emulator::new();
    let (start, end) = window(7, 8);
    emulator
        .put_balance(INSTANCE, USER, ACTIVITY, 480, "MINUTES")
        .await;
    let time_off_id = emulator
        .create_ok(
            INSTANCE,
            USER,
            &create_body(ACTIVITY, start, end, "AWAIT_APPROVAL"),
        )
        .await;

    let first = emulator.get_ok(INSTANCE, USER, &time_off_id).await;
    for round in 0..5 {
        let again = emulator.get_ok(INSTANCE, USER, &time_off_id).await;
        assert_eq!(
            again, first,
            "round {round}: GetTimeOff is @readonly, so repeating it answers the same record"
        );
    }
    // Nor does reading consume the balance the request would be decided against.
    assert_eq!(
        emulator.stored_balance(INSTANCE, USER, ACTIVITY),
        Some((480, "MINUTES".to_string())),
        "a read consumes no balance"
    );
    let (status, _) = emulator
        .act("approve", INSTANCE, USER, &time_off_id, &json!({}))
        .await;
    assert_eq!(status, 200);
    assert_eq!(
        emulator.status_of(INSTANCE, USER, &time_off_id).await,
        "APPROVED",
        "the request the reads left alone still approves against its balance"
    );
}

#[tokio::test]
async fn an_instance_holds_no_more_requests_than_its_store_bound() {
    let emulator = Emulator::new();
    let (start, _) = window(7, 8);

    for index in 0..MAX_TIME_OFF_REQUESTS_PER_INSTANCE {
        // Each window its own hour, so no row's decision depends on another's.
        let own_start = start + (index as f64) * 3_600.0;
        let request = create_body(ACTIVITY, own_start, own_start + 1_800.0, "AWAIT_APPROVAL");
        let (status, body) = emulator.create(INSTANCE, USER, &request).await;
        assert_eq!(
            status, 200,
            "create {index}: the store holds up to its bound, got {status} with {body}"
        );
    }
    assert_eq!(
        emulator.stored_count(INSTANCE),
        MAX_TIME_OFF_REQUESTS_PER_INSTANCE
    );

    let beyond = create_body(ACTIVITY, start - 86_400.0, start - 82_800.0, "AWAIT_APPROVAL");
    let (status, body) = emulator.create(INSTANCE, USER, &beyond).await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (402, "ServiceQuotaExceededException"),
        "past its bound a create is refused rather than stored, got {status} with {body}"
    );
    assert_eq!(
        emulator.stored_count(INSTANCE),
        MAX_TIME_OFF_REQUESTS_PER_INSTANCE,
        "the refused create stored nothing"
    );

    // The bound is per instance: another instance is empty.
    let elsewhere = emulator.create_ok(OTHER_INSTANCE, USER, &beyond).await;
    assert_eq!(
        emulator.status_of(OTHER_INSTANCE, USER, &elsewhere).await,
        "AWAITING_APPROVAL"
    );
}

/// An instance id of the modelled form (`InstanceIdOrArn`'s UUID pattern), one per index, so
/// a row can address as many instances as the store's bound.
fn synthetic_instance(index: usize) -> String {
    format!("{index:08x}-aaaa-bbbb-cccc-{index:012x}")
}

#[tokio::test]
async fn the_store_files_state_under_no_more_instances_than_its_bound() {
    let emulator = Emulator::new();
    let (start, end) = window(7, 8);
    let request = create_body(ACTIVITY, start, end, "AWAIT_APPROVAL");

    let mut held = Vec::new();
    for index in 0..MAX_INSTANCES_HELD {
        let instance = synthetic_instance(index);
        let time_off_id = emulator.create_ok(&instance, USER, &request).await;
        held.push((instance, time_off_id));
    }
    assert_eq!(emulator.stored_instances(), MAX_INSTANCES_HELD);

    // Up to the bound, every instance still answers for what it holds.
    let (first_instance, first_id) = held[0].clone();
    assert_eq!(
        emulator.status_of(&first_instance, USER, &first_id).await,
        "AWAITING_APPROVAL",
        "an instance inside the bound keeps its requests"
    );

    // One instance past it is taken in, and the store drops the one written longest ago
    // rather than growing.
    let overflow = synthetic_instance(MAX_INSTANCES_HELD);
    let overflow_id = emulator.create_ok(&overflow, USER, &request).await;
    assert_eq!(
        emulator.status_of(&overflow, USER, &overflow_id).await,
        "AWAITING_APPROVAL",
        "the instance past the bound holds the request it was addressed with"
    );
    assert_eq!(
        emulator.stored_instances(),
        MAX_INSTANCES_HELD,
        "the store files state under no more instances than its bound"
    );
    let (status, body) = emulator.get(&first_instance, USER, &first_id).await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (404, "ResourceNotFoundException"),
        "the instance written longest ago is the one dropped, got {status} with {body}"
    );
    // A write to an instance the store already holds takes no new slot.
    for _ in 0..8 {
        emulator.create_ok(&overflow, USER, &request).await;
    }
    assert_eq!(emulator.stored_instances(), MAX_INSTANCES_HELD);
}

#[tokio::test]
async fn an_instance_holds_no_more_balances_than_its_store_bound() {
    let emulator = Emulator::new();

    for index in 0..MAX_TIME_OFF_BALANCES_PER_INSTANCE {
        emulator
            .put_balance(INSTANCE, &format!("user-{index}"), ACTIVITY, 60, "MINUTES")
            .await;
    }
    assert_eq!(
        emulator.stored_balances(INSTANCE),
        MAX_TIME_OFF_BALANCES_PER_INSTANCE
    );
    assert_eq!(
        emulator.stored_balance(INSTANCE, "user-0", ACTIVITY),
        Some((60, "MINUTES".to_string())),
        "a balance inside the bound is the one that was written"
    );

    // Replacing the balance of a pair the instance already holds takes no new slot.
    for value in [30, 45, 90] {
        emulator
            .put_balance(INSTANCE, "user-0", ACTIVITY, value, "MINUTES")
            .await;
    }
    assert_eq!(
        emulator.stored_balances(INSTANCE),
        MAX_TIME_OFF_BALANCES_PER_INSTANCE,
        "a replacement is not a new balance"
    );
    assert_eq!(
        emulator.stored_balance(INSTANCE, "user-0", ACTIVITY),
        Some((90, "MINUTES".to_string()))
    );

    // One pair past the bound is taken in, and the balance written longest ago is dropped,
    // which leaves that pair with no balance rather than a stale one.
    emulator
        .put_balance(INSTANCE, "user-beyond", ACTIVITY, 15, "MINUTES")
        .await;
    assert_eq!(
        emulator.stored_balances(INSTANCE),
        MAX_TIME_OFF_BALANCES_PER_INSTANCE,
        "an instance holds no more balances than its bound"
    );
    assert_eq!(
        emulator.stored_balance(INSTANCE, "user-beyond", ACTIVITY),
        Some((15, "MINUTES".to_string()))
    );
    assert_eq!(
        emulator.stored_balance(INSTANCE, "user-0", ACTIVITY),
        None,
        "the balance written longest ago is the one dropped"
    );
}

/// The dedicated target this service's state-changing operations record a call on.
const AUDIT_TARGET: &str = "emulator::audit";

/// A `tracing` subscriber that renders each event as one `target field=value …` line and keeps
/// the lines so a test can read them back.
///
/// Written against `tracing`'s own `Subscriber` trait rather than pulling in a formatting
/// subscriber crate, so capturing records adds no dependency.
#[derive(Clone, Default)]
struct CapturedRecords(Arc<std::sync::Mutex<Vec<String>>>);

impl CapturedRecords {
    /// The audit records written by calls signing with `akid`, TAKEN from the buffer and
    /// leaving every other test's records in it.
    ///
    /// Suites run in parallel over one process-wide subscriber, so a test that wants exactly
    /// one record per call signs with an access key id no other test uses.
    fn drain_for(&self, akid: &str) -> Vec<String> {
        let mut held = self.0.lock().expect("the audit capture lock");
        let (mine, theirs) = std::mem::take(&mut *held).into_iter().partition(|record| {
            record.starts_with(&format!("{AUDIT_TARGET} ")) && record.contains(&format!("akid={akid}"))
        });
        *held = theirs;
        mine
    }
}

/// Renders an event's fields as ` name=value` onto one line.
struct RecordLine(String);

impl tracing::field::Visit for RecordLine {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        use std::fmt::Write as _;
        let _ = write!(self.0, " {}={value:?}", field.name());
    }

    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        use std::fmt::Write as _;
        let _ = write!(self.0, " {}={value}", field.name());
    }
}

impl tracing::Subscriber for CapturedRecords {
    fn enabled(&self, metadata: &tracing::Metadata<'_>) -> bool {
        *metadata.level() <= tracing::Level::INFO
    }

    fn new_span(&self, _span: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        // The service emits events, never spans; an id is still required by the trait.
        tracing::span::Id::from_u64(1)
    }

    fn record(&self, _span: &tracing::span::Id, _values: &tracing::span::Record<'_>) {}

    fn record_follows_from(&self, _span: &tracing::span::Id, _follows: &tracing::span::Id) {}

    fn event(&self, event: &tracing::Event<'_>) {
        let mut line = RecordLine(format!("{} ", event.metadata().target()));
        event.record(&mut line);
        self.0.lock().expect("the audit capture lock").push(line.0);
    }

    fn enter(&self, _span: &tracing::span::Id) {}

    fn exit(&self, _span: &tracing::span::Id) {}
}

/// This test binary's capture, installed once as the global `tracing` subscriber: `tracing`
/// caches per callsite whether any subscriber is interested, and that cache is process-wide.
static AUDIT_CAPTURE: std::sync::OnceLock<CapturedRecords> = std::sync::OnceLock::new();

fn audit_capture() -> &'static CapturedRecords {
    AUDIT_CAPTURE.get_or_init(|| {
        let captured = CapturedRecords::default();
        tracing::subscriber::set_global_default(captured.clone())
            .expect("this test owns the global tracing subscriber");
        captured
    })
}

/// Assert the call just made left exactly one audit record, naming the operation, the
/// resource it addressed, the identity it presented and the outcome.
fn assert_one_record(
    case: &str,
    records: &[String],
    operation: &str,
    resource: &str,
    outcome: &str,
    akid: &str,
) {
    assert_eq!(
        records.len(),
        1,
        "case {case:?}: a state-changing call leaves exactly one audit record, got {records:?}"
    );
    let record = &records[0];
    for field in [
        "service=connect".to_string(),
        format!("operation={operation}"),
        format!("akid={akid}"),
        format!("resource={resource}"),
        format!("outcome={outcome}"),
    ] {
        assert!(
            record.contains(&field),
            "case {case:?}: the audit record must carry {field:?}, got {record:?}"
        );
    }
}

#[tokio::test]
async fn every_state_changing_call_leaves_one_structured_audit_record() {
    /// An access key id no other test in this binary signs with.
    const AKID: &str = "AKIDAUDITEDTIMEOFF";

    let captured = audit_capture();
    let emulator = Emulator::signing_as(AKID);
    let (start, end) = window(7, 8);
    // Whatever an earlier run of this test left, so the counts below are this run's.
    captured.drain_for(AKID);

    emulator
        .put_balance(INSTANCE, USER, ACTIVITY, 480, "MINUTES")
        .await;
    assert_one_record(
        "a balance written",
        &captured.drain_for(AKID),
        "PutTimeOffBalance",
        &format!("{USER}/{ACTIVITY}"),
        "balance-stored",
        AKID,
    );

    let mut request = create_body(ACTIVITY, start, end, "AWAIT_APPROVAL");
    request["ClientToken"] = json!("tok-audited");
    let time_off_id = emulator.create_ok(INSTANCE, USER, &request).await;
    assert_one_record(
        "a request created",
        &captured.drain_for(AKID),
        "CreateTimeOff",
        &time_off_id,
        "time-off-created",
        AKID,
    );

    emulator.create_ok(INSTANCE, USER, &request).await;
    assert_one_record(
        "a create replayed on its client token",
        &captured.drain_for(AKID),
        "CreateTimeOff",
        &time_off_id,
        "time-off-replayed",
        AKID,
    );

    // A read is not a state change, so it records nothing.
    emulator.get_ok(INSTANCE, USER, &time_off_id).await;
    assert!(
        captured.drain_for(AKID).is_empty(),
        "GetTimeOff changes nothing, so it leaves no audit record"
    );

    for (case, action, outcome) in [
        ("an approval", "approve", "time-off-approved"),
        ("an approval of an approved request", "approve", "unchanged"),
        ("a rejection", "reject", "time-off-rejected"),
        ("a rejection of a rejected request", "reject", "unchanged"),
        ("a cancellation", "cancel", "time-off-cancelled"),
        ("a cancellation of a cancelled request", "cancel", "unchanged"),
        ("an approval of a cancelled request", "approve", "refused"),
    ] {
        emulator
            .act(action, INSTANCE, USER, &time_off_id, &json!({}))
            .await;
        let operation = match action {
            "approve" => "ApproveTimeOff",
            "reject" => "RejectTimeOff",
            _ => "CancelTimeOff",
        };
        assert_one_record(
            case,
            &captured.drain_for(AKID),
            operation,
            &time_off_id,
            outcome,
            AKID,
        );
    }

    // A call about a request no create minted records the refusal too.
    emulator
        .act("cancel", INSTANCE, USER, UNKNOWN_TIME_OFF_ID, &json!({}))
        .await;
    assert_one_record(
        "a cancellation of a request no create minted",
        &captured.drain_for(AKID),
        "CancelTimeOff",
        UNKNOWN_TIME_OFF_ID,
        "refused",
        AKID,
    );
}
