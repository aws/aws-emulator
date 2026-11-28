// SPDX-License-Identifier: CC0-1.0

//! `ValidateFlowDefinition`, end to end over HTTP: the operation analyses the
//! definition the request carries and reports one finding per defect — the
//! modelled `FlowValidationType`, its severity and the `FlowValidationDetails`
//! member that type carries — and an EMPTY list for a definition with no defect.
//!
//! The table below spans the defect classes rather than one recorded definition:
//! rows differ in the kind of defect (a reference that resolves to nothing, a
//! node that is not configured for its type, a property of the whole graph), in
//! the node and connection COUNT, in the node types involved, and in the length
//! of the cycle a row forms. Every assertion goes through the router, so it
//! exercises the real wire path.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_bedrockagent_amazonbedrockagentbuildtimelambda::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 `Authorization` header scoped to the `bedrock` signing name: the
/// routing key this service claims on.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/bedrock/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0123456789abcdef";

/// A role ARN matching the modelled `FlowExecutionRoleArn` pattern, for the
/// `CreateFlow` that proves this operation touches no stored flow.
const EXECUTION_ROLE: &str =
    "arn:aws:iam::000000000000:role/service-role/AmazonBedrockExecutionRoleForFlows";

/// One HTTP answer from the emulator, parsed.
///
/// `pub(crate)`, with the seams a sibling suite reads it through, so the suite that
/// asserts WHICH ORDER an enumerated type's value set is listed in
/// (`tests/all/enum_value_set_order.rs`) reads a refusal exactly the way this one does
/// instead of growing a second copy of the harness.
#[derive(Debug)]
pub(crate) struct Answer {
    pub(crate) status: u16,
    /// The restJson1 error shape name, from `x-amzn-errortype` or the body's
    /// `__type`; `None` on a success.
    pub(crate) error_type: Option<String>,
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

    /// The `message` a refusal carries.
    pub(crate) fn message(&self, case: &str) -> String {
        self.member(&["message"])
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: the refusal carries no message: {:?}", self.body))
            .to_string()
    }
}

/// The emulator under test: one router over one shared engine, exactly as the
/// production router mounts this service.
pub(crate) struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    pub(crate) fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    pub(crate) async fn call(&self, method: &str, uri: &str, body: Option<Value>) -> Answer {
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

    async fn validate(&self, definition: Value) -> Answer {
        self.call(
            "POST",
            "/flows/validate-definition",
            Some(json!({ "definition": definition })),
        )
        .await
    }
}

/// One reported finding, reduced to what the rule is about: the modelled
/// validation type, its severity and the `details` union member it carries.
type Finding = (String, String, String);

/// Every finding in `answer`, sorted, with the shape of each one checked: a
/// non-empty message, a severity the model admits, and exactly one `details`
/// member.
fn findings(case: &str, answer: &Answer) -> Vec<Finding> {
    assert_eq!(
        answer.status, 200,
        "case {case}: expected ValidateFlowDefinition -> 200, got {answer:?}"
    );
    let reported = answer
        .member(&["validations"])
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case {case}: expected a validations array, got {:?}",
                answer.body
            )
        });
    let mut collected: Vec<Finding> = reported
        .iter()
        .map(|validation| {
            let validation_type = validation["type"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case}: a finding carries no type: {validation:?}"))
                .to_string();
            let severity = validation["severity"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case}: a finding carries no severity: {validation:?}")
                })
                .to_string();
            assert!(
                validation["message"].as_str().is_some_and(|m| !m.is_empty()),
                "case {case}: every finding carries a message, got {validation:?}"
            );
            let details = validation["details"].as_object().unwrap_or_else(|| {
                panic!("case {case}: a finding carries no details: {validation:?}")
            });
            assert_eq!(
                details.len(),
                1,
                "case {case}: details is a union, so exactly one member is set, got {validation:?}"
            );
            let member = details
                .keys()
                .next()
                .expect("a one-member object has a key")
                .clone();
            (validation_type, severity, member)
        })
        .collect();
    assert!(
        collected.len() <= 100,
        "case {case}: validations is modelled length ..100, got {} findings",
        collected.len()
    );
    collected.sort();
    collected
}

/// The expected findings of a row, in the same reduced form.
fn expected(rows: &[(&str, &str, &str)]) -> Vec<Finding> {
    let mut expected: Vec<Finding> = rows
        .iter()
        .map(|(validation_type, severity, member)| {
            (
                (*validation_type).to_string(),
                (*severity).to_string(),
                (*member).to_string(),
            )
        })
        .collect();
    expected.sort();
    expected
}

/// The flow's starting node, declaring one `String` output.
fn input_node() -> Value {
    json!({
        "name": "FlowInput",
        "type": "Input",
        "configuration": { "input": {} },
        "outputs": [{ "name": "document", "type": "String" }],
    })
}

/// The flow's ending node, declaring one `String` input.
fn output_node() -> Value {
    json!({
        "name": "FlowOutput",
        "type": "Output",
        "configuration": { "output": {} },
        "inputs": [{ "name": "document", "type": "String", "expression": "$.data" }],
    })
}

/// The data connection carrying [`input_node`]'s output into [`output_node`]'s
/// input.
fn passthrough_connection() -> Value {
    json!({
        "name": "InputToOutput",
        "type": "Data",
        "source": "FlowInput",
        "target": "FlowOutput",
        "configuration": { "data": { "sourceOutput": "document", "targetInput": "document" } },
    })
}

/// An `Input` node wired straight to an `Output` node: the smallest definition
/// with no defect at all.
fn passthrough() -> Value {
    json!({
        "nodes": [input_node(), output_node()],
        "connections": [passthrough_connection()],
    })
}

/// A definition whose nodes form a cycle of `length` intermediate nodes, each of
/// a node type the modelled `FlowNodeConfiguration` union configures with an
/// empty structure. The last node of the ring connects back to the first through
/// its second input, so no node input takes more than one connection and the only
/// defect is the cycle. The connection that closes the ring is returned with it.
fn ring(length: usize) -> (Value, String) {
    // Node names matching the modelled `FlowNodeName` pattern
    // `^[a-zA-Z]([_]?[0-9a-zA-Z]){1,50}$`.
    let names = ["Alpha", "Beta", "Gamma", "Delta"];
    let ring: Vec<&str> = names[..length].to_vec();

    let mut nodes = vec![json!({
        "name": "FlowInput",
        "type": "Input",
        "configuration": { "input": {} },
        "outputs": [{ "name": "document", "type": "String" }],
    })];
    for (at, name) in ring.iter().enumerate() {
        // The ring's first node takes the flow input on `seed` and the value
        // coming back round the ring on `again`; the others take one input.
        let inputs = if at == 0 {
            json!([
                { "name": "seed", "type": "String", "expression": "$.data" },
                { "name": "again", "type": "String", "expression": "$.data" },
            ])
        } else {
            json!([{ "name": "carried", "type": "String", "expression": "$.data" }])
        };
        nodes.push(json!({
            "name": name,
            "type": "Collector",
            "configuration": { "collector": {} },
            "inputs": inputs,
            "outputs": [{ "name": "carried", "type": "String" }],
        }));
    }
    nodes.push(json!({
        "name": "FlowOutput",
        "type": "Output",
        "configuration": { "output": {} },
        "inputs": [{ "name": "document", "type": "String", "expression": "$.data" }],
    }));

    let mut connections = vec![json!({
        "name": "InputToRing",
        "type": "Data",
        "source": "FlowInput",
        "target": ring[0],
        "configuration": { "data": { "sourceOutput": "document", "targetInput": "seed" } },
    })];
    for pair in ring.windows(2) {
        connections.push(json!({
            "name": format!("{}To{}", pair[0], pair[1]),
            "type": "Data",
            "source": pair[0],
            "target": pair[1],
            "configuration": { "data": { "sourceOutput": "carried", "targetInput": "carried" } },
        }));
    }
    let closing = format!("{}To{}", ring[ring.len() - 1], ring[0]);
    connections.push(json!({
        "name": closing,
        "type": "Data",
        "source": ring[ring.len() - 1],
        "target": ring[0],
        "configuration": { "data": { "sourceOutput": "carried", "targetInput": "again" } },
    }));
    connections.push(json!({
        "name": "RingToOutput",
        "type": "Data",
        "source": ring[0],
        "target": "FlowOutput",
        "configuration": { "data": { "sourceOutput": "carried", "targetInput": "document" } },
    }));

    (
        json!({ "nodes": nodes, "connections": connections }),
        closing,
    )
}

/// A flow that routes its input through a condition node, with `conditions` as
/// the condition node's conditions and `taken` as the condition the one outgoing
/// conditional connection is taken on.
///
/// The ending node takes its value from a DATA connection of its own: a
/// conditional connection names no target input, so it carries no value into one
/// (`UnfulfilledNodeInputFlowValidationDetails$input` is documented "An input is
/// unfulfilled if there are no data connections to it").
fn conditional(conditions: Value, taken: &str) -> Value {
    json!({
        "nodes": [
            {
                "name": "FlowInput",
                "type": "Input",
                "configuration": { "input": {} },
                "outputs": [{ "name": "document", "type": "String" }],
            },
            {
                "name": "Check",
                "type": "Condition",
                "configuration": { "condition": { "conditions": conditions } },
                "inputs": [{ "name": "answer", "type": "String", "expression": "$.data" }],
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
                "name": "InputToCheck",
                "type": "Data",
                "source": "FlowInput",
                "target": "Check",
                "configuration": {
                    "data": { "sourceOutput": "document", "targetInput": "answer" },
                },
            },
            {
                "name": "CheckToOutput",
                "type": "Conditional",
                "source": "Check",
                "target": "FlowOutput",
                "configuration": { "conditional": { "condition": taken } },
            },
            {
                "name": "InputToOutput",
                "type": "Data",
                "source": "FlowInput",
                "target": "FlowOutput",
                "configuration": {
                    "data": { "sourceOutput": "document", "targetInput": "document" },
                },
            },
        ],
    })
}

/// A flow whose middle node is a `Loop` node running `body` as its loop
/// definition.
fn with_loop_body(body: Value) -> Value {
    json!({
        "nodes": [
            {
                "name": "FlowInput",
                "type": "Input",
                "configuration": { "input": {} },
                "outputs": [{ "name": "document", "type": "String" }],
            },
            {
                "name": "Repeat",
                "type": "Loop",
                "configuration": { "loop": { "definition": { "nodes": body } } },
                "inputs": [{ "name": "seed", "type": "String", "expression": "$.data" }],
                "outputs": [{ "name": "result", "type": "String" }],
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
                "name": "InputToRepeat",
                "type": "Data",
                "source": "FlowInput",
                "target": "Repeat",
                "configuration": {
                    "data": { "sourceOutput": "document", "targetInput": "seed" },
                },
            },
            {
                "name": "RepeatToOutput",
                "type": "Data",
                "source": "Repeat",
                "target": "FlowOutput",
                "configuration": { "data": { "sourceOutput": "result", "targetInput": "document" } },
            },
        ],
    })
}

/// The `LoopInput` node a loop body is entered through.
fn loop_input_node(name: &str) -> Value {
    json!({ "name": name, "type": "LoopInput", "configuration": { "loopInput": {} } })
}

/// The `LoopController` node a loop body iterates under.
fn loop_controller_node(name: &str) -> Value {
    json!({
        "name": name,
        "type": "LoopController",
        "configuration": {
            "loopController": {
                "continueCondition": { "name": "keepGoing", "expression": "carried != \"done\"" },
                "maxIterations": 5,
            },
        },
    })
}

/// The rule, over a table spanning the defect classes: `ValidateFlowDefinition`
/// reports exactly one finding per defect of the submitted definition — the
/// modelled type, the severity and the typed details member — and nothing at all
/// for a definition with no defect.
#[tokio::test]
async fn validate_flow_definition_reports_one_finding_per_defect() {
    let emulator = Emulator::new();
    let (two_node_ring, _) = ring(2);
    let (three_node_ring, _) = ring(3);

    let sound_conditions = json!([
        { "name": "isYes", "expression": "answer == \"yes\"" },
        { "name": "default" },
    ]);

    let cases: Vec<(&str, Value, Vec<Finding>)> = vec![
        // No defect: the rule's other side.
        ("a passthrough definition", passthrough(), expected(&[])),
        (
            "a condition node with a default condition",
            conditional(sound_conditions.clone(), "isYes"),
            expected(&[]),
        ),
        (
            "a loop node with one LoopInput and one LoopController",
            with_loop_body(json!([
                loop_input_node("LoopStart"),
                loop_controller_node("Controller"),
            ])),
            expected(&[]),
        ),
        // References that resolve to nothing.
        (
            "a connection starting at an undeclared node",
            json!({
                "nodes": [input_node(), output_node()],
                "connections": [{
                    "name": "InputToOutput",
                    "type": "Data",
                    "source": "NotANode",
                    "target": "FlowOutput",
                    "configuration": {
                        "data": { "sourceOutput": "document", "targetInput": "document" },
                    },
                }],
            }),
            expected(&[
                (
                    "UnknownConnectionSource",
                    "Error",
                    "unknownConnectionSource",
                ),
                // The connection starts nowhere, so it lands nowhere either: the
                // ending node is left with no path to it.
                ("UnreachableNode", "Warning", "unreachableNode"),
            ]),
        ),
        (
            "a connection ending at an undeclared node",
            json!({
                "nodes": [input_node(), output_node()],
                "connections": [{
                    "name": "InputToOutput",
                    "type": "Data",
                    "source": "FlowInput",
                    "target": "NotANode",
                    "configuration": {
                        "data": { "sourceOutput": "document", "targetInput": "document" },
                    },
                }],
            }),
            expected(&[
                (
                    "UnknownConnectionTarget",
                    "Error",
                    "unknownConnectionTarget",
                ),
                // The findings that FOLLOW from the connection landing nowhere:
                // the ending node's input takes no value and nothing reaches it.
                ("UnfulfilledNodeInput", "Warning", "unfulfilledNodeInput"),
                ("UnreachableNode", "Warning", "unreachableNode"),
            ]),
        ),
        (
            "a connection starting at an output its source node doesn't declare",
            json!({
                "nodes": [input_node(), output_node()],
                "connections": [{
                    "name": "InputToOutput",
                    "type": "Data",
                    "source": "FlowInput",
                    "target": "FlowOutput",
                    "configuration": {
                        "data": { "sourceOutput": "elsewhere", "targetInput": "document" },
                    },
                }],
            }),
            expected(&[(
                "UnknownConnectionSourceOutput",
                "Error",
                "unknownConnectionSourceOutput",
            )]),
        ),
        (
            "a connection ending at an input its target node doesn't declare",
            json!({
                "nodes": [input_node(), output_node()],
                "connections": [{
                    "name": "InputToOutput",
                    "type": "Data",
                    "source": "FlowInput",
                    "target": "FlowOutput",
                    "configuration": {
                        "data": { "sourceOutput": "document", "targetInput": "elsewhere" },
                    },
                }],
            }),
            expected(&[
                (
                    "UnknownConnectionTargetInput",
                    "Error",
                    "unknownConnectionTargetInput",
                ),
                // The connection feeds an input the node does not declare, so the
                // input it DOES declare is fed by nothing.
                ("UnfulfilledNodeInput", "Warning", "unfulfilledNodeInput"),
            ]),
        ),
        (
            "a conditional connection taken on a condition its source doesn't declare",
            conditional(sound_conditions.clone(), "isNo"),
            expected(&[(
                "UnknownConnectionCondition",
                "Error",
                "unknownConnectionCondition",
            )]),
        ),
        // Configuration that is absent, or belongs to the other type.
        (
            "a connection with no configuration",
            json!({
                "nodes": [input_node(), output_node()],
                "connections": [{
                    "name": "InputToOutput",
                    "type": "Data",
                    "source": "FlowInput",
                    "target": "FlowOutput",
                }],
            }),
            expected(&[
                (
                    "MissingConnectionConfiguration",
                    "Error",
                    "missingConnectionConfiguration",
                ),
                // A connection carrying no data configuration names no target
                // input, so it feeds none.
                ("UnfulfilledNodeInput", "Warning", "unfulfilledNodeInput"),
            ]),
        ),
        (
            "a Data connection carrying a conditional configuration",
            json!({
                "nodes": [input_node(), output_node()],
                "connections": [{
                    "name": "InputToOutput",
                    "type": "Data",
                    "source": "FlowInput",
                    "target": "FlowOutput",
                    "configuration": { "conditional": { "condition": "isYes" } },
                }],
            }),
            expected(&[
                (
                    "MissingConnectionConfiguration",
                    "Error",
                    "missingConnectionConfiguration",
                ),
                // A connection carrying no data configuration names no target
                // input, so it feeds none.
                ("UnfulfilledNodeInput", "Warning", "unfulfilledNodeInput"),
            ]),
        ),
        (
            "a node of a type with an empty configuration, carrying none",
            json!({
                "nodes": [
                    {
                        "name": "FlowInput",
                        "type": "Input",
                        "outputs": [{ "name": "document", "type": "String" }],
                    },
                    {
                        "name": "FlowOutput",
                        "type": "Output",
                        "inputs": [
                            { "name": "document", "type": "String", "expression": "$.data" },
                        ],
                    },
                ],
                "connections": [passthrough_connection()],
            }),
            expected(&[]),
        ),
        (
            "a node of a type with a configuration, carrying none",
            json!({
                "nodes": [
                    input_node(),
                    {
                        "name": "Check",
                        "type": "Condition",
                        "inputs": [
                            { "name": "answer", "type": "String", "expression": "$.data" },
                        ],
                    },
                    output_node(),
                ],
                "connections": [
                    passthrough_connection(),
                    {
                        "name": "InputToCheck",
                        "type": "Data",
                        "source": "FlowInput",
                        "target": "Check",
                        "configuration": {
                            "data": { "sourceOutput": "document", "targetInput": "answer" },
                        },
                    },
                ],
            }),
            expected(&[(
                "MissingNodeConfiguration",
                "Error",
                "missingNodeConfiguration",
            )]),
        ),
        (
            "a node carrying the configuration of another node type",
            json!({
                "nodes": [
                    input_node(),
                    {
                        "name": "FlowOutput",
                        "type": "Output",
                        "configuration": { "input": {} },
                        "inputs": [
                            { "name": "document", "type": "String", "expression": "$.data" },
                        ],
                    },
                ],
                "connections": [passthrough_connection()],
            }),
            expected(&[(
                "MissingNodeConfiguration",
                "Error",
                "missingNodeConfiguration",
            )]),
        ),
        // Data that cannot flow: mismatched types, malformed expressions.
        (
            "a connection carrying a String output into a Number input",
            json!({
                "nodes": [
                    input_node(),
                    {
                        "name": "FlowOutput",
                        "type": "Output",
                        "configuration": { "output": {} },
                        "inputs": [
                            { "name": "document", "type": "Number", "expression": "$.data" },
                        ],
                    },
                ],
                "connections": [passthrough_connection()],
            }),
            expected(&[(
                "IncompatibleConnectionDataType",
                "Error",
                "incompatibleConnectionDataType",
            )]),
        ),
        (
            "a node input whose expression doesn't address the incoming data",
            json!({
                "nodes": [
                    input_node(),
                    {
                        "name": "FlowOutput",
                        "type": "Output",
                        "configuration": { "output": {} },
                        "inputs": [
                            { "name": "document", "type": "String", "expression": "data" },
                        ],
                    },
                ],
                "connections": [passthrough_connection()],
            }),
            expected(&[(
                "MalformedNodeInputExpression",
                "Error",
                "malformedNodeInputExpression",
            )]),
        ),
        // The condition node's own rules.
        (
            "a condition node with no default condition",
            conditional(
                json!([{ "name": "isYes", "expression": "answer == \"yes\"" }]),
                "isYes",
            ),
            expected(&[(
                "MissingDefaultCondition",
                "Error",
                "missingDefaultCondition",
            )]),
        ),
        (
            "a condition node whose fallback is named something other than default",
            conditional(
                json!([
                    { "name": "isYes", "expression": "answer == \"yes\"" },
                    { "name": "fallback" },
                ]),
                "isYes",
            ),
            expected(&[(
                "MissingDefaultCondition",
                "Error",
                "missingDefaultCondition",
            )]),
        ),
        (
            "a condition node testing the same expression twice",
            conditional(
                json!([
                    { "name": "isYes", "expression": "answer == \"yes\"" },
                    { "name": "alsoYes", "expression": "answer == \"yes\"" },
                    { "name": "default" },
                ]),
                "isYes",
            ),
            expected(&[(
                "DuplicateConditionExpression",
                "Error",
                "duplicateConditionExpression",
            )]),
        ),
        (
            "a condition referring to none of its node's inputs",
            conditional(
                json!([
                    { "name": "isYes", "expression": "1 == 1" },
                    { "name": "default" },
                ]),
                "isYes",
            ),
            expected(&[(
                "MalformedConditionExpression",
                "Error",
                "malformedConditionExpression",
            )]),
        ),
        // Properties of the whole graph.
        (
            "an empty definition",
            json!({}),
            expected(&[
                ("MissingStartingNodes", "Error", "missingStartingNodes"),
                ("MissingEndingNodes", "Error", "missingEndingNodes"),
            ]),
        ),
        (
            "a definition with an ending node only",
            json!({ "nodes": [output_node()] }),
            expected(&[
                ("MissingStartingNodes", "Error", "missingStartingNodes"),
                // No connection at all, so the ending node's input is fed by
                // nothing.
                ("UnfulfilledNodeInput", "Warning", "unfulfilledNodeInput"),
            ]),
        ),
        (
            "a definition with a starting node only",
            json!({ "nodes": [input_node()] }),
            expected(&[("MissingEndingNodes", "Error", "missingEndingNodes")]),
        ),
        (
            "a cycle of two nodes",
            two_node_ring,
            expected(&[("CyclicConnection", "Error", "cyclicConnection")]),
        ),
        (
            "a cycle of three nodes",
            three_node_ring,
            expected(&[("CyclicConnection", "Error", "cyclicConnection")]),
        ),
        (
            "two connections ending at one node input",
            json!({
                "nodes": [
                    {
                        "name": "FlowInput",
                        "type": "Input",
                        "configuration": { "input": {} },
                        "outputs": [
                            { "name": "first", "type": "String" },
                            { "name": "second", "type": "String" },
                        ],
                    },
                    output_node(),
                ],
                "connections": [
                    {
                        "name": "FirstToOutput",
                        "type": "Data",
                        "source": "FlowInput",
                        "target": "FlowOutput",
                        "configuration": {
                            "data": { "sourceOutput": "first", "targetInput": "document" },
                        },
                    },
                    {
                        "name": "SecondToOutput",
                        "type": "Data",
                        "source": "FlowInput",
                        "target": "FlowOutput",
                        "configuration": {
                            "data": { "sourceOutput": "second", "targetInput": "document" },
                        },
                    },
                ],
            }),
            expected(&[(
                "MultipleNodeInputConnections",
                "Error",
                "multipleNodeInputConnections",
            )]),
        ),
        (
            "the same connection declared twice",
            json!({
                "nodes": [input_node(), output_node()],
                "connections": [
                    {
                        "name": "InputToOutput",
                        "type": "Data",
                        "source": "FlowInput",
                        "target": "FlowOutput",
                        "configuration": {
                            "data": { "sourceOutput": "document", "targetInput": "document" },
                        },
                    },
                    {
                        "name": "InputToOutputAgain",
                        "type": "Data",
                        "source": "FlowInput",
                        "target": "FlowOutput",
                        "configuration": {
                            "data": { "sourceOutput": "document", "targetInput": "document" },
                        },
                    },
                ],
            }),
            expected(&[
                ("DuplicateConnections", "Error", "duplicateConnections"),
                (
                    "MultipleNodeInputConnections",
                    "Error",
                    "multipleNodeInputConnections",
                ),
            ]),
        ),
        (
            "a node no path from the starting node reaches",
            json!({
                "nodes": [
                    input_node(),
                    output_node(),
                    {
                        "name": "Orphan",
                        "type": "Collector",
                        "configuration": { "collector": {} },
                    },
                ],
                "connections": [passthrough_connection()],
            }),
            expected(&[("UnreachableNode", "Warning", "unreachableNode")]),
        ),
        // The loop node's body.
        (
            "a loop body with no LoopController node",
            with_loop_body(json!([loop_input_node("LoopStart")])),
            expected(&[(
                "MissingLoopControllerNode",
                "Error",
                "missingLoopControllerNode",
            )]),
        ),
        (
            "a loop body with no LoopInput node",
            with_loop_body(json!([loop_controller_node("Controller")])),
            expected(&[("MissingLoopInputNode", "Error", "missingLoopInputNode")]),
        ),
        (
            "a loop body with two LoopInput nodes",
            with_loop_body(json!([
                loop_input_node("LoopStart"),
                loop_input_node("OtherStart"),
                loop_controller_node("Controller"),
            ])),
            expected(&[(
                "MultipleLoopInputNodes",
                "Error",
                "multipleLoopInputNodes",
            )]),
        ),
        (
            "a loop body containing a node type a loop can't run",
            with_loop_body(json!([
                loop_input_node("LoopStart"),
                loop_controller_node("Controller"),
                {
                    "name": "Inner",
                    "type": "Iterator",
                    "configuration": { "iterator": {} },
                },
            ])),
            expected(&[(
                "LoopIncompatibleNodeType",
                "Error",
                "loopIncompatibleNodeType",
            )]),
        ),
    ];

    for (case, definition, want) in cases {
        let answer = emulator.validate(definition).await;
        assert_eq!(
            findings(case, &answer),
            want,
            "case {case}: expected the findings {want:?}, got {:?}",
            answer.body
        );
    }
}

/// A cycle is reported against the connection that closes it, whatever the length
/// of the ring — and a ring whose closing connection is removed has no cycle at
/// all.
#[tokio::test]
async fn a_cycle_names_the_connection_that_closes_it() {
    let emulator = Emulator::new();

    for length in [1usize, 2, 3, 4] {
        let case = format!("a ring of {length} node(s)");
        let (definition, closing) = ring(length);

        let answer = emulator.validate(definition.clone()).await;
        let reported = answer
            .member(&["validations"])
            .as_array()
            .expect("validations is an array")
            .clone();
        let cycles: Vec<&Value> = reported
            .iter()
            .filter(|validation| validation["type"] == json!("CyclicConnection"))
            .collect();
        assert_eq!(
            cycles.len(),
            1,
            "case {case}: expected exactly one cycle finding, got {:?}",
            answer.body
        );
        assert_eq!(
            cycles[0]["details"]["cyclicConnection"]["connection"],
            json!(closing),
            "case {case}: the finding must name the connection that closes the ring, got {:?}",
            cycles[0]
        );

        // The same definition with the closing connection removed: no cycle.
        let mut acyclic = definition;
        let kept: Vec<Value> = acyclic["connections"]
            .as_array()
            .expect("connections is an array")
            .iter()
            .filter(|connection| connection["name"] != json!(closing))
            .cloned()
            .collect();
        acyclic["connections"] = json!(kept);
        let answer = emulator.validate(acyclic).await;
        let types: Vec<String> = findings(&case, &answer)
            .into_iter()
            .map(|(validation_type, _, _)| validation_type)
            .collect();
        assert!(
            !types.iter().any(|t| t == "CyclicConnection"),
            "case {case}: removing the closing connection removes the cycle, got {types:?}"
        );
    }
}

/// The operation is a read: validating a definition reports the same findings
/// however often it is called, and changes nothing a later caller sees — the flow
/// created before it is reported exactly as it was, and no flow is minted for the
/// definition that was validated.
#[tokio::test]
async fn validating_a_definition_changes_no_stored_flow() {
    let emulator = Emulator::new();

    let created = emulator
        .call(
            "POST",
            "/flows/",
            Some(json!({
                "name": "untouched-flow",
                "executionRoleArn": EXECUTION_ROLE,
                "definition": passthrough(),
            })),
        )
        .await;
    assert_eq!(
        created.status, 201,
        "case setup: expected CreateFlow -> 201, got {created:?}"
    );
    let flow_id = created.member(&["id"]).as_str().expect("a flowId").to_string();

    let (cyclic, _) = ring(2);
    let first = emulator.validate(cyclic.clone()).await;
    let second = emulator.validate(cyclic).await;
    assert_eq!(
        second.body, first.body,
        "case repeated call: a read reports the same findings for the same definition"
    );

    let fetched = emulator
        .call("GET", &format!("/flows/{flow_id}/"), None)
        .await;
    assert_eq!(
        fetched.status, 200,
        "case stored flow: expected GetFlow -> 200, got {fetched:?}"
    );
    assert_eq!(
        fetched.member(&["definition"]),
        created.member(&["definition"]),
        "case stored flow: validating a definition must not touch a stored flow"
    );
    assert_eq!(
        fetched.member(&["validations"]),
        &json!([]),
        "case stored flow: the validated definition belongs to no flow, so the stored flow \
         reports an empty findings list, got {:?}",
        fetched.member(&["validations"])
    );
}

/// A value the MODEL refuses never reaches the validator: it is a bad request, and
/// the refusal names the member path and the requirement it broke — withholding the
/// value when the model marks it `@sensitive` (`FlowDefinition` is) and echoing it
/// when it does not. An enumerated type's value set is listed in the order the table
/// of those values is walked, which `enum_value_set_order.rs` is about.
///
/// The rows differ along what the rule generalises over: the constraint class (an
/// enum value set, a `@pattern`), whether the member sits inside a `@sensitive`
/// shape, whether the value travels in the body or in the query string, and which
/// operation serves it.
#[tokio::test]
async fn a_refusal_withholds_a_sensitive_or_self_describing_value() {
    let emulator = Emulator::new();

    // A definition whose SECOND node declares a type the modelled `FlowNodeType`
    // enum does not admit.
    let unknown_node_type = json!({
        "nodes": [
            input_node(),
            {
                "name": "FlowOutput",
                "type": "NotARealFlowNodeType",
                "configuration": { "output": {} },
                "inputs": [{ "name": "document", "type": "String", "expression": "$.data" }],
            },
        ],
        "connections": [passthrough_connection()],
    });

    // A definition whose second node's NAME is outside the modelled `FlowNodeName`
    // pattern — a `@pattern` violation inside the same `@sensitive` member.
    let malformed_node_name = json!({
        "nodes": [
            input_node(),
            {
                "name": "not a node name!",
                "type": "Output",
                "configuration": { "output": {} },
                "inputs": [{ "name": "document", "type": "String", "expression": "$.data" }],
            },
        ],
    });

    // One refused request, and the whole message it must be refused with.
    struct RefusalCase {
        case: &'static str,
        method: &'static str,
        uri: String,
        body: Option<Value>,
        expected: String,
        /// Text the clause must NOT carry — the submitted value where the model
        /// withholds it, and a string no clause can contain where it does not.
        withheld: &'static str,
    }

    let cases: Vec<RefusalCase> = vec![
        RefusalCase {
            case: "a node type outside its enum, inside the sensitive definition",
            method: "POST",
            uri: "/flows/validate-definition".to_string(),
            body: Some(json!({ "definition": unknown_node_type })),
            expected: "1 validation error detected: Value at 'definition.nodes.2.member.type' \
                 failed to satisfy constraint: Member must satisfy enum value set: \
                 [LoopController, Output, Loop, Prompt, Agent, LoopInput, InlineCode, Input, \
                 Collector, Iterator, KnowledgeBase, Retrieval, Storage, Condition, \
                 LambdaFunction, Lex]"
                .to_string(),
            withheld: "NotARealFlowNodeType",
        },
        RefusalCase {
            case: "a node name outside its pattern, inside the sensitive definition",
            method: "POST",
            uri: "/flows/validate-definition".to_string(),
            body: Some(json!({ "definition": malformed_node_name })),
            expected: "1 validation error detected: Value at 'definition.nodes.2.member.name' \
                 failed to satisfy constraint: Member must satisfy regular expression pattern: \
                 [a-zA-Z]([_]?[0-9a-zA-Z]){1,50}"
                .to_string(),
            withheld: "not a node name!",
        },
        RefusalCase {
            case: "an includedData outside its enum, in the query string of a read",
            method: "GET",
            uri: "/flows/0123456789/?includedData=NEITHER_VALUE".to_string(),
            body: None,
            expected: "1 validation error detected: Value 'NEITHER_VALUE' at 'includedData' \
                 failed to satisfy constraint: Member must satisfy enum value set: [ALL_DATA, \
                 METADATA_ONLY]"
                .to_string(),
            // This member sits inside no `@sensitive` shape, so its value IS echoed.
            withheld: "\u{0}",
        },
        RefusalCase {
            case: "a flow name outside its pattern: a value the model does not mark sensitive",
            method: "POST",
            uri: "/flows/".to_string(),
            body: Some(json!({ "name": "not a flow name!", "executionRoleArn": EXECUTION_ROLE })),
            expected: "1 validation error detected: Value 'not a flow name!' at 'name' failed to \
                 satisfy constraint: Member must satisfy regular expression pattern: \
                 ([0-9a-zA-Z][_-]?){1,100}"
                .to_string(),
            withheld: "\u{0}",
        },
    ];

    for row in cases {
        let case = row.case;
        let refused = emulator.call(row.method, &row.uri, row.body).await;
        assert_eq!(
            refused.status, 400,
            "case {case}: expected a 400 refusal, got {refused:?}"
        );
        assert_eq!(
            refused.error_type.as_deref(),
            Some("ValidationException"),
            "case {case}: expected ValidationException, got {refused:?}"
        );
        let message = refused.message(case);
        assert_eq!(
            message, row.expected,
            "case {case}: expected the refusal {:?}, got {message:?}",
            row.expected
        );
        assert!(
            !message.contains(row.withheld),
            "case {case}: the refusal must not echo {:?}, got {message:?}",
            row.withheld
        );
    }
}

/// A two-node flow — a starting node carrying one `String` output and an ending
/// node carrying the input `input` — with one data connection from the starting
/// node into that input, declared to end at `target`.
///
/// Naming `target` the ending node gives a definition with no defect; naming
/// anything else gives a connection whose target the definition does not declare.
fn two_node_definition(
    start: &str,
    finish: &str,
    input: &str,
    connection: &str,
    target: &str,
) -> Value {
    json!({
        "nodes": [
            {
                "name": start,
                "type": "Input",
                "configuration": { "input": {} },
                "outputs": [{ "name": "document", "type": "String" }],
            },
            {
                "name": finish,
                "type": "Output",
                "configuration": { "output": {} },
                "inputs": [{ "name": input, "type": "String", "expression": "$.data" }],
            },
        ],
        "connections": [{
            "name": connection,
            "type": "Data",
            "source": start,
            "target": target,
            "configuration": { "data": { "sourceOutput": "document", "targetInput": input } },
        }],
    })
}

/// The whole message an `UnfulfilledNodeInput` finding carries: fixed text, with
/// the input and node names substituted.
fn unfulfilled_input_message(input: &str, node: &str) -> String {
    format!("The input '{input}' in node '{node}' is undefined. Connect a source value to this input.")
}

/// The whole message an `UnknownConnectionTarget` finding carries: fixed text,
/// with the connection's two endpoint names substituted.
fn unknown_target_message(source: &str, target: &str) -> String {
    format!(
        "The connection between the nodes '{source}' and '{target}' references an unknown target \
         node '{target}'. Update the connection to reference a valid node that exists in the \
         flow, or add the missing node to the flow."
    )
}

/// The whole message an `UnknownConnectionSource` finding carries: the same
/// sentence as [`unknown_target_message`], about the other end of the connection.
fn unknown_source_message(source: &str, target: &str) -> String {
    format!(
        "The connection between the nodes '{source}' and '{target}' references an unknown source \
         node '{source}'. Update the connection to reference a valid node that exists in the \
         flow, or add the missing node to the flow."
    )
}

/// The whole message an `UnreachableNode` finding carries: fixed text, with the
/// node name substituted.
fn unreachable_node_message(node: &str) -> String {
    format!(
        "The node '{node}' isn't connected to any other nodes. Attach this node to the flow or \
         delete it."
    )
}

/// `ValidateFlowDefinition` reports EVERY defect of the submitted definition,
/// each with the fixed message text of its class and the names taken from the
/// request: a reference that resolves to nothing is one defect, and the defects
/// that follow from it — an input left with no data connection, a node left with
/// no path to it — are reported beside it rather than withheld.
///
/// The rows differ along what the rule generalises over: which defect classes
/// co-occur, the node, input and connection NAMES the messages substitute, which
/// end of a connection fails to resolve, the kind of connection that does or does
/// not fulfil an input, and the COUNT of findings (none, one, two, three).
#[tokio::test]
async fn every_defect_of_a_definition_is_reported_with_the_fixed_text_of_its_class() {
    let emulator = Emulator::new();

    // An input fed by nothing but a CONDITIONAL connection: that connection names
    // no target input, so it carries no value into one.
    let mut conditional_only = conditional(
        json!([
            { "name": "isYes", "expression": "answer == \"yes\"" },
            { "name": "default" },
        ]),
        "isYes",
    );
    conditional_only["connections"]
        .as_array_mut()
        .expect("the connection list")
        .retain(|connection| connection["name"] != json!("InputToOutput"));

    // A connection whose SOURCE the definition does not declare: the other end of
    // the same rule.
    let mut unknown_source = two_node_definition("Start", "Finish", "document", "StartToFinish", "Finish");
    unknown_source["connections"][0]["source"] = json!("Nowhere");

    let cases: Vec<(&str, Value, Value)> = vec![
        (
            "a connection whose target the definition doesn't declare",
            two_node_definition("Start", "Finish", "document", "StartToFinish", "MissingNode"),
            json!([
                {
                    "type": "UnfulfilledNodeInput",
                    "severity": "Warning",
                    "details": { "unfulfilledNodeInput": { "node": "Finish", "input": "document" } },
                    "message": unfulfilled_input_message("document", "Finish"),
                },
                {
                    "type": "UnknownConnectionTarget",
                    "severity": "Error",
                    "details": { "unknownConnectionTarget": { "connection": "StartToFinish" } },
                    "message": unknown_target_message("Start", "MissingNode"),
                },
                {
                    "type": "UnreachableNode",
                    "severity": "Warning",
                    "details": { "unreachableNode": { "node": "Finish" } },
                    "message": unreachable_node_message("Finish"),
                },
            ]),
        ),
        (
            "the same defect under other node, input and connection names",
            two_node_definition("Alpha", "Omega", "payload", "AlphaToOmegaLink", "Elsewhere"),
            json!([
                {
                    "type": "UnfulfilledNodeInput",
                    "severity": "Warning",
                    "details": { "unfulfilledNodeInput": { "node": "Omega", "input": "payload" } },
                    "message": unfulfilled_input_message("payload", "Omega"),
                },
                {
                    "type": "UnknownConnectionTarget",
                    "severity": "Error",
                    "details": { "unknownConnectionTarget": { "connection": "AlphaToOmegaLink" } },
                    "message": unknown_target_message("Alpha", "Elsewhere"),
                },
                {
                    "type": "UnreachableNode",
                    "severity": "Warning",
                    "details": { "unreachableNode": { "node": "Omega" } },
                    "message": unreachable_node_message("Omega"),
                },
            ]),
        ),
        (
            "a connection whose source the definition doesn't declare",
            unknown_source,
            json!([
                {
                    "type": "UnknownConnectionSource",
                    "severity": "Error",
                    "details": { "unknownConnectionSource": { "connection": "StartToFinish" } },
                    "message": unknown_source_message("Nowhere", "Finish"),
                },
                {
                    "type": "UnreachableNode",
                    "severity": "Warning",
                    "details": { "unreachableNode": { "node": "Finish" } },
                    "message": unreachable_node_message("Finish"),
                },
            ]),
        ),
        (
            "an input no data connection names, on an otherwise sound flow",
            json!({
                "nodes": [
                    input_node(),
                    {
                        "name": "FlowOutput",
                        "type": "Output",
                        "configuration": { "output": {} },
                        "inputs": [
                            { "name": "document", "type": "String", "expression": "$.data" },
                            { "name": "extra", "type": "String", "expression": "$.data" },
                        ],
                    },
                ],
                "connections": [passthrough_connection()],
            }),
            json!([{
                "type": "UnfulfilledNodeInput",
                "severity": "Warning",
                "details": { "unfulfilledNodeInput": { "node": "FlowOutput", "input": "extra" } },
                "message": unfulfilled_input_message("extra", "FlowOutput"),
            }]),
        ),
        (
            "an input a conditional connection reaches but feeds no value into",
            conditional_only,
            json!([{
                "type": "UnfulfilledNodeInput",
                "severity": "Warning",
                "details": { "unfulfilledNodeInput": { "node": "FlowOutput", "input": "document" } },
                "message": unfulfilled_input_message("document", "FlowOutput"),
            }]),
        ),
        (
            "two inputs on two nodes, each fed by nothing",
            json!({
                "nodes": [
                    input_node(),
                    {
                        "name": "Middle",
                        "type": "Collector",
                        "configuration": { "collector": {} },
                        "inputs": [
                            { "name": "first", "type": "String", "expression": "$.data" },
                            { "name": "second", "type": "String", "expression": "$.data" },
                        ],
                        "outputs": [{ "name": "carried", "type": "String" }],
                    },
                    {
                        "name": "FlowOutput",
                        "type": "Output",
                        "configuration": { "output": {} },
                        "inputs": [
                            { "name": "document", "type": "String", "expression": "$.data" },
                            { "name": "also", "type": "String", "expression": "$.data" },
                        ],
                    },
                ],
                "connections": [
                    {
                        "name": "InputToMiddle",
                        "type": "Data",
                        "source": "FlowInput",
                        "target": "Middle",
                        "configuration": {
                            "data": { "sourceOutput": "document", "targetInput": "first" },
                        },
                    },
                    {
                        "name": "MiddleToOutput",
                        "type": "Data",
                        "source": "Middle",
                        "target": "FlowOutput",
                        "configuration": {
                            "data": { "sourceOutput": "carried", "targetInput": "document" },
                        },
                    },
                ],
            }),
            json!([
                {
                    "type": "UnfulfilledNodeInput",
                    "severity": "Warning",
                    "details": { "unfulfilledNodeInput": { "node": "Middle", "input": "second" } },
                    "message": unfulfilled_input_message("second", "Middle"),
                },
                {
                    "type": "UnfulfilledNodeInput",
                    "severity": "Warning",
                    "details": { "unfulfilledNodeInput": { "node": "FlowOutput", "input": "also" } },
                    "message": unfulfilled_input_message("also", "FlowOutput"),
                },
            ]),
        ),
        (
            "a node no connection attaches to the flow",
            json!({
                "nodes": [
                    input_node(),
                    output_node(),
                    {
                        "name": "Orphan",
                        "type": "Collector",
                        "configuration": { "collector": {} },
                    },
                ],
                "connections": [passthrough_connection()],
            }),
            json!([{
                "type": "UnreachableNode",
                "severity": "Warning",
                "details": { "unreachableNode": { "node": "Orphan" } },
                "message": unreachable_node_message("Orphan"),
            }]),
        ),
        // The control: the same definition as the first row with the connection
        // pointed at the node the definition DOES declare has no defect at all.
        (
            "the same definition with the connection's target declared",
            two_node_definition("Start", "Finish", "document", "StartToFinish", "Finish"),
            json!([]),
        ),
        (
            "a passthrough definition",
            passthrough(),
            json!([]),
        ),
    ];

    for (case, definition, want) in cases {
        let answer = emulator.validate(definition).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected ValidateFlowDefinition -> 200, got {answer:?}"
        );
        assert_eq!(
            answer.member(&["validations"]),
            &want,
            "case {case}: expected the findings {want:?}, got {:?}",
            answer.member(&["validations"])
        );
    }
}
