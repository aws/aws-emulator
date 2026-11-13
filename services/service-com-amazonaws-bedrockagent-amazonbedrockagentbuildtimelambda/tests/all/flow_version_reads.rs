// SPDX-License-Identifier: CC0-1.0

//! `GetFlowVersion`, end to end over HTTP: the reader a numbered flow version is
//! round-tripped through.
//!
//! The rule pinned here: `GetFlowVersion` reports the stored snapshot the cut
//! took of the flow's DRAFT, so a version reports what it was cut from however
//! the DRAFT has moved since, and an identifier that resolves to no stored flow
//! or to a number that flow was never cut at is the modelled
//! `ResourceNotFoundException` rather than a response.
//!
//! Each test drives a TABLE, and the rows differ along what the rule generalises
//! over: the flow the version belongs to, the `FlowIdentifier` form that
//! addresses it, the version number within the flow's sequence, the
//! `includedData` scope, and — for the refusals — which of the two addressed
//! resources is missing.
//!
//! `flow_version_snapshots.rs` covers what `CreateFlowVersion` stores and
//! `flow_version_prepare_ordering.rs` the ordering guard in front of it; this
//! suite covers reading one back. Every assertion goes through the router, so it
//! exercises the real wire path (route matching, model constraint validation,
//! serialization) rather than calling a handler directly.
//!
//! The one thing these tests reach into the store for is the INSTANT a pending
//! preparation comes due: `PrepareFlow` stores it rather than sleeping, so a
//! test moves it into the past instead of waiting out
//! [`FLOW_PREPARATION_SPAN`](service_com_amazonaws_bedrockagent_amazonbedrockagentbuildtimelambda::state::FLOW_PREPARATION_SPAN).

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use bytes::Bytes;
use serde_json::{json, Map, Value};
use service_com_amazonaws_bedrockagent_amazonbedrockagentbuildtimelambda::state::service_state;
use service_com_amazonaws_bedrockagent_amazonbedrockagentbuildtimelambda::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 `Authorization` header scoped to the `bedrock` signing name: the
/// routing key this service claims on, and the source of the region its ARNs
/// carry.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/bedrock/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0123456789abcdef";

/// Role ARNs, each a form the modelled `FlowExecutionRoleArn` pattern
/// `^arn:aws(-[^:]+)?:iam::([0-9]{12})?:role/(service-role/)?.+$` admits.
const SERVICE_LINKED_ROLE: &str =
    "arn:aws:iam::000000000000:role/service-role/AmazonBedrockExecutionRoleForFlows";
const PLAIN_ROLE: &str = "arn:aws:iam::000000000000:role/BedrockFlowsRole";

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
    fn string(&self, member: &str) -> String {
        self.body[member]
            .as_str()
            .unwrap_or_else(|| panic!("{member} is not a string in {:?}", self.body))
            .to_string()
    }

    /// The response body as an object, so a test can compare two whole responses
    /// member for member.
    fn object(&self) -> Map<String, Value> {
        self.body
            .as_object()
            .cloned()
            .unwrap_or_else(|| panic!("the body is not an object: {:?}", self.body))
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

    async fn create_flow(&self, case: &str, request: Value) -> Answer {
        let created = self.call("POST", "/flows/", Some(request)).await;
        assert_eq!(
            created.status, 201,
            "case {case}: expected CreateFlow -> 201, got {created:?}"
        );
        created
    }

    async fn update_flow(&self, case: &str, flow_id: &str, request: Value) -> Answer {
        let updated = self
            .call("PUT", &format!("/flows/{flow_id}/"), Some(request))
            .await;
        assert_eq!(
            updated.status, 200,
            "case {case}: expected UpdateFlow -> 200, got {updated:?}"
        );
        updated
    }

    async fn get_flow(&self, flow_id: &str) -> Answer {
        self.call("GET", &format!("/flows/{flow_id}/"), None).await
    }

    async fn create_flow_version(&self, case: &str, identifier: &str, request: Value) -> Answer {
        let cut = self
            .call(
                "POST",
                &format!("/flows/{identifier}/versions"),
                Some(request),
            )
            .await;
        assert_eq!(
            cut.status, 201,
            "case {case}: expected CreateFlowVersion -> 201, got {cut:?}"
        );
        cut
    }

    async fn get_flow_version(
        &self,
        identifier: &str,
        version: &str,
        included_data: Option<&str>,
    ) -> Answer {
        let query =
            included_data.map_or_else(String::new, |value| format!("?includedData={value}"));
        self.call(
            "GET",
            &format!(
                "/flows/{}/versions/{version}/{query}",
                escaped(identifier)
            ),
            None,
        )
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

    /// Drive the flow to `Prepared`, which is the one status a version is cut
    /// from, without waiting out the preparation span.
    async fn prepare(&self, case: &str, flow_id: &str) {
        let prepared = self
            .call("POST", &format!("/flows/{flow_id}/"), Some(json!({})))
            .await;
        assert_eq!(
            prepared.status, 202,
            "case {case}: expected PrepareFlow -> 202, got {prepared:?}"
        );
        self.settle_preparation(case, flow_id);
        let read = self.get_flow(flow_id).await;
        assert_eq!(
            read.string("status"),
            "Prepared",
            "case {case}: expected the flow to reach Prepared, got {read:?}"
        );
    }

    /// Create a flow, prepare it and cut its first version: the shortest path to
    /// a stored numbered version.
    async fn flow_with_one_version(&self, case: &str, request: Value) -> (String, Answer) {
        let flow = self.create_flow(case, request).await;
        let flow_id = flow.string("id");
        self.prepare(case, &flow_id).await;
        let cut = self
            .create_flow_version(case, &flow_id, json!({}))
            .await;
        (flow_id, cut)
    }
}

/// The `@httpLabel` spelling of a flow identifier: the `:` and `/` an ARN
/// carries are percent-encoded, which is how a client sends a label carrying
/// them.
fn escaped(identifier: &str) -> String {
    identifier.replace(':', "%3A").replace('/', "%2F")
}

/// A passthrough flow definition: an `Input` node wired straight to an `Output`
/// node. The node types, the union keys and the IO data types are the modelled
/// `FlowNodeType` / `FlowNodeConfiguration` / `FlowNodeIODataType` values.
fn passthrough_definition(connection_name: &str) -> Value {
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
                "name": connection_name,
                "type": "Data",
                "source": "FlowInput",
                "target": "FlowOutput",
                "configuration": { "data": { "sourceOutput": "document", "targetInput": "document" } },
            },
        ],
    })
}

/// A `CreateFlow`/`UpdateFlow` request body for a flow named `name`.
fn flow_request(name: &str, description: Option<&str>, role: &str, connection: &str) -> Value {
    let mut request = json!({
        "name": name,
        "executionRoleArn": role,
        "definition": passthrough_definition(connection),
    });
    if let Some(description) = description {
        request["description"] = json!(description);
    }
    request
}

/// Rows spanning the flows a version can be cut from: the name, the presence of
/// a description, the `FlowExecutionRoleArn` form and the definition all vary,
/// so the read is asserted over a set of snapshots rather than one.
const FLOW_CASES: &[(&str, &str, Option<&str>, &str, &str)] = &[
    (
        "a flow with a description and the service-linked role",
        "reads-described",
        Some("the flow this version was cut from"),
        SERVICE_LINKED_ROLE,
        "InputToOutput",
    ),
    (
        "a flow with no description and a plain role",
        "reads-plain",
        None,
        PLAIN_ROLE,
        "StraightThrough",
    ),
    (
        "a flow whose name uses the underscore separator",
        "reads_underscored",
        Some("a second snapshot in the same store"),
        PLAIN_ROLE,
        "Passthrough",
    ),
];

/// The rule: `GetFlowVersion` reports the stored snapshot, member for member the
/// same resource the cut reported — through either `FlowIdentifier` form, and
/// for each flow in a store holding several.
#[tokio::test]
async fn get_flow_version_reports_the_version_the_cut_stored() {
    let emulator = Emulator::new();
    // Every row's flow lives in the SAME store, so a read that reported another
    // flow's version would be caught by the member comparison below.
    let mut cuts = Vec::new();
    for (case, name, description, role, connection) in FLOW_CASES {
        let (flow_id, cut) = emulator
            .flow_with_one_version(case, flow_request(name, *description, role, connection))
            .await;
        cuts.push((*case, flow_id, cut));
    }

    for (case, flow_id, cut) in &cuts {
        let arn = cut.string("arn");
        // Both forms the modelled `FlowIdentifier` admits address the same flow.
        for identifier in [flow_id.as_str(), arn.as_str()] {
            let read = emulator.get_flow_version(identifier, "1", None).await;
            assert_eq!(
                read.status, 200,
                "case {case} ({identifier}): expected GetFlowVersion -> 200, got {read:?}"
            );
            assert_eq!(
                read.object(),
                cut.object(),
                "case {case} ({identifier}): expected the read to report the version the cut \
                 stored, got {read:?}"
            );
            assert_eq!(
                read.string("id"),
                *flow_id,
                "case {case} ({identifier}): expected the version's own flow, got {read:?}"
            );
            assert_eq!(
                read.string("version"),
                "1",
                "case {case} ({identifier}): the first version cut is numbered 1, got {read:?}"
            );
            assert_eq!(
                read.string("status"),
                "Prepared",
                "case {case} ({identifier}): the version snapshots the prepared DRAFT, got \
                 {read:?}"
            );
        }
    }

    // A read changes nothing a later caller sees: the same read answers the same
    // thing, and the DRAFT is untouched.
    for (case, flow_id, cut) in &cuts {
        let reread = emulator.get_flow_version(flow_id, "1", None).await;
        assert_eq!(
            reread.object(),
            cut.object(),
            "case {case}: expected a repeated read to report the same version, got {reread:?}"
        );
        let draft = emulator.get_flow(flow_id).await;
        assert_eq!(
            draft.string("version"),
            "DRAFT",
            "case {case}: reading a version left the flow's DRAFT alone, got {draft:?}"
        );
        assert_eq!(
            draft.string("status"),
            "Prepared",
            "case {case}: reading a version moved no status, got {draft:?}"
        );
    }
}

/// The `includedData` values a read can carry. Source: the modelled
/// `IncludedData` enum (`ALL_DATA | METADATA_ONLY`), plus the member left out,
/// which the member's own doc equates with `ALL_DATA` ("Set to ALL_DATA or omit
/// this field to return the full response").
const INCLUDED_DATA_CASES: &[(&str, Option<&str>, bool)] = &[
    ("includedData omitted", None, true),
    ("includedData=ALL_DATA", Some("ALL_DATA"), true),
    ("includedData=METADATA_ONLY", Some("METADATA_ONLY"), false),
];

/// The rule's scope member: `includedData` decides whether the flow definition
/// is reported, and nothing else about the response.
#[tokio::test]
async fn included_data_decides_whether_the_definition_is_reported() {
    let emulator = Emulator::new();
    let (flow_id, cut) = emulator
        .flow_with_one_version(
            "the definition scope",
            flow_request(
                "reads-scoped",
                Some("a version read at each scope"),
                SERVICE_LINKED_ROLE,
                "InputToOutput",
            ),
        )
        .await;

    for (case, included_data, carries_definition) in INCLUDED_DATA_CASES {
        let read = emulator
            .get_flow_version(&flow_id, "1", *included_data)
            .await;
        assert_eq!(
            read.status, 200,
            "case {case}: expected GetFlowVersion -> 200, got {read:?}"
        );
        assert_eq!(
            read.body.get("definition").is_some(),
            *carries_definition,
            "case {case}: expected definition reported = {carries_definition}, got {read:?}"
        );
        if *carries_definition {
            assert_eq!(
                read.body["definition"],
                cut.body["definition"],
                "case {case}: expected the definition the version packages, got {read:?}"
            );
        }

        // Every other member is the snapshot's, whatever the scope.
        let mut expected = cut.object();
        let mut actual = read.object();
        expected.remove("definition");
        actual.remove("definition");
        assert_eq!(
            actual, expected,
            "case {case}: expected the scope to move the definition alone, got {read:?}"
        );
    }
}

/// The rule applied to a DRAFT that keeps moving: a numbered version reports
/// what it was cut from, so each of a flow's versions reports its own snapshot
/// while the DRAFT reports the latest change.
#[tokio::test]
async fn a_version_reports_what_it_was_cut_from_however_the_draft_moves() {
    let case = "one flow cut at three versions as its DRAFT changes";
    let emulator = Emulator::new();
    let (flow_id, first) = emulator
        .flow_with_one_version(
            case,
            flow_request(
                "reads-generation-1",
                Some("generation 1"),
                SERVICE_LINKED_ROLE,
                "Generation1",
            ),
        )
        .await;

    // Each later generation changes every member the snapshot carries, so a read
    // that fell back to the flow record would report the wrong one.
    let generations = [
        ("2", "reads-generation-2", "generation 2", PLAIN_ROLE, "Generation2"),
        (
            "3",
            "reads-generation-3",
            "generation 3",
            SERVICE_LINKED_ROLE,
            "Generation3",
        ),
    ];
    let mut cuts = vec![("1".to_string(), first)];
    for (version, name, description, role, connection) in generations {
        emulator
            .update_flow(
                case,
                &flow_id,
                flow_request(name, Some(description), role, connection),
            )
            .await;
        emulator.prepare(case, &flow_id).await;
        let cut = emulator
            .create_flow_version(case, &flow_id, json!({}))
            .await;
        assert_eq!(
            cut.string("version"),
            version,
            "case {case}: expected the next version to be numbered {version}, got {cut:?}"
        );
        cuts.push((version.to_string(), cut));
    }

    for (version, cut) in &cuts {
        let read = emulator.get_flow_version(&flow_id, version, None).await;
        assert_eq!(
            read.object(),
            cut.object(),
            "case {case} (version {version}): expected the version to keep the snapshot it was \
             cut from, got {read:?}"
        );
    }

    // And the DRAFT reports the latest generation, not any version's snapshot.
    let draft = emulator.get_flow(&flow_id).await;
    assert_eq!(
        draft.string("name"),
        "reads-generation-3",
        "case {case}: expected the DRAFT to report the latest change, got {draft:?}"
    );
}

/// The `ResourceNotFoundException` message a read answers for a resource it
/// could not resolve. The fixed text is this error class's; the operation name
/// and the resource type are its only variable parts.
fn not_found_message(resource_type: &str) -> String {
    format!(
        "Couldn't perform GetFlowVersion operation. Failed to retrieve resource type \
         {resource_type} because it doesn't exist. Check the resource ID and retry your request."
    )
}

/// Which addressed resource a row leaves unresolvable.
#[derive(Debug, Clone, Copy)]
enum Missing {
    /// The identifier names no stored flow.
    Flow,
    /// The flow is stored, but holds no version cut under the number asked for.
    Version,
}

/// Rows spanning the numbers a stored flow holds no version under, including the
/// published bounds of the modelled `NumericalVersion` (`^[0-9]{1,5}$`): the
/// lowest value the pattern admits, and the widest.
const UNRESOLVABLE_CASES: &[(&str, Missing, &str)] = &[
    ("a flow nothing created", Missing::Flow, "1"),
    (
        "a version number above the one version cut",
        Missing::Version,
        "2",
    ),
    (
        "version 0, which numbering starting from 1 never reaches",
        Missing::Version,
        "0",
    ),
    (
        "the widest NumericalVersion, which this flow was never cut at",
        Missing::Version,
        "99999",
    ),
];

/// The rule's refusal half: a read that resolves to no stored version is the
/// modelled `ResourceNotFoundException` (404), and it names which of the two
/// addressed resources is missing.
#[tokio::test]
async fn a_read_of_a_version_nothing_cut_is_refused_as_a_missing_resource() {
    let emulator = Emulator::new();
    // A flow with exactly one version cut, so every row below addresses a real
    // flow except the row that deliberately does not.
    let (flow_id, _) = emulator
        .flow_with_one_version(
            "the refusal fixture",
            flow_request(
                "reads-refusals",
                None,
                SERVICE_LINKED_ROLE,
                "InputToOutput",
            ),
        )
        .await;
    // Shaped as the modelled `FlowId` (`^[0-9a-zA-Z]{10}$`) but minted by
    // nothing, so it resolves to no stored flow.
    let unknown_flow = "Zz0123456z";
    assert_ne!(
        unknown_flow, flow_id,
        "the fixture must not collide with the id nothing minted"
    );

    for (case, missing, version) in UNRESOLVABLE_CASES {
        let (identifier, resource_type) = match missing {
            Missing::Flow => (unknown_flow, "Flow"),
            Missing::Version => (flow_id.as_str(), "FlowVersion"),
        };
        let refused = emulator.get_flow_version(identifier, version, None).await;
        assert_eq!(
            refused.status, 404,
            "case {case}: expected GetFlowVersion -> 404, got {refused:?}"
        );
        assert_eq!(
            refused.error_type.as_deref(),
            Some("ResourceNotFoundException"),
            "case {case}: expected ResourceNotFoundException, got {refused:?}"
        );
        assert_eq!(
            refused.string("message"),
            not_found_message(resource_type),
            "case {case}: expected the refusal to name the {resource_type} it could not \
             resolve, got {refused:?}"
        );
    }

    // The version the flow DOES hold is still served, so the refusals above are
    // about what was addressed and not about the operation.
    let served = emulator.get_flow_version(&flow_id, "1", None).await;
    assert_eq!(
        served.status, 200,
        "case \"the version the flow holds\": expected GetFlowVersion -> 200, got {served:?}"
    );
}

/// A read whose path value satisfies no modelled constraint is a BAD REQUEST —
/// the modelled `ValidationException` (400) — not a missing resource: the
/// value names no version of any flow, so there is nothing to look for.
///
/// The rows span the version one past the widest `NumericalVersion`
/// (`^[0-9]{1,5}$`), the DRAFT token — which is no `NumericalVersion` at all,
/// however many versions a flow holds under it — and an identifier of neither
/// `FlowIdentifier` form.
#[tokio::test]
async fn a_read_addressed_outside_the_modelled_constraints_is_a_bad_request() {
    let emulator = Emulator::new();
    let (flow_id, _) = emulator
        .flow_with_one_version(
            "the bad-request fixture",
            flow_request(
                "reads-badrequests",
                None,
                SERVICE_LINKED_ROLE,
                "InputToOutput",
            ),
        )
        .await;

    let cases: &[(&str, &str, &str)] = &[
        (
            "a version one digit past the widest NumericalVersion",
            &flow_id,
            "100000",
        ),
        (
            "the DRAFT token, which is no NumericalVersion",
            &flow_id,
            "DRAFT",
        ),
        (
            "an identifier of neither FlowIdentifier form",
            "not-a-flow-identifier",
            "1",
        ),
    ];

    for (case, identifier, version) in cases {
        let refused = emulator.get_flow_version(identifier, version, None).await;
        assert_eq!(
            refused.status, 400,
            "case {case}: expected GetFlowVersion -> 400, got {refused:?}"
        );
        assert_eq!(
            refused.error_type.as_deref(),
            Some("ValidationException"),
            "case {case}: expected ValidationException, got {refused:?}"
        );
    }
}
