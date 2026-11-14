// SPDX-License-Identifier: CC0-1.0

//! `CreateFlowVersion` is ordered behind preparation, end to end over HTTP: a
//! numbered version is cut only from a flow the service holds in the `Prepared`
//! status, and a flow in any other modelled `FlowStatus` is refused with the
//! operation's modelled `ConflictException` without a version being cut.
//!
//! The table below spans the statuses a stored flow can be in when the call
//! arrives — never prepared, prepared and then changed, being prepared, left
//! `Failed` by a preparation a defect stopped, and prepared — so the rule is
//! asserted over the whole modelled enum rather than over one recorded status.
//! A second table takes the same statuses across the two forms the modelled
//! `FlowIdentifier` admits and across how often the refused cut is repeated,
//! because what decides the refusal is the addressed flow's stored status and
//! nothing else: not how the request spelled the identifier, not how many times
//! it was taken, and not what another flow in the same store stands in.
//!
//! Every assertion here goes through the router, so it exercises the real wire
//! path (route matching, model constraint validation, serialization) rather than
//! calling a handler directly.
//!
//! The one thing these tests reach into the store for is the INSTANT a pending
//! preparation comes due: `PrepareFlow` stores it rather than sleeping, so a
//! test moves it into the past instead of waiting out
//! [`FLOW_PREPARATION_SPAN`](service_com_amazonaws_bedrockagent_amazonbedrockagentbuildtimelambda::state::FLOW_PREPARATION_SPAN).

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_bedrockagent_amazonbedrockagentbuildtimelambda::state::service_state;
use service_com_amazonaws_bedrockagent_amazonbedrockagentbuildtimelambda::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 `Authorization` header scoped to the `bedrock` signing name: the
/// routing key this service claims on.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/bedrock/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0123456789abcdef";

/// A role ARN matching the modelled `FlowExecutionRoleArn` pattern
/// `^arn:aws(-[^:]+)?:iam::([0-9]{12})?:role/(service-role/)?.+$`.
const EXECUTION_ROLE: &str =
    "arn:aws:iam::000000000000:role/service-role/AmazonBedrockExecutionRoleForFlows";

/// One HTTP answer from the emulator, parsed.
#[derive(Debug)]
struct Answer {
    status: u16,
    /// The restJson1 error shape name, from `x-amzn-errortype` or the body's
    /// `__type`; `None` on a success.
    error_type: Option<String>,
    body: Value,
}

impl Answer {
    fn member(&self, path: &[&str]) -> &Value {
        let mut at = &self.body;
        for key in path {
            at = &at[*key];
        }
        at
    }

    fn string(&self, path: &[&str]) -> String {
        self.member(path)
            .as_str()
            .unwrap_or_else(|| panic!("{path:?} is not a string in {:?}", self.body))
            .to_string()
    }

    /// The `version` member of every reported summary, in the order the
    /// response listed them.
    fn listed_versions(&self) -> Vec<String> {
        self.member(&["flowVersionSummaries"])
            .as_array()
            .cloned()
            .unwrap_or_else(|| {
                panic!("flowVersionSummaries is not an array in {:?}", self.body)
            })
            .iter()
            .map(|summary| {
                summary["version"]
                    .as_str()
                    .unwrap_or_else(|| panic!("a summary carries no version: {summary:?}"))
                    .to_string()
            })
            .collect()
    }
}

/// The emulator under test: one router over one shared engine, exactly as the
/// production router mounts this service.
struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    async fn call(&self, method: &str, uri: &str, body: Option<Value>) -> Answer {
        let bytes = body
            .map(|value| Bytes::from(serde_json::to_vec(&value).expect("serialize request body")))
            .unwrap_or_default();
        let request = http::Request::builder()
            .method(method)
            .uri(uri)
            .header("host", "bedrock-agent.eu-west-1.amazonaws.com")
            .header("authorization", AUTHORIZATION)
            .header("content-type", "application/json")
            .body(bytes)
            .expect("build the request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let header_error = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(ToString::to_string);
        let raw = response.into_body();
        let body: Value = if raw.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&raw).unwrap_or_else(|e| {
                panic!(
                    "{method} {uri} -> {status} body is not JSON ({e}): {}",
                    String::from_utf8_lossy(&raw)
                )
            })
        };
        let error_type = header_error.or_else(|| {
            body.get("__type")
                .and_then(Value::as_str)
                .map(ToString::to_string)
        });
        Answer {
            status,
            error_type,
            body,
        }
    }

    async fn create_flow(&self, case: &str, definition: Value) -> Answer {
        let created = self
            .call(
                "POST",
                "/flows/",
                Some(json!({
                    "name": "prepare-ordering-flow",
                    "executionRoleArn": EXECUTION_ROLE,
                    "definition": definition,
                })),
            )
            .await;
        assert_eq!(
            created.status, 201,
            "case {case}: expected CreateFlow -> 201, got {created:?}"
        );
        created
    }

    async fn update_flow(&self, case: &str, flow_id: &str, definition: Value) -> Answer {
        let updated = self
            .call(
                "PUT",
                &format!("/flows/{flow_id}/"),
                Some(json!({
                    "name": "prepare-ordering-flow",
                    "executionRoleArn": EXECUTION_ROLE,
                    "definition": definition,
                })),
            )
            .await;
        assert_eq!(
            updated.status, 200,
            "case {case}: expected UpdateFlow -> 200, got {updated:?}"
        );
        updated
    }

    async fn prepare_flow(&self, case: &str, flow_id: &str) -> Answer {
        let prepared = self
            .call("POST", &format!("/flows/{flow_id}/"), Some(json!({})))
            .await;
        assert_eq!(
            prepared.status, 202,
            "case {case}: expected PrepareFlow -> 202, got {prepared:?}"
        );
        prepared
    }

    async fn get_flow(&self, flow_id: &str) -> Answer {
        self.call("GET", &format!("/flows/{flow_id}/"), None).await
    }

    async fn create_flow_version(&self, flow_id: &str) -> Answer {
        self.create_flow_version_as(flow_id, json!({})).await
    }

    /// `CreateFlowVersion` addressed by an already-escaped `@httpLabel`
    /// identifier, carrying the request body the case sends.
    async fn create_flow_version_as(&self, identifier: &str, body: Value) -> Answer {
        self.call("POST", &format!("/flows/{identifier}/versions"), Some(body))
            .await
    }

    async fn get_flow_version(&self, identifier: &str, version: &str) -> Answer {
        self.call(
            "GET",
            &format!("/flows/{identifier}/versions/{version}/"),
            None,
        )
        .await
    }

    async fn list_flow_versions(&self, flow_id: &str) -> Answer {
        self.call("GET", &format!("/flows/{flow_id}/versions"), None)
            .await
    }

    /// Move the instant the pending preparation comes due into the past, so the
    /// next caller observes the outcome without the test sleeping for the span.
    fn settle_preparation(&self, case: &str, flow_id: &str) {
        let state = service_state(&*self.engine).expect("the bedrock-agent store");
        let mut state = state.lock();
        let pending = state
            .flows
            .get_mut(flow_id)
            .expect("the stored flow")
            .pending_preparation
            .as_mut()
            .unwrap_or_else(|| panic!("case {case}: expected a preparation in flight"));
        pending.due = SystemTime::now() - Duration::from_secs(1);
    }

    /// Drive the flow to `Prepared` from whatever status it is in: a sound
    /// definition, prepared, and the outcome settled.
    async fn drive_to_prepared(&self, case: &str, flow_id: &str) {
        self.update_flow(case, flow_id, passthrough_definition())
            .await;
        self.prepare_flow(case, flow_id).await;
        self.settle_preparation(case, flow_id);
    }

    /// Drive a freshly created flow into the status a row attempts the cut in.
    async fn drive_into(&self, case: &str, flow_id: &str, drive: Drive) {
        match drive {
            Drive::Created => {}
            Drive::ChangedAfterPreparing => {
                self.prepare_flow(case, flow_id).await;
                self.settle_preparation(case, flow_id);
                self.update_flow(case, flow_id, passthrough_definition())
                    .await;
            }
            Drive::Preparing => {
                self.prepare_flow(case, flow_id).await;
            }
            Drive::PreparationFailed | Drive::Prepared => {
                self.prepare_flow(case, flow_id).await;
                self.settle_preparation(case, flow_id);
            }
        }
    }
}

/// A passthrough flow definition: an `Input` node wired straight to an `Output`
/// node. The node types, the union keys and the IO data types are the modelled
/// `FlowNodeType` / `FlowNodeConfiguration` / `FlowNodeIODataType` values.
fn passthrough_definition() -> Value {
    json!({
        "nodes": [
            {
                "name": "FlowInput",
                "type": "Input",
                "configuration": { "input": {} },
                "outputs": [{ "name": "document", "type": "String" }],
            },
            {
                "name": "FlowOutput",
                "type": "Output",
                "configuration": { "output": {} },
                "inputs": [{ "name": "document", "type": "String", "expression": "$.data" }],
            },
        ],
        "connections": [
            {
                "name": "InputToOutput",
                "type": "Data",
                "source": "FlowInput",
                "target": "FlowOutput",
                "configuration": { "data": { "sourceOutput": "document", "targetInput": "document" } },
            },
        ],
    })
}

/// A definition whose nodes form a cycle, so preparing it ends `Failed`: the
/// flow input enters `Alpha`, `Alpha` feeds `Beta`, and `Beta` feeds `Alpha`
/// back through its second input, so the cycle is the definition's only defect.
fn cyclic_definition() -> Value {
    json!({
        "nodes": [
            {
                "name": "FlowInput",
                "type": "Input",
                "configuration": { "input": {} },
                "outputs": [{ "name": "document", "type": "String" }],
            },
            {
                "name": "Alpha",
                "type": "Collector",
                "configuration": { "collector": {} },
                "inputs": [
                    { "name": "seed", "type": "String", "expression": "$.data" },
                    { "name": "again", "type": "String", "expression": "$.data" },
                ],
                "outputs": [{ "name": "carried", "type": "String" }],
            },
            {
                "name": "Beta",
                "type": "Collector",
                "configuration": { "collector": {} },
                "inputs": [{ "name": "carried", "type": "String", "expression": "$.data" }],
                "outputs": [{ "name": "carried", "type": "String" }],
            },
            {
                "name": "FlowOutput",
                "type": "Output",
                "configuration": { "output": {} },
                "inputs": [{ "name": "document", "type": "String", "expression": "$.data" }],
            },
        ],
        "connections": [
            {
                "name": "InputToAlpha",
                "type": "Data",
                "source": "FlowInput",
                "target": "Alpha",
                "configuration": { "data": { "sourceOutput": "document", "targetInput": "seed" } },
            },
            {
                "name": "AlphaToBeta",
                "type": "Data",
                "source": "Alpha",
                "target": "Beta",
                "configuration": { "data": { "sourceOutput": "carried", "targetInput": "carried" } },
            },
            {
                "name": "BetaToAlpha",
                "type": "Data",
                "source": "Beta",
                "target": "Alpha",
                "configuration": { "data": { "sourceOutput": "carried", "targetInput": "again" } },
            },
            {
                "name": "AlphaToOutput",
                "type": "Data",
                "source": "Alpha",
                "target": "FlowOutput",
                "configuration": { "data": { "sourceOutput": "carried", "targetInput": "document" } },
            },
        ],
    })
}

/// The refusal a flow in `status` answers `CreateFlowVersion` with. The fixed
/// text is this error class's; the operation name and the status token are its
/// only variable parts, and both come from the call and the flow's own state.
fn conflict_message(status: &str) -> String {
    format!(
        "Couldn't perform CreateFlowVersion operation. The resource that you specified \
         is currently in status {status}. Retry your request after the resource changes \
         to a valid status."
    )
}

/// How a row below drives the flow into the status the cut is attempted in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Drive {
    /// Created and left alone: the status `CreateFlow` mints.
    Created,
    /// Prepared, settled, then changed again by `UpdateFlow`.
    ChangedAfterPreparing,
    /// `PrepareFlow` accepted and the outcome not yet due.
    Preparing,
    /// Prepared with a definition a defect stops, and the outcome settled.
    PreparationFailed,
    /// Prepared with a sound definition, and the outcome settled.
    Prepared,
}

/// Rows spanning the modelled `FlowStatus` values a stored flow can carry when
/// `CreateFlowVersion` arrives, with the status each one leaves the flow in.
const STATUS_CASES: &[(&str, Drive, &str)] = &[
    ("a flow nobody has prepared", Drive::Created, "NotPrepared"),
    (
        "a flow changed after it was prepared",
        Drive::ChangedAfterPreparing,
        "NotPrepared",
    ),
    ("a flow still being prepared", Drive::Preparing, "Preparing"),
    (
        "a flow whose preparation failed",
        Drive::PreparationFailed,
        "Failed",
    ),
    ("a prepared flow", Drive::Prepared, "Prepared"),
];

/// The rule: `CreateFlowVersion` cuts a version only from a flow in the
/// `Prepared` status. Every other status is refused 409 `ConflictException`
/// naming the status the flow is in, and the refusal writes nothing — the
/// listing gains no entry and no version number is consumed, which the cut taken
/// after the flow is driven to `Prepared` proves by still being version 1.
#[tokio::test]
async fn create_flow_version_is_ordered_behind_preparation() {
    for (case, drive, expected_status) in STATUS_CASES {
        let emulator = Emulator::new();
        let definition = if *drive == Drive::PreparationFailed {
            cyclic_definition()
        } else {
            passthrough_definition()
        };
        let flow = emulator.create_flow(case, definition).await;
        let flow_id = flow.string(&["id"]);

        emulator.drive_into(case, &flow_id, *drive).await;

        // The status the rule reads is the one the service reports for the flow.
        let read = emulator.get_flow(&flow_id).await;
        assert_eq!(
            read.string(&["status"]),
            *expected_status,
            "case {case}: expected the flow to stand in {expected_status}, got {read:?}"
        );

        let cut = emulator.create_flow_version(&flow_id).await;
        if *expected_status == "Prepared" {
            assert_eq!(
                cut.status, 201,
                "case {case}: a prepared flow is versioned, got {cut:?}"
            );
            assert_eq!(
                cut.string(&["version"]),
                "1",
                "case {case}: the first version cut is numbered 1, got {cut:?}"
            );
            assert_eq!(
                cut.string(&["status"]),
                "Prepared",
                "case {case}: the version snapshots the prepared DRAFT, got {cut:?}"
            );
            assert_eq!(
                emulator.list_flow_versions(&flow_id).await.listed_versions(),
                vec!["DRAFT".to_string(), "1".to_string()],
                "case {case}: the cut version is enumerated beside the DRAFT"
            );
            continue;
        }

        assert_eq!(
            cut.status, 409,
            "case {case}: expected CreateFlowVersion -> 409, got {cut:?}"
        );
        assert_eq!(
            cut.error_type.as_deref(),
            Some("ConflictException"),
            "case {case}: expected ConflictException, got {cut:?}"
        );
        assert_eq!(
            cut.string(&["message"]),
            conflict_message(expected_status),
            "case {case}: expected the refusal to name the operation and the flow's \
             status, got {cut:?}"
        );
        assert_eq!(
            emulator.list_flow_versions(&flow_id).await.listed_versions(),
            vec!["DRAFT".to_string()],
            "case {case}: a refused cut leaves the flow with its DRAFT version alone"
        );

        // No version number was consumed either: once the flow reaches
        // `Prepared`, the version it is cut at is still the first one.
        emulator.drive_to_prepared(case, &flow_id).await;
        let cut = emulator.create_flow_version(&flow_id).await;
        assert_eq!(
            cut.status, 201,
            "case {case}: the flow is versioned once it is prepared, got {cut:?}"
        );
        assert_eq!(
            cut.string(&["version"]),
            "1",
            "case {case}: the refused cut consumed no version number, got {cut:?}"
        );
        assert_eq!(
            emulator.list_flow_versions(&flow_id).await.listed_versions(),
            vec!["DRAFT".to_string(), "1".to_string()],
            "case {case}: the flow holds exactly the one version it was cut at"
        );
    }
}

/// The guard is read off the flow's STORED status, not off the call that
/// preceded it: the same flow refuses a cut while its preparation is in flight,
/// serves one once that preparation's outcome is due, and refuses again as soon
/// as a change puts the flow back out of `Prepared`.
#[tokio::test]
async fn the_version_guard_follows_the_flow_through_its_statuses() {
    let case = "one flow through the whole progression";
    let emulator = Emulator::new();
    let flow = emulator
        .create_flow(case, passthrough_definition())
        .await;
    let flow_id = flow.string(&["id"]);

    // Preparing: refused while the outcome is not yet due.
    emulator.prepare_flow(case, &flow_id).await;
    let refused = emulator.create_flow_version(&flow_id).await;
    assert_eq!(
        refused.string(&["message"]),
        conflict_message("Preparing"),
        "case {case}: a cut taken mid-preparation is refused, got {refused:?}"
    );

    // Prepared: served.
    emulator.settle_preparation(case, &flow_id);
    let first = emulator.create_flow_version(&flow_id).await;
    assert_eq!(
        first.status, 201,
        "case {case}: expected CreateFlowVersion -> 201, got {first:?}"
    );

    // A second cut of the same prepared flow is served too: the guard is about
    // the flow's status, not about how many versions it already holds.
    let second = emulator.create_flow_version(&flow_id).await;
    assert_eq!(
        second.string(&["version"]),
        "2",
        "case {case}: a prepared flow can be cut again, got {second:?}"
    );

    // Changed: refused again, and the versions already cut stand.
    emulator
        .update_flow(case, &flow_id, passthrough_definition())
        .await;
    let refused = emulator.create_flow_version(&flow_id).await;
    assert_eq!(
        refused.status, 409,
        "case {case}: a cut taken after the DRAFT changed is refused, got {refused:?}"
    );
    assert_eq!(
        refused.string(&["message"]),
        conflict_message("NotPrepared"),
        "case {case}: the refusal names the status the change left, got {refused:?}"
    );
    assert_eq!(
        emulator.list_flow_versions(&flow_id).await.listed_versions(),
        vec!["DRAFT".to_string(), "1".to_string(), "2".to_string()],
        "case {case}: the refusal left the versions already cut exactly as they were"
    );
}

/// The `@httpLabel` spelling of a flow identifier: the `:` and `/` an ARN
/// carries are percent-encoded, which is how a client sends a label holding
/// them.
fn escaped(identifier: &str) -> String {
    identifier.replace(':', "%3A").replace('/', "%2F")
}

/// How a row addresses the flow it attempts the cut on: the two forms the
/// modelled `FlowIdentifier` admits (pattern
/// `^(arn:aws:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:flow/[0-9a-zA-Z]{10})|([0-9a-zA-Z]{10})$`).
/// Both name the same stored flow, so both are answered by that flow's status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Address {
    /// The bare `flowId`.
    BareId,
    /// The flow's own ARN, as the create reported it.
    Arn,
}

impl Address {
    /// The `@httpLabel` value this form sends for the created flow.
    fn label(self, flow: &Answer) -> String {
        match self {
            Self::BareId => flow.string(&["id"]),
            Self::Arn => escaped(&flow.string(&["arn"])),
        }
    }
}

/// An idempotency token shaped as the modelled `ClientToken` (len 33..256,
/// pattern `^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,256}$`).
const CLIENT_TOKEN: &str = "prepare-ordering-refusal-token-0001";

/// The rows of [`STATUS_CASES`] a cut is refused in: every modelled
/// `FlowStatus` other than `Prepared`, with the token the service reports for a
/// flow driven into it.
const REFUSING_STATUSES: &[(&str, Drive, &str)] = &[
    ("never prepared", Drive::Created, "NotPrepared"),
    (
        "changed after it was prepared",
        Drive::ChangedAfterPreparing,
        "NotPrepared",
    ),
    ("preparation in flight", Drive::Preparing, "Preparing"),
    (
        "preparation a cyclic definition failed",
        Drive::PreparationFailed,
        "Failed",
    ),
];

/// How many times a row takes the refused cut before the flow is prepared: a
/// refusal that writes nothing writes nothing however often it is repeated.
const ATTEMPTS: &[u32] = &[1, 3];

/// The rule across the inputs it generalises over: the refusal is decided by
/// the ADDRESSED flow's stored status, so it is the same whichever admitted
/// identifier form names the flow and however often the cut is attempted — and
/// it writes nothing, which the cut served after the flow reaches `Prepared`
/// proves by being version 1, by replaying the token the refused calls carried,
/// and by being the first version `GetFlowVersion` can read at all.
#[tokio::test]
async fn the_refused_cut_is_decided_by_the_addressed_flow_and_writes_nothing() {
    for (status_case, drive, expected_status) in REFUSING_STATUSES {
        for address in [Address::BareId, Address::Arn] {
            for attempts in ATTEMPTS {
                let case =
                    format!("{status_case}, addressed by {address:?}, refused {attempts} time(s)");
                let case = case.as_str();
                let emulator = Emulator::new();
                let definition = if *drive == Drive::PreparationFailed {
                    cyclic_definition()
                } else {
                    passthrough_definition()
                };
                let flow = emulator.create_flow(case, definition).await;
                let flow_id = flow.string(&["id"]);
                let label = address.label(&flow);
                emulator.drive_into(case, &flow_id, *drive).await;

                // The status the rule reads is the one the service reports.
                let read = emulator.get_flow(&flow_id).await;
                assert_eq!(
                    read.string(&["status"]),
                    *expected_status,
                    "case {case}: expected the flow to stand in {expected_status}, got {read:?}"
                );

                for attempt in 1..=*attempts {
                    let refused = emulator
                        .create_flow_version_as(&label, json!({ "clientToken": CLIENT_TOKEN }))
                        .await;
                    assert_eq!(
                        refused.status, 409,
                        "case {case}: attempt {attempt} expected CreateFlowVersion -> 409, \
                         got {refused:?}"
                    );
                    assert_eq!(
                        refused.error_type.as_deref(),
                        Some("ConflictException"),
                        "case {case}: attempt {attempt} expected ConflictException, got {refused:?}"
                    );
                    assert_eq!(
                        refused.string(&["message"]),
                        conflict_message(expected_status),
                        "case {case}: attempt {attempt} expected the refusal to name the \
                         operation and the flow's status, got {refused:?}"
                    );
                }

                // Nothing was written: the number the refused cuts would have
                // taken is readable nowhere, the listing holds the DRAFT alone,
                // and the flow still stands in the status it was addressed in.
                let missing = emulator.get_flow_version(&label, "1").await;
                assert_eq!(
                    missing.status, 404,
                    "case {case}: a refused cut leaves no version to read, got {missing:?}"
                );
                assert_eq!(
                    missing.error_type.as_deref(),
                    Some("ResourceNotFoundException"),
                    "case {case}: expected ResourceNotFoundException, got {missing:?}"
                );
                assert_eq!(
                    emulator.list_flow_versions(&flow_id).await.listed_versions(),
                    vec!["DRAFT".to_string()],
                    "case {case}: a refused cut leaves the flow with its DRAFT version alone"
                );
                let read = emulator.get_flow(&flow_id).await;
                assert_eq!(
                    read.string(&["status"]),
                    *expected_status,
                    "case {case}: the refusal moved the flow's status, got {read:?}"
                );

                // Prepared, the very same call is served — and takes the first
                // version number, so the refusals consumed none.
                emulator.drive_to_prepared(case, &flow_id).await;
                let cut = emulator
                    .create_flow_version_as(&label, json!({ "clientToken": CLIENT_TOKEN }))
                    .await;
                assert_eq!(
                    cut.status, 201,
                    "case {case}: the flow is versioned once it is prepared, got {cut:?}"
                );
                assert_eq!(
                    cut.string(&["version"]),
                    "1",
                    "case {case}: the refused cuts consumed no version number, got {cut:?}"
                );
                assert_eq!(
                    cut.string(&["status"]),
                    "Prepared",
                    "case {case}: the version snapshots the prepared DRAFT, got {cut:?}"
                );

                // The token the refused calls carried was never recorded against
                // a version, so it binds to THIS cut and now replays it rather
                // than cutting a second one.
                let replayed = emulator
                    .create_flow_version_as(&label, json!({ "clientToken": CLIENT_TOKEN }))
                    .await;
                assert_eq!(
                    replayed.string(&["version"]),
                    "1",
                    "case {case}: the retried cut replays the version it cut, got {replayed:?}"
                );
                assert_eq!(
                    emulator.list_flow_versions(&flow_id).await.listed_versions(),
                    vec!["DRAFT".to_string(), "1".to_string()],
                    "case {case}: the flow holds exactly the one version it was cut at"
                );
                let read_back = emulator.get_flow_version(&label, "1").await;
                assert_eq!(
                    read_back.string(&["version"]),
                    "1",
                    "case {case}: the cut version reads back, got {read_back:?}"
                );
            }
        }
    }
}

/// The guard is the ADDRESSED flow's, not the store's: with one prepared and
/// one never-prepared flow held side by side, each call is answered by the
/// status of the flow it names, and the prepared flow's numbering advances
/// across refusals of the other.
#[tokio::test]
async fn each_flow_is_answered_by_its_own_status() {
    let case = "one prepared and one unprepared flow in the same store";
    let emulator = Emulator::new();

    let prepared = emulator.create_flow(case, passthrough_definition()).await;
    let prepared_id = prepared.string(&["id"]);
    emulator.prepare_flow(case, &prepared_id).await;
    emulator.settle_preparation(case, &prepared_id);

    let unprepared = emulator.create_flow(case, passthrough_definition()).await;
    let unprepared_id = unprepared.string(&["id"]);

    for round in 1..=2u32 {
        let refused = emulator.create_flow_version(&unprepared_id).await;
        assert_eq!(
            refused.status, 409,
            "case {case}: round {round}: the unprepared flow is refused, got {refused:?}"
        );
        assert_eq!(
            refused.string(&["message"]),
            conflict_message("NotPrepared"),
            "case {case}: round {round}: the refusal names the addressed flow's status, \
             got {refused:?}"
        );

        let cut = emulator.create_flow_version(&prepared_id).await;
        assert_eq!(
            cut.status, 201,
            "case {case}: round {round}: the prepared flow is versioned, got {cut:?}"
        );
        assert_eq!(
            cut.string(&["version"]),
            round.to_string(),
            "case {case}: round {round}: the prepared flow's own numbering advances, got {cut:?}"
        );
    }

    assert_eq!(
        emulator
            .list_flow_versions(&unprepared_id)
            .await
            .listed_versions(),
        vec!["DRAFT".to_string()],
        "case {case}: the refused flow holds its DRAFT version alone"
    );
    assert_eq!(
        emulator
            .list_flow_versions(&prepared_id)
            .await
            .listed_versions(),
        vec!["DRAFT".to_string(), "1".to_string(), "2".to_string()],
        "case {case}: the prepared flow holds exactly the versions it was cut at"
    );
}
