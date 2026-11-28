// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Amazon Q in Connect assistant-association slice
//! (`CreateAssistantAssociation` + `GetAssistantAssociation` +
//! `DeleteAssistantAssociation`).
//!
//! Every assertion goes over the wire: a request is built, dispatched through the same
//! [`MultiServiceRouter`] the production emulator mounts, and only the HTTP status, the
//! `x-amzn-errortype` header and the JSON body are inspected. Nothing reaches into the
//! service's state struct, so a response that is not computed from persisted state
//! cannot pass.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one recorded
//! request:
//!
//! * a create persists the association it mints, so a later read — by id or by ARN —
//!   answers out of the stored record, for every target an association can be made
//!   against and every tag count the create accepts;
//! * one assistant holds associations with SEVERAL DISTINCT targets, and a second
//!   create naming a target it already holds is `ConflictException` (HTTP 409) with the
//!   one fixed sentence this service words that class with;
//! * `associationType` and the `association` union name the same thing: a create whose
//!   pair disagrees is `ValidationException` (HTTP 400);
//! * a replayed `clientToken` carrying the same request answers with the association it
//!   already created, and the same token carrying a different request is
//!   `ConflictException` (HTTP 409);
//! * a label this service holds nothing under — an unknown assistant, an unknown
//!   knowledge base, an association of ANOTHER assistant — is
//!   `ResourceNotFoundException` (HTTP 404);
//! * a delete removes the record, so a read of it is not found and the same target can
//!   be associated again; deleting the assistant takes its associations with it;
//! * a listing reports the associations its assistant holds — every summary projected out
//!   of the stored record — bounded by `maxResults` and continued by the `nextToken` it
//!   hands out, scoped to that assistant, and `ResourceNotFoundException` (HTTP 404) for
//!   an assistant this account does not hold.

use std::collections::BTreeMap;
use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_qconnect_wisdomservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request in this suite is signed for; the handlers read it out of the
/// SigV4 credential scope to build ARNs.
const REGION: &str = "eu-central-1";

/// A UUID no create ever mints, for the not-found assertions.
const ABSENT_UUID: &str = "00000000-0000-0000-0000-000000000000";

/// The two `AssociationType` values the model declares
/// (`smithy_query(query="shape",shape="AssociationType")`), written out rather than
/// imported from the implementation so the assertions pin the modelled values.
const TYPE_KNOWLEDGE_BASE: &str = "KNOWLEDGE_BASE";
const TYPE_EXTERNAL_BEDROCK: &str = "EXTERNAL_BEDROCK_KNOWLEDGE_BASE";

/// A Bedrock knowledge-base ARN inside the modelled `BedrockKnowledgeBaseArn` pattern
/// and length (`^arn:aws(|-cn|-us-gov):bedrock:[a-zA-Z0-9-]*:[0-9]{12}:knowledge-base/[0-9a-zA-Z]+$`,
/// len 47..128) — a resource of ANOTHER service, which this emulator does not hold.
const BEDROCK_ARN: &str = "arn:aws:bedrock:eu-central-1:123456789012:knowledge-base/KB0123ABCD";
const OTHER_BEDROCK_ARN: &str = "arn:aws:bedrock:eu-central-1:123456789012:knowledge-base/KB9876ZYXW";

/// An IAM role ARN inside the modelled `AccessRoleArn` pattern and length
/// (`^arn:aws:iam::[0-9]{12}:role/(?:service-role/)?[a-zA-Z0-9_+=,.@-]{1,64}$`, len
/// 32..2048).
const ACCESS_ROLE_ARN: &str = "arn:aws:iam::123456789012:role/QConnectBedrockAccess";

/// The sentence a create naming a target its assistant is already associated with is
/// refused with. Written out here rather than imported from the implementation, so the
/// assertion pins the wording a client sees; it is FIXED text, with no part taken from
/// the request or from the store.
const MESSAGE_ASSOCIATION_ALREADY_EXISTS: &str =
    "Association already exists for these two resources";

/// One wire answer, reduced to what a client branches on.
#[derive(Debug)]
struct Reply {
    status: u16,
    error_type: String,
    body: Value,
}

impl Reply {
    fn field(&self, pointer: &str) -> Option<&Value> {
        self.body.pointer(pointer).filter(|value| !value.is_null())
    }

    fn string(&self, pointer: &str) -> String {
        self.field(pointer)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }
}

/// An emulator with just this service mounted on a fresh engine, driven in-process
/// through the production router.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Signatures are not what these tests are about; the router still routes by the
        // SigV4 credential scope below, which is also where an ARN's region comes from.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    async fn send(&self, method: &str, uri: &str, body: Option<Value>) -> Reply {
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/wisdom/aws4_request, \
             SignedHeaders=host, Signature=not-verified-in-this-suite"
        );
        let builder = http::Request::builder()
            .method(method)
            .uri(uri)
            .header("host", format!("wisdom.{REGION}.amazonaws.com"))
            .header("authorization", authorization);
        let request = match &body {
            Some(value) => builder
                .header("content-type", "application/json")
                .body(Bytes::from(
                    serde_json::to_vec(value).expect("serialize body"),
                ))
                .expect("build request"),
            None => builder.body(Bytes::new()).expect("build request"),
        };

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_type = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        Reply {
            status,
            error_type,
            body,
        }
    }

    /// An assistant to hang associations off, returned as (id, ARN).
    async fn assistant(&self, name: &str) -> (String, String) {
        let reply = self
            .send(
                "POST",
                "/assistants",
                Some(json!({ "name": name, "type": "AGENT" })),
            )
            .await;
        assert_eq!(
            reply.status, 200,
            "case \"assistant {name}\": expected HTTP 200, got {} with {}",
            reply.status, reply.body
        );
        (
            reply.string("/assistant/assistantId"),
            reply.string("/assistant/assistantArn"),
        )
    }

    /// A knowledge base to associate, returned as (id, ARN).
    async fn knowledge_base(&self, name: &str, knowledge_base_type: &str) -> (String, String) {
        let mut body = json!({ "name": name, "knowledgeBaseType": knowledge_base_type });
        // An `EXTERNAL` knowledge base is its source, so the create carries one: the
        // model documents `sourceConfiguration` as the source of "EXTERNAL or Managed"
        // knowledge bases
        // (`smithy_query(query="doc",shape="CreateKnowledgeBaseRequest",member="sourceConfiguration")`).
        if knowledge_base_type == "EXTERNAL" {
            body["sourceConfiguration"] = json!({
                "appIntegrations": {
                    "appIntegrationArn": format!(
                        "arn:aws:app-integrations:{REGION}:{account}:data-integration/src",
                        account = engine::EMULATOR_ACCOUNT_ID
                    )
                }
            });
        }
        let reply = self.send("POST", "/knowledgeBases", Some(body)).await;
        assert_eq!(
            reply.status, 200,
            "case \"knowledge base {name}\": expected HTTP 200, got {} with {}",
            reply.status, reply.body
        );
        (
            reply.string("/knowledgeBase/knowledgeBaseId"),
            reply.string("/knowledgeBase/knowledgeBaseArn"),
        )
    }

    async fn create_association(&self, assistant_label: &str, body: Value) -> Reply {
        self.send(
            "POST",
            &format!("/assistants/{}/associations", label(assistant_label)),
            Some(body),
        )
        .await
    }

    async fn get_association(&self, assistant_label: &str, association_label: &str) -> Reply {
        self.send(
            "GET",
            &format!(
                "/assistants/{}/associations/{}",
                label(assistant_label),
                label(association_label)
            ),
            None,
        )
        .await
    }

    async fn delete_association(&self, assistant_label: &str, association_label: &str) -> Reply {
        self.send(
            "DELETE",
            &format!(
                "/assistants/{}/associations/{}",
                label(assistant_label),
                label(association_label)
            ),
            None,
        )
        .await
    }

    async fn delete_assistant(&self, assistant_label: &str) -> Reply {
        self.send(
            "DELETE",
            &format!("/assistants/{}", label(assistant_label)),
            None,
        )
        .await
    }
}

/// Percent-encode one path label the way an AWS SDK encodes a non-greedy `@httpLabel`:
/// an ARN label carries `:` and `/`, which must not be read as path structure.
fn label(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                (byte as char).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

/// The assistant-association ARN AWS documents for this service:
/// `arn:aws:wisdom:<region>:<account>:association/<assistantId>/<assistantAssociationId>`.
/// Written out here rather than taken from the handler's helper, so the test pins the
/// format.
fn expected_association_arn(assistant_id: &str, association_id: &str) -> String {
    format!(
        "arn:aws:wisdom:{REGION}:{account}:association/{assistant_id}/{association_id}",
        account = engine::EMULATOR_ACCOUNT_ID
    )
}

/// `association` split into the members a create's request decides and the two the record
/// reports about its own last write (`lastModifiedRegion`, `lastModifiedTime`), so the
/// first can be compared with what the request asked for while the per-run instant is
/// asserted on its own.
fn split_last_modified(case: &str, association: &Value) -> (Value, Value) {
    let members = association
        .as_object()
        .unwrap_or_else(|| panic!("case {case:?}: the answer carries no association object"));
    let last_modified: serde_json::Map<String, Value> = members
        .iter()
        .filter(|(key, _)| key.starts_with("lastModified"))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    let rest: serde_json::Map<String, Value> = members
        .iter()
        .filter(|(key, _)| !key.starts_with("lastModified"))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    (Value::Object(rest), Value::Object(last_modified))
}

/// Whether `value` is a lowercase hyphenated UUID, the form the model's `Uuid` pattern
/// requires (`^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$`).
fn is_modelled_uuid(value: &str) -> bool {
    let groups: Vec<&str> = value.split('-').collect();
    groups.len() == 5
        && [8usize, 4, 4, 4, 12]
            .iter()
            .zip(&groups)
            .all(|(len, group)| {
                group.len() == *len
                    && group
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            })
}

/// What an association is made against in one row of the table.
#[derive(Debug, Clone)]
enum Target {
    /// A knowledge base of this account, created by the row with this modelled
    /// `knowledgeBaseType` (`smithy_query(query="shape",shape="KnowledgeBaseType")`).
    KnowledgeBase(&'static str),
    /// A Bedrock knowledge base outside this service, named by its ARN and the role
    /// that reaches it.
    ExternalBedrock,
}

/// One row of the create/read table.
#[derive(Debug, Clone)]
struct Case {
    label: &'static str,
    target: Target,
    tags: BTreeMap<String, String>,
    client_token: Option<&'static str>,
}

/// The table: one row per axis the create rule generalises over — the target kind (both
/// union members), the knowledge-base type behind it (several modelled values), the tag
/// count (none, one, several) and whether the caller supplied a `clientToken`.
fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "CUSTOM knowledge base, one tag, with a clientToken",
            target: Target::KnowledgeBase("CUSTOM"),
            tags: [("env".to_string(), "test".to_string())]
                .into_iter()
                .collect(),
            client_token: Some("tok-custom"),
        },
        Case {
            label: "EXTERNAL knowledge base, no tags, no clientToken",
            target: Target::KnowledgeBase("EXTERNAL"),
            tags: BTreeMap::new(),
            client_token: None,
        },
        Case {
            label: "QUICK_RESPONSES knowledge base, several tags",
            target: Target::KnowledgeBase("QUICK_RESPONSES"),
            tags: [
                ("team_1".to_string(), "alpha".to_string()),
                ("a.b".to_string(), "1".to_string()),
                ("p-q".to_string(), "2".to_string()),
            ]
            .into_iter()
            .collect(),
            client_token: None,
        },
        Case {
            label: "external Bedrock knowledge base, one tag",
            target: Target::ExternalBedrock,
            tags: [("owner".to_string(), "sim2real".to_string())]
                .into_iter()
                .collect(),
            client_token: Some("tok-bedrock"),
        },
    ]
}

/// A create body for one row, against an already created knowledge base id.
fn create_body(case: &Case, knowledge_base_id: &str) -> Value {
    let (association_type, association) = match case.target {
        Target::KnowledgeBase(_) => (
            TYPE_KNOWLEDGE_BASE,
            json!({ "knowledgeBaseId": knowledge_base_id }),
        ),
        Target::ExternalBedrock => (
            TYPE_EXTERNAL_BEDROCK,
            json!({
                "externalBedrockKnowledgeBaseConfig": {
                    "bedrockKnowledgeBaseArn": BEDROCK_ARN,
                    "accessRoleArn": ACCESS_ROLE_ARN,
                }
            }),
        ),
    };
    let mut body = json!({ "associationType": association_type, "association": association });
    if !case.tags.is_empty() {
        body["tags"] = json!(case.tags);
    }
    if let Some(token) = case.client_token {
        body["clientToken"] = json!(token);
    }
    body
}

/// The `associationData` an association with this row's target reports.
fn expected_association_data(case: &Case, knowledge_base_id: &str, knowledge_base_arn: &str) -> Value {
    match case.target {
        Target::KnowledgeBase(_) => json!({
            "knowledgeBaseAssociation": {
                "knowledgeBaseId": knowledge_base_id,
                "knowledgeBaseArn": knowledge_base_arn,
            }
        }),
        Target::ExternalBedrock => json!({
            "externalBedrockKnowledgeBaseConfig": {
                "bedrockKnowledgeBaseArn": BEDROCK_ARN,
                "accessRoleArn": ACCESS_ROLE_ARN,
            }
        }),
    }
}

/// A create persists what it mints: every row of the table is read back — by id and by
/// ARN — out of the stored record, and ONE assistant holds every one of them, so
/// several distinct targets on one assistant is a success and not a conflict.
#[tokio::test]
async fn a_create_persists_the_association_and_every_read_answers_from_the_record() {
    let emulator = Emulator::new();
    let (assistant_id, assistant_arn) = emulator.assistant("assoc-table-assistant").await;

    for (index, case) in cases().iter().enumerate() {
        let (knowledge_base_id, knowledge_base_arn) = match case.target {
            Target::KnowledgeBase(knowledge_base_type) => {
                emulator
                    .knowledge_base(&format!("assoc-kb-{index}"), knowledge_base_type)
                    .await
            }
            Target::ExternalBedrock => (String::new(), String::new()),
        };
        let created = emulator
            .create_association(&assistant_id, create_body(case, &knowledge_base_id))
            .await;
        assert_eq!(
            created.status, 200,
            "case {:?}: expected HTTP 200, got {} with {}",
            case.label, created.status, created.body
        );

        let association_id = created.string("/assistantAssociation/assistantAssociationId");
        assert!(
            is_modelled_uuid(&association_id),
            "case {:?}: expected a modelled Uuid assistantAssociationId, got {association_id:?}",
            case.label
        );
        let expected_arn = expected_association_arn(&assistant_id, &association_id);
        let expected_tags: Value = json!(case.tags);
        let expected_type = match case.target {
            Target::KnowledgeBase(_) => TYPE_KNOWLEDGE_BASE,
            Target::ExternalBedrock => TYPE_EXTERNAL_BEDROCK,
        };
        let expected = json!({
            "assistantAssociationId": association_id,
            "assistantAssociationArn": expected_arn,
            "assistantId": assistant_id,
            "assistantArn": assistant_arn,
            "associationType": expected_type,
            "associationData": expected_association_data(case, &knowledge_base_id, &knowledge_base_arn),
            "tags": expected_tags,
        });
        let created_association = created
            .field("/assistantAssociation")
            .cloned()
            .unwrap_or(Value::Null);
        // The two members the record reports about its own last write: the Region its ARN
        // names, and the instant it was written — a per-run value, so it is asserted to be
        // a timestamp rather than compared with one.
        let (actual, last_modified) = split_last_modified(case.label, &created_association);
        assert_eq!(
            last_modified.get("lastModifiedRegion").and_then(Value::as_str),
            Some(REGION),
            "case {:?}: the association must report the Region its ARN names, got {last_modified}",
            case.label
        );
        assert!(
            last_modified
                .get("lastModifiedTime")
                .and_then(Value::as_f64)
                .is_some(),
            "case {:?}: the association must report a lastModifiedTime, got {last_modified}",
            case.label
        );
        assert_eq!(
            actual, expected,
            "case {:?}: the create must answer out of the record it stored, expected \
             {expected} got {actual}",
            case.label
        );

        // Both label forms of both labels name the same stored record.
        for (read_label, association_label) in [
            ("association id", association_id.clone()),
            ("association ARN", expected_arn.clone()),
        ] {
            for (parent_label, parent) in [
                ("assistant id", assistant_id.clone()),
                ("assistant ARN", assistant_arn.clone()),
            ] {
                let read = emulator.get_association(&parent, &association_label).await;
                assert_eq!(
                    read.status, 200,
                    "case {:?} read by {read_label} under {parent_label}: expected HTTP 200, \
                     got {} with {}",
                    case.label, read.status, read.body
                );
                let read_actual = read
                    .field("/assistantAssociation")
                    .cloned()
                    .unwrap_or(Value::Null);
                assert_eq!(
                    read_actual, created_association,
                    "case {:?} read by {read_label} under {parent_label}: a read must answer \
                     what the create answered, expected {created_association} got {read_actual}",
                    case.label
                );
            }
        }
    }
}

/// A second create naming a target the assistant is already associated with is the
/// modelled conflict, carrying the one sentence this class is worded with, for every
/// target kind — and the refusal leaves the first association readable.
#[tokio::test]
async fn a_target_the_assistant_already_holds_is_a_conflict() {
    let emulator = Emulator::new();
    let (assistant_id, _) = emulator.assistant("assoc-conflict-assistant").await;

    for case in cases() {
        let (knowledge_base_id, _) = match case.target {
            Target::KnowledgeBase(knowledge_base_type) => {
                emulator
                    .knowledge_base(
                        &format!("conflict-kb-{}", case.label.len()),
                        knowledge_base_type,
                    )
                    .await
            }
            Target::ExternalBedrock => (String::new(), String::new()),
        };
        // The same request twice, with no clientToken: the second is not a retry of the
        // first, it is a second create of an association that already exists.
        let mut body = create_body(&case, &knowledge_base_id);
        body.as_object_mut()
            .expect("a JSON object body")
            .remove("clientToken");

        let first = emulator.create_association(&assistant_id, body.clone()).await;
        assert_eq!(
            first.status, 200,
            "case {:?}: expected the first create to answer HTTP 200, got {} with {}",
            case.label, first.status, first.body
        );
        let association_id = first.string("/assistantAssociation/assistantAssociationId");

        let second = emulator.create_association(&assistant_id, body).await;
        assert_eq!(
            second.status, 409,
            "case {:?}: expected HTTP 409 for a target already associated, got {} with {}",
            case.label, second.status, second.body
        );
        assert!(
            second.error_type.starts_with("ConflictException"),
            "case {:?}: expected ConflictException, got {:?} with {}",
            case.label,
            second.error_type,
            second.body
        );
        // The sentence is FIXED text: one wording for this class, naming neither the
        // assistant, nor the target, nor the association that already holds the pair.
        let message = second.string("/message");
        assert_eq!(
            message, MESSAGE_ASSOCIATION_ALREADY_EXISTS,
            "case {:?}: expected the sentence this class is worded with, got {message:?}",
            case.label
        );
        for quoted in [&assistant_id, &knowledge_base_id] {
            assert!(
                quoted.is_empty() || !message.contains(quoted.as_str()),
                "case {:?}: the conflict must quote nothing from the request or the store, \
                 got {message:?}",
                case.label
            );
        }

        let read = emulator.get_association(&assistant_id, &association_id).await;
        assert_eq!(
            read.status, 200,
            "case {:?}: the refused create must leave the first association readable, got \
             {} with {}",
            case.label, read.status, read.body
        );
    }
}

/// `associationType` and the `association` union name the same thing: a create whose
/// pair disagrees is refused with the modelled `ValidationException`, and nothing is
/// stored.
#[tokio::test]
async fn an_association_type_that_disagrees_with_the_association_is_refused() {
    let emulator = Emulator::new();
    let (assistant_id, _) = emulator.assistant("assoc-mismatch-assistant").await;
    let (knowledge_base_id, _) = emulator.knowledge_base("mismatch-kb", "CUSTOM").await;

    let rows = [
        (
            "knowledge base named under the external Bedrock type",
            TYPE_EXTERNAL_BEDROCK,
            json!({ "knowledgeBaseId": knowledge_base_id }),
        ),
        (
            "external Bedrock config named under the knowledge-base type",
            TYPE_KNOWLEDGE_BASE,
            json!({
                "externalBedrockKnowledgeBaseConfig": {
                    "bedrockKnowledgeBaseArn": BEDROCK_ARN,
                    "accessRoleArn": ACCESS_ROLE_ARN,
                }
            }),
        ),
    ];
    for (case, association_type, association) in rows {
        let reply = emulator
            .create_association(
                &assistant_id,
                json!({ "associationType": association_type, "association": association }),
            )
            .await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: expected HTTP 400, got {} with {}",
            reply.status, reply.body
        );
        assert!(
            reply.error_type.starts_with("ValidationException"),
            "case {case:?}: expected ValidationException, got {:?} with {}",
            reply.error_type,
            reply.body
        );
    }
}

/// An idempotency token identifies ONE request: a replay carrying the identical request
/// answers with the association that token already created, and the SAME token carrying
/// any changed parameter is a conflict rather than a second resource or a silently
/// replayed first one.
#[tokio::test]
async fn a_client_token_replay_is_idempotent_and_a_changed_request_conflicts() {
    let emulator = Emulator::new();
    let (assistant_id, _) = emulator.assistant("assoc-token-assistant").await;
    let (knowledge_base_id, _) = emulator.knowledge_base("token-kb-a", "CUSTOM").await;
    let (other_knowledge_base_id, _) = emulator.knowledge_base("token-kb-b", "CUSTOM").await;
    let token = "tok-idempotent";
    let first_body = json!({
        "associationType": TYPE_KNOWLEDGE_BASE,
        "association": { "knowledgeBaseId": knowledge_base_id },
        "clientToken": token,
        "tags": { "env": "test", "team": "support" },
    });

    let first = emulator
        .create_association(&assistant_id, first_body.clone())
        .await;
    assert_eq!(
        first.status, 200,
        "case \"first create\": expected HTTP 200, got {} with {}",
        first.status, first.body
    );
    let association_id = first.string("/assistantAssociation/assistantAssociationId");

    // The identical request, including a tag map written in another order: one request,
    // so one resource.
    let replay = emulator
        .create_association(
            &assistant_id,
            json!({
                "associationType": TYPE_KNOWLEDGE_BASE,
                "association": { "knowledgeBaseId": knowledge_base_id },
                "clientToken": token,
                "tags": { "team": "support", "env": "test" },
            }),
        )
        .await;
    assert_eq!(
        replay.status, 200,
        "case \"identical replay\": expected HTTP 200, got {} with {}",
        replay.status, replay.body
    );
    let replayed_id = replay.string("/assistantAssociation/assistantAssociationId");
    assert_eq!(
        replayed_id, association_id,
        "case \"identical replay\": expected the association {association_id} the token \
         already created, got {replayed_id}"
    );

    let changed = [
        (
            "a different knowledge base",
            json!({
                "associationType": TYPE_KNOWLEDGE_BASE,
                "association": { "knowledgeBaseId": other_knowledge_base_id },
                "clientToken": token,
                "tags": { "env": "test", "team": "support" },
            }),
        ),
        (
            "different tags",
            json!({
                "associationType": TYPE_KNOWLEDGE_BASE,
                "association": { "knowledgeBaseId": knowledge_base_id },
                "clientToken": token,
                "tags": { "env": "prod" },
            }),
        ),
        (
            "no tags at all",
            json!({
                "associationType": TYPE_KNOWLEDGE_BASE,
                "association": { "knowledgeBaseId": knowledge_base_id },
                "clientToken": token,
            }),
        ),
        (
            "a different target kind",
            json!({
                "associationType": TYPE_EXTERNAL_BEDROCK,
                "association": {
                    "externalBedrockKnowledgeBaseConfig": {
                        "bedrockKnowledgeBaseArn": BEDROCK_ARN,
                        "accessRoleArn": ACCESS_ROLE_ARN,
                    }
                },
                "clientToken": token,
            }),
        ),
    ];
    for (case, body) in changed {
        let reply = emulator.create_association(&assistant_id, body).await;
        assert_eq!(
            reply.status, 409,
            "case {case:?}: expected HTTP 409 for a token reused with a changed request, got \
             {} with {}",
            reply.status, reply.body
        );
        assert!(
            reply.error_type.starts_with("ConflictException"),
            "case {case:?}: expected ConflictException, got {:?} with {}",
            reply.error_type,
            reply.body
        );
    }

    // The token still names the association the first call created, and nothing else was
    // stored under it.
    let read = emulator.get_association(&assistant_id, &association_id).await;
    assert_eq!(
        read.status, 200,
        "case \"after the refused reuses\": expected the first association to still read \
         back, got {} with {}",
        read.status, read.body
    );
}

/// A label this service holds nothing under is the modelled not-found, for every way a
/// request can name something absent.
#[tokio::test]
async fn labels_that_name_nothing_are_not_found() {
    let emulator = Emulator::new();
    let (assistant_id, _) = emulator.assistant("assoc-404-assistant").await;
    let (other_assistant_id, _) = emulator.assistant("assoc-404-other-assistant").await;
    let (knowledge_base_id, _) = emulator.knowledge_base("notfound-kb", "CUSTOM").await;
    let created = emulator
        .create_association(
            &assistant_id,
            json!({
                "associationType": TYPE_KNOWLEDGE_BASE,
                "association": { "knowledgeBaseId": knowledge_base_id },
            }),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "case \"setup create\": expected HTTP 200, got {} with {}",
        created.status, created.body
    );
    let association_id = created.string("/assistantAssociation/assistantAssociationId");

    // A create whose parent, or whose knowledge base, this account does not hold.
    let create_rows = [
        ("an assistant this account does not hold", ABSENT_UUID.to_string(), knowledge_base_id.clone()),
        ("a knowledge base this account does not hold", assistant_id.clone(), ABSENT_UUID.to_string()),
    ];
    for (case, parent, knowledge_base) in create_rows {
        let reply = emulator
            .create_association(
                &parent,
                json!({
                    "associationType": TYPE_KNOWLEDGE_BASE,
                    "association": { "knowledgeBaseId": knowledge_base },
                }),
            )
            .await;
        assert_eq!(
            reply.status, 404,
            "case {case:?}: expected HTTP 404, got {} with {}",
            reply.status, reply.body
        );
        assert!(
            reply.error_type.starts_with("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException, got {:?} with {}",
            reply.error_type,
            reply.body
        );
    }

    // A read whose parent, or whose association, is not there — including an
    // association that exists but belongs to ANOTHER assistant, and an ARN of another
    // account, which names nothing here.
    let read_rows = [
        (
            "an assistant this account does not hold",
            ABSENT_UUID.to_string(),
            association_id.clone(),
        ),
        (
            "an association id nothing was created under",
            assistant_id.clone(),
            ABSENT_UUID.to_string(),
        ),
        (
            "an association of another assistant",
            other_assistant_id.clone(),
            association_id.clone(),
        ),
        (
            "an association ARN of another account",
            assistant_id.clone(),
            format!(
                "arn:aws:wisdom:{REGION}:999999999999:association/{assistant_id}/{association_id}"
            ),
        ),
    ];
    for (case, parent, association) in read_rows {
        for method in ["GET", "DELETE"] {
            let reply = match method {
                "GET" => emulator.get_association(&parent, &association).await,
                _ => emulator.delete_association(&parent, &association).await,
            };
            assert_eq!(
                reply.status, 404,
                "case {case:?} on {method}: expected HTTP 404, got {} with {}",
                reply.status, reply.body
            );
            assert!(
                reply.error_type.starts_with("ResourceNotFoundException"),
                "case {case:?} on {method}: expected ResourceNotFoundException, got {:?} with {}",
                reply.error_type,
                reply.body
            );
        }
    }
}

/// A delete removes the record: the association is no longer readable, the same target
/// can be associated again, and deleting the assistant takes its associations with it.
#[tokio::test]
async fn a_delete_removes_the_association_and_frees_its_target() {
    let emulator = Emulator::new();
    let (assistant_id, _) = emulator.assistant("assoc-delete-assistant").await;
    let (knowledge_base_id, _) = emulator.knowledge_base("delete-kb", "CUSTOM").await;
    let body = json!({
        "associationType": TYPE_KNOWLEDGE_BASE,
        "association": { "knowledgeBaseId": knowledge_base_id },
    });

    let created = emulator
        .create_association(&assistant_id, body.clone())
        .await;
    let association_id = created.string("/assistantAssociation/assistantAssociationId");
    assert_eq!(
        created.status, 200,
        "case \"create before delete\": expected HTTP 200, got {} with {}",
        created.status, created.body
    );

    let deleted = emulator
        .delete_association(&assistant_id, &association_id)
        .await;
    assert_eq!(
        deleted.status, 204,
        "case \"delete\": expected HTTP 204, got {} with {}",
        deleted.status, deleted.body
    );

    let read = emulator.get_association(&assistant_id, &association_id).await;
    assert_eq!(
        read.status, 404,
        "case \"read after delete\": expected HTTP 404, got {} with {}",
        read.status, read.body
    );

    // The target is free again, and the create that takes it mints a NEW association.
    let recreated = emulator.create_association(&assistant_id, body).await;
    assert_eq!(
        recreated.status, 200,
        "case \"re-create after delete\": expected HTTP 200, got {} with {}",
        recreated.status, recreated.body
    );
    let recreated_id = recreated.string("/assistantAssociation/assistantAssociationId");
    assert_ne!(
        recreated_id, association_id,
        "case \"re-create after delete\": expected a new assistantAssociationId, got the \
         deleted one {association_id}"
    );

    // An association lives inside its assistant, so nothing of it survives the parent.
    let gone = emulator.delete_assistant(&assistant_id).await;
    assert_eq!(
        gone.status, 204,
        "case \"delete the assistant\": expected HTTP 204, got {} with {}",
        gone.status, gone.body
    );
    let orphan = emulator.get_association(&assistant_id, &recreated_id).await;
    assert_eq!(
        orphan.status, 404,
        "case \"read after the assistant is deleted\": expected HTTP 404, got {} with {}",
        orphan.status, orphan.body
    );
}

/// Two assistants can be associated with the SAME knowledge base: the one-association-
/// per-target rule is per assistant, not per knowledge base.
#[tokio::test]
async fn one_knowledge_base_can_be_associated_with_several_assistants() {
    let emulator = Emulator::new();
    let (knowledge_base_id, knowledge_base_arn) =
        emulator.knowledge_base("shared-kb", "CUSTOM").await;
    let mut seen = Vec::new();

    for name in ["shared-assistant-a", "shared-assistant-b", "shared-assistant-c"] {
        let (assistant_id, _) = emulator.assistant(name).await;
        let created = emulator
            .create_association(
                &assistant_id,
                json!({
                    "associationType": TYPE_KNOWLEDGE_BASE,
                    "association": { "knowledgeBaseId": knowledge_base_id },
                }),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case {name:?}: expected HTTP 200, got {} with {}",
            created.status, created.body
        );
        let reported = created.string("/assistantAssociation/associationData/knowledgeBaseAssociation/knowledgeBaseArn");
        assert_eq!(
            reported, knowledge_base_arn,
            "case {name:?}: expected the knowledge base's own ARN {knowledge_base_arn}, got \
             {reported}"
        );
        let association_id = created.string("/assistantAssociation/assistantAssociationId");
        assert!(
            !seen.contains(&association_id),
            "case {name:?}: expected a fresh assistantAssociationId, got the already seen \
             {association_id}"
        );
        seen.push(association_id);
    }
}

/// The external Bedrock target is reported back out of the stored record, including when
/// two associations of one assistant name different Bedrock knowledge bases.
#[tokio::test]
async fn distinct_external_bedrock_targets_are_distinct_associations() {
    let emulator = Emulator::new();
    let (assistant_id, _) = emulator.assistant("bedrock-assistant").await;

    let mut ids = Vec::new();
    for (case, bedrock_arn) in [("first Bedrock base", BEDROCK_ARN), ("second Bedrock base", OTHER_BEDROCK_ARN)] {
        let created = emulator
            .create_association(
                &assistant_id,
                json!({
                    "associationType": TYPE_EXTERNAL_BEDROCK,
                    "association": {
                        "externalBedrockKnowledgeBaseConfig": {
                            "bedrockKnowledgeBaseArn": bedrock_arn,
                            "accessRoleArn": ACCESS_ROLE_ARN,
                        }
                    },
                }),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected HTTP 200, got {} with {}",
            created.status, created.body
        );
        let reported = created.string(
            "/assistantAssociation/associationData/externalBedrockKnowledgeBaseConfig/bedrockKnowledgeBaseArn",
        );
        assert_eq!(
            reported, bedrock_arn,
            "case {case:?}: expected the stored bedrockKnowledgeBaseArn {bedrock_arn}, got \
             {reported}"
        );
        ids.push(created.string("/assistantAssociation/assistantAssociationId"));
    }
    assert_ne!(
        ids[0], ids[1],
        "case \"two distinct Bedrock targets\": expected two associations, got the same id \
         {} twice",
        ids[0]
    );
}

/// `ListAssistantAssociations` on one assistant.
async fn list_associations(
    emulator: &Emulator,
    assistant_label: &str,
    max_results: Option<usize>,
    next_token: Option<&str>,
) -> Reply {
    let mut uri = format!("/assistants/{}/associations", label(assistant_label));
    let mut query = Vec::new();
    if let Some(max_results) = max_results {
        query.push(format!("maxResults={max_results}"));
    }
    if let Some(next_token) = next_token {
        query.push(format!("nextToken={next_token}"));
    }
    if !query.is_empty() {
        uri.push('?');
        uri.push_str(&query.join("&"));
    }
    emulator.send("GET", &uri, None).await
}

/// The `assistantAssociationId`s one page reports, in the order it reported them.
fn listed_ids(case: &str, reply: &Reply) -> Vec<String> {
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected HTTP 200, got {} with {}",
        reply.status, reply.body
    );
    reply
        .field("/assistantAssociationSummaries")
        .and_then(Value::as_array)
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: assistantAssociationSummaries is required, got {}",
                reply.body
            )
        })
        .iter()
        .map(|summary| {
            summary
                .get("assistantAssociationId")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// RULE: a listing reports the associations the assistant HOLDS, projected out of the
/// stored records, bounded by `maxResults` and continued by the `nextToken` it hands out
/// until every one has been served exactly once; the listing is scoped to its own
/// assistant, follows a create and a delete, and an assistant this account does not hold
/// is `ResourceNotFoundException` (HTTP 404).
#[tokio::test]
async fn a_listing_reports_the_associations_the_assistant_holds() {
    let emulator = Emulator::new();
    let (assistant_id, assistant_arn) = emulator.assistant("assoc-listing-assistant").await;
    let (other_assistant_id, _) = emulator.assistant("assoc-listing-other").await;

    // An assistant holding nothing reports an empty page and no continuation.
    let empty = list_associations(&emulator, &assistant_id, None, None).await;
    assert!(
        listed_ids("an assistant holding no association", &empty).is_empty(),
        "case \"an assistant holding no association\": expected an empty page, got {}",
        empty.body
    );
    assert_eq!(
        empty.field("/nextToken"),
        None,
        "case \"an assistant holding no association\": expected no nextToken, got {}",
        empty.body
    );

    // Several associations with DISTINCT targets, so the listing has rows to page: three
    // knowledge bases of this account and one external Bedrock target.
    let mut created: Vec<String> = Vec::new();
    for knowledge_base_type in ["CUSTOM", "EXTERNAL", "QUICK_RESPONSES"] {
        let (knowledge_base_id, _) = emulator
            .knowledge_base(
                &format!("listing-kb-{}", knowledge_base_type.to_lowercase()),
                knowledge_base_type,
            )
            .await;
        let reply = emulator
            .create_association(
                &assistant_id,
                json!({
                    "associationType": TYPE_KNOWLEDGE_BASE,
                    "association": { "knowledgeBaseId": knowledge_base_id },
                }),
            )
            .await;
        assert_eq!(
            reply.status, 200,
            "case {knowledge_base_type:?}: expected the association to be created, got {} with {}",
            reply.status, reply.body
        );
        created.push(reply.string("/assistantAssociation/assistantAssociationId"));
    }
    let bedrock = emulator
        .create_association(
            &assistant_id,
            json!({
                "associationType": TYPE_EXTERNAL_BEDROCK,
                "association": {
                    "externalBedrockKnowledgeBaseConfig": {
                        "bedrockKnowledgeBaseArn": BEDROCK_ARN,
                        "accessRoleArn": ACCESS_ROLE_ARN,
                    }
                },
            }),
        )
        .await;
    assert_eq!(
        bedrock.status, 200,
        "case \"the Bedrock target\": expected the association to be created, got {} with {}",
        bedrock.status, bedrock.body
    );
    created.push(bedrock.string("/assistantAssociation/assistantAssociationId"));
    let mut expected = created.clone();
    expected.sort();

    // One page holding everything, and every summary projected out of the stored record:
    // what a page reports is what `GetAssistantAssociation` answers for the same id.
    let whole = list_associations(&emulator, &assistant_id, None, None).await;
    let listed = listed_ids("the whole listing", &whole);
    assert_eq!(
        listed, expected,
        "case \"the whole listing\": expected every association in id order, got {listed:?}"
    );
    let summaries = whole
        .field("/assistantAssociationSummaries")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for summary in &summaries {
        let id = summary
            .get("assistantAssociationId")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let read = emulator.get_association(&assistant_id, id).await;
        assert_eq!(
            read.status, 200,
            "case {id:?}: the listed association must read back, got {} with {}",
            read.status, read.body
        );
        for member in [
            "assistantAssociationId",
            "assistantAssociationArn",
            "assistantId",
            "assistantArn",
            "associationType",
            "associationData",
        ] {
            let expected_member = read.field(&format!("/assistantAssociation/{member}")).cloned();
            let actual = summary.get(member).filter(|value| !value.is_null()).cloned();
            assert_eq!(
                actual, expected_member,
                "case {id:?}: the summary's {member} must be the stored one, got {actual:?} \
                 against {expected_member:?}"
            );
        }
        assert_eq!(
            summary.get("assistantArn").and_then(Value::as_str),
            Some(assistant_arn.as_str()),
            "case {id:?}: expected the parent's own ARN, got {summary}"
        );
    }

    // The listing is scoped to its own assistant: the other one holds none of these.
    let other = list_associations(&emulator, &other_assistant_id, None, None).await;
    assert!(
        listed_ids("another assistant", &other).is_empty(),
        "case \"another assistant\": expected an empty page, got {}",
        other.body
    );

    // Paging: every page size from one to past the selection, walked to exhaustion.
    for page_size in [1usize, 2, 3, 4, 5] {
        let case = format!("pages of {page_size}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let page = list_associations(&emulator, &assistant_id, Some(page_size), token.as_deref())
                .await;
            let ids = listed_ids(&case, &page);
            assert!(
                ids.len() <= page_size,
                "case {case:?}: a page may not exceed maxResults, got {} entries",
                ids.len()
            );
            seen.extend(ids);
            token = page.field("/nextToken").and_then(Value::as_str).map(str::to_string);
            pages += 1;
            assert!(
                pages <= expected.len() + 2,
                "case {case:?}: the walk must terminate, got {pages} pages"
            );
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            seen, expected,
            "case {case:?}: the walk must serve every association exactly once, in one order, \
             got {seen:?}"
        );
    }

    // A delete is reflected in the next page, and so is the parent's own delete.
    let removed = expected[0].clone();
    assert_eq!(
        emulator
            .delete_association(&assistant_id, &removed)
            .await
            .status,
        204,
        "case \"delete one association\": expected HTTP 204"
    );
    let after = list_associations(&emulator, &assistant_id, None, None).await;
    let listed = listed_ids("after a delete", &after);
    assert_eq!(
        listed,
        expected[1..].to_vec(),
        "case \"after a delete\": expected the remaining associations, got {listed:?}"
    );

    // A token this service did not mint names no position.
    let bad_token = list_associations(&emulator, &assistant_id, None, Some("not-a-token")).await;
    assert_eq!(
        (bad_token.status, bad_token.error_type.as_str()),
        (400, "ValidationException"),
        "case \"a token this service did not mint\": expected HTTP 400 ValidationException, got \
         {} {:?} with {}",
        bad_token.status,
        bad_token.error_type,
        bad_token.body
    );

    // An assistant this account does not hold is not found rather than an empty page, by
    // either label form.
    for (case, assistant_label) in [
        ("a bare id nothing holds", ABSENT_UUID.to_string()),
        (
            "an ARN of another account",
            format!("arn:aws:wisdom:{REGION}:999999999999:assistant/{assistant_id}"),
        ),
    ] {
        let reply = list_associations(&emulator, &assistant_label, None, None).await;
        assert_eq!(
            (reply.status, reply.error_type.as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: expected HTTP 404 ResourceNotFoundException, got {} {:?} with {}",
            reply.status,
            reply.error_type,
            reply.body
        );
    }
}
