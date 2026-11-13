// SPDX-License-Identifier: CC0-1.0

//! A CloudFormation stack that declares the Amazon S3 Tables table-bucket resource type
//! really creates it in S3 Tables, and the handle the stack hands back is the ARN that
//! service answers to — beside the two sibling bucket variants, each published under the
//! identifier ITS owning service takes.
//!
//! The rule under test: **a resource type's physical resource id (and so what `Ref`
//! resolves to) is the identifier the OWNING service's own API accepts for the resource —
//! the resource's ARN for a type whose owning service keys it by one, and the resource's
//! name for a type whose owning service keys it by that — so a client holding a stack's
//! `PhysicalResourceId` or a `Ref` output can pass it straight to that service.** Amazon S3
//! keys a bucket by `Bucket=<name>`, while Amazon S3 Tables keys a table bucket by
//! `tableBucketARN` and Amazon S3 Vectors a vector bucket by `vectorBucketArn`, so the three
//! variants publish two different identifier SHAPES from one stack. Four crates have to
//! agree for that to hold (CloudFormation makes the calls, the three S3 services serve
//! them), so the suite lives here rather than in any one of them.
//!
//! The rows differ along what the rule generalises over: which bucket variants the template
//! declares, whether the table bucket is NAMED or left for CloudFormation to generate a
//! name, the declared name's length at both published bounds, whether `Tags` and an
//! `EncryptionConfiguration` are declared, the Region deployed in, both template spellings,
//! and which intrinsic the outputs read the resource with (`Ref`, `Fn::GetAtt`, `Fn::Sub`).
//! A declared `Tags` list travels with the create — a row that declares one reaches
//! CREATE_COMPLETE only if S3 Tables took it — but is not read back here: that service's
//! `ListTagsForResource` is still scaffolded, and this suite asserts only what an
//! implemented read answers.
//!
//! Every expected value is computed here from what the row declares, never read back out of
//! the answer being checked — except the GENERATED name of the row that declares none, which
//! is CloudFormation's to compute: what is checked about it is that the ARN the stack
//! publishes is that name's table-bucket ARN and that S3 Tables resolves it.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::query_wire::{form_encode, opt_element, outputs_of, physical_ids_of};

/// Long enough for the stack's clock-owed span (`STATUS_TRANSITION_DELAY`, 500 ms) to come
/// due, so the stack reports the resources it deployed.
const PAST_STACK_SPAN: Duration = Duration::from_millis(900);

/// The account id every ARN this emulator mints carries.
const ACCOUNT: &str = "000000000000";

/// The emulator with the four services this suite exercises, mounted on ONE engine exactly
/// as `core/router/src/lib.rs` mounts them. Request authentication is off, as in every other
/// suite here; the in-process callback CloudFormation makes signs its request either way
/// (the credential scope is what routes it).
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_cloudformation_cloudformation::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_s3tables_s3tablebuckets::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_s3vectors_s3vectors::EmulatedService::new(engine),
    );
    router
}

/// A SigV4 `Authorization` header whose credential scope names `service` in `region`, which
/// is what routes the request and what each handler reads its Region from. The signature is
/// never checked: this router runs with request authentication off.
fn authorization(service: &str, region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{region}/{service}/aws4_request, \
         SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000"
    )
}

/// Call one CloudFormation operation over its awsQuery wire, signed for `region`.
async fn cloudformation(
    router: &MultiServiceRouter,
    region: &str,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version=2010-05-15");
    for (key, value) in members {
        body.push('&');
        body.push_str(&form_encode(key));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("cloudformation.{region}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization("cloudformation", region))
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).into_owned())
}

/// An ARN as a single `@httpLabel` path segment: its `:` and `/` travel percent-encoded, or
/// the request addresses a different route.
fn label(value: &str) -> String {
    value
        .replace('%', "%25")
        .replace(':', "%3A")
        .replace('/', "%2F")
}

/// Call one Amazon S3 Tables operation over its restJson1 wire, signed for `region`, on the
/// method and URI its own `@http` binding declares.
async fn s3tables(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    uri: &str,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("s3tables.{region}.amazonaws.com"))
        .header("content-type", "application/json")
        .header("authorization", authorization("s3tables", region))
        .body(Bytes::new())
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, payload)
}

/// `GetTableBucket` on the ARN a stack published — the read a client holding only that
/// handle makes.
async fn get_table_bucket(
    router: &MultiServiceRouter,
    region: &str,
    arn: &str,
) -> (u16, Value) {
    s3tables(router, region, "GET", &format!("/buckets/{}", label(arn))).await
}

/// Call one Amazon S3 Vectors operation over its restJson1 wire: a POST to the operation's
/// own URI, which is how that model binds every one of them.
async fn s3vectors(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("/{operation}"))
        .header("host", format!("s3vectors.{region}.amazonaws.com"))
        .header("content-type", "application/json")
        .header("authorization", authorization("s3vectors", region))
        .body(Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, payload)
}

/// `GET /<bucket>?location` — Amazon S3's `GetBucketLocation`, the read a client holding
/// only the bucket NAME makes, which is the identifier shape that service takes.
async fn get_bucket_location(
    router: &MultiServiceRouter,
    region: &str,
    bucket: &str,
) -> (u16, String) {
    let host = format!("s3.{region}.amazonaws.com");
    let request = http::Request::builder()
        .method("GET")
        .uri(format!("https://{host}/{bucket}?location"))
        .header("host", host)
        .header("authorization", authorization("s3", region))
        .body(Bytes::new())
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).into_owned())
}

/// A table bucket's ARN in `region`: the same total function of the Region and the name both
/// services compute it from (`arn:aws:s3tables:<region>:<account>:bucket/<name>`).
fn table_bucket_arn(region: &str, name: &str) -> String {
    format!("arn:aws:s3tables:{region}:{ACCOUNT}:bucket/{name}")
}

/// A vector bucket's ARN in `region`, which differs from the table bucket's only in the ARN
/// namespace its owning service publishes.
fn vector_bucket_arn(region: &str, name: &str) -> String {
    format!("arn:aws:s3vectors:{region}:{ACCOUNT}:bucket/{name}")
}

/// Whether `name` is inside the name form the S3 Tables model publishes for a table bucket
/// (`CreateTableBucket.name`: `@length(min: 3, max: 63)`, pattern `^[0-9a-z-]*$`), and
/// begins and ends with an alphanumeric, which that service holds a name to besides.
fn is_s3tables_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    let alnum = |b: u8| b.is_ascii_lowercase() || b.is_ascii_digit();
    (3..=63).contains(&bytes.len())
        && alnum(bytes[0])
        && alnum(bytes[bytes.len() - 1])
        && bytes.iter().all(|b| alnum(*b) || *b == b'-')
}

/// The sibling variants one row declares beside its table bucket, each published under the
/// identifier its own service takes.
struct Siblings {
    /// The `AWS::S3::Bucket`'s logical id and the name the template gives it, whose physical
    /// id is that bare name.
    plain: (&'static str, &'static str),
    /// The `AWS::S3Vectors::VectorBucket`'s logical id and name, whose physical id is its
    /// own ARN.
    vectors: (&'static str, &'static str),
    /// The output keys carrying each sibling's `Ref`, in the same order.
    outputs: (&'static str, &'static str),
}

/// One row of the table: a template, and what the rule says the resources it declares must
/// then be.
struct Row {
    case: &'static str,
    /// The Region the row deploys in, which is the Region the buckets are created in and the
    /// one their ARNs carry.
    region: &'static str,
    /// The template body, in the form the row writes it (JSON or YAML).
    template: fn() -> String,
    /// The table bucket's logical id in the template.
    logical_id: &'static str,
    /// The `TableBucketName` the template declares, or `None` for a template that names
    /// nothing — whose bucket carries the name CloudFormation generated.
    declared_name: Option<&'static str>,
    /// The output keys whose value must be the table bucket's ARN, each written with a
    /// different intrinsic over the same resource.
    arn_outputs: &'static [&'static str],
    /// The `sseAlgorithm` `GetTableBucketEncryption` must then report, for a row that
    /// declares an `EncryptionConfiguration`.
    encryption: Option<&'static str>,
    /// The two sibling bucket variants, for the row that declares all three.
    siblings: Option<Siblings>,
}

/// The shortest name the published form admits, and the longest: the bounds a declared name
/// is checked at, written here so the row and the assertion cannot disagree.
const SHORTEST_NAME: &str = "abc";
const LONGEST_NAME: &str = "s3tables-stack-bucket-at-the-published-maximum-length-00000-abc";

const ROWS: &[Row] = &[
    Row {
        case: "the three bucket variants in one stack, each read with Ref",
        region: "us-east-1",
        template: || {
            json!({
                "AWSTemplateFormatVersion": "2010-09-09",
                "Description": "the three S3 bucket variants",
                "Resources": {
                    "Plain": {
                        "Type": "AWS::S3::Bucket",
                        "Properties": { "BucketName": "sim2real-ids-plain" }
                    },
                    "Vectors": {
                        "Type": "AWS::S3Vectors::VectorBucket",
                        "Properties": { "VectorBucketName": "sim2real-ids-v" }
                    },
                    "Tables": {
                        "Type": "AWS::S3Tables::TableBucket",
                        "Properties": { "TableBucketName": "sim2real-ids-t" }
                    }
                },
                "Outputs": {
                    "Plain": { "Value": { "Ref": "Plain" } },
                    "Vectors": { "Value": { "Ref": "Vectors" } },
                    "Tables": { "Value": { "Ref": "Tables" } }
                }
            })
            .to_string()
        },
        logical_id: "Tables",
        declared_name: Some("sim2real-ids-t"),
        arn_outputs: &["Tables"],
        encryption: None,
        siblings: Some(Siblings {
            plain: ("Plain", "sim2real-ids-plain"),
            vectors: ("Vectors", "sim2real-ids-v"),
            outputs: ("Plain", "Vectors"),
        }),
    },
    Row {
        case: "a tagged table bucket at the shortest published name, read with Fn::GetAtt",
        region: "us-west-2",
        template: || {
            json!({
                "Resources": {
                    "Tables": {
                        "Type": "AWS::S3Tables::TableBucket",
                        "Properties": {
                            "TableBucketName": SHORTEST_NAME,
                            "Tags": [
                                { "Key": "Owner", "Value": "analytics" },
                                { "Key": "Tier", "Value": "prod" }
                            ]
                        }
                    }
                },
                "Outputs": {
                    "BucketRef": { "Value": { "Ref": "Tables" } },
                    "BucketArn": {
                        "Value": { "Fn::GetAtt": ["Tables", "TableBucketARN"] }
                    }
                }
            })
            .to_string()
        },
        logical_id: "Tables",
        declared_name: Some(SHORTEST_NAME),
        arn_outputs: &["BucketRef", "BucketArn"],
        encryption: None,
        siblings: None,
    },
    Row {
        case: "a table bucket at the longest published name, in YAML, read with Fn::Sub",
        region: "eu-west-1",
        template: || {
            [
                "Resources:",
                "  Tables:",
                "    Type: AWS::S3Tables::TableBucket",
                "    Properties:",
                "      TableBucketName: BUCKET_NAME",
                "      EncryptionConfiguration:",
                "        SSEAlgorithm: AES256",
                "Outputs:",
                "  BucketRef:",
                "    Value: !Ref Tables",
                "  BucketSub:",
                "    Value: !Sub",
                "      - ${Arn}",
                "      - Arn: !Ref Tables",
                "",
            ]
            .join("\n")
            .replace("BUCKET_NAME", LONGEST_NAME)
        },
        logical_id: "Tables",
        declared_name: Some(LONGEST_NAME),
        arn_outputs: &["BucketRef", "BucketSub"],
        encryption: Some("AES256"),
        siblings: None,
    },
    Row {
        case: "a table bucket the template does not name, so CloudFormation generates one",
        region: "ap-northeast-1",
        template: || {
            [
                "Resources:",
                "  Tables:",
                "    Type: AWS::S3Tables::TableBucket",
                "Outputs:",
                "  BucketRef:",
                "    Value: !Ref Tables",
                "",
            ]
            .join("\n")
        },
        logical_id: "Tables",
        declared_name: None,
        arn_outputs: &["BucketRef"],
        encryption: None,
        siblings: None,
    },
];

/// Every row: the handle the stack reports for its table bucket is that bucket's own ARN, S3
/// Tables itself resolves that ARN to the bucket the template declared, the sibling variants
/// are published under the identifier THEIR services take, and deleting the stack deletes
/// the bucket there.
#[tokio::test]
async fn a_stack_publishes_each_bucket_variant_under_its_owning_services_identifier() {
    let router = build_router();

    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let region = row.region;
        let stack_name = format!("s3tables-stack-{index}");
        let template = (row.template)();

        let (status, body) = cloudformation(
            &router,
            region,
            "CreateStack",
            &[("StackName", &stack_name), ("TemplateBody", &template)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {body}"
        );

        // The stack reports what it deployed once the create COMPLETES.
        tokio::time::sleep(PAST_STACK_SPAN).await;
        let (status, described) =
            cloudformation(&router, region, "DescribeStacks", &[("StackName", &stack_name)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeStacks, got {status} with body \
             {described}"
        );
        assert_eq!(
            opt_element(&described, "StackStatus").as_deref(),
            Some("CREATE_COMPLETE"),
            "case {case:?}: expected the stack to reach CREATE_COMPLETE, got {described}"
        );

        let (status, resources) = cloudformation(
            &router,
            region,
            "DescribeStackResources",
            &[("StackName", &stack_name)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeStackResources, got {status} with \
             body {resources}"
        );
        let reported = physical_ids_of(&resources);
        let physical_id = reported.get(row.logical_id).unwrap_or_else(|| {
            panic!(
                "case {case:?}: no physical id for {:?} in {resources}",
                row.logical_id
            )
        });

        // "Ref returns the table bucket ARN": the physical id is that ARN, and the name
        // inside it is the one the template asked for — or, for a template that named
        // nothing, the one CloudFormation generated.
        let name = match row.declared_name {
            Some(declared) => declared.to_string(),
            None => physical_id
                .rsplit_once("bucket/")
                .map(|(_, name)| name.to_string())
                .unwrap_or_default(),
        };
        let arn = table_bucket_arn(region, &name);
        assert_eq!(
            physical_id, &arn,
            "case {case:?}: expected the physical id to be the table bucket ARN {arn}, got \
             {physical_id:?}"
        );
        assert!(
            physical_id.ends_with(&format!(":bucket/{name}")),
            "case {case:?}: expected an ARN ending in :bucket/{name}, got {physical_id:?}"
        );
        assert!(
            is_s3tables_name(&name),
            "case {case:?}: expected a bucket name inside the published name form, got \
             {name:?}"
        );

        let outputs = outputs_of(&described);
        for key in row.arn_outputs {
            assert_eq!(
                outputs.get(*key),
                Some(&arn),
                "case {case:?}: expected output {key:?} to be the table bucket ARN {arn:?}, \
                 got {:?}",
                outputs.get(*key)
            );
        }

        // And S3 Tables itself resolves that ARN to the bucket the template declared.
        let (status, answered) = get_table_bucket(&router, region, &arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from GetTableBucket on the ARN the stack \
             reported, got {status} with body {answered}"
        );
        assert_eq!(
            answered.get("arn").and_then(Value::as_str),
            Some(arn.as_str()),
            "case {case:?}: expected the read to report {arn}, got {answered}"
        );
        assert_eq!(
            answered.get("name").and_then(Value::as_str),
            Some(name.as_str()),
            "case {case:?}: expected the read to report the bucket name {name:?}, got \
             {answered}"
        );

        // The encryption the row declared reached that service too, read back by the same
        // ARN.
        if let Some(algorithm) = row.encryption {
            let (status, encryption) = s3tables(
                &router,
                region,
                "GET",
                &format!("/buckets/{}/encryption", label(&arn)),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 from GetTableBucketEncryption, got {status} \
                 with body {encryption}"
            );
            assert_eq!(
                encryption
                    .get("encryptionConfiguration")
                    .and_then(|configuration| configuration.get("sseAlgorithm"))
                    .and_then(Value::as_str),
                Some(algorithm),
                "case {case:?}: expected the declared sseAlgorithm {algorithm:?}, got \
                 {encryption}"
            );
        }

        // The sibling variants of the row that declares them: the S3 bucket is published
        // under its bare NAME and resolves in S3 by that name, while the vector bucket is
        // published under its own ARN and resolves in S3 Vectors by that ARN.
        if let Some(siblings) = &row.siblings {
            let (plain_logical, plain_name) = siblings.plain;
            let plain_id = reported.get(plain_logical).unwrap_or_else(|| {
                panic!("case {case:?}: no physical id for {plain_logical:?} in {resources}")
            });
            assert_eq!(
                plain_id, plain_name,
                "case {case:?}: expected the S3 bucket's physical id to be the bare name \
                 {plain_name:?}, got {plain_id:?}"
            );
            assert_eq!(
                outputs.get(siblings.outputs.0).map(String::as_str),
                Some(plain_name),
                "case {case:?}: expected output {:?} to be the bucket name {plain_name:?}, \
                 got {:?}",
                siblings.outputs.0,
                outputs.get(siblings.outputs.0)
            );
            let (status, located) = get_bucket_location(&router, region, plain_id).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 from GetBucketLocation on the name the \
                 stack reported, got {status} with body {located}"
            );

            let (vectors_logical, vectors_name) = siblings.vectors;
            let vectors_arn = vector_bucket_arn(region, vectors_name);
            let vectors_id = reported.get(vectors_logical).unwrap_or_else(|| {
                panic!("case {case:?}: no physical id for {vectors_logical:?} in {resources}")
            });
            assert_eq!(
                vectors_id, &vectors_arn,
                "case {case:?}: expected the vector bucket's physical id to be the ARN \
                 {vectors_arn}, got {vectors_id:?}"
            );
            assert_eq!(
                outputs.get(siblings.outputs.1),
                Some(&vectors_arn),
                "case {case:?}: expected output {:?} to be the vector bucket ARN \
                 {vectors_arn:?}, got {:?}",
                siblings.outputs.1,
                outputs.get(siblings.outputs.1)
            );
            let (status, vector_bucket) = s3vectors(
                &router,
                region,
                "GetVectorBucket",
                &json!({ "vectorBucketArn": vectors_arn }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 from GetVectorBucket on the ARN the stack \
                 reported, got {status} with body {vector_bucket}"
            );
        }

        // Deleting the stack deletes what S3 Tables holds for it, addressed by that same
        // ARN.
        let (status, body) =
            cloudformation(&router, region, "DeleteStack", &[("StackName", &stack_name)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DeleteStack, got {status} with body {body}"
        );
        let (status, gone) = get_table_bucket(&router, region, &arn).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected HTTP 404 from GetTableBucket once the stack deleted the \
             bucket, got {status} with body {gone}"
        );
    }
}

/// A stack whose template declares a table bucket S3 Tables REFUSES does not report a
/// resource nothing created: the stack fails with the refusal the owning service gave, and
/// no bucket exists under the name the template asked for.
///
/// The rows are the three ways a declared name leaves the form that service publishes for
/// one (`^[0-9a-z-]*$`, 3 to 63 characters): upper case, one character under the minimum,
/// and one character past the maximum.
#[tokio::test]
async fn a_table_bucket_the_owning_service_refuses_fails_the_stack() {
    let router = build_router();
    let region = "us-east-2";
    let past_longest = format!("{LONGEST_NAME}z");

    for (index, (case, declared)) in [
        ("an upper-case table bucket name", "Sim2Real-Ids-T".to_string()),
        (
            "a table bucket name one character under the published minimum",
            "ab".to_string(),
        ),
        (
            "a table bucket name one character past the published maximum",
            past_longest,
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let stack_name = format!("s3tables-refused-{index}");
        let template = json!({
            "Resources": {
                "Tables": {
                    "Type": "AWS::S3Tables::TableBucket",
                    "Properties": { "TableBucketName": declared }
                }
            }
        })
        .to_string();
        let (status, body) = cloudformation(
            &router,
            region,
            "CreateStack",
            &[("StackName", &stack_name), ("TemplateBody", &template)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CreateStack — the template declares a type \
             CloudFormation publishes — got {status} with body {body}"
        );
        tokio::time::sleep(PAST_STACK_SPAN).await;

        let (_, described) =
            cloudformation(&router, region, "DescribeStacks", &[("StackName", &stack_name)]).await;
        let reached = opt_element(&described, "StackStatus").unwrap_or_default();
        assert!(
            reached.starts_with("CREATE_FAILED") || reached.starts_with("ROLLBACK"),
            "case {case:?}: expected the stack not to report a created resource, got \
             {reached:?} in {described}"
        );
        let reason = opt_element(&described, "StackStatusReason").unwrap_or_default();
        assert!(
            reason.contains("Tables"),
            "case {case:?}: expected the failure to name the resource, got {reason:?}"
        );

        // Nothing was created under the ARN that name would have carried.
        let (status, gone) =
            get_table_bucket(&router, region, &table_bucket_arn(region, &declared)).await;
        assert_ne!(
            status, 200,
            "case {case:?}: expected no table bucket named {declared:?}, the read answered \
             {status} with body {gone}"
        );
    }
}
