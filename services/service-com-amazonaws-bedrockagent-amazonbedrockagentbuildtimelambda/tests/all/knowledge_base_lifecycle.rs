// SPDX-License-Identifier: CC0-1.0

//! The knowledge-base slice, end to end over HTTP: `CreateKnowledgeBase`
//! persists a knowledge base and `GetKnowledgeBase` reports that stored record
//! back.
//!
//! Every assertion here goes through the router, so it exercises the real wire
//! path (route matching, model constraint validation, serialization) rather than
//! calling a handler directly.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_bedrockagent_amazonbedrockagentbuildtimelambda::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 `Authorization` header scoped to the `bedrock` signing name: the
/// routing key this service claims on, and the source of the region its ARNs
/// carry.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/bedrock/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0123456789abcdef";

/// The region of [`AUTHORIZATION`]'s credential scope.
const REGION: &str = "eu-west-1";

/// The account id every emulated ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = "000000000000";

/// A role ARN matching the modelled `KnowledgeBaseRoleArn` pattern
/// `^arn:aws(-[^:]+)?:iam::([0-9]{12})?:role/.+$`.
const KB_ROLE: &str = "arn:aws:iam::000000000000:role/service-role/AmazonBedrockExecutionRoleForKnowledgeBase";

/// An embedding model ARN matching the modelled `BedrockEmbeddingModelArn`
/// pattern (len 20..2048).
const EMBEDDING_MODEL: &str =
    "arn:aws:bedrock:eu-west-1::foundation-model/amazon.titan-embed-text-v2:0";

/// Every member of the modelled `KnowledgeBase` shape that a read must report
/// exactly as the create persisted it. `status` is excluded: it is the one
/// member the CLOCK moves (CREATING -> ACTIVE), and it has its own test.
const PERSISTED_MEMBERS: [&str; 9] = [
    "knowledgeBaseId",
    "name",
    "knowledgeBaseArn",
    "description",
    "roleArn",
    "knowledgeBaseConfiguration",
    "storageConfiguration",
    "createdAt",
    "updatedAt",
];

/// The members `CreateKnowledgeBase` accepts and `KnowledgeBase` reports back,
/// so a request's value for each must survive the round trip verbatim.
const ECHOED_MEMBERS: [&str; 5] = [
    "name",
    "description",
    "roleArn",
    "knowledgeBaseConfiguration",
    "storageConfiguration",
];

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

    /// A member of the response's `knowledgeBase`.
    fn kb(&self, member: &str) -> &Value {
        self.member(&["knowledgeBase", member])
    }

    fn kb_string(&self, member: &str) -> String {
        self.string(&["knowledgeBase", member])
    }
}

/// The emulator under test: one router over one shared engine, exactly as the
/// production router mounts this service.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        // The knowledge bases here bind to an S3 Vectors index, which
        // `CreateKnowledgeBase` resolves in the service that owns it — so the
        // index is created first, as a caller creates it first against AWS.
        crate::vector_store_fixture::seed_index(
            &engine,
            "arn:aws:s3vectors:::bucket/ingestion-demo-vectors",
            "ingestion-demo-index",
            crate::vector_store_fixture::EMBEDDING_DIMENSION,
        );
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
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

    async fn create_knowledge_base(&self, request: Value) -> Answer {
        self.call("PUT", "/knowledgebases/", Some(request)).await
    }

    async fn get_knowledge_base(&self, knowledge_base_id: &str) -> Answer {
        self.call("GET", &format!("/knowledgebases/{knowledge_base_id}"), None)
            .await
    }
}

/// A self-managed vector knowledge base's `knowledgeBaseConfiguration`: the
/// `VECTOR` discriminator plus the embedding model it requires.
fn vector_configuration() -> Value {
    json!({
        "type": "VECTOR",
        "vectorKnowledgeBaseConfiguration": { "embeddingModelArn": EMBEDDING_MODEL },
    })
}

/// An OpenSearch Serverless vector store, with every member its modelled shape
/// requires (`collectionArn`, `vectorIndexName`, `fieldMapping`).
fn opensearch_storage() -> Value {
    json!({
        "type": "OPENSEARCH_SERVERLESS",
        "opensearchServerlessConfiguration": {
            "collectionArn": format!("arn:aws:aoss:{REGION}:{ACCOUNT}:collection/ingestion-demo"),
            "vectorIndexName": "bedrock-knowledge-base-default-index",
            "fieldMapping": {
                "vectorField": "bedrock-knowledge-base-default-vector",
                "textField": "AMAZON_BEDROCK_TEXT_CHUNK",
                "metadataField": "AMAZON_BEDROCK_METADATA",
            },
        },
    })
}

/// An S3 vector store: the storage type the S3-ingestion scenario uses, and a
/// row whose configuration members are all optional rather than required.
fn s3_vectors_storage() -> Value {
    json!({
        "type": "S3_VECTORS",
        "s3VectorsConfiguration": {
            "vectorBucketArn": "arn:aws:s3vectors:::bucket/ingestion-demo-vectors",
            "indexName": "ingestion-demo-index",
        },
    })
}

/// A Pinecone vector store, with every member its modelled shape requires.
fn pinecone_storage() -> Value {
    json!({
        "type": "PINECONE",
        "pineconeConfiguration": {
            "connectionString": "https://ingestion-demo.svc.pinecone.io",
            "credentialsSecretArn":
                format!("arn:aws:secretsmanager:{REGION}:{ACCOUNT}:secret:pinecone-api-key"),
            "namespace": "ingestion-demo",
            "fieldMapping": {
                "textField": "AMAZON_BEDROCK_TEXT_CHUNK",
                "metadataField": "AMAZON_BEDROCK_METADATA",
            },
        },
    })
}

/// Assert `knowledge_base_id` is shaped as the model's `Id` demands
/// (`^[0-9a-zA-Z]{10}$`).
fn assert_well_formed_id(case: &str, knowledge_base_id: &str) {
    assert_eq!(
        knowledge_base_id.len(),
        10,
        "case {case}: expected a 10-character knowledgeBaseId, got {knowledge_base_id:?}"
    );
    assert!(
        knowledge_base_id.chars().all(|c| c.is_ascii_alphanumeric()),
        "case {case}: expected knowledgeBaseId to match ^[0-9a-zA-Z]{{10}}$, \
         got {knowledge_base_id:?}"
    );
}

/// `CreateKnowledgeBase` persists every member the request supplied and
/// `GetKnowledgeBase` reports that record back — over a table of requests that
/// differ in the knowledge-base kind, the vector store kind, which optional
/// members they carry and the form of the name.
#[tokio::test]
async fn created_knowledge_bases_round_trip_through_get_knowledge_base() {
    let emulator = Emulator::new();

    // Rows differ along what the rule generalises over: the
    // `knowledgeBaseConfiguration.type` discriminator (every modelled
    // `KnowledgeBaseType`), the `storageConfiguration.type` discriminator,
    // which optional members are present, and the form of the modelled `Name`.
    let cases: Vec<(&str, Value)> = vec![
        (
            "a VECTOR knowledge base over OpenSearch Serverless",
            json!({
                "name": "opensearch-kb",
                "roleArn": KB_ROLE,
                "knowledgeBaseConfiguration": vector_configuration(),
                "storageConfiguration": opensearch_storage(),
            }),
        ),
        (
            "a VECTOR knowledge base over S3 vectors, with every optional member",
            json!({
                "name": "S3_vectors_kb1",
                "roleArn": "arn:aws:iam::000000000000:role/KnowledgeBaseRole",
                "description": "the S3-backed knowledge base the ingestion scenario retrieves from",
                "knowledgeBaseConfiguration": {
                    "type": "VECTOR",
                    "vectorKnowledgeBaseConfiguration": {
                        "embeddingModelArn": EMBEDDING_MODEL,
                        "embeddingModelConfiguration": {
                            "bedrockEmbeddingModelConfiguration": { "dimensions": 1024 },
                        },
                    },
                },
                "storageConfiguration": s3_vectors_storage(),
                "tags": { "team": "platform" },
            }),
        ),
        (
            "a VECTOR knowledge base over Pinecone",
            json!({
                "name": "pinecone-kb",
                "roleArn": KB_ROLE,
                "knowledgeBaseConfiguration": vector_configuration(),
                "storageConfiguration": pinecone_storage(),
            }),
        ),
        (
            // No storageConfiguration: a managed knowledge base needs no vector
            // store, as this operation's own contract says.
            "a MANAGED knowledge base with a service-managed embedding model",
            json!({
                "name": "managed-kb",
                "roleArn": KB_ROLE,
                "knowledgeBaseConfiguration": {
                    "type": "MANAGED",
                    "managedKnowledgeBaseConfiguration": { "embeddingModelType": "MANAGED" },
                },
            }),
        ),
        (
            "a KENDRA knowledge base",
            json!({
                "name": "kendra_kb",
                "roleArn": KB_ROLE,
                "knowledgeBaseConfiguration": {
                    "type": "KENDRA",
                    "kendraKnowledgeBaseConfiguration": {
                        "kendraIndexArn": format!(
                            "arn:aws:kendra:{REGION}:{ACCOUNT}:index/\
                             12345678-1234-1234-1234-123456789012"
                        ),
                    },
                },
            }),
        ),
        (
            "a SQL knowledge base over Redshift",
            json!({
                "name": "sql-kb-0",
                "roleArn": KB_ROLE,
                "description": "structured data",
                "knowledgeBaseConfiguration": {
                    "type": "SQL",
                    "sqlKnowledgeBaseConfiguration": { "type": "REDSHIFT" },
                },
            }),
        ),
    ];

    for (case, request) in cases {
        let created = emulator.create_knowledge_base(request.clone()).await;
        assert_eq!(
            created.status, 202,
            "case {case}: expected CreateKnowledgeBase -> 202, got {created:?}"
        );

        let knowledge_base_id = created.kb_string("knowledgeBaseId");
        assert_well_formed_id(case, &knowledge_base_id);
        assert_eq!(
            created.kb_string("knowledgeBaseArn"),
            format!("arn:aws:bedrock:{REGION}:{ACCOUNT}:knowledge-base/{knowledge_base_id}"),
            "case {case}: the knowledge base ARN must be built from the request's region, \
             the account and the minted id"
        );
        assert_eq!(
            created.kb_string("status"),
            "CREATING",
            "case {case}: creation is asynchronous (202 Accepted), so a freshly created \
             knowledge base reports the transient CREATING status, got {created:?}"
        );
        // `createdAt`/`updatedAt` target `DateTimestamp`, which carries
        // `@timestampFormat("date-time")`
        // (`smithy_query(query="shape", shape="KnowledgeBase")`), so each one is
        // populated AND carried as an RFC-3339 string rather than an epoch number.
        for member in ["createdAt", "updatedAt"] {
            let at = created.kb(member);
            let text = at.as_str().unwrap_or_else(|| {
                panic!("case {case}: expected {member} to be a date-time string, got {at:?}")
            });
            assert!(
                text.contains('T') && text.ends_with('Z'),
                "case {case}: expected {member} in the modelled date-time wire form, \
                 got {text:?}"
            );
        }
        assert!(
            created.kb("failureReasons").is_null(),
            "case {case}: nothing has failed on a knowledge base just created, got {:?}",
            created.kb("failureReasons")
        );

        // The round trip: the stored record, read back through GetKnowledgeBase.
        let fetched = emulator.get_knowledge_base(&knowledge_base_id).await;
        assert_eq!(
            fetched.status, 200,
            "case {case}: expected GetKnowledgeBase -> 200, got {fetched:?}"
        );
        for member in PERSISTED_MEMBERS {
            assert_eq!(
                fetched.kb(member),
                created.kb(member),
                "case {case}: GetKnowledgeBase must report {member} exactly as \
                 CreateKnowledgeBase persisted it, got {:?}",
                fetched.kb(member)
            );
        }
        assert!(
            ["CREATING", "ACTIVE"].contains(&fetched.kb_string("status").as_str()),
            "case {case}: a read reports either the transient CREATING status or the ACTIVE \
             one the clock has made due, got {:?}",
            fetched.kb("status")
        );

        // Every member the request supplied survives the round trip verbatim.
        for member in ECHOED_MEMBERS {
            if let Some(supplied) = request.get(member) {
                assert_eq!(
                    fetched.kb(member),
                    supplied,
                    "case {case}: expected {member} to round-trip as {supplied:?}, got {:?}",
                    fetched.kb(member)
                );
            } else {
                assert!(
                    fetched.kb(member).is_null(),
                    "case {case}: {member} was not supplied, so nothing may be invented for it, \
                     got {:?}",
                    fetched.kb(member)
                );
            }
        }
    }
}

/// A knowledge base leaves `CREATING` for `ACTIVE` because the CLOCK made the
/// transition due, not because somebody read it: the status is still `CREATING`
/// straight after the create, `ACTIVE` once the activation span has elapsed, and
/// every other member is untouched by either read.
#[tokio::test]
async fn a_created_knowledge_base_becomes_active_once_the_clock_makes_it_due() {
    let emulator = Emulator::new();

    let created = emulator
        .create_knowledge_base(json!({
            "name": "activation-kb",
            "roleArn": KB_ROLE,
            "knowledgeBaseConfiguration": vector_configuration(),
            "storageConfiguration": opensearch_storage(),
        }))
        .await;
    assert_eq!(
        created.status, 202,
        "case setup: expected CreateKnowledgeBase -> 202, got {created:?}"
    );
    let knowledge_base_id = created.kb_string("knowledgeBaseId");

    // Straight after the create the activation span has not elapsed, so the
    // caller still observes the transient status.
    let immediately = emulator.get_knowledge_base(&knowledge_base_id).await;
    assert_eq!(
        immediately.kb_string("status"),
        "CREATING",
        "case immediately after create: the activation span is never zero, so a caller acting \
         straight after the call still sees CREATING, got {immediately:?}"
    );

    // Past the span the transition is owed whether or not anybody called.
    tokio::time::sleep(Duration::from_millis(900)).await;
    let after = emulator.get_knowledge_base(&knowledge_base_id).await;
    assert_eq!(
        after.kb_string("status"),
        "ACTIVE",
        "case after the activation span: the clock owes the knowledge base ACTIVE, \
         got {after:?}"
    );

    // A read changes nothing else, and the status does not move again.
    let again = emulator.get_knowledge_base(&knowledge_base_id).await;
    assert_eq!(
        again.body, after.body,
        "case repeated read: a read must not change what a later caller sees"
    );
    for member in PERSISTED_MEMBERS {
        assert_eq!(
            after.kb(member),
            created.kb(member),
            "case after the activation span: only the status moves, but {member} changed to {:?}",
            after.kb(member)
        );
    }
}

/// `knowledgeBaseConfiguration.type` is a discriminator: the sibling member it
/// names must be supplied, and a `VECTOR` (self-managed) knowledge base must
/// also name the vector store in `storageConfiguration`. A request that does
/// not satisfy its own discriminator is refused with `ValidationException`
/// (400) — not accepted, and not reported as a missing resource.
#[tokio::test]
async fn create_knowledge_base_refuses_a_configuration_its_type_does_not_carry() {
    let emulator = Emulator::new();

    // One row per modelled `KnowledgeBaseType` with its own member missing,
    // plus the VECTOR row that omits the vector store, plus a type the model
    // does not declare at all.
    let cases: Vec<(&str, Value)> = vec![
        (
            "VECTOR without vectorKnowledgeBaseConfiguration",
            json!({
                "name": "vector-no-config",
                "roleArn": KB_ROLE,
                "knowledgeBaseConfiguration": { "type": "VECTOR" },
                "storageConfiguration": opensearch_storage(),
            }),
        ),
        (
            "VECTOR carrying another kind's configuration member",
            json!({
                "name": "vector-wrong-config",
                "roleArn": KB_ROLE,
                "knowledgeBaseConfiguration": {
                    "type": "VECTOR",
                    "managedKnowledgeBaseConfiguration": { "embeddingModelType": "MANAGED" },
                },
                "storageConfiguration": opensearch_storage(),
            }),
        ),
        (
            "VECTOR without a vector store",
            json!({
                "name": "vector-no-storage",
                "roleArn": KB_ROLE,
                "knowledgeBaseConfiguration": vector_configuration(),
            }),
        ),
        (
            "KENDRA without kendraKnowledgeBaseConfiguration",
            json!({
                "name": "kendra-no-config",
                "roleArn": KB_ROLE,
                "knowledgeBaseConfiguration": { "type": "KENDRA" },
            }),
        ),
        (
            "SQL without sqlKnowledgeBaseConfiguration",
            json!({
                "name": "sql-no-config",
                "roleArn": KB_ROLE,
                "knowledgeBaseConfiguration": { "type": "SQL" },
            }),
        ),
        (
            "MANAGED without managedKnowledgeBaseConfiguration",
            json!({
                "name": "managed-no-config",
                "roleArn": KB_ROLE,
                "knowledgeBaseConfiguration": { "type": "MANAGED" },
            }),
        ),
        (
            "a type the model does not declare",
            json!({
                "name": "unknown-type-kb",
                "roleArn": KB_ROLE,
                "knowledgeBaseConfiguration": {
                    "type": "GRAPH",
                    "vectorKnowledgeBaseConfiguration": { "embeddingModelArn": EMBEDDING_MODEL },
                },
                "storageConfiguration": opensearch_storage(),
            }),
        ),
    ];

    for (case, request) in cases {
        let refused = emulator.create_knowledge_base(request).await;
        assert_eq!(
            refused.status, 400,
            "case {case}: expected CreateKnowledgeBase -> 400, got {refused:?}"
        );
        assert_eq!(
            refused.error_type.as_deref(),
            Some("ValidationException"),
            "case {case}: expected ValidationException, got {refused:?}"
        );
    }

    // A refused create persisted nothing: a well-formed request still works.
    let created = emulator
        .create_knowledge_base(json!({
            "name": "after-refusals-kb",
            "roleArn": KB_ROLE,
            "knowledgeBaseConfiguration": vector_configuration(),
            "storageConfiguration": opensearch_storage(),
        }))
        .await;
    assert_eq!(
        created.status, 202,
        "case after the refusals: a well-formed create is still served, got {created:?}"
    );
}

/// `GetKnowledgeBase` answers `ResourceNotFoundException` (404) for a
/// well-formed `knowledgeBaseId` the store does not hold.
#[tokio::test]
async fn get_knowledge_base_refuses_an_id_no_knowledge_base_holds() {
    let emulator = Emulator::new();

    let created = emulator
        .create_knowledge_base(json!({
            "name": "the-one-kb",
            "roleArn": KB_ROLE,
            "knowledgeBaseConfiguration": vector_configuration(),
            "storageConfiguration": opensearch_storage(),
        }))
        .await;
    assert_eq!(
        created.status, 202,
        "case setup: expected CreateKnowledgeBase -> 202, got {created:?}"
    );
    let stored_id = created.kb_string("knowledgeBaseId");

    // Rows: ids the store does not hold — all digits, mixed case, and one
    // differing from the stored id in its last character only.
    let mut unknown_ids = vec!["0123456789".to_string(), "aZ09aZ09aZ".to_string()];
    let mut nearly = stored_id.clone();
    nearly.pop();
    nearly.push(if stored_id.ends_with('X') { 'Y' } else { 'X' });
    if nearly != stored_id {
        unknown_ids.push(nearly);
    }

    for unknown in unknown_ids {
        let fetched = emulator.get_knowledge_base(&unknown).await;
        assert_eq!(
            fetched.status, 404,
            "case {unknown}: expected GetKnowledgeBase -> 404, got {fetched:?}"
        );
        assert_eq!(
            fetched.error_type.as_deref(),
            Some("ResourceNotFoundException"),
            "case {unknown}: expected ResourceNotFoundException, got {fetched:?}"
        );
    }

    // The knowledge base that does exist is still readable.
    let fetched = emulator.get_knowledge_base(&stored_id).await;
    assert_eq!(
        fetched.status, 200,
        "case stored knowledge base: expected GetKnowledgeBase -> 200, got {fetched:?}"
    );
}

/// A repeated `@idempotencyToken` returns the knowledge base the first call
/// created, and a create under a name this account's knowledge bases already
/// hold is refused — the name is the account's to hold once
/// (`knowledge_base_name_conflict`).
#[tokio::test]
async fn create_knowledge_base_honours_its_idempotency_token_and_mints_one_per_call() {
    let emulator = Emulator::new();

    // Two tokens (each at least the modelled 33 characters), each replayed: a
    // replay returns the knowledge base the first call created. Each row carries
    // its own name, since a second knowledge base under a name the account holds
    // is refused.
    for (position, token) in [
        "idempotency-token-for-the-first-kb",
        "IdempotencyTokenForTheSecondKb00000",
    ]
    .into_iter()
    .enumerate()
    {
        let request = json!({
            "name": format!("idempotent-kb-{position}"),
            "roleArn": KB_ROLE,
            "knowledgeBaseConfiguration": vector_configuration(),
            "storageConfiguration": opensearch_storage(),
            "clientToken": token,
        });
        let once = emulator.create_knowledge_base(request.clone()).await;
        let again = emulator.create_knowledge_base(request).await;
        assert_eq!(
            once.status, 202,
            "case token {token}: expected 202 on the first call, got {once:?}"
        );
        assert_eq!(
            again.status, 202,
            "case token {token}: expected 202 on the replay, got {again:?}"
        );
        assert_eq!(
            again.kb_string("knowledgeBaseId"),
            once.kb_string("knowledgeBaseId"),
            "case token {token}: a replayed idempotency token must return the same knowledge base"
        );
        for member in PERSISTED_MEMBERS {
            assert_eq!(
                again.kb(member),
                once.kb(member),
                "case token {token}: a replay reports the stored knowledge base, unchanged in \
                 {member}, got {:?}",
                again.kb(member)
            );
        }
    }

    // Same name, no token: the second create is refused, and the knowledge base
    // holding the name is untouched. The whole rule — which names collide, and
    // when a name is free again — is pinned by `knowledge_base_name_conflict`.
    let first = emulator
        .create_knowledge_base(json!({
            "name": "shared-name-kb",
            "roleArn": KB_ROLE,
            "knowledgeBaseConfiguration": vector_configuration(),
            "storageConfiguration": opensearch_storage(),
        }))
        .await;
    let second = emulator
        .create_knowledge_base(json!({
            "name": "shared-name-kb",
            "roleArn": KB_ROLE,
            "knowledgeBaseConfiguration": vector_configuration(),
            "storageConfiguration": opensearch_storage(),
        }))
        .await;
    assert_eq!(
        second.status, 409,
        "case duplicate name: a knowledge base's name is this account's to hold once, \
         got {second:?}"
    );
    let id = first.kb_string("knowledgeBaseId");
    let fetched = emulator.get_knowledge_base(&id).await;
    assert_eq!(
        fetched.kb_string("knowledgeBaseArn"),
        first.kb_string("knowledgeBaseArn"),
        "case duplicate name: the refused create leaves the knowledge base holding the name \
         exactly as it was"
    );
}
