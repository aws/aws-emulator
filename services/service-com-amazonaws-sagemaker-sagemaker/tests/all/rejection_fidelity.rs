// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for how SageMaker CLASSIFIES and WORDS a rejection.
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.<Op>`,
//! `awsJson1_1`) through the same `MultiServiceRouter` the production router uses, via
//! the `Emulator` harness `inference_deployment_lifecycle` owns, so what these tests
//! exercise is what a real SDK client gets.
//!
//! Each test asserts ONE rule over a TABLE of inputs rather than one recorded call:
//!
//! 1. registering a model package identifier that is already registered is a REQUEST
//!    defect — 400 `ValidationException`, not the retryable conflict the operation also
//!    models;
//! 2. the lineage readers report an entity the account does not hold as
//!    `<Entity> '<resolved ARN>' does not exist.`, for either form of the identifier;
//! 3. the duplicate-model-card conflict names the card by ARN and by the version the
//!    stored card is at;
//! 4. the pipeline readers use the same quoted-ARN template as the lineage ones;
//! 5. the hosting operations (model, endpoint configuration, endpoint) quote the
//!    identifier the REQUEST carried, never an ARN the service synthesised for it;
//! 6. a map-value constraint is stated against the map, echoing every entry and listing
//!    the value shape's whole constraint set;
//! 7. an enum value set is listed in the order the service declares the enum;
//! 8. a `@pattern` is reported WITHOUT the `^`/`$` anchors the model spells it with;
//! 9. a value outside the TRAINING instance types is told the condition it failed instead
//!    of being handed that value set, wherever the member sits.

use serde_json::{json, Map, Value};

use crate::hyperparameter_tuning_job::valid_body as tuning_body;
use crate::inference_deployment_lifecycle::{
    error_code, message, one_variant_config, primary_container, str_field, Emulator, ACCOUNT,
};
use crate::model_card_governance::{content_with_description, create_body as model_card_body};

/// The `ValidationException` envelope around exactly one clause, as the framework writes
/// it.
fn one_violation(clause: &str) -> String {
    format!("1 validation error detected: {clause}")
}

/// One constraint clause: the value echoed, the member path, and the requirement.
fn clause(value: &str, path: &str, requirement: &str) -> String {
    format!("Value '{value}' at '{path}' failed to satisfy constraint: {requirement}")
}

/// The requirement a `@pattern` violation states, for the expression the service reports.
fn pattern_requirement(reported: &str) -> String {
    format!("Member must satisfy regular expression pattern: {reported}")
}

/// A `arn:aws:sagemaker:<region>:<account>:<kind>/<name>` ARN, as this service mints one.
fn arn(region: &str, kind: &str, name: &str) -> String {
    format!("arn:aws:sagemaker:{region}:{ACCOUNT}:{kind}/{name}")
}

/// A `CreateAction` body carrying the three members the model marks required
/// (`smithy_query(query="operation",operation="CreateAction")`).
fn action_body(name: &str) -> Value {
    json!({
        "ActionName": name,
        "ActionType": "ModelDeployment",
        "Source": { "SourceUri": "https://example.test/deployments/1" },
    })
}

/// A `CreatePipeline` body for `name`, in the one published definition schema version.
fn pipeline_body(name: &str) -> Value {
    json!({
        "PipelineName": name,
        "PipelineDefinition": json!({
            "Version": "2020-12-01",
            "Metadata": {},
            "Steps": [{
                "Name": "Only",
                "Type": "Fail",
                "Arguments": { "ErrorMessage": "stopped" },
            }],
        })
        .to_string(),
        "ClientRequestToken": format!("{name:-<32}"),
        "RoleArn": format!("arn:aws:iam::{ACCOUNT}:role/service-role/pipeline-role"),
    })
}

/// A `CreateTransformJob` body naming `model`, with `input_uri` as the input S3 URI and
/// `output_uri` as the output path.
fn transform_body(name: &str, model: &str, input_uri: &str, output_uri: &str) -> Value {
    json!({
        "TransformJobName": name,
        "ModelName": model,
        "TransformInput": {
            "DataSource": {
                "S3DataSource": { "S3DataType": "S3Prefix", "S3Uri": input_uri },
            },
        },
        "TransformOutput": { "S3OutputPath": output_uri },
        "TransformResources": { "InstanceType": "ml.m5.large", "InstanceCount": 1 },
    })
}

/// A `CreateTrainingJob` body whose one input channel reads `input_uri`.
fn training_body(name: &str, input_uri: &str) -> Value {
    json!({
        "TrainingJobName": name,
        "AlgorithmSpecification": {
            "TrainingImage": format!("{ACCOUNT}.dkr.ecr.us-east-1.amazonaws.com/trainer:1"),
            "TrainingInputMode": "File",
        },
        "RoleArn": format!("arn:aws:iam::{ACCOUNT}:role/service-role/training-role"),
        "InputDataConfig": [{
            "ChannelName": "train",
            "DataSource": {
                "S3DataSource": { "S3DataType": "S3Prefix", "S3Uri": input_uri },
            },
        }],
        "OutputDataConfig": { "S3OutputPath": "s3://demo-bucket/output/" },
        "ResourceConfig": {
            "InstanceType": "ml.m5.large",
            "InstanceCount": 1,
            "VolumeSizeInGB": 10,
        },
        "StoppingCondition": { "MaxRuntimeInSeconds": 3600 },
    })
}

/// [`training_body`] with `resources` as its `ResourceConfig`, so a row varies the ML
/// compute resources of an otherwise valid request.
fn training_resources(name: &str, resources: Value) -> Value {
    let mut body = training_body(name, "s3://demo-bucket/train/");
    body["ResourceConfig"] = resources;
    body
}

/// A `CreateAlgorithm` body whose `TrainingSpecification` supports `instance_types`.
fn algorithm_body(name: &str, instance_types: Value) -> Value {
    json!({
        "AlgorithmName": name,
        "TrainingSpecification": {
            "TrainingImage": format!("{ACCOUNT}.dkr.ecr.us-east-1.amazonaws.com/trainer:1"),
            "SupportedTrainingInstanceTypes": instance_types,
            "TrainingChannels": [{
                "Name": "train",
                "SupportedContentTypes": ["text/csv"],
                "SupportedInputModes": ["File"],
            }],
        },
    })
}

/// RULE: a model package identifier is in use once it is registered, so a second
/// registration under that identifier is refused as a REQUEST defect — HTTP 400
/// `ValidationException` — while a registration into a model package group is not a
/// duplicate at all: the service assigns the next version.
#[tokio::test]
async fn re_registering_a_model_package_identifier_is_a_validation_exception() {
    // Rows differ along the identifier's shape, including the modelled 63-character
    // upper bound of `EntityName`, and along the signed region.
    let rows: [(&str, &str, String); 4] = [
        ("a one-character name", "us-east-1", "a".to_string()),
        ("a hyphenated name", "us-east-1", "duplicate-package".to_string()),
        ("a mixed-case name", "eu-west-1", "Mixed-Case-9".to_string()),
        ("a name at the 63-character bound", "ap-south-1", "p".repeat(63)),
    ];

    for (case, region, name) in rows {
        let emulator = Emulator::new();
        let first = emulator
            .call_in(
                region,
                "CreateModelPackage",
                &json!({ "ModelPackageName": name, "ModelPackageDescription": "first" }),
            )
            .await;
        assert_eq!(
            first.0, 200,
            "case {case:?}: the first registration must succeed, got {first:?}"
        );

        let (status, refused) = emulator
            .call_in(
                region,
                "CreateModelPackage",
                &json!({ "ModelPackageName": name, "ModelPackageDescription": "second" }),
            )
            .await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (400, "ValidationException"),
            "case {case:?}: a re-registration is a request defect, got {status} {refused:?}"
        );
        assert!(
            message(&refused).contains(&name),
            "case {case:?}: the refusal must name the identifier, got {:?}",
            message(&refused)
        );
        // The refused create wrote nothing: the first package still stands.
        let described = emulator
            .call_in(
                region,
                "DescribeModelPackage",
                &json!({ "ModelPackageName": name }),
            )
            .await;
        assert_eq!(
            str_field(&described.1, "ModelPackageDescription"),
            "first",
            "case {case:?}: the refused create must not overwrite the package, \
             got {described:?}"
        );
    }

    // The contrast row: a second registration into a GROUP is a new version, not a
    // duplicate, so it succeeds.
    let emulator = Emulator::new();
    emulator
        .ok("CreateModelPackageGroup", &json!({ "ModelPackageGroupName": "grp" }))
        .await;
    for version in 1..=2 {
        let registered = emulator
            .ok("CreateModelPackage", &json!({ "ModelPackageGroupName": "grp" }))
            .await;
        assert_eq!(
            str_field(&registered, "ModelPackageArn"),
            arn("us-east-1", "model-package", &format!("grp/{version}")),
            "case \"version {version} of a group\": expected the next version's ARN, \
             got {registered:?}"
        );
    }
}

/// RULE: SageMaker's lineage readers report an entity the account does not hold as
/// `<Entity> '<resolved ARN>' does not exist.` — the identifier resolved to its ARN
/// (a name against the Region the request signed for, an ARN as it arrived), single
/// quoted — with the modelled `ResourceNotFound` and HTTP 400.
#[tokio::test]
async fn a_missing_action_is_reported_by_its_resolved_arn() {
    // Rows differ along the operation, the identifier's form (name or ARN), the signed
    // region, and why the entity is missing (never created, deleted, a near miss of one
    // that exists).
    let rows: [(&str, &str, &str, &str); 6] = [
        ("never created, by name", "DescribeAction", "us-east-1", "missing"),
        ("a near miss, by name", "DescribeAction", "us-east-1", "existing1"),
        ("a case difference", "DescribeAction", "eu-west-1", "Existing"),
        ("never created, delete", "DeleteAction", "us-east-1", "missing"),
        ("another region, delete", "DeleteAction", "ap-south-1", "missing"),
        ("a 120-character name", "DescribeAction", "us-east-1", "placeholder"),
    ];

    for (case, operation, region, name) in rows {
        let emulator = Emulator::new();
        emulator
            .call_in(region, "CreateAction", &action_body("existing"))
            .await;
        let name = if case == "a 120-character name" {
            "n".repeat(120)
        } else {
            name.to_string()
        };

        let (status, body) = emulator
            .call_in(region, operation, &json!({ "ActionName": name }))
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ResourceNotFound"),
            "case {case:?}: expected 400 ResourceNotFound, got {status} {body:?}"
        );
        assert_eq!(
            message(&body),
            format!("Action '{}' does not exist.", arn(region, "action", &name)),
            "case {case:?}: expected the resolved ARN, single-quoted, got {:?}",
            message(&body)
        );
    }

    // An ARN the caller passed is already resolved, so it is reported as it arrived —
    // including one naming another Region, which addresses no action here.
    let emulator = Emulator::new();
    for (case, identifier) in [
        ("an ARN for a name nobody created", arn("us-east-1", "action", "missing")),
        ("an ARN naming another Region", arn("us-west-2", "action", "missing")),
    ] {
        let (status, body) = emulator
            .call("DescribeAction", &json!({ "ActionName": identifier }))
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ResourceNotFound"),
            "case {case:?}: expected 400 ResourceNotFound, got {status} {body:?}"
        );
        assert_eq!(
            message(&body),
            format!("Action '{identifier}' does not exist."),
            "case {case:?}: an ARN is reported as it arrived, got {:?}",
            message(&body)
        );
    }

    // A deleted action is as missing as one nobody created, and reported the same way.
    let emulator = Emulator::new();
    emulator.ok("CreateAction", &action_body("doomed")).await;
    emulator
        .ok("DeleteAction", &json!({ "ActionName": "doomed" }))
        .await;
    let (status, body) = emulator
        .call("DescribeAction", &json!({ "ActionName": "doomed" }))
        .await;
    assert_eq!(
        (status, message(&body)),
        (
            400,
            format!(
                "Action '{}' does not exist.",
                arn("us-east-1", "action", "doomed")
            )
        ),
        "case \"a deleted action\": expected the resolved ARN, got {status} {body:?}"
    );
}

/// RULE: a model card name already in use is refused with the modelled
/// `ConflictException` (HTTP 400) whose message names the card by its stored ARN and by
/// the version the stored card is AT — `Modelcard <arn> with version <n> already exists`
/// — so a client can tell which version it collided with.
#[tokio::test]
async fn the_duplicate_model_card_conflict_names_the_arn_and_the_stored_version() {
    let content = content_with_description("the card the duplicate must not replace");

    // Rows differ along the name's shape, the signed region, and how many versions the
    // stored card has when the duplicate arrives — the version the message names is the
    // card's latest, not a constant.
    let rows: [(&str, &str, &str, i64); 4] = [
        ("one version", "us-east-1", "behav-mc", 1),
        ("one version, another region", "eu-west-1", "a", 1),
        ("two versions", "us-east-1", "versioned-card", 2),
        ("three versions", "ap-south-1", "Thrice-Updated-9", 3),
    ];

    for (case, region, name, versions) in rows {
        let emulator = Emulator::new();
        emulator
            .call_in(region, "CreateModelCard", &model_card_body(name, "Draft", &content))
            .await;
        for revision in 2..=versions {
            let updated = emulator
                .call_in(
                    region,
                    "UpdateModelCard",
                    &json!({
                        "ModelCardName": name,
                        "Content": content_with_description(&format!("revision {revision}")),
                    }),
                )
                .await;
            assert_eq!(
                updated.0, 200,
                "case {case:?}: the content update must add version {revision}, \
                 got {updated:?}"
            );
        }

        let (status, refused) = emulator
            .call_in(region, "CreateModelCard", &model_card_body(name, "Draft", &content))
            .await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (400, "ConflictException"),
            "case {case:?}: expected 400 ConflictException, got {status} {refused:?}"
        );
        assert_eq!(
            message(&refused),
            format!(
                "Modelcard {} with version {versions} already exists",
                arn(region, "model-card", name)
            ),
            "case {case:?}: expected the ARN and the stored version, got {:?}",
            message(&refused)
        );
    }
}

/// RULE: the pipeline readers use the same template as the lineage ones — the resolved
/// ARN, single-quoted, then `does not exist.` — whichever form of `PipelineNameOrArn`
/// the request carried.
#[tokio::test]
async fn a_missing_pipeline_is_reported_by_its_quoted_arn() {
    // Rows differ along the operation, the identifier's form, the signed region, and
    // whether the pipeline was deleted or never created.
    let rows: [(&str, &str, &str, bool, bool); 5] = [
        ("describe, by name", "DescribePipeline", "us-east-1", false, false),
        ("describe, by ARN", "DescribePipeline", "us-east-1", true, false),
        ("describe after a delete", "DescribePipeline", "eu-west-1", false, true),
        ("delete, by name", "DeletePipeline", "us-east-1", false, false),
        ("delete after a delete", "DeletePipeline", "ap-south-1", false, true),
    ];

    for (case, operation, region, by_arn, deleted) in rows {
        let emulator = Emulator::new();
        let name = "pipe-under-test";
        if deleted {
            emulator
                .call_in(region, "CreatePipeline", &pipeline_body(name))
                .await;
            let removed = emulator
                .call_in(
                    region,
                    "DeletePipeline",
                    &json!({ "PipelineName": name, "ClientRequestToken": format!("{name:-<32}") }),
                )
                .await;
            assert_eq!(
                removed.0, 200,
                "case {case:?}: the delete must succeed, got {removed:?}"
            );
        }
        let expected_arn = arn(region, "pipeline", name);
        let identifier = if by_arn {
            expected_arn.clone()
        } else {
            name.to_string()
        };
        let mut body = Map::new();
        body.insert("PipelineName".to_string(), json!(identifier));
        if operation == "DeletePipeline" {
            body.insert("ClientRequestToken".to_string(), json!("probe---------------------------"));
        }

        let (status, answered) = emulator
            .call_in(region, operation, &Value::Object(body))
            .await;
        assert_eq!(
            (status, error_code(&answered).as_str()),
            (400, "ResourceNotFound"),
            "case {case:?}: expected 400 ResourceNotFound, got {status} {answered:?}"
        );
        assert_eq!(
            message(&answered),
            format!("Pipeline '{expected_arn}' does not exist."),
            "case {case:?}: expected the quoted ARN, got {:?}",
            message(&answered)
        );
    }
}

/// RULE: the hosting operations refuse a reference to a model, an endpoint
/// configuration or an endpoint the account does not hold with
/// `Could not find <kind> "<identifier>".`, quoting the identifier the REQUEST carried —
/// never an ARN the service synthesised from it.
#[tokio::test]
async fn a_missing_hosting_resource_is_quoted_as_the_request_named_it() {
    // Rows differ along the operation, which resource kind is missing, and the name's
    // shape — including a name that merely differs in case from one that exists.
    let rows: [(&str, &str, &str, &str); 8] = [
        ("read a model", "DescribeModel", "model", "absent-model"),
        ("delete a model", "DeleteModel", "model", "Absent-Model"),
        (
            "read a configuration",
            "DescribeEndpointConfig",
            "endpoint configuration",
            "absent-config",
        ),
        (
            "delete a configuration",
            "DeleteEndpointConfig",
            "endpoint configuration",
            "a",
        ),
        ("read an endpoint", "DescribeEndpoint", "endpoint", "absent-endpoint"),
        ("delete an endpoint", "DeleteEndpoint", "endpoint", "x9"),
        (
            "create an endpoint on a missing configuration",
            "CreateEndpoint",
            "endpoint configuration",
            "absent-config",
        ),
        (
            "create a configuration on a missing model",
            "CreateEndpointConfig",
            "model",
            "absent-model",
        ),
    ];

    for (case, operation, kind, name) in rows {
        let emulator = Emulator::new();
        // A resource whose name differs only in case exists throughout, so the refusal
        // is about the identifier as sent rather than about an empty store.
        emulator
            .ok(
                "CreateModel",
                &json!({ "ModelName": "present", "PrimaryContainer": primary_container("i:1") }),
            )
            .await;
        emulator
            .ok("CreateEndpointConfig", &one_variant_config("present-cfg", "present"))
            .await;

        let body = match operation {
            "DescribeModel" | "DeleteModel" => json!({ "ModelName": name }),
            "DescribeEndpointConfig" | "DeleteEndpointConfig" => {
                json!({ "EndpointConfigName": name })
            }
            "DescribeEndpoint" | "DeleteEndpoint" => json!({ "EndpointName": name }),
            "CreateEndpoint" => json!({ "EndpointName": "fresh-ep", "EndpointConfigName": name }),
            _ => one_variant_config("fresh-cfg", name),
        };
        let (status, refused) = emulator.call(operation, &body).await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected 400 ValidationException, got {status} {refused:?}"
        );
        assert_eq!(
            message(&refused),
            format!("Could not find {kind} \"{name}\"."),
            "case {case:?}: expected the identifier the request carried, got {:?}",
            message(&refused)
        );
    }

    // The same rule on `CreateTransformJob`, whose `ModelName` reference is resolved the
    // same way: a dangling reference to another resource is a property of the REQUEST, so
    // it is refused with the same 400 `ValidationException` the hosting creates answer.
    let emulator = Emulator::new();
    let (status, refused) = emulator
        .call(
            "CreateTransformJob",
            &transform_body("job", "absent-model", "s3://b/in/", "s3://b/out/"),
        )
        .await;
    assert_eq!(
        (status, error_code(&refused).as_str(), message(&refused)),
        (
            400,
            "ValidationException",
            "Could not find model \"absent-model\".".to_string()
        ),
        "case \"a transform job on a missing model\": expected the request's name, \
         got {status} {refused:?}"
    );
}

/// RULE: a constraint on a MAP member's VALUES is stated against the MAP: the clause
/// echoes the whole map as the service renders one (`{key=value, …}`, entries in key
/// order), names the map member, and lists the value shape's COMPLETE constraint set —
/// the modelled maximum length, the enforced minimum, then the modelled expression.
#[tokio::test]
async fn a_map_value_constraint_is_stated_against_the_whole_map() {
    let emulator = Emulator::new();

    // The constraint set a property value must satisfy, as the service lists it.
    // `smithy_query(query="shape",shape="ArtifactPropertyValue")`: `@length 0..4096`,
    // `@pattern .*`; the minimum of 1 is the service's own.
    let requirements = "Map value must satisfy constraint: [Member must have length less \
                        than or equal to 4096, Member must have length greater than or \
                        equal to 1, Member must satisfy regular expression pattern: .*]";

    // A 30-entry map (the modelled maximum) with one empty value, so the rendering is
    // exercised at the collection's own bound too.
    let mut full_map = Map::new();
    let mut full_rendered: Vec<String> = Vec::new();
    for index in 0..30 {
        let key = format!("key-{index:02}");
        let value = if index == 17 {
            String::new()
        } else {
            format!("value-{index}")
        };
        full_rendered.push(format!("{key}={value}"));
        full_map.insert(key, Value::String(value));
    }

    // Rows differ along entry count, which entry is empty, the order the entries arrive
    // in, and the keys they are filed under.
    let rows: Vec<(&str, Value, String)> = vec![
        ("one entry", json!({ "stage": "" }), "{stage=}".to_string()),
        (
            "the empty value beside filled ones",
            json!({ "empty_prop": "", "present_prop": "some-value" }),
            "{empty_prop=, present_prop=some-value}".to_string(),
        ),
        (
            "the same two entries, sent in the other order",
            json!({ "present_prop": "some-value", "empty_prop": "" }),
            "{empty_prop=, present_prop=some-value}".to_string(),
        ),
        (
            "an empty key and an empty value",
            json!({ "": "" }),
            "{=}".to_string(),
        ),
        (
            "three entries, one empty",
            json!({ "rows": "", "stage": "train", "owner": "ml" }),
            "{owner=ml, rows=, stage=train}".to_string(),
        ),
        (
            "a full 30-entry map",
            Value::Object(full_map),
            format!("{{{}}}", full_rendered.join(", ")),
        ),
    ];

    for (case, properties, rendered) in rows {
        let (status, refused) = emulator
            .call(
                "CreateArtifact",
                &json!({
                    "Source": { "SourceUri": format!("s3://bucket/{}.csv", rendered.len()) },
                    "ArtifactType": "DataSet",
                    "Properties": properties,
                }),
            )
            .await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected 400 ValidationException, got {status} {refused:?}"
        );
        assert_eq!(
            message(&refused),
            one_violation(&clause(&rendered, "properties", requirements)),
            "case {case:?}: expected the whole map and the whole constraint set, got {:?}",
            message(&refused)
        );
    }
}

/// RULE: an enum value set is listed in the order the SERVICE declares the enum, which
/// for `ModelApprovalStatus` is `[PendingManualApproval, Approved, Rejected]` — the same
/// order whichever operation carries the member, and whatever the rejected value was.
#[tokio::test]
async fn an_enum_value_set_is_listed_in_the_services_declared_order() {
    let emulator = Emulator::new();
    emulator
        .ok("CreateModelPackageGroup", &json!({ "ModelPackageGroupName": "enum-grp" }))
        .await;
    let requirement =
        "Member must satisfy enum value set: [PendingManualApproval, Approved, Rejected]";

    // Rows differ along the operation carrying the member and the rejected value's shape
    // (a plain word, a near miss of a member, a differently-cased member, the empty
    // string).
    let rows: [(&str, &str, Value, &str); 5] = [
        (
            "UpdateModelPackage, a plain word",
            "UpdateModelPackage",
            json!({
                "ModelPackageArn": arn("us-east-1", "model-package", "enum-grp/1"),
                "ModelApprovalStatus": "Maybe",
            }),
            "modelApprovalStatus",
        ),
        (
            "UpdateModelPackage, a near miss",
            "UpdateModelPackage",
            json!({
                "ModelPackageArn": arn("us-east-1", "model-package", "enum-grp/1"),
                "ModelApprovalStatus": "Approve",
            }),
            "modelApprovalStatus",
        ),
        (
            "UpdateModelPackage, a cased member",
            "UpdateModelPackage",
            json!({
                "ModelPackageArn": arn("us-east-1", "model-package", "enum-grp/1"),
                "ModelApprovalStatus": "approved",
            }),
            "modelApprovalStatus",
        ),
        (
            "UpdateModelPackage, the empty string",
            "UpdateModelPackage",
            json!({
                "ModelPackageArn": arn("us-east-1", "model-package", "enum-grp/1"),
                "ModelApprovalStatus": "",
            }),
            "modelApprovalStatus",
        ),
        (
            "CreateModelPackage carries the same member",
            "CreateModelPackage",
            json!({ "ModelPackageGroupName": "enum-grp", "ModelApprovalStatus": "Maybe" }),
            "modelApprovalStatus",
        ),
    ];

    for (case, operation, body, path) in rows {
        let rejected = body
            .get("ModelApprovalStatus")
            .and_then(Value::as_str)
            .expect("every row carries a ModelApprovalStatus")
            .to_string();
        let (status, refused) = emulator.call(operation, &body).await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected 400 ValidationException, got {status} {refused:?}"
        );
        assert_eq!(
            message(&refused),
            one_violation(&clause(&rejected, path, requirement)),
            "case {case:?}: expected the service's declared order, got {:?}",
            message(&refused)
        );
    }
}

/// RULE: a `@pattern` is reported WITHOUT the `^`/`$` anchors the model spells it with —
/// the requirement stated is the expression a value must satisfy, not the anchoring that
/// makes the match total — whatever member and whatever operation it is on.
#[tokio::test]
async fn a_reported_pattern_carries_no_anchors() {
    // The expressions the model declares, as the service reports them.
    // `smithy_query(query="shape",shape="ExperimentEntityName")`:
    // `^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,119}$`.
    let entity_name = r"[a-zA-Z0-9](-*[a-zA-Z0-9]){0,119}";
    // `smithy_query(query="shape",shape="S3Uri")`: `^(https|s3)://([^/]+)/?(.*)$`.
    let s3_uri = r"(https|s3)://([^/]+)/?(.*)";
    // `smithy_query(query="shape",shape="EntityName")`:
    // `^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,62}$` — the target of `CreateEndpoint`'s
    // `EndpointConfigName` and of `CreateModelPackageGroup`'s `ModelPackageGroupName`.
    let entity_name_63 = r"[a-zA-Z0-9](-*[a-zA-Z0-9]){0,62}";

    let emulator = Emulator::new();
    emulator
        .ok(
            "CreateModel",
            &json!({ "ModelName": "pat", "PrimaryContainer": primary_container("img:1") }),
        )
        .await;

    // Rows differ along the operation, the member's depth in the request (a top-level
    // member, one nested in a structure, one nested inside a LIST element), the
    // expression reported, and the rejected value's script — including a non-ASCII one.
    let rows: Vec<(&str, &str, Value, &str, &str, &str)> = vec![
        (
            "a trial component display name",
            "CreateTrialComponent",
            json!({ "TrialComponentName": "tc", "DisplayName": "実験-component" }),
            "実験-component",
            "displayName",
            entity_name,
        ),
        (
            "a transform input S3 URI",
            "CreateTransformJob",
            transform_body("tj1", "pat", "not-a-valid-s3-uri", "s3://b/out/"),
            "not-a-valid-s3-uri",
            "transformInput.dataSource.s3DataSource.s3Uri",
            s3_uri,
        ),
        (
            "a transform output S3 path",
            "CreateTransformJob",
            transform_body("tj2", "pat", "s3://b/in/", "/local/output"),
            "/local/output",
            "transformOutput.s3OutputPath",
            s3_uri,
        ),
        (
            "a training input channel S3 URI",
            "CreateTrainingJob",
            training_body("trn", "not-a-valid-s3-uri"),
            "not-a-valid-s3-uri",
            "inputDataConfig.1.member.dataSource.s3DataSource.s3Uri",
            s3_uri,
        ),
        (
            "an endpoint configuration name on CreateEndpoint",
            "CreateEndpoint",
            json!({ "EndpointName": "ep", "EndpointConfigName": "bad_config!name" }),
            "bad_config!name",
            "endpointConfigName",
            entity_name_63,
        ),
        (
            "a model package group name",
            "CreateModelPackageGroup",
            json!({ "ModelPackageGroupName": "not a group name" }),
            "not a group name",
            "modelPackageGroupName",
            entity_name_63,
        ),
    ];

    for (case, operation, body, rejected, path, reported) in rows {
        let (status, refused) = emulator.call(operation, &body).await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected 400 ValidationException, got {status} {refused:?}"
        );
        assert_eq!(
            message(&refused),
            one_violation(&clause(rejected, path, &pattern_requirement(reported))),
            "case {case:?}: expected the unanchored expression, got {:?}",
            message(&refused)
        );
        assert!(
            !message(&refused).contains("pattern: ^"),
            "case {case:?}: a reported pattern carries no start anchor, got {:?}",
            message(&refused)
        );
    }
}

/// RULE: a value outside the TRAINING instance types is refused with the CONDITION it
/// failed — `Member must be a supported instance type` — rather than with the value set
/// that would satisfy it, for every member targeting that set and wherever in the request
/// it sits; every OTHER value set of this service is still listed in full, the hosting
/// instance types among them.
#[tokio::test]
async fn a_training_instance_type_outside_the_set_states_the_condition_it_failed() {
    let requirement = "Member must be a supported instance type";

    // A `ResourceConfig` naming `instance_type`, with the other members the model allows
    // beside it.
    let resources = |instance_type: &str| {
        json!({ "InstanceType": instance_type, "InstanceCount": 1, "VolumeSizeInGB": 10 })
    };
    let mut tuning = tuning_body("tuning-with-a-bad-type");
    tuning["TrainingJobDefinition"]["ResourceConfig"]["InstanceType"] = json!("ml.m5.largex");

    // Rows differ along the operation, the member's depth in the request (a nested
    // structure, a structure inside a LIST, a list of bare enum values, a structure two
    // levels down), and the rejected value's shape — a type of ANOTHER instance-type set
    // (`smithy_query(query="shape",shape="ProductionVariantInstanceType")` lists
    // `ml.t2.medium`, which the training set
    // (`smithy_query(query="shape",shape="TrainingInstanceType")`) does not), a member of
    // the set in the wrong case, a near miss of one, a word that is no instance type and
    // the empty string.
    let rows: Vec<(&str, &str, Value, &str, &str)> = vec![
        (
            "a type of the HOSTING set",
            "CreateTrainingJob",
            training_resources("trn-hosting-type", resources("ml.t2.medium")),
            "ml.t2.medium",
            "resourceConfig.instanceType",
        ),
        (
            "a member of the set in the wrong case",
            "CreateTrainingJob",
            training_resources("trn-cased", resources("ML.M5.LARGE")),
            "ML.M5.LARGE",
            "resourceConfig.instanceType",
        ),
        (
            "the empty string",
            "CreateTrainingJob",
            training_resources("trn-empty", resources("")),
            "",
            "resourceConfig.instanceType",
        ),
        (
            "an instance group of a heterogeneous cluster",
            "CreateTrainingJob",
            training_resources(
                "trn-group",
                json!({
                    "InstanceGroups": [{
                        "InstanceGroupName": "group-one",
                        "InstanceType": "ml.bogus.xlarge",
                        "InstanceCount": 2,
                    }],
                    "VolumeSizeInGB": 10,
                }),
            ),
            "ml.bogus.xlarge",
            "resourceConfig.instanceGroups.1.member.instanceType",
        ),
        (
            "an algorithm's supported training instance types",
            "CreateAlgorithm",
            algorithm_body("algo-bad-type", json!(["ml.m5.large", "ml.not.a.type"])),
            "ml.not.a.type",
            "trainingSpecification.supportedTrainingInstanceTypes.2.member",
        ),
        (
            "a tuning job's training definition",
            "CreateHyperParameterTuningJob",
            tuning,
            "ml.m5.largex",
            "trainingJobDefinition.resourceConfig.instanceType",
        ),
    ];

    for (case, operation, body, rejected, path) in rows {
        let emulator = Emulator::new();
        let (status, refused) = emulator.call(operation, &body).await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected 400 ValidationException, got {status} {refused:?}"
        );
        assert_eq!(
            message(&refused),
            one_violation(&clause(rejected, path, requirement)),
            "case {case:?}: expected the condition the value failed, got {:?}",
            message(&refused)
        );
    }

    // The contrast: the HOSTING instance types are another value set, and a value outside
    // it is still told the set — so the condition above is a property of the training set
    // and not of every member named `InstanceType`.
    let emulator = Emulator::new();
    let (status, refused) = emulator
        .call(
            "CreateEndpointConfig",
            &json!({
                "EndpointConfigName": "hosting-type-cfg",
                "ProductionVariants": [{
                    "VariantName": "AllTraffic",
                    "ModelName": "any-model",
                    "InitialInstanceCount": 1,
                    "InstanceType": "ml.t2.nano",
                }],
            }),
        )
        .await;
    let stated = message(&refused);
    let requirement_stated = stated
        .rsplit_once(" failed to satisfy constraint: ")
        .map(|(_, tail)| tail.to_string())
        .unwrap_or_default();
    assert_eq!(
        (status, error_code(&refused).as_str()),
        (400, "ValidationException"),
        "case \"a hosting instance type outside its set\": expected 400 \
         ValidationException, got {status} {refused:?}"
    );
    assert!(
        requirement_stated.starts_with("Member must satisfy enum value set: [ml.t2.medium, ")
            && requirement_stated != requirement,
        "case \"a hosting instance type outside its set\": expected the value set to be \
         listed, got {requirement_stated:?}"
    );
}
