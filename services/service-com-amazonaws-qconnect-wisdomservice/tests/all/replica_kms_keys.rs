// SPDX-License-Identifier: CC0-1.0

//! The replica-Region KMS keys a resource carries, and the Region and instant every
//! projection of a resource reports it was last modified in.
//!
//! Every assertion goes over the wire, through the same [`MultiServiceRouter`] the
//! production emulator mounts: a request is built, dispatched, and only the HTTP status,
//! the `x-amzn-errortype` header and the JSON body are inspected, so an answer that is not
//! computed from persisted state cannot pass.
//!
//! The rules under test, each asserted over a TABLE rather than one request:
//!
//! * `RegisterAssistantReplicaKey` and `RegisterKnowledgeBaseReplicaKey` register the
//!   replica-Region KMS key the request carries on the resource the route names, so every
//!   later read of that resource reports it among its encryption configuration's
//!   `replicaKmsKeys` — once per key, in the order they were registered, whether or not
//!   the create named a `kmsKeyId` of its own;
//! * a registration past the bound the model puts on that list
//!   (`smithy_query(query="shape",shape="ServerSideEncryptionConfiguration")`: 1 to 5
//!   entries) is refused and stores nothing;
//! * a replica key for a resource this account does not hold is the modelled not-found,
//!   and the same `@idempotencyToken` carrying another key is the modelled idempotency
//!   conflict;
//! * every projection of a resource — the write's own answer, the read, and the summary a
//!   listing reports — reports `lastModifiedRegion` as the Region in that resource's own
//!   ARN and `lastModifiedTime` as the instant the record was last written, which a
//!   registration moves forward and a read never does.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_qconnect_wisdomservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The Regions the suite signs with: an ARN's Region comes from the SigV4 credential
/// scope, so one row per Region shape the modelled `AwsRegion`
/// (`smithy_query(query="shape",shape="AwsRegion")`: len 8..31, pattern
/// `^[a-z]{2}(-[a-z]+){1,2}(-[0-9])?$`) admits — two segments and three, and the shortest
/// and longest names in that set this suite uses.
const REGIONS: [&str; 3] = ["us-east-1", "eu-west-1", "ap-southeast-2"];

/// The most replica-Region KMS keys one resource's encryption configuration may carry,
/// written out here rather than imported so the assertion pins the modelled bound: the
/// `@length` of `ServerSideEncryptionConfiguration$replicaKmsKeys` is `min: 1, max: 5`.
const MAX_REPLICA_KMS_KEYS: usize = 5;

/// A UUID no create ever mints, for the not-found rows.
const ABSENT_UUID: &str = "ffffffff-ffff-ffff-ffff-ffffffffffff";

/// The service's fixed sentences these tests reach, written out here rather than imported
/// from `state.rs` so the assertion states the text instead of restating the
/// implementation's constant.
const MESSAGE_ASSISTANT_NOT_FOUND: &str = "Assistant does not exist.";
const MESSAGE_KNOWLEDGE_BASE_NOT_FOUND: &str = "Knowledge Base does not exist.";
const MESSAGE_IDEMPOTENCY_CONFLICT: &str = "Idempotency conflict";

/// The whole sentence a registration past [`MAX_REPLICA_KMS_KEYS`] is refused with: fixed
/// text naming the ceiling and the resource it is per, and nothing from the request.
fn expected_quota_message(resource: &str) -> String {
    format!(
        "The maximum number of replica Region KMS keys ({MAX_REPLICA_KMS_KEYS}) for this \
         {resource} has been reached."
    )
}

/// A KMS key ARN in a replica Region, as a caller registers one. The member is a plain
/// `NonEmptyString` in the model, so the value is the caller's: these are only ever
/// stored and reported back.
fn replica_key(region: &str, index: usize) -> String {
    format!("arn:aws:kms:{region}:111122223333:key/0000000{index}-1111-2222-3333-444444444444")
}

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

    fn string(&self, pointer: &str) -> Option<String> {
        self.field(pointer)
            .and_then(Value::as_str)
            .map(str::to_string)
    }

    fn number(&self, pointer: &str) -> Option<f64> {
        self.field(pointer).and_then(Value::as_f64)
    }
}

/// An emulator with this service mounted on a fresh engine, driven in-process through the
/// production router.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Signatures are not what these tests are about; the router still routes by the
        // SigV4 credential scope below, which is also where an ARN's Region comes from.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    async fn send(&self, method: &str, uri: &str, region: &str, body: Option<Value>) -> Reply {
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/wisdom/aws4_request, \
             SignedHeaders=host, Signature=not-verified-in-this-suite"
        );
        let builder = http::Request::builder()
            .method(method)
            .uri(uri)
            .header("host", format!("wisdom.{region}.amazonaws.com"))
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
}

/// The two resource planes a replica key is registered on. Both operations write the same
/// member of the same shape, so the rules are asserted over both.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Plane {
    Assistant,
    KnowledgeBase,
}

impl Plane {
    /// The resource as the refusal sentence names it.
    fn resource(self) -> &'static str {
        match self {
            Self::Assistant => "assistant",
            Self::KnowledgeBase => "knowledge base",
        }
    }

    /// The collection route of this plane.
    fn collection(self) -> &'static str {
        match self {
            Self::Assistant => "/assistants",
            Self::KnowledgeBase => "/knowledgeBases",
        }
    }

    /// A JSON pointer into the single resource a write or a read answers with.
    fn pointer(self, member: &str) -> String {
        match self {
            Self::Assistant => format!("/assistant{member}"),
            Self::KnowledgeBase => format!("/knowledgeBase{member}"),
        }
    }

    /// The member of the resource that carries its own id, and the one that carries its
    /// ARN.
    fn id_member(self) -> &'static str {
        match self {
            Self::Assistant => "/assistantId",
            Self::KnowledgeBase => "/knowledgeBaseId",
        }
    }

    fn arn_member(self) -> &'static str {
        match self {
            Self::Assistant => "/assistantArn",
            Self::KnowledgeBase => "/knowledgeBaseArn",
        }
    }

    /// The listing's array of summaries, and the member of a summary that carries its id.
    fn summaries(self) -> &'static str {
        match self {
            Self::Assistant => "/assistantSummaries",
            Self::KnowledgeBase => "/knowledgeBaseSummaries",
        }
    }

    fn not_found_message(self) -> &'static str {
        match self {
            Self::Assistant => MESSAGE_ASSISTANT_NOT_FOUND,
            Self::KnowledgeBase => MESSAGE_KNOWLEDGE_BASE_NOT_FOUND,
        }
    }

    /// The create this plane takes, with `kms_key_id` named only when the row asks for a
    /// resource encrypted with a customer managed key of its own.
    fn create_body(self, name: &str, kms_key_id: Option<&str>) -> Value {
        let mut body = match self {
            Self::Assistant => json!({ "name": name, "type": "AGENT" }),
            Self::KnowledgeBase => json!({ "name": name, "knowledgeBaseType": "CUSTOM" }),
        };
        if let Some(kms_key_id) = kms_key_id {
            body["serverSideEncryptionConfiguration"] = json!({ "kmsKeyId": kms_key_id });
        }
        body
    }
}

/// Create one resource of `plane`, answering its create reply.
async fn create(emulator: &Emulator, plane: Plane, region: &str, name: &str, kms: Option<&str>) -> Reply {
    emulator
        .send(
            "POST",
            plane.collection(),
            region,
            Some(plane.create_body(name, kms)),
        )
        .await
}

/// `RegisterAssistantReplicaKey` / `RegisterKnowledgeBaseReplicaKey`
/// (`PUT /assistants/{assistantId}/replicaKey`, `PUT /knowledgeBases/{knowledgeBaseId}/replicaKey`).
async fn register(
    emulator: &Emulator,
    plane: Plane,
    region: &str,
    id: &str,
    key: &str,
    client_token: Option<&str>,
) -> Reply {
    let mut body = json!({ "replicaKmsKey": { "arn": key } });
    if let Some(client_token) = client_token {
        body["clientToken"] = json!(client_token);
    }
    emulator
        .send(
            "PUT",
            &format!("{}/{id}/replicaKey", plane.collection()),
            region,
            Some(body),
        )
        .await
}

/// The read of one resource: `GetAssistant` / `GetKnowledgeBase`.
async fn read(emulator: &Emulator, plane: Plane, region: &str, id: &str) -> Reply {
    emulator
        .send("GET", &format!("{}/{id}", plane.collection()), region, None)
        .await
}

/// The listing of this plane: `ListAssistants` / `ListKnowledgeBases`.
async fn list(emulator: &Emulator, plane: Plane, region: &str) -> Reply {
    emulator.send("GET", plane.collection(), region, None).await
}

/// The `arn` of every registered replica key, in the order the answer reports them.
fn registered_keys(reply: &Reply, pointer: &str) -> Vec<String> {
    reply
        .field(pointer)
        .and_then(Value::as_array)
        .map(|keys| {
            keys.iter()
                .map(|key| {
                    key.get("arn")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The Region field of an ARN: `arn:aws:<service>:<region>:<account>:<resource>`.
fn arn_region(arn: &str) -> String {
    arn.split(':').nth(3).unwrap_or_default().to_string()
}

/// One row of the registration table: the keys a resource has registered on it, in order,
/// and the keys every later read of that resource must then report.
#[derive(Debug)]
struct Registration {
    case: &'static str,
    /// The customer managed key the create named, or `None` for a create that named no
    /// encryption configuration at all.
    kms: Option<&'static str>,
    /// The keys registered, in order, as indices into [`replica_key`].
    registered: Vec<usize>,
    expected: Vec<usize>,
}

/// The table: one row per axis the registration rule generalises over — whether the
/// resource was created with a customer managed key of its own, how many keys are
/// registered, and a key registered twice, immediately and after another one.
fn registration_rows() -> Vec<Registration> {
    /// A customer managed key a create names for the resource itself, which a replica key
    /// never replaces.
    const CUSTOMER_KEY: &str =
        "arn:aws:kms:us-east-1:111122223333:key/abcd1234-1111-2222-3333-444444444444";

    vec![
        Registration {
            case: "one key on a resource whose create named a customer managed key",
            kms: Some(CUSTOMER_KEY),
            registered: vec![0],
            expected: vec![0],
        },
        Registration {
            case: "one key on a resource whose create named no encryption configuration",
            kms: None,
            registered: vec![0],
            expected: vec![0],
        },
        Registration {
            case: "three keys, reported in the order they were registered",
            kms: None,
            registered: vec![0, 1, 2],
            expected: vec![0, 1, 2],
        },
        Registration {
            case: "the same key registered twice is registered once",
            kms: Some(CUSTOMER_KEY),
            registered: vec![1, 1],
            expected: vec![1],
        },
        Registration {
            case: "a key registered again after another one keeps its place",
            kms: None,
            registered: vec![2, 3, 2],
            expected: vec![2, 3],
        },
    ]
}

#[tokio::test]
async fn a_registered_replica_key_is_reported_by_every_later_read() {
    let rows = registration_rows();

    for plane in [Plane::Assistant, Plane::KnowledgeBase] {
        for (index, row) in rows.iter().enumerate() {
            let Registration {
                case,
                kms,
                registered,
                expected,
            } = row;
            let emulator = Emulator::new();
            let region = REGIONS[index % REGIONS.len()];
            let created = create(
                &emulator,
                plane,
                region,
                &format!("replica-key-{index}"),
                *kms,
            )
            .await;
            assert_eq!(
                created.status, 200,
                "case {case:?} ({plane:?}): the create must be accepted, got {} with body {}",
                created.status, created.body
            );
            let id = created
                .string(&plane.pointer(plane.id_member()))
                .unwrap_or_else(|| panic!("case {case:?} ({plane:?}): the create answered no id"));

            for key in registered {
                let reply = register(
                    &emulator,
                    plane,
                    region,
                    &id,
                    &replica_key(region, *key),
                    None,
                )
                .await;
                assert_eq!(
                    reply.status, 200,
                    "case {case:?} ({plane:?}): registering key {key} must be accepted, got {} \
                     with body {}",
                    reply.status, reply.body
                );
            }

            let expected: Vec<String> = expected
                .iter()
                .map(|key| replica_key(region, *key))
                .collect();
            let keys_pointer =
                plane.pointer("/serverSideEncryptionConfiguration/replicaKmsKeys");
            // The registration's own answer, the read of the stored record and the summary
            // a listing reports all answer out of the same record.
            let last_register = register(
                &emulator,
                plane,
                region,
                &id,
                &replica_key(region, registered[registered.len() - 1]),
                None,
            )
            .await;
            assert_eq!(
                registered_keys(&last_register, &keys_pointer),
                expected,
                "case {case:?} ({plane:?}): the registration must answer with the stored keys, \
                 got body {}",
                last_register.body
            );
            let stored = read(&emulator, plane, region, &id).await;
            assert_eq!(
                registered_keys(&stored, &keys_pointer),
                expected,
                "case {case:?} ({plane:?}): the read must report the stored keys, got body {}",
                stored.body
            );
            // The customer managed key the create named is untouched by a registration.
            assert_eq!(
                stored.string(&plane.pointer("/serverSideEncryptionConfiguration/kmsKeyId")),
                kms.map(str::to_string),
                "case {case:?} ({plane:?}): the stored kmsKeyId must be the one the create \
                 named, got body {}",
                stored.body
            );
        }
    }
}

#[tokio::test]
async fn registering_past_the_modelled_maximum_is_refused_and_stores_nothing() {
    for plane in [Plane::Assistant, Plane::KnowledgeBase] {
        let emulator = Emulator::new();
        let region = REGIONS[0];
        let created = create(&emulator, plane, region, "replica-key-bound", None).await;
        let id = created
            .string(&plane.pointer(plane.id_member()))
            .unwrap_or_else(|| panic!("{plane:?}: the create answered no id"));

        // Every key up to the bound is registered; the one past it is refused.
        for key in 0..MAX_REPLICA_KMS_KEYS {
            let reply = register(&emulator, plane, region, &id, &replica_key(region, key), None).await;
            assert_eq!(
                reply.status, 200,
                "{plane:?}: key {key} of {MAX_REPLICA_KMS_KEYS} must be accepted, got {} with \
                 body {}",
                reply.status, reply.body
            );
        }
        let refused = register(
            &emulator,
            plane,
            region,
            &id,
            &replica_key(region, MAX_REPLICA_KMS_KEYS),
            None,
        )
        .await;
        assert_eq!(
            refused.status, 400,
            "{plane:?}: the key past the bound must be refused with HTTP 400, got {} with body \
             {}",
            refused.status, refused.body
        );
        assert!(
            refused.error_type.contains("ValidationException"),
            "{plane:?}: expected a ValidationException, got {:?} with body {}",
            refused.error_type,
            refused.body
        );
        assert_eq!(
            refused.string("/message"),
            Some(expected_quota_message(plane.resource())),
            "{plane:?}: expected the fixed ceiling sentence, got {:?}",
            refused.field("/message")
        );

        // Refused, so nothing was stored: the resource still carries the keys it had, and
        // the refused key is not among them.
        let stored = read(&emulator, plane, region, &id).await;
        let keys = registered_keys(
            &stored,
            &plane.pointer("/serverSideEncryptionConfiguration/replicaKmsKeys"),
        );
        assert_eq!(
            keys.len(),
            MAX_REPLICA_KMS_KEYS,
            "{plane:?}: the refused registration must store nothing, got body {}",
            stored.body
        );
        assert!(
            !keys.contains(&replica_key(region, MAX_REPLICA_KMS_KEYS)),
            "{plane:?}: the refused key must not be stored, got {keys:?}"
        );
    }
}

#[tokio::test]
async fn a_replica_key_for_a_resource_this_account_does_not_hold_is_not_found() {
    for plane in [Plane::Assistant, Plane::KnowledgeBase] {
        let emulator = Emulator::new();
        let region = REGIONS[0];
        // One resource exists, so the refusal is about the LABEL and not about an empty
        // store.
        create(&emulator, plane, region, "replica-key-neighbour", None).await;

        // One row per label form the modelled `UuidOrArn` admits and this account holds
        // nothing under: a well-formed id nothing was minted for, and an ARN of another
        // account.
        let absent = [
            ("a well-formed id nothing holds", ABSENT_UUID.to_string()),
            (
                "an ARN of another account",
                format!(
                    "arn:aws:wisdom:{region}:999988887777:{}/{ABSENT_UUID}",
                    match plane {
                        Plane::Assistant => "assistant",
                        Plane::KnowledgeBase => "knowledge-base",
                    }
                )
                .replace('/', "%2F"),
            ),
        ];
        for (case, label) in absent {
            let refused = register(
                &emulator,
                plane,
                region,
                &label,
                &replica_key(region, 0),
                None,
            )
            .await;
            assert_eq!(
                refused.status, 404,
                "case {case:?} ({plane:?}): expected HTTP 404, got {} with body {}",
                refused.status, refused.body
            );
            assert!(
                refused.error_type.contains("ResourceNotFoundException"),
                "case {case:?} ({plane:?}): expected a ResourceNotFoundException, got {:?} with \
                 body {}",
                refused.error_type,
                refused.body
            );
            assert_eq!(
                refused.string("/message").as_deref(),
                Some(plane.not_found_message()),
                "case {case:?} ({plane:?}): expected the fixed not-found sentence, got {:?}",
                refused.field("/message")
            );
        }
    }
}

#[tokio::test]
async fn an_idempotency_token_identifies_one_registration() {
    for plane in [Plane::Assistant, Plane::KnowledgeBase] {
        let emulator = Emulator::new();
        let region = REGIONS[0];
        let created = create(&emulator, plane, region, "replica-key-token", None).await;
        let id = created
            .string(&plane.pointer(plane.id_member()))
            .unwrap_or_else(|| panic!("{plane:?}: the create answered no id"));
        let keys_pointer = plane.pointer("/serverSideEncryptionConfiguration/replicaKmsKeys");
        let token = "a-client-token-for-one-registration";

        let first = register(
            &emulator,
            plane,
            region,
            &id,
            &replica_key(region, 0),
            Some(token),
        )
        .await;
        assert_eq!(
            first.status, 200,
            "{plane:?}: the first registration must be accepted, got {} with body {}",
            first.status, first.body
        );
        // The same token carrying the same request is a retry: the same answer, and the
        // key is still registered once.
        let retry = register(
            &emulator,
            plane,
            region,
            &id,
            &replica_key(region, 0),
            Some(token),
        )
        .await;
        assert_eq!(
            retry.status, 200,
            "{plane:?}: a retry must be accepted, got {} with body {}",
            retry.status, retry.body
        );
        assert_eq!(
            registered_keys(&retry, &keys_pointer),
            vec![replica_key(region, 0)],
            "{plane:?}: a retry must report the one registered key, got body {}",
            retry.body
        );

        // The same token carrying ANOTHER key is the modelled idempotency conflict, and
        // stores nothing.
        let conflict = register(
            &emulator,
            plane,
            region,
            &id,
            &replica_key(region, 1),
            Some(token),
        )
        .await;
        assert_eq!(
            conflict.status, 409,
            "{plane:?}: expected HTTP 409, got {} with body {}",
            conflict.status, conflict.body
        );
        assert!(
            conflict.error_type.contains("ConflictException"),
            "{plane:?}: expected a ConflictException, got {:?} with body {}",
            conflict.error_type,
            conflict.body
        );
        assert_eq!(
            conflict.string("/message").as_deref(),
            Some(MESSAGE_IDEMPOTENCY_CONFLICT),
            "{plane:?}: expected the fixed idempotency-conflict sentence, got {:?}",
            conflict.field("/message")
        );
        let stored = read(&emulator, plane, region, &id).await;
        assert_eq!(
            registered_keys(&stored, &keys_pointer),
            vec![replica_key(region, 0)],
            "{plane:?}: the refused registration must store nothing, got body {}",
            stored.body
        );
    }
}

#[tokio::test]
async fn every_projection_reports_the_region_the_resource_arn_names() {
    for region in REGIONS {
        let emulator = Emulator::new();

        for plane in [Plane::Assistant, Plane::KnowledgeBase] {
            let created = create(&emulator, plane, region, "region-of-record", None).await;
            assert_eq!(
                created.status, 200,
                "case {region:?} ({plane:?}): the create must be accepted, got {} with body {}",
                created.status, created.body
            );
            let id = created
                .string(&plane.pointer(plane.id_member()))
                .unwrap_or_else(|| panic!("case {region:?} ({plane:?}): no id"));
            let arn = created
                .string(&plane.pointer(plane.arn_member()))
                .unwrap_or_else(|| panic!("case {region:?} ({plane:?}): no ARN"));
            assert_eq!(
                arn_region(&arn),
                region,
                "case {region:?} ({plane:?}): the ARN must name the signed Region, got {arn}"
            );

            let stored = read(&emulator, plane, region, &id).await;
            for (source, reply) in [("the create's answer", &created), ("the read", &stored)] {
                assert_eq!(
                    reply.string(&plane.pointer("/lastModifiedRegion")).as_deref(),
                    Some(region),
                    "case {region:?} ({plane:?}, {source}): expected the Region the ARN names, \
                     got {:?}",
                    reply.field(&plane.pointer("/lastModifiedRegion"))
                );
                assert!(
                    reply.number(&plane.pointer("/lastModifiedTime")).is_some(),
                    "case {region:?} ({plane:?}, {source}): expected a lastModifiedTime, got \
                     body {}",
                    reply.body
                );
            }
            // The member's own documentation says it is absent for an assistant that is
            // not associated with a Connect instance, and that absent must not be read as
            // DISABLED: nothing here associates one.
            if plane == Plane::Assistant {
                assert!(
                    stored
                        .field(&plane.pointer("/assistantGlobalResiliencyStatus"))
                        .is_none(),
                    "case {region:?}: no assistant here is associated with a Connect instance, \
                     so no replication status may be claimed, got body {}",
                    stored.body
                );
            }

            // The summary a listing reports answers out of the same record.
            let listed = list(&emulator, plane, region).await;
            let summaries = listed
                .field(plane.summaries())
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let summary = summaries
                .iter()
                .find(|summary| {
                    summary
                        .get(plane.id_member().trim_start_matches('/'))
                        .and_then(Value::as_str)
                        == Some(id.as_str())
                })
                .unwrap_or_else(|| {
                    panic!("case {region:?} ({plane:?}): the listing holds no summary of {id}")
                });
            assert_eq!(
                summary.get("lastModifiedRegion").and_then(Value::as_str),
                Some(region),
                "case {region:?} ({plane:?}): the summary must report the Region the ARN names, \
                 got {summary}"
            );
            assert!(
                summary.get("lastModifiedTime").is_some(),
                "case {region:?} ({plane:?}): the summary must report a lastModifiedTime, got \
                 {summary}"
            );
        }

        // The AI agents the service provisions with an assistant are read back through
        // `GetAIAgent`, which reports the same two members out of the same Region.
        let assistant = create(&emulator, Plane::Assistant, region, "region-of-agents", None).await;
        let assistant_id = assistant
            .string("/assistant/assistantId")
            .expect("the create answered no assistantId");
        let bindings = assistant
            .field("/assistant/aiAgentConfiguration")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        assert!(
            !bindings.is_empty(),
            "case {region:?}: an assistant is provisioned with AI agents, got body {}",
            assistant.body
        );
        for (agent_type, binding) in &bindings {
            // A binding names the agent, optionally qualified by the version it pins; the
            // agent itself is read under its own id.
            let ai_agent_id = binding
                .get("aiAgentId")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .split(':')
                .next()
                .unwrap_or_default()
                .to_string();
            let agent = emulator
                .send(
                    "GET",
                    &format!("/assistants/{assistant_id}/aiagents/{ai_agent_id}"),
                    region,
                    None,
                )
                .await;
            assert_eq!(
                agent.status, 200,
                "case {region:?} ({agent_type}): the built-in agent must be readable, got {} \
                 with body {}",
                agent.status, agent.body
            );
            assert_eq!(
                agent.string("/aiAgent/lastModifiedRegion").as_deref(),
                Some(region),
                "case {region:?} ({agent_type}): an AI agent must report the Region its ARN \
                 names, got body {}",
                agent.body
            );
            assert!(
                agent.number("/aiAgent/lastModifiedTime").is_some(),
                "case {region:?} ({agent_type}): an AI agent must report a lastModifiedTime, \
                 got body {}",
                agent.body
            );
        }

        // And a quick response, whose record keeps its own last-written instant: the
        // reported `lastModifiedTime` is never earlier than its `createdTime`. Its parent
        // is a `QUICK_RESPONSES` knowledge base, which is the only type that plane serves.
        let knowledge_base = emulator
            .send(
                "POST",
                "/knowledgeBases",
                region,
                Some(json!({
                    "name": "region-of-quick-responses",
                    "knowledgeBaseType": "QUICK_RESPONSES"
                })),
            )
            .await;
        let knowledge_base_id = knowledge_base
            .string("/knowledgeBase/knowledgeBaseId")
            .expect("the create answered no knowledgeBaseId");
        let quick_response = emulator
            .send(
                "POST",
                &format!("/knowledgeBases/{knowledge_base_id}/quickResponses"),
                region,
                Some(json!({ "name": "regional-reply", "content": { "content": "Hello" } })),
            )
            .await;
        assert_eq!(
            quick_response.status, 200,
            "case {region:?}: the quick-response create must be accepted, got {} with body {}",
            quick_response.status, quick_response.body
        );
        assert_eq!(
            quick_response
                .string("/quickResponse/lastModifiedRegion")
                .as_deref(),
            Some(region),
            "case {region:?}: the quick response must report the Region its ARN names, got {:?}",
            quick_response.field("/quickResponse/lastModifiedRegion")
        );
        let created_time = quick_response
            .number("/quickResponse/createdTime")
            .expect("the create answered no createdTime");
        let modified_time = quick_response
            .number("/quickResponse/lastModifiedTime")
            .expect("the create answered no lastModifiedTime");
        assert!(
            modified_time >= created_time,
            "case {region:?}: lastModifiedTime {modified_time} must not be earlier than \
             createdTime {created_time}"
        );
    }
}

#[tokio::test]
async fn a_registration_moves_the_last_modified_instant_and_a_read_does_not() {
    for plane in [Plane::Assistant, Plane::KnowledgeBase] {
        let emulator = Emulator::new();
        let region = REGIONS[0];
        let created = create(&emulator, plane, region, "last-modified-instant", None).await;
        let id = created
            .string(&plane.pointer(plane.id_member()))
            .unwrap_or_else(|| panic!("{plane:?}: the create answered no id"));
        let instant = |reply: &Reply| {
            reply
                .number(&plane.pointer("/lastModifiedTime"))
                .unwrap_or_else(|| panic!("{plane:?}: no lastModifiedTime in body {}", reply.body))
        };

        let at_create = instant(&created);
        // A read reports the record; it never writes it, so the instant does not move.
        let first_read = read(&emulator, plane, region, &id).await;
        assert_eq!(
            instant(&first_read),
            at_create,
            "{plane:?}: a read must not move the last-modified instant"
        );
        let second_read = read(&emulator, plane, region, &id).await;
        assert_eq!(
            instant(&second_read),
            at_create,
            "{plane:?}: a second read must not move it either"
        );

        // A registration writes the record, so the instant it reports afterwards is not
        // earlier than the one before it — and every later read reports that same one.
        let registered = register(
            &emulator,
            plane,
            region,
            &id,
            &replica_key(region, 0),
            None,
        )
        .await;
        let at_register = instant(&registered);
        assert!(
            at_register >= at_create,
            "{plane:?}: the instant after a write ({at_register}) must not be earlier than the \
             one at the create ({at_create})"
        );
        let after = read(&emulator, plane, region, &id).await;
        assert_eq!(
            instant(&after),
            at_register,
            "{plane:?}: a read after the write must report the instant the write stamped"
        );
    }
}
