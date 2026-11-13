// SPDX-License-Identifier: CC0-1.0

//! `GetTemplateSummary`, end to end over the awsQuery wire.
//!
//! The rule under test: **what `GetTemplateSummary` answers is computed from ONE template —
//! the body the request carries, or the template the stack it names holds — and reports
//! what that template DECLARES**: its `AWSTemplateFormatVersion`, its `Description`, its
//! `Metadata` section, one `ParameterDeclaration` per declared parameter (its type, its
//! `Default`, its `NoEcho`, its `Description` and the `AllowedValues` it permits), the
//! capabilities its resources require with the reason naming them, each resource type it
//! defines without repetition, and the transforms it declares. It writes nothing, so a
//! second caller sees exactly what the first did.
//!
//! The rows span the class the rule generalises over rather than one reported body: both
//! template spellings (JSON and YAML, long- and short-form intrinsics), a declared format
//! version and none, a `Description` and none, a `Metadata` section of nested objects and
//! of a list, parameter counts of zero, one and three, each spelling of a declaration
//! member (a string default, a numeric one, `NoEcho` as a boolean and as text, string and
//! numeric `AllowedValues`, a declaration carrying no optional member), an IAM resource
//! with a custom name, one resource type and four, the same type declared twice, the S3
//! bucket variants (`AWS::S3::Bucket`, `AWS::S3Tables::TableBucket` and both S3 Vectors
//! types), and each spelling of the `Transform` section.
//!
//! The stack half of the rule is the round trip: a template is deployed through
//! `CreateStack`, and the summary read back for that stack reports the same resource types
//! the stack's own `DescribeStackResources` reports — a summary of the template that stack
//! deployed rather than of a document nobody holds.

use serde_json::json;
use smithy_server_runtime::MultiServiceRouter;

use crate::stack_lifecycle::{
    call, deploy, list_members, member, outside_parameters, reason_resource_types, router,
    router_with_engine, store, xml_text, xml_unescape, CAPABILITIES_REASON_PREFIX,
};
use crate::stack_resources::{resources_of, wire_resources};

/// One `ParameterDeclaration` as the response carries it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct WireParameter {
    key: Option<String>,
    parameter_type: Option<String>,
    default_value: Option<String>,
    /// Read as text, because that is what the XML envelope carries (`true`/`false`).
    no_echo: Option<String>,
    description: Option<String>,
    /// `ParameterConstraints/AllowedValues`, empty for a declaration that permits any value
    /// of its type.
    allowed_values: Vec<String>,
}

/// A parameter row, spelled as briefly as a table row can be.
fn parameter(
    key: &str,
    parameter_type: &str,
    default_value: Option<&str>,
    no_echo: bool,
    description: Option<&str>,
    allowed_values: &[&str],
) -> WireParameter {
    WireParameter {
        key: Some(key.to_string()),
        parameter_type: Some(parameter_type.to_string()),
        default_value: default_value.map(ToString::to_string),
        no_echo: Some(no_echo.to_string()),
        description: description.map(ToString::to_string),
        allowed_values: allowed_values.iter().map(ToString::to_string).collect(),
    }
}

/// The `Parameters` the response reports, member by member, so a declaration's key stays
/// paired with its own default and constraints.
fn wire_parameters(xml: &str) -> Vec<WireParameter> {
    list_members(xml, "Parameters")
        .iter()
        .map(|entry| WireParameter {
            key: xml_text(entry, "ParameterKey"),
            parameter_type: xml_text(entry, "ParameterType"),
            default_value: xml_text(entry, "DefaultValue"),
            no_echo: xml_text(entry, "NoEcho"),
            description: xml_text(entry, "Description"),
            allowed_values: list_members(entry, "AllowedValues")
                .iter()
                .map(|value| xml_unescape(value.trim()))
                .collect(),
        })
        .collect()
}

/// The members of one named list the response reports, trimmed, in wire order.
fn wire_list(xml: &str, tag: &str) -> Vec<String> {
    list_members(xml, tag)
        .iter()
        .map(|entry| xml_unescape(entry.trim()))
        .collect()
}

/// `GetTemplateSummary` over the body a request carries.
async fn summarise_body(router: &MultiServiceRouter, template_body: &str) -> (u16, String) {
    call(
        router,
        "GetTemplateSummary",
        &[member("TemplateBody", template_body)],
    )
    .await
}

/// `GetTemplateSummary` for a stack, by name or by stack id.
async fn summarise_stack(router: &MultiServiceRouter, name_or_id: &str) -> (u16, String) {
    call(
        router,
        "GetTemplateSummary",
        &[member("StackName", name_or_id)],
    )
    .await
}

/// The response with its request id removed: two reads of the same template must answer the
/// same summary, and the id names the REQUEST rather than the summary.
fn without_request_id(xml: &str) -> String {
    let open = "<RequestId>";
    let close = "</RequestId>";
    match (xml.find(open), xml.find(close)) {
        (Some(start), Some(end)) => format!("{}{}", &xml[..start], &xml[end + close.len()..]),
        _ => xml.to_string(),
    }
}

/// The latest `AWSTemplateFormatVersion`, which is the version in force for a template
/// that declares none: "if you don't specify a value, CloudFormation assumes the latest
/// template format version", and the latest is the only value the section accepts. Spelled
/// out here rather than read from the crate: the value is the requirement.
const LATEST_FORMAT_VERSION: &str = "2010-09-09";

/// One row of the "what the template declares" table.
struct Case {
    case: &'static str,
    template: String,
    /// The `AWSTemplateFormatVersion` the template DECLARES, `None` for one that declares
    /// none — whose summary reports [`LATEST_FORMAT_VERSION`], the version in force for it.
    version: Option<&'static str>,
    description: Option<&'static str>,
    /// The `Metadata` section as the JSON document the response must carry.
    metadata: Option<&'static str>,
    parameters: Vec<WireParameter>,
    capabilities: Vec<&'static str>,
    /// The resource types `CapabilitiesReason` must name, sorted; empty when the response
    /// must carry no reason at all.
    reason_types: Vec<&'static str>,
    /// The type of every resource the template defines — one entry per declared resource,
    /// ascending by type name, whatever order the body declares them in.
    resource_types: Vec<&'static str>,
    transforms: Vec<&'static str>,
}

/// The templates this table summarises.
fn cases() -> Vec<Case> {
    vec![
        Case {
            case: "JSON, a format version, a Description and one bucket",
            template: json!({
                "AWSTemplateFormatVersion": "2010-09-09",
                "Description": "One bucket and nothing else",
                "Resources": {"Bucket": {"Type": "AWS::S3::Bucket"}}
            })
            .to_string(),
            version: Some("2010-09-09"),
            description: Some("One bucket and nothing else"),
            metadata: None,
            parameters: Vec::new(),
            capabilities: Vec::new(),
            reason_types: Vec::new(),
            resource_types: vec!["AWS::S3::Bucket"],
            transforms: Vec::new(),
        },
        Case {
            case: "JSON with neither a version nor a Description, one type declared twice",
            template: json!({
                "Resources": {
                    "Alpha": {"Type": "AWS::S3::Bucket"},
                    "Beta": {"Type": "AWS::SNS::Topic"},
                    "Gamma": {"Type": "AWS::S3::Bucket"}
                }
            })
            .to_string(),
            version: None,
            description: None,
            metadata: None,
            parameters: Vec::new(),
            capabilities: Vec::new(),
            reason_types: Vec::new(),
            // "A list of all the template resource types that are defined in the template":
            // the type of EACH declared resource, so a type two resources share is two
            // entries, and the entries ascend by type name.
            resource_types: vec!["AWS::S3::Bucket", "AWS::S3::Bucket", "AWS::SNS::Topic"],
            transforms: Vec::new(),
        },
        Case {
            case: "the S3 bucket variants, each its own resource type",
            template: json!({
                "Resources": {
                    "Bucket": {"Type": "AWS::S3::Bucket"},
                    "TableBucket": {"Type": "AWS::S3Tables::TableBucket",
                                    "Properties": {"TableBucketName": "a-table-bucket"}},
                    "VectorBucket": {"Type": "AWS::S3Vectors::VectorBucket",
                                     "Properties": {"VectorBucketName": "a-vector-bucket"}},
                    "VectorIndex": {"Type": "AWS::S3Vectors::Index",
                                    "Properties": {"IndexName": "an-index",
                                                   "VectorBucketName": "a-vector-bucket"}}
                }
            })
            .to_string(),
            version: None,
            description: None,
            metadata: None,
            parameters: Vec::new(),
            capabilities: Vec::new(),
            reason_types: Vec::new(),
            resource_types: vec![
                "AWS::S3::Bucket",
                "AWS::S3Tables::TableBucket",
                "AWS::S3Vectors::Index",
                "AWS::S3Vectors::VectorBucket",
            ],
            transforms: Vec::new(),
        },
        Case {
            case: "YAML, short-form intrinsics, three parameters and a Metadata section",
            template: "AWSTemplateFormatVersion: '2010-09-09'\n\
                 Description: A topic named by a parameter\n\
                 Metadata:\n\
                 \x20 Build:\n\
                 \x20   Revision: '7'\n\
                 Parameters:\n\
                 \x20 AlphaSecret:\n\
                 \x20   Type: String\n\
                 \x20   NoEcho: true\n\
                 \x20 BetaName:\n\
                 \x20   Type: String\n\
                 \x20   Default: beta\n\
                 \x20   Description: The name the topic takes\n\
                 \x20   AllowedValues:\n\
                 \x20     - beta\n\
                 \x20     - gamma\n\
                 \x20 GammaCount:\n\
                 \x20   Type: Number\n\
                 \x20   Default: 7\n\
                 \x20   AllowedValues: [7, 11]\n\
                 Resources:\n\
                 \x20 Topic:\n\
                 \x20   Type: AWS::SNS::Topic\n\
                 \x20   Properties:\n\
                 \x20     TopicName: !Ref BetaName\n"
                .to_string(),
            version: Some("2010-09-09"),
            description: Some("A topic named by a parameter"),
            metadata: Some(r#"{"Build":{"Revision":"7"}}"#),
            parameters: vec![
                parameter("AlphaSecret", "String", None, true, None, &[]),
                parameter(
                    "BetaName",
                    "String",
                    Some("beta"),
                    false,
                    Some("The name the topic takes"),
                    &["beta", "gamma"],
                ),
                // A numeric default and numeric allowed values are reported as the text the
                // template spelled.
                parameter("GammaCount", "Number", Some("7"), false, None, &["7", "11"]),
            ],
            capabilities: Vec::new(),
            reason_types: Vec::new(),
            resource_types: vec!["AWS::SNS::Topic"],
            transforms: Vec::new(),
        },
        Case {
            case: "JSON whose NoEcho is spelled as text, with string AllowedValues",
            template: json!({
                "Metadata": {"Instances": ["alpha", "beta"]},
                "Parameters": {"Secret": {"Type": "String", "NoEcho": "true",
                                          "AllowedValues": ["alpha", "beta"]}},
                "Resources": {"Queue": {"Type": "AWS::SQS::Queue"}}
            })
            .to_string(),
            version: None,
            description: None,
            metadata: Some(r#"{"Instances":["alpha","beta"]}"#),
            parameters: vec![parameter(
                "Secret",
                "String",
                None,
                true,
                None,
                &["alpha", "beta"],
            )],
            capabilities: Vec::new(),
            reason_types: Vec::new(),
            resource_types: vec!["AWS::SQS::Queue"],
            transforms: Vec::new(),
        },
        Case {
            case: "an IAM role carrying a custom RoleName",
            template: json!({
                "Resources": {"Role": {"Type": "AWS::IAM::Role",
                                       "Properties": {"RoleName": "a-named-role"}}}
            })
            .to_string(),
            version: None,
            description: None,
            metadata: None,
            parameters: Vec::new(),
            capabilities: vec!["CAPABILITY_NAMED_IAM"],
            reason_types: vec!["AWS::IAM::Role"],
            resource_types: vec!["AWS::IAM::Role"],
            transforms: Vec::new(),
        },
        Case {
            case: "a Transform section naming two macros, as a YAML list",
            template: "Transform:\n\
                 \x20 - MyFirstMacro\n\
                 \x20 - MySecondMacro\n\
                 Resources:\n\
                 \x20 Bucket:\n\
                 \x20   Type: AWS::S3::Bucket\n"
                .to_string(),
            version: None,
            description: None,
            metadata: None,
            parameters: Vec::new(),
            // A template that invokes a macro requires the macro acknowledgement; the reason
            // names RESOURCES, of which this template declares none that requires anything.
            capabilities: vec!["CAPABILITY_AUTO_EXPAND"],
            reason_types: Vec::new(),
            resource_types: vec!["AWS::S3::Bucket"],
            transforms: vec!["MyFirstMacro", "MySecondMacro"],
        },
        Case {
            case: "a Transform section spelled as a macro object",
            template: json!({
                "Transform": {"Name": "MyMacro", "Parameters": {"Scope": "all"}},
                "Resources": {"Topic": {"Type": "AWS::SNS::Topic"}}
            })
            .to_string(),
            version: None,
            description: None,
            metadata: None,
            parameters: Vec::new(),
            capabilities: vec!["CAPABILITY_AUTO_EXPAND"],
            reason_types: Vec::new(),
            resource_types: vec!["AWS::SNS::Topic"],
            transforms: vec!["MyMacro"],
        },
    ]
}

#[tokio::test]
async fn get_template_summary_reports_what_the_template_declares() {
    let router = router();
    for case in cases() {
        let label = case.case;
        let (status, body) = summarise_body(&router, &case.template).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected 200, got {status} {body}"
        );
        let outside = outside_parameters(&body);
        assert_eq!(
            xml_text(&outside, "Version").as_deref(),
            Some(case.version.unwrap_or(LATEST_FORMAT_VERSION)),
            "case {label:?}: expected the format version in force, got {body}"
        );
        assert_eq!(
            xml_text(&outside, "Description").as_deref(),
            case.description,
            "case {label:?}: expected the template's own Description, got {body}"
        );
        assert_eq!(
            xml_text(&outside, "Metadata").as_deref(),
            case.metadata,
            "case {label:?}: expected the template's own Metadata section, got {body}"
        );
        assert_eq!(
            wire_parameters(&body),
            case.parameters,
            "case {label:?}: expected one ParameterDeclaration per declared parameter, \
             got {body}"
        );
        assert_eq!(
            wire_list(&body, "Capabilities"),
            case.capabilities,
            "case {label:?}: expected the capabilities the template requires, got {body}"
        );
        let reason = xml_text(&outside, "CapabilitiesReason");
        if case.reason_types.is_empty() {
            assert!(
                reason.is_none(),
                "case {label:?}: expected no CapabilitiesReason, got {reason:?}"
            );
        } else {
            let reason = reason.unwrap_or_else(|| {
                panic!("case {label:?}: expected a CapabilitiesReason, got {body}")
            });
            assert!(
                reason.starts_with(CAPABILITIES_REASON_PREFIX) && reason.ends_with(']'),
                "case {label:?}: expected a reason of the form \
                 {CAPABILITIES_REASON_PREFIX}…], got {reason:?}"
            );
            assert_eq!(
                reason_resource_types(&reason),
                case.reason_types,
                "case {label:?}: expected the reason to name the resource types that \
                 required the capability, got {reason:?}"
            );
        }
        assert_eq!(
            wire_list(&body, "ResourceTypes"),
            case.resource_types,
            "case {label:?}: expected the type of each declared resource, ascending by type \
             name, got {body}"
        );
        assert_eq!(
            wire_list(&body, "DeclaredTransforms"),
            case.transforms,
            "case {label:?}: expected the declared transforms, got {body}"
        );
        assert!(
            xml_text(&body, "UnrecognizedResourceTypes").is_none(),
            "case {label:?}: expected no warnings for a template of recognised types, \
             got {body}"
        );
    }
}

/// One row of the stack round trip: a template deployed as a stack, and what the summary of
/// THAT stack must report.
struct StackCase {
    case: &'static str,
    stack_name: &'static str,
    template: String,
    description: Option<&'static str>,
    /// The resource types the stack's template defines, sorted: the same set
    /// `DescribeStackResources` reports for the stack.
    resource_types: Vec<&'static str>,
}

/// The stacks the round trip deploys: a single resource, several of different services, and
/// one whose resources are named by a parameter the template declares.
fn stack_cases() -> Vec<StackCase> {
    vec![
        StackCase {
            case: "one bucket",
            stack_name: "summary-one-bucket",
            template: json!({
                "AWSTemplateFormatVersion": "2010-09-09",
                "Description": "A stack of one bucket",
                "Resources": {"Bucket": {"Type": "AWS::S3::Bucket",
                                         "Properties": {"BucketName": "summary-one-bucket"}}}
            })
            .to_string(),
            description: Some("A stack of one bucket"),
            resource_types: vec!["AWS::S3::Bucket"],
        },
        StackCase {
            case: "the bucket variants beside a topic",
            stack_name: "summary-variants",
            template: json!({
                "Resources": {
                    "Bucket": {"Type": "AWS::S3::Bucket",
                               "Properties": {"BucketName": "summary-variants-bucket"}},
                    "TableBucket": {"Type": "AWS::S3Tables::TableBucket",
                                    "Properties": {"TableBucketName": "summary-variants-table"}},
                    "Topic": {"Type": "AWS::SNS::Topic",
                              "Properties": {"TopicName": "summary-variants-topic"}}
                }
            })
            .to_string(),
            description: None,
            resource_types: vec![
                "AWS::S3::Bucket",
                "AWS::S3Tables::TableBucket",
                "AWS::SNS::Topic",
            ],
        },
        StackCase {
            case: "a topic named by a declared parameter",
            stack_name: "summary-parameterised",
            template: json!({
                "Description": "A parameterised stack",
                "Parameters": {"TopicName": {"Type": "String",
                                             "Default": "summary-parameterised-topic"}},
                "Resources": {"Topic": {"Type": "AWS::SNS::Topic",
                                        "Properties": {"TopicName": {"Ref": "TopicName"}}}}
            })
            .to_string(),
            description: Some("A parameterised stack"),
            resource_types: vec!["AWS::SNS::Topic"],
        },
    ]
}

/// The resource types `DescribeStackResources` reports for a stack, sorted.
fn described_resource_types(xml: &str) -> Vec<String> {
    let mut types: Vec<String> = wire_resources(xml)
        .iter()
        .map(|resource| resource.resource_type.clone().unwrap_or_default())
        .collect();
    types.sort();
    types.dedup();
    types
}

#[tokio::test]
async fn the_summary_of_a_stack_agrees_with_the_resources_the_stack_reports() {
    let router = router();
    for case in stack_cases() {
        let label = case.case;
        deploy(&router, case.stack_name, &case.template).await;

        let (status, body) = summarise_stack(&router, case.stack_name).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected 200 from GetTemplateSummary, got {status} {body}"
        );
        let mut summarised = wire_list(&body, "ResourceTypes");
        summarised.sort();
        let expected: Vec<String> = {
            let mut expected: Vec<String> = case
                .resource_types
                .iter()
                .map(ToString::to_string)
                .collect();
            expected.sort();
            expected
        };
        assert_eq!(
            summarised, expected,
            "case {label:?}: expected the stack's own template's resource types, got {body}"
        );
        assert_eq!(
            xml_text(&outside_parameters(&body), "Description").as_deref(),
            case.description,
            "case {label:?}: expected the stack template's Description, got {body}"
        );

        let (status, described) = resources_of(&router, case.stack_name).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected 200 from DescribeStackResources, got {status} {described}"
        );
        assert_eq!(
            summarised,
            described_resource_types(&described),
            "case {label:?}: expected the summary and the described resources to agree on \
             the resource types, got {body} and {described}"
        );

        // The same stack by its stack ID, which is what a client holding the id of a stack
        // reads it by: the summary is of the stack, not of the name.
        let stack_id = xml_text(&described, "StackId").unwrap_or_default();
        let (status, by_id) = summarise_stack(&router, &stack_id).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected 200 for the stack id {stack_id:?}, got {status} {by_id}"
        );
        assert_eq!(
            without_request_id(&by_id),
            without_request_id(&body),
            "case {label:?}: expected the same summary by stack id as by name, got \
             {by_id} and {body}"
        );
    }
}

#[tokio::test]
async fn summarising_a_template_changes_nothing_a_later_caller_sees() {
    let (router, engine) = router_with_engine();
    let template = json!({
        "Description": "A summarised template",
        "Parameters": {"Name": {"Type": "String", "Default": "a-name",
                                "AllowedValues": ["a-name", "b-name"]}},
        "Resources": {"Role": {"Type": "AWS::IAM::Role", "Properties": {"RoleName": "r"}}}
    })
    .to_string();
    let (first_status, first) = summarise_body(&router, &template).await;
    let (second_status, second) = summarise_body(&router, &template).await;
    assert_eq!(
        (first_status, second_status),
        (200, 200),
        "case \"summarise twice\": expected both calls to answer 200, got \
         {first_status} and {second_status}"
    );
    assert_eq!(
        without_request_id(&first),
        without_request_id(&second),
        "case \"summarise twice\": expected the same summary both times, got \
         {first} and {second}"
    );
    let held = store(&engine);
    let state = held.lock();
    assert!(
        state.stacks.is_empty() && state.change_sets.is_empty(),
        "case \"summarise twice\": expected a summary to persist nothing, got {} stack(s) \
         and {} change set(s)",
        state.stacks.len(),
        state.change_sets.len()
    );
}

/// One row of the unrecognized-types table: the types a template declares that
/// CloudFormation does not recognise are an ERROR, and warnings only where the request's
/// `TemplateSummaryConfig` asks for them.
struct WarningCase {
    case: &'static str,
    template: String,
    /// The types the warning must name, in wire order.
    unrecognized: Vec<&'static str>,
    /// The type of every resource the template declares, in wire order, which the summary
    /// reports either way.
    resource_types: Vec<&'static str>,
}

fn warning_cases() -> Vec<WarningCase> {
    vec![
        WarningCase {
            case: "one unrecognised type beside a recognised one",
            template: json!({
                "Resources": {
                    "Bucket": {"Type": "AWS::S3::Bucket"},
                    "Mystery": {"Type": "AWS::Nonesuch::Thing"}
                }
            })
            .to_string(),
            unrecognized: vec!["AWS::Nonesuch::Thing"],
            resource_types: vec!["AWS::Nonesuch::Thing", "AWS::S3::Bucket"],
        },
        WarningCase {
            case: "two unrecognised types, one of them declared twice",
            template: json!({
                "Resources": {
                    "Alpha": {"Type": "AWS::Nonesuch::Thing"},
                    "Beta": {"Type": "Example::Private::Resource"},
                    "Gamma": {"Type": "AWS::Nonesuch::Thing"}
                }
            })
            .to_string(),
            unrecognized: vec!["AWS::Nonesuch::Thing", "Example::Private::Resource"],
            resource_types: vec![
                "AWS::Nonesuch::Thing",
                "AWS::Nonesuch::Thing",
                "Example::Private::Resource",
            ],
        },
        WarningCase {
            // The configuration asked for warnings and this template gives them nothing to
            // name: the warnings are reported all the same, which is what lets a caller tell
            // "no unrecognized types" from "warnings not produced".
            case: "a template of recognised types only",
            template: json!({
                "Resources": {
                    "Bucket": {"Type": "AWS::S3::Bucket"},
                    "Topic": {"Type": "AWS::SNS::Topic"}
                }
            })
            .to_string(),
            unrecognized: Vec::new(),
            resource_types: vec!["AWS::S3::Bucket", "AWS::SNS::Topic"],
        },
    ]
}

#[tokio::test]
async fn unrecognized_resource_types_are_warnings_only_where_the_request_asks_for_them() {
    let router = router();
    for case in warning_cases() {
        let label = case.case;
        // Without the configuration: "any unrecognized resource types generate warnings and
        // not an error" is what the configuration BUYS, so a template declaring one is
        // refused here — and one declaring none is summarised, with no warnings at all.
        let (status, body) = summarise_body(&router, &case.template).await;
        if case.unrecognized.is_empty() {
            assert_eq!(
                status, 200,
                "case {label:?}: expected 200 for a template of recognised types, got \
                 {status} {body}"
            );
            assert!(
                !body.contains("<Warnings"),
                "case {label:?}: expected no Warnings where the request asked for none, got \
                 {body}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {label:?}: expected 400 without the warnings configuration, got \
                 {status} {body}"
            );
            assert_eq!(
                xml_text(&body, "Code").as_deref(),
                Some("ValidationError"),
                "case {label:?}: expected the wire code ValidationError, got {body}"
            );
            let message = xml_text(&body, "Message").unwrap_or_default();
            for unrecognized in &case.unrecognized {
                assert!(
                    message.contains(unrecognized),
                    "case {label:?}: expected the refusal to name {unrecognized:?}, got \
                     {message:?}"
                );
            }
        }

        // With it: the summary is answered, and the warnings are reported — naming the types
        // the catalogue does not recognise, and naming none where there is none to name.
        let (status, body) = call(
            &router,
            "GetTemplateSummary",
            &[
                member("TemplateBody", &case.template),
                member(
                    "TemplateSummaryConfig.TreatUnrecognizedResourceTypesAsWarnings",
                    "true",
                ),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected 200 with the warnings configuration, got \
             {status} {body}"
        );
        assert!(
            body.contains("<Warnings"),
            "case {label:?}: expected the warnings the configuration asked for, got {body}"
        );
        assert_eq!(
            wire_list(&body, "UnrecognizedResourceTypes"),
            case.unrecognized,
            "case {label:?}: expected the unrecognised types as warnings, got {body}"
        );
        let reported = wire_list(&body, "ResourceTypes");
        assert_eq!(
            reported, case.resource_types,
            "case {label:?}: expected every declared resource's type reported, got {body}"
        );
    }
}

/// A template declaring a `Transform` is held to no resource-type catalogue: "CloudFormation
/// processes the template fragments" a macro is given and validates what came back, so the
/// types such a body spells may be the macro's INPUT. The summary reports them, refuses
/// nothing and warns about nothing.
#[tokio::test]
async fn a_transformed_template_is_summarised_without_a_catalogue_check() {
    let case = "an unrecognised type under a macro this service does not expand";
    let router = router();
    let template = json!({
        "Transform": "MyMacro",
        "Resources": {"Mystery": {"Type": "AWS::Nonesuch::Thing"}}
    })
    .to_string();
    let (status, body) = summarise_body(&router, &template).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 for a transformed template, got {status} {body}"
    );
    assert_eq!(
        wire_list(&body, "ResourceTypes"),
        vec!["AWS::Nonesuch::Thing".to_string()],
        "case {case:?}: expected the types the body spells, got {body}"
    );
    assert_eq!(
        wire_list(&body, "DeclaredTransforms"),
        vec!["MyMacro".to_string()],
        "case {case:?}: expected the declared transform, got {body}"
    );
    assert!(
        xml_text(&body, "UnrecognizedResourceTypes").is_none(),
        "case {case:?}: expected no warning for a template held to no catalogue, got {body}"
    );
}

/// One row of the refusal table: a request `GetTemplateSummary` must not answer a summary
/// for, and the status, wire code and message its refusal carries.
struct RefusalCase {
    case: &'static str,
    members: Vec<(String, String)>,
    status: u16,
    code: &'static str,
    /// Text the message must contain, so a row cannot pass by failing for another reason.
    message: String,
}

#[tokio::test]
async fn get_template_summary_refuses_a_request_it_cannot_summarise() {
    let router = router();
    let refusals = vec![
        RefusalCase {
            case: "no template named at all",
            members: Vec::new(),
            status: 400,
            code: "ValidationError",
            message: "Exactly one of Template URL, Template Body, Stack Name must be \
                      specified."
                .to_string(),
        },
        RefusalCase {
            case: "a TemplateBody and a StackName together",
            members: vec![
                member(
                    "TemplateBody",
                    r#"{"Resources": {"B": {"Type": "AWS::S3::Bucket"}}}"#,
                ),
                member("StackName", "a-stack"),
            ],
            status: 400,
            code: "ValidationError",
            message: "Exactly one of Template URL, Template Body, Stack Name must be \
                      specified."
                .to_string(),
        },
        RefusalCase {
            // A THIRD way of naming more than one source: the sentence is fixed text, so it
            // is the same whichever pair the request states.
            case: "a StackName and a StackSetName together",
            members: vec![
                member("StackName", "a-stack"),
                member("StackSetName", "a-stack-set"),
            ],
            status: 400,
            code: "ValidationError",
            message: "Exactly one of Template URL, Template Body, Stack Name must be \
                      specified."
                .to_string(),
        },
        RefusalCase {
            // A `TemplateURL` is resolved by reading the object it addresses out of this
            // emulator's own Amazon S3, so one naming a bucket nothing created is the
            // REQUEST's own bad reference — not a summary of a template nobody read.
            case: "a TemplateURL naming no object this account can read",
            members: vec![member(
                "TemplateURL",
                "https://s3.amazonaws.com/a-bucket/a-template.json",
            )],
            status: 400,
            code: "ValidationError",
            message: "S3 error: The specified bucket does not exist\nFor more information \
                      check http://docs.aws.amazon.com/AmazonS3/latest/API/ErrorResponses.html"
                .to_string(),
        },
        RefusalCase {
            case: "a StackName no stack of this Region carries",
            members: vec![member("StackName", "no-such-stack")],
            status: 400,
            code: "ValidationError",
            message: "Stack with id no-such-stack does not exist".to_string(),
        },
        RefusalCase {
            case: "a StackSetName this account and Region holds no StackSet for",
            members: vec![member("StackSetName", "no-such-stack-set")],
            status: 404,
            code: "StackSetNotFoundException",
            message: "StackSet no-such-stack-set not found".to_string(),
        },
        RefusalCase {
            case: "a body that is neither valid JSON nor valid YAML",
            members: vec![member("TemplateBody", "{\"Resources\": ")],
            status: 400,
            code: "ValidationError",
            message: "Template format error".to_string(),
        },
        RefusalCase {
            case: "a template whose Resources section is empty",
            members: vec![member("TemplateBody", r#"{"Resources": {}}"#)],
            status: 400,
            code: "ValidationError",
            message: "At least one Resources member must be defined".to_string(),
        },
        RefusalCase {
            case: "a template whose only resource declares no Type",
            members: vec![member(
                "TemplateBody",
                r#"{"Resources": {"Bucket": {"Properties": {}}}}"#,
            )],
            status: 400,
            code: "ValidationError",
            // The entry is named by its own path: "Type: required" is a rule about THAT
            // entry, not about the section.
            message: "[/Resources/Bucket] Every Resources object must contain a Type member."
                .to_string(),
        },
        RefusalCase {
            case: "a template declaring one resource beside an untyped sibling",
            members: vec![member(
                "TemplateBody",
                r#"{"Resources": {
                    "Bucket": {"Type": "AWS::S3::Bucket"},
                    "Untyped": {"Properties": {}}
                }}"#,
            )],
            status: 400,
            code: "ValidationError",
            message: "[/Resources/Untyped] Every Resources object must contain a Type member."
                .to_string(),
        },
    ];

    for refusal in &refusals {
        let label = refusal.case;
        let (status, body) = call(&router, "GetTemplateSummary", &refusal.members).await;
        assert_eq!(
            status, refusal.status,
            "case {label:?}: expected {}, got {status} {body}",
            refusal.status
        );
        assert_eq!(
            xml_text(&body, "Code").as_deref(),
            Some(refusal.code),
            "case {label:?}: expected the wire code {:?}, got {body}",
            refusal.code
        );
        let message = xml_text(&body, "Message").unwrap_or_default();
        assert!(
            message.contains(refusal.message.as_str()),
            "case {label:?}: expected a message naming {:?}, got {message:?}",
            refusal.message
        );
    }
}
