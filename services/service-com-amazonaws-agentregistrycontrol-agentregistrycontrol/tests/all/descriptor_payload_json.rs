// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the descriptor payload a registry record may carry, driven over
//! HTTP through the real router so every assertion goes through routing, deserialization,
//! the handler and serialization.
//!
//! The rule under test: a descriptor's content is serialized as descriptor payload data and
//! validated against the protocol schema of the descriptor's type, so a `data` member whose
//! value is not even JSON is refused with `ValidationException` (HTTP 400) and the record is
//! not written — on a create and on an edit alike, and for the payloads nested under a
//! descriptor's `additionalData` as much as for a descriptor's own. A refused create leaves
//! the registry exactly as it was, so it occupies no name and appears in no listing.
//!
//! The second half of the same rule: content that IS JSON but that no supported version of
//! the descriptor's own protocol schema accepts is refused the same way. The MCP server
//! descriptor is "validated against the MCP protocol schema", so a payload that satisfies
//! none of the supported MCP server schema versions is a 400 that writes nothing, while one
//! that satisfies any of them is accepted, settles at `DRAFT` and is served back unchanged.
//!
//! Every test asserts over a TABLE of inputs, so it holds for a class of requests rather
//! than for one recorded shape: the rows differ along which descriptor of the typed set
//! carries the payload, how deep in that set it sits, what KIND of JSON failure the payload
//! has, and — for the schema match — which member of a server definition is missing, of the
//! wrong type or empty, and which supported version a payload is written for.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The region in the test's SigV4 credential scope.
const REGION: &str = "us-west-2";

/// A signed-looking request, so the router resolves this service's signing name
/// (`agent-registry`). The signature itself is never checked: the test router runs with
/// authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDTESTAGENTREGISTRY/20260101/us-west-2/agent-registry/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0123456789abcdef";

/// How long a wait on a transition the clock owes may take before the test gives up.
/// Generous: the wait is on the CONDITION, so this bounds only a hang.
const TRANSITION_DEADLINE: Duration = Duration::from_secs(30);

/// How long to wait between two looks while polling for a condition.
const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// The error class a request refused over one of its fields carries, from the model's own
/// error list for these operations (`smithy_query(query="operation",
/// operation="CreateRegistryRecord")`).
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// An MCP server definition a supported MCP server schema version accepts: a named server
/// with a description and a version. Used wherever a row's subject is something OTHER than
/// the MCP server descriptor's own content, so the server payload is never what refuses the
/// request.
const MCP_SERVER_DEFINITION: &str =
    r#"{"name":"com.example/checkout","description":"Checkout tools","version":"1.4.2"}"#;

/// How the service refuses an MCP server payload that no supported MCP server schema version
/// accepts: fixed text throughout, naming the descriptor path whose content was matched.
const MCP_SERVER_VERSION_MISMATCH: &str = "mcp.server data does not match any supported version";

/// One HTTP answer, reduced to what these tests assert on.
struct Reply {
    status: u16,
    body: Value,
    /// The wire error code (`x-amzn-errortype`), present only on a failure.
    error_code: Option<String>,
}

impl Reply {
    fn str_field(&self, name: &str) -> &str {
        self.body[name].as_str().unwrap_or_default()
    }

    fn message(&self) -> String {
        self.body["message"]
            .as_str()
            .or_else(|| self.body["Message"].as_str())
            .unwrap_or_default()
            .to_string()
    }
}

fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_agentregistrycontrol_agentregistrycontrol::EmulatedService::new(
            engine,
        ),
    );
    router
}

async fn call(router: &MultiServiceRouter, method: &str, path: &str, body: Option<Value>) -> Reply {
    let payload = match &body {
        Some(value) => bytes::Bytes::from(serde_json::to_vec(value).expect("serializable body")),
        None => bytes::Bytes::new(),
    };
    let request = http::Request::builder()
        .method(method)
        .uri(format!(
            "https://agent-registry-control.{REGION}.amazonaws.com{path}"
        ))
        .header("authorization", AUTHORIZATION)
        .header("content-type", "application/json")
        .body(payload)
        .expect("a valid request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_code = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(|value| value.rsplit('#').next().unwrap_or(value).to_string());
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    Reply {
        status,
        body,
        error_code,
    }
}

/// Percent-encode a path segment the way an SDK encodes a non-greedy `@httpLabel`.
fn encode_label(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

fn id_of(arn: &str) -> &str {
    arn.rsplit('/').next().unwrap_or(arn)
}

/// A name of `RegistryName` / `RegistryRecordName` shape derived from a case label, so each
/// row works on its own resources.
fn name_from(prefix: &str, case: &str) -> String {
    let sanitized: String = case
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character
            } else {
                '-'
            }
        })
        .collect();
    format!("{prefix}-{sanitized}")
}

async fn create_record(router: &MultiServiceRouter, registry: &str, body: Value) -> Reply {
    call(
        router,
        "POST",
        &format!("/registries/{}/records", encode_label(registry)),
        Some(body),
    )
    .await
}

async fn get_record(router: &MultiServiceRouter, registry: &str, record: &str) -> Reply {
    call(
        router,
        "GET",
        &format!(
            "/registries/{}/records/{}",
            encode_label(registry),
            encode_label(record)
        ),
        None,
    )
    .await
}

async fn update_record(
    router: &MultiServiceRouter,
    registry: &str,
    record: &str,
    body: Value,
) -> Reply {
    call(
        router,
        "PATCH",
        &format!(
            "/registries/{}/records/{}",
            encode_label(registry),
            encode_label(record)
        ),
        Some(body),
    )
    .await
}

async fn list_records(router: &MultiServiceRouter, registry: &str, body: Value) -> Reply {
    call(
        router,
        "POST",
        &format!("/registries/{}/records-list", encode_label(registry)),
        Some(body),
    )
    .await
}

/// Look at a record until it reports `wanted`, or give up at the deadline. The wait is on
/// the CONDITION — a transition the clock owes — never on a fixed span.
async fn record_reaching(
    router: &MultiServiceRouter,
    case: &str,
    registry: &str,
    record: &str,
    wanted: &str,
) -> Reply {
    let started = std::time::Instant::now();
    loop {
        let reply = get_record(router, registry, record).await;
        if reply.str_field("status") == wanted {
            return reply;
        }
        assert!(
            started.elapsed() < TRANSITION_DEADLINE,
            "case {case:?}: the record never reached {wanted}, last answer {} {:?}",
            reply.status,
            reply.body
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// A `READY` registry, created through this service's own operations.
async fn ready_registry(router: &MultiServiceRouter, case: &str) -> String {
    let created = call(
        router,
        "POST",
        "/registries",
        Some(json!({ "name": name_from("payload", case) })),
    )
    .await;
    assert_eq!(
        created.status, 202,
        "case {case:?}: CreateRegistry must answer 202, got {} {:?}",
        created.status, created.body
    );
    let registry_id = id_of(created.str_field("registryArn")).to_string();

    let started = std::time::Instant::now();
    loop {
        let reply = call(
            router,
            "GET",
            &format!("/registries/{}", encode_label(&registry_id)),
            None,
        )
        .await;
        if reply.str_field("status") == "READY" {
            return registry_id;
        }
        assert!(
            started.elapsed() < TRANSITION_DEADLINE,
            "case {case:?}: the registry never became READY, last answer {} {:?}",
            reply.status,
            reply.body
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// The record type whose descriptor `descriptors` belongs to, and the descriptors value
/// itself: the member names are `Descriptors`' own (`smithy_query(query="shape",
/// shape="Descriptors")`), and the payload sits in each descriptor's `data` member.
struct Payload {
    record_type: &'static str,
    descriptors: Value,
}

/// A row of the invalid-payload table: the case, the descriptors the request carries, and
/// the refusal the service words for that KIND of JSON failure.
struct InvalidRow {
    case: &'static str,
    payload: Payload,
    message: &'static str,
}

/// The refusals, one per kind of JSON failure a payload can have: a bare word where a value
/// belongs, a character that can begin no value, one that belongs where a member name does,
/// a missing separator, and input that ends before the document does (for each container
/// kind). The readings are the tree's shared ones
/// (`services/common/src/json_parse_fault.rs`), so a row that is worded differently there is
/// worded differently for every service that quotes them.
fn invalid_rows() -> Vec<InvalidRow> {
    vec![
        InvalidRow {
            case: "a bare word where a value belongs",
            payload: Payload {
                record_type: "CUSTOM",
                descriptors: json!({ "custom": { "data": "not json" } }),
            },
            message: "data is not valid JSON: Unrecognized token 'not': was expecting (JSON \
                      String, Number, Array, Object or token 'null', 'true' or 'false')",
        },
        InvalidRow {
            case: "a bare word as an object member's value",
            payload: Payload {
                record_type: "MCP",
                descriptors: json!({ "mcpServer": { "data": "{\"tools\": nope}" } }),
            },
            message: "data is not valid JSON: Unrecognized token 'nope': was expecting (JSON \
                      String, Number, Array, Object or token 'null', 'true' or 'false')",
        },
        InvalidRow {
            case: "a payload nested in additionalData",
            payload: Payload {
                record_type: "MCP",
                descriptors: json!({
                    "mcpServer": {
                        "data": MCP_SERVER_DEFINITION,
                        "additionalData": { "tools": { "data": "undefined" } }
                    }
                }),
            },
            message: "data is not valid JSON: Unrecognized token 'undefined': was expecting \
                      (JSON String, Number, Array, Object or token 'null', 'true' or 'false')",
        },
        InvalidRow {
            case: "an object that is never closed",
            payload: Payload {
                record_type: "AGENT",
                descriptors: json!({ "a2aAgentCard": { "data": "{\"name\": \"checkout\"" } }),
            },
            message: "data is not valid JSON: Unexpected end-of-input: expected close marker \
                      for Object",
        },
        InvalidRow {
            case: "an array that is never closed",
            payload: Payload {
                record_type: "SKILL",
                descriptors: json!({ "agentSkillsDefinition": { "data": "[1, 2" } }),
            },
            message: "data is not valid JSON: Unexpected end-of-input: expected close marker \
                      for Array",
        },
        InvalidRow {
            case: "a string that is never closed",
            payload: Payload {
                record_type: "CUSTOM",
                descriptors: json!({ "custom": { "data": "{\"name\": \"checkout" } }),
            },
            message: "data is not valid JSON: Unexpected end-of-input: expected close marker \
                      for String",
        },
        InvalidRow {
            case: "a character that begins no value",
            payload: Payload {
                record_type: "CUSTOM",
                descriptors: json!({ "custom": { "data": "@" } }),
            },
            message: "data is not valid JSON: Unexpected character ('@' (code 64)): was \
                      expecting a valid value (JSON String, Number, Array, Object or token \
                      'null', 'true' or 'false')",
        },
        InvalidRow {
            case: "a member name that is not quoted",
            payload: Payload {
                record_type: "CUSTOM",
                descriptors: json!({ "custom": { "data": "{tools: []}" } }),
            },
            message: "data is not valid JSON: Unexpected character ('t' (code 116)): was \
                      expecting double-quote to start field name",
        },
        InvalidRow {
            case: "two members with no comma between them",
            payload: Payload {
                record_type: "CUSTOM",
                descriptors: json!({ "custom": { "data": "{\"a\": 1 \"b\": 2}" } }),
            },
            message: "data is not valid JSON: Unexpected character ('\"' (code 34)): was \
                      expecting comma to separate Object entries",
        },
        InvalidRow {
            case: "a payload nested under a skills definition",
            payload: Payload {
                record_type: "SKILL",
                descriptors: json!({
                    "agentSkillsDefinition": {
                        "data": "{\"skills\":[]}",
                        "additionalData": { "skillMd": { "data": "# how to check out" } }
                    }
                }),
            },
            message: "data is not valid JSON: Unexpected character ('#' (code 35)): was \
                      expecting a valid value (JSON String, Number, Array, Object or token \
                      'null', 'true' or 'false')",
        },
        InvalidRow {
            case: "a signature agent card payload that is not JSON",
            payload: Payload {
                record_type: "WEB_BOT",
                descriptors: json!({
                    "signatureAgentCard": {
                        "data": "not json",
                        "additionalData": { "category": "ai_search" }
                    }
                }),
            },
            message: "data is not valid JSON: Unrecognized token 'not': was expecting (JSON \
                      String, Number, Array, Object or token 'null', 'true' or 'false')",
        },
    ]
}

/// Payloads that ARE JSON, one per JSON value kind — including the string a text document
/// (markdown, say) is serialized as — so the rule refuses non-JSON without refusing any
/// JSON.
fn valid_payloads() -> Vec<(&'static str, Payload)> {
    vec![
        (
            "an object",
            Payload {
                record_type: "CUSTOM",
                descriptors: json!({ "custom": { "data": "{\"anything\":true}" } }),
            },
        ),
        (
            "an array",
            Payload {
                record_type: "MCP",
                // The array sits in the payload the MCP TOOLS descriptor carries: the
                // server descriptor's own content has to be a server definition a supported
                // MCP server schema version accepts, which the tests below are about.
                descriptors: json!({
                    "mcpServer": {
                        "data": MCP_SERVER_DEFINITION,
                        "additionalData": { "tools": { "data": "[{\"name\":\"search\"}]" } }
                    }
                }),
            },
        ),
        (
            "a string",
            Payload {
                record_type: "SKILL",
                descriptors: json!({
                    "agentSkillsDefinition": {
                        "data": "{\"skills\":[]}",
                        "additionalData": { "skillMd": { "data": "\"# how to check out\"" } }
                    }
                }),
            },
        ),
        (
            "a number",
            Payload {
                record_type: "AGENT",
                descriptors: json!({ "a2aAgentCard": { "data": "17" } }),
            },
        ),
        (
            "a boolean",
            Payload {
                record_type: "CUSTOM",
                descriptors: json!({ "custom": { "data": "true" } }),
            },
        ),
        (
            "the null literal",
            Payload {
                record_type: "CUSTOM",
                descriptors: json!({ "custom": { "data": "null" } }),
            },
        ),
        (
            "no payload at all, a source-only descriptor",
            Payload {
                record_type: "GATEWAY",
                descriptors: json!({
                    "http": { "source": { "fromUrl": { "url": "https://example.test/gw" } } }
                }),
            },
        ),
    ]
}

#[tokio::test]
async fn a_create_whose_descriptor_payload_is_not_json_is_refused_and_writes_nothing() {
    for row in invalid_rows() {
        let case = row.case;
        let router = router();
        let registry_id = ready_registry(&router, case).await;
        let record_name = name_from("rec", case);

        let refused = create_record(
            &router,
            &registry_id,
            json!({
                "name": record_name,
                "recordType": row.payload.record_type,
                "descriptors": row.payload.descriptors,
            }),
        )
        .await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: a payload that is not JSON is the modelled 400, got {} {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code.as_deref(),
            Some(VALIDATION_EXCEPTION),
            "case {case:?}: expected error code {VALIDATION_EXCEPTION}, got {:?}",
            refused.error_code
        );
        assert_eq!(
            refused.message(),
            row.message,
            "case {case:?}: expected the refusal to be worded {:?}, got {:?}",
            row.message,
            refused.message()
        );

        // Nothing was written: the registry's listing is empty, with and without a filter
        // on the name the refused request asked for.
        let listed = list_records(&router, &registry_id, json!({})).await;
        assert_eq!(
            listed.body["registryRecords"].as_array().map(Vec::len),
            Some(0),
            "case {case:?}: a refused create leaves the listing empty, got {:?}",
            listed.body
        );
        let filtered = list_records(
            &router,
            &registry_id,
            json!({ "filters": [{ "name": "name", "values": [name_from("rec", case)] }] }),
        )
        .await;
        assert_eq!(
            filtered.body["registryRecords"].as_array().map(Vec::len),
            Some(0),
            "case {case:?}: the refused record is in no listing of its name, got {:?}",
            filtered.body
        );

        // And it took no name: the same name is still free for a payload that IS JSON.
        let accepted = create_record(
            &router,
            &registry_id,
            json!({
                "name": name_from("rec", case),
                "recordType": "CUSTOM",
                "descriptors": json!({ "custom": { "data": "{\"anything\":true}" } }),
            }),
        )
        .await;
        assert_eq!(
            accepted.status, 202,
            "case {case:?}: the refused create took no name, got {} {:?}",
            accepted.status, accepted.body
        );
    }
}

#[tokio::test]
async fn a_create_whose_descriptor_payload_is_json_is_accepted_and_served_back() {
    let router = router();
    let registry_id = ready_registry(&router, "every json payload kind").await;

    for (case, payload) in valid_payloads() {
        let created = create_record(
            &router,
            &registry_id,
            json!({
                "name": name_from("rec", case),
                "recordType": payload.record_type,
                "descriptors": payload.descriptors.clone(),
            }),
        )
        .await;
        assert_eq!(
            created.status, 202,
            "case {case:?}: a JSON payload is accepted, got {} {:?}",
            created.status, created.body
        );
        let record_id = id_of(created.str_field("recordArn")).to_string();
        let read = record_reaching(&router, case, &registry_id, &record_id, "DRAFT").await;
        assert_eq!(
            read.body["descriptors"], payload.descriptors,
            "case {case:?}: the stored descriptors are the ones supplied, got {:?}",
            read.body["descriptors"]
        );
    }
}

#[tokio::test]
async fn an_edit_whose_descriptor_payload_is_not_json_is_refused_and_changes_nothing() {
    // The same rule on the edit path, where the payload arrives inside the update wrappers:
    // the rows are the create table's, each patching the descriptor its payload belongs to.
    for row in invalid_rows() {
        let case = row.case;
        let router = router();
        let registry_id = ready_registry(&router, case).await;
        let created = create_record(
            &router,
            &registry_id,
            json!({
                "name": name_from("rec", case),
                "recordType": "CUSTOM",
                "descriptors": { "custom": { "data": "{\"created\":true}" } },
            }),
        )
        .await;
        assert_eq!(
            created.status, 202,
            "case {case:?}: the record under edit is created, got {} {:?}",
            created.status, created.body
        );
        let record_id = id_of(created.str_field("recordArn")).to_string();
        let before = record_reaching(&router, case, &registry_id, &record_id, "DRAFT").await;

        // The patch wrapper form: each level carries the value under `optionalValue`, as the
        // update shapes declare (`smithy_query(query="shape", shape="UpdatedDescriptors")`).
        let patch = wrap_descriptors(&row.payload.descriptors);
        let refused = update_record(
            &router,
            &registry_id,
            &record_id,
            json!({ "descriptors": patch }),
        )
        .await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: an edit to a non-JSON payload is the modelled 400, got {} {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code.as_deref(),
            Some(VALIDATION_EXCEPTION),
            "case {case:?}: expected error code {VALIDATION_EXCEPTION}, got {:?}",
            refused.error_code
        );
        assert_eq!(
            refused.message(),
            row.message,
            "case {case:?}: expected the refusal to be worded {:?}, got {:?}",
            row.message,
            refused.message()
        );

        // The refused edit changed nothing: the record keeps its descriptors and its status.
        let after = get_record(&router, &registry_id, &record_id).await;
        assert_eq!(
            (
                after.body["descriptors"].clone(),
                after.str_field("status").to_string()
            ),
            (
                before.body["descriptors"].clone(),
                before.str_field("status").to_string()
            ),
            "case {case:?}: the refused edit left the record alone, got {:?}",
            after.body
        );
    }
}

#[tokio::test]
async fn an_edit_whose_descriptor_payload_is_json_is_applied() {
    let router = router();
    let registry_id = ready_registry(&router, "edits with json payloads").await;

    // Rows differ along the JSON value kind the patch writes into the payload.
    for (case, written) in [
        ("an object", "{\"patched\":true}"),
        ("an array", "[1, 2, 3]"),
        ("a string", "\"# how to check out\""),
        ("a number", "17"),
        ("the null literal", "null"),
    ] {
        let created = create_record(
            &router,
            &registry_id,
            json!({
                "name": name_from("edit", case),
                "recordType": "CUSTOM",
                "descriptors": { "custom": { "data": "{\"created\":true}" } },
            }),
        )
        .await;
        assert_eq!(
            created.status, 202,
            "case {case:?}: the record under edit is created, got {} {:?}",
            created.status, created.body
        );
        let record_id = id_of(created.str_field("recordArn")).to_string();
        record_reaching(&router, case, &registry_id, &record_id, "DRAFT").await;

        let edited = update_record(
            &router,
            &registry_id,
            &record_id,
            json!({
                "descriptors": {
                    "optionalValue": {
                        "custom": { "optionalValue": { "data": { "optionalValue": written } } }
                    }
                }
            }),
        )
        .await;
        assert_eq!(
            edited.status, 202,
            "case {case:?}: a JSON payload is accepted on an edit, got {} {:?}",
            edited.status, edited.body
        );
        let settled = record_reaching(&router, case, &registry_id, &record_id, "DRAFT").await;
        assert_eq!(
            settled.body["descriptors"]["custom"]["data"],
            Value::from(written),
            "case {case:?}: the edited payload is what a later read serves, got {:?}",
            settled.body["descriptors"]
        );
    }
}

/// The patch form of a `Descriptors` value: every level of the update shapes carries its
/// value under `optionalValue` (`smithy_query(query="shape", shape="UpdatedDescriptors")`),
/// so a create's descriptors value can be re-used as the body of an edit.
fn wrap_descriptors(descriptors: &Value) -> Value {
    json!({ "optionalValue": wrap_members(descriptors) })
}

/// Wrap every member of an update level: a nested object is itself wrapped, member by
/// member, and a leaf value is wrapped as it stands.
fn wrap_members(level: &Value) -> Value {
    match level {
        Value::Object(members) => Value::Object(
            members
                .iter()
                .map(|(name, value)| {
                    (
                        name.clone(),
                        json!({ "optionalValue": wrap_members(value) }),
                    )
                })
                .collect(),
        ),
        leaf => leaf.clone(),
    }
}

/// MCP server payloads that ARE JSON documents but that no supported MCP server schema
/// version accepts. The rows differ along which member of a server definition is missing, is
/// of the wrong JSON type or is empty; along the JSON value kind the payload is, since a
/// server definition is an object and no other kind can be one; and along a document that is
/// a DIFFERENT descriptor's content (a tools document) rather than a server definition.
fn off_schema_server_payloads() -> Vec<(&'static str, &'static str)> {
    vec![
        (
            "an object carrying none of the server members",
            r#"{"x": 1}"#,
        ),
        ("an object with no members at all", "{}"),
        ("a name and nothing else", r#"{"name":"com.example/checkout"}"#),
        (
            "a definition with no description",
            r#"{"name":"com.example/checkout","version":"1.4.2"}"#,
        ),
        (
            "a definition with no version",
            r#"{"name":"com.example/checkout","description":"Checkout tools"}"#,
        ),
        (
            "a version_detail carrying no version",
            r#"{"name":"com.example/checkout","description":"Checkout tools","version_detail":{}}"#,
        ),
        (
            "a name that is empty",
            r#"{"name":"","description":"Checkout tools","version":"1.4.2"}"#,
        ),
        (
            "a version that is not a string",
            r#"{"name":"com.example/checkout","description":"Checkout tools","version":2}"#,
        ),
        (
            "a tools document where a server definition belongs",
            r#"{"tools":[]}"#,
        ),
        (
            "an array of server definitions",
            r#"[{"name":"com.example/checkout","description":"Checkout tools","version":"1.4.2"}]"#,
        ),
        ("a string", r#""com.example/checkout""#),
        ("a number", "17"),
        ("a boolean", "true"),
        ("the null literal", "null"),
    ]
}

/// MCP server payloads a supported MCP server schema version DOES accept: one per supported
/// version (they differ in where the definition's version sits), one carrying members beyond
/// the ones a version requires, and one stating the payload's own schema version.
fn on_schema_server_payloads() -> Vec<(&'static str, &'static str)> {
    vec![
        ("a server definition carrying its version", MCP_SERVER_DEFINITION),
        (
            "a server definition whose version sits in version_detail",
            r#"{"name":"com.example/search","description":"Search tools","version_detail":{"version":"0.9.0"}}"#,
        ),
        (
            "a definition carrying members beyond the required ones",
            r#"{"name":"com.example/checkout","description":"Checkout tools","version":"1.4.2","websiteUrl":"https://example.test/checkout","packages":[],"repository":{"url":"https://example.test/checkout.git","source":"example"}}"#,
        ),
    ]
}

/// The descriptor sets a create is accepted with, built around the payloads a supported
/// version accepts: the payload on its own, beside the tools the server exposes, and with
/// the payload's schema version stated.
fn on_schema_descriptor_sets() -> Vec<(&'static str, Value)> {
    let mut sets: Vec<(&'static str, Value)> = on_schema_server_payloads()
        .into_iter()
        .map(|(case, payload)| (case, json!({ "mcpServer": { "data": payload } })))
        .collect();
    sets.push((
        "a definition beside the tools the server exposes",
        json!({
            "mcpServer": {
                "data": MCP_SERVER_DEFINITION,
                "additionalData": { "tools": { "data": "[{\"name\":\"search\"}]" } }
            }
        }),
    ));
    sets.push((
        "a definition with the payload's schema version stated",
        json!({
            "mcpServer": { "data": MCP_SERVER_DEFINITION, "dataSchemaVersion": "2025-09-29" }
        }),
    ));
    sets
}

/// The patch body that writes `payload` into the stored MCP server descriptor's content.
fn mcp_server_data_patch(payload: &str) -> Value {
    json!({
        "descriptors": {
            "optionalValue": {
                "mcpServer": { "optionalValue": { "data": { "optionalValue": payload } } }
            }
        }
    })
}

#[tokio::test]
async fn a_create_whose_mcp_server_payload_matches_no_schema_version_is_refused_and_writes_nothing()
{
    let router = router();
    let registry_id = ready_registry(&router, "off schema mcp servers").await;

    for (case, payload) in off_schema_server_payloads() {
        let record_name = name_from("rec", case);
        let refused = create_record(
            &router,
            &registry_id,
            json!({
                "name": record_name,
                "recordType": "MCP",
                "descriptors": { "mcpServer": { "data": payload } },
            }),
        )
        .await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: a payload no supported MCP server schema version accepts is the \
             modelled 400, got {} {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code.as_deref(),
            Some(VALIDATION_EXCEPTION),
            "case {case:?}: expected error code {VALIDATION_EXCEPTION}, got {:?}",
            refused.error_code
        );
        assert_eq!(
            refused.message(),
            MCP_SERVER_VERSION_MISMATCH,
            "case {case:?}: expected the refusal to be worded {MCP_SERVER_VERSION_MISMATCH:?}, \
             got {:?}",
            refused.message()
        );

        // Nothing was written: no record of the requested name is in the registry.
        let filtered = list_records(
            &router,
            &registry_id,
            json!({ "filters": [{ "name": "name", "values": [record_name.clone()] }] }),
        )
        .await;
        assert_eq!(
            filtered.body["registryRecords"].as_array().map(Vec::len),
            Some(0),
            "case {case:?}: the refused record is in no listing of its name, got {:?}",
            filtered.body
        );

        // And it took no name: the same name is still free for a definition a version takes.
        let accepted = create_record(
            &router,
            &registry_id,
            json!({
                "name": record_name,
                "recordType": "MCP",
                "descriptors": { "mcpServer": { "data": MCP_SERVER_DEFINITION } },
            }),
        )
        .await;
        assert_eq!(
            accepted.status, 202,
            "case {case:?}: the refused create took no name, got {} {:?}",
            accepted.status, accepted.body
        );
    }
}

#[tokio::test]
async fn a_create_whose_mcp_server_payload_matches_a_schema_version_is_accepted_and_served_back() {
    let router = router();
    let registry_id = ready_registry(&router, "on schema mcp servers").await;

    for (case, descriptors) in on_schema_descriptor_sets() {
        let created = create_record(
            &router,
            &registry_id,
            json!({
                "name": name_from("rec", case),
                "recordType": "MCP",
                "descriptors": descriptors.clone(),
            }),
        )
        .await;
        assert_eq!(
            created.status, 202,
            "case {case:?}: a payload a supported version accepts is accepted, got {} {:?}",
            created.status, created.body
        );
        let record_id = id_of(created.str_field("recordArn")).to_string();
        let read = record_reaching(&router, case, &registry_id, &record_id, "DRAFT").await;
        assert_eq!(
            read.str_field("recordType"),
            "MCP",
            "case {case:?}: the record is read back as an MCP record, got {:?}",
            read.body
        );
        assert_eq!(
            read.body["descriptors"], descriptors,
            "case {case:?}: the stored descriptors are the ones supplied, got {:?}",
            read.body["descriptors"]
        );
    }
}

#[tokio::test]
async fn an_edit_whose_mcp_server_payload_matches_no_schema_version_is_refused_and_changes_nothing()
{
    let router = router();
    let registry_id = ready_registry(&router, "edited off schema mcp").await;

    for (case, payload) in off_schema_server_payloads() {
        let created = create_record(
            &router,
            &registry_id,
            json!({
                "name": name_from("edit", case),
                "recordType": "MCP",
                "descriptors": { "mcpServer": { "data": MCP_SERVER_DEFINITION } },
            }),
        )
        .await;
        assert_eq!(
            created.status, 202,
            "case {case:?}: the record under edit is created, got {} {:?}",
            created.status, created.body
        );
        let record_id = id_of(created.str_field("recordArn")).to_string();
        let before = record_reaching(&router, case, &registry_id, &record_id, "DRAFT").await;

        let refused = update_record(
            &router,
            &registry_id,
            &record_id,
            mcp_server_data_patch(payload),
        )
        .await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: an edit to a payload no supported version accepts is the modelled \
             400, got {} {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code.as_deref(),
            Some(VALIDATION_EXCEPTION),
            "case {case:?}: expected error code {VALIDATION_EXCEPTION}, got {:?}",
            refused.error_code
        );
        assert_eq!(
            refused.message(),
            MCP_SERVER_VERSION_MISMATCH,
            "case {case:?}: expected the refusal to be worded {MCP_SERVER_VERSION_MISMATCH:?}, \
             got {:?}",
            refused.message()
        );

        // The refused edit changed nothing: the record keeps its descriptors and its status.
        let after = get_record(&router, &registry_id, &record_id).await;
        assert_eq!(
            (
                after.body["descriptors"].clone(),
                after.str_field("status").to_string()
            ),
            (
                before.body["descriptors"].clone(),
                before.str_field("status").to_string()
            ),
            "case {case:?}: the refused edit left the record alone, got {:?}",
            after.body
        );
    }
}

#[tokio::test]
async fn an_edit_to_an_mcp_server_payload_a_schema_version_accepts_is_applied() {
    let router = router();
    let registry_id = ready_registry(&router, "edited on schema mcp").await;

    for (case, payload) in on_schema_server_payloads() {
        let created = create_record(
            &router,
            &registry_id,
            json!({
                "name": name_from("edit", case),
                "recordType": "MCP",
                "descriptors": { "mcpServer": { "data": "{\"name\":\"com.example/seed\",\
                                  \"description\":\"Seed tools\",\"version\":\"0.1.0\"}" } },
            }),
        )
        .await;
        assert_eq!(
            created.status, 202,
            "case {case:?}: the record under edit is created, got {} {:?}",
            created.status, created.body
        );
        let record_id = id_of(created.str_field("recordArn")).to_string();
        record_reaching(&router, case, &registry_id, &record_id, "DRAFT").await;

        let edited = update_record(
            &router,
            &registry_id,
            &record_id,
            mcp_server_data_patch(payload),
        )
        .await;
        assert_eq!(
            edited.status, 202,
            "case {case:?}: a payload a supported version accepts is accepted on an edit, got \
             {} {:?}",
            edited.status, edited.body
        );
        let settled = record_reaching(&router, case, &registry_id, &record_id, "DRAFT").await;
        assert_eq!(
            settled.body["descriptors"]["mcpServer"]["data"],
            Value::from(payload),
            "case {case:?}: the edited payload is what a later read serves, got {:?}",
            settled.body["descriptors"]
        );
    }
}

/// The `reason` a field-validation refusal of this service carries
/// (`smithy_query(query="shape", shape="ValidationException")`: `reason` ->
/// `ValidationExceptionReason`, whose modelled default is `FieldValidationFailed`).
const FIELD_VALIDATION_FAILED: &str = "FieldValidationFailed";

/// An MCP server payload that is a JSON document no supported MCP server schema version
/// accepts: an object carrying none of the members a server definition is told by. Used by
/// the rows below whose subject is what ELSE the request carries, so the payload is the one
/// thing about them that is off schema.
const OFF_SCHEMA_SERVER: &str = r#"{"x": 1}"#;

/// A client token of `ClientToken`'s shape (`smithy_query(query="operation",
/// operation="CreateRegistryRecord")`: `@length 33..256`,
/// `^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,256}$`), derived from a case label so each row presents
/// its own.
fn token(seed: &str) -> String {
    let alphanumeric: String = seed
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .collect();
    let padded = format!("{alphanumeric}{}", "0".repeat(40));
    padded[..40].to_string()
}

/// One create whose MCP server descriptor carries content no supported version accepts: the
/// case, the record type the request names and the descriptor set, plus the members the
/// request carries BEYOND its name, its record type and its descriptors.
struct OffSchemaCreate {
    case: &'static str,
    record_type: &'static str,
    descriptors: Value,
    beyond: Value,
}

/// The creates an off-schema MCP server payload can arrive in. Every row carries the SAME
/// off-schema server content and differs along what else the request holds — a second
/// descriptor of the typed set beside the server one, the tools nested under the server
/// descriptor, a stated `dataSchemaVersion`, the optional members of the create
/// (`displayName`, `description`, `recordVersion`, `tags`, `customMetadata`), an idempotency
/// token, and a `recordType` other than the one the server descriptor belongs to — because
/// what is matched against the MCP server schema versions is the content of
/// `descriptors.mcpServer.data`, whatever else accompanies it.
fn off_schema_creates() -> Vec<OffSchemaCreate> {
    vec![
        OffSchemaCreate {
            case: "the server payload on its own",
            record_type: "MCP",
            descriptors: json!({ "mcpServer": { "data": OFF_SCHEMA_SERVER } }),
            beyond: json!({}),
        },
        OffSchemaCreate {
            case: "a server payload beside the tools the server exposes",
            record_type: "MCP",
            descriptors: json!({
                "mcpServer": {
                    "data": OFF_SCHEMA_SERVER,
                    "additionalData": { "tools": { "data": "[{\"name\":\"search\"}]" } }
                }
            }),
            beyond: json!({}),
        },
        OffSchemaCreate {
            case: "a server payload stating a schema version of its own",
            record_type: "MCP",
            descriptors: json!({
                "mcpServer": { "data": OFF_SCHEMA_SERVER, "dataSchemaVersion": "2025-09-29" }
            }),
            beyond: json!({}),
        },
        OffSchemaCreate {
            case: "a server payload beside a custom descriptor that is JSON",
            record_type: "MCP",
            descriptors: json!({
                "mcpServer": { "data": OFF_SCHEMA_SERVER },
                "custom": { "data": "{\"anything\":true}" }
            }),
            beyond: json!({}),
        },
        OffSchemaCreate {
            case: "a server payload beside an agent card that is JSON",
            record_type: "MCP",
            descriptors: json!({
                "mcpServer": { "data": OFF_SCHEMA_SERVER },
                "a2aAgentCard": { "data": "{\"name\":\"checkout\"}" }
            }),
            beyond: json!({}),
        },
        OffSchemaCreate {
            case: "a create with every optional member of its own filled in",
            record_type: "MCP",
            descriptors: json!({ "mcpServer": { "data": OFF_SCHEMA_SERVER } }),
            beyond: json!({
                "displayName": "Checkout tools",
                "description": "the tools a checkout exposes",
                "recordVersion": "2.0.1",
                "tags": { "team": "checkout" },
                "customMetadata": { "owner": "checkout-team" }
            }),
        },
        OffSchemaCreate {
            case: "a create presenting an idempotency token",
            record_type: "MCP",
            descriptors: json!({ "mcpServer": { "data": OFF_SCHEMA_SERVER } }),
            beyond: json!({ "clientToken": token("an-off-schema-create-with-a-token") }),
        },
        OffSchemaCreate {
            case: "a server payload under a record type of another descriptor",
            record_type: "CUSTOM",
            descriptors: json!({
                "mcpServer": { "data": OFF_SCHEMA_SERVER },
                "custom": { "data": "{\"anything\":true}" }
            }),
            beyond: json!({}),
        },
    ]
}

/// The body of one row's create: its own name, its record type, its descriptors and whatever
/// else the row carries.
fn off_schema_body(row: &OffSchemaCreate) -> Value {
    let mut body = json!({
        "name": name_from("env", row.case),
        "recordType": row.record_type,
        "descriptors": row.descriptors.clone(),
    });
    let members = body.as_object_mut().expect("a create body is an object");
    for (member, value) in row
        .beyond
        .as_object()
        .expect("the members beyond the three are an object")
    {
        members.insert(member.clone(), value.clone());
    }
    body
}

/// Every record the registry holds, whatever its name: the listing with no filters, so a
/// record a refused create left behind is seen wherever it sits.
async fn all_records(router: &MultiServiceRouter, registry: &str) -> Vec<Value> {
    let listed = list_records(router, registry, json!({})).await;
    assert_eq!(
        listed.status, 200,
        "ListRegistryRecords must answer 200, got {} {:?}",
        listed.status, listed.body
    );
    listed.body["registryRecords"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

#[tokio::test]
async fn an_off_schema_mcp_server_create_is_refused_whatever_else_the_request_carries() {
    let router = router();
    let registry_id = ready_registry(&router, "off schema envelopes").await;

    for row in off_schema_creates() {
        let case = row.case;
        let refused = create_record(&router, &registry_id, off_schema_body(&row)).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: a payload no supported MCP server schema version accepts is the \
             modelled 400, got {} {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code.as_deref(),
            Some(VALIDATION_EXCEPTION),
            "case {case:?}: expected error code {VALIDATION_EXCEPTION}, got {:?}",
            refused.error_code
        );
        assert_eq!(
            refused.message(),
            MCP_SERVER_VERSION_MISMATCH,
            "case {case:?}: expected the refusal to be worded {MCP_SERVER_VERSION_MISMATCH:?}, \
             got {:?}",
            refused.message()
        );
        assert_eq!(
            refused.str_field("reason"),
            FIELD_VALIDATION_FAILED,
            "case {case:?}: expected reason {FIELD_VALIDATION_FAILED}, got {:?}",
            refused.body
        );

        // The refusal wrote nothing ANYWHERE in the registry, not merely nothing under the
        // name the row asked for.
        let held = all_records(&router, &registry_id).await;
        assert!(
            held.is_empty(),
            "case {case:?}: a refused create leaves the registry empty, got {held:?}"
        );
    }

    // And the listing that stayed empty above does report what IS written: one create a
    // supported version accepts, and the registry holds exactly that record.
    let accepted = create_record(
        &router,
        &registry_id,
        json!({
            "name": name_from("env", "the control create"),
            "recordType": "MCP",
            "descriptors": { "mcpServer": { "data": MCP_SERVER_DEFINITION } },
        }),
    )
    .await;
    assert_eq!(
        accepted.status, 202,
        "a payload a supported version accepts is accepted, got {} {:?}",
        accepted.status, accepted.body
    );
    let held = all_records(&router, &registry_id).await;
    assert_eq!(
        held.len(),
        1,
        "the registry holds the one record the accepted create wrote, got {held:?}"
    );
    assert_eq!(
        held[0]["name"],
        Value::from(name_from("env", "the control create")),
        "the record the registry holds is the accepted one, got {:?}",
        held[0]
    );
    let record_id = id_of(accepted.str_field("recordArn")).to_string();
    record_reaching(
        &router,
        "the control create",
        &registry_id,
        &record_id,
        "DRAFT",
    )
    .await;
}

#[tokio::test]
async fn a_refused_off_schema_create_leaves_its_idempotency_token_unbound() {
    let router = router();
    let registry_id = ready_registry(&router, "off schema tokens").await;
    let mut last_accepted: Option<(&str, String)> = None;

    for (case, payload) in off_schema_server_payloads() {
        let record_name = name_from("tok", case);
        let client_token = token(case);
        let refused_body = json!({
            "name": record_name,
            "recordType": "MCP",
            "descriptors": { "mcpServer": { "data": payload } },
            "clientToken": client_token,
        });

        // The refusal is the request's, so presenting it twice under one token is refused
        // twice: nothing of the first call was remembered for the token to answer with.
        for attempt in ["the first presentation", "the same request again"] {
            let refused = create_record(&router, &registry_id, refused_body.clone()).await;
            assert_eq!(
                refused.status, 400,
                "case {case:?}, {attempt}: a payload no supported MCP server schema version \
                 accepts is the modelled 400, got {} {:?}",
                refused.status, refused.body
            );
            assert_eq!(
                refused.error_code.as_deref(),
                Some(VALIDATION_EXCEPTION),
                "case {case:?}, {attempt}: expected error code {VALIDATION_EXCEPTION}, got {:?}",
                refused.error_code
            );
            assert_eq!(
                refused.message(),
                MCP_SERVER_VERSION_MISMATCH,
                "case {case:?}, {attempt}: expected the refusal to be worded \
                 {MCP_SERVER_VERSION_MISMATCH:?}, got {:?}",
                refused.message()
            );
        }

        // The token the refused create presented was never bound to it, so the SAME token
        // carrying other parameters is a create of its own rather than a token conflict.
        let accepted_body = json!({
            "name": record_name,
            "recordType": "MCP",
            "descriptors": { "mcpServer": { "data": MCP_SERVER_DEFINITION } },
            "clientToken": client_token,
        });
        let accepted = create_record(&router, &registry_id, accepted_body.clone()).await;
        assert_eq!(
            accepted.status, 202,
            "case {case:?}: the token of a refused create is free for a payload a supported \
             version accepts, got {} {:?}",
            accepted.status, accepted.body
        );
        let record_arn = accepted.str_field("recordArn").to_string();

        // The token IS bound by the create that was accepted: its replay answers with that
        // same record rather than writing a second one.
        let replayed = create_record(&router, &registry_id, accepted_body).await;
        assert_eq!(
            (replayed.status, replayed.str_field("recordArn")),
            (202, record_arn.as_str()),
            "case {case:?}: a replay of the accepted create answers with the record it wrote, \
             got {} {:?}",
            replayed.status,
            replayed.body
        );

        // So the name carries exactly one record: the accepted one, and nothing the refusals
        // left behind.
        let listed = list_records(
            &router,
            &registry_id,
            json!({ "filters": [{ "name": "name", "values": [record_name.clone()] }] }),
        )
        .await;
        let named = listed.body["registryRecords"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            named.len(),
            1,
            "case {case:?}: the name carries only the accepted record, got {named:?}"
        );
        assert_eq!(
            named[0]["recordArn"],
            Value::from(record_arn.clone()),
            "case {case:?}: the record the name carries is the accepted one, got {:?}",
            named[0]
        );
        last_accepted = Some((case, id_of(&record_arn).to_string()));
    }

    // The record the last accepted create wrote settles and serves the payload a supported
    // version accepted, so the accept side of the rule is a write and not just a status.
    let (case, record_id) = last_accepted.expect("the table has rows");
    let settled = record_reaching(&router, case, &registry_id, &record_id, "DRAFT").await;
    assert_eq!(
        settled.body["descriptors"]["mcpServer"]["data"],
        Value::from(MCP_SERVER_DEFINITION),
        "case {case:?}: the accepted payload is what a later read serves, got {:?}",
        settled.body["descriptors"]
    );
}
