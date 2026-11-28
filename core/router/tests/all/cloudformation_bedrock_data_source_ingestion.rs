// SPDX-License-Identifier: CC0-1.0

//! The handle a CloudFormation stack publishes for an `AWS::Bedrock::DataSource` is the one
//! Amazon Bedrock Agent itself issued, so the documented deploy-then-ingest pattern works: the
//! `DataSourceId` a stack exports is accepted by every operation of that service which takes a
//! `dataSourceId`.
//!
//! The rule under test: **for a resource type whose identifier the OWNING service mints, every
//! handle a stack publishes for it — `Ref`, the physical resource id, the `Fn::GetAtt` of its id
//! attribute — carries the identifier that service answered, never a value CloudFormation
//! decided (the declared `Name`), whatever form the declaration's own name takes.** A data
//! source is such a resource: `CreateDataSource` takes a `name` of
//! `^([0-9a-zA-Z][_-]?){1,100}$` and answers a `dataSourceId` targeting `Id`
//! (`^[0-9a-zA-Z]{10}$`), which is the label `StartIngestionJob`, `GetIngestionJob`,
//! `ListIngestionJobs` and `GetDataSource` each bind — so a stack that exported the name instead
//! publishes a handle every one of those calls refuses on that pattern. Two crates have to agree
//! for the rule to hold (CloudFormation makes the create, Bedrock Agent mints the id and serves
//! the ingestion calls, and the knowledge base resolves its vector store in S3 Vectors), so the
//! suite lives here rather than in any one of them.
//!
//! The rows differ along what the rule generalises over: the FORM of the declared `Name` — the
//! value the defect this suite pins handed out in the id's place — spanning the modelled `Name`
//! pattern (`smithy_query(query="shape",shape="Name")`): its shortest admissible name, a
//! hyphenated and an underscored name longer than an id, a name that is itself ten alphanumeric
//! characters (so it has the id's own FORM and is still not the id), and a name of the hundred
//! characters the pattern's hundred groups admit.
//!
//! Every expected value here is computed from what the row declares, except the minted
//! identifiers, which are the owning service's to assign: what is asserted about those is their
//! FORM, that every reader of the stack reports the same one, and that the owning service
//! resolves them — while the declared name, which is NOT an id, is asserted to be refused as the
//! label it is not.

use std::collections::BTreeMap;
use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The XML readers this crate writes ONCE (`tests/all/query_wire.rs`): "Nothing new should write
// another one — call this module."
use super::query_wire::{outputs_of, physical_ids_of};
// The shared stack waiter and the awsQuery CloudFormation caller this test crate writes ONCE
// rather than per suite; both sign for the Region below.
use crate::cloudformation_firehose_stream_stabilisation::wait_for_stack_status;
use crate::sns_lambda_subscription_permission_boundary::{cloudformation, CREATE_COMPLETE};

/// The account id every ARN this emulator mints carries.
const ACCOUNT: &str = "000000000000";

/// The Region this stack deploys in.
const REGION: &str = "us-east-1";

/// How many characters a minted `dataSourceId` carries, and which characters: the modelled `Id`
/// shape every `dataSourceId` and `ingestionJobId` member targets (`@pattern ^[0-9a-zA-Z]{10}$`).
const ID_CHARACTERS: usize = 10;

/// The modelled expression, as the model writes it; a refusal quotes it WITHOUT the
/// whole-value anchors ([`STATED_ID_PATTERN`]), which is how the service advertises the
/// expression a value has to satisfy.
const ID_PATTERN: &str = "^[0-9a-zA-Z]{10}$";

/// [`ID_PATTERN`] as a refusal of a value outside it quotes it: the expression minus the
/// `^` / `$` the model writes around it to demand a whole-string match.
const STATED_ID_PATTERN: &str = "[0-9a-zA-Z]{10}";

/// How many groups the modelled `Name` pattern admits (`^([0-9a-zA-Z][_-]?){1,100}$`), which is
/// the longest name of single-character groups a declaration can state.
const NAME_GROUPS: usize = 100;

/// The separator CloudFormation joins the parts of a COMPOSITE physical resource id with, which
/// an `AWS::Bedrock::DataSource`'s `Ref` returns the knowledge base and the data source id
/// joined with.
const COMPOSITE_SEPARATOR: &str = "|";

/// The `IngestionJobStatus` a job this suite starts can be reported in before the clock has
/// carried it further: the modelled enum's first state ("STARTING").
const STARTING: &str = "STARTING";

/// The embedding model the knowledge base reads, and how many values its vectors carry: Titan
/// Text Embeddings G1 produces 1536, which is the length the declared index holds.
const TITAN_G1: &str = "amazon.titan-embed-text-v1";
const TITAN_G1_DIMENSION: u32 = 1536;

/// The S3 bucket every declared connector reads, as `S3DataSourceConfiguration$bucketArn`'s own
/// pattern spells one (`^arn:aws(|-cn|-us-gov):s3:::[a-z0-9][a-z0-9.-]{1,61}[a-z0-9]$`).
const DOCUMENT_BUCKET_ARN: &str = "arn:aws:s3:::dsingest-router-documents";

/// The emulator with the three services this suite exercises, mounted on ONE engine exactly as
/// `core/router/src/lib.rs` mounts them. Request authentication is off, as in every other suite
/// here; the in-process callback CloudFormation makes signs its request either way (the
/// credential scope is what routes it).
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_cloudformation_cloudformation::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_s3vectors_s3vectors::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_bedrockagent_amazonbedrockagentbuildtimelambda::EmulatedService::new(
            engine,
        ),
    );
    router
}

/// A SigV4 `Authorization` header whose credential scope names `service` in [`REGION`], which is
/// what routes the request and what each handler reads its Region from.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{REGION}/{service}/\
         aws4_request, SignedHeaders=host, Signature=0000000000000000000000000000000000000000\
         000000000000000000000000"
    )
}

/// One answer Amazon Bedrock Agent gave over its `restJson1` wire: the status, the parsed
/// document, and — for a refusal — the shape name it names and the message it carries.
#[derive(Debug)]
struct Answer {
    status: u16,
    document: Value,
    /// The `restJson1` error shape name, from `x-amzn-errortype` or the body's `__type`; the
    /// header form can carry a trailing `:<url>`, which is not part of the name.
    error_type: Option<String>,
}

impl Answer {
    /// The `message` the refusal carries, panicking (quoting the whole answer) where it carries
    /// none: a refusal's wording is a requirement, so an answer without one is a defect rather
    /// than something to assert around.
    fn message(&self, case: &str) -> String {
        self.document
            .get("message")
            .and_then(Value::as_str)
            .map(ToString::to_string)
            .unwrap_or_else(|| panic!("case {case:?}: no message in {self:?}"))
    }
}

/// Call one Amazon Bedrock Agent operation over the `restJson1` wire its model declares, on the
/// method and URI that operation's own `@http` binding names, signed for `bedrock` — the signing
/// name every Bedrock-family model declares.
async fn bedrock_agent(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> Answer {
    let payload = match &body {
        Some(document) => Bytes::from(document.to_string()),
        None => Bytes::new(),
    };
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("bedrock.{REGION}.amazonaws.com"))
        .header("content-type", "application/json")
        .header("authorization", authorization("bedrock"))
        .body(payload)
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let header_error = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(|value| {
            value
                .split(':')
                .next()
                .unwrap_or(value)
                .rsplit('/')
                .next()
                .unwrap_or(value)
                .to_string()
        });
    let document: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    let error_type = header_error.or_else(|| {
        document
            .get("__type")
            .and_then(Value::as_str)
            .map(|value| value.rsplit('#').next().unwrap_or(value).to_string())
    });
    Answer {
        status,
        document,
        error_type,
    }
}

/// Whether `value` is an identifier of the form this service mints: exactly [`ID_CHARACTERS`]
/// alphanumeric characters, the whole of the modelled `Id` pattern.
fn is_minted_id(value: &str) -> bool {
    value.len() == ID_CHARACTERS && value.chars().all(|c| c.is_ascii_alphanumeric())
}

/// The whole refusal a value outside the modelled `Id` pattern is answered with when it is
/// submitted as a `dataSourceId`: fixed text with the submitted value and the member it was
/// submitted for as its variable parts.
fn pattern_refusal(submitted: &str) -> String {
    format!(
        "1 validation error detected: Value '{submitted}' at 'dataSourceId' failed to satisfy \
         constraint: Member must satisfy regular expression pattern: {STATED_ID_PATTERN}"
    )
}

/// The whole refusal an id of the modelled form that names no data source is answered with:
/// `ResourceNotFoundException`, worded about the resource the request addresses by its own
/// identifier.
fn not_found_refusal(submitted: &str) -> String {
    format!("DataSource with id {submitted} is not found.")
}

/// One declared data source: its logical id in the template and the `Name` it states.
struct Row {
    case: &'static str,
    logical_id: &'static str,
    name: String,
}

impl Row {
    /// How the owning service refuses a call that addresses this row's data source by its
    /// declared NAME instead of by the minted id — the call a stack that published the name
    /// makes its client make: the `(status, shape, message)` the name's own form earns.
    ///
    /// A name outside the `Id` pattern cannot be a `dataSourceId` at all, so the request is
    /// refused on the pattern before anything is looked up; a name that happens to have the id's
    /// form is a well-formed id that names no data source.
    fn refusal_by_name(&self) -> (u16, &'static str, String) {
        if is_minted_id(&self.name) {
            (404, "ResourceNotFoundException", not_found_refusal(&self.name))
        } else {
            (400, "ValidationException", pattern_refusal(&self.name))
        }
    }
}

/// The rows: one per FORM the declared `Name` can take, which is what the rule generalises over.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the shortest name the modelled pattern admits",
            logical_id: "Shortest",
            name: "d".to_string(),
        },
        Row {
            case: "a hyphenated name longer than an id",
            logical_id: "Manuals",
            name: "dsingest-router-manuals".to_string(),
        },
        Row {
            case: "an underscored name longer than an id",
            logical_id: "Tickets",
            name: "dsingest_router_tickets".to_string(),
        },
        Row {
            // Ten alphanumeric characters: the FORM of an id, and still not the id the owning
            // service minted for this data source.
            case: "a name that is itself ten alphanumeric characters",
            logical_id: "IdShaped",
            name: "dsingest10".to_string(),
        },
        Row {
            case: "a name of the hundred characters the pattern's groups admit",
            logical_id: "Longest",
            name: format!(
                "dsingestrouter{}",
                "x".repeat(NAME_GROUPS - "dsingestrouter".len())
            ),
        },
    ]
}

/// The template: the vector store, the knowledge base over it, and one data source per row, with
/// each data source's identity published both ways.
fn template(declared: &[Row]) -> String {
    let mut resources = serde_json::Map::new();
    resources.insert(
        "Vectors".to_string(),
        json!({
            "Type": "AWS::S3Vectors::VectorBucket",
            "Properties": { "VectorBucketName": "dsingest-router-vectors" }
        }),
    );
    resources.insert(
        "Embeddings".to_string(),
        json!({
            "Type": "AWS::S3Vectors::Index",
            "Properties": {
                "VectorBucketArn": { "Ref": "Vectors" },
                "IndexName": "dsingest-router-embeddings",
                "DataType": "float32",
                "Dimension": TITAN_G1_DIMENSION,
                "DistanceMetric": "cosine"
            }
        }),
    );
    resources.insert(
        "Corpus".to_string(),
        json!({
            "Type": "AWS::Bedrock::KnowledgeBase",
            "Properties": {
                "Name": "dsingest-router-corpus",
                "RoleArn": format!("arn:aws:iam::{ACCOUNT}:role/dsingest-router"),
                "KnowledgeBaseConfiguration": {
                    "Type": "VECTOR",
                    "VectorKnowledgeBaseConfiguration": {
                        "EmbeddingModelArn":
                            format!("arn:aws:bedrock:{REGION}::foundation-model/{TITAN_G1}")
                    }
                },
                "StorageConfiguration": {
                    "Type": "S3_VECTORS",
                    "S3VectorsConfiguration": { "IndexArn": { "Ref": "Embeddings" } }
                }
            }
        }),
    );
    let mut outputs = serde_json::Map::new();
    outputs.insert(
        "CorpusRef".to_string(),
        json!({ "Value": { "Ref": "Corpus" } }),
    );
    for row in declared {
        resources.insert(
            row.logical_id.to_string(),
            json!({
                "Type": "AWS::Bedrock::DataSource",
                "Properties": {
                    "KnowledgeBaseId": { "Ref": "Corpus" },
                    "Name": row.name,
                    "DataSourceConfiguration": {
                        "Type": "S3",
                        "S3Configuration": { "BucketArn": DOCUMENT_BUCKET_ARN }
                    }
                }
            }),
        );
        outputs.insert(
            format!("{}Ref", row.logical_id),
            json!({ "Value": { "Ref": row.logical_id } }),
        );
        outputs.insert(
            format!("{}Id", row.logical_id),
            json!({ "Value": { "Fn::GetAtt": [row.logical_id, "DataSourceId"] } }),
        );
    }
    json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Resources": resources,
        "Outputs": outputs
    })
    .to_string()
}

/// `GetDataSource` for one data source of one knowledge base.
async fn get_data_source(
    router: &MultiServiceRouter,
    knowledge_base_id: &str,
    data_source_id: &str,
) -> Answer {
    bedrock_agent(
        router,
        "GET",
        &format!("/knowledgebases/{knowledge_base_id}/datasources/{data_source_id}"),
        None,
    )
    .await
}

/// `StartIngestionJob` for one data source of one knowledge base — the call a client makes with
/// the handle the stack published.
async fn start_ingestion_job(
    router: &MultiServiceRouter,
    knowledge_base_id: &str,
    data_source_id: &str,
) -> Answer {
    bedrock_agent(
        router,
        "PUT",
        &format!(
            "/knowledgebases/{knowledge_base_id}/datasources/{data_source_id}/ingestionjobs/"
        ),
        Some(json!({})),
    )
    .await
}

/// Every `ingestionJobSummaries` entry `ListIngestionJobs` reports for one data source, over all
/// its pages.
async fn list_ingestion_jobs(
    router: &MultiServiceRouter,
    knowledge_base_id: &str,
    data_source_id: &str,
) -> Vec<Value> {
    let mut summaries = Vec::new();
    let mut next_token: Option<String> = None;
    loop {
        let mut request = json!({ "maxResults": 1 });
        if let Some(token) = &next_token {
            request["nextToken"] = Value::String(token.clone());
        }
        let answer = bedrock_agent(
            router,
            "POST",
            &format!(
                "/knowledgebases/{knowledge_base_id}/datasources/{data_source_id}/ingestionjobs/"
            ),
            Some(request),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "ListIngestionJobs on {data_source_id}: expected HTTP 200, got {answer:?}"
        );
        summaries.extend(
            answer
                .document
                .get("ingestionJobSummaries")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default(),
        );
        next_token = answer
            .document
            .get("nextToken")
            .and_then(Value::as_str)
            .map(ToString::to_string);
        if next_token.is_none() {
            return summaries;
        }
    }
}

/// The member of one answered structure, as the text it carries.
fn member<'a>(document: &'a Value, path: &str, member_name: &str) -> Option<&'a str> {
    document
        .get(path)
        .and_then(|inner| inner.get(member_name))
        .and_then(Value::as_str)
}

/// The id a stack publishes for a data source is the one Amazon Bedrock Agent minted, so an
/// ingestion job can be started for the data source the stack reported created — while the
/// declared NAME, whatever its form, is refused as the label it is not.
#[tokio::test]
async fn a_stack_publishes_an_ingestible_data_source_id() {
    let router = build_router();
    let stack = "dsingest-router";
    let declared = rows();
    // The shared caller asserts the 200 itself, so a setup that did not deploy fails here.
    cloudformation(
        &router,
        "CreateStack",
        &[("StackName", stack), ("TemplateBody", &template(&declared))],
    )
    .await;

    let (reached, described) = wait_for_stack_status(&router, stack, CREATE_COMPLETE).await;
    assert_eq!(
        reached, CREATE_COMPLETE,
        "setup: expected the stack to reach {CREATE_COMPLETE}, got {described}"
    );
    let outputs: BTreeMap<String, String> = outputs_of(&described);
    let knowledge_base_id = outputs
        .get("CorpusRef")
        .cloned()
        .unwrap_or_else(|| panic!("setup: no CorpusRef output in {described}"));
    assert!(
        is_minted_id(&knowledge_base_id),
        "setup: expected the published knowledge base id to be of the modelled {ID_PATTERN} \
         form, got {knowledge_base_id:?}"
    );

    let resources =
        cloudformation(&router, "DescribeStackResources", &[("StackName", stack)]).await;
    let physical_ids = physical_ids_of(&resources);

    for row in &declared {
        let case = row.case;
        let published = outputs
            .get(&format!("{}Id", row.logical_id))
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: no DataSourceId output in {described}"));

        // The published handle is an id of the form the owning service mints, which is what
        // every call on a data source binds — and not the name the template decided.
        assert!(
            is_minted_id(&published),
            "case {case:?}: expected GetAtt DataSourceId to be an identifier of the modelled \
             {ID_PATTERN} form, got {published:?}"
        );
        assert_ne!(
            published, row.name,
            "case {case:?}: expected GetAtt DataSourceId to be the minted id rather than the \
             declared name, got {published:?}"
        );

        // `Ref` and the resource's physical id are the PAIR the type is identified by, built
        // around that same id.
        let identity = format!("{knowledge_base_id}{COMPOSITE_SEPARATOR}{published}");
        for (reader, answered) in [
            ("Ref", outputs.get(&format!("{}Ref", row.logical_id))),
            ("PhysicalResourceId", physical_ids.get(row.logical_id)),
        ] {
            assert_eq!(
                answered,
                Some(&identity),
                "case {case:?}: expected {reader} to be {identity:?}, got {answered:?}"
            );
        }

        // The published id resolves, in the owning service, to the data source the row declared.
        let read = get_data_source(&router, &knowledge_base_id, &published).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected HTTP 200 from GetDataSource on the published id, got \
             {read:?}"
        );
        let name = member(&read.document, "dataSource", "name");
        assert_eq!(
            name,
            Some(row.name.as_str()),
            "case {case:?}: expected the resolved data source's name to be {:?}, got {name:?}",
            row.name
        );

        // ...and an ingestion job can be started for it: the call the documented
        // deploy-then-ingest pattern makes with the exported handle.
        let started = start_ingestion_job(&router, &knowledge_base_id, &published).await;
        assert_eq!(
            started.status, 202,
            "case {case:?}: expected HTTP 202 from StartIngestionJob on the published id, got \
             {started:?}"
        );
        for (member_name, expected) in [
            ("knowledgeBaseId", knowledge_base_id.as_str()),
            ("dataSourceId", published.as_str()),
            ("status", STARTING),
        ] {
            let answered = member(&started.document, "ingestionJob", member_name);
            assert_eq!(
                answered,
                Some(expected),
                "case {case:?}: expected the started job's {member_name:?} to be {expected:?}, \
                 got {answered:?} in {started:?}"
            );
        }
        let job_id = member(&started.document, "ingestionJob", "ingestionJobId")
            .unwrap_or_else(|| panic!("case {case:?}: no ingestionJobId in {started:?}"))
            .to_string();
        assert!(
            is_minted_id(&job_id),
            "case {case:?}: expected the minted ingestionJobId to be of the modelled \
             {ID_PATTERN} form, got {job_id:?}"
        );

        // The job is the data source's own, so it is readable and listed under the published id.
        let fetched = bedrock_agent(
            &router,
            "GET",
            &format!(
                "/knowledgebases/{knowledge_base_id}/datasources/{published}/ingestionjobs/\
                 {job_id}"
            ),
            None,
        )
        .await;
        assert_eq!(
            fetched.status, 200,
            "case {case:?}: expected HTTP 200 from GetIngestionJob, got {fetched:?}"
        );
        let fetched_id = member(&fetched.document, "ingestionJob", "ingestionJobId");
        assert_eq!(
            fetched_id,
            Some(job_id.as_str()),
            "case {case:?}: expected GetIngestionJob to resolve {job_id:?}, got {fetched_id:?}"
        );
        let listed: Vec<String> = list_ingestion_jobs(&router, &knowledge_base_id, &published)
            .await
            .iter()
            .filter_map(|summary| {
                summary
                    .get("ingestionJobId")
                    .and_then(Value::as_str)
                    .map(ToString::to_string)
            })
            .collect();
        assert_eq!(
            listed,
            vec![job_id.clone()],
            "case {case:?}: expected ListIngestionJobs to report exactly the started job, got \
             {listed:?}"
        );

        // The declared NAME is not a handle: the call a stack that published it would make is
        // refused, with the wording the submitted value earns.
        let (expected_status, expected_shape, expected_message) = row.refusal_by_name();
        let by_name = start_ingestion_job(&router, &knowledge_base_id, &row.name).await;
        assert_eq!(
            by_name.status, expected_status,
            "case {case:?}: expected HTTP {expected_status} from StartIngestionJob addressed by \
             the declared name, got {by_name:?}"
        );
        assert_eq!(
            by_name.error_type.as_deref(),
            Some(expected_shape),
            "case {case:?}: expected the refusal to be {expected_shape}, got {by_name:?}"
        );
        let message = by_name.message(case);
        assert_eq!(
            message, expected_message,
            "case {case:?}: expected the refusal {expected_message:?}, got {message:?}"
        );
    }
}
