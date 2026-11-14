// SPDX-License-Identifier: CC0-1.0

//! `ListTagsForResource` end to end over HTTP: the tags of a resource of this plane are a
//! PROJECTION of the record that resource's own reads report, never a value of their own.
//!
//! RULE: a tagging operation is addressed by the ARN of a resource this plane MINTS — the
//! resource types the modelled `TaggableResourcesArn` enumerates (an agent, an agent alias,
//! a knowledge base, a flow, a flow alias, a prompt, a retrieval instruction) — and reads
//! and writes the tags the record that ARN names holds: `ListTagsForResource` reports them,
//! `TagResource` merges into them and `UntagResource` removes the keys it names. An ARN of
//! that syntax naming a resource this store does not hold is the missing resource
//! `ResourceNotFoundException` refuses; a value outside that syntax is the bad request
//! `ValidationException` refuses, quoting the expression the member publishes; a merge past
//! the store's own bound is the modelled `ServiceQuotaExceededException`; and reading the
//! tags changes nothing a later caller sees.
//!
//! The rows span what the rule generalises over: the resource KIND the ARN names (a
//! knowledge base, a flow, a flow alias — each keyed differently in the store, the alias by
//! a PAIR of identifiers), how many tags the record holds (none, one, several), the ARN
//! fields' partition, and every way a value can fall outside the published syntax.
//!
//! Every assertion goes through the router, so it exercises the real wire path — the claim
//! on the `tags` path space among it, which is what decides that a knowledge base's ARN is
//! served by THIS plane rather than by the data plane that binds the identical routes.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_bedrockagent_amazonbedrockagentbuildtimelambda::state::{
    service_state, TAG_QUOTA,
};
use service_com_amazonaws_bedrockagent_amazonbedrockagentbuildtimelambda::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 `Authorization` header scoped to the `bedrock` signing name: the routing key
/// every Bedrock-family service claims on.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/bedrock/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0123456789abcdef";

/// The region of [`AUTHORIZATION`]'s credential scope.
const REGION: &str = "eu-west-1";

/// The account id every emulated ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = "000000000000";

/// A role ARN matching the modelled `KnowledgeBaseRoleArn` pattern.
const KB_ROLE: &str =
    "arn:aws:iam::000000000000:role/service-role/AmazonBedrockExecutionRoleForKnowledgeBase";

/// A role ARN matching the modelled `FlowExecutionRoleArn` pattern.
const FLOW_ROLE: &str = "arn:aws:iam::000000000000:role/service-role/AmazonBedrockExecutionRoleForFlows";

/// An embedding model ARN naming a model whose published output is an embedding.
const EMBEDDING_MODEL: &str =
    "arn:aws:bedrock:eu-west-1::foundation-model/amazon.titan-embed-text-v2:0";

/// The tags CloudFormation gives a resource it created, as a stack propagates them: the
/// three `aws:cloudformation:*` keys plus the stack's own. Keys and values are within the
/// modelled `TagKey`/`TagValue` patterns.
fn stack_tags() -> BTreeMap<String, String> {
    [
        ("aws:cloudformation:stack-name", "tags-estate"),
        (
            "aws:cloudformation:stack-id",
            "arn:aws:cloudformation:eu-west-1:000000000000:stack/tags-estate/0",
        ),
        ("aws:cloudformation:logical-id", "Escalation"),
        ("estate", "tags"),
    ]
    .into_iter()
    .map(|(key, value)| (key.to_string(), value.to_string()))
    .collect()
}

/// One tag, for a record that holds a single one.
fn one_tag() -> BTreeMap<String, String> {
    [("estate".to_string(), "tags".to_string())]
        .into_iter()
        .collect()
}

/// One value as a percent-encoded URI component, as a client that sends a non-greedy
/// `@httpLabel` or a query value encodes it.
fn label(value: &str) -> String {
    value
        .replace('%', "%25")
        .replace(':', "%3A")
        .replace('/', "%2F")
        .replace('=', "%3D")
        .replace('+', "%2B")
        .replace('&', "%26")
}

/// One HTTP answer from the emulator, parsed.
#[derive(Debug)]
struct Answer {
    status: u16,
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

    /// The reported `tags` map, as a sorted map so a comparison is order-free.
    fn tags(&self, case: &str) -> BTreeMap<String, String> {
        self.member(&["tags"])
            .as_object()
            .unwrap_or_else(|| {
                panic!("case {case}: expected a tags map, got {:?}", self.body)
            })
            .iter()
            .map(|(key, value)| {
                (
                    key.clone(),
                    value
                        .as_str()
                        .unwrap_or_else(|| panic!("case {case}: a tag value is a string"))
                        .to_string(),
                )
            })
            .collect()
    }
}

/// The emulator under test: one router over one shared engine, exactly as the production
/// router mounts this service.
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
            serde_json::from_slice(&raw).unwrap_or_else(|error| {
                panic!(
                    "{method} {uri} -> {status} body is not JSON ({error}): {}",
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

    /// `ListTagsForResource`, with the ARN percent-encoded as a client that sends a
    /// non-greedy `@httpLabel` encodes it.
    async fn list_tags(&self, resource_arn: &str) -> Answer {
        self.call("GET", &format!("/tags/{}", label(resource_arn)), None)
            .await
    }

    /// `TagResource` on one resource.
    async fn tag(&self, resource_arn: &str, tags: &BTreeMap<String, String>) -> Answer {
        self.call(
            "POST",
            &format!("/tags/{}", label(resource_arn)),
            Some(json!({ "tags": tags })),
        )
        .await
    }

    /// `UntagResource` on one resource, whose `tagKeys` is an `@httpQuery` list.
    async fn untag(&self, resource_arn: &str, tag_keys: &[&str]) -> Answer {
        let query: Vec<String> = tag_keys
            .iter()
            .map(|key| format!("tagKeys={}", label(key)))
            .collect();
        self.call(
            "DELETE",
            &format!("/tags/{}?{}", label(resource_arn), query.join("&")),
            None,
        )
        .await
    }

    /// Create one knowledge base carrying `tags` and report its ARN.
    async fn knowledge_base(
        &self,
        case: &str,
        name: &str,
        tags: &BTreeMap<String, String>,
    ) -> String {
        let mut request = json!({
            "name": name,
            "roleArn": KB_ROLE,
            "knowledgeBaseConfiguration": {
                "type": "VECTOR",
                "vectorKnowledgeBaseConfiguration": { "embeddingModelArn": EMBEDDING_MODEL },
            },
            "storageConfiguration": {
                "type": "OPENSEARCH_SERVERLESS",
                "opensearchServerlessConfiguration": {
                    "collectionArn": format!("arn:aws:aoss:{REGION}:{ACCOUNT}:collection/tags"),
                    "vectorIndexName": format!("{name}-index"),
                    "fieldMapping": {
                        "vectorField": "bedrock-knowledge-base-default-vector",
                        "textField": "AMAZON_BEDROCK_TEXT_CHUNK",
                        "metadataField": "AMAZON_BEDROCK_METADATA",
                    },
                },
            },
        });
        if !tags.is_empty() {
            request["tags"] = json!(tags);
        }
        let created = self.call("PUT", "/knowledgebases/", Some(request)).await;
        assert_eq!(
            created.status, 202,
            "case {case}: expected CreateKnowledgeBase -> 202, got {created:?}"
        );
        created.string(&["knowledgeBase", "knowledgeBaseArn"])
    }

    /// Create one flow carrying `tags` and report its id and ARN.
    async fn flow(
        &self,
        case: &str,
        name: &str,
        tags: &BTreeMap<String, String>,
    ) -> (String, String) {
        let mut request = json!({
            "name": name,
            "executionRoleArn": FLOW_ROLE,
            "definition": passthrough_definition(),
        });
        if !tags.is_empty() {
            request["tags"] = json!(tags);
        }
        let created = self.call("POST", "/flows/", Some(request)).await;
        assert_eq!(
            created.status, 201,
            "case {case}: expected CreateFlow -> 201, got {created:?}"
        );
        (created.string(&["id"]), created.string(&["arn"]))
    }

    /// Prepare the flow and move the outcome the clock owes into the past, so the flow
    /// stands prepared without the test waiting for the span.
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

    /// Create one alias of a prepared flow, carrying `tags`, and report its ARN.
    async fn flow_alias(
        &self,
        case: &str,
        flow_id: &str,
        name: &str,
        tags: &BTreeMap<String, String>,
    ) -> String {
        self.prepare_and_settle(case, flow_id).await;
        let cut = self
            .call("POST", &format!("/flows/{flow_id}/versions"), Some(json!({})))
            .await;
        assert_eq!(
            cut.status, 201,
            "case {case}: expected CreateFlowVersion -> 201, got {cut:?}"
        );
        let mut request = json!({
            "name": name,
            "routingConfiguration": [{ "flowVersion": "1" }],
        });
        if !tags.is_empty() {
            request["tags"] = json!(tags);
        }
        let created = self
            .call("POST", &format!("/flows/{flow_id}/aliases"), Some(request))
            .await;
        assert_eq!(
            created.status, 201,
            "case {case}: expected CreateFlowAlias -> 201, got {created:?}"
        );
        created.string(&["arn"])
    }
}

/// A passthrough flow definition: an `Input` node wired straight to an `Output` node.
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
                "configuration": {
                    "data": { "sourceOutput": "document", "targetInput": "document" }
                },
            },
        ],
    })
}

/// The tags of a resource of this plane are the ones its create stored, whatever kind of
/// resource the ARN names and however many tags it holds — and reading them changes nothing
/// a later caller sees.
#[tokio::test]
async fn the_tags_reported_are_the_ones_the_resource_was_created_with() {
    let emulator = Emulator::new();

    // Rows: the resource kind the ARN names against how many tags the record holds.
    let mut rows: Vec<(&str, String, BTreeMap<String, String>)> = Vec::new();
    rows.push((
        "a knowledge base carrying the tags a stack propagated",
        emulator
            .knowledge_base("a knowledge base with stack tags", "tags-kb", &stack_tags())
            .await,
        stack_tags(),
    ));
    rows.push((
        "a knowledge base carrying one tag",
        emulator
            .knowledge_base("a knowledge base with one tag", "tags-kb-one", &one_tag())
            .await,
        one_tag(),
    ));
    rows.push((
        "a knowledge base carrying none",
        emulator
            .knowledge_base(
                "a knowledge base with no tags",
                "tags-kb-none",
                &BTreeMap::new(),
            )
            .await,
        BTreeMap::new(),
    ));

    let (flow_id, flow_arn) = emulator
        .flow("a flow with stack tags", "tags-flow", &stack_tags())
        .await;
    rows.push((
        "a flow, the other kind keyed by its own id",
        flow_arn,
        stack_tags(),
    ));
    rows.push((
        "a flow alias, keyed by the PAIR of its flow and its own id",
        emulator
            .flow_alias("a flow alias with one tag", &flow_id, "tags-alias", &one_tag())
            .await,
        one_tag(),
    ));

    for (case, resource_arn, expected) in &rows {
        let listed = emulator.list_tags(resource_arn).await;
        assert_eq!(
            listed.status, 200,
            "case {case} ({resource_arn}): expected ListTagsForResource -> 200, got {listed:?}"
        );
        let reported = listed.tags(case);
        assert_eq!(
            &reported, expected,
            "case {case} ({resource_arn}): expected the tags the create stored, got \
             {reported:?}"
        );

        // A read: the second call reports the same thing, and so does the resource's own
        // read afterwards.
        let again = emulator.list_tags(resource_arn).await;
        assert_eq!(
            again.tags(case),
            reported,
            "case {case}: a read must not change what a later caller sees, got {again:?}"
        );
    }
}

/// `TagResource` MERGES into the same record the listing reports and `UntagResource`
/// removes only the keys it names, so a tag a caller sets is read back through the same
/// reader as the ones the create stored.
#[tokio::test]
async fn tags_a_caller_sets_are_read_back_and_the_keys_it_removes_are_gone() {
    let emulator = Emulator::new();

    // Rows: the resource kinds a tagging write addresses, each starting from a different
    // set of stored tags — one that already holds a key the write re-values, and one that
    // holds none at all.
    let knowledge_base = emulator
        .knowledge_base("a knowledge base to tag", "tags-write-kb", &one_tag())
        .await;
    let (flow_id, flow) = emulator
        .flow("a flow to tag", "tags-write-flow", &BTreeMap::new())
        .await;
    let alias = emulator
        .flow_alias(
            "a flow alias to tag",
            &flow_id,
            "tags-write-alias",
            &stack_tags(),
        )
        .await;

    for (case, resource_arn, held) in [
        (
            "a knowledge base already holding the key the write re-values",
            knowledge_base,
            one_tag(),
        ),
        ("a flow holding no tag at all", flow, BTreeMap::new()),
        (
            "a flow alias holding the tags a stack propagated",
            alias,
            stack_tags(),
        ),
    ] {
        // The merge: the key the resource already holds takes the request's value, a new
        // key is added, and every other key the record holds stays.
        let written: BTreeMap<String, String> = [
            ("estate".to_string(), "tagged-by-the-caller".to_string()),
            ("owner".to_string(), "platform".to_string()),
        ]
        .into_iter()
        .collect();
        let tagged = emulator.tag(&resource_arn, &written).await;
        assert_eq!(
            tagged.status, 200,
            "case {case}: expected TagResource -> 200, got {tagged:?}"
        );
        let mut expected = held.clone();
        expected.extend(written.clone());
        let listed = emulator.list_tags(&resource_arn).await;
        assert_eq!(
            listed.tags(case),
            expected,
            "case {case}: expected the merge of the stored tags and the written ones, got \
             {listed:?}"
        );

        // The removal: the keys named go, the ones not named stay, and a key the resource
        // never held is nothing to remove.
        let removed = emulator
            .untag(&resource_arn, &["estate", "never-held"])
            .await;
        assert_eq!(
            removed.status, 200,
            "case {case}: expected UntagResource -> 200, got {removed:?}"
        );
        expected.remove("estate");
        let listed = emulator.list_tags(&resource_arn).await;
        assert_eq!(
            listed.tags(case),
            expected,
            "case {case}: expected only the named keys to be removed, got {listed:?}"
        );

        // Repeating the removal answers the same way and changes nothing.
        let again = emulator
            .untag(&resource_arn, &["estate", "never-held"])
            .await;
        assert_eq!(
            again.status, 200,
            "case {case}: expected a repeated UntagResource -> 200, got {again:?}"
        );
        let listed = emulator.list_tags(&resource_arn).await;
        assert_eq!(
            listed.tags(case),
            expected,
            "case {case}: a repeated removal changes nothing, got {listed:?}"
        );
    }
}

/// The store is bounded: a merge that would leave more than the emulator's own maximum on
/// one resource is refused with the modelled `ServiceQuotaExceededException`, and the
/// resource keeps exactly the tags it had.
#[tokio::test]
async fn a_merge_past_the_tag_quota_is_refused_and_writes_nothing() {
    let emulator = Emulator::new();
    let knowledge_base = emulator
        .knowledge_base("the bounded knowledge base", "tags-quota-kb", &one_tag())
        .await;

    // Exactly the quota is served (the stored key is re-valued, so the set stays at the
    // bound); one past it is refused.
    let at_the_bound: BTreeMap<String, String> = (0..TAG_QUOTA)
        .map(|position| {
            if position == 0 {
                ("estate".to_string(), "re-valued".to_string())
            } else {
                (format!("key-{position}"), position.to_string())
            }
        })
        .collect();
    let tagged = emulator.tag(&knowledge_base, &at_the_bound).await;
    assert_eq!(
        tagged.status, 200,
        "case at the bound: expected TagResource -> 200, got {tagged:?}"
    );
    let listed = emulator.list_tags(&knowledge_base).await;
    assert_eq!(
        listed.tags("at the bound").len(),
        TAG_QUOTA,
        "case at the bound: expected the resource to hold exactly {TAG_QUOTA} tags, got \
         {listed:?}"
    );

    let one_more: BTreeMap<String, String> = [("one-too-many".to_string(), "x".to_string())]
        .into_iter()
        .collect();
    let refused = emulator.tag(&knowledge_base, &one_more).await;
    assert_eq!(
        refused.status, 402,
        "case one past the bound: expected TagResource -> 402, got {refused:?}"
    );
    assert_eq!(
        refused.error_type.as_deref(),
        Some("ServiceQuotaExceededException"),
        "case one past the bound: expected the modelled ServiceQuotaExceededException, got \
         {refused:?}"
    );
    let listed = emulator.list_tags(&knowledge_base).await;
    assert_eq!(
        listed.tags("one past the bound").len(),
        TAG_QUOTA,
        "case one past the bound: a refused merge writes nothing, got {listed:?}"
    );
}

/// An ARN of the published syntax that names no resource this store holds is the missing
/// resource, whatever kind it names.
#[tokio::test]
async fn an_arn_naming_nothing_stored_is_a_missing_resource() {
    let emulator = Emulator::new();
    let stored = emulator
        .knowledge_base("the stored knowledge base", "tags-absent-kb", &one_tag())
        .await;

    // Rows: an identifier of the right form no record holds, for each resource type the
    // published syntax admits — including the three kinds no operation of this emulator
    // stores yet, which hold no record either.
    let rows: Vec<(&str, String)> = vec![
        (
            "a knowledge base id nothing was minted for",
            format!("arn:aws:bedrock:{REGION}:{ACCOUNT}:knowledge-base/aZ09aZ09aZ"),
        ),
        (
            "a flow id nothing was minted for",
            format!("arn:aws:bedrock:{REGION}:{ACCOUNT}:flow/aZ09aZ09aZ"),
        ),
        (
            "an alias of a flow that does not exist",
            format!("arn:aws:bedrock:{REGION}:{ACCOUNT}:flow/aZ09aZ09aZ/alias/aZ09aZ09aZ"),
        ),
        (
            "an agent id nothing was minted for",
            format!("arn:aws:bedrock:{REGION}:{ACCOUNT}:agent/aZ09aZ09aZ"),
        ),
        (
            "an agent alias",
            format!("arn:aws:bedrock:{REGION}:{ACCOUNT}:agent-alias/aZ09aZ09aZ/aZ09aZ09aZ"),
        ),
        (
            "a prompt",
            format!("arn:aws:bedrock:{REGION}:{ACCOUNT}:prompt/aZ09aZ09aZ"),
        ),
        (
            "a prompt version",
            format!("arn:aws:bedrock:{REGION}:{ACCOUNT}:prompt/aZ09aZ09aZ:3"),
        ),
        (
            "a retrieval instruction",
            format!("arn:aws:bedrock:{REGION}:{ACCOUNT}:retrieval-instruction/ri09aZ09aZ"),
        ),
        (
            "a knowledge base of another partition",
            format!("arn:aws-us-gov:bedrock:us-gov-west-1:{ACCOUNT}:knowledge-base/aZ09aZ09aZ"),
        ),
    ];

    for (case, resource_arn) in &rows {
        let refused = emulator.list_tags(resource_arn).await;
        assert_eq!(
            refused.status, 404,
            "case {case} ({resource_arn}): expected ListTagsForResource -> 404, got {refused:?}"
        );
        assert_eq!(
            refused.error_type.as_deref(),
            Some("ResourceNotFoundException"),
            "case {case} ({resource_arn}): expected the modelled ResourceNotFoundException, \
             got {refused:?}"
        );
    }

    // The resource that does exist still reports its tags.
    let listed = emulator.list_tags(&stored).await;
    assert_eq!(
        listed.tags("after the refusals"),
        one_tag(),
        "case after the refusals: a refused read changes nothing, got {listed:?}"
    );
}

/// A `resourceArn` outside the syntax the member publishes is a bad request, refused with
/// the modelled `ValidationException` naming the member and quoting that syntax.
#[tokio::test]
async fn an_arn_outside_the_published_syntax_is_a_bad_request() {
    let emulator = Emulator::new();

    // Rows: every way a value whose resource TYPE this plane's tagging admits can still
    // fall outside the published syntax — a field that is not an ARN field, an ARN with no
    // resource identifier, one segment too many, and the alias form spelled wrong. An ARN
    // whose resource type this plane does not admit is not even addressed to this service:
    // the `tags` path space is split by that type, and the sharer that MINTS the named
    // resource is the one that answers (`service_common::bedrock`).
    let rows: Vec<(&str, String)> = vec![
        (
            "an account field that is not an account id",
            format!("arn:aws:bedrock:{REGION}:not-an-account:knowledge-base/aZ09aZ09aZ"),
        ),
        (
            "an account field of the wrong length",
            format!("arn:aws:bedrock:{REGION}:00000000000:knowledge-base/aZ09aZ09aZ"),
        ),
        (
            "an empty region field",
            format!("arn:aws:bedrock::{ACCOUNT}:knowledge-base/aZ09aZ09aZ"),
        ),
        (
            "a region field outside the Region grammar every ARN pattern states",
            format!("arn:aws:bedrock:EU-WEST-1:{ACCOUNT}:knowledge-base/aZ09aZ09aZ"),
        ),
        (
            "a resource field with no identifier",
            format!("arn:aws:bedrock:{REGION}:{ACCOUNT}:knowledge-base/"),
        ),
        (
            "a resource field carrying one segment too many",
            format!("arn:aws:bedrock:{REGION}:{ACCOUNT}:knowledge-base/aZ09aZ09aZ/extra"),
        ),
        (
            "a flow alias ARN whose middle segment is not the alias one",
            format!("arn:aws:bedrock:{REGION}:{ACCOUNT}:flow/aZ09aZ09aZ/version/1"),
        ),
        (
            "an agent alias ARN missing the alias identifier",
            format!("arn:aws:bedrock:{REGION}:{ACCOUNT}:agent-alias/aZ09aZ09aZ"),
        ),
    ];

    for (case, resource_arn) in &rows {
        let refused = emulator.list_tags(resource_arn).await;
        assert_eq!(
            refused.status, 400,
            "case {case} ({resource_arn}): expected ListTagsForResource -> 400, got {refused:?}"
        );
        assert_eq!(
            refused.error_type.as_deref(),
            Some("ValidationException"),
            "case {case} ({resource_arn}): expected the modelled ValidationException, got \
             {refused:?}"
        );
        let message = refused.string(&["message"]);
        assert!(
            message.contains("at 'resourceArn' failed to satisfy constraint"),
            "case {case} ({resource_arn}): expected the refusal to name the member and its \
             constraint, got {message:?}"
        );
        assert!(
            message.contains(resource_arn.as_str()),
            "case {case} ({resource_arn}): expected the refusal to echo the submitted value, \
             got {message:?}"
        );
    }
}
