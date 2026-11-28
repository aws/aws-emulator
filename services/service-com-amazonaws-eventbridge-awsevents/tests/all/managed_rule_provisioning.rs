// SPDX-License-Identifier: CC0-1.0

//! WHO wrote a rule, and what that makes of it: the managed-rule seam, over the wire
//! (`awsJson1_1`, `POST /` with `X-Amz-Target: AWSEvents.<Operation>`) through the real
//! `MultiServiceRouter`.
//!
//! RULE: "if this is a managed rule, created by an Amazon Web Services service on your behalf,
//! this field displays the principal name of the service that created the rule"
//! (`Rule$ManagedBy`) — so `ManagedBy` is no member of any request: it is read off WHO made the
//! call. A rule an emulated service wrote as its own service principal is that service's
//! managed rule and reports that principal; a rule a client wrote is the client's own and
//! reports none. And a managed rule admits exactly ONE writer, the service that owns it —
//! "you cannot modify these managed rules by using DisableRule, EnableRule, PutTargets,
//! PutRule, TagResource, or UntagResource" (`ManagedRuleException`) — so a client's write of it,
//! and another service's, are refused with that error and leave the rule exactly as it was.
//!
//! WHICH credential the call presents is what says so: the shared directory of the
//! cross-service calls a service is making as its own principal
//! (`service_common::service_principal_calls`, keyed by the access key id of the credential
//! the call is signed with), which the calling service writes and this service reads. The rows
//! below therefore differ along exactly that: whether the request's access key id has a record
//! in that directory, and which principal that record names.
//!
//! This suite seeds the directory DIRECTLY, which is the only half of the hand-over that is
//! this crate's: the minting of the credential and the signing of the call belong to the
//! calling service (`core/router/tests/all/guardduty_malware_protection_plan_wiring.rs` drives
//! the whole chain through a GuardDuty Malware Protection plan).

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_eventbridge_awsevents::state::DEFAULT_EVENT_BUS_NAME;
use service_com_amazonaws_eventbridge_awsevents::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below is signed for.
const REGION: &str = "eu-west-2";

/// The access key id a CLIENT's call presents: one the directory holds no record for.
const CUSTOMER_KEY: &str = "AKIDCUSTOMERTEST";

/// The access key ids the two service calls present, and the principals their records name.
const OWNER_KEY: &str = "AKIDOWNERSERVICE";
const OWNER_PRINCIPAL: &str = "malware-protection-plan.guardduty.amazonaws.com";
const OTHER_KEY: &str = "AKIDOTHERSERVICE";
const OTHER_PRINCIPAL: &str = "backup.amazonaws.com";

/// How long the seeded records stand: far past the few in-process calls each row makes, so no
/// row depends on the clock.
const RECORD_HOLDS_FOR: Duration = Duration::from_secs(300);

/// The emulator: one router carrying only this service over one shared engine, plus that
/// engine, so a test can seed the shared directory both sides read.
struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

fn emulator() -> Emulator {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    Emulator { router, engine }
}

/// Record that the call signed with `access_key_id` is `principal`'s own, made for the resource
/// `source_arn` — what the calling service writes before it makes such a call.
fn remember_service_call(
    emulator: &Emulator,
    access_key_id: &str,
    principal: &str,
    source_arn: &str,
) {
    let now = SystemTime::now();
    let directory = emulator
        .engine
        .state::<service_common::service_principal_calls::ServicePrincipalCalls>();
    let remembered = directory.lock().remember(
        access_key_id.to_string(),
        service_common::service_principal_calls::ServicePrincipalCall {
            principal: principal.to_string(),
            source_arn: source_arn.to_string(),
            source_account: engine::emulator_account_id().to_string(),
            expires_at: now + RECORD_HOLDS_FOR,
        },
        now,
    );
    assert!(
        remembered,
        "setup: the directory must accept the record for {access_key_id}"
    );
}

/// One call, signed with `access_key_id`: its HTTP status and parsed body.
async fn call_as(
    emulator: &Emulator,
    access_key_id: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let credential = format!(
        "{access_key_id}/20260101/{REGION}/events/aws4_request, SignedHeaders=host, \
         Signature=0000"
    );
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("events.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("AWSEvents.{operation}"))
        .header(
            "authorization",
            format!("AWS4-HMAC-SHA256 Credential={credential}"),
        )
        .body(bytes::Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let response = emulator.router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice::<Value>(&bytes).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// The wire error code (`__type`), without its namespace; the empty string on a success.
fn error_code(body: &Value) -> &str {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
}

/// A `PutRule` body for `name` carrying the pattern `pattern`.
fn put_rule_body(name: &str, source: &str) -> Value {
    json!({
        "Name": name,
        "EventBusName": DEFAULT_EVENT_BUS_NAME,
        "EventPattern": json!({"source": [source]}).to_string(),
        "State": "ENABLED",
    })
}

/// What `DescribeRule` reports for `name`: its `ManagedBy` and its `EventPattern`.
async fn described(emulator: &Emulator, case: &str, name: &str) -> (Value, Value) {
    let (status, rule) = call_as(
        emulator,
        CUSTOMER_KEY,
        "DescribeRule",
        &json!({"Name": name, "EventBusName": DEFAULT_EVENT_BUS_NAME}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeRule {name:?} must answer 200, got {status} with {rule}"
    );
    (
        rule.get("ManagedBy").cloned().unwrap_or(Value::Null),
        rule.get("EventPattern").cloned().unwrap_or(Value::Null),
    )
}

/// One row: who writes the rule, and the `ManagedBy` the rule then reports.
struct Writer {
    case: &'static str,
    access_key_id: &'static str,
    expected_managed_by: Value,
}

/// RULE (the write half): a rule reports as its `ManagedBy` the service principal of the call
/// that wrote it, and none at all for a client's own call. The rows differ along exactly what
/// the rule turns on: whether the call's credential has a record in the shared directory, and
/// which principal it names.
#[tokio::test]
async fn a_rule_reports_the_service_principal_of_the_call_that_wrote_it() {
    let emulator = emulator();
    remember_service_call(
        &emulator,
        OWNER_KEY,
        OWNER_PRINCIPAL,
        &format!(
            "arn:aws:guardduty:{REGION}:{}:malware-protection-plan/0123456789abcdef0123",
            engine::emulator_account_id()
        ),
    );
    remember_service_call(
        &emulator,
        OTHER_KEY,
        OTHER_PRINCIPAL,
        &format!(
            "arn:aws:backup:{REGION}:{}:backup-plan:0123",
            engine::emulator_account_id()
        ),
    );

    let rows = [
        Writer {
            case: "a client's own call",
            access_key_id: CUSTOMER_KEY,
            expected_managed_by: Value::Null,
        },
        Writer {
            case: "a call made as one service's principal",
            access_key_id: OWNER_KEY,
            expected_managed_by: json!(OWNER_PRINCIPAL),
        },
        Writer {
            case: "a call made as another service's principal",
            access_key_id: OTHER_KEY,
            expected_managed_by: json!(OTHER_PRINCIPAL),
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let name = format!("written-by-{index}");
        let (status, written) = call_as(
            &emulator,
            row.access_key_id,
            "PutRule",
            &put_rule_body(&name, "example.first"),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: PutRule must answer 200, got {status} with {written}",
            row.case
        );
        let (managed_by, _) = described(&emulator, row.case, &name).await;
        assert_eq!(
            managed_by, row.expected_managed_by,
            "case {:?}: expected ManagedBy {}, got {managed_by}",
            row.case, row.expected_managed_by
        );
        // The same reading reaches the listing, which is where a client discovers a managed
        // rule it did not write.
        let (status, listed) = call_as(
            &emulator,
            CUSTOMER_KEY,
            "ListRules",
            &json!({"NamePrefix": &name, "EventBusName": DEFAULT_EVENT_BUS_NAME}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: ListRules must answer 200, got {status} with {listed}",
            row.case
        );
        assert_eq!(
            listed
                .pointer("/Rules/0/ManagedBy")
                .cloned()
                .unwrap_or(Value::Null),
            row.expected_managed_by,
            "case {:?}: expected the listing to report ManagedBy {}, got {listed}",
            row.case,
            row.expected_managed_by
        );
    }

    // A service's write over a rule the CUSTOMER owns does not take it over: the owner is
    // settled when the rule comes into existence, so the rule stays the customer's and the
    // customer can still delete it unforced.
    let shared = "written-by-the-customer-first";
    let (status, written) = call_as(
        &emulator,
        CUSTOMER_KEY,
        "PutRule",
        &put_rule_body(shared, "example.customer"),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"the customer's own rule\": PutRule must answer 200, got {status} with {written}"
    );
    let (status, rewritten) = call_as(
        &emulator,
        OWNER_KEY,
        "PutRule",
        &put_rule_body(shared, "example.service"),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"a service's write over the customer's rule\": expected it to be served, got \
         {status} with {rewritten}"
    );
    let (managed_by, _) = described(&emulator, "a service's write over the customer's rule", shared).await;
    assert_eq!(
        managed_by,
        Value::Null,
        "case \"a service's write over the customer's rule\": the rule stays the customer's, got \
         {managed_by}"
    );
    let (status, deleted) = call_as(
        &emulator,
        CUSTOMER_KEY,
        "DeleteRule",
        &json!({"Name": shared, "EventBusName": DEFAULT_EVENT_BUS_NAME}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"a service's write over the customer's rule\": the customer's unforced delete must \
         still be served, got {status} with {deleted}"
    );
}

/// One row of the writer table over an existing MANAGED rule: who writes it, and whether that
/// write is admitted.
struct ManagedWrite {
    case: &'static str,
    access_key_id: &'static str,
    admitted: bool,
}

/// The rows: the one writer a managed rule admits is the service that OWNS it; a client and
/// another service are refused.
fn managed_writes() -> [ManagedWrite; 3] {
    [
        ManagedWrite {
            case: "the service that owns the rule",
            access_key_id: OWNER_KEY,
            admitted: true,
        },
        ManagedWrite {
            case: "a client's own call",
            access_key_id: CUSTOMER_KEY,
            admitted: false,
        },
        ManagedWrite {
            case: "another service's call",
            access_key_id: OTHER_KEY,
            admitted: false,
        },
    ]
}

/// RULE (the re-write half): a managed rule admits the write of the service that owns it and
/// refuses every other writer with `ManagedRuleException`, leaving the rule exactly as it was —
/// for the rule itself (`PutRule`) and for its targets (`PutTargets`).
#[tokio::test]
async fn a_managed_rule_admits_only_the_service_that_owns_it() {
    let emulator = emulator();
    remember_service_call(
        &emulator,
        OWNER_KEY,
        OWNER_PRINCIPAL,
        &format!(
            "arn:aws:guardduty:{REGION}:{}:malware-protection-plan/0123456789abcdef0123",
            engine::emulator_account_id()
        ),
    );
    remember_service_call(
        &emulator,
        OTHER_KEY,
        OTHER_PRINCIPAL,
        &format!(
            "arn:aws:backup:{REGION}:{}:backup-plan:0123",
            engine::emulator_account_id()
        ),
    );
    let name = "owned-by-one-service";
    let queue_arn = format!(
        "arn:aws:sqs:{REGION}:{}:a-queue",
        engine::emulator_account_id()
    );
    let (status, written) = call_as(
        &emulator,
        OWNER_KEY,
        "PutRule",
        &put_rule_body(name, "example.owner"),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: the owning service's PutRule must answer 200, got {status} with {written}"
    );

    for (index, row) in managed_writes().iter().enumerate() {
        // The pattern each writer submits is its own, so a write that was WRONGLY admitted
        // shows up as the stored pattern having moved.
        let source = format!("example.rewrite-{index}");
        let (before_managed_by, before_pattern) = described(&emulator, row.case, name).await;
        let (status, answered) = call_as(
            &emulator,
            row.access_key_id,
            "PutRule",
            &put_rule_body(name, &source),
        )
        .await;
        let (managed_by, pattern) = described(&emulator, row.case, name).await;
        assert_eq!(
            managed_by,
            json!(OWNER_PRINCIPAL),
            "case {:?}: the rule stays the owning service's managed rule, got {managed_by}",
            row.case
        );
        if row.admitted {
            assert_eq!(
                status, 200,
                "case {:?}: expected the owning service's write to be admitted, got {status} \
                 with {answered}",
                row.case
            );
            assert_eq!(
                pattern,
                json!(json!({"source": [source]}).to_string()),
                "case {:?}: expected the owning service's pattern to be stored, got {pattern}",
                row.case
            );
        } else {
            assert_eq!(
                status, 400,
                "case {:?}: expected a managed rule's other writers to be refused, got {status} \
                 with {answered}",
                row.case
            );
            assert_eq!(
                error_code(&answered),
                "ManagedRuleException",
                "case {:?}: expected a ManagedRuleException, got {answered}",
                row.case
            );
            assert_eq!(
                answered.get("message").cloned().unwrap_or(Value::Null),
                json!(format!("{name} is a managed rule.")),
                "case {:?}: expected the managed-rule sentence, got {answered}",
                row.case
            );
            assert_eq!(
                (managed_by, pattern),
                (before_managed_by, before_pattern),
                "case {:?}: a refused write must leave the rule exactly as it was",
                row.case
            );
        }

        // ... and the same rule for the rule's TARGETS.
        let (status, answered) = call_as(
            &emulator,
            row.access_key_id,
            "PutTargets",
            &json!({
                "Rule": name,
                "EventBusName": DEFAULT_EVENT_BUS_NAME,
                "Targets": [{"Id": format!("t{index}"), "Arn": &queue_arn}],
            }),
        )
        .await;
        let (listed_status, targets) = call_as(
            &emulator,
            CUSTOMER_KEY,
            "ListTargetsByRule",
            &json!({"Rule": name, "EventBusName": DEFAULT_EVENT_BUS_NAME}),
        )
        .await;
        assert_eq!(
            listed_status, 200,
            "case {:?}: ListTargetsByRule must answer 200, got {listed_status} with {targets}",
            row.case
        );
        let installed = targets["Targets"]
            .as_array()
            .map(Vec::len)
            .unwrap_or_default();
        if row.admitted {
            assert_eq!(
                status, 200,
                "case {:?}: expected the owning service's target write to be admitted, got \
                 {status} with {answered}",
                row.case
            );
            assert_eq!(
                installed, 1,
                "case {:?}: expected the owning service's target to be installed, got {targets}",
                row.case
            );
        } else {
            assert_eq!(
                status, 400,
                "case {:?}: expected a managed rule's other target writers to be refused, got \
                 {status} with {answered}",
                row.case
            );
            assert_eq!(
                error_code(&answered),
                "ManagedRuleException",
                "case {:?}: expected a ManagedRuleException, got {answered}",
                row.case
            );
            assert_eq!(
                installed, 1,
                "case {:?}: a refused target write must leave the rule's targets as they were, \
                 got {targets}",
                row.case
            );
        }
    }
}
