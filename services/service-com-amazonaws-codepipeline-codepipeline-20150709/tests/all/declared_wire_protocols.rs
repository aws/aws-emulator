// SPDX-License-Identifier: CC0-1.0

//! The two wire protocols this service's model DECLARES, and the one request of it that
//! names neither.
//!
//! The rule under test: **a request is served in the protocol it names — `awsJson1_1` or
//! `rpcv2Cbor`, the two this model declares (`smithy_query query="service_info"`) — and both
//! answer out of the same state, each in its own wire form: the awsJson shape discriminator
//! `__type` is a member of the awsJson wire form, so a CBOR answer carries none, and a
//! framework-level refusal is rendered in the envelope of the protocol the refused call was
//! made in. A request addressed to an address this service PUBLISHED — the trigger URL a
//! webhook advertises — is served whatever protocol its request names, including none,
//! because it is no API call; a request to the API itself that names no declared protocol
//! stays refused.**
//!
//! Every assertion goes through the router, so nothing here can pass by reading a handler's
//! internals: a CBOR call's request body is written with that protocol's own codec and its
//! answer is read back with the same codec, never encoded or decoded by hand.

use std::collections::BTreeSet;
use std::sync::Arc;

use aws_smithy_schema::codec::{Codec, FinishSerializer};
use aws_smithy_schema::serde::{SerializableStruct, ShapeSerializer};
use aws_smithy_schema::Schema;
use aws_smithy_schema_server::{CborCodec, DeserializableStruct};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_codepipeline_codepipeline_20150709::ops::types::{
    GetPipelineInput, GetPipelineOutput, ListActionTypesInput, ListActionTypesOutput,
    ListPipelinesInput, ListPipelinesOutput, GET_PIPELINE_INPUT_SCHEMA,
    LIST_ACTION_TYPES_INPUT_SCHEMA, LIST_PIPELINES_INPUT_SCHEMA,
};

/// The `X-Amz-Target` prefix OPERATIONS.md records for this service, which is also the
/// Smithy service shape the CBOR route names in its path.
const TARGET_PREFIX: &str = "CodePipeline_20150709";

/// The media type an `awsJson1_1` call carries its body under, and the one its answers carry.
const JSON_CONTENT_TYPE: &str = "application/x-amz-json-1.1";

/// The media type an `rpcv2Cbor` call carries its body under, and the header that protocol's
/// specification requires of every request and every answer ("the `smithy-protocol` header…
/// MUST be set to `rpc-v2-cbor`").
const CBOR_CONTENT_TYPE: &str = "application/cbor";
const SMITHY_PROTOCOL_HEADER: &str = "smithy-protocol";
const RPC_V2_CBOR: &str = "rpc-v2-cbor";

/// The member the awsJson family names a serialized shape in.
const TYPE_MEMBER: &str = "__type";

/// The error code this service answers an input-constraint violation with, in either
/// protocol: the modelled `ValidationException`.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// A `roleArn` satisfying `RoleArn`'s modelled pattern.
const ROLE_ARN: &str = "arn:aws:iam::123456789012:role/service-role/cp";

/// The branch the pipelines here configure their source action for.
const BRANCH: &str = "main";

/// The CBOR encoding of an EMPTY map (major type 5, length 0): an input naming no member at
/// all, which is how a required member is left absent on this wire.
const EMPTY_CBOR_MAP: &[u8] = &[0xa0];

/// The names the pipelines of the first suite are stored under. They differ in the characters
/// `PipelineName`'s pattern admits, which is what each answer is read back by.
const STORED_PIPELINES: &[&str] = &["one", "two.pipeline", "three-4_5"];

/// One emulator: the shared engine, the service mounted on it, and the router that
/// dispatches to it — exactly how `core/router` assembles the production emulator.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_codepipeline_codepipeline_20150709::EmulatedService::new(engine),
        );
        Self { router }
    }

    /// The credential scope that routes a request to this service, whatever protocol it is
    /// made in.
    fn authorization() -> &'static str {
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/us-east-1/codepipeline/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=0123456789abcdef"
    }

    /// Call `operation` in this service's `awsJson1_1` protocol: `POST /` with the target
    /// header that names it.
    async fn json_call(&self, operation: &str, body: &Value) -> (u16, String, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", JSON_CONTENT_TYPE)
            .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
            .header("authorization", Self::authorization())
            .body(bytes::Bytes::from(body.to_string()))
            .expect("a well-formed awsJson request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let content_type = header(&response, "content-type");
        let body = response.into_body();
        let parsed = if body.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&body).unwrap_or_else(|error| {
                panic!("{operation}: the answer is not JSON ({error}): {body:?}")
            })
        };
        (status, content_type, parsed)
    }

    /// A successful awsJson call.
    async fn json_ok(&self, operation: &str, body: &Value) -> Value {
        let (status, _content_type, answer) = self.json_call(operation, body).await;
        assert_eq!(
            status, 200,
            "{operation}: expected HTTP 200 on the JSON wire, got {status} with {answer}"
        );
        answer
    }

    /// Call `operation` in this service's `rpcv2Cbor` protocol: the route that protocol
    /// addresses every call with, carrying its own discriminator header.
    async fn cbor_call(&self, operation: &str, body: bytes::Bytes) -> CborAnswer {
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("/service/{TARGET_PREFIX}/operation/{operation}"))
            .header("content-type", CBOR_CONTENT_TYPE)
            .header("accept", CBOR_CONTENT_TYPE)
            .header(SMITHY_PROTOCOL_HEADER, RPC_V2_CBOR)
            .header("authorization", Self::authorization())
            .body(body)
            .expect("a well-formed rpcv2Cbor request");
        let response = self.router.dispatch(request).await;
        CborAnswer {
            status: response.status().as_u16(),
            content_type: header(&response, "content-type"),
            protocol: header(&response, SMITHY_PROTOCOL_HEADER),
            body: response.into_body(),
        }
    }

    /// A successful CBOR call.
    async fn cbor_ok(&self, operation: &str, body: bytes::Bytes) -> CborAnswer {
        let answer = self.cbor_call(operation, body).await;
        assert_eq!(
            answer.status, 200,
            "{operation}: expected HTTP 200 on the CBOR wire, got {} with {:?}",
            answer.status, answer.body
        );
        answer
    }

    /// Send `method` to `url` — a URL this service PUBLISHED — with `headers` and `body`, the
    /// way a third party dials a webhook: no AWS credential of any kind.
    async fn dial(
        &self,
        method: &str,
        url: &str,
        headers: &[(&str, &str)],
        body: &str,
    ) -> (u16, String) {
        let mut request = http::Request::builder().method(method).uri(url);
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        let response = self
            .router
            .dispatch(
                request
                    .body(bytes::Bytes::from(body.to_string()))
                    .expect("a valid request"),
            )
            .await;
        let status = response.status().as_u16();
        let answered = String::from_utf8_lossy(&response.into_body()).to_string();
        (status, answered)
    }

    /// Store a pipeline named `name` whose source action is `Custom`-owned (so an execution
    /// that reaches it waits for a job worker instead of being carried on by the clock).
    async fn create_pipeline(&self, name: &str) {
        self.json_ok(
            "CreatePipeline",
            &json!({"pipeline": {
                "name": name,
                "roleArn": ROLE_ARN,
                "artifactStore": {"type": "S3", "location": "my-bucket"},
                "stages": [
                    {
                        "name": "Source",
                        "actions": [{
                            "name": "Src",
                            "actionTypeId": {
                                "category": "Source",
                                "owner": "Custom",
                                "provider": "MySource",
                                "version": "1",
                            },
                            "configuration": {"Branch": BRANCH, "Repo": "demo"},
                            "outputArtifacts": [{"name": "SourceOut"}],
                        }],
                    },
                    {
                        "name": "Build",
                        "actions": [{
                            "name": "Build0",
                            "actionTypeId": {
                                "category": "Build",
                                "owner": "AWS",
                                "provider": "CodeBuild",
                                "version": "1",
                            },
                        }],
                    },
                ],
            }}),
        )
        .await;
    }

    /// Define an UNAUTHENTICATED webhook named `name` on `pipeline` — one every POST
    /// satisfies, since it is held to no authentication and to no filter — and answer the
    /// trigger URL `PutWebhook` published for it.
    async fn put_webhook(&self, name: &str, pipeline: &str) -> String {
        let answer = self
            .json_ok(
                "PutWebhook",
                &json!({"webhook": {
                    "name": name,
                    "targetPipeline": pipeline,
                    "targetAction": "Src",
                    "filters": [],
                    "authentication": "UNAUTHENTICATED",
                    "authenticationConfiguration": {},
                }}),
            )
            .await;
        answer["webhook"]["url"]
            .as_str()
            .unwrap_or_else(|| panic!("PutWebhook reports a trigger URL, got {answer}"))
            .to_string()
    }

    /// How many executions of `pipeline` carry the `Webhook` trigger type — the runs a POST
    /// to a trigger URL started, and nothing else.
    async fn webhook_runs(&self, pipeline: &str) -> usize {
        let listed = self
            .json_ok("ListPipelineExecutions", &json!({"pipelineName": pipeline}))
            .await;
        listed["pipelineExecutionSummaries"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|summary| summary["trigger"]["triggerType"] == json!("Webhook"))
            .count()
    }
}

/// One answer on the CBOR route, as a client holds it.
struct CborAnswer {
    status: u16,
    content_type: String,
    protocol: String,
    body: bytes::Bytes,
}

/// The value `response` carries in the header `name`, or the empty string for an answer that
/// carries none.
fn header(response: &http::Response<bytes::Bytes>, name: &str) -> String {
    response
        .headers()
        .get(name)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string()
}

/// `input` as the request body of an `rpcv2Cbor` call: written through that protocol's own
/// codec against the shape's schema, so nothing here encodes CBOR by hand.
fn cbor_body<I: SerializableStruct>(schema: &Schema, input: &I) -> bytes::Bytes {
    let codec = CborCodec::default();
    let mut serializer = codec.create_serializer();
    serializer
        .write_struct(schema, input)
        .expect("the input shape serializes");
    bytes::Bytes::from(serializer.finish())
}

/// One CBOR answer read back as the typed output it encodes — the decode a client's SDK
/// performs, through the same codec.
fn cbor_output<O: DeserializableStruct>(case: &str, body: &[u8]) -> O {
    let codec = CborCodec::default();
    let mut deserializer = codec.create_deserializer(body);
    O::deserialize(&mut deserializer)
        .unwrap_or_else(|error| panic!("case {case:?}: the answer does not decode ({error})"))
}

/// The entries of the flat CBOR map an error answer on this route is
/// (`{"__type": …, "message": …}`), read with the maintained CBOR decoder.
fn cbor_entries(case: &str, body: &[u8]) -> Vec<(String, String)> {
    let mut decoder = minicbor::Decoder::new(body);
    let len = decoder
        .map()
        .unwrap_or_else(|error| panic!("case {case:?}: the answer is no CBOR map ({error})"))
        .unwrap_or_else(|| panic!("case {case:?}: the answer is no definite-length CBOR map"));
    (0..len)
        .map(|_| {
            let key = decoder
                .str()
                .unwrap_or_else(|error| panic!("case {case:?}: a map key is no text ({error})"))
                .to_string();
            let value = decoder
                .str()
                .unwrap_or_else(|error| panic!("case {case:?}: a map value is no text ({error})"))
                .to_string();
            (key, value)
        })
        .collect()
}

/// Whether the top of `body` is a CBOR map — what every operation's output is encoded as —
/// of either length form, since this codec writes indefinite-length maps.
fn is_a_cbor_map(body: &[u8]) -> bool {
    minicbor::Decoder::new(body).map().is_ok()
}

/// Every shape name an awsJson answer names a structure of its own with, deduped: the
/// `__type` members, wherever in the document they sit.
fn named_shapes(value: &Value) -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    collect_named_shapes(value, &mut found);
    found
}

fn collect_named_shapes(value: &Value, found: &mut BTreeSet<String>) {
    match value {
        Value::Object(members) => {
            for (member, entry) in members {
                if member == TYPE_MEMBER {
                    if let Some(name) = entry.as_str() {
                        found.insert(name.to_string());
                    }
                }
                collect_named_shapes(entry, found);
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_named_shapes(item, found);
            }
        }
        _ => {}
    }
}

/// Whether `body` carries the bytes of a text string spelling `text`.
///
/// A CBOR text string's bytes ARE its UTF-8 content, so a document carrying a member of that
/// name carries these bytes somewhere: a body that does not carries no such member, at any
/// depth.
fn carries_the_text(body: &[u8], text: &str) -> bool {
    body.windows(text.len())
        .any(|window| window == text.as_bytes())
}

/// A call made in either declared protocol answers out of the SAME state: a pipeline stored
/// through one protocol is reported, member for member, by a call in the other.
#[tokio::test]
async fn both_declared_protocols_answer_out_of_one_store() {
    let emulator = Emulator::new();
    for name in STORED_PIPELINES {
        emulator.create_pipeline(name).await;

        let json = emulator.json_ok("GetPipeline", &json!({"name": name})).await;
        let answered = emulator
            .cbor_ok(
                "GetPipeline",
                cbor_body(
                    GET_PIPELINE_INPUT_SCHEMA,
                    &GetPipelineInput {
                        name: (*name).to_string(),
                        version: None,
                    },
                ),
            )
            .await;
        assert_eq!(
            answered.content_type, CBOR_CONTENT_TYPE,
            "case {name:?}: a CBOR call is answered in CBOR, got {:?}",
            answered.content_type
        );
        assert_eq!(
            answered.protocol, RPC_V2_CBOR,
            "case {name:?}: a CBOR answer names its protocol, got {:?}",
            answered.protocol
        );
        let decoded: GetPipelineOutput = cbor_output(name, &answered.body);
        let pipeline = decoded
            .pipeline
            .unwrap_or_else(|| panic!("case {name:?}: the CBOR answer carries the pipeline"));
        assert_eq!(
            pipeline.name, *name,
            "case {name:?}: the CBOR answer names the pipeline {:?}",
            pipeline.name
        );
        assert_eq!(
            json["pipeline"]["name"],
            json!(pipeline.name),
            "case {name:?}: the two protocols name different pipelines"
        );
        assert_eq!(
            json["pipeline"]["roleArn"],
            json!(pipeline.role_arn),
            "case {name:?}: the two protocols report different roles"
        );
        let stages: Vec<&str> = pipeline
            .stages
            .iter()
            .map(|stage| stage.name.as_str())
            .collect();
        assert_eq!(
            stages,
            vec!["Source", "Build"],
            "case {name:?}: the CBOR answer reports the stages as {stages:?}"
        );
        assert_eq!(
            json["pipeline"]["version"].as_i64(),
            pipeline.version.map(i64::from),
            "case {name:?}: the two protocols report different versions"
        );
    }

    // And the listing, which reports every pipeline the account holds whichever protocol
    // stored it.
    let listed: ListPipelinesOutput = cbor_output(
        "the listing",
        &emulator
            .cbor_ok(
                "ListPipelines",
                cbor_body(LIST_PIPELINES_INPUT_SCHEMA, &ListPipelinesInput::default()),
            )
            .await
            .body,
    );
    let reported: BTreeSet<String> = listed
        .pipelines
        .unwrap_or_default()
        .iter()
        .filter_map(|summary| summary.name.clone())
        .collect();
    let expected: BTreeSet<String> = STORED_PIPELINES
        .iter()
        .map(|name| (*name).to_string())
        .collect();
    assert_eq!(
        reported, expected,
        "the CBOR listing reports {reported:?} of the pipelines stored through the JSON wire"
    );
}

/// The shape discriminator is a member of the awsJson wire form: the JSON answer names the
/// shapes this service's catalog of them lists, and the CBOR answer of the same call carries
/// no such member while still being that operation's own output.
#[tokio::test]
async fn the_shape_discriminator_is_a_member_of_the_json_wire_form_only() {
    let emulator = Emulator::new();
    emulator.create_pipeline("demo").await;

    // The rows differ in WHICH shapes the answer carries a discriminator for: an action
    // type's identity reached through a pipeline's own structure, the same identity reached
    // through the catalog listing, and an answer carrying no discriminated structure at all.
    // Each row names the member its operation's output carries, so a CBOR answer is read as
    // that operation's answer rather than as any map.
    let rows: &[(&str, &str, Value, &[&str], &str)] = &[
        (
            "a pipeline's structure",
            "GetPipeline",
            json!({"name": "demo"}),
            &["ActionTypeId"],
            "pipeline",
        ),
        (
            "the action type catalog",
            "ListActionTypes",
            json!({}),
            &["ActionTypeId"],
            "actionTypes",
        ),
        (
            "a listing of summaries",
            "ListPipelines",
            json!({}),
            &[],
            "pipelines",
        ),
    ];
    for (case, operation, input, shapes, member) in rows {
        let json = emulator.json_ok(operation, input).await;
        let named = named_shapes(&json);
        let expected: BTreeSet<String> = shapes.iter().map(|name| (*name).to_string()).collect();
        assert_eq!(
            named, expected,
            "case {case:?} ({operation}): the JSON answer names {named:?}"
        );

        let answered = emulator.cbor_ok(operation, cbor_input(operation)).await;
        assert!(
            is_a_cbor_map(&answered.body),
            "case {case:?} ({operation}): the answer is no CBOR map: {:?}",
            answered.body
        );
        assert!(
            carries_the_text(&answered.body, member),
            "case {case:?} ({operation}): the CBOR answer carries no {member:?} member: {:?}",
            answered.body
        );
        assert!(
            !carries_the_text(&answered.body, TYPE_MEMBER),
            "case {case:?} ({operation}): the CBOR answer carries a {TYPE_MEMBER:?} member, \
             which is how the awsJson wire form names a shape"
        );
    }

    // The CBOR catalog is the same catalog, read as the typed output it encodes: the
    // discriminator is the one thing missing from it, not the content.
    let catalog: ListActionTypesOutput = cbor_output(
        "the action type catalog",
        &emulator
            .cbor_ok("ListActionTypes", cbor_input("ListActionTypes"))
            .await
            .body,
    );
    let json = emulator.json_ok("ListActionTypes", &json!({})).await;
    let reported: Vec<String> = catalog
        .action_types
        .iter()
        .map(|action_type| {
            format!(
                "{}:{}:{}:{}",
                action_type.id.category,
                action_type.id.owner,
                action_type.id.provider,
                action_type.id.version
            )
        })
        .collect();
    let on_the_json_wire: Vec<String> = json["actionTypes"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|action_type| {
            let id = &action_type["id"];
            format!(
                "{}:{}:{}:{}",
                id["category"].as_str().unwrap_or_default(),
                id["owner"].as_str().unwrap_or_default(),
                id["provider"].as_str().unwrap_or_default(),
                id["version"].as_str().unwrap_or_default()
            )
        })
        .collect();
    assert!(
        !reported.is_empty(),
        "the CBOR catalog reports no action type at all"
    );
    assert_eq!(
        reported, on_the_json_wire,
        "the two protocols report different action types: {reported:?} against \
         {on_the_json_wire:?}"
    );
}

/// The request body of one of the reads above, encoded through the protocol's own codec: each
/// takes its own input shape, so the operation names it.
fn cbor_input(operation: &str) -> bytes::Bytes {
    match operation {
        "GetPipeline" => cbor_body(
            GET_PIPELINE_INPUT_SCHEMA,
            &GetPipelineInput {
                name: "demo".to_string(),
                version: None,
            },
        ),
        "ListActionTypes" => cbor_body(
            LIST_ACTION_TYPES_INPUT_SCHEMA,
            &ListActionTypesInput::default(),
        ),
        "ListPipelines" => cbor_body(LIST_PIPELINES_INPUT_SCHEMA, &ListPipelinesInput::default()),
        other => panic!("{other} is not an operation this suite calls on the CBOR route"),
    }
}

/// A framework-level refusal is rendered in the envelope of the protocol the refused call was
/// made in, stating the same code and the same words in both.
#[tokio::test]
async fn a_refusal_is_rendered_in_the_envelope_of_the_calls_own_protocol() {
    let emulator = Emulator::new();
    // The rows differ in HOW the request misses the model's own rules: a required member left
    // absent, a value below its `@length` floor, and a value its `@pattern` does not admit.
    // `GetPipelineInput$name` is `@required`, `@length(min: 1, max: 100)` and
    // `@pattern("^[A-Za-z0-9.@\\-_]+$")`
    // (`smithy_query query="shape" shape="GetPipelineInput"`).
    let rows: &[(&str, Value, bytes::Bytes, &str)] = &[
        (
            "a required member left absent",
            json!({}),
            bytes::Bytes::from_static(EMPTY_CBOR_MAP),
            "1 validation error detected: Value null at 'name' failed to satisfy constraint: \
             Member must not be null",
        ),
        (
            "a value below its length floor",
            json!({"name": ""}),
            cbor_body(
                GET_PIPELINE_INPUT_SCHEMA,
                &GetPipelineInput {
                    name: String::new(),
                    version: None,
                },
            ),
            "1 validation error detected: Value at 'name' failed to satisfy constraint: \
             Member must have length greater than or equal to 1",
        ),
        (
            "a value its pattern does not admit",
            json!({"name": "no spaces allowed"}),
            cbor_body(
                GET_PIPELINE_INPUT_SCHEMA,
                &GetPipelineInput {
                    name: "no spaces allowed".to_string(),
                    version: None,
                },
            ),
            "1 validation error detected: Value at 'name' failed to satisfy constraint: \
             Member must satisfy regular expression pattern: [A-Za-z0-9.@\\-_]+",
        ),
    ];
    for (case, json_input, cbor_body, message) in rows {
        let (status, content_type, answer) = emulator.json_call("GetPipeline", json_input).await;
        assert_eq!(
            (status, content_type.as_str()),
            (400, JSON_CONTENT_TYPE),
            "case {case:?}: the JSON refusal answered {status} as {content_type:?} with {answer}"
        );
        assert_eq!(
            (answer[TYPE_MEMBER].as_str(), answer["message"].as_str()),
            (Some(VALIDATION_EXCEPTION), Some(*message)),
            "case {case:?}: the JSON refusal states {answer}"
        );

        let refused = emulator.cbor_call("GetPipeline", cbor_body.clone()).await;
        assert_eq!(
            (
                refused.status,
                refused.content_type.as_str(),
                refused.protocol.as_str()
            ),
            (400, CBOR_CONTENT_TYPE, RPC_V2_CBOR),
            "case {case:?}: the CBOR refusal answered {} as {:?} / {:?}",
            refused.status,
            refused.content_type,
            refused.protocol
        );
        let entries = cbor_entries(case, &refused.body);
        let stated = |member: &str| {
            entries
                .iter()
                .find(|(key, _)| key == member)
                .map(|(_, value)| value.clone())
        };
        assert_eq!(
            (stated(TYPE_MEMBER), stated("message")),
            (
                Some(VALIDATION_EXCEPTION.to_string()),
                Some((*message).to_string())
            ),
            "case {case:?}: the CBOR refusal states {entries:?}"
        );
    }
}

/// A POST to a trigger URL this service PUBLISHED is served whatever protocol its request
/// names — including none, which is what a provider sends — while a call to the API itself
/// that names none of the declared protocols stays refused.
#[tokio::test]
async fn a_published_trigger_url_is_served_whatever_protocol_its_request_names() {
    let emulator = Emulator::new();
    // The rows span what a trigger request can carry where a protocol marker would sit: the
    // provider's own media type, no content type at all, each of the two markers this model
    // declares, and the marker of a protocol it declares none of.
    let rows: &[(&str, &[(&str, &str)])] = &[
        (
            "the provider's own media type",
            &[("content-type", "application/json")],
        ),
        ("no content type at all", &[]),
        (
            "this service's JSON marker",
            &[("content-type", JSON_CONTENT_TYPE)],
        ),
        (
            "this service's CBOR marker",
            &[
                ("content-type", CBOR_CONTENT_TYPE),
                (SMITHY_PROTOCOL_HEADER, RPC_V2_CBOR),
            ],
        ),
        (
            "the marker of a protocol this model does not declare",
            &[("content-type", "application/x-www-form-urlencoded")],
        ),
    ];
    for (index, (case, headers)) in rows.iter().enumerate() {
        // A pipeline and a webhook of its own per row, so the execution each trigger starts
        // is read back for that row alone.
        let pipeline = format!("triggered{index}");
        emulator.create_pipeline(&pipeline).await;
        let url = emulator
            .put_webhook(&format!("hook{index}"), &pipeline)
            .await;

        let payload = json!({"ref": format!("refs/heads/{BRANCH}")}).to_string();
        let (status, answer) = emulator.dial("POST", &url, headers, &payload).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the published URL answered {status} ({answer:?}) to the POST it \
             describes"
        );
        let runs = emulator.webhook_runs(&pipeline).await;
        assert_eq!(
            runs, 1,
            "case {case:?}: expected the POST to start one Webhook-triggered execution, got \
             {runs}"
        );

        // The one write that URL describes is a POST, and every other method is answered by
        // the plane too rather than by the protocol selection in front of it.
        let (status, answer) = emulator.dial("GET", &url, headers, "").await;
        assert_eq!(
            status, 405,
            "case {case:?}: a GET to the trigger URL answered {status} ({answer:?})"
        );
    }

    // The exemption is the published plane's alone: a call to the API naming none of the
    // declared protocols is still refused, as a request no codec of this service can read.
    let (status, answer) = emulator
        .dial(
            "POST",
            "/",
            &[
                ("content-type", "application/json"),
                ("authorization", Emulator::authorization()),
            ],
            &json!({"name": "triggered0"}).to_string(),
        )
        .await;
    assert_eq!(
        status, 404,
        "a call naming no declared protocol answered {status} ({answer:?})"
    );
}
