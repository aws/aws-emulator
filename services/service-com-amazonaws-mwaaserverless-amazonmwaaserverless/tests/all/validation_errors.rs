// SPDX-License-Identifier: CC0-1.0

//! The modelled `ValidationException` payload for a member value that fails the
//! constraint its model declares.
//!
//! RULE under test: whenever a request member's value violates a constraint the
//! Smithy model declares for it (`@length`, `@pattern`, `@range`, an enum value
//! set), this service answers HTTP 400 `ValidationException` carrying all three
//! modelled members — a top-level `Message` stating what was refused, `Reason` =
//! `fieldValidationFailed`, and a `FieldList` with one entry per offending
//! member naming it and why.
//!
//! The table spans the class the rule generalises over: the operation, the
//! member, where the member sits (top level, inside a nested structure, inside a
//! list or map), and which kind of constraint was broken.
//!
//! The ARN-typed members are NOT here: this service validates the ARNs a request
//! carries with its own validators, which state their own field name and
//! message, so that rule and its table live in `arn_validation_errors`. Every
//! row below therefore holds a well-formed ARN wherever it needs one.
//!
//! Every case drives the real wire path through the shared `Harness` (see
//! `workflow_lifecycle`), so what is asserted is what a client sees.

use service_com_amazonaws_mwaaserverless_amazonmwaaserverless::ops::types::VALIDATION_EXCEPTION_REASON_VALUES;

use crate::workflow_lifecycle::{role_arn, Harness};

/// The region every case signs with, and so the region a valid ARN carries.
///
/// `pub(crate)` so the ARN suite signs and builds ARNs in the same region.
pub(crate) const REGION: &str = "us-east-1";

/// A `WorkflowArn`/`TaggableResourceArn` that satisfies the modelled pattern
/// (`smithy_query(query="shape", shape="WorkflowArn")`). No workflow by this ARN
/// exists, which is deliberate: a well-formed ARN must NOT be refused as a
/// validation failure.
pub(crate) fn valid_workflow_arn() -> String {
    format!("arn:aws:airflow-serverless:{REGION}:000000000000:workflow/demo-0123456789")
}

/// A `WorkflowVersion` that satisfies `^[a-z0-9]{32}$`, length 32..32.
fn valid_workflow_version() -> String {
    "0123456789abcdef0123456789abcdef".to_string()
}

/// A `DefinitionS3Location` whose members carry no modelled constraint, used as
/// the valid filler for the create/update rows.
pub(crate) fn good_definition() -> serde_json::Value {
    serde_json::json!({"Bucket": "defs", "ObjectKey": "dags/v.yaml"})
}

/// An `Environment$Variables` map of one entry, for a row whose variable NAME is the
/// value under test and so cannot be written as a literal key.
fn one_variable(name: &str, value: &str) -> serde_json::Value {
    let mut variables = serde_json::Map::new();
    variables.insert(name.to_string(), serde_json::json!(value));
    serde_json::Value::Object(variables)
}

/// One row of the table: a label, the operation to call, the request body, and
/// the `FieldList` names the answer must carry — exactly those, in any order.
struct Case {
    label: &'static str,
    operation: &'static str,
    body: serde_json::Value,
    fields: Vec<String>,
}

fn case(
    label: &'static str,
    operation: &'static str,
    body: serde_json::Value,
    fields: &[&str],
) -> Case {
    Case {
        label,
        operation,
        body,
        fields: fields.iter().map(|f| (*f).to_string()).collect(),
    }
}

/// Every refusal row. Rows differ along the operation, the member, the member's
/// position in the input tree, and the kind of constraint broken.
fn refusal_cases() -> Vec<Case> {
    let arn = valid_workflow_arn();
    vec![
        // ---- @pattern on a top-level string ----
        case(
            "CreateWorkflow Name starts with a character the pattern excludes",
            "CreateWorkflow",
            serde_json::json!({
                "Name": "-leading-dash",
                "DefinitionS3Location": good_definition(),
                "RoleArn": role_arn(),
            }),
            &["Name"],
        ),
        case(
            "CreateWorkflow ClientToken holds a space, outside \\x21-\\x7E",
            "CreateWorkflow",
            serde_json::json!({
                "Name": "spacedToken",
                "DefinitionS3Location": good_definition(),
                "RoleArn": role_arn(),
                "ClientToken": "has a space",
            }),
            &["ClientToken"],
        ),
        // ---- @length on a top-level string, at each published bound ----
        case(
            "CreateWorkflow Name below NameString's minimum of 1",
            "CreateWorkflow",
            serde_json::json!({
                "Name": "",
                "DefinitionS3Location": good_definition(),
                "RoleArn": role_arn(),
            }),
            &["Name"],
        ),
        case(
            "CreateWorkflow Name one past NameString's maximum of 255",
            "CreateWorkflow",
            serde_json::json!({
                "Name": format!("x{}", "9".repeat(255)),
                "DefinitionS3Location": good_definition(),
                "RoleArn": role_arn(),
            }),
            &["Name"],
        ),
        case(
            "CreateWorkflow Description below DescriptionString's minimum of 1",
            "CreateWorkflow",
            serde_json::json!({
                "Name": "emptyDescription",
                "DefinitionS3Location": good_definition(),
                "RoleArn": role_arn(),
                "Description": "",
            }),
            &["Description"],
        ),
        case(
            "CreateWorkflow TriggerMode one past GenericString's maximum of 255",
            "CreateWorkflow",
            serde_json::json!({
                "Name": "longTrigger",
                "DefinitionS3Location": good_definition(),
                "RoleArn": role_arn(),
                "TriggerMode": "t".repeat(256),
            }),
            &["TriggerMode"],
        ),
        // ---- an enum value set, on a member of a nested structure ----
        case(
            "CreateWorkflow EncryptionConfiguration.Type outside the EncryptionType set",
            "CreateWorkflow",
            serde_json::json!({
                "Name": "badEncryption",
                "DefinitionS3Location": good_definition(),
                "RoleArn": role_arn(),
                "EncryptionConfiguration": {"Type": "SOME_OTHER_KEY"},
            }),
            &["EncryptionConfiguration.type"],
        ),
        // ---- @length on a member reached through a union ----
        case(
            "CreateWorkflow Code.S3Location.Bucket below the minimum of 3",
            "CreateWorkflow",
            serde_json::json!({
                "Name": "shortBucket",
                "DefinitionS3Location": good_definition(),
                "RoleArn": role_arn(),
                "Code": {"S3Location": {"Bucket": "ab", "ObjectKey": "src.zip"}},
            }),
            &["Code.s3Location.bucket"],
        ),
        // ---- @pattern on a list ELEMENT, reported with its index ----
        case(
            "CreateWorkflow NetworkConfiguration.SubnetIds' first element is not a subnet id",
            "CreateWorkflow",
            serde_json::json!({
                "Name": "badSubnet",
                "DefinitionS3Location": good_definition(),
                "RoleArn": role_arn(),
                "NetworkConfiguration": {"SubnetIds": ["not-a-subnet"]},
            }),
            &["NetworkConfiguration.subnetIds.1.member"],
        ),
        case(
            "CreateWorkflow NetworkConfiguration.SecurityGroupIds' second element is not a group id",
            "CreateWorkflow",
            serde_json::json!({
                "Name": "badSecurityGroup",
                "DefinitionS3Location": good_definition(),
                "RoleArn": role_arn(),
                "NetworkConfiguration": {"SecurityGroupIds": ["sg-1", "GROUP"]},
            }),
            &["NetworkConfiguration.securityGroupIds.2.member"],
        ),
        // ---- @length on a map, counted in entries ----
        case(
            "CreateWorkflow Tags one past the Tags maximum of 50 entries",
            "CreateWorkflow",
            serde_json::json!({
                "Name": "tooManyTags",
                "DefinitionS3Location": good_definition(),
                "RoleArn": role_arn(),
                "Tags": (0..51)
                    .map(|i| (format!("k{i}"), serde_json::json!("v")))
                    .collect::<serde_json::Map<String, serde_json::Value>>(),
            }),
            &["Tags"],
        ),
        // ---- several offending members in one request ----
        case(
            "CreateWorkflow with both Name and Description outside their constraints",
            "CreateWorkflow",
            serde_json::json!({
                "Name": "-bad-name",
                "DefinitionS3Location": good_definition(),
                "RoleArn": role_arn(),
                "Description": "",
            }),
            &["Name", "Description"],
        ),
        // ---- the key and the value of a map inside a nested structure ----
        // `EnvironmentVariablesMap` is `@length(0..50)` entries over a key
        // `@length(1..128)` + `^[a-zA-Z][a-zA-Z0-9_]*$` and a value `@length(0..255)`
        // (`smithy_query(query="shape", shape="EnvironmentVariablesMap")`).
        case(
            "CreateWorkflow Environment variable name starts with a digit the pattern excludes",
            "CreateWorkflow",
            serde_json::json!({
                "Name": "badVariableName",
                "DefinitionS3Location": good_definition(),
                "RoleArn": role_arn(),
                "Environment": {"Variables": {"1RETRIES": "2"}},
            }),
            &["Environment.variables"],
        ),
        case(
            "CreateWorkflow Environment variable name one past its maximum of 128",
            "CreateWorkflow",
            serde_json::json!({
                "Name": "longVariableName",
                "DefinitionS3Location": good_definition(),
                "RoleArn": role_arn(),
                "Environment": {"Variables": one_variable(&format!("V{}", "x".repeat(128)), "2")},
            }),
            &["Environment.variables"],
        ),
        case(
            "CreateWorkflow Environment variable value one past its maximum of 255",
            "CreateWorkflow",
            serde_json::json!({
                "Name": "longVariableValue",
                "DefinitionS3Location": good_definition(),
                "RoleArn": role_arn(),
                "Environment": {"Variables": {"RETRIES": "v".repeat(256)}},
            }),
            &["Environment.variables.RETRIES"],
        ),
        case(
            "CreateWorkflow Environment one past the variables maximum of 50 entries",
            "CreateWorkflow",
            serde_json::json!({
                "Name": "tooManyVariables",
                "DefinitionS3Location": good_definition(),
                "RoleArn": role_arn(),
                "Environment": {"Variables": (0..51)
                    .map(|index| (format!("VAR_{index}"), serde_json::json!("1")))
                    .collect::<serde_json::Map<String, serde_json::Value>>()},
            }),
            &["Environment.variables"],
        ),
        case(
            "UpdateWorkflow Environment variable name is empty, below its minimum of 1",
            "UpdateWorkflow",
            serde_json::json!({
                "WorkflowArn": arn.clone(),
                "DefinitionS3Location": good_definition(),
                "RoleArn": role_arn(),
                "Environment": {"Variables": {"": "2"}},
            }),
            &["Environment.variables"],
        ),
        // ---- the same rule on the other operations ----
        case(
            "GetWorkflow WorkflowVersion is not 32 characters",
            "GetWorkflow",
            serde_json::json!({"WorkflowArn": arn.clone(), "WorkflowVersion": "short"}),
            &["WorkflowVersion"],
        ),
        case(
            "UpdateWorkflow Description is empty, below DescriptionString's minimum of 1",
            "UpdateWorkflow",
            serde_json::json!({
                "WorkflowArn": arn.clone(),
                "DefinitionS3Location": good_definition(),
                "RoleArn": role_arn(),
                "Description": "",
            }),
            &["Description"],
        ),
        case(
            "StartWorkflowRun WorkflowVersion is empty, below VersionId's minimum",
            "StartWorkflowRun",
            serde_json::json!({"WorkflowArn": arn.clone(), "WorkflowVersion": ""}),
            &["WorkflowVersion"],
        ),
        case(
            "StopWorkflowRun RunId breaks the IdString pattern",
            "StopWorkflowRun",
            serde_json::json!({"WorkflowArn": arn.clone(), "RunId": "_bad"}),
            &["RunId"],
        ),
        case(
            "GetWorkflowRun RunId breaks the IdString pattern",
            "GetWorkflowRun",
            serde_json::json!({"WorkflowArn": arn.clone(), "RunId": "-run"}),
            &["RunId"],
        ),
        case(
            "GetTaskInstance TaskInstanceId breaks the IdString pattern",
            "GetTaskInstance",
            serde_json::json!({
                "WorkflowArn": arn.clone(),
                "RunId": "run-1",
                "TaskInstanceId": "/task",
            }),
            &["TaskInstanceId"],
        ),
        case(
            "ListWorkflowRuns WorkflowVersion is empty, below VersionId's minimum",
            "ListWorkflowRuns",
            serde_json::json!({"WorkflowArn": arn.clone(), "WorkflowVersion": ""}),
            &["WorkflowVersion"],
        ),
        case(
            "ListTaskInstances RunId breaks the IdString pattern",
            "ListTaskInstances",
            serde_json::json!({"WorkflowArn": arn.clone(), "RunId": ".run"}),
            &["RunId"],
        ),
        case(
            "TagResource tag value one past TagValue's maximum of 256",
            "TagResource",
            serde_json::json!({
                "ResourceArn": arn.clone(),
                "Tags": {"big": "v".repeat(257)},
            }),
            &["Tags.big"],
        ),
        case(
            "UntagResource TagKeys is empty, below the TagKeys minimum of 1",
            "UntagResource",
            serde_json::json!({"ResourceArn": arn.clone(), "TagKeys": []}),
            &["TagKeys"],
        ),
        case(
            "UntagResource TagKeys' first element is empty, below TagKey's minimum of 1",
            "UntagResource",
            serde_json::json!({"ResourceArn": arn.clone(), "TagKeys": [""]}),
            &["TagKeys.1.member"],
        ),
    ]
}

#[tokio::test]
async fn a_bad_member_value_is_refused_with_the_whole_validation_exception_payload() {
    let harness = Harness::new(REGION);

    for Case {
        label,
        operation,
        body,
        fields,
    } in refusal_cases()
    {
        let (status, answered) = harness.call(operation, body).await;

        assert_eq!(
            status, 400,
            "case {label:?}: expected HTTP 400, got {status} {answered:?}"
        );
        assert_eq!(
            answered["__type"].as_str(),
            Some("ValidationException"),
            "case {label:?}: expected __type ValidationException, got {:?}",
            answered["__type"]
        );

        // `Message` is @required on the error shape, so it must be present,
        // non-empty and within `ErrorMessage`'s modelled length of 1..2048.
        let message = answered["Message"].as_str().unwrap_or_else(|| {
            panic!("case {label:?}: expected a Message member, got {answered:?}")
        });
        assert!(
            (1..=2048).contains(&message.chars().count()),
            "case {label:?}: Message must satisfy ErrorMessage's length 1..2048, got {} \
             characters: {message:?}",
            message.chars().count()
        );

        // `Reason` is @required and, for a value that failed its member's
        // declared constraint, is `fieldValidationFailed`.
        let reason = answered["Reason"].as_str().unwrap_or_else(|| {
            panic!("case {label:?}: expected a Reason member, got {answered:?}")
        });
        assert!(
            VALIDATION_EXCEPTION_REASON_VALUES.contains(&reason),
            "case {label:?}: Reason must be a ValidationExceptionReason value, got {reason:?}"
        );
        assert_eq!(
            reason, "fieldValidationFailed",
            "case {label:?}: expected Reason fieldValidationFailed, got {reason:?}"
        );

        // `FieldList` names every offending member, with a per-field message.
        let field_list = answered["FieldList"].as_array().unwrap_or_else(|| {
            panic!("case {label:?}: expected a FieldList member, got {answered:?}")
        });
        let mut names: Vec<String> = Vec::new();
        for entry in field_list {
            let name = entry["Name"].as_str().unwrap_or_else(|| {
                panic!("case {label:?}: every FieldList entry needs a Name, got {entry:?}")
            });
            let field_message = entry["Message"].as_str().unwrap_or_else(|| {
                panic!("case {label:?}: every FieldList entry needs a Message, got {entry:?}")
            });
            assert!(
                (1..=2048).contains(&field_message.chars().count()),
                "case {label:?}: field {name:?} message must satisfy length 1..2048, got \
                 {field_message:?}"
            );
            // The clause carries the walk's own lowerCamel path, which is the
            // form AWS validation messages use; `Name` restores the member's
            // first segment to the model's spelling, so the two agree up to
            // that one character.
            assert!(
                field_message.to_lowercase().contains(&name.to_lowercase()),
                "case {label:?}: field {name:?} message must name the member it refused, got \
                 {field_message:?}"
            );
            names.push(name.to_string());
        }
        let mut expected = fields.clone();
        expected.sort();
        // One member can break two constraints at once (an empty string fails
        // both its minimum length and its pattern), and the framework's walk
        // accumulates both — so compare the SET of members named.
        names.sort();
        names.dedup();
        assert_eq!(
            names, expected,
            "case {label:?}: FieldList must name exactly the offending members, got \
             {field_list:?}"
        );

        // The top-level Message counts the clauses the FieldList carries and
        // states the same refusals.
        let count_prefix = if field_list.len() == 1 {
            "1 validation error detected".to_string()
        } else {
            format!("{} validation errors detected", field_list.len())
        };
        assert!(
            message.starts_with(&count_prefix),
            "case {label:?}: expected Message to open with {count_prefix:?}, got {message:?}"
        );
        for name in &expected {
            assert!(
                message.to_lowercase().contains(&name.to_lowercase()),
                "case {label:?}: Message must name the offending member {name:?}, got {message:?}"
            );
        }
    }
}

/// An out-of-range `MaxResults` keeps the request-validation layer's own
/// envelope — a 400 whose body carries a lowercase `message` and neither
/// `Reason` nor `FieldList`.
///
/// This is the one modelled constraint of this service NOT answered with the
/// modelled `ValidationException`, and it is pinned here because the recorded
/// AWS answer for an out-of-range page size is exactly this envelope. See the
/// note on request validation in `crate::state`.
#[tokio::test]
async fn an_out_of_range_page_size_keeps_the_request_validation_envelope() {
    let harness = Harness::new(REGION);
    let arn = valid_workflow_arn();

    let cases: Vec<(&str, &str, serde_json::Value)> = vec![
        (
            "ListWorkflows one below the minimum of 1",
            "ListWorkflows",
            serde_json::json!({"MaxResults": 0}),
        ),
        (
            "ListWorkflows one above the maximum of 100",
            "ListWorkflows",
            serde_json::json!({"MaxResults": 101}),
        ),
        (
            "ListWorkflowRuns one above the maximum of 100",
            "ListWorkflowRuns",
            serde_json::json!({"WorkflowArn": arn.clone(), "MaxResults": 101}),
        ),
        (
            "ListWorkflowVersions one below the minimum of 1",
            "ListWorkflowVersions",
            serde_json::json!({"WorkflowArn": arn.clone(), "MaxResults": 0}),
        ),
        (
            "ListTaskInstances one above the maximum of 100",
            "ListTaskInstances",
            serde_json::json!({
                "WorkflowArn": arn.clone(),
                "RunId": "run-01",
                "MaxResults": 101,
            }),
        ),
    ];

    for (label, operation, body) in cases {
        let (status, answered) = harness.call(operation, body).await;
        assert_eq!(
            status, 400,
            "case {label:?}: expected HTTP 400, got {status} {answered:?}"
        );
        assert_eq!(
            answered["__type"].as_str(),
            Some("ValidationException"),
            "case {label:?}: expected __type ValidationException, got {:?}",
            answered["__type"]
        );
        assert!(
            answered["message"].as_str().is_some_and(|m| !m.is_empty()),
            "case {label:?}: expected the request-validation layer's lowercase message, got \
             {answered:?}"
        );
        assert!(
            answered["Reason"].is_null() && answered["FieldList"].is_null(),
            "case {label:?}: the request-validation envelope carries no Reason or FieldList, \
             got {answered:?}"
        );
    }
}

/// A value INSIDE every published bound is not refused. The same table rows that
/// break a bound are run at it, so the validation cannot be a blanket refusal:
/// each case must answer something other than a 400 `ValidationException`.
#[tokio::test]
async fn a_value_within_its_modelled_constraint_is_never_a_validation_failure() {
    let harness = Harness::new(REGION);
    let arn = valid_workflow_arn();

    // 50 tags is the published `Tags` maximum; 255 characters the published
    // `NameString` maximum; 1 and 100 the published `MaxResults` bounds.
    let at_the_bound: Vec<(&str, &str, serde_json::Value)> = vec![
        (
            "CreateWorkflow with a one-character Name, the minimum",
            "CreateWorkflow",
            serde_json::json!({
                "Name": "a",
                "DefinitionS3Location": good_definition(),
                "RoleArn": role_arn(),
            }),
        ),
        (
            "CreateWorkflow with a 255-character Name, the maximum",
            "CreateWorkflow",
            serde_json::json!({
                "Name": format!("x{}", "9".repeat(254)),
                "DefinitionS3Location": good_definition(),
                "RoleArn": role_arn(),
            }),
        ),
        (
            "CreateWorkflow with exactly 50 tags, the maximum",
            "CreateWorkflow",
            serde_json::json!({
                "Name": "fiftyTags",
                "DefinitionS3Location": good_definition(),
                "RoleArn": role_arn(),
                "Tags": (0..50)
                    .map(|i| (format!("k{i}"), serde_json::json!("v")))
                    .collect::<serde_json::Map<String, serde_json::Value>>(),
            }),
        ),
        (
            "CreateWorkflow with a three-character bucket and both id lists",
            "CreateWorkflow",
            serde_json::json!({
                "Name": "boundaryBucket",
                "DefinitionS3Location": good_definition(),
                "RoleArn": role_arn(),
                "Code": {"S3Location": {"Bucket": "abc", "ObjectKey": "s.zip"}},
                "EncryptionConfiguration": {"Type": "AWS_MANAGED_KEY"},
                "NetworkConfiguration": {
                    "SecurityGroupIds": ["sg-0123456789abcdef0"],
                    "SubnetIds": ["subnet-0123456789abcdef0"],
                },
            }),
        ),
        (
            "GetWorkflow with a well-formed ARN and a 32-character version",
            "GetWorkflow",
            serde_json::json!({
                "WorkflowArn": arn.clone(),
                "WorkflowVersion": valid_workflow_version(),
            }),
        ),
        (
            "DeleteWorkflow with a well-formed ARN",
            "DeleteWorkflow",
            serde_json::json!({"WorkflowArn": arn.clone()}),
        ),
        (
            "ListWorkflows at the MaxResults minimum of 1",
            "ListWorkflows",
            serde_json::json!({"MaxResults": 1}),
        ),
        (
            "ListWorkflows at the MaxResults maximum of 100",
            "ListWorkflows",
            serde_json::json!({"MaxResults": 100}),
        ),
        (
            "ListWorkflowRuns with a well-formed ARN at the MaxResults maximum",
            "ListWorkflowRuns",
            serde_json::json!({"WorkflowArn": arn.clone(), "MaxResults": 100}),
        ),
        (
            "TagResource with a well-formed ARN and a 256-character tag value",
            "TagResource",
            serde_json::json!({
                "ResourceArn": arn.clone(),
                "Tags": {"big": "v".repeat(256)},
            }),
        ),
        (
            "UntagResource with a well-formed ARN and one key",
            "UntagResource",
            serde_json::json!({"ResourceArn": arn.clone(), "TagKeys": ["team"]}),
        ),
        (
            "GetWorkflowRun with a well-formed ARN and run id",
            "GetWorkflowRun",
            serde_json::json!({"WorkflowArn": arn.clone(), "RunId": "run-01"}),
        ),
    ];

    for (label, operation, body) in at_the_bound {
        let (status, answered) = harness.call(operation, body).await;
        let is_validation_failure = status == 400
            && answered["__type"]
                .as_str()
                .is_some_and(|wire| wire.contains("ValidationException"));
        assert!(
            !is_validation_failure,
            "case {label:?}: a value inside its modelled constraint must not be refused as a \
             validation failure, got {status} {answered:?}"
        );
    }
}

/// Every re-declared constraint must be the model's own, for the member the
/// handler actually passes it for.
///
/// `skip_validation` strips the model's `@length`/`@pattern`/value set off the
/// generated member schemas, so the shadow member schemas in `crate::state` are
/// the only thing carrying them into the framework's walk. Each row below is the
/// schema a handler hands the walk for ONE operation input member, checked
/// against what the model declares for THAT operation's member — so a bound that
/// drifts and a member that is retargeted in the model both fail here.
#[test]
fn every_shadow_member_schema_carries_the_model_s_own_constraints() {
    use service_com_amazonaws_mwaaserverless_amazonmwaaserverless::state;

    let model = smithy_model();

    // (operation, member, the shadow schema that operation's handler passes)
    let operation_members: Vec<(&str, &str, &::aws_smithy_schema::Schema)> = vec![
        ("CreateWorkflow", "Name", &state::M_NAME),
        ("CreateWorkflow", "ClientToken", &state::M_CLIENT_TOKEN),
        ("CreateWorkflow", "RoleArn", &state::M_ROLE_ARN),
        ("CreateWorkflow", "Description", &state::M_DESCRIPTION),
        ("CreateWorkflow", "TriggerMode", &state::M_TRIGGER_MODE),
        ("CreateWorkflow", "Tags", &state::M_TAGS),
        ("UpdateWorkflow", "WorkflowArn", &state::M_WORKFLOW_ARN),
        ("UpdateWorkflow", "RoleArn", &state::M_ROLE_ARN),
        ("UpdateWorkflow", "Description", &state::M_DESCRIPTION),
        ("UpdateWorkflow", "TriggerMode", &state::M_TRIGGER_MODE),
        ("GetWorkflow", "WorkflowArn", &state::M_WORKFLOW_ARN),
        (
            "GetWorkflow",
            "WorkflowVersion",
            &state::M_WORKFLOW_VERSION_ID,
        ),
        ("DeleteWorkflow", "WorkflowArn", &state::M_WORKFLOW_ARN),
        (
            "DeleteWorkflow",
            "WorkflowVersion",
            &state::M_WORKFLOW_VERSION_ID,
        ),
        ("GetWorkflowRun", "WorkflowArn", &state::M_WORKFLOW_ARN),
        ("GetWorkflowRun", "RunId", &state::M_RUN_ID),
        ("GetTaskInstance", "WorkflowArn", &state::M_WORKFLOW_ARN),
        ("GetTaskInstance", "RunId", &state::M_RUN_ID),
        (
            "GetTaskInstance",
            "TaskInstanceId",
            &state::M_TASK_INSTANCE_ID,
        ),
        ("StopWorkflowRun", "WorkflowArn", &state::M_WORKFLOW_ARN),
        ("StopWorkflowRun", "RunId", &state::M_RUN_ID),
        ("StartWorkflowRun", "WorkflowArn", &state::M_WORKFLOW_ARN),
        ("StartWorkflowRun", "ClientToken", &state::M_CLIENT_TOKEN),
        (
            "StartWorkflowRun",
            "WorkflowVersion",
            &state::M_WORKFLOW_VERSION_SELECTOR,
        ),
        ("ListWorkflowRuns", "WorkflowArn", &state::M_WORKFLOW_ARN),
        (
            "ListWorkflowRuns",
            "WorkflowVersion",
            &state::M_WORKFLOW_VERSION_SELECTOR,
        ),
        ("ListWorkflowVersions", "WorkflowArn", &state::M_WORKFLOW_ARN),
        ("ListTaskInstances", "WorkflowArn", &state::M_WORKFLOW_ARN),
        ("ListTaskInstances", "RunId", &state::M_RUN_ID),
        ("ListTagsForResource", "ResourceArn", &state::M_RESOURCE_ARN),
        ("TagResource", "ResourceArn", &state::M_RESOURCE_ARN),
        ("TagResource", "Tags", &state::M_TAGS),
        ("UntagResource", "ResourceArn", &state::M_RESOURCE_ARN),
        ("UntagResource", "TagKeys", &state::M_TAG_KEYS),
    ];
    for (operation, member, schema) in operation_members {
        let request = request_shape(&model, operation);
        let label = format!("{operation}${member}");
        assert_shadow_matches_model(&model, &label, &request, member, schema);
    }

    // The nested shapes a handler reaches through `Code`,
    // `EncryptionConfiguration` and `NetworkConfiguration`.
    let nested: Vec<(&str, &str, &::aws_smithy_schema::Schema)> = vec![
        ("S3Location", "Bucket", &state::M_BUCKET),
        ("S3Location", "ObjectKey", &state::M_OBJECT_KEY),
        ("S3Location", "VersionId", &state::M_S3_VERSION_ID),
        ("EncryptionConfiguration", "Type", &state::M_ENCRYPTION_TYPE),
        (
            "NetworkConfiguration",
            "SecurityGroupIds",
            &state::M_SECURITY_GROUP_IDS,
        ),
        ("NetworkConfiguration", "SubnetIds", &state::M_SUBNET_IDS),
        (
            "Environment",
            "Variables",
            &state::M_ENVIRONMENT_VARIABLES,
        ),
    ];
    for (shape, member, schema) in nested {
        let shape_id = format!("com.amazonaws.mwaaserverless#{shape}");
        let label = format!("{shape}${member}");
        assert_shadow_matches_model(&model, &label, &shape_id, member, schema);
    }
}

/// This service's Smithy model, the source every shadow schema is checked
/// against.
///
/// `pub(crate)` so another suite that must read what the model declares (an
/// operation's own error set, say) reads the same file rather than loading a
/// second copy of it.
pub(crate) fn smithy_model() -> serde_json::Value {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("smithy-model/com-amazonaws-mwaaserverless-amazonmwaaserverless.json");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|why| panic!("read the Smithy model at {}: {why}", path.display()));
    serde_json::from_str(&text).expect("the Smithy model parses as JSON")
}

/// The input shape id of `operation`.
fn request_shape(model: &serde_json::Value, operation: &str) -> String {
    let id = format!("com.amazonaws.mwaaserverless#{operation}");
    model["shapes"][&id]["input"]["target"]
        .as_str()
        .unwrap_or_else(|| panic!("the model declares no input shape for {id}"))
        .to_string()
}

/// The constraints of one member, in a form both the model and a shadow schema
/// can be read into.
#[derive(Debug, Default, PartialEq, Eq)]
struct Constraints {
    /// `@length` minimum (characters for a string, entries for a collection).
    min: Option<i64>,
    /// `@length` maximum.
    max: Option<i64>,
    /// `@pattern`, verbatim.
    pattern: Option<String>,
    /// An enum shape's value set, sorted so declaration order cannot matter.
    enum_values: Option<Vec<String>>,
    /// A collection element's (or map value's) own constraints.
    element: Option<Box<Constraints>>,
}

impl Constraints {
    fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

fn assert_shadow_matches_model(
    model: &serde_json::Value,
    label: &str,
    shape_id: &str,
    member: &str,
    schema: &::aws_smithy_schema::Schema,
) {
    let declared = model_constraints(model, shape_id, member);
    let carried = schema_constraints(schema);
    assert!(
        !declared.is_empty(),
        "case {label:?}: the model declares no constraint for this member, so no shadow \
         schema should carry one"
    );
    assert_eq!(
        carried, declared,
        "case {label:?}: the shadow member schema must carry the model's own constraints \
         {declared:?}, got {carried:?}"
    );
    assert_eq!(
        schema.member_name(),
        Some(member),
        "case {label:?}: the shadow member schema must be named for the member it stands \
         for, got {:?}",
        schema.member_name()
    );
}

/// What the model declares for `shape_id$member`, resolved the way codegen
/// resolves it: the member's own traits first, then the traits of the shape it
/// targets, then one descent into a list element or map value.
fn model_constraints(model: &serde_json::Value, shape_id: &str, member: &str) -> Constraints {
    let member_node = &model["shapes"][shape_id]["members"][member];
    assert!(
        !member_node.is_null(),
        "the model declares no member {shape_id}${member}"
    );
    let target = member_node["target"].as_str().unwrap_or_default();
    let mut out = leaf_constraints(model, member_node.get("traits"), target);
    out.element = match collection_element_target(model, target) {
        Some(element) => {
            let inner = leaf_constraints(
                model,
                element.get("traits"),
                element["target"].as_str().unwrap_or_default(),
            );
            (!inner.is_empty()).then(|| Box::new(inner))
        }
        None => None,
    };
    out
}

/// `@length`/`@pattern`/value set from a member's own traits, falling back to
/// the traits (and enum membership) of the shape it targets.
fn leaf_constraints(
    model: &serde_json::Value,
    member_traits: Option<&serde_json::Value>,
    target: &str,
) -> Constraints {
    let target_node = &model["shapes"][target];
    let mut out = Constraints::default();
    let mut sources: Vec<&serde_json::Value> = Vec::new();
    if let Some(traits) = member_traits {
        sources.push(traits);
    }
    if let Some(traits) = target_node.get("traits") {
        sources.push(traits);
    }
    let mut length_seen = false;
    for traits in sources {
        if !length_seen {
            if let Some(length) = traits.get("smithy.api#length") {
                out.min = length.get("min").and_then(serde_json::Value::as_i64);
                out.max = length.get("max").and_then(serde_json::Value::as_i64);
                length_seen = true;
            }
        }
        if out.pattern.is_none() {
            out.pattern = traits
                .get("smithy.api#pattern")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string);
        }
    }
    if target_node["type"].as_str() == Some("enum") {
        if let Some(members) = target_node["members"].as_object() {
            let mut values: Vec<String> = members
                .iter()
                .map(|(name, member)| {
                    member["traits"]["smithy.api#enumValue"]
                        .as_str()
                        .unwrap_or(name)
                        .to_string()
                })
                .collect();
            values.sort();
            out.enum_values = Some(values);
        }
    }
    out
}

/// The element node of a list target, or the value node of a map target.
fn collection_element_target<'a>(
    model: &'a serde_json::Value,
    target: &str,
) -> Option<&'a serde_json::Value> {
    let node = &model["shapes"][target];
    match node["type"].as_str() {
        Some("list") => Some(&node["member"]),
        Some("map") => Some(&node["value"]),
        _ => None,
    }
}

/// What a shadow member schema carries, read back through the framework's own
/// public constraint traits.
fn schema_constraints(schema: &::aws_smithy_schema::Schema) -> Constraints {
    use ::aws_smithy_schema_server::constraints::ElementConstraintsTrait;

    let mut out = trait_map_constraints(schema.traits());
    let element = schema
        .traits()
        .and_then(|traits| traits.get(&ElementConstraintsTrait::TRAIT_ID))
        .and_then(|found| found.as_any().downcast_ref::<ElementConstraintsTrait>())
        .map(|wrapper| trait_map_constraints(Some(wrapper.inner())));
    out.element = element.filter(|inner| !inner.is_empty()).map(Box::new);
    out
}

fn trait_map_constraints(traits: Option<&::aws_smithy_schema::TraitMap>) -> Constraints {
    use ::aws_smithy_schema_server::constraints::{EnumTrait, LengthTrait, PatternTrait};

    let mut out = Constraints::default();
    let Some(traits) = traits else {
        return out;
    };
    if let Some(found) = traits.get(&LengthTrait::TRAIT_ID) {
        if let Some(length) = found.as_any().downcast_ref::<LengthTrait>() {
            out.min = length.min;
            out.max = length.max;
        }
    }
    if let Some(found) = traits.get(&PatternTrait::TRAIT_ID) {
        if let Some(pattern) = found.as_any().downcast_ref::<PatternTrait>() {
            out.pattern = Some(pattern.regex.to_string());
        }
    }
    if let Some(found) = traits.get(&EnumTrait::TRAIT_ID) {
        if let Some(values) = found.as_any().downcast_ref::<EnumTrait>() {
            let mut values: Vec<String> =
                values.values.iter().map(|v| (*v).to_string()).collect();
            values.sort();
            out.enum_values = Some(values);
        }
    }
    out
}

/// The refusal replaces nothing else: a well-formed ARN naming no stored
/// workflow is a lookup miss, not a validation failure.
#[tokio::test]
async fn a_well_formed_request_keeps_the_answer_it_had() {
    let harness = Harness::new(REGION);
    let arn = valid_workflow_arn();

    // Implemented operations: a well-formed ARN for a workflow that does not
    // exist is `ResourceNotFoundException`, at whatever status this service
    // answers a lookup miss with (`crate::state::LOOKUP_MISS_STATUS`).
    let implemented_cases: Vec<(&str, serde_json::Value)> = vec![
        ("GetWorkflow", serde_json::json!({"WorkflowArn": arn.clone()})),
        (
            "DeleteWorkflow",
            serde_json::json!({"WorkflowArn": arn.clone()}),
        ),
        (
            "StartWorkflowRun",
            serde_json::json!({"WorkflowArn": arn.clone()}),
        ),
        (
            "GetTaskInstance",
            serde_json::json!({
                "WorkflowArn": arn.clone(),
                "RunId": "run-01",
                "TaskInstanceId": "task-01",
            }),
        ),
        // The ARN member an operation resolves the resource through is not always
        // named `WorkflowArn`: the tagging operations name it `ResourceArn`.
        (
            "ListTagsForResource",
            serde_json::json!({"ResourceArn": arn.clone()}),
        ),
        (
            "UntagResource",
            serde_json::json!({"ResourceArn": arn.clone(), "TagKeys": ["team"]}),
        ),
        (
            "TagResource",
            serde_json::json!({"ResourceArn": arn.clone(), "Tags": {"team": "data"}}),
        ),
        // A run of a workflow this endpoint does not hold is a lookup miss too: the
        // stop resolves the run, and a workflow that is absent holds none.
        (
            "StopWorkflowRun",
            serde_json::json!({"WorkflowArn": arn.clone(), "RunId": "run-01"}),
        ),
    ];
    for (operation, body) in implemented_cases {
        let (status, answered) = harness.call(operation, body).await;
        assert_eq!(
            answered["__type"].as_str(),
            Some("ResourceNotFoundException"),
            "case {operation:?}: a well-formed but unknown ARN must be a lookup miss, not a \
             validation failure, got {status} {answered:?}"
        );
    }

    // A paginated child listing declares no not-found error of its own
    // (`smithy_query(query="operation",
    // operation="ListWorkflowRuns,ListWorkflowVersions,ListTaskInstances")`), so a
    // well-formed ARN naming no stored workflow is the empty page it models — still not a
    // validation failure. `child_listing_error_set` and `task_instance_listing` carry that
    // rule in full.
    let listing_cases: Vec<(&str, serde_json::Value, &str)> = vec![
        (
            "ListWorkflowRuns",
            serde_json::json!({"WorkflowArn": arn.clone()}),
            "WorkflowRuns",
        ),
        (
            "ListWorkflowVersions",
            serde_json::json!({"WorkflowArn": arn.clone()}),
            "WorkflowVersions",
        ),
        (
            "ListTaskInstances",
            serde_json::json!({"WorkflowArn": arn.clone(), "RunId": "run-01"}),
            "TaskInstances",
        ),
    ];
    for (operation, body, items) in listing_cases {
        let (status, answered) = harness.call(operation, body).await;
        assert_eq!(
            status, 200,
            "case {operation:?}: a well-formed but unknown ARN must be an empty page, not a \
             validation failure, got {status} {answered:?}"
        );
        assert_eq!(
            answered[items].as_array().map(Vec::len),
            Some(0),
            "case {operation:?}: expected an empty {items} list, got {:?}",
            answered[items]
        );
    }
}

/// RULE: a member whose shape ENUMERATES the values it may carry admits only those
/// values, so a request naming another one is refused with the modelled
/// `ValidationException` and nothing is created or appended.
///
/// `EngineVersion` is the member this suite drives it through: it targets an `intEnum`
/// whose only defined value is `1`
/// (`smithy_query(query="shape", shape="EngineVersion")` — member `ONE`), and it reaches
/// a handler as a plain integer no `@length`, `@pattern` or `@range` judges, so the
/// membership is the service's own check. The rows differ along the VALUE: the one value
/// the shape defines, the member omitted altogether, each neighbour of the defined value,
/// zero, a negative, and each end of the modelled `Integer`.
#[tokio::test]
async fn an_engine_version_outside_the_values_the_model_defines_is_refused() {
    use service_com_amazonaws_mwaaserverless_amazonmwaaserverless::state::{
        ENGINE_VERSION_MEMBER, ENGINE_VERSION_VALUES, INVALID_ENGINE_VERSION_MESSAGE,
    };

    let harness = Harness::new(REGION);

    // `None` is the member omitted entirely; every other row names a value.
    let rows: Vec<(&str, Option<i64>)> = vec![
        ("the one value the shape defines", Some(1)),
        ("the member omitted altogether", None),
        ("one past the defined value", Some(2)),
        ("one below the defined value", Some(0)),
        ("a version far past the defined one", Some(999)),
        ("a negative version", Some(-1)),
        ("the largest value the modelled Integer holds", Some(i64::from(i32::MAX))),
        ("the smallest value the modelled Integer holds", Some(i64::from(i32::MIN))),
    ];

    for (index, (case, engine_version)) in rows.iter().enumerate() {
        let defined = engine_version
            .is_none_or(|version| ENGINE_VERSION_VALUES.iter().any(|&one| i64::from(one) == version));
        let name = format!("engineVersion{index}");
        let mut body = serde_json::json!({
            "Name": name,
            "DefinitionS3Location": good_definition(),
            "RoleArn": role_arn(),
        });
        if let Some(version) = engine_version {
            body[ENGINE_VERSION_MEMBER] = serde_json::json!(version);
        }
        let (status, answered) = harness.call("CreateWorkflow", body.clone()).await;

        if defined {
            assert_eq!(
                status, 200,
                "case {case:?}: a value the shape defines must be accepted, got {status} \
                 {answered:?}"
            );
            // ROUND TRIP: the created workflow is ready and reports the member it was
            // created with.
            let arn = answered["WorkflowArn"].as_str().unwrap_or_default();
            let (status, reported) = harness
                .call("GetWorkflow", serde_json::json!({"WorkflowArn": arn}))
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected GetWorkflow to succeed, got {status} {reported:?}"
            );
            assert_eq!(
                reported["WorkflowStatus"].as_str(),
                Some("READY"),
                "case {case:?}: expected the created workflow to be READY, got {:?}",
                reported["WorkflowStatus"]
            );
            assert_eq!(
                reported[ENGINE_VERSION_MEMBER].as_i64(),
                *engine_version,
                "case {case:?}: expected the stored engine version to be {engine_version:?}, \
                 got {:?}",
                reported[ENGINE_VERSION_MEMBER]
            );
            continue;
        }

        assert_eq!(
            status, 400,
            "case {case:?}: expected the refusal at 400, got {status} {answered:?}"
        );
        assert!(
            answered["__type"]
                .as_str()
                .is_some_and(|kind| kind.contains("ValidationException")),
            "case {case:?}: expected a ValidationException, got {:?}",
            answered["__type"]
        );
        assert_eq!(
            answered["Message"].as_str(),
            Some(INVALID_ENGINE_VERSION_MESSAGE),
            "case {case:?}: expected the fixed sentence for an engine version the service \
             has no engine for, got {:?}",
            answered["Message"]
        );
        assert_eq!(
            answered["Reason"].as_str(),
            Some("fieldValidationFailed"),
            "case {case:?}: expected the field-validation reason, got {:?}",
            answered["Reason"]
        );
        let named: Vec<&str> = answered["FieldList"]
            .as_array()
            .map(|fields| {
                fields
                    .iter()
                    .filter_map(|field| field["Name"].as_str())
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            named,
            vec![ENGINE_VERSION_MEMBER],
            "case {case:?}: expected the member at fault to be named, got {named:?}"
        );

        // Nothing was minted: the name the refused create asked for is held by no
        // workflow, so a later create may still use it.
        let (status, listed) = harness
            .call("ListWorkflows", serde_json::json!({"MaxResults": 100}))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected ListWorkflows to succeed, got {status} {listed:?}"
        );
        let held: Vec<&str> = listed["Workflows"]
            .as_array()
            .map(|entries| {
                entries
                    .iter()
                    .filter_map(|summary| summary["Name"].as_str())
                    .collect()
            })
            .unwrap_or_default();
        assert!(
            !held.contains(&name.as_str()),
            "case {case:?}: a refused create must mint no workflow, got {held:?}"
        );
    }
}

/// RULE, at the other operation that accepts the member: an `UpdateWorkflow` naming an
/// engine version the model does not define is refused the same way, and appends no
/// version.
#[tokio::test]
async fn an_update_naming_an_undefined_engine_version_appends_no_version() {
    use service_com_amazonaws_mwaaserverless_amazonmwaaserverless::state::{
        ENGINE_VERSION_MEMBER, INVALID_ENGINE_VERSION_MESSAGE,
    };

    let harness = Harness::new(REGION);
    let (status, created) = harness
        .call(
            "CreateWorkflow",
            serde_json::json!({
                "Name": "updatedEngineVersion",
                "DefinitionS3Location": good_definition(),
                "RoleArn": role_arn(),
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "setup: expected CreateWorkflow to succeed, got {status} {created:?}"
    );
    let arn = created["WorkflowArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let before = harness.stored_versions(&arn);

    // Rows differ along the value, as above; the one defined value is the control that
    // DOES append a version.
    for (case, engine_version, refused) in [
        ("a version the model does not define", 7, true),
        ("a negative version", -2, true),
        ("the one value the shape defines", 1, false),
    ] {
        let (status, answered) = harness
            .call(
                "UpdateWorkflow",
                serde_json::json!({
                    "WorkflowArn": arn.clone(),
                    "DefinitionS3Location": good_definition(),
                    "RoleArn": role_arn(),
                    ENGINE_VERSION_MEMBER: engine_version,
                }),
            )
            .await;
        if !refused {
            assert_eq!(
                status, 200,
                "case {case:?}: a value the shape defines must be accepted, got {status} \
                 {answered:?}"
            );
            assert_eq!(
                harness.stored_versions(&arn).len(),
                before.len() + 1,
                "case {case:?}: an accepted update appends one version"
            );
            continue;
        }
        assert_eq!(
            status, 400,
            "case {case:?}: expected the refusal at 400, got {status} {answered:?}"
        );
        assert_eq!(
            answered["Message"].as_str(),
            Some(INVALID_ENGINE_VERSION_MESSAGE),
            "case {case:?}: expected the same fixed sentence the create is refused with, \
             got {:?}",
            answered["Message"]
        );
        assert_eq!(
            harness.stored_versions(&arn),
            before,
            "case {case:?}: a refused update must append no version"
        );
    }
}
