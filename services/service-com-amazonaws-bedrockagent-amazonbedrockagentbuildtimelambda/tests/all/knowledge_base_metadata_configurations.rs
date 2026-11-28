// SPDX-License-Identifier: CC0-1.0

//! A knowledge base's custom metadata field configurations, end to end over HTTP.
//!
//! RULE: `metadataConfigurations` is a member OF the knowledge base that
//! `UpdateKnowledgeBase` overwrites like every other mutable member — the list the
//! request carries is stored as it stands and reported back, in that order, by the
//! update's own answer and by every later read of that knowledge base, and an update
//! that carries no list leaves the knowledge base carrying none — while a list outside
//! the constraints the model puts on it is refused at the request boundary and writes
//! nothing.
//!
//! The rows span what the rule generalises over: how many configurations the list holds
//! (one, several, the modelled maximum, none at all), the value each enumerated member
//! takes, the form of the field name, and whether the member is supplied at all. The
//! refusal rows span the constraint classes the model declares for the list and its
//! members — the list's length, each enum's value set, and the name's length and
//! pattern.
//!
//! Every assertion goes through the router, so it exercises the real wire path (route
//! matching, model constraint validation, serialization) rather than calling a handler
//! directly.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_bedrockagent_amazonbedrockagentbuildtimelambda::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 `Authorization` header scoped to the `bedrock` signing name: the routing key
/// this service claims on.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/bedrock/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0123456789abcdef";

/// A role ARN matching the modelled `KnowledgeBaseRoleArn` pattern.
const KB_ROLE: &str =
    "arn:aws:iam::000000000000:role/service-role/AmazonBedrockExecutionRoleForKnowledgeBase";

/// An embedding model ARN naming a model whose published output is an embedding.
const EMBEDDING_MODEL: &str =
    "arn:aws:bedrock:eu-west-1::foundation-model/amazon.titan-embed-text-v2:0";

/// The values the modelled `FieldType` enum admits
/// (`smithy_query(query="shape",shape="FieldType")`): the data type a custom metadata
/// field holds.
const FIELD_TYPES: [&str; 2] = ["STRING", "STRING_LIST"];

/// The values the modelled `SearchableStatus` enum admits
/// (`smithy_query(query="shape",shape="SearchableStatus")`): whether a custom metadata
/// field is searchable.
const SEARCHABLE_STATUSES: [&str; 2] = ["ENABLED", "DISABLED"];

/// The most custom metadata field configurations one knowledge base carries: the
/// modelled `MetadataConfigurations` length bound
/// (`smithy_query(query="shape",shape="MetadataConfigurations")`).
const MAX_METADATA_CONFIGURATIONS: usize = 30;

/// The longest custom metadata field name: the modelled `MetadataKeyName` length bound
/// (`smithy_query(query="shape",shape="MetadataKeyName")`, len 1..200).
const MAX_METADATA_KEY_NAME: usize = 200;

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

    fn string(&self, path: &[&str]) -> String {
        self.member(path)
            .as_str()
            .unwrap_or_else(|| panic!("{path:?} is not a string in {:?}", self.body))
            .to_string()
    }

    fn message(&self, case: &str) -> String {
        self.member(&["message"])
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: the refusal states no message: {:?}", self.body))
            .to_string()
    }
}

/// The emulator under test: one router over one shared engine, exactly as the production
/// router mounts this service.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
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

    /// Create a knowledge base under `name` and answer the id it minted.
    async fn created(&self, name: &str) -> String {
        let created = self
            .call("PUT", "/knowledgebases/", Some(request_body(name, None)))
            .await;
        assert_eq!(
            created.status, 202,
            "case setup: expected CreateKnowledgeBase -> 202, got {created:?}"
        );
        // `CreateKnowledgeBaseRequest` models no `metadataConfigurations` member, so a
        // knowledge base carries none until an update supplies some.
        assert_eq!(
            created.member(&["knowledgeBase", "metadataConfigurations"]),
            &Value::Null,
            "case setup: expected a created knowledge base to carry no metadata field \
             configurations, got {created:?}"
        );
        created.string(&["knowledgeBase", "knowledgeBaseId"])
    }

    async fn update(
        &self,
        knowledge_base_id: &str,
        name: &str,
        metadata_configurations: Option<Value>,
    ) -> Answer {
        self.call(
            "PUT",
            &format!("/knowledgebases/{knowledge_base_id}"),
            Some(request_body(name, metadata_configurations)),
        )
        .await
    }

    async fn get(&self, knowledge_base_id: &str) -> Answer {
        self.call("GET", &format!("/knowledgebases/{knowledge_base_id}"), None)
            .await
    }

    /// The metadata field configurations this store reports for one knowledge base.
    async fn reported_metadata_configurations(&self, case: &str, knowledge_base_id: &str) -> Value {
        let fetched = self.get(knowledge_base_id).await;
        assert_eq!(
            fetched.status, 200,
            "case {case}: expected GetKnowledgeBase -> 200, got {fetched:?}"
        );
        fetched
            .member(&["knowledgeBase", "metadataConfigurations"])
            .clone()
    }
}

/// A `CreateKnowledgeBase`/`UpdateKnowledgeBase` body for a self-managed vector knowledge
/// base, carrying `metadata_configurations` when the row supplies one and leaving the
/// member out when it does not. Every other member is the same across the rows, so the
/// metadata field configurations are the only thing a row changes.
fn request_body(name: &str, metadata_configurations: Option<Value>) -> Value {
    let mut body = json!({
        "name": name,
        "roleArn": KB_ROLE,
        "knowledgeBaseConfiguration": {
            "type": "VECTOR",
            "vectorKnowledgeBaseConfiguration": { "embeddingModelArn": EMBEDDING_MODEL },
        },
        "storageConfiguration": {
            "type": "OPENSEARCH_SERVERLESS",
            "opensearchServerlessConfiguration": {
                "collectionArn":
                    "arn:aws:aoss:eu-west-1:000000000000:collection/metadata-configurations",
                "vectorIndexName": "metadata-configurations-index",
                "fieldMapping": {
                    "vectorField": "bedrock-knowledge-base-default-vector",
                    "textField": "AMAZON_BEDROCK_TEXT_CHUNK",
                    "metadataField": "AMAZON_BEDROCK_METADATA",
                },
            },
        },
    });
    if let Some(metadata_configurations) = metadata_configurations {
        body["metadataConfigurations"] = metadata_configurations;
    }
    body
}

/// One custom metadata field configuration: the three members the modelled
/// `MetadataConfiguration` requires.
fn field(name: &str, field_type: &str, searchability: &str) -> Value {
    json!({ "name": name, "type": field_type, "searchability": searchability })
}

/// Every pairing of the modelled `FieldType` and `SearchableStatus` values, one field
/// each — so no admitted value of either enum goes untested.
fn every_pairing() -> Value {
    let mut fields = Vec::new();
    for field_type in FIELD_TYPES {
        for searchability in SEARCHABLE_STATUSES {
            fields.push(field(
                &format!("{field_type}_{searchability}").to_lowercase(),
                field_type,
                searchability,
            ));
        }
    }
    Value::Array(fields)
}

/// `MAX_METADATA_CONFIGURATIONS` distinct fields: the list at its modelled longest.
fn a_full_list() -> Value {
    Value::Array(
        (0..MAX_METADATA_CONFIGURATIONS)
            .map(|index| {
                field(
                    &format!("field_{index:02}"),
                    FIELD_TYPES[index % FIELD_TYPES.len()],
                    SEARCHABLE_STATUSES[index % SEARCHABLE_STATUSES.len()],
                )
            })
            .collect(),
    )
}

/// The list the update supplies is the list the knowledge base carries, and the one every
/// read of it reports — and an update that supplies none leaves it carrying none.
#[tokio::test]
async fn metadata_configurations_round_trip_through_get_knowledge_base() {
    let emulator = Emulator::new();
    let knowledge_base_id = emulator.created("metadata-round-trip-kb").await;

    // The rows differ along how many configurations the list holds, the value each
    // enumerated member takes, the form of the field name (the modelled `MetadataKeyName`
    // pattern `^[a-zA-Z0-9_][a-zA-Z0-9_-]*$` admits a leading digit or underscore, hyphens
    // and underscores inside, a single character and 200 of them), and whether the member
    // is supplied at all. The last row supplies none, so it proves the member is dropped
    // rather than kept from the row before it.
    let rows: Vec<(&str, Option<Value>)> = vec![
        (
            "one searchable string field",
            Some(json!([field("department", "STRING", "ENABLED")])),
        ),
        (
            "one unsearchable string-list field",
            Some(json!([field("tags", "STRING_LIST", "DISABLED")])),
        ),
        ("every type and searchability pairing", Some(every_pairing())),
        (
            "field names across the forms the pattern admits",
            Some(json!([
                field("_leading_underscore", "STRING", "ENABLED"),
                field("9leading_digit", "STRING_LIST", "DISABLED"),
                field("with-hyphens-and_underscores", "STRING", "DISABLED"),
                field("a", "STRING_LIST", "ENABLED"),
                field(
                    &"n".repeat(MAX_METADATA_KEY_NAME),
                    "STRING",
                    "ENABLED",
                ),
            ])),
        ),
        (
            "the modelled maximum number of fields",
            Some(a_full_list()),
        ),
        ("an empty list", Some(json!([]))),
        ("no list at all", None),
    ];

    for (case, supplied) in rows {
        let updated = emulator
            .update(&knowledge_base_id, "metadata-round-trip-kb", supplied.clone())
            .await;
        assert_eq!(
            updated.status, 202,
            "case {case}: expected UpdateKnowledgeBase -> 202, got {updated:?}"
        );

        // A row that supplied a list is answered that very list, in the order it was
        // supplied; a row that supplied none is answered a knowledge base carrying none.
        let expected = supplied.clone().unwrap_or(Value::Null);
        let answered = updated.member(&["knowledgeBase", "metadataConfigurations"]);
        assert_eq!(
            answered, &expected,
            "case {case}: expected UpdateKnowledgeBase to answer {expected:?}, got {answered:?}"
        );

        // Carrying the list changes nothing about the knowledge base's other members:
        // `createdAt` and `updatedAt` target `DateTimestamp`, which carries
        // `@timestampFormat("date-time")`, so each one stays an RFC-3339 string rather
        // than the epoch number the protocol default would write.
        for member in ["createdAt", "updatedAt"] {
            let value = updated.member(&["knowledgeBase", member]);
            assert!(
                value.as_str().is_some_and(|text| text.contains('T')),
                "case {case}: expected {member} in the modelled date-time wire form, got \
                 {value:?}"
            );
        }

        let reported = emulator
            .reported_metadata_configurations(case, &knowledge_base_id)
            .await;
        assert_eq!(
            reported, expected,
            "case {case}: expected GetKnowledgeBase to report {expected:?}, got {reported:?}"
        );
    }
}

/// A list outside the constraints the model declares for it is a bad request: the refusal
/// is the modelled `ValidationException` at its modelled status, states the member path
/// and the requirement that was broken, withholds a value the model marks `@sensitive`,
/// and leaves the knowledge base carrying exactly the list the last served update stored.
#[tokio::test]
async fn metadata_configurations_outside_their_constraints_are_refused_and_write_nothing() {
    let emulator = Emulator::new();
    let knowledge_base_id = emulator.created("metadata-refusal-kb").await;

    // The list the knowledge base is left carrying, whatever the refusals below ask for.
    let stored = json!([
        field("department", "STRING", "ENABLED"),
        field("authors", "STRING_LIST", "DISABLED"),
    ]);
    let accepted = emulator
        .update(
            &knowledge_base_id,
            "metadata-refusal-kb",
            Some(stored.clone()),
        )
        .await;
    assert_eq!(
        accepted.status, 202,
        "case setup: expected UpdateKnowledgeBase -> 202, got {accepted:?}"
    );

    // One past the list's modelled length bound.
    let too_many = Value::Array(
        (0..=MAX_METADATA_CONFIGURATIONS)
            .map(|index| field(&format!("field_{index:02}"), "STRING", "ENABLED"))
            .collect(),
    );

    /// One refused list, with the clause the refusal must state and the text it must not
    /// carry.
    struct RefusalCase {
        case: &'static str,
        metadata_configurations: Value,
        /// Text the refusal's message must contain: the member path and the requirement.
        clause: String,
        /// Every value the message must also name — an enumerated type's admissible
        /// values, which this front door lists in its own order.
        admits: Vec<&'static str>,
        /// A value the message must NOT echo: what the model marks `@sensitive`.
        withheld: Option<String>,
    }

    let cases: Vec<RefusalCase> = vec![
        RefusalCase {
            case: "one field past the list's length bound",
            metadata_configurations: too_many,
            clause: format!(
                "at 'metadataConfigurations' failed to satisfy constraint: Member must have \
                 length less than or equal to {MAX_METADATA_CONFIGURATIONS}"
            ),
            admits: Vec::new(),
            withheld: None,
        },
        RefusalCase {
            case: "a data type outside the FieldType enum",
            metadata_configurations: json!([field("department", "NUMBER", "ENABLED")]),
            clause: "failed to satisfy constraint: Member must satisfy enum value set:".to_string(),
            admits: FIELD_TYPES.to_vec(),
            withheld: None,
        },
        RefusalCase {
            case: "a searchability outside the SearchableStatus enum",
            metadata_configurations: json!([field("department", "STRING", "SEARCHABLE")]),
            clause: "failed to satisfy constraint: Member must satisfy enum value set:".to_string(),
            admits: SEARCHABLE_STATUSES.to_vec(),
            withheld: None,
        },
        RefusalCase {
            case: "a field name outside the name's pattern",
            metadata_configurations: json!([field("not a field name!", "STRING", "ENABLED")]),
            clause: "failed to satisfy constraint: Member must satisfy regular expression \
                 pattern: [a-zA-Z0-9_][a-zA-Z0-9_-]*"
                .to_string(),
            admits: Vec::new(),
            // The name targets a `@sensitive` shape, so the refusal does not echo it.
            withheld: Some("not a field name!".to_string()),
        },
        RefusalCase {
            case: "a field name past the name's length bound",
            metadata_configurations: json!([field(
                &"n".repeat(MAX_METADATA_KEY_NAME + 1),
                "STRING",
                "ENABLED"
            )]),
            clause: format!(
                "failed to satisfy constraint: Member must have length less than or equal to \
                 {MAX_METADATA_KEY_NAME}"
            ),
            admits: Vec::new(),
            withheld: Some("n".repeat(MAX_METADATA_KEY_NAME + 1)),
        },
        RefusalCase {
            case: "an empty field name",
            metadata_configurations: json!([field("", "STRING", "ENABLED")]),
            clause: "failed to satisfy constraint: Member must have length greater than or equal \
                 to 1"
                .to_string(),
            admits: Vec::new(),
            withheld: None,
        },
    ];

    for row in cases {
        let case = row.case;
        let refused = emulator
            .update(
                &knowledge_base_id,
                "metadata-refusal-kb",
                Some(row.metadata_configurations),
            )
            .await;
        assert_eq!(
            refused.status, 400,
            "case {case}: expected UpdateKnowledgeBase -> 400, got {refused:?}"
        );
        assert_eq!(
            refused.error_type.as_deref(),
            Some("ValidationException"),
            "case {case}: expected the modelled ValidationException, got {refused:?}"
        );
        let message = refused.message(case);
        assert!(
            message.contains(&row.clause),
            "case {case}: expected the refusal to state {:?}, got {message:?}",
            row.clause
        );
        assert!(
            message.contains("metadataConfigurations"),
            "case {case}: expected the refusal to name the member it was about, got {message:?}"
        );
        for admitted in &row.admits {
            assert!(
                message.contains(admitted),
                "case {case}: expected the refusal to list the admitted value {admitted:?}, got \
                 {message:?}"
            );
        }
        if let Some(withheld) = &row.withheld {
            assert!(
                !message.contains(withheld.as_str()),
                "case {case}: expected the refusal to withhold the submitted value, got \
                 {message:?}"
            );
        }

        // The refusal wrote nothing: the knowledge base still carries the list the served
        // update stored.
        let reported = emulator
            .reported_metadata_configurations(case, &knowledge_base_id)
            .await;
        assert_eq!(
            reported, stored,
            "case {case}: expected the refused update to write nothing, got {reported:?}"
        );
    }
}
