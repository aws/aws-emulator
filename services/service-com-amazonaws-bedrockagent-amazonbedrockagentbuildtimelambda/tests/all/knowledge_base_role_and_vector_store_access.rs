// SPDX-License-Identifier: CC0-1.0

//! The two IDENTITY pre-conditions a knowledge base is created under, end to end over HTTP.
//!
//! Amazon Bedrock acts as the role a knowledge base names — it reads the data sources and
//! reads and writes the vector store AS that role — so two things about that role are
//! resolved before a knowledge base is created at all, and this suite pins each over a
//! TABLE rather than over one example:
//!
//! * the role's own trust policy must let `bedrock.amazonaws.com` assume it; the rows span
//!   the forms a trust document takes — the service principal named alone and among others,
//!   an explicit `Deny`, a statement about another action, another service principal, and a
//!   role ARN the account's IAM holds no record of;
//! * for an `OPENSEARCH_SERVERLESS` storage configuration, the collection's own `data`
//!   access policies must grant that role the permissions a knowledge base performs on the
//!   vector index; the rows span the permissions a policy grants, which principal it names,
//!   and a collection ARN nothing holds.
//!
//! A refused create leaves NOTHING behind: each row reads `ListKnowledgeBases` back and
//! asserts the knowledge base is absent.
//!
//! Every call goes through the router, so the real wire path is exercised — and through the
//! real IAM and OpenSearch Serverless services on the same engine, so the records these
//! rules are decided against are the ones those services' own operations wrote.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The region every call of this suite is made in.
const REGION: &str = "us-east-1";

/// A SigV4 `Authorization` header scoped to one signing name, which is the routing key the
/// addressed service claims on.
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{signing_name}/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=0123456789abcdef"
    )
}

/// The embedding model every row names: a published model whose output modality is an
/// embedding, so no row is refused on the model.
const EMBEDDING_MODEL: &str =
    "arn:aws:bedrock:us-east-1::foundation-model/amazon.titan-embed-text-v2:0";

/// The vector index every OpenSearch Serverless row binds to.
const VECTOR_INDEX: &str = "bedrock-knowledge-base-index";

/// The `ValidationException` message Amazon Bedrock answers for a role it could not assume.
const UNASSUMABLE_ROLE: &str = "Bedrock Knowledge Base was unable to assume the given role. \
                                Provide the proper permissions and retry the request.";

/// The `ValidationException` message Amazon Bedrock answers when the vector store refuses
/// the knowledge base's role.
const UNREACHABLE_STORAGE: &str = "The knowledge base storage configuration provided is \
                                   invalid... Dependency error document status code: 403, \
                                   error message: Bad Authorization";

/// One HTTP answer from the emulator, parsed.
#[derive(Debug)]
struct Answer {
    status: u16,
    /// The restJson1 error shape name, from `x-amzn-errortype` or the body's `__type`;
    /// `None` on a success.
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

    fn text(&self, path: &[&str]) -> String {
        self.member(path).as_str().unwrap_or_default().to_string()
    }
}

/// This service, IAM and OpenSearch Serverless on ONE engine — the shape
/// `core/router/src/lib.rs` mounts them in.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_bedrockagent_amazonbedrockagentbuildtimelambda::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        router.register_service(
            service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        router.register_service(
            service_com_amazonaws_opensearchserverless_opensearchserverless::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        Self { router }
    }

    /// One restJson1 call to this service.
    async fn call(&self, method: &str, uri: &str, body: Option<Value>) -> Answer {
        let bytes = body
            .map(|value| Bytes::from(serde_json::to_vec(&value).expect("serialize request body")))
            .unwrap_or_default();
        let request = http::Request::builder()
            .method(method)
            .uri(uri)
            .header("host", format!("bedrock-agent.{REGION}.amazonaws.com"))
            .header("authorization", authorization("bedrock"))
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

    async fn create_knowledge_base(&self, request: Value) -> Answer {
        self.call("PUT", "/knowledgebases/", Some(request)).await
    }

    /// The ids `ListKnowledgeBases` reports.
    async fn knowledge_base_names(&self) -> Vec<String> {
        let listed = self.call("POST", "/knowledgebases/", Some(json!({}))).await;
        assert_eq!(
            listed.status, 200,
            "case setup: expected ListKnowledgeBases -> 200, got {listed:?}"
        );
        listed
            .member(&["knowledgeBaseSummaries"])
            .as_array()
            .unwrap_or(&Vec::new())
            .iter()
            .map(|summary| summary["name"].as_str().unwrap_or_default().to_string())
            .collect()
    }

    /// One IAM call: IAM speaks `awsQuery`, so its parameters travel as a form body and its
    /// answer comes back as XML.
    async fn iam(&self, action: &str, params: &[(&str, &str)]) -> (u16, String) {
        let mut body = format!("Action={action}&Version=2010-05-08");
        for (name, value) in params {
            body.push('&');
            body.push_str(&form_encode(name));
            body.push('=');
            body.push_str(&form_encode(value));
        }
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", format!("iam.{REGION}.amazonaws.com"))
            .header("content-type", "application/x-www-form-urlencoded")
            .header("authorization", authorization("iam"))
            .body(Bytes::from(body))
            .expect("build the IAM request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        (status, String::from_utf8_lossy(&response.into_body()).to_string())
    }

    /// One OpenSearch Serverless call, over its own `awsJson1_0` front door.
    async fn aoss(&self, operation: &str, body: Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", format!("aoss.{REGION}.amazonaws.com"))
            .header("content-type", "application/x-amz-json-1.0")
            .header("x-amz-target", format!("OpenSearchServerless.{operation}"))
            .header("authorization", authorization("aoss"))
            .body(Bytes::from(
                serde_json::to_vec(&body).expect("serialize the request body"),
            ))
            .expect("build the aoss request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed: Value =
            serde_json::from_slice(&response.into_body()).unwrap_or(Value::Null);
        (status, parsed)
    }

    /// Create an IAM role carrying `trust` as its assume-role policy, answering the ARN IAM
    /// persisted.
    async fn given_role(&self, name: &str, trust: &str) -> String {
        let (status, created) = self
            .iam(
                "CreateRole",
                &[("RoleName", name), ("AssumeRolePolicyDocument", trust)],
            )
            .await;
        assert_eq!(
            status, 200,
            "case CreateRole({name}): expected 200, got {status} {created}"
        );
        let arn = xml_text(&created, "Arn");
        assert!(
            !arn.is_empty(),
            "case CreateRole({name}): expected an ARN, got {created}"
        );
        arn
    }

    /// Create a collection, answering the ARN its create reported.
    async fn given_collection(&self, name: &str) -> String {
        let (status, created) = self
            .aoss(
                "CreateCollection",
                json!({
                    "name": name,
                    "type": "VECTORSEARCH",
                    "encryptionConfig": { "aWSOwnedKey": true },
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case CreateCollection({name}): expected 200, got {status} {created}"
        );
        let arn = created["createCollectionDetail"]["arn"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            !arn.is_empty(),
            "case CreateCollection({name}): expected an ARN, got {created}"
        );
        arn
    }

    /// Create a `data` access policy, asserting it was accepted.
    async fn given_data_access(&self, name: &str, policy: &Value) {
        let (status, created) = self
            .aoss(
                "CreateAccessPolicy",
                json!({ "type": "data", "name": name, "policy": policy.to_string() }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case CreateAccessPolicy(data/{name}): expected 200, got {status} {created}"
        );
    }
}

/// Percent-encode one form value the way an AWS SDK encodes an `awsQuery` body.
fn form_encode(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(char::from(*byte));
            }
            other => encoded.push_str(&format!("%{other:02X}")),
        }
    }
    encoded
}

/// The text of the first `<tag>` of an XML body.
fn xml_text(xml: &str, tag: &str) -> String {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let Some(start) = xml.find(&open).map(|at| at + open.len()) else {
        return String::new();
    };
    let Some(end) = xml[start..].find(&close).map(|at| at + start) else {
        return String::new();
    };
    xml[start..end].to_string()
}

/// A trust policy: one statement with `effect`, `action` and the principals `principal`
/// names, written as a client sends it.
fn trust_policy(effect: &str, action: Value, principal: Value) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": effect,
            "Principal": { "Service": principal },
            "Action": action,
        }],
    })
    .to_string()
}

/// The trust policy a knowledge-base role carries on AWS.
fn admitting_trust() -> String {
    trust_policy(
        "Allow",
        json!("sts:AssumeRole"),
        json!("bedrock.amazonaws.com"),
    )
}

/// A `data` access policy granting `permissions` on the indexes of `collection` to
/// `principal` — the shape AWS's data-access documentation writes.
fn index_data_policy(collection: &str, permissions: &[&str], principal: &str) -> Value {
    json!([{
        "Rules": [{
            "ResourceType": "index",
            "Resource": [format!("index/{collection}/*")],
            "Permission": permissions,
        }],
        "Principal": [principal],
    }])
}

/// A `CreateKnowledgeBase` body naming `role_arn` and the storage configuration `storage`.
fn create_request(name: &str, role_arn: &str, storage: Value) -> Value {
    json!({
        "name": name,
        "roleArn": role_arn,
        "knowledgeBaseConfiguration": {
            "type": "VECTOR",
            "vectorKnowledgeBaseConfiguration": { "embeddingModelArn": EMBEDDING_MODEL },
        },
        "storageConfiguration": storage,
    })
}

/// An `OPENSEARCH_SERVERLESS` storage configuration naming one collection and index.
fn opensearch_storage(collection_arn: &str, index: &str) -> Value {
    json!({
        "type": "OPENSEARCH_SERVERLESS",
        "opensearchServerlessConfiguration": {
            "collectionArn": collection_arn,
            "vectorIndexName": index,
            "fieldMapping": {
                "vectorField": "vector",
                "textField": "text",
                "metadataField": "metadata",
            },
        },
    })
}

/// A storage configuration the OpenSearch Serverless rule does not decide: it names a
/// collection this emulator holds no record of, so the trust-policy table's rows are about
/// the role alone.
fn unjudged_storage() -> Value {
    opensearch_storage(
        &format!("arn:aws:aoss:{REGION}:000000000000:collection/trust-row-collection"),
        VECTOR_INDEX,
    )
}

/// RULE: a knowledge base is created only under a role whose trust policy lets
/// `bedrock.amazonaws.com` assume it; a role that withholds that grant is refused
/// `ValidationException` and no knowledge base is created.
///
/// The rows span the forms a trust document takes.
#[tokio::test]
async fn a_knowledge_base_is_created_only_under_a_role_bedrock_can_assume() {
    /// One case: the trust document the role carries (`None` for a role ARN IAM holds no
    /// record of), and whether the create is accepted.
    struct Row {
        case: &'static str,
        trust: Option<String>,
        accepted: bool,
    }
    let rows: Vec<Row> = vec![
        Row {
            case: "a role trusting the Bedrock service principal",
            trust: Some(admitting_trust()),
            accepted: true,
        },
        Row {
            case: "a role trusting Bedrock among several principals",
            trust: Some(trust_policy(
                "Allow",
                json!("sts:AssumeRole"),
                json!(["lambda.amazonaws.com", "bedrock.amazonaws.com"]),
            )),
            accepted: true,
        },
        Row {
            case: "a role trusting Bedrock for every sts action",
            trust: Some(trust_policy(
                "Allow",
                json!("sts:*"),
                json!("bedrock.amazonaws.com"),
            )),
            accepted: true,
        },
        Row {
            case: "a role trusting only another service",
            trust: Some(trust_policy(
                "Allow",
                json!("sts:AssumeRole"),
                json!("lambda.amazonaws.com"),
            )),
            accepted: false,
        },
        Row {
            case: "a role denying Bedrock the assume",
            trust: Some(trust_policy(
                "Deny",
                json!("sts:AssumeRole"),
                json!("bedrock.amazonaws.com"),
            )),
            accepted: false,
        },
        Row {
            case: "a role naming Bedrock for another action",
            trust: Some(trust_policy(
                "Allow",
                json!("s3:GetObject"),
                json!("bedrock.amazonaws.com"),
            )),
            accepted: false,
        },
        Row {
            // Nothing stored says what such a role trusts, so the create is not refused on
            // one: the role may belong to another account.
            case: "a role ARN this account's IAM holds no record of",
            trust: None,
            accepted: true,
        },
    ];
    for (index, Row { case, trust, accepted }) in rows.into_iter().enumerate() {
        let emulator = Emulator::new();
        let role_name = format!("kb-trust-role-{index}");
        let role_arn = match &trust {
            Some(trust) => emulator.given_role(&role_name, trust).await,
            None => format!("arn:aws:iam::000000000000:role/{role_name}"),
        };
        let name = format!("kb-trust-{index}");
        let created = emulator
            .create_knowledge_base(create_request(&name, &role_arn, unjudged_storage()))
            .await;
        if accepted {
            assert_eq!(
                created.status, 202,
                "case {case}: expected CreateKnowledgeBase -> 202, got {created:?}"
            );
            assert_eq!(
                created.text(&["knowledgeBase", "roleArn"]),
                role_arn,
                "case {case}: expected the created knowledge base to carry the role the \
                 request named, got {created:?}"
            );
        } else {
            assert_eq!(
                created.status, 400,
                "case {case}: expected CreateKnowledgeBase -> 400, got {created:?}"
            );
            assert!(
                created
                    .error_type
                    .as_deref()
                    .is_some_and(|held| held.contains("ValidationException")),
                "case {case}: expected ValidationException, got {created:?}"
            );
            assert_eq!(
                created.text(&["message"]),
                UNASSUMABLE_ROLE,
                "case {case}: expected the refusal Amazon Bedrock words for an unassumable \
                 role, got {created:?}"
            );
        }
        let held = emulator.knowledge_base_names().await;
        assert_eq!(
            held.contains(&name),
            accepted,
            "case {case}: expected the knowledge base {}, got {held:?}",
            if accepted { "listed" } else { "absent" }
        );
    }
}

/// RULE: an `OPENSEARCH_SERVERLESS` storage configuration is served only where the
/// collection's own `data` access policies grant the knowledge base's role the permissions a
/// knowledge base performs on the vector index; a role granted less is refused
/// `ValidationException` and no knowledge base is created.
///
/// The rows span the permissions a policy grants and the principal it names.
#[tokio::test]
async fn an_opensearch_serverless_binding_is_served_only_where_the_role_is_granted_access() {
    /// One case: the permissions the collection's data access policy grants the knowledge
    /// base's role (`None` for no policy at all), whether it names that role, and whether
    /// the create is accepted.
    struct Row {
        case: &'static str,
        granted: Option<&'static [&'static str]>,
        names_the_role: bool,
        accepted: bool,
    }
    let rows: &[Row] = &[
        Row {
            case: "a policy granting the role every permission a knowledge base performs",
            granted: Some(&[
                "aoss:DescribeIndex",
                "aoss:ReadDocument",
                "aoss:WriteDocument",
            ]),
            names_the_role: true,
            accepted: true,
        },
        Row {
            case: "a policy granting the role every aoss permission",
            granted: Some(&["aoss:*"]),
            names_the_role: true,
            accepted: true,
        },
        Row {
            case: "a policy granting the role the index writes as well",
            granted: Some(&[
                "aoss:CreateIndex",
                "aoss:DescribeIndex",
                "aoss:ReadDocument",
                "aoss:WriteDocument",
            ]),
            names_the_role: true,
            accepted: true,
        },
        Row {
            case: "no data access policy at all",
            granted: None,
            names_the_role: true,
            accepted: false,
        },
        Row {
            case: "a policy naming another principal",
            granted: Some(&["aoss:*"]),
            names_the_role: false,
            accepted: false,
        },
        Row {
            case: "a policy granting the role only the document read",
            granted: Some(&["aoss:ReadDocument"]),
            names_the_role: true,
            accepted: false,
        },
        Row {
            case: "a policy granting the role only the index reads",
            granted: Some(&["aoss:DescribeIndex", "aoss:ReadDocument"]),
            names_the_role: true,
            accepted: false,
        },
    ];
    for (
        index,
        Row {
            case,
            granted,
            names_the_role,
            accepted,
        },
    ) in rows.iter().enumerate()
    {
        let emulator = Emulator::new();
        let role_arn = emulator
            .given_role(&format!("kb-access-role-{index}"), &admitting_trust())
            .await;
        let other_arn = emulator
            .given_role(&format!("kb-other-role-{index}"), &admitting_trust())
            .await;
        let collection = format!("kb-access-{index}");
        let collection_arn = emulator.given_collection(&collection).await;
        if let Some(granted) = granted {
            let principal = if *names_the_role {
                role_arn.as_str()
            } else {
                other_arn.as_str()
            };
            emulator
                .given_data_access(
                    &format!("kb-access-policy-{index}"),
                    &index_data_policy(&collection, granted, principal),
                )
                .await;
        }
        let name = format!("kb-access-{index}");
        let created = emulator
            .create_knowledge_base(create_request(
                &name,
                &role_arn,
                opensearch_storage(&collection_arn, VECTOR_INDEX),
            ))
            .await;
        if *accepted {
            assert_eq!(
                created.status, 202,
                "case {case}: expected CreateKnowledgeBase -> 202, got {created:?}"
            );
            assert_eq!(
                created.text(&[
                    "knowledgeBase",
                    "storageConfiguration",
                    "opensearchServerlessConfiguration",
                    "collectionArn",
                ]),
                collection_arn,
                "case {case}: expected the created knowledge base to carry the collection \
                 the request named, got {created:?}"
            );
        } else {
            assert_eq!(
                created.status, 400,
                "case {case}: expected CreateKnowledgeBase -> 400, got {created:?}"
            );
            assert_eq!(
                created.text(&["message"]),
                UNREACHABLE_STORAGE,
                "case {case}: expected the refusal Amazon Bedrock words for a storage \
                 configuration its role cannot reach, got {created:?}"
            );
        }
        let held = emulator.knowledge_base_names().await;
        assert_eq!(
            held.contains(&name),
            *accepted,
            "case {case}: expected the knowledge base {}, got {held:?}",
            if *accepted { "listed" } else { "absent" }
        );
    }
}

/// RULE: the OpenSearch Serverless decision is about the stored collection the ARN names —
/// a collection this emulator holds no record of says nothing about what its policies grant,
/// so such a binding is not refused on one.
#[tokio::test]
async fn a_collection_nothing_holds_is_not_judged_on_its_policies() {
    let emulator = Emulator::new();
    let role_arn = emulator
        .given_role("kb-unheld-collection-role", &admitting_trust())
        .await;
    let created = emulator
        .create_knowledge_base(create_request(
            "kb-unheld-collection",
            &role_arn,
            opensearch_storage(
                &format!("arn:aws:aoss:{REGION}:000000000000:collection/neverexisted000"),
                VECTOR_INDEX,
            ),
        ))
        .await;
    assert_eq!(
        created.status, 202,
        "case a collection ARN nothing holds: expected CreateKnowledgeBase -> 202, got \
         {created:?}"
    );
}
