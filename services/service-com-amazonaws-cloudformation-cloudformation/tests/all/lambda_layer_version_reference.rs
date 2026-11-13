// SPDX-License-Identifier: CC0-1.0

//! What a reference to an `AWS::Lambda::LayerVersion` of a hand-written template resolves to,
//! and what the function that names it really carries.
//!
//! The rule every row here holds the service to: **a resource's physical id is the identifier
//! the service that owns the resource ANSWERED with, so every reference to it in the template
//! resolves to that identifier.** For this type the identifier is the version ARN
//! `PublishLayerVersion` answers — "Ref returns the ARN of the layer version" — and it carries
//! a VERSION NUMBER AWS Lambda allocates ("each time you call PublishLayerVersion with the same
//! layer name, a new version is created"), so it is not a value CloudFormation could compute
//! for itself. A reference left resolving to the layer's NAME is what the recorded trace this
//! suite was written for hit: `AWS::Lambda::Function`'s `Layers` member takes a
//! `LayerVersionArn`, so a name there is refused by Lambda's own input validation and the
//! resource fails.
//!
//! The rows span what the rule generalises over: HOW the template refers to the layer (a `Ref`
//! in the function's `Layers`, an `Fn::Sub` variable, a stack output), and whether the
//! declaration NAMES the layer or leaves the name to CloudFormation. Beside the table: two
//! versions published under one layer name, which pins that the number in the ARN is Lambda's
//! to allocate rather than a constant.

use std::sync::Arc;

use serde_json::Value;
use smithy_server_runtime::MultiServiceRouter;

use crate::change_set_lifecycle::settle_the_store;
use crate::hotswapped_function_code::package;
use crate::stack_lifecycle::{
    call, create_stack, member, router_with_engine, xml_blocks, xml_text, ACCOUNT_ID,
};
use crate::stack_resource_provisioning::s3;

/// The bucket every template below names its archives in, and the keys they name in it.
const CODE_BUCKET: &str = "code-bucket";
const CODE_KEYS: &[&str] = &["layer.zip", "worker.zip", "first.zip", "second.zip"];

/// Stage the archives the templates name in Amazon S3, the way a client uploads them before
/// deploying: AWS Lambda READS the archive out of the object a `Content`/`Code` names, so the
/// object has to be there and has to be a readable .zip, or the publish is refused and the
/// stack rolls back.
async fn stage_archives(router: &MultiServiceRouter, case: &str) {
    let created = s3(router, "PUT", &format!("/{CODE_BUCKET}"), b"").await;
    assert!(
        (200..300).contains(&created.status),
        "case {case:?}: the code bucket must be created first, got {} {}",
        created.status,
        created.body
    );
    for key in CODE_KEYS {
        let archive = package("index.py", "def handler(event, context):\n    return {}\n");
        let staged = s3(router, "PUT", &format!("/{CODE_BUCKET}/{key}"), &archive).await;
        assert!(
            (200..300).contains(&staged.status),
            "case {case:?}: the archive {key} must be staged first, got {} {}",
            staged.status,
            staged.body
        );
    }
}

/// The Region every request here is signed for, and so the Region the layer and the function
/// the stack creates belong to.
const REGION: &str = "us-east-1";

/// The capabilities a template declaring an IAM role has to be submitted with.
fn capabilities() -> Vec<(String, String)> {
    vec![member("Capabilities.member.1", "CAPABILITY_IAM")]
}

/// Call one AWS Lambda operation over its restJson1 wire, signed for this suite's Region.
async fn lambda(router: &MultiServiceRouter, method: &str, uri: &str) -> (u16, Value) {
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("lambda.{REGION}.amazonaws.com"))
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/lambda/aws4_request, \
                 SignedHeaders=host, Signature=0123456789abcdef"
            ),
        )
        .body(bytes::Bytes::new())
        .expect("a well-formed Lambda request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let answered: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, answered)
}

/// The ARN one version of one layer carries: the total function of the Region, the account,
/// the layer name and the version number, which is how both services spell it.
fn layer_version_arn(layer: &str, version: u32) -> String {
    format!("arn:aws:lambda:{REGION}:{ACCOUNT_ID}:layer:{layer}:{version}")
}

/// Create a stack, settle the store and answer every `(logical id, type, physical id)`
/// `DescribeStackResources` reports for it.
async fn deployed(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    case: &str,
    stack: &str,
    template: &str,
) -> Vec<(String, String, String)> {
    stage_archives(router, case).await;
    let (status_code, body) = create_stack(router, stack, template, &capabilities()).await;
    assert_eq!(
        status_code, 200,
        "case {case:?}: expected CreateStack to be accepted, got {status_code} {body}"
    );
    settle_the_store(router, engine).await;

    let (status_code, body) = call(router, "DescribeStacks", &[member("StackName", stack)]).await;
    assert_eq!(
        status_code, 200,
        "case {case:?}: expected DescribeStacks to answer, got {status_code} {body}"
    );
    assert_eq!(
        xml_text(&body, "StackStatus").as_deref(),
        Some("CREATE_COMPLETE"),
        "case {case:?}: expected the stack to deploy, got {body}"
    );

    let (status_code, body) = call(
        router,
        "DescribeStackResources",
        &[member("StackName", stack)],
    )
    .await;
    assert_eq!(
        status_code, 200,
        "case {case:?}: expected DescribeStackResources to answer, got {status_code} {body}"
    );
    xml_blocks(&body, "member")
        .iter()
        .map(|entry| {
            (
                xml_text(entry, "LogicalResourceId").unwrap_or_default(),
                xml_text(entry, "ResourceType").unwrap_or_default(),
                xml_text(entry, "PhysicalResourceId").unwrap_or_default(),
            )
        })
        .collect()
}

/// The physical id the stack published for one of its logical resources.
fn physical_id(case: &str, deployed: &[(String, String, String)], logical_id: &str) -> String {
    deployed
        .iter()
        .find(|(id, _, _)| id == logical_id)
        .map(|(_, _, physical)| physical.clone())
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected the stack to hold {logical_id}, got {deployed:?}")
        })
}

/// The ARNs of the layers AWS Lambda reports on the function `name`.
async fn carried_layers(router: &MultiServiceRouter, case: &str, name: &str) -> Vec<String> {
    let (status_code, configuration) = lambda(
        router,
        "GET",
        &format!("/2015-03-31/functions/{name}/configuration"),
    )
    .await;
    assert_eq!(
        status_code, 200,
        "case {case:?}: expected AWS Lambda to hold the function, got {status_code} \
         {configuration}"
    );
    configuration
        .get("Layers")
        .and_then(Value::as_array)
        .map(|layers| {
            layers
                .iter()
                .filter_map(|layer| layer.get("Arn").and_then(Value::as_str))
                .map(ToString::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// The execution role every template here declares, and the function's reference to it.
const ROLE: &str = r#""Role":{"Type":"AWS::IAM::Role","Properties":{
     "AssumeRolePolicyDocument":{"Version":"2012-10-17","Statement":[{"Effect":"Allow",
       "Principal":{"Service":"lambda.amazonaws.com"},"Action":"sts:AssumeRole"}]}}}"#;

/// One row: how the function names the layer, and whether the declaration names the layer
/// itself.
struct Row {
    case: &'static str,
    stack: &'static str,
    /// The `LayerName` the declaration states, or `None` to leave the name to CloudFormation.
    layer_name: Option<&'static str>,
    /// The `Layers` member of the function, which is where the reference under test sits.
    layers: &'static str,
}

/// The rows. Each names the layer through a DIFFERENT form, and the last leaves the layer
/// unnamed so CloudFormation's own generated name is what the publish is addressed by.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a Ref in the function's Layers",
            stack: "plain-layer-ref",
            layer_name: Some("plain-deps"),
            layers: r#"[{"Ref":"Layer"}]"#,
        },
        Row {
            case: "an Fn::Sub variable in the function's Layers",
            stack: "plain-layer-sub",
            layer_name: Some("sub-deps"),
            layers: r#"[{"Fn::Sub":"${Layer}"}]"#,
        },
        Row {
            case: "an Fn::Join over the reference",
            stack: "plain-layer-join",
            layer_name: Some("join-deps"),
            layers: r#"[{"Fn::Join":["",[{"Ref":"Layer"}]]}]"#,
        },
        Row {
            case: "a layer CloudFormation names",
            stack: "plain-layer-generated",
            layer_name: None,
            layers: r#"[{"Ref":"Layer"}]"#,
        },
    ]
}

/// The template a row deploys: the layer, the role and the function whose `Layers` is the
/// row's reference, with the reference published as an output too.
fn template(row: &Row) -> String {
    let name = match row.layer_name {
        Some(name) => format!(r#""LayerName":"{name}","#),
        None => String::new(),
    };
    format!(
        r#"{{"Resources":{{
             "Layer":{{"Type":"AWS::Lambda::LayerVersion","Properties":{{
               {name}"Content":{{"S3Bucket":"code-bucket","S3Key":"layer.zip"}},
               "CompatibleRuntimes":["python3.12"]}}}},
             {ROLE},
             "Worker":{{"Type":"AWS::Lambda::Function","Properties":{{
               "FunctionName":"{}-worker","Handler":"index.handler","Runtime":"python3.12",
               "Role":{{"Fn::GetAtt":["Role","Arn"]}},
               "Code":{{"S3Bucket":"code-bucket","S3Key":"worker.zip"}},
               "Layers":{}}}}}}},
           "Outputs":{{"LayerReference":{{"Value":{{"Ref":"Layer"}}}}}}}}"#,
        row.stack, row.layers
    )
}

/// What a layer version ARN this emulator published names: the layer and the version number.
/// Split here rather than compared whole, because the layer a row leaves unnamed is named by
/// CloudFormation and so is not a value this suite can write down.
fn parsed_layer_version(case: &str, arn: &str) -> (String, String) {
    let prefix = format!("arn:aws:lambda:{REGION}:{ACCOUNT_ID}:layer:");
    let rest = arn.strip_prefix(&prefix).unwrap_or_else(|| {
        panic!("case {case:?}: expected a layer version ARN under {prefix:?}, got {arn:?}")
    });
    let (layer, version) = rest.rsplit_once(':').unwrap_or_else(|| {
        panic!("case {case:?}: expected the ARN to end in a version number, got {arn:?}")
    });
    (layer.to_string(), version.to_string())
}

/// RULE: every reference to an `AWS::Lambda::LayerVersion` resolves to the version ARN AWS
/// Lambda answered the publish with — the resource's physical id, the stack's output, and the
/// value the function's `Layers` reaches that service with.
#[tokio::test]
async fn every_reference_to_a_layer_version_resolves_to_the_published_arn() {
    for row in rows() {
        let (router, engine) = router_with_engine();
        let resources = deployed(&router, &engine, row.case, row.stack, &template(&row)).await;
        let published = physical_id(row.case, &resources, "Layer");

        // The physical id is a version ARN, whose layer segment is the declared name where the
        // declaration states one and CloudFormation's generated name where it does not, and
        // whose last segment is the version AWS Lambda allocated.
        let (layer, version) = parsed_layer_version(row.case, &published);
        if let Some(declared) = row.layer_name {
            assert_eq!(
                layer, declared,
                "case {:?}: expected the ARN to name the declared layer, got {published:?}",
                row.case
            );
        } else {
            assert!(
                !layer.is_empty() && layer != "Layer",
                "case {:?}: expected the ARN to name the layer CloudFormation generated a name \
                 for, got {published:?}",
                row.case
            );
        }
        assert_eq!(
            version, "1",
            "case {:?}: expected the first version of the layer, got {published:?}",
            row.case
        );

        // AWS Lambda really holds that version under that ARN.
        let (status_code, held) = lambda(
            &router,
            "GET",
            &format!("/2018-10-31/layers/{layer}/versions/{version}"),
        )
        .await;
        assert_eq!(
            status_code, 200,
            "case {:?}: expected AWS Lambda to hold the published version, got {status_code} \
             {held}",
            row.case
        );
        assert_eq!(
            held.get("LayerVersionArn").and_then(Value::as_str),
            Some(published.as_str()),
            "case {:?}: expected the version to report {published:?}, got {held}",
            row.case
        );

        // ...and the function the same template declares carries exactly that ARN, which is
        // what the reference in its `Layers` resolved to.
        let carried = carried_layers(&router, row.case, &format!("{}-worker", row.stack)).await;
        assert_eq!(
            carried,
            vec![published.clone()],
            "case {:?}: expected the function's layers to be [{published:?}], got {carried:?}",
            row.case
        );

        // The stack's own output resolves the same reference to the same ARN.
        let (status_code, described) =
            call(&router, "DescribeStacks", &[member("StackName", row.stack)]).await;
        assert_eq!(
            status_code, 200,
            "case {:?}: expected DescribeStacks to answer, got {status_code} {described}",
            row.case
        );
        assert_eq!(
            xml_text(&described, "OutputValue").as_deref(),
            Some(published.as_str()),
            "case {:?}: expected the output to carry {published:?}, got {described}",
            row.case
        );
    }
}

/// RULE, on the part of the ARN no template states: the version number is AWS Lambda's to
/// allocate, so two layer versions published under ONE layer name are two different
/// identifiers — and each function carries the one its own reference named.
#[tokio::test]
async fn two_versions_of_one_layer_carry_the_numbers_lambda_allocated() {
    let case = "two versions under one layer name";
    let layer = "shared-deps";
    let template = format!(
        r#"{{"Resources":{{
             "First":{{"Type":"AWS::Lambda::LayerVersion","Properties":{{
               "LayerName":"{layer}",
               "Content":{{"S3Bucket":"code-bucket","S3Key":"first.zip"}}}}}},
             "Second":{{"Type":"AWS::Lambda::LayerVersion","DependsOn":"First","Properties":{{
               "LayerName":"{layer}",
               "Content":{{"S3Bucket":"code-bucket","S3Key":"second.zip"}}}}}},
             {ROLE},
             "Worker":{{"Type":"AWS::Lambda::Function","Properties":{{
               "FunctionName":"two-version-worker","Handler":"index.handler",
               "Runtime":"python3.12","Role":{{"Fn::GetAtt":["Role","Arn"]}},
               "Code":{{"S3Bucket":"code-bucket","S3Key":"worker.zip"}},
               "Layers":[{{"Ref":"Second"}}]}}}}}}}}"#
    );
    let (router, engine) = router_with_engine();
    let resources = deployed(&router, &engine, case, "plain-layer-versions", &template).await;

    let first = physical_id(case, &resources, "First");
    let second = physical_id(case, &resources, "Second");
    assert_eq!(
        first,
        layer_version_arn(layer, 1),
        "case {case:?}: expected the first publish to be version 1, got {first:?}"
    );
    assert_eq!(
        second,
        layer_version_arn(layer, 2),
        "case {case:?}: expected the second publish to be version 2, got {second:?}"
    );

    let carried = carried_layers(&router, case, "two-version-worker").await;
    assert_eq!(
        carried,
        vec![second.clone()],
        "case {case:?}: expected the function to carry the version its reference named \
         ({second:?}), got {carried:?}"
    );
}
