// SPDX-License-Identifier: CC0-1.0

//! `AWS::Bedrock::KnowledgeBase` end to end: the knowledge base a stack declares exists in
//! Amazon Bedrock Agent itself, under the identifier the stack publishes.
//!
//! RULE: for a resource type whose owning service MINTS the resource's identifier, the stack
//! creates the resource through that service and publishes the identifier THAT service
//! answered as the resource's physical id — so the value a client reads out of `Ref`, of the
//! type's own identifier attribute, of `DescribeStackResource(s)`, of `ListStackResources`,
//! of a stack output or of a parameter the stack wrote it to is one the owning service
//! accepts. A knowledge base is such a resource: `CreateKnowledgeBase` takes its NAME and
//! answers the `knowledgeBaseId` it minted, modelled `^[0-9a-zA-Z]{10}$`, which no name
//! CloudFormation decides could satisfy. The other side of the rule is a type whose
//! identifier IS the name CloudFormation decides (an `AWS::SSM::Parameter`), whose physical
//! id stays that name.
//!
//! The rows differ along what the rule generalises over: a knowledge base the template NAMES
//! and one it leaves CloudFormation to name, one knowledge base in a stack and two, two
//! embedding models, and two kinds of vector store its storage configuration binds it to —
//! an Amazon OpenSearch Serverless collection and a Pinecone index. The S3-Vectors-backed
//! knowledge base, whose binding the owning service resolves in a SECOND service, is
//! `core/router/tests/all/cloudformation_provisions_a_bedrock_knowledge_base.rs`.
//!
//! Everything goes over the wire each service speaks through the one router
//! `stack_lifecycle.rs` builds — the CloudFormation calls over its awsQuery wire, the
//! knowledge-base reads over Amazon Bedrock Agent's own `restJson1` wire, the parameter read
//! over Systems Manager's awsJson1_1 one — so what is asserted is what a client that deploys
//! the template and then calls those services observes.

use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::stack_lifecycle::{
    call, deploy, describe_stack, member, router, wait_for_the_transition,
    xml_blocks, xml_text, ACCOUNT_ID, REGION,
};
use crate::update_rollback_recovery::parameter_value;

/// An embedding model the published catalog carries: Titan Text Embeddings G1 and V2.
/// Source: the catalog the owning service resolves an `embeddingModelArn` against
/// (`state::vector_store::EMBEDDING_MODEL_DIMENSIONS` of the bedrock-agent crate).
pub(crate) const TITAN_G1: &str = "amazon.titan-embed-text-v1";
pub(crate) const TITAN_V2: &str = "amazon.titan-embed-text-v2:0";

/// How many characters a minted `knowledgeBaseId` carries, and which they may be: the
/// modelled `Id` shape of `GetKnowledgeBase`'s `knowledgeBaseId`
/// (`@pattern ^[0-9a-zA-Z]{10}$`).
const KNOWLEDGE_BASE_ID_CHARACTERS: usize = 10;

/// The status a knowledge base ready to be queried reports
/// (`KnowledgeBase$status`: "ACTIVE – The knowledge base is ready to be queried").
const ACTIVE: &str = "ACTIVE";

/// The ARN of a foundation model, as an `embeddingModelArn` names one.
pub(crate) fn model_arn(model: &str) -> String {
    format!("arn:aws:bedrock:{REGION}::foundation-model/{model}")
}

/// The ARN of the knowledge base `id` names, as `KnowledgeBaseArn`'s own pattern spells it
/// (`arn:aws:bedrock:<region>:<account>:knowledge-base/<id>`).
fn knowledge_base_arn(id: &str) -> String {
    format!("arn:aws:bedrock:{REGION}:{ACCOUNT_ID}:knowledge-base/{id}")
}

/// Whether `value` is an identifier of the form the owning service mints: exactly
/// [`KNOWLEDGE_BASE_ID_CHARACTERS`] alphanumeric characters, which is the whole of the
/// modelled pattern.
pub(crate) fn is_knowledge_base_id(value: &str) -> bool {
    value.len() == KNOWLEDGE_BASE_ID_CHARACTERS
        && value.chars().all(|c| c.is_ascii_alphanumeric())
}

/// One Amazon Bedrock Agent call over the `restJson1` wire its model declares, on the method
/// and URI that operation's own `@http` binding names, signed for `bedrock` — the signing
/// name every Bedrock-family model declares — in the Region the stack's resources live in.
async fn bedrock_agent(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    let payload = body.map(ToString::to_string).unwrap_or_default();
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://bedrock.{REGION}.amazonaws.com{uri}"))
        .header("host", format!("bedrock.{REGION}.amazonaws.com"))
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/bedrock/\
                 aws4_request, SignedHeaders=host, Signature=0123456789abcdef"
            ),
        )
        .body(bytes::Bytes::from(payload))
        .expect("a well-formed Amazon Bedrock Agent request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let raw = response.into_body();
    let parsed = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// `GetKnowledgeBase` for one knowledge base: the HTTP status and the `knowledgeBase` the
/// answer carries.
pub(crate) async fn get_knowledge_base(
    router: &MultiServiceRouter,
    knowledge_base_id: &str,
) -> (u16, Value) {
    let (status, document) = bedrock_agent(
        router,
        "GET",
        &format!("/knowledgebases/{knowledge_base_id}"),
        None,
    )
    .await;
    let knowledge_base = document
        .get("knowledgeBase")
        .cloned()
        .unwrap_or(document.clone());
    (status, knowledge_base)
}

/// The `PhysicalResourceId` `DescribeStackResource` reports for one logical id.
pub(crate) async fn described_physical_id(
    router: &MultiServiceRouter,
    case: &str,
    stack: &str,
    logical_id: &str,
) -> String {
    let (status, body) = call(
        router,
        "DescribeStackResource",
        &[
            member("StackName", stack),
            member("LogicalResourceId", logical_id),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeStackResource({stack}, {logical_id}) answered {status}: {body}"
    );
    xml_text(&body, "PhysicalResourceId").unwrap_or_else(|| {
        panic!("case {case:?}: expected a physical resource id for {logical_id}, got {body}")
    })
}

/// The `PhysicalResourceId` `ListStackResources` reports for one logical id.
async fn listed_physical_id(
    router: &MultiServiceRouter,
    case: &str,
    stack: &str,
    logical_id: &str,
) -> Option<String> {
    let (status, body) = call(router, "ListStackResources", &[member("StackName", stack)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListStackResources({stack}) answered {status}: {body}"
    );
    xml_blocks(&body, "member").into_iter().find_map(|entry| {
        (xml_text(&entry, "LogicalResourceId").as_deref() == Some(logical_id))
            .then(|| xml_text(&entry, "PhysicalResourceId"))
            .flatten()
    })
}

/// The value the stack reports for one declared output.
pub(crate) async fn output_value(
    router: &MultiServiceRouter,
    case: &str,
    stack: &str,
    key: &str,
) -> Option<String> {
    let (status, body) = describe_stack(router, stack).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeStacks({stack}) answered {status}: {body}"
    );
    let outputs = xml_blocks(&body, "Outputs").into_iter().next()?;
    xml_blocks(&outputs, "member")
        .into_iter()
        .find_map(|entry| {
            (xml_text(&entry, "OutputKey").as_deref() == Some(key))
                .then(|| xml_text(&entry, "OutputValue"))
                .flatten()
        })
}

/// The `StorageConfiguration` of a knowledge base whose vectors live in an Amazon OpenSearch
/// Serverless collection: the type's own `OpensearchServerlessConfiguration`, whose three
/// members `CreateKnowledgeBase` requires (`collectionArn`, `vectorIndexName` and the field
/// mapping) the declaration states.
pub(crate) fn opensearch_storage(collection: &str, index: &str) -> Value {
    json!({
        "Type": "OPENSEARCH_SERVERLESS",
        "OpensearchServerlessConfiguration": {
            "CollectionArn": format!("arn:aws:aoss:{REGION}:{ACCOUNT_ID}:collection/{collection}"),
            "VectorIndexName": index,
            "FieldMapping": {
                "VectorField": "bedrock-knowledge-base-default-vector",
                "TextField": "AMAZON_BEDROCK_TEXT_CHUNK",
                "MetadataField": "AMAZON_BEDROCK_METADATA"
            }
        }
    })
}

/// The `StorageConfiguration` of a knowledge base whose vectors live in a Pinecone index:
/// the type's own `PineconeConfiguration`, whose three required members
/// (`connectionString`, `credentialsSecretArn` and the field mapping) the declaration
/// states.
fn pinecone_storage(index: &str) -> Value {
    json!({
        "Type": "PINECONE",
        "PineconeConfiguration": {
            "ConnectionString": format!("https://{index}.svc.pinecone.io"),
            "CredentialsSecretArn":
                format!("arn:aws:secretsmanager:{REGION}:{ACCOUNT_ID}:secret:pinecone-api-key"),
            "Namespace": index,
            "FieldMapping": {
                "TextField": "AMAZON_BEDROCK_TEXT_CHUNK",
                "MetadataField": "AMAZON_BEDROCK_METADATA"
            }
        }
    })
}

/// One `AWS::Bedrock::KnowledgeBase` declaration, as a row writes it.
pub(crate) fn knowledge_base_resource(
    name: Option<&str>,
    description: Option<&str>,
    role: &str,
    model: &str,
    storage: Value,
) -> Value {
    let mut properties = json!({
        "RoleArn": format!("arn:aws:iam::{ACCOUNT_ID}:role/{role}"),
        "KnowledgeBaseConfiguration": {
            "Type": "VECTOR",
            "VectorKnowledgeBaseConfiguration": { "EmbeddingModelArn": model_arn(model) }
        },
        "StorageConfiguration": storage,
        "Tags": { "estate": "kbcfn" }
    });
    if let Some(name) = name {
        properties["Name"] = json!(name);
    }
    if let Some(description) = description {
        properties["Description"] = json!(description);
    }
    json!({ "Type": "AWS::Bedrock::KnowledgeBase", "Properties": properties })
}

/// The outputs every row publishes per knowledge base: what `Ref` resolves to, and the two
/// identity attributes the type publishes.
pub(crate) fn published_outputs(logical_ids: &[&str]) -> Value {
    let mut outputs = serde_json::Map::new();
    for logical_id in logical_ids {
        outputs.insert(
            format!("{logical_id}Ref"),
            json!({ "Value": { "Ref": logical_id } }),
        );
        outputs.insert(
            format!("{logical_id}Id"),
            json!({ "Value": { "Fn::GetAtt": [logical_id, "KnowledgeBaseId"] } }),
        );
        outputs.insert(
            format!("{logical_id}Arn"),
            json!({ "Value": { "Fn::GetAtt": [logical_id, "KnowledgeBaseArn"] } }),
        );
    }
    Value::Object(outputs)
}

/// One knowledge base a row declares, and what the rule says the stack must publish for it.
struct DeclaredKnowledgeBase {
    logical_id: &'static str,
    /// The `Name` the declaration states, or `None` where CloudFormation generates it.
    declared_name: Option<&'static str>,
    role: &'static str,
    model: &'static str,
}

/// One row: a template, and the knowledge bases it declares.
struct KnowledgeBaseCase {
    case: &'static str,
    stack: &'static str,
    template: String,
    knowledge_bases: Vec<DeclaredKnowledgeBase>,
}

fn knowledge_base_cases() -> Vec<KnowledgeBaseCase> {
    vec![
        KnowledgeBaseCase {
            case: "a knowledge base the template names, in an OpenSearch Serverless store",
            stack: "kbcfn-named",
            template: json!({
                "Resources": {
                    "Escalation": knowledge_base_resource(
                        Some("kbcfn-escalation-g1"),
                        Some("the escalation knowledge base"),
                        "kbcfn-escalation",
                        TITAN_G1,
                        opensearch_storage("kbcfn-escalation", "escalation-index"),
                    )
                },
                "Outputs": published_outputs(&["Escalation"])
            })
            .to_string(),
            knowledge_bases: vec![DeclaredKnowledgeBase {
                logical_id: "Escalation",
                declared_name: Some("kbcfn-escalation-g1"),
                role: "kbcfn-escalation",
                model: TITAN_G1,
            }],
        },
        KnowledgeBaseCase {
            case: "a knowledge base the template does not name",
            stack: "kbcfn-unnamed",
            template: json!({
                "Resources": {
                    "Escalation": knowledge_base_resource(
                        None,
                        None,
                        "kbcfn-unnamed",
                        TITAN_V2,
                        opensearch_storage("kbcfn-unnamed", "unnamed-index"),
                    )
                },
                "Outputs": published_outputs(&["Escalation"])
            })
            .to_string(),
            knowledge_bases: vec![DeclaredKnowledgeBase {
                logical_id: "Escalation",
                declared_name: None,
                role: "kbcfn-unnamed",
                model: TITAN_V2,
            }],
        },
        KnowledgeBaseCase {
            case: "two knowledge bases in one stack, reading different embedding models",
            stack: "kbcfn-pair",
            template: json!({
                "Resources": {
                    "Escalation": knowledge_base_resource(
                        Some("kbcfn-pair-escalation"),
                        None,
                        "kbcfn-pair",
                        TITAN_G1,
                        opensearch_storage("kbcfn-pair-one", "escalation-index"),
                    ),
                    "Runbooks": knowledge_base_resource(
                        Some("kbcfn-pair-runbooks"),
                        Some("the runbook knowledge base"),
                        "kbcfn-pair",
                        TITAN_V2,
                        opensearch_storage("kbcfn-pair-two", "runbook-index"),
                    )
                },
                "Outputs": published_outputs(&["Escalation", "Runbooks"])
            })
            .to_string(),
            knowledge_bases: vec![
                DeclaredKnowledgeBase {
                    logical_id: "Escalation",
                    declared_name: Some("kbcfn-pair-escalation"),
                    role: "kbcfn-pair",
                    model: TITAN_G1,
                },
                DeclaredKnowledgeBase {
                    logical_id: "Runbooks",
                    declared_name: Some("kbcfn-pair-runbooks"),
                    role: "kbcfn-pair",
                    model: TITAN_V2,
                },
            ],
        },
        KnowledgeBaseCase {
            case: "a knowledge base whose vectors live in a Pinecone index",
            stack: "kbcfn-pinecone",
            template: json!({
                "Resources": {
                    "Escalation": knowledge_base_resource(
                        Some("kbcfn-pinecone-escalation"),
                        None,
                        "kbcfn-pinecone",
                        TITAN_G1,
                        pinecone_storage("kbcfn-embeddings"),
                    )
                },
                "Outputs": published_outputs(&["Escalation"])
            })
            .to_string(),
            knowledge_bases: vec![DeclaredKnowledgeBase {
                logical_id: "Escalation",
                declared_name: Some("kbcfn-pinecone-escalation"),
                role: "kbcfn-pinecone",
                model: TITAN_G1,
            }],
        },
    ]
}

/// Every row: the identifier the stack publishes for a declared knowledge base is the one
/// Amazon Bedrock Agent minted, and that service answers for it.
#[tokio::test]
async fn a_cloudformation_knowledge_base_is_created_under_the_id_the_stack_publishes() {
    let router = router();
    for case in knowledge_base_cases() {
        let name = case.case;
        deploy(&router, case.stack, &case.template).await;

        let mut published: Vec<String> = Vec::new();
        for declared in &case.knowledge_bases {
            let logical_id = declared.logical_id;
            let id = described_physical_id(&router, name, case.stack, logical_id).await;
            assert!(
                is_knowledge_base_id(&id),
                "case {name:?}: expected {logical_id} to report an id of the modelled \
                 ^[0-9a-zA-Z]{{10}}$ form, got {id:?}"
            );
            published.push(id.clone());

            // Every reader of the stack reports the SAME identifier...
            let listed = listed_physical_id(&router, name, case.stack, logical_id).await;
            assert_eq!(
                listed.as_deref(),
                Some(id.as_str()),
                "case {name:?}: expected ListStackResources to report {id:?} for \
                 {logical_id}, got {listed:?}"
            );
            for (output, expected) in [
                (format!("{logical_id}Ref"), id.clone()),
                (format!("{logical_id}Id"), id.clone()),
                (format!("{logical_id}Arn"), knowledge_base_arn(&id)),
            ] {
                let reported = output_value(&router, name, case.stack, &output).await;
                assert_eq!(
                    reported.as_deref(),
                    Some(expected.as_str()),
                    "case {name:?}: expected the output {output:?} to report {expected:?}, \
                     got {reported:?}"
                );
            }

            // ...and the owning service answers for it, with the knowledge base the
            // declaration described.
            let (status, knowledge_base) = get_knowledge_base(&router, &id).await;
            assert_eq!(
                status, 200,
                "case {name:?}: expected GetKnowledgeBase({id}) to answer 200, got {status} \
                 with {knowledge_base}"
            );
            for (member_name, expected) in [
                ("knowledgeBaseId", id.clone()),
                ("knowledgeBaseArn", knowledge_base_arn(&id)),
                ("status", ACTIVE.to_string()),
                (
                    "roleArn",
                    format!("arn:aws:iam::{ACCOUNT_ID}:role/{}", declared.role),
                ),
            ] {
                let reported = knowledge_base.get(member_name).and_then(Value::as_str);
                assert_eq!(
                    reported,
                    Some(expected.as_str()),
                    "case {name:?}: expected the knowledge base's {member_name:?} to be \
                     {expected:?}, got {reported:?} in {knowledge_base}"
                );
            }
            let model = knowledge_base
                .pointer("/knowledgeBaseConfiguration/vectorKnowledgeBaseConfiguration/embeddingModelArn")
                .and_then(Value::as_str);
            assert_eq!(
                model,
                Some(model_arn(declared.model).as_str()),
                "case {name:?}: expected the knowledge base to read the declared embedding \
                 model, got {model:?} in {knowledge_base}"
            );

            // The NAME is CloudFormation's — declared or generated — and is never the
            // identifier, which is the whole of the defect this rule closes.
            let reported_name = knowledge_base
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            match declared.declared_name {
                Some(declared_name) => assert_eq!(
                    reported_name, declared_name,
                    "case {name:?}: expected the knowledge base to carry the declared name \
                     {declared_name:?}, got {reported_name:?}"
                ),
                None => assert!(
                    reported_name.starts_with(case.stack)
                        && reported_name.len() > case.stack.len() + logical_id.len(),
                    "case {name:?}: expected a generated <stack>-<logical id>-<unique id> \
                     name, got {reported_name:?}"
                ),
            }
            assert_ne!(
                reported_name, id,
                "case {name:?}: expected the published id to be the minted identifier rather \
                 than the knowledge base's name, got {id:?}"
            );
        }

        // Two knowledge bases of one stack are two knowledge bases: each carries an
        // identifier of its own.
        let mut unique = published.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(
            unique.len(),
            published.len(),
            "case {name:?}: expected one minted identifier per knowledge base, got {published:?}"
        );
    }
}

/// The published identifier travels through everything that carries it: the stack's own
/// readers, an output, and the Systems Manager parameter the template writes it to — and the
/// other side of the rule, the parameter itself, keeps the NAME CloudFormation decided as its
/// physical id.
#[tokio::test]
async fn the_published_id_reaches_a_parameter_a_client_reads_it_out_of() {
    let case = "a knowledge base id published through a parameter";
    let router = router();
    let stack = "kbcfn-published";
    let parameter = "/kbcfn/published/knowledge-base-id";
    let template = json!({
        "Resources": {
            "Escalation": knowledge_base_resource(
                Some("kbcfn-published-escalation"),
                None,
                "kbcfn-published",
                TITAN_G1,
                opensearch_storage("kbcfn-published", "published-index"),
            ),
            "Handle": {
                "Type": "AWS::SSM::Parameter",
                "Properties": {
                    "Name": parameter,
                    "Type": "String",
                    "Value": { "Ref": "Escalation" }
                }
            }
        },
        "Outputs": published_outputs(&["Escalation"])
    })
    .to_string();
    deploy(&router, stack, &template).await;

    let id = described_physical_id(&router, case, stack, "Escalation").await;
    assert!(
        is_knowledge_base_id(&id),
        "case {case:?}: expected a minted identifier, got {id:?}"
    );
    let held = parameter_value(&router, parameter).await;
    assert_eq!(
        held.as_deref(),
        Some(id.as_str()),
        "case {case:?}: expected the parameter to hold the published id {id:?}, got {held:?}"
    );
    let (status, knowledge_base) = get_knowledge_base(&router, &held.unwrap_or_default()).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the id read out of the parameter to be one \
         GetKnowledgeBase accepts, got {status} with {knowledge_base}"
    );

    // The other side of the rule: Systems Manager identifies a parameter BY the name
    // CloudFormation decides, so that resource's physical id is that name.
    let parameter_id = described_physical_id(&router, case, stack, "Handle").await;
    assert_eq!(
        parameter_id, parameter,
        "case {case:?}: expected the parameter to keep the declared name as its physical id, \
         got {parameter_id:?}"
    );
}

/// A changed declaration is applied to the knowledge base the stack created — it keeps the
/// identifier that service minted — and deleting the stack deletes the knowledge base from
/// the service that holds it.
#[tokio::test]
async fn an_update_keeps_the_minted_id_and_a_delete_removes_the_knowledge_base() {
    let case = "an updated and then deleted knowledge base";
    let router = router();
    let stack = "kbcfn-updated";
    let template = |description: &str| {
        json!({
            "Resources": {
                "Escalation": knowledge_base_resource(
                    Some("kbcfn-updated-escalation"),
                    Some(description),
                    "kbcfn-updated",
                    TITAN_G1,
                    opensearch_storage("kbcfn-updated", "updated-index"),
                )
            },
            "Outputs": published_outputs(&["Escalation"])
        })
        .to_string()
    };
    deploy(&router, stack, &template("the first revision")).await;
    let created = described_physical_id(&router, case, stack, "Escalation").await;

    let (status, body) = call(
        &router,
        "UpdateStack",
        &[
            member("StackName", stack),
            member("TemplateBody", &template("the second revision")),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: UpdateStack({stack}) answered {status}: {body}"
    );
    wait_for_the_transition().await;
    let (status, body) = describe_stack(&router, stack).await;
    assert_eq!(
        xml_text(&body, "StackStatus").as_deref(),
        Some("UPDATE_COMPLETE"),
        "case {case:?}: expected the update to complete, got {status} with {body}"
    );

    let updated = described_physical_id(&router, case, stack, "Escalation").await;
    assert_eq!(
        updated, created,
        "case {case:?}: expected the knowledge base to keep the id its create minted, got \
         {updated:?} against {created:?}"
    );
    let (status, knowledge_base) = get_knowledge_base(&router, &updated).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected GetKnowledgeBase({updated}) to answer 200, got {status} \
         with {knowledge_base}"
    );
    let description = knowledge_base.get("description").and_then(Value::as_str);
    assert_eq!(
        description,
        Some("the second revision"),
        "case {case:?}: expected the changed declaration to reach the knowledge base, got \
         {description:?} in {knowledge_base}"
    );

    // Deleting the stack deletes the knowledge base: the handle it published stops
    // resolving, which is what says the stack's resource really was that service's.
    let (status, body) = call(&router, "DeleteStack", &[member("StackName", stack)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DeleteStack({stack}) answered {status}: {body}"
    );
    let gone = poll_until_gone(&router, &updated).await;
    assert!(
        gone,
        "case {case:?}: expected the knowledge base {updated:?} to be removed from the \
         service that held it"
    );
}

/// How long the delete of a knowledge base is given to come through, and how often the stack
/// is polled while it does.
///
/// The removal is the OWNING service's clock, which only a read of the stack advances (the
/// deployment asks that service again on every read), so the poll is what a client's own
/// `stack_delete_complete` wait is: `DescribeStacks` until the stack is gone.
const REMOVAL_POLLS: usize = 40;
const REMOVAL_POLL_INTERVAL: Duration = Duration::from_millis(250);

/// Poll the stack until the service that holds the knowledge base no longer does.
async fn poll_until_gone(router: &MultiServiceRouter, knowledge_base_id: &str) -> bool {
    for _ in 0..REMOVAL_POLLS {
        let (status, _) = get_knowledge_base(router, knowledge_base_id).await;
        if status == 404 {
            return true;
        }
        tokio::time::sleep(REMOVAL_POLL_INTERVAL).await;
        // Every read of the stack is what asks the owning service whether its delete has
        // come through.
        let _ = call(router, "ListStacks", &[]).await;
    }
    false
}
