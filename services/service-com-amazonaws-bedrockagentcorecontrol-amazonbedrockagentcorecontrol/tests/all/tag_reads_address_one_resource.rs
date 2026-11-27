// SPDX-License-Identifier: CC0-1.0

//! `ListTagsForResource` reports THE ADDRESSED RESOURCE'S OWN tag set, driven end to end
//! over the wire path (a `restJson1` request through the multi-service router into the
//! mounted service), so what is asserted is what a client sees.
//!
//! THE RULE UNDER TEST: the tags a read reports are the map the record of the resource the
//! `resourceArn` names holds, so (a) every resource carries its own set — a write or a
//! removal on one resource never shows on another, not even on a sibling of the same
//! family carrying the same key — and (b) the set belongs to the resource rather than to
//! one configuration of it, so it survives a reconfiguration of that resource through the
//! resource's own update operation.
//!
//! The tables span what the rule generalises over: the resource FAMILY (all five the
//! operation documents — "AgentCore Runtime, Browser, Browser Profile, Code Interpreter
//! tool, and Gateway", `smithy_query(query="doc",shape="ListTagsForResource")`), the
//! relation between two sets held at once (a key only one of them carries, a key both
//! carry with different values, and a sibling carrying no tag at all) and, for the
//! families this service lets a caller reconfigure, the OPERATION that mutates the
//! resource between the write and the read.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use bytes::Bytes;
use percent_encoding::{utf8_percent_encode, AsciiSet, NON_ALPHANUMERIC};
use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_bedrockagentcorecontrol_amazonbedrockagentcorecontrol::{
    state, EmulatedService,
};

/// The Region every row signs for.
const REGION: &str = "us-east-1";

/// A tag set as a test writes and compares it: `TagsMap`'s key-value pairs, ordered so two
/// sets compare by content rather than by the order they were written in.
type Tags = BTreeMap<String, String>;

/// One row of the sibling table: the case, two resources of one family and the tag set
/// written on each.
type SiblingRow = (&'static str, String, String, Tags, Tags);

/// A role ARN inside `RoleArn`'s pattern `^arn:aws(-[^:]+)?:iam::([0-9]{12})?:role/.+$`.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/AgentRole";

fn router() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::default();
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

fn authorization() -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/bedrock-agentcore/aws4_request, \
         SignedHeaders=host, Signature=abc"
    )
}

async fn send(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    let payload = body.map_or_else(Bytes::new, |value| {
        Bytes::from(serde_json::to_vec(value).expect("the request body serializes"))
    });
    let mut request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", authorization())
        .header("host", "bedrock-agentcore-control.amazonaws.com");
    if body.is_some() {
        request = request.header("content-type", "application/json");
    }
    let request = request.body(payload).expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// Percent-encode every byte a URI component may not carry literally, leaving the
/// unreserved set (RFC 3986 `ALPHA / DIGIT / "-" / "." / "_" / "~"`) alone.
fn percent_encoded(value: &str) -> String {
    /// Everything outside RFC 3986's unreserved set is encoded.
    const UNRESERVED: &AsciiSet = &NON_ALPHANUMERIC
        .remove(b'-')
        .remove(b'.')
        .remove(b'_')
        .remove(b'~');
    utf8_percent_encode(value, UNRESERVED).to_string()
}

/// The `/tags/{resourceArn}` path of `arn`, with the label encoded as a client encodes a
/// non-greedy `@httpLabel` (`smithy_query(query="operation",operation="TagResource")`).
fn tags_uri(arn: &str) -> String {
    format!("/tags/{}", percent_encoded(arn))
}

/// `TagResource`: `POST /tags/{resourceArn}` carrying the tag map.
async fn tag(router: &MultiServiceRouter, arn: &str, tags: &Tags) -> (u16, Value) {
    let mut body = Map::new();
    body.insert(
        "tags".to_string(),
        Value::Object(
            tags.iter()
                .map(|(key, value)| (key.clone(), Value::String(value.clone())))
                .collect(),
        ),
    );
    send(router, "POST", &tags_uri(arn), Some(&Value::Object(body))).await
}

/// `ListTagsForResource`: `GET /tags/{resourceArn}`.
async fn list_tags(router: &MultiServiceRouter, arn: &str) -> (u16, Value) {
    send(router, "GET", &tags_uri(arn), None).await
}

/// `UntagResource`: `DELETE /tags/{resourceArn}?tagKeys=…`, the keys on the query string
/// (`smithy_query(query="operation",operation="UntagResource")`).
async fn untag(router: &MultiServiceRouter, arn: &str, keys: &[&str]) -> (u16, Value) {
    let query = keys
        .iter()
        .map(|key| format!("tagKeys={}", percent_encoded(key)))
        .collect::<Vec<_>>()
        .join("&");
    send(router, "DELETE", &format!("{}?{query}", tags_uri(arn)), None).await
}

fn text(body: &Value, member: &str) -> String {
    body.get(member)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The `tags` member of a `ListTagsForResource` answer, as a map a test compares.
fn reported_tags(body: &Value) -> Tags {
    body.get("tags")
        .and_then(Value::as_object)
        .map(|map| {
            map.iter()
                .map(|(key, value)| (key.clone(), value.as_str().unwrap_or_default().to_string()))
                .collect()
        })
        .unwrap_or_default()
}

fn tags_of(pairs: &[(&str, &str)]) -> Tags {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// A container artifact whose URI satisfies `RuntimeContainerUri`'s ECR pattern.
fn container_artifact(tag: &str) -> Value {
    json!({"containerConfiguration": {
        "containerUri": format!("public.ecr.aws/agentcore/demo:{tag}")
    }})
}

/// `CreateAgentRuntime`: `PUT /runtimes/`, answering the runtime's ARN.
async fn create_runtime(router: &MultiServiceRouter, name: &str) -> String {
    let (status, created) = send(
        router,
        "PUT",
        "/runtimes/",
        Some(&json!({
            "agentRuntimeName": name,
            "agentRuntimeArtifact": container_artifact("v1"),
            "roleArn": ROLE_ARN,
        })),
    )
    .await;
    assert_eq!(
        status, 202,
        "setup: the agent runtime {name:?} is created, got {status} ({created})"
    );
    text(&created, "agentRuntimeArn")
}

/// `CreateGateway`: `POST /gateways/`, answering the gateway's ARN.
async fn create_gateway(router: &MultiServiceRouter, name: &str) -> String {
    let (status, created) = send(
        router,
        "POST",
        "/gateways/",
        Some(&gateway_configuration(name)),
    )
    .await;
    assert_eq!(
        status, 202,
        "setup: the gateway {name:?} is created, got {status} ({created})"
    );
    text(&created, "gatewayArn")
}

/// The whole gateway configuration `CreateGateway` and `UpdateGateway` each carry.
fn gateway_configuration(name: &str) -> Value {
    json!({
        "name": name,
        "roleArn": "arn:aws:iam::000000000000:role/GatewayRole",
        "authorizerType": "AWS_IAM",
    })
}

/// `CreateBrowser`: `PUT /browsers`, answering the browser's ARN.
async fn create_browser(router: &MultiServiceRouter, name: &str) -> String {
    let (status, created) = send(
        router,
        "PUT",
        "/browsers",
        Some(&json!({
            "name": name,
            "networkConfiguration": {"networkMode": "PUBLIC"},
        })),
    )
    .await;
    assert_eq!(
        status, 202,
        "setup: the browser {name:?} is created, got {status} ({created})"
    );
    text(&created, "browserArn")
}

/// `CreateBrowserProfile`: `PUT /browser-profiles`, answering the profile's ARN.
async fn create_browser_profile(router: &MultiServiceRouter, name: &str) -> String {
    let (status, created) =
        send(router, "PUT", "/browser-profiles", Some(&json!({"name": name}))).await;
    assert_eq!(
        status, 200,
        "setup: the browser profile {name:?} is created, got {status} ({created})"
    );
    text(&created, "profileArn")
}

/// `CreateCodeInterpreter`: `PUT /code-interpreters`, answering the tool's ARN.
async fn create_code_interpreter(router: &MultiServiceRouter, name: &str) -> String {
    let (status, created) = send(
        router,
        "PUT",
        "/code-interpreters",
        Some(&json!({
            "name": name,
            "networkConfiguration": {"networkMode": "SANDBOX"},
        })),
    )
    .await;
    assert_eq!(
        status, 202,
        "setup: the code interpreter {name:?} is created, got {status} ({created})"
    );
    text(&created, "codeInterpreterArn")
}

/// The id segment of a resource ARN this service published.
fn id_of(arn: &str) -> String {
    arn.rsplit('/').next().unwrap_or_default().to_string()
}

/// Make every clock-owed gateway transition fall due, so the next request materialises the
/// status the clock already owes without waiting the span out on the wall clock. Writes
/// only the due instant the emulator itself stores.
fn settle_gateways_now(engine: &Arc<engine::InMemoryEngine>) {
    let store = engine
        .state::<state::ComAmazonawsBedrockagentcorecontrolAmazonbedrockagentcorecontrolState>();
    let mut store = store.lock();
    let past = SystemTime::now() - Duration::from_millis(1);
    for gateway in store.gateways.values_mut() {
        gateway.settles_at = past;
    }
}

/// RULE (a): each resource's tag set is its own, so what a read reports is decided by the
/// ARN it addressed — a write on one resource, and a removal from it, leave every other
/// resource's set exactly as it was, including a sibling of the same family carrying the
/// same key.
#[tokio::test]
async fn each_resource_reports_only_the_tags_it_carries() {
    let (router, _engine) = router();

    // (case, the first resource's ARN, the second's, the set written on the first, the set
    // written on the second) — one row per taggable family, the sets differing along what
    // the rule generalises over: a key only one sibling carries, a key both carry with
    // different values, and a sibling left carrying no tag at all.
    let rows: Vec<SiblingRow> = vec![
        (
            "two agent runtimes, a shared key at different values",
            create_runtime(&router, "first_runtime").await,
            create_runtime(&router, "second_runtime").await,
            tags_of(&[("owner", "first"), ("shared", "from-the-first")]),
            tags_of(&[("shared", "from-the-second")]),
        ),
        (
            "two gateways, the second left untagged",
            create_gateway(&router, "firstgateway").await,
            create_gateway(&router, "secondgateway").await,
            tags_of(&[("owner", "first"), ("shared", "only-here")]),
            BTreeMap::new(),
        ),
        (
            "two browsers, disjoint keys",
            create_browser(&router, "firstBrowser").await,
            create_browser(&router, "secondBrowser").await,
            tags_of(&[("shared", "browser-one"), ("tier", "a")]),
            tags_of(&[("shared", "browser-two"), ("other", "b")]),
        ),
        (
            "two browser profiles, a shared key at different values",
            create_browser_profile(&router, "firstProfile").await,
            create_browser_profile(&router, "secondProfile").await,
            tags_of(&[("shared", "profile-one"), ("owner", "first")]),
            tags_of(&[("shared", "profile-two")]),
        ),
        (
            "two code interpreters, a shared key at different values",
            create_code_interpreter(&router, "firstInterpreter").await,
            create_code_interpreter(&router, "secondInterpreter").await,
            tags_of(&[("shared", "interpreter-one"), ("owner", "first")]),
            tags_of(&[("shared", "interpreter-two")]),
        ),
    ];

    for (case, first, second, first_tags, second_tags) in rows {
        for (which, arn, written) in [
            ("the first", &first, &first_tags),
            ("the second", &second, &second_tags),
        ] {
            if written.is_empty() {
                continue;
            }
            let (status, answer) = tag(&router, arn, written).await;
            assert_eq!(
                status, 204,
                "case {case:?} ({which}): expected HTTP 204 from TagResource, got {status} \
                 ({answer})"
            );
        }

        for (which, arn, expected) in [
            ("the first", &first, &first_tags),
            ("the second", &second, &second_tags),
        ] {
            let (status, answer) = list_tags(&router, arn).await;
            assert_eq!(
                (status, reported_tags(&answer)),
                (200, expected.clone()),
                "case {case:?} ({which}): expected only its own tag set {expected:?}, got \
                 {status} ({answer})"
            );
        }

        // A removal is addressed the same way: it takes the key off the resource the ARN
        // names and off nothing else, so the sibling's value for that very key stays.
        let (status, answer) = untag(&router, &first, &["shared"]).await;
        assert_eq!(
            status, 204,
            "case {case:?}: expected HTTP 204 from UntagResource, got {status} ({answer})"
        );
        let mut after_first = first_tags.clone();
        after_first.remove("shared");
        let (status, answer) = list_tags(&router, &first).await;
        assert_eq!(
            (status, reported_tags(&answer)),
            (200, after_first),
            "case {case:?}: expected the removal to take only the named key, got {status} \
             ({answer})"
        );
        let (status, answer) = list_tags(&router, &second).await;
        assert_eq!(
            (status, reported_tags(&answer)),
            (200, second_tags.clone()),
            "case {case:?}: expected the sibling's set untouched by the removal, got {status} \
             ({answer})"
        );
    }
}

/// RULE (b): a tag set belongs to the resource, not to one configuration of it, so a
/// reconfiguration of the resource through its own update operation leaves the set where
/// it is and the read reports it unchanged.
#[tokio::test]
async fn tags_outlive_a_reconfiguration_of_the_resource() {
    let (router, engine) = router();

    let runtime = create_runtime(&router, "updated_runtime").await;
    let gateway = create_gateway(&router, "updatedgateway").await;
    let written = tags_of(&[("owner", "agents"), ("environment", "prod")]);
    for (which, arn) in [("the runtime", &runtime), ("the gateway", &gateway)] {
        let (status, answer) = tag(&router, arn, &written).await;
        assert_eq!(
            status, 204,
            "setup ({which}): expected HTTP 204 from TagResource, got {status} ({answer})"
        );
    }

    // A gateway serves a mutation only in the terminal status it settles into, so the
    // clock-owed transition is made due rather than slept out.
    settle_gateways_now(&engine);

    // (case, the update request that reconfigures the resource, the ARN its tags are read
    // through) — the two families of this plane a caller can reconfigure: `UpdateAgentRuntime`
    // publishes another version of the runtime, `UpdateGateway` replaces the gateway's whole
    // configuration.
    let updates: Vec<(&str, String, Value, String)> = vec![
        (
            "UpdateAgentRuntime publishes another version",
            format!("/runtimes/{}/", id_of(&runtime)),
            json!({
                "agentRuntimeArtifact": container_artifact("v2"),
                "roleArn": ROLE_ARN,
            }),
            runtime.clone(),
        ),
        (
            "UpdateGateway replaces the whole configuration",
            format!("/gateways/{}/", id_of(&gateway)),
            gateway_configuration("updatedgateway"),
            gateway.clone(),
        ),
    ];

    for (case, uri, body, arn) in updates {
        let (status, updated) = send(&router, "PUT", &uri, Some(&body)).await;
        assert_eq!(
            status, 202,
            "case {case:?}: expected the update to be accepted, got {status} ({updated})"
        );

        let (status, answer) = list_tags(&router, &arn).await;
        assert_eq!(
            (status, reported_tags(&answer)),
            (200, written.clone()),
            "case {case:?}: expected the tag set {written:?} to outlive the update, got \
             {status} ({answer})"
        );
    }
}
