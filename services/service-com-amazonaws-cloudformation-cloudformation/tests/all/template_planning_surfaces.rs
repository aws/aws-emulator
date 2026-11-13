// SPDX-License-Identifier: CC0-1.0

//! The PRE-DEPLOY PLANNING surface, end to end over the awsQuery wire: what
//! `ValidateTemplate`, `GetTemplateSummary` and `GetTemplate` tell a client about a template
//! before it deploys one, and what each of them refuses.
//!
//! Four rules, each asserted over a table whose rows differ along what the rule generalises
//! over:
//!
//! * **A parameter's `Default` has to be one of the values its own `AllowedValues` admits**:
//!   the default is the value in force when a request passes none, so a declaration stating
//!   one outside that set is refused — by every operation that takes a template body, with
//!   the service's own sentence naming the parameter. The rows differ along the parameter's
//!   type (string, number, list), the spelling of the template (JSON and YAML), which
//!   parameter of several is the offending one, and — as the controls — a default inside the
//!   set, a declaration with no `AllowedValues` at all and one with no `Default`.
//! * **An `Fn::GetAtt` naming a resource the template does not declare is a template error
//!   about the FUNCTION**, not an unresolved dependency of the section it stands in, so the
//!   refusal names the intrinsic and the logical id it could not resolve. The rows differ
//!   along the section the instance stands in, the form the argument takes (the two-element
//!   list and the dotted short form), the attribute read and the logical id named — with a
//!   `Ref` to an undeclared name as the control, which IS reported as its section's
//!   unresolved dependency.
//! * **The capabilities reported for a template are the ones a request submitting it must
//!   acknowledge**: the IAM acknowledgement its resources require, and
//!   `CAPABILITY_AUTO_EXPAND` for a template that invokes a macro. Both reporting surfaces
//!   answer the same set, and what they report is what `CreateStack` really demands. The rows
//!   differ along how the macro is invoked (the `Transform` section, an `Fn::Transform`
//!   intrinsic), whether an IAM resource is declared beside it, and a template invoking
//!   nothing.
//! * **A template summary reports the template's format version, the resource types it
//!   defines and the properties an import of each of those types takes**: the version in
//!   force, one type per DECLARED RESOURCE in the order the body declares them, and one
//!   `ResourceIdentifierSummary` per type carrying that type's primary identifier and the
//!   logical ids declared with it, in that same order. The rows
//!   differ along where the summarised template comes from (the request's own body, a
//!   deployed stack, a StackSet) and the identifier kind each type is named by (a name
//!   property, an ARN, a service-assigned id).
//!
//! Beside them, three refusal rules: a `TemplateURL` is refused for the kind of location it
//! is (an unsupported scheme or endpoint, a bucket's static website endpoint, an Amazon S3
//! endpoint naming no object), a request that names its template BOTH by value and by
//! reference states two templates and is refused by every operation that takes the pair, and
//! `GetTemplate`'s `TemplateStage` value set is stated in the order CloudFormation states it,
//! which is not the order the model declares the enum in.

use serde_json::json;
use smithy_server_runtime::MultiServiceRouter;

use crate::stack_lifecycle::{
    call, create_stack, describe_stack, member, router, wait_for_the_transition, xml_blocks,
    xml_text, xml_texts,
};

/// `ValidateTemplate` for one body.
async fn validate(router: &MultiServiceRouter, template: &str) -> (u16, String) {
    call(
        router,
        "ValidateTemplate",
        &[member("TemplateBody", template)],
    )
    .await
}

/// `GetTemplateSummary` for one body.
async fn summarise(router: &MultiServiceRouter, template: &str) -> (u16, String) {
    call(
        router,
        "GetTemplateSummary",
        &[member("TemplateBody", template)],
    )
    .await
}

/// The `<Message>` of a refusal, or the whole body when it carries none.
fn message(body: &str) -> String {
    xml_text(body, "Message").unwrap_or_else(|| body.to_string())
}

/// One row of the parameter-default table.
struct DefaultCase<'a> {
    case: &'a str,
    template: String,
    /// The parameter the refusal must name, or `None` for a template whose declarations are
    /// all consistent.
    refused_parameter: Option<&'a str>,
}

/// How the service refuses a `Default` outside its parameter's `AllowedValues`.
fn must_be_one_of(parameter: &str) -> String {
    format!("Parameter '{parameter}' must be one of AllowedValues")
}

fn default_cases() -> Vec<DefaultCase<'static>> {
    let resources = json!({"Topic": {"Type": "AWS::SNS::Topic"}});
    vec![
        DefaultCase {
            case: "a string parameter whose default is outside its allowed values",
            template: json!({
                "Parameters": {"Flavor": {"Type": "String", "Default": "pistachio",
                                          "AllowedValues": ["vanilla", "chocolate"]}},
                "Resources": resources,
            })
            .to_string(),
            refused_parameter: Some("Flavor"),
        },
        DefaultCase {
            case: "a number parameter whose default is outside its allowed values",
            template: json!({
                "Parameters": {"Size": {"Type": "Number", "Default": 5,
                                        "AllowedValues": [1, 2, 3]}},
                "Resources": resources,
            })
            .to_string(),
            refused_parameter: Some("Size"),
        },
        DefaultCase {
            case: "the SECOND of two parameters is the offending one",
            template: json!({
                "Parameters": {
                    "Stage": {"Type": "String", "Default": "dev",
                              "AllowedValues": ["dev", "prod"]},
                    "Tier": {"Type": "String", "Default": "platinum",
                             "AllowedValues": ["gold", "silver"]},
                },
                "Resources": resources,
            })
            .to_string(),
            refused_parameter: Some("Tier"),
        },
        DefaultCase {
            case: "a list parameter one of whose default entries is outside the set",
            template: json!({
                "Parameters": {"Zones": {"Type": "CommaDelimitedList", "Default": "a,z",
                                         "AllowedValues": ["a", "b", "c"]}},
                "Resources": resources,
            })
            .to_string(),
            refused_parameter: Some("Zones"),
        },
        DefaultCase {
            case: "a YAML template whose default is outside its allowed values",
            template: concat!(
                "Parameters:\n",
                "  Flavour:\n",
                "    Type: String\n",
                "    Default: pistachio\n",
                "    AllowedValues:\n",
                "      - vanilla\n",
                "      - chocolate\n",
                "Resources:\n",
                "  Topic:\n",
                "    Type: AWS::SNS::Topic\n",
            )
            .to_string(),
            refused_parameter: Some("Flavour"),
        },
        DefaultCase {
            // The control: a default the set admits is a declaration the service accepts.
            case: "a default the allowed values admit",
            template: json!({
                "Parameters": {"Flavor": {"Type": "String", "Default": "vanilla",
                                          "AllowedValues": ["vanilla", "chocolate"]}},
                "Resources": resources,
            })
            .to_string(),
            refused_parameter: None,
        },
        DefaultCase {
            // A list default whose every entry the set admits, in any order.
            case: "a list default whose entries the allowed values all admit",
            template: json!({
                "Parameters": {"Zones": {"Type": "CommaDelimitedList", "Default": "c,a",
                                         "AllowedValues": ["a", "b", "c"]}},
                "Resources": resources,
            })
            .to_string(),
            refused_parameter: None,
        },
        DefaultCase {
            // No constraint to be outside of.
            case: "a default and no allowed values at all",
            template: json!({
                "Parameters": {"Flavor": {"Type": "String", "Default": "pistachio"}},
                "Resources": resources,
            })
            .to_string(),
            refused_parameter: None,
        },
        DefaultCase {
            // No default to hold to the set: the request supplies the value.
            case: "allowed values and no default",
            template: json!({
                "Parameters": {"Flavor": {"Type": "String",
                                          "AllowedValues": ["vanilla", "chocolate"]}},
                "Resources": resources,
            })
            .to_string(),
            refused_parameter: None,
        },
    ]
}

#[tokio::test]
async fn a_parameter_default_outside_its_allowed_values_is_refused() {
    let router = router();
    for row in default_cases() {
        let case = row.case;
        let (status, body) = validate(&router, &row.template).await;
        let Some(parameter) = row.refused_parameter else {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the consistent declaration accepted, got {status} \
                 {body}"
            );
            continue;
        };
        assert_eq!(
            status, 400,
            "case {case:?}: expected ValidateTemplate to refuse the template, got {status} \
             {body}"
        );
        assert_eq!(
            xml_text(&body, "Code").as_deref(),
            Some("ValidationError"),
            "case {case:?}: expected the ValidationError refusal, got {body}"
        );
        assert_eq!(
            xml_text(&body, "Type").as_deref(),
            Some("Sender"),
            "case {case:?}: expected the refusal to be the sender's, got {body}"
        );
        let reported = message(&body);
        assert_eq!(
            reported,
            must_be_one_of(parameter),
            "case {case:?}: expected {:?}, got {reported:?}",
            must_be_one_of(parameter)
        );

        // The same body is refused by the operations that SUBMIT a template, with the same
        // sentence, and nothing is created.
        let stack = "default-outside-allowed-values";
        let (status, created) = create_stack(&router, stack, &row.template, &[]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected CreateStack to refuse the same body, got {status} \
             {created}"
        );
        let reported = message(&created);
        assert_eq!(
            reported,
            must_be_one_of(parameter),
            "case {case:?}: expected CreateStack's refusal to be worded identically, got \
             {reported:?}"
        );
        let (described, body) = describe_stack(&router, stack).await;
        assert_eq!(
            described, 400,
            "case {case:?}: the refused create must leave no stack, got {described} {body}"
        );
    }
}

/// One row of the dangling-reference table: the template, and the sentence the refusal must
/// be.
struct DanglingCase<'a> {
    case: &'a str,
    template: String,
    expected: String,
}

/// How the service refuses an `Fn::GetAtt` reading an attribute off a resource the template
/// does not declare.
fn undefined_resource(logical_id: &str) -> String {
    format!("Template error: instance of Fn::GetAtt references undefined resource {logical_id}")
}

fn dangling_cases() -> Vec<DanglingCase<'static>> {
    vec![
        DanglingCase {
            case: "the two-element form in the Outputs section",
            template: json!({
                "Resources": {"Topic": {"Type": "AWS::SNS::Topic"}},
                "Outputs": {"Ghost": {"Value": {"Fn::GetAtt": ["GhostFn", "Arn"]}}},
            })
            .to_string(),
            expected: undefined_resource("GhostFn"),
        },
        DanglingCase {
            case: "the dotted short form in the Outputs section",
            template: json!({
                "Resources": {"Topic": {"Type": "AWS::SNS::Topic"}},
                "Outputs": {"Ghost": {"Value": {"Fn::GetAtt": "Missing.Arn"}}},
            })
            .to_string(),
            expected: undefined_resource("Missing"),
        },
        DanglingCase {
            case: "inside a resource's own properties",
            template: json!({
                "Resources": {"Param": {"Type": "AWS::SSM::Parameter",
                                        "Properties": {"Type": "String",
                                                       "Value": {"Fn::GetAtt": ["Nowhere",
                                                                                "QueueUrl"]}}}},
            })
            .to_string(),
            expected: undefined_resource("Nowhere"),
        },
        DanglingCase {
            case: "inside a condition",
            template: json!({
                "Conditions": {"Same": {"Fn::Equals": [{"Fn::GetAtt": ["Absent", "TopicName"]},
                                                       "a"]}},
                "Resources": {"Topic": {"Type": "AWS::SNS::Topic"}},
            })
            .to_string(),
            expected: undefined_resource("Absent"),
        },
        DanglingCase {
            // The control: a `Ref` to an undeclared name IS an unresolved dependency of the
            // section it stands in, and is still reported as one.
            case: "a Ref naming nothing the template declares",
            template: json!({
                "Resources": {"Topic": {"Type": "AWS::SNS::Topic"}},
                "Outputs": {"Ghost": {"Value": {"Ref": "GhostFn"}}},
            })
            .to_string(),
            expected: "Template format error: Unresolved resource dependencies [GhostFn] in \
                       the Outputs block of the template"
                .to_string(),
        },
    ]
}

#[tokio::test]
async fn a_dangling_get_att_is_refused_for_the_function_instance_that_read_it() {
    let router = router();
    for row in dangling_cases() {
        let case = row.case;
        let (status, body) = validate(&router, &row.template).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected ValidateTemplate to refuse the template, got {status} \
             {body}"
        );
        assert_eq!(
            xml_text(&body, "Code").as_deref(),
            Some("ValidationError"),
            "case {case:?}: expected the ValidationError refusal, got {body}"
        );
        let reported = message(&body);
        assert_eq!(
            reported, row.expected,
            "case {case:?}: expected {:?}, got {reported:?}",
            row.expected
        );
    }
}

/// One row of the capability table.
struct CapabilityCase<'a> {
    case: &'a str,
    stack: &'a str,
    template: String,
    /// The `Capability` values both reporting surfaces must answer with, in wire order.
    capabilities: Vec<&'a str>,
}

fn capability_cases() -> Vec<CapabilityCase<'static>> {
    vec![
        CapabilityCase {
            case: "a Transform section naming a macro",
            stack: "plan-macro-section",
            template: json!({
                "Transform": "MyWholeTemplateMacro",
                "Resources": {"Topic": {"Type": "AWS::SNS::Topic"}},
            })
            .to_string(),
            capabilities: vec!["CAPABILITY_AUTO_EXPAND"],
        },
        CapabilityCase {
            case: "an Fn::Transform intrinsic naming a macro",
            stack: "plan-macro-intrinsic",
            template: json!({
                "Resources": {"Topic": {"Type": "AWS::SNS::Topic",
                                        "Properties": {"Fn::Transform": {
                                            "Name": "AWS::Include",
                                            "Parameters": {"Location": "s3://b/s.json"}}}}},
            })
            .to_string(),
            capabilities: vec!["CAPABILITY_AUTO_EXPAND"],
        },
        CapabilityCase {
            case: "a macro beside an IAM resource",
            stack: "plan-macro-and-iam",
            template: json!({
                "Transform": "MyWholeTemplateMacro",
                "Resources": {"Role": {"Type": "AWS::IAM::Role"}},
            })
            .to_string(),
            capabilities: vec!["CAPABILITY_IAM", "CAPABILITY_AUTO_EXPAND"],
        },
        CapabilityCase {
            case: "an IAM resource with a custom name and no macro",
            stack: "plan-named-iam",
            template: json!({
                "Resources": {"Role": {"Type": "AWS::IAM::Role",
                                       "Properties": {"RoleName": "plan-named-role"}}},
            })
            .to_string(),
            capabilities: vec!["CAPABILITY_NAMED_IAM"],
        },
        CapabilityCase {
            // The control: a template that declares nothing needing acknowledgement reports
            // no capability at all.
            case: "neither a macro nor an IAM resource",
            stack: "plan-nothing-to-acknowledge",
            template: json!({"Resources": {"Topic": {"Type": "AWS::SNS::Topic"}}}).to_string(),
            capabilities: Vec::new(),
        },
    ]
}

#[tokio::test]
async fn the_capabilities_reported_are_the_ones_a_create_demands() {
    let router = router();
    for row in capability_cases() {
        let case = row.case;
        for (surface, (status, body)) in [
            ("ValidateTemplate", validate(&router, &row.template).await),
            ("GetTemplateSummary", summarise(&router, &row.template).await),
        ] {
            assert_eq!(
                status, 200,
                "case {case:?} on {surface}: expected 200, got {status} {body}"
            );
            let reported = xml_blocks(&body, "Capabilities")
                .first()
                .map(|block| xml_texts(block, "member"))
                .unwrap_or_default();
            assert_eq!(
                reported, row.capabilities,
                "case {case:?} on {surface}: expected {:?}, got {body}",
                row.capabilities
            );
        }

        // What the surfaces report is what a create really demands: the same body, submitted
        // with none of those capabilities, is refused for exactly them.
        let (status, created) = create_stack(&router, row.stack, &row.template, &[]).await;
        if row.capabilities.is_empty() {
            assert_eq!(
                status, 200,
                "case {case:?}: a template needing no acknowledgement must be accepted, got \
                 {status} {created}"
            );
            continue;
        }
        assert_eq!(
            status, 400,
            "case {case:?}: expected the unacknowledged create refused, got {status} {created}"
        );
        assert_eq!(
            xml_text(&created, "Code").as_deref(),
            Some("InsufficientCapabilitiesException"),
            "case {case:?}: expected the modelled refusal, got {created}"
        );
        // The capability the refusal names is one of those the surfaces reported: which of
        // several a request withheld the service names first is the service's own.
        let reported = message(&created);
        assert!(
            row.capabilities
                .iter()
                .any(|capability| reported.contains(capability)),
            "case {case:?}: expected the refusal to name one of {:?}, got {reported:?}",
            row.capabilities
        );
    }
}

/// One `ResourceIdentifierSummary` as the response carries it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct WireSummary {
    resource_type: Option<String>,
    logical_resource_ids: Vec<String>,
    resource_identifiers: Vec<String>,
}

/// A summary row, spelled as briefly as a table row can be.
fn summary(
    resource_type: &str,
    logical_resource_ids: &[&str],
    resource_identifiers: &[&str],
) -> WireSummary {
    WireSummary {
        resource_type: Some(resource_type.to_string()),
        logical_resource_ids: logical_resource_ids
            .iter()
            .map(|id| (*id).to_string())
            .collect(),
        resource_identifiers: resource_identifiers
            .iter()
            .map(|property| (*property).to_string())
            .collect(),
    }
}

/// Every `ResourceIdentifierSummary` in a response, in wire order.
///
/// The entries are split on the element each one OPENS with rather than read as `<member>`
/// blocks: a summary carries two nested lists whose own entries are `<member>`s too, so a
/// scan for that element name cannot tell an entry's end from its first nested one.
fn wire_summaries(xml: &str) -> Vec<WireSummary> {
    let Some(block) = xml_blocks(xml, "ResourceIdentifierSummaries")
        .into_iter()
        .next()
    else {
        return Vec::new();
    };
    block
        .split("<ResourceType>")
        .skip(1)
        .map(|entry| {
            let entry = format!("<ResourceType>{entry}");
            WireSummary {
                resource_type: xml_text(&entry, "ResourceType"),
                logical_resource_ids: xml_blocks(&entry, "LogicalResourceIds")
                    .first()
                    .map(|ids| xml_texts(ids, "member"))
                    .unwrap_or_default(),
                resource_identifiers: xml_blocks(&entry, "ResourceIdentifiers")
                    .first()
                    .map(|properties| xml_texts(properties, "member"))
                    .unwrap_or_default(),
            }
        })
        .collect()
}

/// The `ResourceTypes` a summary reports, in wire order.
fn wire_resource_types(xml: &str) -> Vec<String> {
    xml_blocks(xml, "ResourceTypes")
        .first()
        .map(|block| xml_texts(block, "member"))
        .unwrap_or_default()
}

/// One row of the summary table: a template, and what a summary of it must report.
struct SummaryCase<'a> {
    case: &'a str,
    template: String,
    /// The `AWSTemplateFormatVersion` in force for it.
    version: &'a str,
    resource_types: Vec<&'a str>,
    summaries: Vec<WireSummary>,
}

fn summary_cases() -> Vec<SummaryCase<'static>> {
    // The bodies of the rows whose DECLARED ORDER is part of what they assert are written
    // out as text rather than built with `json!`, which states an object's members in
    // whichever order the build's `serde_json` serialises them in: the order a summary
    // follows is the order the body itself states, so each such row states it.
    vec![
        SummaryCase {
            // The identifier is the type's NAME property. Each resource contributes its own
            // type entry and the entries ascend by type name, while the identifier summaries
            // follow the order the body declares the resources in, one entry per type — so
            // this row reads the two members back in two different orders.
            case: "a bucket declared before a role",
            template: r#"{"AWSTemplateFormatVersion":"2010-09-09",
                "Resources":{
                    "Bucket":{"Type":"AWS::S3::Bucket"},
                    "Role":{"Type":"AWS::IAM::Role"}}}"#
                .to_string(),
            version: "2010-09-09",
            resource_types: vec!["AWS::IAM::Role", "AWS::S3::Bucket"],
            summaries: vec![
                summary("AWS::S3::Bucket", &["Bucket"], &["BucketName"]),
                summary("AWS::IAM::Role", &["Role"], &["RoleName"]),
            ],
        },
        SummaryCase {
            // A type whose `Ref` returns an ARN is identified by that ARN, and one whose
            // `Ref` returns a URL by the URL.
            case: "a topic and a queue, each identified by what its Ref returns",
            template: r#"{"Resources":{
                    "Topic":{"Type":"AWS::SNS::Topic"},
                    "Queue":{"Type":"AWS::SQS::Queue"}}}"#
                .to_string(),
            // A template declaring no format version still conforms to one, which is the
            // version a summary reports.
            version: "2010-09-09",
            resource_types: vec!["AWS::SNS::Topic", "AWS::SQS::Queue"],
            summaries: vec![
                summary("AWS::SNS::Topic", &["Topic"], &["TopicArn"]),
                summary("AWS::SQS::Queue", &["Queue"], &["QueueUrl"]),
            ],
        },
        SummaryCase {
            // Several resources of ONE type: the identifier summary of that type is reported
            // once, carrying every logical id declared with it in declared order, while
            // `ResourceTypes` carries that type once per resource.
            case: "three resources of two types",
            template: r#"{"Resources":{
                    "Data":{"Type":"AWS::S3::Bucket"},
                    "Archive":{"Type":"AWS::S3::Bucket"},
                    "Vpc":{"Type":"AWS::EC2::VPC",
                           "Properties":{"CidrBlock":"10.0.0.0/16"}}}}"#
                .to_string(),
            version: "2010-09-09",
            resource_types: vec!["AWS::EC2::VPC", "AWS::S3::Bucket", "AWS::S3::Bucket"],
            summaries: vec![
                summary("AWS::S3::Bucket", &["Data", "Archive"], &["BucketName"]),
                summary("AWS::EC2::VPC", &["Vpc"], &["VpcId"]),
            ],
        },
        SummaryCase {
            // A type whose identifier its owning service ASSIGNS is named by that id.
            case: "a KMS key, identified by the id KMS assigns",
            template: json!({
                "Resources": {"Key": {"Type": "AWS::KMS::Key",
                                      "Properties": {"KeyPolicy": {"Statement": []}}}},
            })
            .to_string(),
            version: "2010-09-09",
            resource_types: vec!["AWS::KMS::Key"],
            summaries: vec![summary("AWS::KMS::Key", &["Key"], &["KeyId"])],
        },
    ]
}

#[tokio::test]
async fn a_template_summary_reports_its_version_types_and_identifier_summaries() {
    let router = router();
    for row in summary_cases() {
        let case = row.case;
        let (status, body) = summarise(&router, &row.template).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {body}"
        );
        assert_eq!(
            xml_text(&body, "Version").as_deref(),
            Some(row.version),
            "case {case:?}: expected the version in force {:?}, got {body}",
            row.version
        );
        let reported = wire_resource_types(&body);
        assert_eq!(
            reported, row.resource_types,
            "case {case:?}: expected the type of each declared resource, ascending by type \
             name, {:?}, got {reported:?}",
            row.resource_types
        );
        let reported = wire_summaries(&body);
        assert_eq!(
            reported, row.summaries,
            "case {case:?}: expected {:?}, got {reported:?}",
            row.summaries
        );
    }
}

#[tokio::test]
async fn a_summary_of_a_stack_and_of_a_stack_set_reports_the_same_members() {
    let router = router();
    let template = json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Resources": {"Topic": {"Type": "AWS::SNS::Topic",
                                "Properties": {"TopicName": "planning-surfaces-topic"}}},
    })
    .to_string();
    let expected = vec![summary("AWS::SNS::Topic", &["Topic"], &["TopicArn"])];

    // The STACK path: the template a deployed stack holds.
    let (status, created) = create_stack(&router, "planning-surfaces", &template, &[]).await;
    assert_eq!(
        status, 200,
        "case \"a deployed stack\": expected the create to succeed, got {status} {created}"
    );
    wait_for_the_transition().await;

    // The STACKSET path: the template a StackSet was created with.
    let (status, created) = call(
        &router,
        "CreateStackSet",
        &[
            member("StackSetName", "planning-surfaces-set"),
            member("TemplateBody", &template),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"a StackSet\": expected the create to succeed, got {status} {created}"
    );

    for (case, members) in [
        (
            "a deployed stack",
            vec![member("StackName", "planning-surfaces")],
        ),
        (
            "a StackSet",
            vec![member("StackSetName", "planning-surfaces-set")],
        ),
        (
            "the body itself",
            vec![member("TemplateBody", &template)],
        ),
    ] {
        let (status, body) = call(&router, "GetTemplateSummary", &members).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {body}"
        );
        assert_eq!(
            xml_text(&body, "Version").as_deref(),
            Some("2010-09-09"),
            "case {case:?}: expected the template's own format version, got {body}"
        );
        assert_eq!(
            wire_resource_types(&body),
            vec!["AWS::SNS::Topic".to_string()],
            "case {case:?}: expected the type the template defines, got {body}"
        );
        let reported = wire_summaries(&body);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn a_request_that_names_a_template_twice_is_refused_by_every_operation_that_takes_one() {
    let router = router();
    let template = json!({"Resources": {"Topic": {"Type": "AWS::SNS::Topic"}}}).to_string();
    // The rows are the operations whose input carries exactly the two template members, so
    // the sentence is one operation's no more than another's.
    for (action, extra) in [
        ("ValidateTemplate", Vec::new()),
        ("GetTemplateSummary", Vec::new()),
        ("EstimateTemplateCost", Vec::new()),
        ("CreateStack", vec![member("StackName", "named-twice")]),
        (
            "CreateStackSet",
            vec![member("StackSetName", "named-twice-set")],
        ),
        (
            "UpdateStack",
            vec![member("StackName", "named-twice")],
        ),
        (
            "CreateChangeSet",
            vec![
                member("StackName", "named-twice"),
                member("ChangeSetName", "cs-named-twice"),
                member("ChangeSetType", "CREATE"),
            ],
        ),
    ] {
        let mut members = vec![
            member("TemplateBody", &template),
            member("TemplateURL", "https://s3.amazonaws.com/a-bucket/other.json"),
        ];
        members.extend(extra);
        let (status, body) = call(&router, action, &members).await;
        assert_eq!(
            status, 400,
            "case {action:?}: expected the request refused, got {status} {body}"
        );
        assert_eq!(
            xml_text(&body, "Code").as_deref(),
            Some("ValidationError"),
            "case {action:?}: expected a ValidationError, got {body}"
        );
        let reported = message(&body);
        // `GetTemplateSummary` states the exclusivity ITS own input documents — it takes two
        // further template sources beside these two — so it is the one row with a sentence
        // of its own.
        let expected = if action == "GetTemplateSummary" {
            "Exactly one of Template URL, Template Body, Stack Name must be specified."
        } else {
            "You cannot specify both Template URL and Template Body."
        };
        assert_eq!(
            reported, expected,
            "case {action:?}: expected {expected:?}, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn a_template_url_is_refused_for_the_kind_of_location_it_is() {
    let router = router();
    // The rows differ along what is wrong with the location: the scheme, the endpoint, and
    // an Amazon S3 endpoint naming no object.
    for (case, url, expected) in [
        (
            "a scheme the documentation does not allow",
            "s3://a-bucket/a-template.json",
            "The location for an Amazon S3 bucket must start with https://",
        ),
        (
            "a host that is no Amazon S3 endpoint and names no object",
            "https://templates.example.com/a-template.json",
            "TemplateURL must be a supported URL.",
        ),
        (
            "text no URL reader will read at all",
            "a-bucket/a-template.json",
            "TemplateURL must be a supported URL.",
        ),
        (
            "a bucket's static website endpoint",
            "http://a-bucket.s3-website-us-east-1.amazonaws.com/a-template.json",
            "S3 static website URLs are not supported.",
        ),
        (
            "the dot-separated spelling of that endpoint, over https",
            "https://a-bucket.s3-website.us-east-1.amazonaws.com/a-template.json",
            "S3 static website URLs are not supported.",
        ),
        (
            "an Amazon S3 endpoint naming a bucket but no object in it",
            "https://s3.us-east-1.amazonaws.com/a-bucket",
            "TemplateURL must reference a valid S3 object to which you have access.",
        ),
    ] {
        let (status, body) = call(
            &router,
            "ValidateTemplate",
            &[member("TemplateURL", url)],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the request refused, got {status} {body}"
        );
        assert_eq!(
            xml_text(&body, "Code").as_deref(),
            Some("ValidationError"),
            "case {case:?}: expected a ValidationError, got {body}"
        );
        let reported = message(&body);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn get_template_states_the_template_stage_value_set_in_the_services_own_order() {
    let router = router();
    let (status, created) = create_stack(
        &router,
        "template-stage-order",
        &json!({"Resources": {"Topic": {"Type": "AWS::SNS::Topic"}}}).to_string(),
        &[],
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"a stack to read the template of\": expected the create to succeed, got \
         {status} {created}"
    );

    // Several values outside the set, so the sentence is not fitted to one of them.
    for supplied in ["Bogus", "processed", "", "0"] {
        let (status, body) = call(
            &router,
            "GetTemplate",
            &[
                member("StackName", "template-stage-order"),
                member("TemplateStage", supplied),
            ],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {supplied:?}: expected the request refused, got {status} {body}"
        );
        assert_eq!(
            xml_text(&body, "Code").as_deref(),
            Some("ValidationError"),
            "case {supplied:?}: expected ValidationError, got {body}"
        );
        let reported = message(&body);
        assert_eq!(
            reported,
            format!(
                "1 validation error detected: Value '{supplied}' at 'templateStage' failed \
                 to satisfy constraint: Member must satisfy enum value set: [Processed, \
                 Original]"
            ),
            "case {supplied:?}: expected the service's own value set order, got {reported:?}"
        );
    }
}
