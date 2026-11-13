// SPDX-License-Identifier: CC0-1.0

//! What the `AWS::Serverless-2016-10-31` transform does with a layer version: the logical id
//! it gives it, the references it re-points at that id, and the version it really publishes
//! in AWS Lambda.
//!
//! The rule every row here holds the service to: **an expansion that gives a resource a
//! DIFFERENT logical id re-points every reference to that resource — a `Ref`, an
//! `Fn::GetAtt` in either spelling, an `Fn::Sub` variable and a `DependsOn` — at the id the
//! expansion gave it, in every section of the processed template, because the template
//! CloudFormation plans and deploys is the macro's OUTPUT.** An `AWS::Serverless::LayerVersion`
//! is the declaration that gets a different id: "AWS SAM … appends a hash to the logical ID",
//! because a layer version cannot be updated in place. A reference left naming the
//! declaration's own id resolves to nothing, which is what the recorded trace this suite was
//! written for hit: `CreateStack` refused with `Template format error: Unresolved resource
//! dependencies [Shared] in the Resources block of the template`.
//!
//! The rows span the FORMS a template refers to a resource by — which is what the rule
//! generalises over — and the SECTIONS they appear in, the recorded case (a `Ref` in a
//! function's `Layers`) being one of them.
//!
//! The second half of the suite is the resource itself: the version the stack publishes in
//! AWS Lambda, read back through that service's own wire, the function that really carries
//! it, and what the declaration's `RetentionPolicy` leaves behind once the stack is deleted.

use std::sync::Arc;

use serde_json::Value;
use smithy_server_runtime::MultiServiceRouter;

use crate::change_set_lifecycle::settle_the_store;
use crate::sam_transform::sam_template;
use crate::stack_resource_provisioning::stage_code_objects;
use crate::stack_lifecycle::{
    call, create_stack, get_template, member, router_with_engine, xml_blocks, xml_text,
    xml_unescape, ACCOUNT_ID,
};

/// The Region every request here is signed for, and so the Region the stack's resources —
/// and the ARNs they carry — belong to.
const REGION: &str = "us-east-1";

/// The logical id the template declares the layer under: the id every reference below
/// states, and the one the expansion replaces.
const DECLARED_ID: &str = "Shared";

/// How many hex characters the content hash the expansion appends to that id carries.
const DIGEST_CHARS: usize = 10;

/// The layer the rows' templates declare. One declaration, so every row's expansion hashes
/// the same content and the id under test is the same shape.
const LAYER: &str = r#""Shared":{"Type":"AWS::Serverless::LayerVersion","Properties":{
     "LayerName":"shared-deps","ContentUri":"s3://code-bucket/layer.zip",
     "CompatibleRuntimes":["python3.12"]}}"#;

/// A function whose `Layers` names the layer through `reference`.
fn function_with_layer(reference: &str) -> String {
    format!(
        r#""Worker":{{"Type":"AWS::Serverless::Function","Properties":{{
             "Handler":"index.handler","Runtime":"python3.12",
             "CodeUri":"s3://code-bucket/worker.zip",
             "Layers":[{reference}]}}}}"#
    )
}

/// One row of the reference-form table: the template it submits, and what to call it.
struct Row {
    case: &'static str,
    stack: &'static str,
    template: String,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a Ref in a function's Layers — the recorded case",
            stack: "sam-layer-ref",
            template: sam_template(&format!(
                "{{{LAYER},{}}}",
                function_with_layer(r#"{"Ref":"Shared"}"#)
            )),
        },
        Row {
            case: "an Fn::Sub variable in a function's Layers",
            stack: "sam-layer-sub",
            template: sam_template(&format!(
                "{{{LAYER},{}}}",
                function_with_layer(r#"{"Fn::Sub":"${Shared}"}"#)
            )),
        },
        Row {
            case: "a DependsOn naming the layer, as one name",
            stack: "sam-layer-dependson",
            template: sam_template(&format!(
                r#"{{{LAYER},"Notify":{{"Type":"AWS::SNS::Topic","DependsOn":"Shared"}}}}"#
            )),
        },
        Row {
            case: "a DependsOn naming the layer, as a list",
            stack: "sam-layer-dependson-list",
            template: sam_template(&format!(
                r#"{{{LAYER},"Notify":{{"Type":"AWS::SNS::Topic","DependsOn":["Shared"]}}}}"#
            )),
        },
        Row {
            case: "a Ref in an output",
            stack: "sam-layer-output-ref",
            template: format!(
                r#"{{"Transform":"AWS::Serverless-2016-10-31","Resources":{{{LAYER}}},
                    "Outputs":{{"Layer":{{"Value":{{"Ref":"Shared"}}}}}}}}"#
            ),
        },
        Row {
            case: "an Fn::Sub variable with text around it, in an output",
            stack: "sam-layer-output-sub",
            template: format!(
                r#"{{"Transform":"AWS::Serverless-2016-10-31","Resources":{{{LAYER}}},
                    "Outputs":{{"Layer":{{"Value":
                      {{"Fn::Sub":"the layer is ${{Shared}} today"}}}}}}}}"#
            ),
        },
        Row {
            case: "an Fn::Sub list form, whose own variable map supplies another name",
            stack: "sam-layer-output-sub-list",
            template: format!(
                r#"{{"Transform":"AWS::Serverless-2016-10-31","Resources":{{{LAYER}}},
                    "Outputs":{{"Layer":{{"Value":
                      {{"Fn::Sub":["${{Shared}}/${{Extra}}",{{"Extra":"tail"}}]}}}}}}}}"#
            ),
        },
        Row {
            case: "an Fn::GetAtt in an output, in both spellings",
            stack: "sam-layer-output-getatt",
            template: format!(
                r#"{{"Transform":"AWS::Serverless-2016-10-31","Resources":{{{LAYER}}},
                    "Outputs":{{
                      "ByList":{{"Value":{{"Fn::GetAtt":["Shared","Arn"]}}}},
                      "ByPath":{{"Value":{{"Fn::GetAtt":"Shared.Arn"}}}}}}}}"#
            ),
        },
    ]
}

/// The capabilities a template declaring the SAM macro — and the execution role the macro
/// synthesizes for a function — has to be submitted with.
fn capabilities() -> Vec<(String, String)> {
    vec![
        member("Capabilities.member.1", "CAPABILITY_IAM"),
        member("Capabilities.member.2", "CAPABILITY_AUTO_EXPAND"),
    ]
}

/// Create one row's stack and settle the store, answering what `DescribeStackResources`
/// reports: every `(logical id, type, physical id)` the deployed stack holds.
async fn deployed(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    case: &str,
    stack: &str,
    template: &str,
) -> Vec<(String, String, String)> {
    stage_code_objects(router, case, template).await;
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
        "case {case:?}: expected the stack to deploy the expanded template, got {body}"
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

/// The logical id the deployed stack holds the layer version under, asserted to be the
/// declared id plus a content hash.
fn hashed_layer_id(case: &str, deployed: &[(String, String, String)]) -> String {
    let layers: Vec<&(String, String, String)> = deployed
        .iter()
        .filter(|(_, resource_type, _)| resource_type == "AWS::Lambda::LayerVersion")
        .collect();
    assert_eq!(
        layers.len(),
        1,
        "case {case:?}: expected exactly one expanded layer version, got {layers:?}"
    );
    let id = layers[0].0.clone();
    let hash = id.strip_prefix(DECLARED_ID).unwrap_or_default();
    assert!(
        hash.len() == DIGEST_CHARS
            && hash
                .chars()
                .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)),
        "case {case:?}: expected the logical id {DECLARED_ID}<{DIGEST_CHARS} hex characters>, \
         got {id:?}"
    );
    id
}

/// The PROCESSED template body — the macro's output, which is what CloudFormation plans and
/// deploys.
async fn processed_template(router: &MultiServiceRouter, case: &str, stack: &str) -> String {
    let (status_code, body) = get_template(router, stack, None).await;
    assert_eq!(
        status_code, 200,
        "case {case:?}: expected GetTemplate to answer, got {status_code} {body}"
    );
    xml_text(&body, "TemplateBody")
        .map(|template| xml_unescape(&template))
        .unwrap_or_else(|| panic!("case {case:?}: expected a processed template, got {body}"))
}

/// Every way a template can still be naming the declaration's OWN logical id: a `Ref`, a
/// `DependsOn`, an `Fn::GetAtt` argument or a `Resources` key spell it as the quoted name;
/// an `Fn::Sub` variable and the dotted `Fn::GetAtt` spell it with what follows it.
///
/// The expanded id is `Shared` plus a hash, so none of these can match it: a hit is a
/// reference the expansion left behind.
fn left_behind(processed: &str) -> Vec<String> {
    [
        format!("\"{DECLARED_ID}\""),
        format!("\"{DECLARED_ID}."),
        format!("${{{DECLARED_ID}}}"),
        format!("${{{DECLARED_ID}."),
    ]
    .into_iter()
    .filter(|spelling| processed.contains(spelling.as_str()))
    .collect()
}

#[tokio::test]
async fn every_reference_to_a_renamed_serverless_layer_names_the_expanded_id() {
    for row in rows() {
        let (router, engine) = router_with_engine();
        let resources = deployed(&router, &engine, row.case, row.stack, &row.template).await;
        let hashed = hashed_layer_id(row.case, &resources);

        let processed = processed_template(&router, row.case, row.stack).await;
        let remaining = left_behind(&processed);
        assert!(
            remaining.is_empty(),
            "case {:?}: expected every reference to name {hashed:?}, got {remaining:?} left in \
             {processed}",
            row.case
        );
        assert!(
            processed.contains(&hashed),
            "case {:?}: expected the processed template to name {hashed:?}, got {processed}",
            row.case
        );
    }
}

// ---------------------------------------------------------------------------
// The resource itself: published in AWS Lambda, and what the stack's deletion leaves
// ---------------------------------------------------------------------------

/// Call one AWS Lambda operation over its restJson1 wire, signed for this suite's Region —
/// the Region the layer and the function the stack creates belong to.
async fn lambda(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    let payload = match body {
        Some(document) => serde_json::to_vec(document).expect("the request body serializes"),
        None => Vec::new(),
    };
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
        .body(bytes::Bytes::from(payload))
        .expect("a well-formed Lambda request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let answered: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, answered)
}

/// `GetLayerVersion` for one version of one layer.
async fn get_layer_version(
    router: &MultiServiceRouter,
    layer: &str,
    version: u32,
) -> (u16, Value) {
    lambda(
        router,
        "GET",
        &format!("/2018-10-31/layers/{layer}/versions/{version}"),
        None,
    )
    .await
}

/// The ARN a layer version carries: the total function of the Region, the account, the layer
/// name and the version number both services compute it from.
fn layer_version_arn(layer: &str, version: u32) -> String {
    format!("arn:aws:lambda:{REGION}:{ACCOUNT_ID}:layer:{layer}:{version}")
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

/// One row of the retention table: what the declaration says about keeping old versions, and
/// whether the version is still in Lambda once the stack is gone.
struct Retention {
    case: &'static str,
    stack: &'static str,
    layer: &'static str,
    /// The `RetentionPolicy` the declaration states, or `None` for the default.
    policy: Option<&'static str>,
    /// The `DeletionPolicy` the expansion must state on the generated resource.
    expected_policy: &'static str,
    /// Whether AWS Lambda still holds the version after the stack has been deleted.
    outlives_the_stack: bool,
}

/// "RetentionPolicy: specifies whether old versions of your layer version are retained or
/// deleted after an update" — `Retain` and `Delete` are its values, and the default is to
/// retain, which is what the generated layer version's `DeletionPolicy` then states.
fn retention_rows() -> Vec<Retention> {
    vec![
        Retention {
            case: "the default retention policy",
            stack: "sam-layer-retained",
            layer: "retained-deps",
            policy: None,
            expected_policy: "Retain",
            outlives_the_stack: true,
        },
        Retention {
            case: "a declared Delete retention policy",
            stack: "sam-layer-deleted",
            layer: "deleted-deps",
            policy: Some("Delete"),
            expected_policy: "Delete",
            outlives_the_stack: false,
        },
    ]
}

/// The template one retention row submits: the layer, a function that names it, and the
/// output a client reads the layer's ARN from.
fn retention_template(layer: &str, policy: Option<&str>) -> String {
    let retention = match policy {
        Some(policy) => format!(r#","RetentionPolicy":"{policy}""#),
        None => String::new(),
    };
    format!(
        r#"{{"Transform":"AWS::Serverless-2016-10-31","Resources":{{
             "Shared":{{"Type":"AWS::Serverless::LayerVersion","Properties":{{
               "LayerName":"{layer}","ContentUri":"s3://code-bucket/layer.zip",
               "CompatibleRuntimes":["python3.12"]{retention}}}}},
             {}}},
           "Outputs":{{"Layer":{{"Value":{{"Ref":"Shared"}}}}}}}}"#,
        function_with_layer(r#"{"Ref":"Shared"}"#)
    )
}

#[tokio::test]
async fn a_sam_layer_version_is_published_in_lambda_and_its_retention_survives_the_stack() {
    for row in retention_rows() {
        let (router, engine) = router_with_engine();
        let template = retention_template(row.layer, row.policy);
        let resources = deployed(&router, &engine, row.case, row.stack, &template).await;
        let hashed = hashed_layer_id(row.case, &resources);
        let expected_arn = layer_version_arn(row.layer, 1);

        // The expansion states what the declaration's retention asks for on the resource
        // CloudFormation governs.
        let processed = processed_template(&router, row.case, row.stack).await;
        let declaration: Value = serde_json::from_str(&processed).unwrap_or_else(|error| {
            panic!("case {:?}: expected a JSON processed template, got {error}", row.case)
        });
        let stated = declaration
            .get("Resources")
            .and_then(|resources| resources.get(&hashed))
            .and_then(|layer| layer.get("DeletionPolicy"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert_eq!(
            stated, row.expected_policy,
            "case {:?}: expected the expanded layer version to state the {} deletion policy, \
             got {stated:?} in {processed}",
            row.case, row.expected_policy
        );

        // The version really exists in AWS Lambda, read back through that service's own
        // wire: the stack's resource is one `GetLayerVersion` resolves.
        let (status_code, published) = get_layer_version(&router, row.layer, 1).await;
        assert_eq!(
            status_code, 200,
            "case {:?}: expected AWS Lambda to hold the published version, got {status_code} \
             {published}",
            row.case
        );
        assert_eq!(
            published.get("LayerVersionArn").and_then(Value::as_str),
            Some(expected_arn.as_str()),
            "case {:?}: expected the version {expected_arn:?}, got {published}",
            row.case
        );

        // "Ref returns the ARN of the layer version", so the stack's own output — and the
        // physical id it publishes for the resource — is that ARN.
        assert_eq!(
            physical_id(row.case, &resources, &hashed),
            expected_arn,
            "case {:?}: expected the resource's physical id to be the published version's \
             ARN, got {resources:?}",
            row.case
        );
        let (status_code, described) =
            call(&router, "DescribeStacks", &[member("StackName", row.stack)]).await;
        assert_eq!(
            status_code, 200,
            "case {:?}: expected DescribeStacks to answer, got {status_code} {described}",
            row.case
        );
        assert_eq!(
            xml_text(&described, "OutputValue").as_deref(),
            Some(expected_arn.as_str()),
            "case {:?}: expected the output to carry the published version's ARN, got \
             {described}",
            row.case
        );

        // ...and the function the same template declares really carries the layer.
        let function = physical_id(row.case, &resources, "Worker");
        let (status_code, configuration) = lambda(
            &router,
            "GET",
            &format!("/2015-03-31/functions/{function}/configuration"),
            None,
        )
        .await;
        assert_eq!(
            status_code, 200,
            "case {:?}: expected AWS Lambda to hold the function, got {status_code} \
             {configuration}",
            row.case
        );
        let carried: Vec<&str> = configuration
            .get("Layers")
            .and_then(Value::as_array)
            .map(|layers| {
                layers
                    .iter()
                    .filter_map(|layer| layer.get("Arn").and_then(Value::as_str))
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            carried,
            vec![expected_arn.as_str()],
            "case {:?}: expected the function's layers to be [{expected_arn:?}], got \
             {configuration}",
            row.case
        );

        // The stack's deletion: a retained version outlives the stack, a deleted one does
        // not — and the function goes either way.
        let (status_code, body) =
            call(&router, "DeleteStack", &[member("StackName", row.stack)]).await;
        assert_eq!(
            status_code, 200,
            "case {:?}: expected DeleteStack to be accepted, got {status_code} {body}",
            row.case
        );
        settle_the_store(&router, &engine).await;
        settle_the_store(&router, &engine).await;

        let (status_code, gone) = lambda(
            &router,
            "GET",
            &format!("/2015-03-31/functions/{function}/configuration"),
            None,
        )
        .await;
        assert_eq!(
            status_code, 404,
            "case {:?}: expected the deleted stack's function to be gone, got {status_code} \
             {gone}",
            row.case
        );

        let (status_code, after) = get_layer_version(&router, row.layer, 1).await;
        let expected_status = if row.outlives_the_stack { 200 } else { 404 };
        assert_eq!(
            status_code, expected_status,
            "case {:?}: expected AWS Lambda to answer {expected_status} for the version after \
             the stack was deleted, got {status_code} {after}",
            row.case
        );
    }
}
