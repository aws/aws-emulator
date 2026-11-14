// SPDX-License-Identifier: CC0-1.0

//! `UpdateFlowAlias`, end to end over HTTP and read back through
//! `GetFlowAlias` / `ListFlowAliases`.
//!
//! The rule: an update REPLACES the mutable members of the alias the request
//! addresses — so a member the request leaves out is left out of the stored
//! alias too — while the alias keeps its identity (`id`, `arn`, the flow it
//! belongs to) and its `createdAt`, and reports the instant of the change as
//! `updatedAt`. The alias is resolved against the flow it belongs to, and every
//! version the new routing configuration names must be one that flow holds; a
//! reference the flow cannot resolve is refused and nothing is written.
//!
//! The tables below span what one request can change (each mutable member on
//! its own, at the bounds the model publishes for it) and how the alias can be
//! addressed (each modelled identifier form, an alias of another flow, an
//! unknown flow, and identifiers of neither modelled form).
//!
//! Every assertion here goes through the router, so it exercises the real wire
//! path (route matching, model constraint validation, serialization) rather than
//! calling a handler directly.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
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

/// A role ARN matching the modelled `FlowExecutionRoleArn` pattern
/// `^arn:aws(-[^:]+)?:iam::([0-9]{12})?:role/(service-role/)?.+$`.
const EXECUTION_ROLE: &str =
    "arn:aws:iam::000000000000:role/service-role/AmazonBedrockExecutionRoleForFlows";

/// The members of an alias an `UpdateFlowAlias` request carries, and so the ones
/// it replaces. Every other member of the response is the stored alias's own.
const MUTABLE_MEMBERS: &[&str] = &[
    "name",
    "description",
    "routingConfiguration",
    "concurrencyConfiguration",
];

/// How many numbered versions the flow every row below runs against holds, so a
/// routing row can re-point an alias at a version other than the one it was
/// created with.
const VERSIONS_CUT: u32 = 2;

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

    fn summaries(&self) -> Vec<Value> {
        self.member(&["flowAliasSummaries"])
            .as_array()
            .cloned()
            .unwrap_or_else(|| panic!("flowAliasSummaries is not an array in {:?}", self.body))
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

    fn escape(identifier: &str) -> String {
        identifier.replace(':', "%3A").replace('/', "%2F")
    }

    /// A flow prepared and cut into `versions` numbered versions: the state an
    /// alias is created and updated against. Answers the `CreateFlow` response.
    async fn flow_with_versions(&self, case: &str, name: &str, versions: u32) -> Answer {
        let created = self
            .call(
                "POST",
                "/flows/",
                Some(json!({
                    "name": name,
                    "executionRoleArn": EXECUTION_ROLE,
                    "definition": passthrough_definition(),
                })),
            )
            .await;
        assert_eq!(
            created.status, 201,
            "case {case}: expected CreateFlow -> 201, got {created:?}"
        );
        let flow_id = created.string(&["id"]);
        self.prepare_and_settle(case, &flow_id).await;
        for expected in 1..=versions {
            let cut = self
                .call("POST", &format!("/flows/{flow_id}/versions"), Some(json!({})))
                .await;
            assert_eq!(
                cut.status, 201,
                "case {case}: expected CreateFlowVersion -> 201, got {cut:?}"
            );
            assert_eq!(
                cut.string(&["version"]),
                expected.to_string(),
                "case {case}: expected version {expected}, got {cut:?}"
            );
        }
        created
    }

    /// `PrepareFlow`, with the outcome the clock owes moved into the past so the
    /// flow stands at `Prepared` without the test sleeping for the span.
    async fn prepare_and_settle(&self, case: &str, flow_id: &str) {
        let prepared = self
            .call("POST", &format!("/flows/{flow_id}/"), Some(json!({})))
            .await;
        assert_eq!(
            prepared.status, 202,
            "case {case}: expected PrepareFlow -> 202, got {prepared:?}"
        );
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

    async fn create_flow_alias(&self, identifier: &str, request: Value) -> Answer {
        let escaped = Self::escape(identifier);
        self.call("POST", &format!("/flows/{escaped}/aliases"), Some(request))
            .await
    }

    async fn update_flow_alias(
        &self,
        flow_identifier: &str,
        alias_identifier: &str,
        request: Value,
    ) -> Answer {
        let flow = Self::escape(flow_identifier);
        let alias = Self::escape(alias_identifier);
        self.call(
            "PUT",
            &format!("/flows/{flow}/aliases/{alias}"),
            Some(request),
        )
        .await
    }

    async fn get_flow_alias(&self, flow_identifier: &str, alias_identifier: &str) -> Answer {
        let flow = Self::escape(flow_identifier);
        let alias = Self::escape(alias_identifier);
        self.call("GET", &format!("/flows/{flow}/aliases/{alias}"), None)
            .await
    }

    async fn list_flow_aliases(&self, identifier: &str) -> Answer {
        let escaped = Self::escape(identifier);
        self.call("GET", &format!("/flows/{escaped}/aliases"), None)
            .await
    }

    /// A flow holding [`VERSIONS_CUT`] versions and one alias, created with
    /// every mutable member set: the state an update row runs against. Answers
    /// the `CreateFlow` and `CreateFlowAlias` responses.
    async fn flow_with_an_alias(&self, case: &str) -> (Answer, Answer) {
        let flow = self
            .flow_with_versions(case, "alias-update-flow", VERSIONS_CUT)
            .await;
        let flow_id = flow.string(&["id"]);
        let created = self
            .create_flow_alias(&flow_id, baseline_alias())
            .await;
        assert_eq!(
            created.status, 201,
            "case {case}: expected CreateFlowAlias -> 201, got {created:?}"
        );
        (flow, created)
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

/// The alias every update row starts from: every mutable member set, so a row
/// that leaves one out is a row that removes it.
fn baseline_alias() -> Value {
    json!({
        "name": "base-alias",
        "description": "the alias before the update",
        "routingConfiguration": [{ "flowVersion": "DRAFT" }],
        "concurrencyConfiguration": { "type": "Automatic", "maxConcurrency": 4 },
    })
}

/// The refusal an operation answers when a resource it was asked to resolve does
/// not exist. The fixed text is this error class's; the operation name and the
/// resource type are its only variable parts.
fn not_found_message(operation: &str, resource_type: &str) -> String {
    format!(
        "Couldn't perform {operation} operation. Failed to retrieve resource type \
         {resource_type} because it doesn't exist. Check the resource ID and retry \
         your request."
    )
}

/// A timestamp member of an answer, parsed out of the modelled `date-time` wire
/// form — which is a STRING, never the epoch number the restJson1 default would
/// write — as the instant it names, so two of them can be ordered.
fn instant(answer: &Answer, member: &str) -> (i64, u32) {
    let raw = answer.string(&[member]);
    let at = DateTime::from_str(&raw, Format::DateTime).unwrap_or_else(|e| {
        panic!("{member} {raw:?} is not in the modelled date-time wire form ({e})")
    });
    (at.secs(), at.subsec_nanos())
}

/// The members of `request` that the stored alias should carry after it, laid
/// over the alias as it stood: a member the request names is replaced, a member
/// it leaves out is gone.
fn expected_after(stored: &Value, request: &Value) -> Map<String, Value> {
    let mut expected = stored
        .as_object()
        .cloned()
        .unwrap_or_else(|| panic!("the stored alias is not an object: {stored:?}"));
    for member in MUTABLE_MEMBERS {
        match request.get(*member) {
            Some(value) => {
                expected.insert((*member).to_string(), value.clone());
            }
            None => {
                expected.remove(*member);
            }
        }
    }
    expected
}

/// Rows spanning what one `UpdateFlowAlias` request can change about the alias
/// in [`baseline_alias`], over a flow holding [`VERSIONS_CUT`] numbered
/// versions.
///
/// Each row differs from the one before along a different member, and the
/// bounds are the ones the model publishes: `Description` is `length 1..200`,
/// `FlowAliasConcurrencyConfiguration$type` is the `ConcurrencyType` enum
/// (`Automatic`, `Manual`) with `maxConcurrency` `range 1..100`, and
/// `FlowAliasRoutingConfigurationListItem$flowVersion` is
/// `pattern ^(DRAFT|[0-9]{0,4}[1-9][0-9]{0,4})$`.
fn update_rows() -> Vec<(&'static str, Value)> {
    vec![
        (
            "the name alone changes",
            json!({
                "name": "renamed-alias",
                "description": "the alias before the update",
                "routingConfiguration": [{ "flowVersion": "DRAFT" }],
                "concurrencyConfiguration": { "type": "Automatic", "maxConcurrency": 4 },
            }),
        ),
        (
            "the routing is re-pointed at the first version cut",
            json!({
                "name": "base-alias",
                "description": "the alias before the update",
                "routingConfiguration": [{ "flowVersion": "1" }],
                "concurrencyConfiguration": { "type": "Automatic", "maxConcurrency": 4 },
            }),
        ),
        (
            "the routing is re-pointed at the latest version cut",
            json!({
                "name": "base-alias",
                "description": "the alias before the update",
                "routingConfiguration": [{ "flowVersion": "2" }],
                "concurrencyConfiguration": { "type": "Automatic", "maxConcurrency": 4 },
            }),
        ),
        (
            "the description is replaced by the shortest one there is",
            json!({
                "name": "base-alias",
                "description": "a",
                "routingConfiguration": [{ "flowVersion": "DRAFT" }],
                "concurrencyConfiguration": { "type": "Automatic", "maxConcurrency": 4 },
            }),
        ),
        (
            "the description is replaced by the longest one there is",
            json!({
                "name": "base-alias",
                "description": "d".repeat(200),
                "routingConfiguration": [{ "flowVersion": "DRAFT" }],
                "concurrencyConfiguration": { "type": "Automatic", "maxConcurrency": 4 },
            }),
        ),
        (
            "the description is left out, so the alias carries none",
            json!({
                "name": "base-alias",
                "routingConfiguration": [{ "flowVersion": "DRAFT" }],
                "concurrencyConfiguration": { "type": "Automatic", "maxConcurrency": 4 },
            }),
        ),
        (
            "the concurrency becomes manual at the smallest maximum",
            json!({
                "name": "base-alias",
                "description": "the alias before the update",
                "routingConfiguration": [{ "flowVersion": "DRAFT" }],
                "concurrencyConfiguration": { "type": "Manual", "maxConcurrency": 1 },
            }),
        ),
        (
            "the concurrency becomes manual at the largest maximum",
            json!({
                "name": "base-alias",
                "description": "the alias before the update",
                "routingConfiguration": [{ "flowVersion": "DRAFT" }],
                "concurrencyConfiguration": { "type": "Manual", "maxConcurrency": 100 },
            }),
        ),
        (
            "the concurrency names no maximum",
            json!({
                "name": "base-alias",
                "description": "the alias before the update",
                "routingConfiguration": [{ "flowVersion": "DRAFT" }],
                "concurrencyConfiguration": { "type": "Automatic" },
            }),
        ),
        (
            "the concurrency configuration is left out, so the alias carries none",
            json!({
                "name": "base-alias",
                "description": "the alias before the update",
                "routingConfiguration": [{ "flowVersion": "DRAFT" }],
            }),
        ),
        (
            "every mutable member changes at once",
            json!({
                "name": "promoted-alias",
                "description": "the alias after the update",
                "routingConfiguration": [{ "flowVersion": "2" }],
                "concurrencyConfiguration": { "type": "Manual", "maxConcurrency": 7 },
            }),
        ),
    ]
}

/// The rule: `UpdateFlowAlias` replaces the alias's mutable members with the
/// ones the request carries — leaving out the ones it does not name — keeps the
/// alias's identity and `createdAt`, and reports the change through every reader
/// of the stored alias.
#[tokio::test]
async fn update_flow_alias_replaces_the_mutable_members_and_keeps_the_identity() {
    for (case, request) in update_rows() {
        let emulator = Emulator::new();
        let (flow, created) = emulator.flow_with_an_alias(case).await;
        let flow_id = flow.string(&["id"]);
        let alias_id = created.string(&["id"]);

        let updated = emulator
            .update_flow_alias(&flow_id, &alias_id, request.clone())
            .await;
        assert_eq!(
            updated.status, 200,
            "case {case}: expected UpdateFlowAlias -> 200, got {updated:?}"
        );

        // The identity members and `createdAt` are not the request's to set.
        for member in ["flowId", "id", "arn", "createdAt"] {
            assert_eq!(
                updated.string(&[member]),
                created.string(&[member]),
                "case {case}: {member} is the stored alias's own, got {updated:?}"
            );
        }
        // The alias changed, so it reports an `updatedAt` no earlier than the
        // instant it was created, both in the modelled `date-time` wire form.
        let updated_at = updated.string(&["updatedAt"]);
        assert!(
            instant(&updated, "updatedAt") >= instant(&created, "createdAt"),
            "case {case}: expected updatedAt at or after createdAt, got {updated:?}"
        );

        // Every mutable member is the request's, and one the request left out is
        // reported by nothing.
        let mut expected = expected_after(&created.body, &request);
        expected.insert("updatedAt".to_string(), json!(updated_at));
        assert_eq!(
            updated.body,
            Value::Object(expected),
            "case {case}: the update replaces exactly the members it carries, \
             got {updated:?}"
        );

        // The round trip: the change is the stored alias's, so every reader
        // reports it.
        let read = emulator.get_flow_alias(&flow_id, &alias_id).await;
        assert_eq!(
            read.status, 200,
            "case {case}: expected GetFlowAlias -> 200, got {read:?}"
        );
        assert_eq!(
            read.body, updated.body,
            "case {case}: the stored alias is reported as the update left it, got {read:?}"
        );

        let listed = emulator.list_flow_aliases(&flow_id).await;
        assert_eq!(
            listed.status, 200,
            "case {case}: expected ListFlowAliases -> 200, got {listed:?}"
        );
        assert_eq!(
            listed.summaries(),
            vec![updated.body.clone()],
            "case {case}: the flow enumerates the one alias as the update left it, \
             got {listed:?}"
        );
    }
}

/// Rows spanning the routing targets an update can name over a flow holding
/// [`VERSIONS_CUT`] numbered versions: the numbers cut and the DRAFT version
/// every stored flow has resolve, and numbers past the ones cut — up to `99999`,
/// the widest the modelled `flowVersion` pattern admits — resolve to no version
/// of this flow.
const ROUTING_ROWS: &[(&str, &str, bool)] = &[
    ("the first version cut", "1", true),
    ("the latest version cut", "2", true),
    ("the DRAFT version every flow holds", "DRAFT", true),
    ("the next number, not cut yet", "3", false),
    ("a number far past the versions cut", "999", false),
    ("the widest version number there is", "99999", false),
];

/// The rule: an alias routes to a version of the flow it belongs to, so an
/// update naming a version that flow does not hold is refused 404
/// `ResourceNotFoundException` for the flow VERSION, and the stored alias keeps
/// the state it had.
#[tokio::test]
async fn update_flow_alias_resolves_its_routing_target_against_the_flows_versions() {
    for (case, flow_version, resolvable) in ROUTING_ROWS {
        let emulator = Emulator::new();
        let (flow, created) = emulator.flow_with_an_alias(case).await;
        let flow_id = flow.string(&["id"]);
        let alias_id = created.string(&["id"]);

        let request = json!({
            "name": "re-pointed-alias",
            "routingConfiguration": [{ "flowVersion": flow_version }],
        });
        let updated = emulator
            .update_flow_alias(&flow_id, &alias_id, request.clone())
            .await;

        if !*resolvable {
            assert_eq!(
                updated.status, 404,
                "case {case} ({flow_version}): expected UpdateFlowAlias -> 404, \
                 got {updated:?}"
            );
            assert_eq!(
                updated.error_type.as_deref(),
                Some("ResourceNotFoundException"),
                "case {case} ({flow_version}): expected ResourceNotFoundException, \
                 got {updated:?}"
            );
            assert_eq!(
                updated.string(&["message"]),
                not_found_message("UpdateFlowAlias", "FlowVersion"),
                "case {case} ({flow_version}): expected the refusal to name the \
                 operation and the resource type it could not resolve, got {updated:?}"
            );
            let read = emulator.get_flow_alias(&flow_id, &alias_id).await;
            assert_eq!(
                read.body, created.body,
                "case {case} ({flow_version}): the refused update wrote nothing, \
                 got {read:?}"
            );
            continue;
        }

        assert_eq!(
            updated.status, 200,
            "case {case} ({flow_version}): expected UpdateFlowAlias -> 200, got {updated:?}"
        );
        assert_eq!(
            updated.member(&["routingConfiguration"]),
            &json!([{ "flowVersion": flow_version }]),
            "case {case} ({flow_version}): the alias reports the routing it was \
             updated to, got {updated:?}"
        );
        let read = emulator.get_flow_alias(&flow_id, &alias_id).await;
        assert_eq!(
            read.body, updated.body,
            "case {case} ({flow_version}): the re-pointed alias is stored, got {read:?}"
        );
    }
}

/// The rule: the alias an update addresses is the one the `(flowIdentifier,
/// aliasIdentifier)` pair names on the addressed flow, in either modelled form
/// of each; a pair naming no stored alias is a missing resource, and an
/// identifier of neither modelled form is a bad request. Nothing is written for
/// either refusal.
#[tokio::test]
async fn update_flow_alias_resolves_the_alias_the_request_addresses() {
    let case = "two flows, each with its own alias";
    let emulator = Emulator::new();

    let first = emulator
        .flow_with_versions(case, "alias-update-owner", VERSIONS_CUT)
        .await;
    let first_id = first.string(&["id"]);
    let first_arn = first.string(&["arn"]);
    let created = emulator.create_flow_alias(&first_id, baseline_alias()).await;
    assert_eq!(
        created.status, 201,
        "case {case}: expected CreateFlowAlias -> 201, got {created:?}"
    );
    let alias_id = created.string(&["id"]);
    let alias_arn = created.string(&["arn"]);

    let second = emulator
        .flow_with_versions(case, "alias-update-other", VERSIONS_CUT)
        .await;
    let second_id = second.string(&["id"]);
    let elsewhere = emulator
        .create_flow_alias(&second_id, baseline_alias())
        .await;
    assert_eq!(
        elsewhere.status, 201,
        "case {case}: expected CreateFlowAlias -> 201, got {elsewhere:?}"
    );
    let elsewhere_arn = elsewhere.string(&["arn"]);

    // Rows: every modelled identifier form of each member names the one alias
    // the first flow holds, and each row's name is what the alias carries after
    // it.
    for (form, flow_identifier, alias_identifier) in [
        ("bare ids", first_id.clone(), alias_id.clone()),
        ("a flow ARN", first_arn.clone(), alias_id.clone()),
        ("an alias ARN", first_id.clone(), alias_arn.clone()),
        ("both ARNs", first_arn.clone(), alias_arn.clone()),
    ] {
        let renamed = format!("addressed-by-{}", form.replace(' ', "-"));
        let updated = emulator
            .update_flow_alias(
                &flow_identifier,
                &alias_identifier,
                json!({
                    "name": renamed,
                    "routingConfiguration": [{ "flowVersion": "1" }],
                }),
            )
            .await;
        assert_eq!(
            updated.status, 200,
            "case {case} ({form}): expected UpdateFlowAlias -> 200, got {updated:?}"
        );
        assert_eq!(
            updated.string(&["id"]),
            alias_id,
            "case {case} ({form}): every identifier form names the same alias, \
             got {updated:?}"
        );
        let read = emulator.get_flow_alias(&first_id, &alias_id).await;
        assert_eq!(
            read.string(&["name"]),
            renamed,
            "case {case} ({form}): the update through this form is the stored \
             alias's, got {read:?}"
        );
    }

    // Rows: pairs naming no alias of the addressed flow. The missing resource is
    // the flow when the flow is unknown, and the alias otherwise — including
    // when the alias ARN names another flow's alias, and when the identifier is
    // the test-alias token no create has used.
    let unknown_flow = "0123456789";
    for (form, flow_identifier, alias_identifier, resource) in [
        ("an unknown flow", unknown_flow, alias_id.as_str(), "Flow"),
        (
            "an alias id nothing created",
            first_id.as_str(),
            "0123456789",
            "FlowAlias",
        ),
        (
            "the test alias no create has used",
            first_id.as_str(),
            "TSTALIASID",
            "FlowAlias",
        ),
        (
            "another flow's alias id",
            second_id.as_str(),
            alias_id.as_str(),
            "FlowAlias",
        ),
        (
            "another flow's alias ARN",
            first_id.as_str(),
            elsewhere_arn.as_str(),
            "FlowAlias",
        ),
    ] {
        let refused = emulator
            .update_flow_alias(
                flow_identifier,
                alias_identifier,
                json!({
                    "name": "unreachable-alias",
                    "routingConfiguration": [{ "flowVersion": "1" }],
                }),
            )
            .await;
        assert_eq!(
            refused.status, 404,
            "case {case} ({form}): expected UpdateFlowAlias -> 404, got {refused:?}"
        );
        assert_eq!(
            refused.error_type.as_deref(),
            Some("ResourceNotFoundException"),
            "case {case} ({form}): expected ResourceNotFoundException, got {refused:?}"
        );
        assert_eq!(
            refused.string(&["message"]),
            not_found_message("UpdateFlowAlias", resource),
            "case {case} ({form}): the refusal names the resource type it could not \
             resolve, got {refused:?}"
        );
    }

    // Rows: identifiers of neither modelled form are bad requests, not missing
    // resources.
    for (form, flow_identifier, alias_identifier) in [
        (
            "a flow name as the flow identifier",
            "alias-update-owner",
            alias_id.as_str(),
        ),
        (
            "a flow id one character short",
            "012345678",
            alias_id.as_str(),
        ),
        (
            "an alias name as the alias identifier",
            first_id.as_str(),
            "base-alias",
        ),
        ("an alias id one character long", first_id.as_str(), "a"),
    ] {
        let refused = emulator
            .update_flow_alias(
                flow_identifier,
                alias_identifier,
                json!({
                    "name": "unreachable-alias",
                    "routingConfiguration": [{ "flowVersion": "1" }],
                }),
            )
            .await;
        assert_eq!(
            refused.status, 400,
            "case {case} ({form}): expected UpdateFlowAlias -> 400, got {refused:?}"
        );
        assert_eq!(
            refused.error_type.as_deref(),
            Some("ValidationException"),
            "case {case} ({form}): expected ValidationException, got {refused:?}"
        );
    }

    // Every refusal above wrote nothing: each flow still holds exactly one
    // alias, the first one renamed by the addressing rows alone.
    for (flow, identifier, expected_name) in [
        (
            "the first flow",
            first_id.as_str(),
            "addressed-by-both-ARNs",
        ),
        ("the second flow", second_id.as_str(), "base-alias"),
    ] {
        let listed = emulator.list_flow_aliases(identifier).await;
        let summaries = listed.summaries();
        assert_eq!(
            summaries.len(),
            1,
            "case {case} ({flow}): expected exactly one alias, got {summaries:?}"
        );
        assert_eq!(
            summaries[0]["name"].as_str(),
            Some(expected_name),
            "case {case} ({flow}): no refusal changed the stored alias, got {summaries:?}"
        );
    }
}
