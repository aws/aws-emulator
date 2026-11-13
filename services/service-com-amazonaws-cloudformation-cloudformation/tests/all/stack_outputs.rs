// SPDX-License-Identifier: CC0-1.0

//! A stack's `Outputs`, end to end over the awsQuery wire.
//!
//! The rule under test: for a stack whose template declares an `Outputs` section,
//! `DescribeStacks` reports one `Output` per declared output, carrying the declared key,
//! description and export name, and an `OutputValue` that is the declared `Value` with
//! CloudFormation's intrinsic functions resolved against the stack's parameters, the
//! identifiers of the resources the template declares, and the pseudo parameters.
//!
//! Every case drives the real request path (form-encoded body in, XML envelope out)
//! through the wire harness `stack_lifecycle.rs` builds, so what it asserts is what a
//! client reading a deployed stack sees. The table spans the class the rule generalises
//! over: both template spellings (JSON and YAML, long-form and short-form intrinsics),
//! several resource types — including the ones a Lambda + Amazon RDS + Secrets Manager
//! stack declares, whose `Ref` and `Fn::GetAtt` return values are what a caller addresses
//! the deployed resources by — each intrinsic function this service evaluates, and the
//! count of declared outputs. The issue's own case (a YAML `AWS::SNS::Topic` template
//! whose one output is `!Ref` of the topic) is the first row.

use crate::harness::{
    call, create_stack, member, router, wait_for_the_transition, xml_text, xml_unescape, REGION,
};
use crate::stack_lifecycle::{acknowledging_iam, describe_stack, expected_status, ACCOUNT_ID};
use service_com_amazonaws_cloudformation_cloudformation::state::template::MAX_OUTPUTS_PER_TEMPLATE;
use service_com_amazonaws_cloudformation_cloudformation::state::{
    status, TEMPLATE_BODY_MAX_BYTES,
};

/// One `Output` as the response carries it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WireOutput {
    pub(crate) key: Option<String>,
    pub(crate) value: Option<String>,
    pub(crate) description: Option<String>,
    pub(crate) export_name: Option<String>,
}

/// Every `Output` in the first `Stack` of a `DescribeStacks` response, in wire order.
///
/// Read member by member rather than tag by tag, so an output's key stays paired with
/// its own value (and so a declared output that carries NO `OutputValue` is visible as
/// such instead of silently borrowing the next one's).
pub(crate) fn wire_outputs(xml: &str) -> Vec<WireOutput> {
    let Some(start) = xml.find("<Outputs>") else {
        return Vec::new();
    };
    let after = &xml[start + "<Outputs>".len()..];
    let end = after.find("</Outputs>").unwrap_or(after.len());
    let block = &after[..end];

    let mut outputs = Vec::new();
    let mut rest = block;
    while let Some(open) = rest.find("<member>") {
        let after_open = &rest[open + "<member>".len()..];
        let Some(close) = after_open.find("</member>") else {
            break;
        };
        let entry = &after_open[..close];
        outputs.push(WireOutput {
            key: element(entry, "OutputKey"),
            value: element(entry, "OutputValue"),
            description: element(entry, "Description"),
            export_name: element(entry, "ExportName"),
        });
        rest = &after_open[close + "</member>".len()..];
    }
    outputs
}

/// One element's text inside a single `Output` member.
fn element(entry: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = entry.find(&open)?;
    let after = &entry[start + open.len()..];
    let end = after.find(&close)?;
    Some(xml_unescape(&after[..end]))
}

/// The `OutputValue` the response carries for one key, and whether the key is there at
/// all.
pub(crate) fn output_for<'a>(outputs: &'a [WireOutput], key: &str) -> Option<&'a WireOutput> {
    outputs
        .iter()
        .find(|output| output.key.as_deref() == Some(key))
}

/// What one declared output must report.
#[derive(Debug, Clone)]
enum Expected {
    /// Exactly this `OutputValue`.
    Value(String),
    /// The output is reported (its key is there) but carries no `OutputValue`, because
    /// this service cannot evaluate the declared expression.
    NoValue,
    /// The output carries a value of this FORM. For an identifier the OWNING SERVICE
    /// assigns — which no request fixes, so no row can spell it — the rule is the
    /// documented form, not a literal.
    Form {
        described: &'static str,
        check: fn(&str) -> bool,
    },
}

/// Whether `value` is an AWS Transfer Family `WorkflowId`: `w-` and 17 characters of
/// `[a-z0-9]` (Transfer's model: `^w-([a-z0-9]{17})$`, `length 19..19`).
fn is_transfer_workflow_id(value: &str) -> bool {
    value.strip_prefix("w-").is_some_and(|id| {
        id.len() == 17
            && id
                .chars()
                .all(|c| c.is_ascii_digit() || c.is_ascii_lowercase())
    })
}

/// Whether `value` is a CloudWatch RUM `AppMonitorId`: the canonical UUID form RUM's own
/// Smithy model fixes —
/// `^[a-fA-F0-9]{8}-[a-fA-F0-9]{4}-[a-fA-F0-9]{4}-[a-fA-F0-9]{4}-[a-fA-F0-9]{12}$`,
/// `length 36..36`.
fn is_app_monitor_id(value: &str) -> bool {
    let groups: Vec<&str> = value.split('-').collect();
    value.len() == 36
        && groups.len() == 5
        && [8, 4, 4, 4, 12]
            .iter()
            .zip(&groups)
            .all(|(width, group)| {
                group.len() == *width && group.chars().all(|c| c.is_ascii_hexdigit())
            })
}

fn is_lower_hex(c: char) -> bool {
    c.is_ascii_digit() || ('a'..='f').contains(&c)
}

/// Whether `value` is the ARN of a Transfer resource of type `kind` whose id satisfies
/// `id_form` — `arn:aws:transfer:<region>:<account>:<kind>/<id>`.
fn is_transfer_arn(value: &str, kind: &str, id_form: fn(&str) -> bool) -> bool {
    value
        .strip_prefix(&format!("arn:aws:transfer:{REGION}:{ACCOUNT_ID}:{kind}/"))
        .is_some_and(id_form)
}

/// An Amazon Bedrock AgentCore identifier splits at its LAST `-` into a label and a
/// ten-character suffix: every one of the AgentCore control-plane model's identifier
/// patterns ends in `-[a-zA-Z0-9]{10}` (`MemoryId`, `AgentRuntimeId`) or
/// `-[0-9a-z]{10}` (`GatewayId`).
///
/// `pub(crate)` so the AgentCore suite in this target reads the identifier form from
/// here rather than keeping a second copy of the rule.
pub(crate) fn agentcore_id_parts(value: &str) -> Option<(&str, &str)> {
    let (label, suffix) = value.rsplit_once('-')?;
    (suffix.len() == 10
        && suffix.chars().all(|c| c.is_ascii_alphanumeric())
        && !label.is_empty())
    .then_some((label, suffix))
}

/// Whether `value` is a `MemoryId`: `^[a-zA-Z][a-zA-Z0-9-_]{0,99}-[a-zA-Z0-9]{10}$`,
/// `length 12..`.
fn is_agentcore_memory_id(value: &str) -> bool {
    agentcore_id_parts(value).is_some_and(|(label, _)| {
        value.len() >= 12
            && label.len() <= 100
            && label.starts_with(|c: char| c.is_ascii_alphabetic())
            && label
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    })
}

/// Whether `value` is an `AgentRuntimeId`:
/// `^[a-zA-Z][a-zA-Z0-9_]{0,99}-[a-zA-Z0-9]{10}$` — a label with NO `-` in it, and at
/// most 47 characters of it inside `AgentRuntimeArn`. `CodeInterpreterId` and `BrowserId`
/// are that same form (`^[a-zA-Z][a-zA-Z0-9_]{0,47}-[a-zA-Z0-9]{10}$`), so the AgentCore
/// suite checks its sandbox identifiers against this one rule — hence `pub(crate)`.
pub(crate) fn is_agentcore_runtime_id(value: &str) -> bool {
    agentcore_id_parts(value).is_some_and(|(label, _)| {
        label.len() <= 48
            && label.starts_with(|c: char| c.is_ascii_alphabetic())
            && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    })
}

/// Whether `value` is a `GatewayId`: `^([0-9a-z][-]?){1,100}-[0-9a-z]{10}$` — lower case
/// only, with no doubled or trailing `-` in the label, and at most 48 characters of it
/// inside `GatewayArn`.
fn is_agentcore_gateway_id(value: &str) -> bool {
    agentcore_id_parts(value).is_some_and(|(label, suffix)| {
        label.len() <= 48
            && suffix.chars().all(|c| c.is_ascii_digit() || c.is_ascii_lowercase())
            && !label.starts_with('-')
            && !label.ends_with('-')
            && !label.contains("--")
            && label
                .chars()
                .all(|c| c.is_ascii_digit() || c.is_ascii_lowercase() || c == '-')
    })
}

/// Whether `value` is the ARN of an AgentCore resource of type `kind` whose id satisfies
/// `id_form` — `arn:aws:bedrock-agentcore:<region>:<account>:<kind>/<id>`, the form every
/// `arnReference` in the AgentCore control-plane model publishes. `pub(crate)` for the
/// same reason as [`agentcore_id_parts`].
pub(crate) fn is_agentcore_arn(value: &str, kind: &str, id_form: fn(&str) -> bool) -> bool {
    value
        .strip_prefix(&format!(
            "arn:aws:bedrock-agentcore:{REGION}:{ACCOUNT_ID}:{kind}/"
        ))
        .is_some_and(id_form)
}

/// One row of the outputs table: a template, the parameters the create supplies, and
/// what each declared output must report.
struct OutputCase {
    case: &'static str,
    stack_name: &'static str,
    template: String,
    parameters: Vec<(String, String)>,
    expected: Vec<(&'static str, Expected)>,
}

fn params(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .enumerate()
        .flat_map(|(index, (key, value))| {
            let position = index + 1;
            vec![
                member(&format!("Parameters.member.{position}.ParameterKey"), key),
                member(
                    &format!("Parameters.member.{position}.ParameterValue"),
                    value,
                ),
            ]
        })
        .collect()
}

fn expect(pairs: &[(&'static str, String)]) -> Vec<(&'static str, Expected)> {
    pairs
        .iter()
        .map(|(key, value)| (*key, Expected::Value(value.clone())))
        .collect()
}

fn sns_arn(name: &str) -> String {
    format!("arn:aws:sns:{REGION}:{ACCOUNT_ID}:{name}")
}

/// The ARN `Fn::GetAtt <function>.Arn` reports for a Lambda function, per the "Return
/// values" section of the `AWS::Lambda::Function` resource-type reference.
fn lambda_function_arn(name: &str) -> String {
    format!("arn:aws:lambda:{REGION}:{ACCOUNT_ID}:function:{name}")
}

/// The ARN `Fn::GetAtt <role>.Arn` reports for an IAM role. IAM is a global service, so
/// its ARNs carry no region.
fn iam_role_arn(name: &str) -> String {
    format!("arn:aws:iam::{ACCOUNT_ID}:role/{name}")
}

/// The ARN `Fn::GetAtt <cluster>.DBClusterArn` reports for an Amazon RDS DB cluster, per
/// the "Return values" section of the `AWS::RDS::DBCluster` resource-type reference.
fn rds_cluster_arn(identifier: &str) -> String {
    format!("arn:aws:rds:{REGION}:{ACCOUNT_ID}:cluster:{identifier}")
}

/// The ARN `Fn::GetAtt <instance>.DBInstanceArn` reports for an Amazon RDS DB instance.
fn rds_instance_arn(identifier: &str) -> String {
    format!("arn:aws:rds:{REGION}:{ACCOUNT_ID}:db:{identifier}")
}

/// Whether `value` is an Amazon RDS endpoint DNS name of the published form
/// `<identifier><infix><service-assigned label>.<region>.rds.amazonaws.com`: the cluster
/// endpoint's infix is `.cluster-`, the reader endpoint's `.cluster-ro-` and a DB
/// instance's a plain `.` ("Amazon Aurora connection management" / "Finding the connection
/// information for an RDS DB instance").
///
/// The label is SERVICE-ASSIGNED — for a DB instance the stack really creates it is the one
/// Amazon RDS minted and reported, not a value CloudFormation computed — so the only thing
/// asserted about it is the alphabet AWS's own endpoints use: lowercase alphanumeric.
fn is_rds_endpoint(value: &str, identifier: &str, infix: &str) -> bool {
    let suffix = format!(".{REGION}.rds.amazonaws.com");
    value
        .strip_prefix(&format!("{identifier}{infix}"))
        .and_then(|rest| rest.strip_suffix(&suffix))
        .is_some_and(|label| {
            !label.is_empty()
                && label
                    .chars()
                    .all(|c| c.is_ascii_digit() || c.is_ascii_lowercase())
        })
}

/// Whether `value` is an Amazon RDS immutable resource id: `prefix-` and 26 UPPERCASE
/// characters, the form `DbClusterResourceId` and `DbiResourceId` publish.
fn is_rds_resource_id(value: &str, prefix: &str) -> bool {
    value
        .strip_prefix(&format!("{prefix}-"))
        .is_some_and(|id| {
            id.len() == 26
                && id
                    .chars()
                    .all(|c| c.is_ascii_digit() || c.is_ascii_uppercase())
        })
}

/// Whether `value` is the ARN of the Secrets Manager secret named `name`:
/// `arn:aws:secretsmanager:<region>:<account>:secret:<name>-<six characters Secrets
/// Manager assigns>`.
///
/// The suffix is the OWNING service's: a stack's secret is created in Secrets Manager and
/// the ARN the stack publishes is the one that create answered with, drawn from that
/// service's own suffix alphabet (ASCII letters and digits, either case).
fn is_secret_arn(value: &str, name: &str) -> bool {
    value
        .strip_prefix(&format!(
            "arn:aws:secretsmanager:{REGION}:{ACCOUNT_ID}:secret:{name}-"
        ))
        .is_some_and(|suffix| {
            suffix.len() == 6 && suffix.chars().all(|character| character.is_ascii_alphanumeric())
        })
}

/// Whether `value` is an Amazon EC2 security group id: `sg-` and 17 lowercase hexadecimal
/// characters, the current (long) id form EC2 publishes.
fn is_security_group_id(value: &str) -> bool {
    value
        .strip_prefix("sg-")
        .is_some_and(|id| id.len() == 17 && id.chars().all(is_lower_hex))
}

/// The class the rule covers. Each row differs from the others along what the rule
/// generalises over: the template's spelling, the intrinsic function the output uses, the
/// resource type the identifier comes from, and how many outputs are declared.
fn output_cases() -> Vec<OutputCase> {
    vec![
        OutputCase {
            // The issue's own case, in the YAML spelling it was recorded in.
            case: "yaml, an sns topic named by a parameter, output is !Ref of the topic",
            stack_name: "probe-yaml",
            template: concat!(
                "Parameters:\n",
                "  TopicName:\n",
                "    Type: String\n",
                "Resources:\n",
                "  ProbeTopic:\n",
                "    Type: AWS::SNS::Topic\n",
                "    Properties:\n",
                "      TopicName: !Ref TopicName\n",
                "Outputs:\n",
                "  TopicArn:\n",
                "    Value: !Ref ProbeTopic\n",
            )
            .to_string(),
            parameters: params(&[("TopicName", "probe-topic-yaml")]),
            expected: expect(&[("TopicArn", sns_arn("probe-topic-yaml"))]),
        },
        OutputCase {
            case: "json, the same template with long-form Ref",
            stack_name: "probe-json",
            template: r#"{"Parameters":{"TopicName":{"Type":"String"}},
                 "Resources":{"ProbeTopic":{"Type":"AWS::SNS::Topic",
                   "Properties":{"TopicName":{"Ref":"TopicName"}}}},
                 "Outputs":{"TopicArn":{"Value":{"Ref":"ProbeTopic"}}}}"#
                .to_string(),
            parameters: params(&[("TopicName", "probe-topic-json")]),
            expected: expect(&[("TopicArn", sns_arn("probe-topic-json"))]),
        },
        OutputCase {
            case: "yaml, a topic name taken from the template's own parameter Default",
            stack_name: "probe-default",
            template: concat!(
                "Parameters:\n",
                "  TopicName:\n",
                "    Type: String\n",
                "    Default: defaulted-topic\n",
                "Resources:\n",
                "  T:\n",
                "    Type: AWS::SNS::Topic\n",
                "    Properties:\n",
                "      TopicName: !Ref TopicName\n",
                "Outputs:\n",
                "  TopicArn:\n",
                "    Value: !Ref T\n",
            )
            .to_string(),
            parameters: vec![],
            expected: expect(&[("TopicArn", sns_arn("defaulted-topic"))]),
        },
        OutputCase {
            case: "Fn::GetAtt in both spellings, on a topic's two attributes",
            stack_name: "getatt",
            template: concat!(
                "Resources:\n",
                "  T:\n",
                "    Type: AWS::SNS::Topic\n",
                "    Properties:\n",
                "      TopicName: attributed-topic\n",
                "Outputs:\n",
                "  ShortForm:\n",
                "    Value: !GetAtt T.TopicArn\n",
                "  LongForm:\n",
                "    Value:\n",
                "      Fn::GetAtt: [T, TopicName]\n",
            )
            .to_string(),
            parameters: vec![],
            expected: expect(&[
                ("ShortForm", sns_arn("attributed-topic")),
                ("LongForm", "attributed-topic".to_string()),
            ]),
        },
        OutputCase {
            // A log group's Ref is documented as its NAME while the ARN it publishes ends
            // in `:*`: the rule is the resource type's documented return value, not "an
            // ARN".
            case: "a log group, whose Ref is the name and whose Arn ends in a wildcard",
            stack_name: "logs-conventional",
            template: r#"{"Resources":{"G":{"Type":"AWS::Logs::LogGroup",
                   "Properties":{"LogGroupName":"probe-logs"}}},
                 "Outputs":{"GroupName":{"Value":{"Ref":"G"}},
                   "GroupArn":{"Value":{"Fn::GetAtt":["G","Arn"]}},
                   "GroupConventionalName":{"Value":{"Fn::GetAtt":["G","LogGroupName"]}}}}"#
                .to_string(),
            parameters: vec![],
            expected: expect(&[
                ("GroupName", "probe-logs".to_string()),
                (
                    "GroupArn",
                    format!("arn:aws:logs:{REGION}:{ACCOUNT_ID}:log-group:probe-logs:*"),
                ),
                ("GroupConventionalName", "probe-logs".to_string()),
            ]),
        },
        OutputCase {
            // Two more catalogued types the issue never cited, with the two shapes a
            // documented return value takes: a bare name and a service ARN.
            //
            // The catalogued type whose `Ref` is a URL rather than a name — an
            // `AWS::SQS::Queue`, whose documented `Ref` is the queue url — is asserted
            // where the queue is really created, in
            // `core/router/tests/all/cloudformation_provisions_sqs_queues.rs`: this
            // emulator PROVISIONS that type, and Amazon SQS is not one of the services
            // this crate's harness mounts, so such a stack fails here rather than
            // publishing a url nothing answers to.
            case: "a bucket and a table: Ref is the name, Arn is the attribute",
            stack_name: "storage",
            template: r#"{"Resources":{"B":{"Type":"AWS::S3::Bucket",
                   "Properties":{"BucketName":"probe-bucket"}},
                   "D":{"Type":"AWS::DynamoDB::Table","Properties":{
                     "TableName":"probe-table","BillingMode":"PAY_PER_REQUEST",
                     "KeySchema":[{"AttributeName":"pk","KeyType":"HASH"}],
                     "AttributeDefinitions":[{"AttributeName":"pk","AttributeType":"S"}]}}},
                 "Outputs":{"BucketName":{"Value":{"Ref":"B"}},
                   "BucketArn":{"Value":{"Fn::GetAtt":["B","Arn"]}},
                   "TableName":{"Value":{"Ref":"D"}},
                   "TableArn":{"Value":{"Fn::GetAtt":["D","Arn"]}}}}"#
                .to_string(),
            parameters: vec![],
            expected: expect(&[
                ("BucketName", "probe-bucket".to_string()),
                ("BucketArn", "arn:aws:s3:::probe-bucket".to_string()),
                ("TableName", "probe-table".to_string()),
                (
                    "TableArn",
                    format!("arn:aws:dynamodb:{REGION}:{ACCOUNT_ID}:table/probe-table"),
                ),
            ]),
        },
        OutputCase {
            // A stack of two Lambda functions — one named by a parameter and tagged, one
            // named outright — whose outputs publish one function's NAME (`Ref`) and the
            // other's ARN (`Fn::GetAtt`), which is the contract between a template and
            // whatever runs after it. `AWS::IAM::Role` rides along as a catalogued type
            // whose ARN carries no region, proving the rule is the resource type's own
            // documented return value rather than one ARN shape.
            case: "two lambda functions and a role: Ref is the name, Arn is the attribute",
            stack_name: "lambda-stack",
            template: concat!(
                "Parameters:\n",
                "  TaggedName:\n",
                "    Type: String\n",
                "Resources:\n",
                "  Tagged:\n",
                "    Type: AWS::Lambda::Function\n",
                "    Properties:\n",
                "      FunctionName: !Ref TaggedName\n",
                "      Handler: index.handler\n",
                "      Runtime: python3.12\n",
                "      Role: !GetAtt ExecutionRole.Arn\n",
                // `Code` is required on the type, and the deployment really creates the
                // function in Lambda, which requires a package too.
                "      Code:\n",
                "        ZipFile: |\n",
                "          def handler(event, context):\n",
                "              return {}\n",
                "      Tags:\n",
                "        - Key: probe\n",
                "          Value: tagged\n",
                "  Untagged:\n",
                "    Type: AWS::Lambda::Function\n",
                "    Properties:\n",
                "      FunctionName: probe-untagged\n",
                "      Handler: index.handler\n",
                "      Runtime: python3.12\n",
                "      Role: !GetAtt ExecutionRole.Arn\n",
                "      Code:\n",
                "        ZipFile: |\n",
                "          def handler(event, context):\n",
                "              return {}\n",
                "  ExecutionRole:\n",
                "    Type: AWS::IAM::Role\n",
                "    Properties:\n",
                "      RoleName: probe-exec-role\n",
                // `AssumeRolePolicyDocument` is required on the type, and the deployment
                // really creates the role in IAM, which requires it too.
                "      AssumeRolePolicyDocument:\n",
                "        Version: \"2012-10-17\"\n",
                "        Statement:\n",
                "          - Effect: Allow\n",
                "            Principal:\n",
                "              Service: lambda.amazonaws.com\n",
                "            Action: sts:AssumeRole\n",
                "Outputs:\n",
                "  TaggedFunctionName:\n",
                "    Value: !Ref Tagged\n",
                "  TaggedFunctionArn:\n",
                "    Value: !GetAtt Tagged.Arn\n",
                "  UntaggedFunctionArn:\n",
                "    Value:\n",
                "      Fn::GetAtt: [Untagged, Arn]\n",
                "  RoleArn:\n",
                "    Value: !GetAtt ExecutionRole.Arn\n",
            )
            .to_string(),
            parameters: params(&[("TaggedName", "probe-tagged")]),
            expected: expect(&[
                ("TaggedFunctionName", "probe-tagged".to_string()),
                ("TaggedFunctionArn", lambda_function_arn("probe-tagged")),
                ("UntaggedFunctionArn", lambda_function_arn("probe-untagged")),
                ("RoleArn", iam_role_arn("probe-exec-role")),
            ]),
        },
        OutputCase {
            // The issue's own CloudFormation case now lives in the router's own suite
            // (see the note on the workflow case below), so what this row carries is the
            // Lambda pair above and the workflow below.
            case: "a transfer workflow: a second declared-only service-assigned id",
            stack_name: "transfer-second-workflow",
            template: concat!(
                "Resources:\n",
                "  Flow:\n",
                "    Type: AWS::Transfer::Workflow\n",
                "Outputs:\n",
                "  WorkflowId:\n",
                "    Value: !GetAtt Flow.WorkflowId\n",
                "  WorkflowRef:\n",
                "    Value: !Ref Flow\n",
            )
            .to_string(),
            parameters: vec![],
            expected: vec![
                (
                    "WorkflowId",
                    Expected::Form {
                        described: "a WorkflowId of the form w- + 17 of [a-z0-9]",
                        check: is_transfer_workflow_id,
                    },
                ),
                (
                    // "Ref returns the Amazon Resource Name (ARN)" for this type too.
                    "WorkflowRef",
                    Expected::Form {
                        described: "a transfer workflow ARN",
                        check: |value| is_transfer_arn(value, "workflow", is_transfer_workflow_id),
                    },
                ),
            ],
        },
        OutputCase {
            // The same rule on a type whose owning service this router does not mount:
            // a workflow id is `w-` + 17 of `[a-z0-9]`, so the FORM differs while the
            // rule does not. The `AWS::Transfer::Server` case this suite used to carry
            // moved to `core/router/tests/all/cloudformation_provisions_transfer_servers.rs`
            // when that type gained a provisioner: a stack that declares it now CALLS
            // AWS Transfer Family, which only a router with both services mounted can
            // serve — and that suite asserts the published id against the server the
            // service really created, which is stronger than a form check.
            case: "a transfer workflow: its own service-assigned id form",
            stack_name: "transfer-workflow",
            template: r#"{"Resources":{"Flow":{"Type":"AWS::Transfer::Workflow",
                   "Properties":{"Steps":[]}}},
                 "Outputs":{"WorkflowId":{"Value":{"Fn::GetAtt":["Flow","WorkflowId"]}},
                   "WorkflowArn":{"Value":{"Fn::GetAtt":["Flow","Arn"]}}}}"#
                .to_string(),
            parameters: vec![],
            expected: vec![
                (
                    "WorkflowId",
                    Expected::Form {
                        described: "a WorkflowId of the form w- + 17 of [a-z0-9]",
                        check: is_transfer_workflow_id,
                    },
                ),
                (
                    "WorkflowArn",
                    Expected::Form {
                        described: "a transfer workflow ARN",
                        check: |value| {
                            is_transfer_arn(value, "workflow", is_transfer_workflow_id)
                        },
                    },
                ),
            ],
        },
        OutputCase {
            // The AgentCore deployment case: a memory whose identifier the OWNING SERVICE
            // assigns, so the rule is the documented form.
            //
            // The `AWS::BedrockAgentCore::WorkloadIdentity` resource this row used to
            // declare beside it moved to
            // `core/router/tests/all/cloudformation_provisions_agentcore_workload_identities.rs`
            // when that type gained a provisioner, for the same reason the
            // `AWS::Transfer::Server` case above did: a stack that declares it now CALLS
            // the AgentCore control plane, which only a router with both services mounted
            // can serve — and that suite asserts the published name and ARN against the
            // identity the service really created, which is stronger than a form check.
            case: "an agentcore memory: the id its service assigns",
            stack_name: "agentcore-memory",
            template: concat!(
                "Resources:\n",
                "  Memory:\n",
                "    Type: AWS::BedrockAgentCore::Memory\n",
                "    Properties:\n",
                "      EventExpiryDuration: 30\n",
                "Outputs:\n",
                "  MemoryId:\n",
                "    Value: !GetAtt Memory.MemoryId\n",
                "  MemoryArn:\n",
                "    Value:\n",
                "      Fn::GetAtt: [Memory, MemoryArn]\n",
                "  MemoryRef:\n",
                "    Value: !Ref Memory\n",
            )
            .to_string(),
            parameters: vec![],
            expected: vec![
                (
                    "MemoryId",
                    Expected::Form {
                        described: "a MemoryId of the form <label>- + 10 characters",
                        check: is_agentcore_memory_id,
                    },
                ),
                (
                    "MemoryArn",
                    Expected::Form {
                        described: "an agentcore memory ARN",
                        check: |value| is_agentcore_arn(value, "memory", is_agentcore_memory_id),
                    },
                ),
                (
                    // The type's primary identifier is its memory id, so that is what
                    // `Ref` answers.
                    "MemoryRef",
                    Expected::Form {
                        described: "a MemoryId",
                        check: is_agentcore_memory_id,
                    },
                ),
            ],
        },
        OutputCase {
            // The same rule two types along, where each identifier's documented ALPHABET
            // differs: a runtime label carries no `-` and a gateway's is lower case only.
            case: "an agentcore runtime and gateway: each id in its own documented form",
            stack_name: "AgentCoreRuntime",
            template: r#"{"Resources":{
                   "Runtime":{"Type":"AWS::BedrockAgentCore::Runtime",
                     "Properties":{"AgentRuntimeName":"probe_runtime",
                       "RoleArn":"arn:aws:iam::000000000000:role/AgentRole"}},
                   "Gateway":{"Type":"AWS::BedrockAgentCore::Gateway",
                     "Properties":{"Name":"Probe-Gateway","RoleArn":"arn:aws:iam::000000000000:role/GatewayRole"}}},
                 "Outputs":{
                   "RuntimeId":{"Value":{"Fn::GetAtt":["Runtime","AgentRuntimeId"]}},
                   "RuntimeArn":{"Value":{"Fn::GetAtt":["Runtime","AgentRuntimeArn"]}},
                   "RuntimeVersion":{"Value":{"Fn::GetAtt":["Runtime","AgentRuntimeVersion"]}},
                   "GatewayId":{"Value":{"Fn::GetAtt":["Gateway","GatewayId"]}},
                   "GatewayArn":{"Value":{"Fn::GetAtt":["Gateway","GatewayArn"]}},
                   "GatewayRef":{"Value":{"Ref":"Gateway"}}}}"#
                .to_string(),
            parameters: vec![],
            expected: vec![
                (
                    "RuntimeId",
                    Expected::Form {
                        described: "an AgentRuntimeId whose label carries no '-'",
                        check: is_agentcore_runtime_id,
                    },
                ),
                (
                    "RuntimeArn",
                    Expected::Form {
                        described: "an agentcore runtime ARN",
                        check: |value| is_agentcore_arn(value, "runtime", is_agentcore_runtime_id),
                    },
                ),
                (
                    // "The version of the AgentCore Runtime" — a freshly declared runtime
                    // has its first.
                    "RuntimeVersion",
                    Expected::Value("1".to_string()),
                ),
                (
                    "GatewayId",
                    Expected::Form {
                        described: "a lower-case GatewayId",
                        check: is_agentcore_gateway_id,
                    },
                ),
                (
                    "GatewayArn",
                    Expected::Form {
                        described: "an agentcore gateway ARN",
                        check: |value| is_agentcore_arn(value, "gateway", is_agentcore_gateway_id),
                    },
                ),
                (
                    "GatewayRef",
                    Expected::Form {
                        described: "a GatewayId",
                        check: is_agentcore_gateway_id,
                    },
                ),
            ],
        },
        OutputCase {
            // The same rule on the type a CFN-driven RUM deployment reads its id from:
            // "Ref returns the app monitor name", and `Fn::GetAtt: Id` is "the unique ID
            // of the new app monitor" — an id RUM ASSIGNS, so the rule is its documented
            // UUID form and not a literal any request fixes.
            case: "a rum app monitor: !GetAtt of the id its service assigns, and its name",
            stack_name: "rum-app-monitor",
            template: concat!(
                "Resources:\n",
                "  AppMonitor:\n",
                "    Type: AWS::RUM::AppMonitor\n",
                "    Properties:\n",
                "      Name: probe-monitor\n",
                "      Domain: localhost\n",
                "      CwLogEnabled: false\n",
                "Outputs:\n",
                "  AppMonitorId:\n",
                "    Value: !GetAtt AppMonitor.Id\n",
                "  AppMonitorName:\n",
                "    Value: !Ref AppMonitor\n",
            )
            .to_string(),
            parameters: vec![],
            expected: vec![
                (
                    "AppMonitorId",
                    Expected::Form {
                        described: "an AppMonitorId of the modelled 36-character UUID form",
                        check: is_app_monitor_id,
                    },
                ),
                (
                    "AppMonitorName",
                    Expected::Value("probe-monitor".to_string()),
                ),
            ],
        },
        OutputCase {
            // A catalogued type whose name the TEMPLATE fixes, for contrast: `Ref` is that
            // name and the ARN is built from it, and a log-group ARN ends in `:*`.
            case: "a log group: Ref is the declared name, Arn ends in :*",
            stack_name: "logs",
            template: r#"{"Resources":{"G":{"Type":"AWS::Logs::LogGroup",
                   "Properties":{"LogGroupName":"/probe/group"}}},
                 "Outputs":{"GroupName":{"Value":{"Ref":"G"}},
                   "GroupArn":{"Value":{"Fn::GetAtt":["G","Arn"]}}}}"#
                .to_string(),
            parameters: vec![],
            expected: expect(&[
                ("GroupName", "/probe/group".to_string()),
                (
                    "GroupArn",
                    format!("arn:aws:logs:{REGION}:{ACCOUNT_ID}:log-group:/probe/group:*"),
                ),
            ]),
        },
        OutputCase {
            // A type with no return-value catalogue entry, which no owning service here
            // provisions: `Ref` answers the physical name — the name the template declared
            // — and `Fn::GetAtt Arn` answers the resource's ARN under CloudFormation's
            // uniform naming (the type's service segment as the ARN namespace, its type
            // segment as the resource path), rather than leaving a declared output of a
            // CREATE_COMPLETE stack valueless. (The EventBridge Scheduler types this row
            // used to be written with are catalogued and provisioned now, so they are
            // exercised end to end against that service in
            // `core/router/tests/all/cloudformation_provisions_scheduler_schedules.rs`.)
            case: "an uncatalogued type: Ref is its physical name, GetAtt Arn its arn",
            stack_name: "uncatalogued",
            template: r#"{"Resources":{
                   "G":{"Type":"AWS::MemoryDB::ParameterGroup",
                     "Properties":{"ParameterGroupName":"probe-group"}},
                   "S":{"Type":"AWS::MemoryDB::Cluster","Properties":{"ClusterName":"probe-cluster",
                     "ParameterGroupName":{"Ref":"G"}}}},
                 "Outputs":{"GroupName":{"Value":{"Ref":"G"}},
                   "ClusterName":{"Value":{"Ref":"S"}},
                   "ClusterParameterGroup":{"Value":{"Fn::GetAtt":["S","ParameterGroupName"]}},
                   "ClusterArn":{"Value":{"Fn::GetAtt":["S","Arn"]}}}}"#
                .to_string(),
            parameters: vec![],
            expected: vec![
                ("GroupName", Expected::Value("probe-group".to_string())),
                ("ClusterName", Expected::Value("probe-cluster".to_string())),
                // The attribute IS the property the template declared for it, and that
                // property names another resource of the same template.
                (
                    "ClusterParameterGroup",
                    Expected::Value("probe-group".to_string()),
                ),
                (
                    "ClusterArn",
                    Expected::Value(format!(
                        "arn:aws:memorydb:{REGION}:{ACCOUNT_ID}:cluster/probe-cluster"
                    )),
                ),
            ],
        },
        OutputCase {
            case: "the pseudo parameters a Ref resolves without any resource",
            stack_name: "pseudo",
            template: concat!(
                "Outputs:\n",
                "  Region:\n",
                "    Value: !Ref AWS::Region\n",
                "  Account:\n",
                "    Value: !Ref AWS::AccountId\n",
                "  Name:\n",
                "    Value: !Ref AWS::StackName\n",
                "  Partition:\n",
                "    Value: !Ref AWS::Partition\n",
                "  Suffix:\n",
                "    Value: !Ref AWS::URLSuffix\n",
            )
            .to_string(),
            parameters: vec![],
            expected: expect(&[
                ("Region", REGION.to_string()),
                ("Account", ACCOUNT_ID.to_string()),
                ("Name", "pseudo".to_string()),
                ("Partition", "aws".to_string()),
                ("Suffix", "amazonaws.com".to_string()),
            ]),
        },
        OutputCase {
            case: "Fn::Join and Fn::Sub over a parameter, an attribute and a literal",
            stack_name: "composed",
            template: concat!(
                "Parameters:\n",
                "  Stage:\n",
                "    Type: String\n",
                "Resources:\n",
                "  T:\n",
                "    Type: AWS::SNS::Topic\n",
                "    Properties:\n",
                "      TopicName: !Sub \"${Stage}-events\"\n",
                "Outputs:\n",
                "  Joined:\n",
                "    Value: !Join [\"-\", [!Ref Stage, !Ref \"AWS::Region\", !GetAtt T.TopicName]]\n",
                "  Substituted:\n",
                "    Value: !Sub \"${Stage}/${T.TopicArn}/${!Literal}\"\n",
            )
            .to_string(),
            parameters: params(&[("Stage", "prod")]),
            expected: expect(&[
                ("Joined", format!("prod-{REGION}-prod-events")),
                (
                    "Substituted",
                    format!("prod/{}/${{Literal}}", sns_arn("prod-events")),
                ),
            ]),
        },
        OutputCase {
            case: "Fn::Select over Fn::Split, and Fn::FindInMap over the Mappings section",
            stack_name: "listish",
            template: r#"{"Mappings":{"Sizes":{"small":{"Queue":"probe-small"}}},
                 "Outputs":{"Middle":{"Value":{"Fn::Select":["1",{"Fn::Split":[",","a,b,c"]}]}},
                   "Mapped":{"Value":{"Fn::FindInMap":["Sizes","small","Queue"]}}}}"#
                .to_string(),
            parameters: vec![],
            expected: expect(&[
                ("Middle", "b".to_string()),
                ("Mapped", "probe-small".to_string()),
            ]),
        },
        OutputCase {
            case: "an output declaring a description and an export name",
            stack_name: "exported",
            template: concat!(
                "Resources:\n",
                "  T:\n",
                "    Type: AWS::SNS::Topic\n",
                "    Properties:\n",
                "      TopicName: exported-topic\n",
                "Outputs:\n",
                "  TopicArn:\n",
                "    Description: the probe topic\n",
                "    Value: !Ref T\n",
                "    Export:\n",
                "      Name: !Sub \"${AWS::StackName}-TopicArn\"\n",
            )
            .to_string(),
            parameters: vec![],
            expected: expect(&[("TopicArn", sns_arn("exported-topic"))]),
        },
        OutputCase {
            // The count dimension: ten declared outputs, every one reported.
            case: "ten declared outputs",
            stack_name: "many",
            template: {
                let mut body = String::from("Outputs:\n");
                for index in 1..=10 {
                    body.push_str(&format!(
                        "  Out{index}:\n    Value: !Join [\"-\", [\"o\", \"{index}\"]]\n"
                    ));
                }
                body
            },
            parameters: vec![],
            expected: (1..=10)
                .map(|index| {
                    let key: &'static str = [
                        "Out1", "Out2", "Out3", "Out4", "Out5", "Out6", "Out7", "Out8", "Out9",
                        "Out10",
                    ][index - 1];
                    (key, Expected::Value(format!("o-{index}")))
                })
                .collect(),
        },
        OutputCase {
            // The topology the Lambda + Amazon RDS + Secrets Manager issue deploys: a
            // function reading its database credentials from a secret and querying an
            // Aurora Serverless v2 cluster. Every identifier the caller needs afterwards
            // is an output, and each comes from the "Return values" section of its own
            // resource type — `Ref` for the cluster is its DBClusterIdentifier, for the
            // secret the secret's ARN, and `Fn::GetAtt` reads the cluster's ARN, resource
            // id and endpoints.
            case: "a lambda, a secret and an aurora cluster: every declared output resolves",
            stack_name: "serverless-app",
            template: concat!(
                "Parameters:\n",
                "  ClusterIdentifier:\n",
                "    Type: String\n",
                "Resources:\n",
                "  ExecutionRole:\n",
                "    Type: AWS::IAM::Role\n",
                "    Properties:\n",
                "      RoleName: probe-app-role\n",
                "      AssumeRolePolicyDocument:\n",
                "        Version: \"2012-10-17\"\n",
                "        Statement:\n",
                "          - Effect: Allow\n",
                "            Principal:\n",
                // The function RUNS AS this role, so its trust policy names the principal
                // Lambda assumes it as — which that service resolves before it creates the
                // function.
                "              Service: lambda.amazonaws.com\n",
                "            Action: sts:AssumeRole\n",
                "  DBSecret:\n",
                "    Type: AWS::SecretsManager::Secret\n",
                "    Properties:\n",
                "      Name: probe-app-secret\n",
                "  DBCluster:\n",
                "    Type: AWS::RDS::DBCluster\n",
                "    Properties:\n",
                "      DBClusterIdentifier: !Ref ClusterIdentifier\n",
                "      Engine: aurora-postgresql\n",
                "      EnableHttpEndpoint: true\n",
                "  AppFunction:\n",
                "    Type: AWS::Lambda::Function\n",
                "    Properties:\n",
                "      FunctionName: probe-app-fn\n",
                "      Handler: index.handler\n",
                "      Runtime: python3.12\n",
                "      Role: !GetAtt ExecutionRole.Arn\n",
                "      Code:\n",
                "        ZipFile: |\n",
                "          def handler(event, context):\n",
                "              return {}\n",
                "Outputs:\n",
                "  FunctionName:\n",
                "    Value: !Ref AppFunction\n",
                "  FunctionArn:\n",
                "    Value: !GetAtt AppFunction.Arn\n",
                "  SecretArn:\n",
                "    Value: !Ref DBSecret\n",
                "  SecretId:\n",
                "    Value: !GetAtt DBSecret.Id\n",
                "  ClusterRef:\n",
                "    Value: !Ref DBCluster\n",
                "  ClusterArn:\n",
                "    Value: !GetAtt DBCluster.DBClusterArn\n",
                "  ClusterResourceId:\n",
                "    Value: !GetAtt DBCluster.DBClusterResourceId\n",
                "  ClusterEndpoint:\n",
                "    Value: !GetAtt DBCluster.Endpoint.Address\n",
                "  ClusterReaderEndpoint:\n",
                "    Value: !GetAtt DBCluster.ReadEndpoint.Address\n",
                "  ClusterPort:\n",
                "    Value: !GetAtt DBCluster.Endpoint.Port\n",
            )
            .to_string(),
            parameters: params(&[("ClusterIdentifier", "probe-app-cluster")]),
            expected: vec![
                (
                    "FunctionName",
                    Expected::Value("probe-app-fn".to_string()),
                ),
                (
                    "FunctionArn",
                    Expected::Value(lambda_function_arn("probe-app-fn")),
                ),
                (
                    // "Ref returns the ARN of the secret."
                    "SecretArn",
                    Expected::Form {
                        described: "the ARN of the secret, with the suffix its service \
                                    assigns",
                        check: |value| is_secret_arn(value, "probe-app-secret"),
                    },
                ),
                (
                    "SecretId",
                    Expected::Form {
                        described: "the ARN of the secret",
                        check: |value| is_secret_arn(value, "probe-app-secret"),
                    },
                ),
                (
                    // "Ref returns the DBClusterIdentifier" — here taken from a parameter
                    // through the type's own `Identifier` name property.
                    "ClusterRef",
                    Expected::Value("probe-app-cluster".to_string()),
                ),
                (
                    "ClusterArn",
                    Expected::Value(rds_cluster_arn("probe-app-cluster")),
                ),
                (
                    "ClusterResourceId",
                    Expected::Form {
                        described: "a DbClusterResourceId of the form cluster- + 26 \
                                    uppercase characters",
                        check: |value| is_rds_resource_id(value, "cluster"),
                    },
                ),
                (
                    "ClusterEndpoint",
                    Expected::Form {
                        described: "the Aurora cluster endpoint DNS name",
                        check: |value| {
                            is_rds_endpoint(value, "probe-app-cluster", ".cluster-")
                        },
                    },
                ),
                (
                    "ClusterReaderEndpoint",
                    Expected::Form {
                        described: "the Aurora reader endpoint DNS name",
                        check: |value| {
                            is_rds_endpoint(value, "probe-app-cluster", ".cluster-ro-")
                        },
                    },
                ),
                (
                    // The engine's documented default port, since the template declares
                    // none: "RDS for PostgreSQL - 5432".
                    "ClusterPort",
                    Expected::Value("5432".to_string()),
                ),
            ],
        },
        OutputCase {
            // The same rule two types along, in the JSON spelling, on inputs the row above
            // does not cover: a DB INSTANCE (whose endpoint carries no `cluster-` infix)
            // with the port DECLARED rather than defaulted, and a VPC security group,
            // whose `Ref` is the id its service assigns rather than a name.
            //
            // The engine is one the RDS emulator serves no engine listener for, so its
            // endpoint is the DNS-form name and its port is the DECLARED one, which is
            // what this row reads back. The row below it is the same template on an
            // engine that IS served, where the published endpoint is the listener's.
            case: "a db instance and a security group: ids and an explicit port",
            stack_name: "db-instance",
            template: r#"{"Resources":{
                   "Database":{"Type":"AWS::RDS::DBInstance",
                     "Properties":{"DBInstanceIdentifier":"probe-db",
                       "Engine":"oracle-se2","Port":"11521",
                       "DBInstanceClass":"db.t4g.micro"}},
                   "ClientGroup":{"Type":"AWS::EC2::SecurityGroup",
                     "Properties":{"GroupDescription":"probe"}}},
                 "Outputs":{"InstanceRef":{"Value":{"Ref":"Database"}},
                   "InstanceArn":{"Value":{"Fn::GetAtt":["Database","DBInstanceArn"]}},
                   "InstanceResourceId":{"Value":{"Fn::GetAtt":["Database","DbiResourceId"]}},
                   "InstanceEndpoint":{"Value":{"Fn::GetAtt":["Database","Endpoint.Address"]}},
                   "InstancePort":{"Value":{"Fn::GetAtt":["Database","Endpoint.Port"]}},
                   "GroupRef":{"Value":{"Ref":"ClientGroup"}},
                   "GroupId":{"Value":{"Fn::GetAtt":["ClientGroup","GroupId"]}}}}"#
                .to_string(),
            parameters: vec![],
            expected: vec![
                ("InstanceRef", Expected::Value("probe-db".to_string())),
                (
                    "InstanceArn",
                    Expected::Value(rds_instance_arn("probe-db")),
                ),
                (
                    "InstanceResourceId",
                    Expected::Form {
                        described: "a DbiResourceId of the form db- + 26 uppercase \
                                    characters",
                        check: |value| is_rds_resource_id(value, "db"),
                    },
                ),
                (
                    "InstanceEndpoint",
                    Expected::Form {
                        described: "the DB instance endpoint DNS name",
                        check: |value| is_rds_endpoint(value, "probe-db", "."),
                    },
                ),
                (
                    // The port the template DECLARED, not the engine's default (1521 for
                    // RDS for Oracle).
                    "InstancePort",
                    Expected::Value("11521".to_string()),
                ),
                (
                    // "Ref returns the ID of the security group."
                    "GroupRef",
                    Expected::Form {
                        described: "a security group id of the form sg- + 17 hexadecimal \
                                    characters",
                        check: is_security_group_id,
                    },
                ),
                (
                    "GroupId",
                    Expected::Form {
                        described: "a security group id",
                        check: is_security_group_id,
                    },
                ),
            ],
        },
        OutputCase {
            // The SAME template on an engine the RDS emulator DOES serve a native wire
            // for: the DB instance is provisioned with its own engine listener, so the
            // endpoint it publishes — and so the output `Endpoint.Address`/`Endpoint.Port`
            // carry — is that listener's loopback host and the port the core assigned it,
            // which is the address a client can reach. (That the endpoint ANSWERS there is
            // pinned in the RDS crate's own `db_instance_engine_endpoint` suite.) The
            // declared `Port` is still recorded by the instance; the row above reads it
            // back on an engine with no listener.
            case: "a db instance whose engine this emulator serves publishes its listener",
            stack_name: "db-instance-served",
            template: r#"{"Resources":{
                   "Database":{"Type":"AWS::RDS::DBInstance",
                     "Properties":{"DBInstanceIdentifier":"probe-served-db",
                       "Engine":"mysql","Port":"13306",
                       "DBInstanceClass":"db.t4g.micro"}}},
                 "Outputs":{
                   "InstanceEndpoint":{"Value":{"Fn::GetAtt":["Database","Endpoint.Address"]}},
                   "InstancePort":{"Value":{"Fn::GetAtt":["Database","Endpoint.Port"]}}}}"#
                .to_string(),
            parameters: vec![],
            expected: vec![
                (
                    "InstanceEndpoint",
                    Expected::Form {
                        described: "the loopback host the instance's engine listener is \
                                    bound to",
                        check: |value| value == "127.0.0.1",
                    },
                ),
                (
                    "InstancePort",
                    Expected::Form {
                        described: "the port that engine listener was assigned",
                        check: |value| value.parse::<u16>().is_ok_and(|port| port > 0),
                    },
                ),
            ],
        },
        OutputCase {
            // The port and the engine of a DB resource are what the TEMPLATE declares, and
            // a template declares them with an intrinsic as readily as with a literal: this
            // row takes both from parameters, so `Endpoint.Port` can only be right if the
            // declared properties were evaluated. The last resource's engine resolves to
            // nothing (a `Ref` to the pseudo parameter that stands for no value), so its
            // port is reported with no value rather than defaulted to one engine's. Every
            // resource
            // of this row is a DB CLUSTER, a type this emulator records rather than
            // provisions, so the row stays about evaluating the declared properties: an
            // engine that resolves to nothing is not a create Amazon RDS would accept.
            case: "a db port and engine declared through intrinsics, and an unresolvable engine",
            stack_name: "db-intrinsics",
            template: concat!(
                "Parameters:\n",
                "  DBPort:\n",
                "    Type: String\n",
                "  EngineName:\n",
                "    Type: String\n",
                "Resources:\n",
                "  FixedPortCluster:\n",
                "    Type: AWS::RDS::DBCluster\n",
                "    Properties:\n",
                "      DBClusterIdentifier: probe-fixed-port\n",
                "      Engine: !Ref EngineName\n",
                "      Port: !Ref DBPort\n",
                "  DefaultPortCluster:\n",
                "    Type: AWS::RDS::DBCluster\n",
                "    Properties:\n",
                "      DBClusterIdentifier: probe-default-port\n",
                "      Engine: !Ref EngineName\n",
                "  UnknownEngineDatabase:\n",
                "    Type: AWS::RDS::DBCluster\n",
                "    Properties:\n",
                "      DBClusterIdentifier: probe-unknown-engine\n",
                // The pseudo parameter that resolves to NO value at all
                // ("AWS::NoValue … removes the corresponding resource property"), which is
                // what makes this engine unresolvable — a declared parameter cannot be
                // left without a value, since a declaration with no `Default` the request
                // supplies no value for is refused outright.
                "      Engine: !Ref AWS::NoValue\n",
                "Outputs:\n",
                "  FixedPort:\n",
                "    Value: !GetAtt FixedPortCluster.Endpoint.Port\n",
                "  DefaultPort:\n",
                "    Value: !GetAtt DefaultPortCluster.Endpoint.Port\n",
                "  UnknownEnginePort:\n",
                "    Value: !GetAtt UnknownEngineDatabase.Endpoint.Port\n",
            )
            .to_string(),
            parameters: params(&[("DBPort", "15432"), ("EngineName", "aurora-postgresql")]),
            expected: vec![
                // The port the template declared, through a parameter.
                ("FixedPort", Expected::Value("15432".to_string())),
                // No port declared: the default of the engine the parameter named, which
                // is only reachable if the engine itself resolved.
                ("DefaultPort", Expected::Value("5432".to_string())),
                // The engine resolves to nothing, so the port does too.
                ("UnknownEnginePort", Expected::NoValue),
            ],
        },
        OutputCase {
            case: "outputs this service cannot evaluate are reported without a value",
            stack_name: "unresolved",
            // Two expressions that resolve to no value: a `Fn::Select` past the end of the
            // list it selects from, and a `Ref` to the pseudo parameter that stands for no
            // value. (An `Fn::ImportValue` of a name nothing exports is NOT one of them — it
            // fails the whole deployment, which `unresolvable_imports.rs` pins.)
            template: r#"{"Outputs":{"Selected":{"Value":{"Fn::Select":["3",["a","b"]]}},
                   "Missing":{"Value":{"Ref":"AWS::NoValue"}}}}"#
                .to_string(),
            parameters: vec![],
            expected: vec![
                ("Selected", Expected::NoValue),
                ("Missing", Expected::NoValue),
            ],
        },
    ]
}

#[tokio::test]
async fn declared_outputs_are_evaluated_and_reported_by_describe_stacks() {
    let router = router();

    for case in output_cases() {
        // A row's template may declare the IAM role its other resources use, so the
        // request acknowledges the template's IAM resources.
        let (status_code, body) = create_stack(
            &router,
            case.stack_name,
            &case.template,
            &acknowledging_iam(&case.parameters),
        )
        .await;
        assert_eq!(
            status_code, 200,
            "case {:?}: expected CreateStack to succeed, got {status_code} {body}",
            case.case
        );
        wait_for_the_transition().await;

        let (status_code, body) = describe_stack(&router, case.stack_name).await;
        assert_eq!(
            status_code, 200,
            "case {:?}: expected DescribeStacks to succeed, got {status_code} {body}",
            case.case
        );
        assert_eq!(
            xml_text(&body, "StackStatus").as_deref(),
            Some(status::CREATE_COMPLETE),
            "case {:?}: the stack must have reached CREATE_COMPLETE, got {body}",
            case.case
        );

        let outputs = wire_outputs(&body);
        assert_eq!(
            outputs.len(),
            case.expected.len(),
            "case {:?}: expected one Output per declared output ({} of them), got {outputs:?} \
             from {body}",
            case.case,
            case.expected.len()
        );
        for (key, expected) in &case.expected {
            let Some(output) = output_for(&outputs, key) else {
                panic!(
                    "case {:?}: no Output with OutputKey {key:?}; got {outputs:?}",
                    case.case
                );
            };
            match expected {
                Expected::Value(value) => assert_eq!(
                    output.value.as_deref(),
                    Some(value.as_str()),
                    "case {:?}: output {key:?} must carry the evaluated value {value:?}, got {:?}",
                    case.case,
                    output.value
                ),
                Expected::NoValue => assert!(
                    output.value.is_none(),
                    "case {:?}: output {key:?} is not evaluable, so it must carry no \
                     OutputValue, got {:?}",
                    case.case,
                    output.value
                ),
                Expected::Form { described, check } => {
                    let actual = output.value.as_deref().unwrap_or_default();
                    assert!(
                        output.value.is_some() && check(actual),
                        "case {:?}: output {key:?} must carry {described}, got {:?}",
                        case.case,
                        output.value
                    );
                }
            }
        }
    }
}

/// The declared `Description` and `Export.Name` travel with the output, and the export
/// name is itself an evaluated expression.
#[tokio::test]
async fn an_outputs_description_and_export_name_are_reported() {
    let router = router();
    let template = concat!(
        "Resources:\n",
        "  T:\n",
        "    Type: AWS::SNS::Topic\n",
        "    Properties:\n",
        "      TopicName: described-topic\n",
        "Outputs:\n",
        "  TopicArn:\n",
        "    Description: the probe topic\n",
        "    Value: !Ref T\n",
        "    Export:\n",
        "      Name: !Sub \"${AWS::StackName}-TopicArn\"\n",
    );
    create_stack(&router, "described", template, &[]).await;
    wait_for_the_transition().await;

    let (_, body) = describe_stack(&router, "described").await;
    let outputs = wire_outputs(&body);
    let output = output_for(&outputs, "TopicArn").expect("the declared output");
    assert_eq!(
        output,
        &WireOutput {
            key: Some("TopicArn".to_string()),
            value: Some(sns_arn("described-topic")),
            description: Some("the probe topic".to_string()),
            export_name: Some("described-TopicArn".to_string()),
        },
        "an output must report its declared description and its evaluated export name, \
         got {body}"
    );
}

/// A template with no `Outputs` section reports no outputs at all — the member is absent
/// rather than an empty list, and nothing is invented for a stack that declares none.
#[tokio::test]
async fn a_template_declaring_no_outputs_reports_none() {
    let router = router();
    let cases: Vec<(&str, &str, &str)> = vec![
        ("no Outputs section at all", "silent-json", r#"{"Resources":{"T":{"Type":"AWS::SNS::Topic"}}}"#),
        (
            "no Outputs section, yaml",
            "silent-yaml",
            "Resources:\n  T:\n    Type: AWS::SNS::Topic\n",
        ),
        ("an empty template", "silent-empty", "{}"),
        (
            "a body declaring only a description",
            "silent-description",
            "Description: a stack with nothing to report\n",
        ),
    ];

    for (case, name, template) in cases {
        let (status_code, body) = create_stack(&router, name, template, &[]).await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: expected CreateStack to succeed, got {status_code} {body}"
        );
        wait_for_the_transition().await;
        let (_, body) = describe_stack(&router, name).await;
        assert!(
            !body.contains("<Outputs>"),
            "case {case:?}: a stack declaring no outputs must report no Outputs, got {body}"
        );
    }
}

/// A resource the template does not name takes the physical name CloudFormation
/// generates, `<stack name>-<logical id>-<unique id>`, and that name is STABLE: the same
/// read twice, and the same resource after an unrelated update, report the same id.
#[tokio::test]
async fn an_unnamed_resource_refs_its_generated_physical_name() {
    let router = router();
    let template = concat!(
        "Resources:\n",
        "  Unnamed:\n",
        "    Type: AWS::SNS::Topic\n",
        "Outputs:\n",
        "  TopicArn:\n",
        "    Value: !Ref Unnamed\n",
    );
    create_stack(&router, "generated", template, &[]).await;
    wait_for_the_transition().await;

    let (_, body) = describe_stack(&router, "generated").await;
    let first = output_for(&wire_outputs(&body), "TopicArn")
        .and_then(|output| output.value.clone())
        .unwrap_or_else(|| panic!("expected an evaluated TopicArn, got {body}"));
    let prefix = format!("{}:generated-Unnamed-", sns_arn("").trim_end_matches(':'));
    assert!(
        first.starts_with(&prefix),
        "an unnamed resource's Ref must be built from its generated name {prefix:?}, got \
         {first:?}"
    );
    let unique = &first[prefix.len()..];
    assert!(
        unique.len() == 12 && unique.chars().all(|c| c.is_ascii_hexdigit()),
        "the generated name's unique segment must be a fixed-width id, got {unique:?}"
    );

    let (_, body) = describe_stack(&router, "generated").await;
    let second = output_for(&wire_outputs(&body), "TopicArn").and_then(|o| o.value.clone());
    assert_eq!(
        second.as_deref(),
        Some(first.as_str()),
        "reading the stack again must report the same physical id, got {body}"
    );
}

/// `{"Ref": "AWS::StackId"}` resolves to the stack's own id, which the same response
/// reports as `StackId` — the output is computed from the stack, not from the request.
#[tokio::test]
async fn the_stack_id_pseudo_parameter_resolves_to_this_stacks_id() {
    let router = router();
    create_stack(
        &router,
        "identified",
        "Outputs:\n  Self:\n    Value: !Ref AWS::StackId\n",
        &[],
    )
    .await;
    wait_for_the_transition().await;

    let (_, body) = describe_stack(&router, "identified").await;
    let stack_id = xml_text(&body, "StackId").expect("the stack's id");
    let output = output_for(&wire_outputs(&body), "Self").and_then(|o| o.value.clone());
    assert_eq!(
        output.as_deref(),
        Some(stack_id.as_str()),
        "Ref AWS::StackId must resolve to the stack's own id {stack_id:?}, got {body}"
    );
}

/// Outputs are the RESULT of the operation that deploys the template, so they are
/// reported once that operation has completed: a read taken while the create is still in
/// progress carries none.
#[tokio::test]
async fn outputs_appear_only_once_the_create_has_completed() {
    let router = router();
    let template = concat!(
        "Resources:\n",
        "  T:\n",
        "    Type: AWS::SNS::Topic\n",
        "    Properties:\n",
        "      TopicName: pending-topic\n",
        "Outputs:\n",
        "  TopicArn:\n",
        "    Value: !Ref T\n",
    );
    let started = std::time::Instant::now();
    create_stack(&router, "pending", template, &[]).await;

    let (_, body) = describe_stack(&router, "pending").await;
    let in_progress = expected_status(
        started,
        status::CREATE_IN_PROGRESS,
        status::CREATE_COMPLETE,
    ) == status::CREATE_IN_PROGRESS;
    if in_progress {
        assert_eq!(
            xml_text(&body, "StackStatus").as_deref(),
            Some(status::CREATE_IN_PROGRESS),
            "the read was taken before the transition was due, got {body}"
        );
        assert!(
            wire_outputs(&body).is_empty(),
            "a stack whose create is still in progress must report no outputs, got {body}"
        );
    }

    wait_for_the_transition().await;
    let (_, body) = describe_stack(&router, "pending").await;
    let output = output_for(&wire_outputs(&body), "TopicArn").and_then(|o| o.value.clone());
    assert_eq!(
        output.as_deref(),
        Some(sns_arn("pending-topic").as_str()),
        "once the create has completed the declared output must carry its value, got {body}"
    );
}

/// An update deploys a new template, so the stack's outputs are those of the template it
/// now carries: a key the new template drops is gone, and a value a changed parameter
/// moves is re-evaluated. Until the update COMPLETES, the values in force are still the
/// ones the completed create left.
#[tokio::test]
async fn an_update_re_evaluates_the_outputs() {
    let router = router();
    let original = concat!(
        "Parameters:\n",
        "  TopicName:\n",
        "    Type: String\n",
        "Resources:\n",
        "  T:\n",
        "    Type: AWS::SNS::Topic\n",
        "    Properties:\n",
        "      TopicName: !Ref TopicName\n",
        "Outputs:\n",
        "  TopicArn:\n",
        "    Value: !Ref T\n",
        "  Dropped:\n",
        "    Value: !Ref AWS::Region\n",
    );
    create_stack(
        &router,
        "evolving",
        original,
        &params(&[("TopicName", "first-topic")]),
    )
    .await;
    wait_for_the_transition().await;

    let (_, body) = describe_stack(&router, "evolving").await;
    assert_eq!(
        output_for(&wire_outputs(&body), "TopicArn").and_then(|o| o.value.clone()),
        Some(sns_arn("first-topic")),
        "the created stack's output must carry the first topic's arn, got {body}"
    );

    let updated = concat!(
        "Parameters:\n",
        "  TopicName:\n",
        "    Type: String\n",
        "Resources:\n",
        "  T:\n",
        "    Type: AWS::SNS::Topic\n",
        "    Properties:\n",
        "      TopicName: !Ref TopicName\n",
        "Outputs:\n",
        "  TopicArn:\n",
        "    Value: !GetAtt T.TopicArn\n",
    );
    let mut members = vec![member("StackName", "evolving"), member("TemplateBody", updated)];
    members.extend(params(&[("TopicName", "second-topic")]));
    let started = std::time::Instant::now();
    let (status_code, body) = call(&router, "UpdateStack", &members).await;
    assert_eq!(
        status_code, 200,
        "expected UpdateStack to succeed, got {status_code} {body}"
    );

    // While the update is still in progress the stack reports the values the completed
    // create left, not the ones the update is about to install.
    let (_, body) = describe_stack(&router, "evolving").await;
    if expected_status(started, status::UPDATE_IN_PROGRESS, status::UPDATE_COMPLETE)
        == status::UPDATE_IN_PROGRESS
    {
        let outputs = wire_outputs(&body);
        assert_eq!(
            output_for(&outputs, "TopicArn").and_then(|o| o.value.clone()),
            Some(sns_arn("first-topic")),
            "an update in progress must still report the outputs in force, got {body}"
        );
        assert!(
            output_for(&outputs, "Dropped").is_some(),
            "an update in progress must still report the output it is about to drop, got \
             {body}"
        );
    }
    wait_for_the_transition().await;

    let (_, body) = describe_stack(&router, "evolving").await;
    let outputs = wire_outputs(&body);
    assert_eq!(
        output_for(&outputs, "TopicArn").and_then(|o| o.value.clone()),
        Some(sns_arn("second-topic")),
        "the update must re-evaluate the output against the new parameter, got {body}"
    );
    assert!(
        output_for(&outputs, "Dropped").is_none(),
        "an output the new template no longer declares must be gone, got {body}"
    );
}

/// A stack whose deletion has completed has no resources left, so it reports no output
/// values — it is still addressable by its stack id, and what it carries is the terminal
/// status, not stale identifiers.
#[tokio::test]
async fn a_deleted_stacks_outputs_are_gone() {
    let router = router();
    let template = concat!(
        "Resources:\n",
        "  T:\n",
        "    Type: AWS::SNS::Topic\n",
        "    Properties:\n",
        "      TopicName: doomed-topic\n",
        "Outputs:\n",
        "  TopicArn:\n",
        "    Value: !Ref T\n",
    );
    create_stack(&router, "doomed", template, &[]).await;
    wait_for_the_transition().await;

    let (_, body) = describe_stack(&router, "doomed").await;
    let stack_id = xml_text(&body, "StackId").expect("the stack's id");
    assert_eq!(
        output_for(&wire_outputs(&body), "TopicArn").and_then(|o| o.value.clone()),
        Some(sns_arn("doomed-topic")),
        "the live stack must report its output, got {body}"
    );

    call(&router, "DeleteStack", &[member("StackName", "doomed")]).await;
    wait_for_the_transition().await;

    let (status_code, body) = describe_stack(&router, &stack_id).await;
    assert_eq!(
        status_code, 200,
        "a deleted stack is still addressable by its id, got {status_code} {body}"
    );
    assert_eq!(
        xml_text(&body, "StackStatus").as_deref(),
        Some(status::DELETE_COMPLETE),
        "the stack must have reached DELETE_COMPLETE, got {body}"
    );
    assert!(
        wire_outputs(&body).is_empty(),
        "a stack whose deletion completed must report no outputs, got {body}"
    );
}

/// A template body built to explode the parse is read as NO template — the request is
/// ANSWERED, with the template-format refusal a body this service will not read is owed, and
/// the process is still serving.
///
/// Nesting is what matters here: the YAML loader descends once per level and cannot be
/// aborted from the event stream, so the nesting a YAML body can reach is bounded before
/// the parser sees it. Each row is a different way to buy depth cheaply, and each is past
/// one of the bounds, so every one of them is refused rather than read.
#[tokio::test]
async fn a_template_body_built_to_explode_the_parse_is_read_as_no_template() {
    let router = router();
    // The documented `TemplateBody` maximum, so each body is as large as a caller can
    // make one.
    let body_max = TEMPLATE_BODY_MAX_BYTES;
    // Each row: the case, the body and its index. Every one of them is past a bound the
    // reader puts on a document, so every one is refused.
    let cases: Vec<(&str, String, usize)> = vec![
        (
            "flow sequences, one byte per level",
            "[".repeat(body_max),
            1,
        ),
        (
            "flow mappings, one byte per level",
            "{".repeat(body_max),
            2,
        ),
        (
            "compact block sequences, two bytes per level",
            "- ".repeat(body_max / 2),
            3,
        ),
        (
            "indentation, one column per level",
            (0..300)
                .map(|level| format!("{}k{level}:\n", " ".repeat(level)))
                .collect(),
            4,
        ),
        (
            // Valid JSON, but nested past what either reader will build: refused for its
            // form, not read as a document declaring nothing.
            "well-formed json nested past what any template needs",
            format!("{}1{}", "[".repeat(2_000), "]".repeat(2_000)),
            5,
        ),
        (
            // Closers hidden inside scalars keep the running flow depth low while the
            // loader's own depth climbs; the opener COUNT is what bounds that.
            "flow openers interleaved with closers quoted inside scalars",
            format!("Outputs: {}", "[\"]\"".repeat(body_max / 8)),
            6,
        ),
        (
            // The worst case the opener count permits: every closer is quoted, so the
            // running depth never rises, and the loader really does descend once per
            // level. The YAML reader stops in it, and the position it stopped at is what
            // the refusal reports.
            "the deepest nesting the opener count allows, every closer quoted",
            format!("Outputs: {}", "[\"]\",".repeat(2_000)),
            7,
        ),
        (
            "nesting just past what the reader builds, with outputs declared beneath it",
            format!(
                "Outputs:\n  Deep:\n    Value: {}\"x\"{}\n",
                "[".repeat(40),
                "]".repeat(40)
            ),
            8,
        ),
    ];

    for (case, template, index) in cases {
        let name = format!("exploding-{index}");
        let (status_code, body) = create_stack(&router, &name, &template, &[]).await;
        // A body this service will not READ at all is refused as a template FORM error —
        // and the request is ANSWERED rather than taking the process down, which is what
        // the bounds are for.
        assert_eq!(
            status_code, 400,
            "case {case:?}: a body this service will not read must be refused, got \
             {status_code} {body}"
        );
        assert_eq!(
            xml_text(&body, "Code").as_deref(),
            Some("ValidationError"),
            "case {case:?}: a body this service will not read is refused a template \
             format error, got {status_code} {body}"
        );
        let message = xml_text(&body, "Message").unwrap_or_default();
        assert!(
            message.starts_with("Template format error"),
            "case {case:?}: the refusal must be a template format error, got {message:?}"
        );
        // And nothing was created for it: the name is still free.
        let (status_code, described) = describe_stack(&router, &name).await;
        assert_eq!(
            status_code, 400,
            "case {case:?}: a refused body must leave no stack, got {status_code} \
             {described}"
        );
    }
}

/// A big, FLAT template is read like any other: nothing that bounds nesting may be
/// spent by a template that is merely large — many resources and outputs, hyphenated
/// names, long `Fn::Join` lines, a `Description` — in either spelling.
#[tokio::test]
async fn a_large_flat_template_is_read_in_full() {
    let router = router();

    // 400 resources and 100 outputs in the JSON spelling: well over a thousand braces and
    // brackets, and every one of them closed immediately.
    let resources: serde_json::Map<String, serde_json::Value> = (0..400)
        .map(|index| {
            (
                format!("R{index:03}"),
                serde_json::json!({
                    "Type": "AWS::SNS::Topic",
                    "Properties": {"TopicName": format!("flat-topic-{index:03}")}
                }),
            )
        })
        .collect();
    let outputs: serde_json::Map<String, serde_json::Value> = (0..100)
        .map(|index| {
            (
                format!("O{index:03}"),
                serde_json::json!({"Value": {"Ref": format!("R{index:03}")}}),
            )
        })
        .collect();
    let template = serde_json::json!({
        "Description": "a large flat template",
        "Resources": resources,
        "Outputs": outputs,
    })
    .to_string();
    assert!(
        template.len() < TEMPLATE_BODY_MAX_BYTES,
        "the fixture must be a body a caller may send, got {} bytes",
        template.len()
    );

    let (status_code, body) = create_stack(&router, "flat-json", &template, &[]).await;
    assert_eq!(
        status_code, 200,
        "expected CreateStack to succeed, got {status_code} {body}"
    );
    wait_for_the_transition().await;
    let (_, body) = describe_stack(&router, "flat-json").await;
    assert_eq!(
        xml_text(&body, "Description").as_deref(),
        Some("a large flat template"),
        "a large template still reports its Description, got {body}"
    );
    let outputs = wire_outputs(&body);
    assert_eq!(
        outputs.len(),
        100,
        "every declared output of a large template must be reported, got {} of them",
        outputs.len()
    );
    assert_eq!(
        output_for(&outputs, "O399").and_then(|o| o.value.clone()),
        None,
        "O399 is not declared by this template"
    );
    for key in ["O000", "O042", "O099"] {
        let index: usize = key[1..].parse().expect("the output's index");
        assert_eq!(
            output_for(&outputs, key).and_then(|o| o.value.clone()),
            Some(sns_arn(&format!("flat-topic-{index:03}"))),
            "output {key:?} must carry its topic's arn, got {body}"
        );
    }

    // The same shape in YAML — which DOES go through the pre-flight guard — with
    // hyphenated names and a long `Fn::Join` line carrying hundreds of hyphens.
    let mut yaml = String::from("Description: a large flat yaml template\nResources:\n");
    for index in 0..200 {
        yaml.push_str(&format!(
            "  R{index:03}:\n    Type: AWS::SNS::Topic\n    Properties:\n      TopicName: a-very-hyphenated-topic-name-{index:03}\n"
        ));
    }
    yaml.push_str("Outputs:\n  Joined:\n    Value: !Join [\"-\", [");
    yaml.push_str(
        &(0..100)
            .map(|index| format!("\"part-{index:03}\""))
            .collect::<Vec<String>>()
            .join(", "),
    );
    yaml.push_str("]]\n  First:\n    Value: !Ref R000\n");
    assert!(
        yaml.len() < TEMPLATE_BODY_MAX_BYTES,
        "the fixture must be a body a caller may send, got {} bytes",
        yaml.len()
    );

    let (status_code, body) = create_stack(&router, "flat-yaml", &yaml, &[]).await;
    assert_eq!(
        status_code, 200,
        "expected CreateStack to succeed, got {status_code} {body}"
    );
    wait_for_the_transition().await;
    let (_, body) = describe_stack(&router, "flat-yaml").await;
    let outputs = wire_outputs(&body);
    assert_eq!(
        output_for(&outputs, "First").and_then(|o| o.value.clone()),
        Some(sns_arn("a-very-hyphenated-topic-name-000")),
        "a hyphenated resource name must still be read, got {body}"
    );
    let joined = (0..100)
        .map(|index| format!("part-{index:03}"))
        .collect::<Vec<String>>()
        .join("-");
    assert_eq!(
        output_for(&outputs, "Joined").and_then(|o| o.value.clone()),
        Some(joined),
        "a long Fn::Join line full of hyphens must still be read, got {body}"
    );
    assert_eq!(
        xml_text(&body, "Description").as_deref(),
        Some("a large flat yaml template"),
        "a large yaml template still reports its Description, got {body}"
    );
}

/// `Fn::Sub` resolves each variable ONCE however often its source names it, and the
/// deployment's work budget bounds the total: a chain of nested substitutions that would
/// re-evaluate its parts exponentially is answered — promptly — with no value rather than
/// with unbounded work.
#[tokio::test]
async fn a_substitution_chain_cannot_multiply_the_work_of_its_parts() {
    let router = router();

    /// One `Fn::Sub` per level, each source naming its variable `occurrences` times, the
    /// variable being the next level down. `body` is the innermost source.
    fn chain(levels: usize, occurrences: usize, innermost: &str) -> serde_json::Value {
        let source = "${a}".repeat(occurrences);
        let mut value = serde_json::Value::String(innermost.to_string());
        for _ in 0..levels {
            value = serde_json::json!({"Fn::Sub": [source.clone(), {"a": value}]});
        }
        value
    }

    let cases: Vec<(&str, serde_json::Value, Option<&str>)> = vec![
        (
            // Exponential WITHOUT memoisation: 8^16 evaluations. Each level resolves to
            // the empty string, so no size cap would ever trip.
            "sixteen levels, eight occurrences each, empty innermost",
            chain(16, 8, ""),
            Some(""),
        ),
        (
            // The same shape, but each level's text grows, so the evaluation is refused
            // by the value-size cap or the work budget — either way, with no value.
            "sixteen levels, eight occurrences each, growing text",
            chain(16, 8, "0123456789abcdef"),
            None,
        ),
        (
            "one level, one occurrence of a plain parameter-less literal",
            chain(1, 1, "x"),
            Some("x"),
        ),
    ];

    for (index, (case, value, expected)) in cases.into_iter().enumerate() {
        let template =
            serde_json::json!({"Outputs": {"Chained": {"Value": value}}}).to_string();
        let name = format!("chained-{index}");
        let started = std::time::Instant::now();
        let (status_code, body) = create_stack(&router, &name, &template, &[]).await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: expected CreateStack to succeed, got {status_code} {body}"
        );
        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "case {case:?}: the deployment must be bounded work, took {:?}",
            started.elapsed()
        );
        wait_for_the_transition().await;
        let (_, body) = describe_stack(&router, &name).await;
        let outputs = wire_outputs(&body);
        let output = output_for(&outputs, "Chained")
            .unwrap_or_else(|| panic!("case {case:?}: the declared output, got {body}"));
        assert_eq!(
            output.value.as_deref(),
            expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            output.value
        );
    }
}

/// A resource whose identifier the OWNING SERVICE assigns gets one identifier per
/// resource, of that service's documented form, and it is STABLE: two reads report the
/// same one, and two resources of the same type in one stack never share it.
///
/// The rows differ along what the rule generalises over: the resource type, its
/// documented prefix, and the character class its id is drawn from.
#[tokio::test]
async fn a_service_assigned_resource_id_is_unique_per_resource_and_stable() {
    let router = router();
    // Every type here is one this router leaves DECLARED-ONLY: a type whose owning
    // service this router does not mount is created nowhere, and the id CloudFormation
    // publishes for it is what this rule is about. `AWS::Transfer::Server` was one of
    // them until it gained a provisioner — a stack that declares it now calls AWS
    // Transfer Family — so its place here is taken by a second `AWS::Transfer::Workflow`,
    // whose id has the same service-assigned shape, and the provisioned type is asserted
    // against the service that creates it in
    // `core/router/tests/all/cloudformation_provisions_transfer_servers.rs`.
    let template = concat!(
        "Resources:\n",
        "  First:\n",
        "    Type: AWS::Transfer::Workflow\n",
        "  Second:\n",
        "    Type: AWS::Transfer::Workflow\n",
        "  Monitor:\n",
        "    Type: AWS::RUM::AppMonitor\n",
        "    Properties:\n",
        "      Name: assigned-ids-one\n",
        "  OtherMonitor:\n",
        "    Type: AWS::RUM::AppMonitor\n",
        "    Properties:\n",
        "      Name: assigned-ids-two\n",
        "Outputs:\n",
        "  FirstId:\n",
        "    Value: !GetAtt First.WorkflowId\n",
        "  SecondId:\n",
        "    Value: !GetAtt Second.WorkflowId\n",
        "  MonitorId:\n",
        "    Value: !GetAtt Monitor.Id\n",
        "  OtherMonitorId:\n",
        "    Value: !GetAtt OtherMonitor.Id\n",
    );
    create_stack(&router, "assigned-ids", template, &[]).await;
    wait_for_the_transition().await;

    let (_, body) = describe_stack(&router, "assigned-ids").await;
    let first = wire_outputs(&body);
    let read = |key: &str, outputs: &[WireOutput]| -> String {
        output_for(outputs, key)
            .and_then(|output| output.value.clone())
            .unwrap_or_else(|| panic!("expected an evaluated {key}, got {body}"))
    };

    // (case, output key, the form the owning service documents, the predicate)
    struct IdRow {
        case: &'static str,
        key: &'static str,
        described: &'static str,
        check: fn(&str) -> bool,
    }
    let rows: [IdRow; 4] = [
        IdRow {
            case: "a transfer workflow",
            key: "FirstId",
            described: "w- + 17 of [a-z0-9]",
            check: is_transfer_workflow_id,
        },
        IdRow {
            case: "a second transfer workflow in the same stack",
            key: "SecondId",
            described: "w- + 17 of [a-z0-9]",
            check: is_transfer_workflow_id,
        },
        IdRow {
            case: "a rum app monitor",
            key: "MonitorId",
            described: "the modelled 36-character AppMonitorId UUID",
            check: is_app_monitor_id,
        },
        IdRow {
            case: "a second rum app monitor in the same stack",
            key: "OtherMonitorId",
            described: "the modelled 36-character AppMonitorId UUID",
            check: is_app_monitor_id,
        },
    ];
    for IdRow {
        case,
        key,
        described,
        check,
    } in rows
    {
        let value = read(key, &first);
        assert!(
            check(&value),
            "case {case:?}: {key} must be of the form {described}, got {value:?}"
        );
    }
    assert_ne!(
        read("FirstId", &first),
        read("SecondId", &first),
        "two resources of one type in one stack must not share an assigned id, got {body}"
    );
    assert_ne!(
        read("MonitorId", &first),
        read("OtherMonitorId", &first),
        "two app monitors in one stack must not share an assigned id, got {body}"
    );

    let (_, body) = describe_stack(&router, "assigned-ids").await;
    let second = wire_outputs(&body);
    for key in ["FirstId", "SecondId", "MonitorId", "OtherMonitorId"] {
        assert_eq!(
            read(key, &second),
            read(key, &first),
            "reading the stack again must report the same assigned id for {key}, got {body}"
        );
    }
}

/// A template past a documented per-template quota is not one CloudFormation accepts, so
/// the request submitting it is REFUSED — the quota is one of the ceilings
/// `DescribeAccountLimits` advertises for the account (`StackOutputsLimit`), and an
/// advertised ceiling a write is not held to is no ceiling (`account_limits.rs` is where
/// that link is pinned). A template at the quota is accepted and reports every output it
/// declares.
#[tokio::test]
async fn a_template_past_a_documented_quota_is_refused() {
    let router = router();
    let cases: Vec<(&str, usize, bool)> = vec![
        // The published bound and one past it.
        ("outputs at the documented quota", MAX_OUTPUTS_PER_TEMPLATE, true),
        (
            "one output past the documented quota",
            MAX_OUTPUTS_PER_TEMPLATE + 1,
            false,
        ),
    ];

    for (case, count, accepted) in cases {
        let outputs: serde_json::Map<String, serde_json::Value> = (0..count)
            .map(|index| {
                (
                    format!("Out{index:04}"),
                    serde_json::json!({"Value": {"Ref": "AWS::Region"}}),
                )
            })
            .collect();
        let template = serde_json::json!({"Outputs": outputs}).to_string();
        let name = format!("quota-{count}");
        let (status_code, body) = create_stack(&router, &name, &template, &[]).await;
        if !accepted {
            assert_eq!(
                status_code, 400,
                "case {case:?}: expected the create refused, got {status_code} {body}"
            );
            assert_eq!(
                xml_text(&body, "Code").as_deref(),
                Some("ValidationError"),
                "case {case:?}: expected ValidationError, got {body}"
            );
            continue;
        }
        assert_eq!(
            status_code, 200,
            "case {case:?}: expected CreateStack to succeed, got {status_code} {body}"
        );
        wait_for_the_transition().await;
        let (_, body) = describe_stack(&router, &name).await;
        assert_eq!(
            wire_outputs(&body).len(),
            count,
            "case {case:?}: expected {count} outputs reported, got {} of them",
            wire_outputs(&body).len()
        );
    }
}
