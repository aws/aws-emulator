// SPDX-License-Identifier: CC0-1.0

//! The outputs of a deployed search-indexing pipeline stack, end to end over the awsQuery
//! wire.
//!
//! RULE under test: **once a stack's deployment has completed, `DescribeStacks` reports
//! every output the template declares with its resolved `OutputValue` — `Ref` answering the
//! resource's physical id and `Fn::GetAtt` answering each attribute its type publishes: from
//! the type's published return values, from the property the template declared, from
//! CloudFormation's uniform `Arn`/`Id`/`Name` naming, and, for a resource the OWNING service
//! created, from that service's OWN report of the attribute — including one the type
//! publishes as a nested member of a reported container, which `Fn::GetAtt` reads through a
//! dotted path.** An output carries no value only where the expression names something the
//! estate does not hold (a version 2 address a domain that is not `dualstack` is provisioned
//! none of), never because an attribute was left uncollected.
//!
//! The reported case is a multi-service pipeline — an Amazon S3 landing bucket whose
//! notification drives an AWS Lambda indexer under an execution role, its CloudWatch Logs log
//! group, a CloudWatch alarm over its failures, and the Amazon OpenSearch Service domain the
//! documents are indexed into — whose stack reached `CREATE_COMPLETE` while its outputs came
//! back with keys and no values, so the client that deployed it could not learn the
//! identifiers of anything it had just created and every later leg of the workflow was
//! unreachable.
//!
//! The rows differ along what the rule generalises over rather than restating one recorded
//! template:
//!
//! * the SPELLING of the body — YAML with the short-form intrinsic tags, and the same estate
//!   as JSON with the long-form functions;
//! * where each resource's IDENTITY comes from — a row that NAMES every resource (and takes
//!   the domain's name from a template parameter) and one that names none, so CloudFormation
//!   generates them;
//! * which attributes the domain HAS: a public IPv4 domain, a public `dualstack` one (which
//!   is provisioned a version 2 endpoint beside its IPv4 host), and one reached inside a VPC
//!   (which has no public endpoint at all and publishes its hosts in a map instead);
//! * the SOURCE of the attribute — the published return values of `AWS::S3::Bucket`, the
//!   uniform naming of the types that publish only an ARN, and the owning service's own
//!   report for every attribute of the deployed domain.
//!
//! No expected value is a literal from a recording: each is computed from the identity the
//! SAME response reports for the resource, or — for every attribute of the domain — read out
//! of Amazon OpenSearch Service through `DescribeDomain` on its own wire, so a row cannot
//! drift from the service the stack created the resource in.
//!
//! The estate needs FOUR services on one engine — CloudFormation reaches Amazon OpenSearch
//! Service, Amazon S3 and IAM through the in-process callback to create the domain, the bucket
//! and the execution role — which is why the suite lives here. The wire helpers, the domain
//! read and the stack poller are the test crate's own
//! ([`crate::cloudformation_provisions_opensearch_domains`], [`crate::query_wire`]) rather
//! than a second copy.

use std::sync::Arc;

use serde_json::Value;
use smithy_server_runtime::MultiServiceRouter;

use crate::cloudformation_provisions_opensearch_domains::{
    cloudformation, describe_domain, settle_stack, xml_text, REGION,
};
use crate::query_wire::{block_of, elements, members_of};

/// The account every ARN this emulator mints carries.
const ACCOUNT_ID: &str = "000000000000";

/// How many hexadecimal characters the unique segment of a physical id CloudFormation
/// GENERATES carries (`<stack name>-<logical id>-<unique id>`).
const GENERATED_ID_HEX_DIGITS: usize = 12;

/// The prefix of an IAM role's `RoleId`: AWS's unique-identifier prefix for a role is `AROA`,
/// followed by upper-case alphanumeric characters. It is the id IAM assigns, so it is neither
/// the role's name nor anything a template could state.
const ROLE_ID_PREFIX: &str = "AROA";

/// The emulator with the five services this suite exercises, mounted on ONE engine exactly as
/// `core/router/src/lib.rs` mounts them: the stack really creates its domain, its bucket, its
/// role, its function and its log group in the services that own them.
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
        service_com_amazonaws_opensearch_amazonopensearchservice::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        Arc::clone(&engine),
    ));
    // AWS Lambda, for the `AWS::Lambda::Function` and the `AWS::Lambda::Permission` the
    // template declares: a stack really creates both there, so the owning service has to be
    // mounted beside CloudFormation or the create is refused and the stack fails.
    router.register_service(
        service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(Arc::clone(&engine)),
    );
    // Amazon CloudWatch Logs, for the `AWS::Logs::LogGroup` the template declares: a stack
    // really creates it there, so the owning service has to be mounted beside CloudFormation
    // or the create is refused and the stack fails.
    router.register_service(
        service_com_amazonaws_cloudwatchlogs_logs_20140328::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(engine),
    );
    router
}

/// One `Output` as the response carries it.
///
/// Read member by member through the shared list reader, so an output's key stays paired with
/// its own value and a declared output carrying NO `OutputValue` is visible as such instead of
/// silently borrowing the next one's.
#[derive(Debug, Clone)]
struct WireOutput {
    key: Option<String>,
    value: Option<String>,
}

/// Every `Output` of a `DescribeStacks` answer, in wire order.
fn reported_outputs(xml: &str) -> Vec<WireOutput> {
    let Some(block) = block_of(xml, "Outputs") else {
        return Vec::new();
    };
    members_of(block)
        .into_iter()
        .map(|member| WireOutput {
            key: elements(member, "OutputKey").into_iter().next(),
            value: elements(member, "OutputValue").into_iter().next(),
        })
        .collect()
}

/// Each output over an attribute of the DOMAIN, paired with the `DomainStatus` member Amazon
/// OpenSearch Service reports that attribute as — a dotted path naming a member of a reported
/// container.
///
/// This is the published `Fn::GetAtt` set of `AWS::OpenSearchService::Domain` as that
/// service's own model names the values behind it
/// (`smithy_query(query="shape",shape="DomainStatus,ServiceSoftwareOptions,AdvancedSecurityOptions")`):
/// the ARN, the domain id, the IPv4 and dual-stack endpoints, the hosts a VPC-access domain is
/// reached on, the domain's service-software record and its anonymous-auth migration date.
/// Every row asserts BOTH directions at once — an attribute the service reports a value for
/// carries exactly that value, and one it reports nothing for carries nothing — so a member
/// this emulator does not hold cannot be papered over with an invented value either.
const DOMAIN_ATTRIBUTE_OUTPUTS: &[(&str, &str)] = &[
    ("DomainArn", "ARN"),
    ("DomainArnNamed", "ARN"),
    ("DomainIdOut", "DomainId"),
    ("DomainEndpoint", "Endpoint"),
    ("DomainEndpointV2", "EndpointV2"),
    ("DomainVpcHost", "Endpoints.vpc"),
    (
        "SoftwareCurrentVersion",
        "ServiceSoftwareOptions.CurrentVersion",
    ),
    ("SoftwareNewVersion", "ServiceSoftwareOptions.NewVersion"),
    (
        "SoftwareUpdateAvailable",
        "ServiceSoftwareOptions.UpdateAvailable",
    ),
    ("SoftwareCancellable", "ServiceSoftwareOptions.Cancellable"),
    ("SoftwareUpdateStatus", "ServiceSoftwareOptions.UpdateStatus"),
    ("SoftwareDescription", "ServiceSoftwareOptions.Description"),
    (
        "SoftwareAutomatedUpdateDate",
        "ServiceSoftwareOptions.AutomatedUpdateDate",
    ),
    (
        "SoftwareOptionalDeployment",
        "ServiceSoftwareOptions.OptionalDeployment",
    ),
    (
        "AnonymousAuthDisableDate",
        "AdvancedSecurityOptions.AnonymousAuthDisableDate",
    ),
];

/// The members of that set the owning service reports for EVERY domain it holds, so an output
/// over one of them must carry a value whatever else a row declares: the domain's ARN, its
/// id ("the unique identifier for the domain") and the current release of its service software
/// ("the current service software version present on the domain").
const ALWAYS_REPORTED_DOMAIN_OUTPUTS: &[&str] =
    &["DomainArn", "DomainIdOut", "SoftwareCurrentVersion"];

/// Where each resource of the pipeline takes its name from in one row.
enum Identity {
    /// The template NAMES every resource: the bucket, the log group, the function and the
    /// alarm are named after the stack, and the domain after a template parameter.
    Declared,
    /// The template names none of them, so every physical id is one CloudFormation generated
    /// for the resource.
    Generated,
}

/// One row: a pipeline template, how its resources are identified, and what its domain is
/// reachable on.
struct Case {
    case: &'static str,
    stack: &'static str,
    template: String,
    /// The `Parameters` the create passes.
    parameters: Vec<(String, String)>,
    identity: Identity,
    /// The domain's name, for a row whose template takes it from a parameter.
    domain: Option<&'static str>,
}

/// The outputs every row declares: `Ref` and every published attribute of each resource of
/// the pipeline.
const OUTPUTS_YAML: &str = r#"Outputs:
  BucketName:
    Value: !Ref RawBucket
  BucketArn:
    Value: !GetAtt RawBucket.Arn
  BucketDomainName:
    Value: !GetAtt RawBucket.DomainName
  BucketDualStackDomainName:
    Value: !GetAtt RawBucket.DualStackDomainName
  BucketRegionalDomainName:
    Value: !GetAtt RawBucket.RegionalDomainName
  BucketWebsiteURL:
    Value: !GetAtt RawBucket.WebsiteURL
  RoleName:
    Value: !Ref IndexerRole
  RoleArn:
    Value: !GetAtt IndexerRole.Arn
  RoleId:
    Value: !GetAtt IndexerRole.RoleId
  LogGroupName:
    Value: !Ref IndexerLogGroup
  LogGroupArn:
    Value: !GetAtt IndexerLogGroup.Arn
  FunctionName:
    Value: !Ref Indexer
  FunctionArn:
    Value: !GetAtt Indexer.Arn
  PermissionId:
    Value: !Ref InvokePermission
  AlarmName:
    Value: !Ref IndexingErrors
  AlarmArn:
    Value: !GetAtt IndexingErrors.Arn
  DomainRef:
    Value: !Ref SearchDomain
  DomainArn:
    Value: !GetAtt SearchDomain.Arn
  DomainArnNamed:
    Value: !GetAtt SearchDomain.DomainArn
  DomainIdOut:
    Value: !GetAtt SearchDomain.Id
  DomainEndpoint:
    Value: !GetAtt SearchDomain.DomainEndpoint
  DomainEndpointV2:
    Value: !GetAtt SearchDomain.DomainEndpointV2
  DomainVpcHost:
    Value: !GetAtt SearchDomain.DomainEndpoints.vpc
  SoftwareCurrentVersion:
    Value: !GetAtt SearchDomain.ServiceSoftwareOptions.CurrentVersion
  SoftwareNewVersion:
    Value: !GetAtt SearchDomain.ServiceSoftwareOptions.NewVersion
  SoftwareUpdateAvailable:
    Value: !GetAtt SearchDomain.ServiceSoftwareOptions.UpdateAvailable
  SoftwareCancellable:
    Value: !GetAtt SearchDomain.ServiceSoftwareOptions.Cancellable
  SoftwareUpdateStatus:
    Value: !GetAtt SearchDomain.ServiceSoftwareOptions.UpdateStatus
  SoftwareDescription:
    Value: !GetAtt SearchDomain.ServiceSoftwareOptions.Description
  SoftwareAutomatedUpdateDate:
    Value: !GetAtt SearchDomain.ServiceSoftwareOptions.AutomatedUpdateDate
  SoftwareOptionalDeployment:
    Value: !GetAtt SearchDomain.ServiceSoftwareOptions.OptionalDeployment
  AnonymousAuthDisableDate:
    Value: !GetAtt SearchDomain.AdvancedSecurityOptions.AnonymousAuthDisableDate
"#;

/// The same output set in JSON with the long-form functions: the spelling of the body is no
/// part of what an output resolves to.
const OUTPUTS_JSON: &str = r#""Outputs":{
  "BucketName":{"Value":{"Ref":"RawBucket"}},
  "BucketArn":{"Value":{"Fn::GetAtt":["RawBucket","Arn"]}},
  "BucketDomainName":{"Value":{"Fn::GetAtt":["RawBucket","DomainName"]}},
  "BucketDualStackDomainName":{"Value":{"Fn::GetAtt":["RawBucket","DualStackDomainName"]}},
  "BucketRegionalDomainName":{"Value":{"Fn::GetAtt":["RawBucket","RegionalDomainName"]}},
  "BucketWebsiteURL":{"Value":{"Fn::GetAtt":["RawBucket","WebsiteURL"]}},
  "RoleName":{"Value":{"Ref":"IndexerRole"}},
  "RoleArn":{"Value":{"Fn::GetAtt":["IndexerRole","Arn"]}},
  "RoleId":{"Value":{"Fn::GetAtt":["IndexerRole","RoleId"]}},
  "LogGroupName":{"Value":{"Ref":"IndexerLogGroup"}},
  "LogGroupArn":{"Value":{"Fn::GetAtt":["IndexerLogGroup","Arn"]}},
  "FunctionName":{"Value":{"Ref":"Indexer"}},
  "FunctionArn":{"Value":{"Fn::GetAtt":["Indexer","Arn"]}},
  "PermissionId":{"Value":{"Ref":"InvokePermission"}},
  "AlarmName":{"Value":{"Ref":"IndexingErrors"}},
  "AlarmArn":{"Value":{"Fn::GetAtt":["IndexingErrors","Arn"]}},
  "DomainRef":{"Value":{"Ref":"SearchDomain"}},
  "DomainArn":{"Value":{"Fn::GetAtt":["SearchDomain","Arn"]}},
  "DomainArnNamed":{"Value":{"Fn::GetAtt":["SearchDomain","DomainArn"]}},
  "DomainIdOut":{"Value":{"Fn::GetAtt":["SearchDomain","Id"]}},
  "DomainEndpoint":{"Value":{"Fn::GetAtt":["SearchDomain","DomainEndpoint"]}},
  "DomainEndpointV2":{"Value":{"Fn::GetAtt":["SearchDomain","DomainEndpointV2"]}},
  "DomainVpcHost":{"Value":{"Fn::GetAtt":["SearchDomain","DomainEndpoints.vpc"]}},
  "SoftwareCurrentVersion":{"Value":{"Fn::GetAtt":["SearchDomain","ServiceSoftwareOptions.CurrentVersion"]}},
  "SoftwareNewVersion":{"Value":{"Fn::GetAtt":["SearchDomain","ServiceSoftwareOptions.NewVersion"]}},
  "SoftwareUpdateAvailable":{"Value":{"Fn::GetAtt":["SearchDomain","ServiceSoftwareOptions.UpdateAvailable"]}},
  "SoftwareCancellable":{"Value":{"Fn::GetAtt":["SearchDomain","ServiceSoftwareOptions.Cancellable"]}},
  "SoftwareUpdateStatus":{"Value":{"Fn::GetAtt":["SearchDomain","ServiceSoftwareOptions.UpdateStatus"]}},
  "SoftwareDescription":{"Value":{"Fn::GetAtt":["SearchDomain","ServiceSoftwareOptions.Description"]}},
  "SoftwareAutomatedUpdateDate":{"Value":{"Fn::GetAtt":["SearchDomain","ServiceSoftwareOptions.AutomatedUpdateDate"]}},
  "SoftwareOptionalDeployment":{"Value":{"Fn::GetAtt":["SearchDomain","ServiceSoftwareOptions.OptionalDeployment"]}},
  "AnonymousAuthDisableDate":{"Value":{"Fn::GetAtt":["SearchDomain","AdvancedSecurityOptions.AnonymousAuthDisableDate"]}}}"#;

/// The properties that put a domain inside a VPC: such a domain has no public endpoint at
/// all and publishes the hosts it is reached on in `DomainStatus$Endpoints` instead.
/// Indented to the property level of the YAML body below.
const VPC_DOMAIN_PROPERTIES: &str = r#"      IPAddressType: dualstack
      VPCOptions:
        SubnetIds:
          - subnet-00000000000000001
        SecurityGroupIds:
          - sg-00000000000000001"#;

/// The pipeline as YAML with the short-form intrinsic tags, every resource NAMED and the
/// domain's name taken from a template parameter. `domain_properties` is spliced in so the
/// rows differ only in how the domain is reached.
fn yaml_pipeline(domain_properties: &str) -> String {
    format!(
        r#"AWSTemplateFormatVersion: '2010-09-09'
Description: A search-indexing pipeline
Parameters:
  DomainName:
    Type: String
Resources:
  RawBucket:
    Type: AWS::S3::Bucket
    DependsOn: InvokePermission
    Properties:
      BucketName: !Sub '${{AWS::StackName}}-raw'
      NotificationConfiguration:
        LambdaConfigurations:
          - Event: 's3:ObjectCreated:*'
            Function: !GetAtt Indexer.Arn
            Filter:
              S3Key:
                Rules:
                  - Name: suffix
                    Value: .json
  IndexerRole:
    Type: AWS::IAM::Role
    Properties:
      RoleName: !Sub '${{AWS::StackName}}-indexer'
      AssumeRolePolicyDocument:
        Version: '2012-10-17'
        Statement:
          - Effect: Allow
            Principal:
              Service: lambda.amazonaws.com
            Action: sts:AssumeRole
  IndexerLogGroup:
    Type: AWS::Logs::LogGroup
    Properties:
      LogGroupName: !Sub '/aws/lambda/${{AWS::StackName}}-indexer'
      RetentionInDays: 7
  SearchDomain:
    Type: AWS::OpenSearchService::Domain
    Properties:
      DomainName: !Ref DomainName
      EngineVersion: OpenSearch_2.11
      ClusterConfig:
        InstanceType: t3.small.search
        InstanceCount: 1
      EBSOptions:
        EBSEnabled: true
        VolumeType: gp2
        VolumeSize: 10
{domain_properties}
  Indexer:
    Type: AWS::Lambda::Function
    Properties:
      FunctionName: !Sub '${{AWS::StackName}}-indexer'
      Runtime: python3.12
      Handler: index.handler
      Role: !GetAtt IndexerRole.Arn
      Code:
        ZipFile: |
          def handler(event, context):
              return {{}}
      Environment:
        Variables:
          DOMAIN_NAME: !Ref SearchDomain
  InvokePermission:
    Type: AWS::Lambda::Permission
    Properties:
      FunctionName: !GetAtt Indexer.Arn
      Action: lambda:InvokeFunction
      Principal: s3.amazonaws.com
      SourceArn: !Sub 'arn:aws:s3:::${{AWS::StackName}}-raw'
  IndexingErrors:
    Type: AWS::CloudWatch::Alarm
    Properties:
      AlarmName: !Sub '${{AWS::StackName}}-indexer-errors'
      Namespace: AWS/Lambda
      MetricName: Errors
      Statistic: Sum
      Period: 60
      EvaluationPeriods: 1
      Threshold: 1
      ComparisonOperator: GreaterThanOrEqualToThreshold
{OUTPUTS_YAML}"#
    )
}

/// The same estate as JSON with the long-form functions, naming NO resource: every physical
/// id is the one CloudFormation generates, and the domain is `dualstack`.
fn json_pipeline() -> String {
    format!(
        r#"{{"AWSTemplateFormatVersion":"2010-09-09",
  "Resources":{{
    "RawBucket":{{"Type":"AWS::S3::Bucket"}},
    "IndexerRole":{{"Type":"AWS::IAM::Role","Properties":{{
      "AssumeRolePolicyDocument":{{"Version":"2012-10-17","Statement":[
        {{"Effect":"Allow","Principal":{{"Service":"lambda.amazonaws.com"}},
         "Action":"sts:AssumeRole"}}]}}}}}},
    "IndexerLogGroup":{{"Type":"AWS::Logs::LogGroup","Properties":{{"RetentionInDays":14}}}},
    "SearchDomain":{{"Type":"AWS::OpenSearchService::Domain","Properties":{{
      "EngineVersion":"OpenSearch_2.11","IPAddressType":"dualstack",
      "ClusterConfig":{{"InstanceType":"t3.small.search","InstanceCount":1}},
      "EBSOptions":{{"EBSEnabled":true,"VolumeType":"gp3","VolumeSize":20}}}}}},
    "Indexer":{{"Type":"AWS::Lambda::Function","Properties":{{
      "Runtime":"python3.12","Handler":"index.handler",
      "Role":{{"Fn::GetAtt":["IndexerRole","Arn"]}},
      "Code":{{"ZipFile":"def handler(event, context):\n    return {{}}\n"}},
      "Environment":{{"Variables":{{"DOMAIN_NAME":{{"Ref":"SearchDomain"}}}}}}}}}},
    "InvokePermission":{{"Type":"AWS::Lambda::Permission","Properties":{{
      "FunctionName":{{"Fn::GetAtt":["Indexer","Arn"]}},
      "Action":"lambda:InvokeFunction","Principal":"s3.amazonaws.com"}}}},
    "IndexingErrors":{{"Type":"AWS::CloudWatch::Alarm","Properties":{{
      "Namespace":"AWS/Lambda","MetricName":"Errors","Statistic":"Sum","Period":300,
      "EvaluationPeriods":2,"Threshold":5,
      "ComparisonOperator":"GreaterThanOrEqualToThreshold"}}}}}},
  {OUTPUTS_JSON}}}"#
    )
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            case: "a named pipeline, YAML short form, a public domain",
            stack: "pipe-named",
            template: yaml_pipeline(""),
            parameters: parameters(&[("DomainName", "pipe-named-search")]),
            identity: Identity::Declared,
            domain: Some("pipe-named-search"),
        },
        Case {
            case: "the same estate as JSON, nothing named, a dualstack domain",
            stack: "pipe-json",
            template: json_pipeline(),
            parameters: Vec::new(),
            identity: Identity::Generated,
            domain: None,
        },
        Case {
            case: "a named pipeline whose domain is reached inside a VPC",
            stack: "pipe-vpc",
            template: yaml_pipeline(VPC_DOMAIN_PROPERTIES),
            parameters: parameters(&[("DomainName", "pipe-vpc-search")]),
            identity: Identity::Declared,
            domain: Some("pipe-vpc-search"),
        },
    ]
}

/// The `Parameters.member.<n>` pairs a create carries.
fn parameters(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .enumerate()
        .flat_map(|(index, (key, value))| {
            let position = index + 1;
            vec![
                (
                    format!("Parameters.member.{position}.ParameterKey"),
                    (*key).to_string(),
                ),
                (
                    format!("Parameters.member.{position}.ParameterValue"),
                    (*value).to_string(),
                ),
            ]
        })
        .collect()
}

/// What the OWNING service publishes for the domain under the `DomainStatus` member an
/// attribute of the type reports, as the single text that attribute reads as.
///
/// For every member but the two ENDPOINTS this is the member itself ([`reported_text`]). An
/// endpoint is the address a client submits its requests to, and a domain reached inside a VPC
/// has no public host at all: that service publishes its address in `DomainStatus$Endpoints`
/// instead, the IPv4 one under `vpc` and the dual-stack one under `vpcv2`, so for such a
/// domain those entries are the endpoints it publishes.
fn published_as(status: &Value, member: &str) -> Option<String> {
    let in_a_vpc = match member {
        "Endpoint" => ENDPOINTS_KEY_VPC,
        "EndpointV2" => ENDPOINTS_KEY_VPC_DUALSTACK,
        _ => return reported_text(status, member),
    };
    reported_text(status, member)
        .or_else(|| reported_text(status, &format!("Endpoints.{in_a_vpc}")))
}

/// The keys `DomainStatus$Endpoints` publishes a VPC-reached domain's hosts under, from that
/// member's own documentation (`smithy_query(query="doc",shape="DomainStatus",
/// member="Endpoints")`): the IPv4 host under `vpc` and the dual-stack one under `vpcv2`.
const ENDPOINTS_KEY_VPC: &str = "vpc";
const ENDPOINTS_KEY_VPC_DUALSTACK: &str = "vpcv2";

/// One member of a reported record, by the dotted path naming it, as the single text
/// `Fn::GetAtt` answers it as — or `None` where the record carries no such member.
///
/// A scalar has exactly one textual spelling (a string as itself, a number or a boolean as
/// its own text), which is the same rule the service's record and the stack's attribute are
/// held to; a member that is a whole container is no attribute at all.
fn reported_text(status: &Value, path: &str) -> Option<String> {
    let mut found = status;
    for segment in path.split('.') {
        found = found.get(segment)?;
    }
    match found {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        Value::Bool(flag) => Some(flag.to_string()),
        _ => None,
    }
}

/// The output one key names, panicking when the response does not report it at all: every
/// output a template declares is reported, whether or not its value resolved.
fn reported_output<'a>(outputs: &'a [WireOutput], key: &str, case: &str) -> &'a WireOutput {
    outputs
        .iter()
        .find(|output| output.key.as_deref() == Some(key))
        .unwrap_or_else(|| {
            panic!("case {case:?}: no Output with OutputKey {key:?}; got {outputs:?}")
        })
}

/// The value of one output, which the case requires to have resolved.
fn value_of(outputs: &[WireOutput], key: &str, case: &str) -> String {
    let output = reported_output(outputs, key, case);
    output.value.clone().unwrap_or_else(|| {
        panic!("case {case:?}: output {key:?} must carry an OutputValue, got {output:?}")
    })
}

/// Whether `value` is a physical id CloudFormation generated for `logical_id` of `stack`:
/// `<stack name>-<logical id>-<unique id>`, the unique segment being hexadecimal. No request
/// fixes that segment, so the rule is the FORM.
///
/// Compared without case, because the generated name is folded into the character set the
/// TYPE admits: an Amazon S3 bucket name is lower-case throughout, a log group's keeps the
/// logical id's case.
fn is_generated(value: &str, stack: &str, logical_id: &str) -> bool {
    value
        .to_ascii_lowercase()
        .strip_prefix(&format!("{stack}-{logical_id}-").to_ascii_lowercase())
        .is_some_and(|unique| {
            unique.len() == GENERATED_ID_HEX_DIGITS && unique.chars().all(|c| c.is_ascii_hexdigit())
        })
}

#[tokio::test]
async fn every_output_of_a_deployed_pipeline_carries_the_value_its_service_reports() {
    let router = build_router();

    for case in cases() {
        let label = case.case;
        let mut members: Vec<(&str, &str)> = vec![
            ("StackName", case.stack),
            ("TemplateBody", case.template.as_str()),
            // The estate declares an IAM role, and a role the template NAMES needs the named
            // acknowledgement; a request may acknowledge a capability its template does not
            // need, so every row carries both.
            ("Capabilities.member.1", "CAPABILITY_IAM"),
            ("Capabilities.member.2", "CAPABILITY_NAMED_IAM"),
        ];
        members.extend(
            case.parameters
                .iter()
                .map(|(key, value)| (key.as_str(), value.as_str())),
        );
        let (code, body) = cloudformation(&router, "CreateStack", &members).await;
        assert_eq!(
            code, 200,
            "case {label:?}: CreateStack must accept the template, got {code} with {body}"
        );

        // The domain's creation is held open until Amazon OpenSearch Service reports it
        // ready, so the stack settles a provisioning span after its own transition.
        let body = settle_stack(&router, label, case.stack).await;
        assert_eq!(
            xml_text(&body, "StackStatus"),
            "CREATE_COMPLETE",
            "case {label:?}: the stack must reach CREATE_COMPLETE before its outputs are \
             read, got {body}"
        );
        let outputs = reported_outputs(&body);

        // Every resource the pipeline declares reports the identity the row's rule gives it,
        // and each of its published attributes is built on that same identity — so the
        // outputs name the resources the stack created rather than agreeing with a literal.
        let bucket = value_of(&outputs, "BucketName", label);
        let role = value_of(&outputs, "RoleName", label);
        let log_group = value_of(&outputs, "LogGroupName", label);
        let function = value_of(&outputs, "FunctionName", label);
        let alarm = value_of(&outputs, "AlarmName", label);
        let domain = value_of(&outputs, "DomainRef", label);

        match case.identity {
            Identity::Declared => {
                for (key, expected) in [
                    ("BucketName", format!("{}-raw", case.stack)),
                    ("RoleName", format!("{}-indexer", case.stack)),
                    ("LogGroupName", format!("/aws/lambda/{}-indexer", case.stack)),
                    ("FunctionName", format!("{}-indexer", case.stack)),
                    ("AlarmName", format!("{}-indexer-errors", case.stack)),
                ] {
                    let actual = value_of(&outputs, key, label);
                    assert_eq!(
                        actual, expected,
                        "case {label:?}: output {key:?} must report the name the template \
                         declared, expected {expected:?}, got {actual:?}"
                    );
                }
                let expected = case.domain.unwrap_or_default();
                assert_eq!(
                    domain, expected,
                    "case {label:?}: output \"DomainRef\" must report the name the parameter \
                     gave the domain, expected {expected:?}, got {domain:?}"
                );
            }
            Identity::Generated => {
                for (key, logical_id, value) in [
                    ("BucketName", "RawBucket", &bucket),
                    ("RoleName", "IndexerRole", &role),
                    ("LogGroupName", "IndexerLogGroup", &log_group),
                    ("FunctionName", "Indexer", &function),
                    ("AlarmName", "IndexingErrors", &alarm),
                ] {
                    assert!(
                        is_generated(value, case.stack, logical_id),
                        "case {label:?}: output {key:?} must report the generated id \
                         {}-{logical_id}-<unique id>, got {value:?}",
                        case.stack
                    );
                }
                // A domain's name is generated under its own published bounds ("len 3..28,
                // pattern ^[a-z][a-z0-9\\-]+$"), so it is not the uniform generated form.
                assert!(
                    (3..=28).contains(&domain.len())
                        && domain.starts_with(|c: char| c.is_ascii_lowercase())
                        && domain
                            .chars()
                            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'),
                    "case {label:?}: output \"DomainRef\" must report a generated domain name \
                     inside the published bounds, got {domain:?}"
                );
            }
        }

        // The published attributes of each type, built on the identity the same response
        // reported: the five DNS names `AWS::S3::Bucket` publishes, and the ARN the uniform
        // naming gives the types that publish only one.
        for (key, expected) in [
            ("BucketArn", format!("arn:aws:s3:::{bucket}")),
            ("BucketDomainName", format!("{bucket}.s3.amazonaws.com")),
            (
                "BucketDualStackDomainName",
                format!("{bucket}.s3.dualstack.{REGION}.amazonaws.com"),
            ),
            (
                "BucketRegionalDomainName",
                format!("{bucket}.s3.{REGION}.amazonaws.com"),
            ),
            (
                "BucketWebsiteURL",
                format!("http://{bucket}.s3-website-{REGION}.amazonaws.com"),
            ),
            ("RoleArn", format!("arn:aws:iam::{ACCOUNT_ID}:role/{role}")),
            (
                "LogGroupArn",
                format!("arn:aws:logs:{REGION}:{ACCOUNT_ID}:log-group:{log_group}:*"),
            ),
            (
                "FunctionArn",
                format!("arn:aws:lambda:{REGION}:{ACCOUNT_ID}:function:{function}"),
            ),
            (
                "AlarmArn",
                format!("arn:aws:cloudwatch:{REGION}:{ACCOUNT_ID}:alarm:{alarm}"),
            ),
        ] {
            let actual = value_of(&outputs, key, label);
            assert_eq!(
                actual, expected,
                "case {label:?}: output {key:?} must name the resource the stack created, \
                 expected {expected:?}, got {actual:?}"
            );
        }

        // An identifier the OWNING service assigns is that service's: a role's `RoleId` is
        // the `AROA…` id IAM minted, never the role's name.
        let role_id = value_of(&outputs, "RoleId", label);
        assert!(
            role_id.starts_with(ROLE_ID_PREFIX)
                && role_id.len() > ROLE_ID_PREFIX.len()
                && role_id
                    .chars()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit()),
            "case {label:?}: output \"RoleId\" must report the {ROLE_ID_PREFIX}… id IAM \
             assigned, got {role_id:?}"
        );
        let permission = value_of(&outputs, "PermissionId", label);
        assert!(
            !permission.is_empty(),
            "case {label:?}: output \"PermissionId\" must report the permission's own id, got \
             {permission:?}"
        );

        // Every attribute of the DOMAIN is what Amazon OpenSearch Service itself reports for
        // the domain the stack created — read through that service's own API, so neither
        // half of the pair is a value this suite made up.
        let (code, status_document) = describe_domain(&router, &domain).await;
        assert_eq!(
            code, 200,
            "case {label:?}: DescribeDomain({domain}) must answer 200, got {code} with \
             {status_document}"
        );
        let mut without_value: Vec<&str> = Vec::new();
        for (key, member) in DOMAIN_ATTRIBUTE_OUTPUTS {
            let expected = published_as(&status_document, member);
            let output = reported_output(&outputs, key, label);
            assert_eq!(
                output.value, expected,
                "case {label:?}: output {key:?} must carry what the owning service reports \
                 as DomainStatus${member} ({expected:?}), got {:?}",
                output.value
            );
            if expected.is_none() {
                without_value.push(key);
            }
        }
        // The members that service reports for EVERY domain carry a value on every row, so a
        // pair of absences can never pass the comparison above vacuously.
        for key in ALWAYS_REPORTED_DOMAIN_OUTPUTS {
            let carried = value_of(&outputs, key, label);
            assert!(
                !carried.is_empty(),
                "case {label:?}: output {key:?} is one the owning service reports for every \
                 domain, so it must carry a value, got {carried:?}"
            );
        }
        // The domain of this estate is reachable, and the attribute a client addresses it
        // through carries the address the service published for it: the public endpoint for a
        // public domain, and the host under `vpc` for one reached inside a VPC — which is the
        // same host that map entry carries, since it is the one address such a domain has.
        let endpoint = value_of(&outputs, "DomainEndpoint", label);
        assert!(
            !endpoint.is_empty(),
            "case {label:?}: a deployed domain is addressed somewhere, so \"DomainEndpoint\" \
             must carry that address, got {outputs:?}"
        );
        if let Some(host) = reported_output(&outputs, "DomainVpcHost", label).value.clone() {
            assert_eq!(
                endpoint, host,
                "case {label:?}: a domain reached inside a VPC is addressed on the host its \
                 endpoint map carries, got {outputs:?}"
            );
        }

        // The rule over the WHOLE response rather than the keys named above: no reported
        // output lacks its value except the attributes the owning service itself reports
        // nothing for.
        for output in &outputs {
            let key = output.key.clone().unwrap_or_default();
            assert!(
                output.value.is_some() || without_value.contains(&key.as_str()),
                "case {label:?}: every Output of a CREATE_COMPLETE stack must carry an \
                 OutputValue; {key:?} carries none ({outputs:?})"
            );
        }
    }
}
