// SPDX-License-Identifier: CC0-1.0

//! The per-service quota ROW TABLES of the seeded default-quota catalog, and the
//! `UsageMetric` constructors they share.
//!
//! One function per `ServiceCode`, each returning the quotas that service publishes in
//! the order the catalog publishes them; [`super::seed_quota_catalog`] binds each to its
//! service code. They live here rather than beside the catalog's own machinery so that
//! the data — which is the bulk of the catalog and the part that grows with every
//! service code — is separate from the code that reads it.
//!
//! THE ONE RULE EVERY ROW OBEYS: a row published under a `QuotaCode` carries the
//! `QuotaName`, `Value`, `Unit`, `Adjustable`, `GlobalQuota`, `Period`, `UsageMetric`
//! and `Description` of the quota AWS publishes UNDER THAT CODE, never another quota's.
//! The code is the identity a client resolves a limit by, so where a row's published
//! facts are known only for one code, that code is what they are published under and no
//! neighbouring row borrows them.

use super::{quota, QuotaRow, LEVEL_ALL};
use crate::ops::types::MetricInfo;

// The row tables of the services whose catalogs run long enough to own a file: one module
// per `ServiceCode`, so a service's rows and the facts behind them sit together. They read
// the row builder and the shared `UsageMetric` / rate constructors below out of this
// module, which is what keeps one construction of each class for the whole catalog.
mod batch;
mod cassandra;
mod cloud9;
mod cloudhsm;
mod cloudtrail;
mod codeartifact;
mod codedeploy;
mod codepipeline;
mod cognito_identity;
mod cognito_idp;
mod comprehend;
mod dax;
mod directconnect;
mod dms;
mod docdb;
mod quicksight;
mod sns;
mod sqs;
mod sso;
mod storagegateway;
mod timestream;
mod vpc;
mod vpc_lattice;
mod wafv2;
mod workspaces;
mod xray;

pub(super) use batch::batch_quotas;
pub(super) use cassandra::cassandra_quotas;
pub(super) use cloud9::cloud9_quotas;
pub(super) use cloudhsm::cloudhsm_quotas;
pub(super) use cloudtrail::cloudtrail_quotas;
pub(super) use codeartifact::codeartifact_quotas;
pub(super) use codedeploy::codedeploy_quotas;
pub(super) use codepipeline::codepipeline_quotas;
pub(super) use cognito_identity::cognito_identity_quotas;
pub(super) use cognito_idp::cognito_idp_quotas;
pub(super) use comprehend::comprehend_quotas;
pub(super) use dax::dax_quotas;
pub(super) use directconnect::directconnect_quotas;
pub(super) use dms::dms_quotas;
pub(super) use docdb::docdb_quotas;
pub(super) use quicksight::quicksight_quotas;
pub(super) use sns::sns_quotas;
pub(super) use sqs::sqs_quotas;
pub(super) use sso::sso_quotas;
pub(super) use storagegateway::storagegateway_quotas;
pub(super) use timestream::timestream_quotas;
pub(super) use vpc::vpc_quotas;
pub(super) use vpc_lattice::vpc_lattice_quotas;
pub(super) use wafv2::wafv2_quotas;
pub(super) use workspaces::workspaces_quotas;
pub(super) use xray::xray_quotas;

/// Amazon API Gateway's published default-quota catalog.
///
/// SOURCE: AWS's published Service Quotas data for the `apigateway` service code
/// (what `aws service-quotas list-aws-default-service-quotas --service-code
/// apigateway` reports, which is the "Amazon API Gateway quotas" table of the API
/// Gateway Developer Guide). Every row carries the `L-` code AWS publishes the quota
/// under beside its published name, default value, unit and adjustability, so a
/// client that read a code out of AWS's catalog resolves the same quota here.
pub(super) fn apigateway_quotas() -> Vec<QuotaRow> {
    vec![
        quota("Regional APIs per Region", 600.0).code("L-01C8A9E0"),
        quota("Edge-optimized APIs per account", 120.0).code("L-05295E0B"),
        quota("Private APIs per Region", 600.0).code("L-154598D1"),
        quota("HTTP APIs per account per Region", 500.0).code("L-1D180A63"),
        quota("WebSocket APIs per account per Region", 600.0).code("L-20859C74"),
        quota("Resources per API", 300.0).code("L-24E7E662"),
        quota("Stages per API", 10.0).code("L-379E48B0"),
        quota("Routes per HTTP API", 300.0).code("L-3A613F94"),
        quota("Integrations per HTTP API", 300.0).code("L-45CC0E8B"),
        quota("Authorizers per API", 10.0).code("L-46624B39"),
        quota("Domain name access associations", 100.0)
            .code("L-4D98A8A5")
            .describe(
                "The maximum number of domain name access associations that you can create \
                 in this account in the current region",
            ),
        quota("API keys per account per Region", 10_000.0).code("L-5244589D"),
        quota("Client certificates per account per Region", 60.0).code("L-608BDCD4"),
        quota("Usage plans per account per Region", 300.0).code("L-60AC41CD"),
        quota("Usage plans per API key", 10.0).code("L-65B5C802"),
        quota("Custom domain names per account per Region", 120.0).code("L-668C9B28"),
        quota("Base path mappings per domain name", 200.0).code("L-68B79FF0"),
        quota("Documentation parts per API", 2000.0).code("L-7D4D47CD"),
        quota("VPC links per account per Region", 20.0).code("L-824C9E42"),
        quota("Models per API", 300.0).code("L-85120D8E"),
        quota("Request validators per API", 20.0).code("L-8A5B8E43"),
        // The account-level request rate and its burst allowance: a rate quota, so it
        // carries the one-second window AWS publishes it over.
        quota("Throttle rate", 10_000.0)
            .code("L-8B81B02C")
            .period(1, "SECOND"),
        quota("Throttle burst rate", 5000.0).code("L-8C2F9A1D").fixed(),
        quota("Integration timeout", 29_000.0)
            .code("L-8E6A5A87")
            .unit("Milliseconds")
            .fixed(),
        quota("Resource policy size", 8192.0)
            .code("L-8F8AAAFE")
            .unit("Bytes")
            .fixed(),
        quota("Stage variables per stage", 100.0).code("L-95BA6EA5"),
        quota("Stage Variable Name Length", 64.0)
            .code("L-985EB478")
            .fixed(),
        quota("Mapping template size", 300.0)
            .code("L-9C147DE4")
            .unit("Kilobytes")
            .fixed(),
        quota("WebSocket new connections rate", 500.0)
            .code("L-9ED1E49A")
            .period(1, "SECOND"),
        quota("WebSocket messages per second", 500.0)
            .code("L-A4C7274F")
            .period(1, "SECOND"),
        quota("WebSocket connection duration", 7200.0)
            .code("L-A6CCE716")
            .unit("Seconds")
            .fixed(),
        quota("WebSocket idle connection timeout", 600.0)
            .code("L-A7033131")
            .unit("Seconds")
            .fixed(),
        quota("WebSocket frame size", 32.0)
            .code("L-A93447B8")
            .unit("Kilobytes")
            .fixed(),
        quota("WebSocket message payload size", 128.0)
            .code("L-A966AB5C")
            .unit("Kilobytes")
            .fixed(),
        quota("Routes per WebSocket API", 300.0).code("L-A9DBC573"),
        quota("Authorizers per HTTP API", 10.0).code("L-AA0FF27B"),
        quota("Stage Variable Value Length", 512.0)
            .code("L-B2CF62DC")
            .fixed(),
        quota("Private custom domain names per account per Region", 120.0).code("L-B4D69DB7"),
        quota("Tags per API stage", 50.0).code("L-B97207D0").fixed(),
        quota("Payload size", 10.0)
            .code("L-CAEF46D2")
            .unit("Megabytes")
            .fixed(),
        quota("Total combined size of all header values", 10_240.0)
            .code("L-CC2525B6")
            .unit("Bytes")
            .fixed(),
        quota("VPC links per account per Region for HTTP APIs", 10.0).code("L-CDF5615A"),
        quota("Gateway responses per API", 20.0)
            .code("L-E11F7D5D")
            .fixed(),
        quota("Documentation versions per API", 200.0).code("L-E1465507"),
        quota("Custom authorizer result TTL", 3600.0)
            .code("L-E5AE38E3")
            .unit("Seconds"),
        quota("Iterations in a mapping template loop", 1000.0)
            .code("L-E8693075")
            .fixed(),
        quota("Rate of API management requests", 10.0)
            .code("L-E9EEB922")
            .period(1, "SECOND"),
        quota("Maximum Logo Size", 200.0)
            .code("L-F8A34CAF")
            .unit("Kilobytes"),
        quota("Integrations per API", 300.0).code("L-F8BD84D3"),
        quota("Authorizers per WebSocket API", 10.0).code("L-FB4F0270"),
        quota("Tags per API", 50.0).code("L-FD0EB744").fixed(),
    ]
}

/// AWS AppConfig's published default-quota catalog.
///
/// SOURCE: AWS's published Service Quotas data for the `appconfig` service code (the
/// "AWS AppConfig quotas" table of the AWS AppConfig User Guide), whose quota names
/// are published in the "Maximum number of …" form the rows below carry.
pub(super) fn appconfig_quotas() -> Vec<QuotaRow> {
    vec![
        quota("Maximum number of treatments per experiment", 5.0).code("L-08D2C632"),
        quota(
            "Configuration size limit in AWS AppConfig hosted configuration store",
            2048.0,
        )
        .code("L-48F9B951")
        .unit("Kilobytes"),
        // A quota AWS publishes over a DAY rather than per resource: the deployment
        // starts an account may make in one day, metered by that API's own
        // `AWS/Usage` `CallCount` metric.
        quota("Maximum number of deployments per day", 100.0)
            .code("L-A52E46BE")
            .period(1, "DAY")
            .metric(call_count_metric("AppConfig", "StartDeployment")),
        quota(
            "Maximum number of configuration profiles per application",
            100.0,
        )
        .code("L-A5FC0339"),
        quota("Maximum number of experiment definitions per application", 10.0)
            .code("L-B858C085"),
        quota("Maximum number of environments per application", 100.0).code("L-EEB0151E"),
        quota("Maximum number of deployment strategies per Region", 20.0).code("L-F59D302B"),
        quota("Maximum number of applications per Region", 100.0).code("L-FA210A1F"),
    ]
}

/// Application Auto Scaling's published default-quota catalog.
///
/// SOURCE: AWS's published Service Quotas data for the `application-autoscaling`
/// service code. Application Auto Scaling publishes one scalable-target quota per
/// SCALABLE SERVICE (the service whose resources it scales) beside the per-target
/// policy and scheduled-action quotas, which is why the rows are named for the
/// services rather than for a resource type.
pub(super) fn application_autoscaling_quotas() -> Vec<QuotaRow> {
    vec![
        quota("Scalable targets for Comprehend", 500.0).code("L-17D5F681"),
        quota("Scalable targets for AppStream", 500.0).code("L-1A11EB4B"),
        quota("Scalable targets for Aurora", 500.0).code("L-1AAF0700"),
        quota("Scalable targets for EC2", 500.0).code("L-297D9EC9"),
        quota("Scalable targets for Custom Resource", 500.0).code("L-4A11ECEB"),
        quota("Scalable targets for DynamoDB", 500.0).code("L-60E4E5E2"),
        quota("Scalable targets for ECS", 500.0).code("L-782A3EE2"),
        quota("Scalable targets for Cassandra", 500.0).code("L-799ACCDF"),
        quota("Scalable targets for ElastiCache", 500.0).code("L-800F6F7B"),
        quota("Scalable targets for EMR", 500.0).code("L-8998F403"),
        quota("Scalable targets for Lambda", 500.0).code("L-95848B5F"),
        quota("Scalable targets for MSK", 500.0).code("L-9C25247C"),
        quota("Scalable targets for Neptune", 500.0).code("L-A1D42901"),
        quota("Scalable targets for SageMaker", 500.0).code("L-A2AD6458"),
        // The per-target planes AWS publishes as not adjustable.
        quota("Scaling policies per scalable target", 50.0)
            .code("L-B395C81B")
            .fixed(),
        quota("Scheduled actions per scalable target", 200.0)
            .code("L-D75CA9D2")
            .fixed(),
    ]
}

/// AWS App Mesh's published default-quota catalog.
///
/// SOURCE: AWS's published Service Quotas data for the `appmesh` service code (the
/// "App Mesh service quotas" table of the AWS App Mesh User Guide).
pub(super) fn appmesh_quotas() -> Vec<QuotaRow> {
    vec![
        quota("Connected Envoy processes per virtual gateway", 50.0).code("L-33E8F9C9"),
        quota("Virtual routers per mesh", 200.0).code("L-50F6C35A"),
        quota("Connected Envoy processes per virtual node", 50.0).code("L-606A910B"),
        // The backend plane of a virtual node, which AWS publishes as fixed.
        quota("Backends per virtual node", 50.0)
            .code("L-8775AB18")
            .fixed(),
        quota("Targets per route", 10.0).code("L-87E74146"),
        quota("Meshes per account", 15.0).code("L-AC861A39"),
        quota("Virtual gateways per mesh", 15.0).code("L-AE1D9567"),
        quota("Routes per virtual router", 50.0).code("L-BB90B7FF"),
        quota("Virtual services per mesh", 200.0).code("L-DA7495A7"),
        quota("Virtual nodes per mesh", 200.0).code("L-E043DFB4"),
        quota("Gateway routes per virtual gateway", 10.0).code("L-F6F26D09"),
    ]
}

/// AWS App Runner's published default-quota catalog.
///
/// SOURCE: AWS's published Service Quotas data for the `apprunner` service code (the
/// "App Runner service quotas" table of the AWS App Runner Developer Guide), whose
/// quota names are the resource names themselves.
pub(super) fn apprunner_quotas() -> Vec<QuotaRow> {
    vec![
        quota("Connections", 10.0).code("L-1BDBAAB6"),
        quota("Auto scaling configurations", 10.0).code("L-1D8C5BDD"),
        // AWS publishes the ingress connection allowance per VPC as fixed.
        quota("VPC ingress connections", 1.0)
            .code("L-4064914D")
            .fixed(),
        quota("Services", 30.0).code("L-69F96A0C"),
        quota("Observability configurations", 10.0).code("L-A0B46A0C"),
        quota("VPC connectors", 10.0).code("L-F7ADEB8C"),
    ]
}

/// The pair of quotas Athena publishes for the calls of ONE of its APIs: the
/// REPLENISHMENT RATE of the token bucket those calls are served from (a per-second
/// rate, metered by the API's `AWS/Usage` `CallCount` metric) and the BURST
/// MULTIPLIER on it, which AWS publishes as not adjustable.
fn athena_api_call_rate(
    replenishment_code: &str,
    rate: f64,
    burst_code: &str,
    burst: f64,
    api_name: &str,
) -> [QuotaRow; 2] {
    [
        quota(format!("Replenishment rate of {api_name} API calls"), rate)
            .code(replenishment_code)
            .period(1, "SECOND")
            .metric(call_count_metric("Athena", api_name)),
        quota(format!("Burst multiplier quota for {api_name} API"), burst)
            .code(burst_code)
            .fixed(),
    ]
}

/// Amazon Athena's published default-quota catalog.
///
/// SOURCE: AWS's published Service Quotas data for the `athena` service code (the
/// "Service Quotas" page of the Amazon Athena User Guide): the query and workgroup
/// quotas, plus the API call-rate pair every throttled Athena API publishes.
pub(super) fn athena_quotas() -> Vec<QuotaRow> {
    let mut quotas = vec![
        quota("Active DML queries", 20.0).code("L-56A94400"),
        quota("DDL query timeout", 600.0)
            .code("L-0F303755")
            .unit("Minutes")
            .fixed(),
        quota("DML query timeout", 30.0)
            .code("L-136E993A")
            .unit("Minutes"),
        quota("Workgroups per Region", 1000.0).code("L-3702C1C1"),
        quota("Active DDL queries", 20.0).code("L-3CE0BBA0"),
        quota("Prepared statements per workgroup", 1000.0).code("L-41075247"),
        quota("Data catalogs per Region", 1000.0).code("L-4FF11DD3"),
        quota("Named queries per workgroup", 1000.0).code("L-52EFA794"),
    ];
    for pair in [
        athena_api_call_rate("L-076245C0", 5.0, "L-5A8D5237", 2.0, "CreateNamedQuery"),
        athena_api_call_rate("L-70FD91F7", 5.0, "L-722F8D5C", 2.0, "GetNamedQuery"),
        athena_api_call_rate("L-76AFE6F2", 5.0, "L-7A12A1E6", 2.0, "DeleteNamedQuery"),
        athena_api_call_rate("L-82E47245", 100.0, "L-9D86AA19", 2.0, "GetQueryExecution"),
        athena_api_call_rate("L-A6257554", 20.0, "L-AC64E3A0", 2.0, "BatchGetQueryExecution"),
        athena_api_call_rate("L-07F4444F", 100.0, "L-B13EED8E", 2.0, "GetQueryResults"),
        athena_api_call_rate("L-C60BEAE0", 20.0, "L-CC64F7CE", 2.0, "GetWorkGroup"),
        athena_api_call_rate("L-D0D8C912", 5.0, "L-D8C0E3AF", 2.0, "ListNamedQueries"),
        athena_api_call_rate("L-E20AD6B8", 20.0, "L-FE3693EE", 2.0, "ListQueryExecutions"),
        athena_api_call_rate("L-E80DC288", 20.0, "L-EFB619E3", 2.0, "StartQueryExecution"),
        athena_api_call_rate("L-FC5F6546", 20.0, "L-FD9D80C2", 2.0, "StopQueryExecution"),
    ] {
        quotas.extend(pair);
    }
    quotas
}

/// Amazon EC2 Auto Scaling's published default-quota catalog.
///
/// SOURCE: AWS's published Service Quotas data for the `autoscaling` service code
/// (the "Quotas for Amazon EC2 Auto Scaling" table of its user guide). The group
/// count is the one quota CloudWatch publishes usage for, under the service's own
/// `AWS/Usage` dimension.
pub(super) fn autoscaling_quotas() -> Vec<QuotaRow> {
    vec![
        // The per-group planes AWS publishes as fixed.
        quota("Target groups per Auto Scaling group", 50.0)
            .code("L-05CB8B12")
            .fixed(),
        quota("Lifecycle hooks per Auto Scaling group", 50.0)
            .code("L-1312BBBF")
            .fixed(),
        quota("Launch configurations per region", 200.0).code("L-6B80B8FA"),
        quota("Step adjustments per step scaling policy", 20.0)
            .code("L-6C2A2F6E")
            .fixed(),
        quota("Scaling policies per Auto Scaling group", 50.0)
            .code("L-72753F6F")
            .fixed(),
        quota("Auto Scaling groups per region", 500.0)
            .code("L-CDE20ADC")
            .metric(usage_metric(
                "AutoScaling",
                &[
                    ("Class", "None"),
                    ("Resource", "NumberOfAutoScalingGroup"),
                    ("Type", "Resource"),
                ],
            )),
        quota("Notification configurations per Auto Scaling group", 10.0).code("L-CEE5E714"),
        quota("Scheduled actions per Auto Scaling group", 125.0).code("L-F0B00D71"),
        quota("Classic Load Balancers per Auto Scaling group", 50.0).code("L-F786B2E5"),
    ]
}

/// AWS Backup's published default-quota catalog.
///
/// SOURCE: AWS's published Service Quotas data for the `backup` service code (the
/// "AWS Backup quotas" table of the AWS Backup Developer Guide), whose per-Region
/// quotas are published in the "… per Region per account" form the rows below carry.
pub(super) fn backup_quotas() -> Vec<QuotaRow> {
    vec![
        // The rule plane of a backup plan, which AWS publishes as fixed.
        quota("Rules per backup plan", 10.0)
            .code("L-366B61FD")
            .fixed(),
        quota("Recovery points per backup vault", 1_000_000.0).code("L-514878B6"),
        quota("Controls per framework", 50.0).code("L-7705D2CB"),
        quota("Backup selections per backup plan", 50.0).code("L-9122A82F"),
        // AWS publishes the framework allowance of a report plan as fixed.
        quota("Frameworks per report plan", 1000.0)
            .code("L-9CAF0B92")
            .fixed(),
        quota("Backup vaults per Region per account", 100.0).code("L-B4021FB0"),
        quota("Backup plans per Region per account", 300.0).code("L-BD69F607"),
        quota("Legal holds per account", 50.0).code("L-C0A8C14B"),
        quota("Report plans per Region per account", 20.0).code("L-C296F1F5"),
        quota("Frameworks per Region per account", 10.0).code("L-E43E0ED6"),
        quota("Restore testing plans per Region per account", 10.0).code("L-FB1D55EC"),
        quota("Tags per backup plan", 50.0).code("L-FFD6444F"),
    ]
}

/// EC2's account-wide resource quotas: the published EC2 quotas that are neither
/// vCPU quotas nor Dedicated Hosts quotas.
pub(super) fn ec2_resource_quotas() -> Vec<QuotaRow> {
    vec![
        quota("EC2-VPC Elastic IPs", 5.0).code("L-0263D0A3"),
        quota("AMI sharing", 500.0),
        quota("Launch templates per Region", 5000.0),
        quota("Versions per launch template", 10000.0),
        quota("Key pairs per Region", 5000.0),
        quota("Placement groups per Region", 500.0),
        quota("Partitions per partition placement group", 7.0).fixed(),
        quota("Active Spot Fleet requests per Region", 1000.0),
        quota("Target capacity per Spot Fleet request", 5000.0),
        quota("Active EC2 Fleets per Region", 1000.0),
        quota("Target capacity per EC2 Fleet", 5000.0),
        quota("Attributes per launch template", 300.0).fixed(),
    ]
}

/// Amazon EventBridge's published default-quota catalog, under the `ServiceCode` its
/// API endpoint uses.
///
/// SOURCE: AWS's published Service Quotas data for the `events` service code (the
/// "Amazon EventBridge quotas" page of the Amazon EventBridge User Guide, which is
/// what `aws service-quotas list-aws-default-service-quotas --service-code events`
/// reports). EventBridge publishes its resource planes beside a per-API REQUEST RATE
/// for each throttled API, which it names "<Api> throttle limit in transactions per
/// second": a rate quota, so each carries the one-second window AWS publishes it over
/// and the `AWS/Usage` `CallCount` metric of the API it throttles.
///
/// Which `QuotaCode` names which quota, and each code's adjustability, come from the
/// recorded default-quota listing of `events`: the code is the quota's identity, so a
/// row carries the name, value, unit, adjustability, window and metric of the quota
/// AWS publishes UNDER THAT CODE, never another quota's. Where that listing shows a
/// code names a quota whose name and value this catalog cannot cite, the row keeps the
/// facts the listing does state for it and no other row borrows the code.
///
/// A quota this catalog can name but whose AWS `QuotaCode` that listing does not state
/// is NOT published under another quota's code, and the stand-in code is not published
/// for this service either (the listing keys every row it carries on an AWS `L-` code,
/// so a stand-in row is a surplus entry): the replay plane, the `PutEvents` rate and
/// the `TestEventPattern` rate are therefore left to the default-quota-set gap rather
/// than served under the codes the listing binds to the `DeleteEndpoint` rate, the
/// event-bus plane and the event-bus policy size. The code this catalog had served the
/// event-bus plane under a second time is left to that same gap: a quota it cannot cite
/// is published under no code at all.
pub(super) fn events_quotas() -> Vec<QuotaRow> {
    vec![
        // The RULE plane of an event bus as the account's own record reports it, under
        // the code the recorded listing binds it to and with the sentence that listing
        // carries for it — which is published without a closing period.
        quota("Number of rules", 300.0)
            .code("L-244521F2")
            .describe("Maximum number of rules an account can have per event bus"),
        // The target plane of a rule, which AWS publishes as fixed, under the code the
        // recorded listing binds it to, with the description that listing carries.
        quota("Targets per rule", 5.0)
            .code("L-388D1D08")
            .fixed()
            .describe("Maximum number of targets that can be associated with a rule"),
        // The rule plane counted ON ONE EVENT BUS: the recorded row for this code
        // publishes a resource count over the window those counts are published on and a
        // per-resource context, so it is a counted plane of a resource rather than a
        // per-second rate, and an increase can be asked for one event bus.
        quota("Rules per event bus", 300.0)
            .code("L-39263BDC")
            .adjustable_at(LEVEL_ALL, "AWS::Events::EventBus")
            .counted_per_resource()
            .metric(usage_metric(
                "EventBridge",
                &[("Class", "None"), ("Resource", "Rule"), ("Type", "Resource")],
            ))
            .describe("The maximum number of rules per event bus."),
        quota("Archives", 100.0)
            .code("L-3C47459F")
            .fixed()
            .describe("The maximum number of archives in this account in the current Region."),
        // The DeleteEndpoint rate, one of the endpoint-API rates the service publishes
        // as fixed: the recorded listing binds this code to a rate — it carries the
        // one-second window of the class — and not to a counted plane.
        events_request_rate("L-5425B7E9", "DeleteEndpoint", 5.0).fixed(),
        // The INVOCATION rate of the account: published like a per-API rate — a
        // one-second window and an `AWS/Usage` `CallCount` metric — but metered under
        // the `Invocations` series rather than one API's, and described by what an
        // invocation IS rather than by the per-API sentence the rate class carries.
        events_request_rate("L-5540C5E3", "Invocations", 18_750.0).describe(
            "An invocation is an event matching a rule and being sent on to the rule's \
             targets. After the limit is reached, the invocations are throttled; that is, \
             they still happen but they are delayed.",
        ),
        events_request_rate("L-595D6D42", "PutRule", 50.0),
        quota("Connections", 3000.0)
            .code("L-5D1D13E3")
            .describe("The maximum number of connections in this account in the current Region."),
        // The EVENT BUS plane of an account, which the recorded listing publishes under
        // this code with the sentence it carries for it.
        quota("Event buses", 100.0)
            .code("L-658A4FD9")
            .describe("Maximum event buses per account."),
        // The size of the pattern a rule matches events with, which AWS publishes in
        // CHARACTERS: a measured quota rather than a request rate, so it carries no
        // window and no call-count metric.
        quota("Event pattern size", 2048.0)
            .code("L-664C5505")
            .describe("Maximum size of an event pattern, in characters."),
        // The invocation rate of ONE API destination endpoint — the rate EventBridge
        // sends invocations to the endpoint at, not the rate of an API call — under the
        // code the recorded listing binds it to, with the value and the sentence that
        // listing carries for it.
        quota("Rate of invocations per API destination", 300.0)
            .code("L-755FD01C")
            .period(1, "SECOND")
            .describe(
                "The maximum number of invocations per second to send to each API \
                 destination endpoint per account per Region. Once the quota is met, \
                 future invocations to that API endpoint are throttled. The invocations \
                 will still occur, but are delayed.",
            ),
        events_request_rate("L-8331B29E", "PutTargets", 50.0).fixed(),
        events_request_rate("L-9B653E91", "RemoveTargets", 50.0),
        events_request_rate("L-A8AFA624", "UpdateEndpoint", 5.0).fixed(),
        events_request_rate("L-CB72B930", "CreateEndpoint", 5.0).fixed(),
        // NOT published here: the code this catalog had served the account's event-bus
        // plane under a second time. That plane is published under L-658A4FD9 above, the
        // code the listing binds it to, and the listing states no quota this catalog can
        // cite under this one, so it is left to the default-quota-set gap rather than
        // paired with a quota by guesswork.
        // An EventBridge v2 RESOURCE plane: a count of resources in use rather than a
        // request rate, so it is metered by that plane's `AWS/Usage` `ResourceCount`
        // over the window those counts are published on, not by an API call count.
        quota("[EventsV2] Event Buses", 5.0)
            .code("L-DF788473")
            .metric(usage_metric(
                "EventBridge",
                &[
                    ("Class", "None"),
                    ("Resource", "EventsV2/EventBus"),
                    ("Type", "Resource"),
                ],
            ))
            .describe(
                "The maximum number of V2 Event Buses owned by this account per Region. \
                 Does not include buses shared with this account.",
            ),
        events_request_rate("L-E2110C81", "ListRules", 50.0).fixed(),
        quota("Endpoints", 100.0)
            .code("L-EAC9A2AC")
            .describe("The maximum number of endpoints per account per Region."),
        events_request_rate("L-FB1C3A6D", "ListTargetsByRule", 50.0),
        // The SIZE of an event bus's resource policy, which AWS publishes in CHARACTERS
        // and points at the API a caller reads the current size with: a measured quota
        // rather than a request rate, so it carries no window and no call-count metric.
        quota("Event bus policy size", 10_240.0)
            .code("L-FC354966")
            .describe(
                "Maximum policy size, in characters. You can view the current policy size \
                 for an event bus by using the DescribeEventBus API.",
            ),
    ]
}

/// One EventBridge per-API request-rate quota: the transactions per second EventBridge
/// serves calls of that one API at, metered by the API's own `AWS/Usage` `CallCount`
/// metric under the service's `AWS/Usage` display name, beside the sentence EventBridge
/// publishes for a request-rate quota ([`events_api_rate_description`]).
fn events_request_rate(code: &str, api_name: &str, transactions_per_second: f64) -> QuotaRow {
    quota(
        format!("{api_name} throttle limit in transactions per second"),
        transactions_per_second,
    )
    .code(code)
    .period(1, "SECOND")
    .metric(call_count_metric("EventBridge", api_name))
    .describe(&events_api_rate_description(api_name))
}

/// The `Description` EventBridge publishes for a per-API REQUEST-RATE quota, with the
/// API it throttles named in it.
///
/// SOURCE: the recorded default-quota listing of `events`, which carries this exact
/// sentence for the rate quota of its `CreateEndpoint` API. It is the wording of the
/// whole class — EventBridge words a throttled API's rate as a number of requests per
/// second for that API, not as a number of requests an account may make — so one
/// constructor fills it and two rows of the class cannot drift apart. It is NOT the
/// sentence other services word the same fact with ([`request_rate_description`]): pick
/// the one the service's own recorded rows carry.
fn events_api_rate_description(api_name: &str) -> String {
    format!(
        "The maximum number of requests per second for {api_name} API. Additional \
         requests are throttled."
    )
}

/// The `Description` AWS publishes for a per-API REQUEST-RATE quota, with the operation
/// it throttles named in it.
///
/// SOURCE: the recorded default-quota listing of `firehose`, which carries this exact
/// sentence for the rate quota of its `DeleteDeliveryStream` API. It is the wording of
/// the whole class, so one constructor fills it and two rows of the class cannot drift
/// apart.
///
/// The sentence is PER SERVICE: a service whose recorded rows word their rate differently
/// has its own constructor beside this one ([`api_rate_description`] for the service whose
/// rows read "The maximum number of <Api> API requests per second."), so a third copy is
/// never needed — pick the one the service's own recorded rows carry.
fn request_rate_description(api_name: &str) -> String {
    format!(
        "The maximum number of {api_name} requests you can make per second in this account \
         in the current Region."
    )
}

/// The `Description` Service Quotas publishes for a quota that throttles ONE API operation
/// to so many requests PER SECOND, for the service whose recorded rows word it this way.
///
/// The per-second wording is what distinguishes a RATE quota from a count of the same name,
/// and it is fixed text around the operation's own name — the recorded applied-quota
/// listing of `lambda` carries exactly this sentence for its `Rate of <Api> API requests`
/// rows — so every row of the class is described through this one function rather than by a
/// sentence written out per row. It sits beside [`request_rate_description`] because the
/// two are the SAME fact worded as two services publish it.
fn api_rate_description(api_name: &str) -> String {
    format!("The maximum number of {api_name} API requests per second.")
}

/// The `Description` Service Quotas publishes for the BURST allowance beside such a rate:
/// how many of that operation's requests may arrive AT ONCE rather than the steady rate
/// they may keep up.
///
/// SOURCE: the recorded applied-quota listing of `lambda`, which carries this exact
/// sentence for the burst allowance of its `ResumeMicrovm` API. Like
/// [`api_rate_description`] it is fixed text around the operation's own name, so every row
/// of the class is described through this one function rather than by a sentence written
/// out per row.
fn api_burst_rate_description(api_name: &str) -> String {
    format!("The maximum number of {api_name} API requests that can be sent in one burst.")
}

/// One quota published as a COUNT of the resources of one plane in use: the plane's
/// `AWS/Usage` `ResourceCount` meters it ([`usage_metric`]), so the row also carries the
/// window those counts are published over, and its `Description` is the sentence AWS
/// publishes for a counted plane with what the plane counts named in it.
///
/// `usage_service_name` is the service's `AWS/Usage` display name, never its
/// `ServiceCode` (see [`aws_usage_metric`]); `resource` is the series CloudWatch
/// publishes the count under and `counted` what the quota counts, as the quota's own
/// published text names it. One constructor fills the class so two metered planes
/// cannot drift apart, the way [`request_rate`] does for the rate class.
fn resource_count_plane(
    code: &str,
    quota_name: &str,
    value: f64,
    usage_service_name: &str,
    resource: &str,
    counted: &str,
) -> QuotaRow {
    quota(quota_name, value)
        .code(code)
        .metric(usage_metric(
            usage_service_name,
            &[
                ("Class", "None"),
                ("Resource", resource),
                ("Type", "Resource"),
            ],
        ))
        .describe(&format!(
            "The maximum number of {counted} in this account in the current Region."
        ))
}

/// One per-API REQUEST-RATE quota of a service that publishes its API throttles under
/// the name `Rate of <Api> requests`: a rate, so it carries the one-second window AWS
/// publishes such a quota over and the `AWS/Usage` `CallCount` metric of the API it
/// throttles ([`call_count_metric`]), beside the sentence AWS publishes for the class
/// with the operation named in it.
///
/// `usage_service_name` is the service's `AWS/Usage` display name, never its
/// `ServiceCode` (see [`aws_usage_metric`]).
fn request_rate(
    code: &str,
    usage_service_name: &str,
    api_name: &str,
    requests_per_second: f64,
) -> QuotaRow {
    quota(
        format!("Rate of {api_name} requests"),
        requests_per_second,
    )
    .code(code)
    .period(1, "SECOND")
    .metric(call_count_metric(usage_service_name, api_name))
    .describe(&request_rate_description(api_name))
}

/// Amazon Data Firehose's published default-quota catalog.
///
/// SOURCE: AWS's published Service Quotas data for the `firehose` service code (the
/// "Amazon Data Firehose quotas" page of its developer guide): the delivery-stream
/// planes beside the control-plane API rates that page publishes as so many
/// transactions per second. Which `QuotaCode` names which API's rate, that the service
/// publishes its control-plane rates as NOT adjustable, the `AWS/Usage` display name
/// the usage of each is published under, and the API each row's metric is published
/// against come from the recorded default-quota listing of `firehose`.
///
/// That listing binds three codes to the DATA-PLANE rates of a delivery stream (the
/// records, the Put requests and the data it takes per second) rather than to the
/// `ListDeliveryStreams`, `PutRecord` and `PutRecordBatch` call rates this catalog had
/// published under them; those three call rates name no code the listing states, so
/// they are left to the default-quota-set gap rather than served under a data-plane
/// quota's code.
pub(super) fn firehose_quotas() -> Vec<QuotaRow> {
    // Firehose publishes its control-plane API rates as NOT adjustable.
    let rate = |code: &str, api: &str, per_second: f64| {
        request_rate(code, FIREHOSE_USAGE_SERVICE, api, per_second).fixed()
    };
    vec![
        rate("L-040D05FF", "CreateDeliveryStream", 5.0),
        rate("L-09E8B729", "StopDeliveryStreamEncryption", 5.0),
        rate("L-0D32C2BC", "StartDeliveryStreamEncryption", 5.0),
        // The delivery-stream plane of the account: a count of RESOURCES, so it is
        // metered by that plane's `AWS/Usage` resource count rather than by the call
        // count of an API, and carries no one-second rate window. The resource count of
        // this plane is published under the service's SHORT `AWS/Usage` display name,
        // which is the `Service` dimension the recorded row carries — not the longer
        // one its API call counts are published under.
        quota("Delivery streams", 5000.0)
            .code("L-14BB0BE7")
            .metric(usage_metric(
                "Firehose",
                &[
                    ("Class", "None"),
                    ("Resource", "DeliveryStreams"),
                    ("Type", "Resource"),
                ],
            ))
            .describe(
                "The maximum number of delivery streams you can create in this account in the \
                 current Region.",
            ),
        // The data-plane RATES of one delivery stream — the records, the Put requests and
        // the data a stream takes per second — which the recorded listing binds to these
        // three codes. Each is a rate, so it carries the one-second window the service
        // publishes it over, and none of them is an API call count. The listing's
        // published sentence for the three is not stated by any source available here, so
        // the rows carry NO `Description` rather than one this catalog would be wording
        // itself (`tests/all/recorded_rows_events_through_imagebuilder.rs` pins exactly
        // these three as the rows published without one).
        quota("Rate of records", 500_000.0)
            .code("L-17D4EADE")
            .fixed()
            .period(1, "SECOND"),
        // The DYNAMIC PARTITION plane of one delivery stream, which the service
        // publishes as fixed: a count of the partitions a stream may have rather than a
        // request rate, so it carries neither a window nor an API's call count.
        quota("Dynamic Partitions", 500.0)
            .code("L-23F03908")
            .fixed()
            .describe(
                "The maximum number of dynamic partitions for a delivery stream in the \
                 current Region.",
            ),
        rate("L-36A7E04C", "ListTagsForDeliveryStream", 5.0),
        rate("L-3A800C3A", "DeleteDeliveryStream", 5.0),
        rate("L-47C73A38", "UntagDeliveryStream", 5.0),
        rate("L-61E2CA1D", "DescribeDeliveryStream", 5.0),
        quota("Rate of Put requests", 2000.0)
            .code("L-7681C790")
            .fixed()
            .period(1, "SECOND"),
        quota("Rate of data", 5.0)
            .code("L-937C879B")
            .fixed()
            .period(1, "SECOND"),
        rate("L-CC1FDDAA", "UpdateDestination", 5.0),
        rate("L-ED7DA7FC", "TagDeliveryStream", 5.0),
        // A control-plane rate the service meters in its OWN metric namespace rather
        // than with an `AWS/Usage` call count, which is the metric the recorded row for
        // this code carries.
        quota("Rate of ListDeliveryStream requests", 5.0)
            .code("L-FDED3C5B")
            .period(1, "SECOND")
            .metric(firehose_request_metric("ListDeliveryStreams.Requests"))
            .fixed()
            .describe(&request_rate_description("ListDeliveryStream")),
    ]
}

/// The `UsageMetric` of an Amazon Data Firehose quota the service meters in its OWN
/// metric namespace rather than in `AWS/Usage`.
///
/// Such a metric needs no dimension to identify the series it measures, so it carries
/// the EMPTY dimension map the model's `@length 0..10` allows, and its statistic is
/// `Sum` because a rate is the requests ADDED UP over the quota's window.
fn firehose_request_metric(metric_name: &str) -> MetricInfo {
    MetricInfo {
        metric_namespace: Some("AWS/Firehose".to_string()),
        metric_name: Some(metric_name.to_string()),
        metric_dimensions: Some(std::collections::HashMap::new()),
        metric_statistic_recommendation: Some("Sum".to_string()),
    }
}

/// The name `AWS/Usage` publishes Amazon Data Firehose's API call counts under.
///
/// SOURCE: the `Service` dimension of the `AWS/Usage` `CallCount` metrics the recorded
/// default-quota listing of `firehose` carries. It is the value a `GetMetricData` query
/// has to name, so it is held once here rather than spelled at each row.
const FIREHOSE_USAGE_SERVICE: &str = "Kinesis Firehose";

/// AWS Firewall Manager's published default-quota catalog.
///
/// SOURCE: AWS's published Service Quotas data for the `fms` service code (the "AWS
/// Firewall Manager quotas" page of its developer guide): the organization-wide
/// policy, list and resource-set planes, beside the per-API request rates the service
/// publishes for its control plane. Which `QuotaCode` names which quota and each code's
/// adjustability come from the recorded default-quota listing of `fms`, which binds the
/// code this catalog had published the `BatchAssociateResource` rate under to the Amazon
/// VPC instances a common security group policy may have in scope; that call rate names
/// no code the listing states, so it is left to the default-quota-set gap.
pub(super) fn fms_quotas() -> Vec<QuotaRow> {
    let rate =
        |code: &str, api: &str, per_second: f64| request_rate(code, "Firewall Manager", api, per_second);
    vec![
        quota("Applications per application list", 50.0)
            .code("L-0745D646")
            .describe(
                "The maximum number of applications that you can define in an application \
                 list.",
            ),
        quota("Applications lists per organization", 20.0)
            .code("L-07FCB28B")
            .describe("The maximum number of application lists per organization."),
        quota("Protocols lists per organization", 20.0)
            .code("L-0B28E140")
            .describe("The maximum number of protocols lists per organization."),
        // The custom-managed application lists an ACCOUNT may define, under the code the
        // recorded listing binds that plane to, with the value and the sentence that
        // listing carries for it.
        quota("Custom managed application lists per account", 10.0)
            .code("L-0E131699")
            .describe(
                "The maximum number of custom managed application lists that you can \
                 define for an account.",
            ),
        quota("Partner rule groups per AWS WAF policy", 1.0)
            .code("L-1608788B")
            .describe(
                "The maximum number of partner rule groups that you can include in an AWS WAF \
                 policy.",
            ),
        rate("L-1E778CA5", "AssociateThirdPartyFirewall", 5.0).fixed(),
        // The Amazon VPC instances one COMMON SECURITY GROUP policy may have in scope,
        // which the recorded listing publishes under this code: a counted plane rather
        // than a request rate, so it carries neither a window nor an API's call count.
        quota(
            "Amazon VPC instances in scope of a common security group policy",
            100.0,
        )
        .code("L-25F99602")
        .describe(
            "The maximum number of Amazon VPC instances that can be in scope of a common \
             security group policy.",
        ),
        // The security groups ONE content-audit policy may audit, under the code the
        // recorded listing binds that plane to: a count rather than a request rate.
        quota(
            "Audit security groups per security group content audit policy",
            1.0,
        )
        .code("L-3BC4947E")
        .describe(
            "The maximum number of security groups that you can audit in a security group \
             content audit policy.",
        ),
        rate("L-3DCEAE01", "DeleteAppsList", 5.0),
        // A COUNT of the resources in one resource set, which the recorded listing
        // publishes under this code: a plane rather than a request rate, so it carries
        // neither a window nor an API's call count.
        quota("Resources per resource set", 100.0)
            .code("L-3F3557EB")
            .describe(
                "The maximum number of resources that a single resource set can contain.",
            ),
        rate("L-450C679C", "DeletePolicy", 5.0),
        // The custom-managed-list plane of a content-audit security group policy, which
        // the recorded listing publishes under this code: a plane, as above.
        quota(
            "Custom managed application lists in any content audit security group policy setting",
            1.0,
        )
        .code("L-49265FF2")
        .describe(
            "The maximum number of custom managed application lists in any content audit \
             security group policy setting.",
        ),
        rate("L-4E3D82C8", "DeleteResourceSet", 5.0),
        quota("Resource sets per Firewall Manager admin account", 20.0)
            .code("L-573ADF04")
            .describe(
                "The maximum number of resource sets that you can create under a single \
                 Firewall Manager admin account.",
            ),
        rate("L-5AB0AC42", "GetAdminAccount", 20.0),
        rate("L-72AC804F", "GetAppsList", 20.0),
        quota("Protocols per protocol list", 5.0)
            .code("L-742188BD")
            .describe("The maximum number of protocols that you can define in a protocol list."),
        rate("L-743D9B7E", "GetNotificationChannel", 20.0).fixed(),
        rate("L-74591874", "GetPolicy", 20.0).fixed(),
        rate("L-7AB3A351", "GetProtectionStatus", 20.0),
        rate("L-7F63BF51", "GetProtocolsList", 20.0).fixed(),
        rate("L-941CF907", "GetResourceSet", 20.0).fixed(),
        rate("L-98694ACE", "GetViolationDetails", 20.0),
        rate("L-9E030C62", "ListAppsLists", 20.0).fixed(),
        rate("L-BCA8AD34", "ListComplianceStatus", 20.0).fixed(),
        rate("L-C6CF2DBB", "ListMemberAccounts", 20.0),
        rate("L-C80E7B9E", "ListPolicies", 20.0),
        rate("L-CA0A307F", "ListProtocolsLists", 20.0),
        rate("L-DE1D154D", "PutNotificationChannel", 5.0),
        rate("L-F4032167", "PutPolicy", 5.0),
    ]
}

/// Amazon FSx's published default-quota catalog.
///
/// SOURCE: AWS's published Service Quotas data for the `fsx` service code (the "Quotas"
/// page of the Amazon FSx user guides). FSx publishes its resource planes PER FILE
/// SYSTEM TYPE — the file systems, the storage capacity, the IOPS and the throughput
/// capacity of each of the four file-system types it serves — which is why the rows are
/// named for the type rather than for FSx as a whole. A throughput row carries the unit
/// AWS publishes it in rather than being a count. Which `QuotaCode` names which type's
/// plane, each code's adjustability and the unit each row is published in come from the
/// recorded default-quota listing of `fsx`, which is also where the wording of the one
/// row whose published description it states comes from. That listing binds the Windows
/// throughput plane, the NetApp ONTAP file-system plane and the Lustre Persistent
/// Intelligent-Tiering read cache to codes other than the ones this catalog had published
/// them under, so each row below is published under the code the listing binds ITS quota
/// to; the codes they vacate name no quota this catalog can cite, so nothing is published
/// under them.
pub(super) fn fsx_quotas() -> Vec<QuotaRow> {
    let rate = |code: &str, api: &str, per_second: f64| request_rate(code, "FSx", api, per_second);
    vec![
        quota("Windows File Server file systems", 100.0)
            .code("L-0CD18A5D")
            .describe("The maximum number of Windows File Server file systems."),
        quota("Lustre Cache_1 storage capacity", 100_800.0)
            .code("L-15D9FE87")
            .describe(
                "The maximum amount of Cache_1 storage capacity (in GiB) for all Amazon FSx \
                 for Lustre file systems that you can have in this account.",
            ),
        quota("OpenZFS provisioned SSD read cache storage capacity", 262_144.0)
            .code("L-4E6C2FB3")
            .describe(
                "The maximum amount of provisioned SSD read cache storage capacity (in GiB) \
                 that you can configure for all Amazon FSx for OpenZFS file systems in this \
                 account.",
            ),
        // NOT published here: the three codes this catalog had served the Windows
        // throughput plane, the NetApp ONTAP file-system plane and the ONTAP throughput
        // plane under. The listing binds the first two planes, and the Lustre Persistent
        // Intelligent-Tiering read cache, to the codes they are published under below,
        // and states no quota this catalog can cite under the codes they vacate, so those
        // are left to the default-quota-set gap rather than paired by guesswork.
        quota("Windows File Server backups per file system", 500.0)
            .code("L-57578687")
            .describe("The maximum number of backups per Windows File Server file system."),
        quota("Lustre file systems", 100.0)
            .code("L-59D0763F")
            .describe("The maximum number of Lustre file systems."),
        quota("Lustre total storage capacity", 1_048_576.0)
            .code("L-5B89F9CE")
            .describe("The maximum total storage capacity of Lustre file systems."),
        quota("Lustre data repository associations per file system", 8.0)
            .code("L-60836D3E")
            .describe(
                "The maximum number of data repository associations per Lustre file system.",
            ),
        quota("Lustre backups per file system", 500.0)
            .code("L-736F3D6F")
            .describe("The maximum number of backups per Lustre file system."),
        quota("ONTAP total SSD storage capacity", 524_288.0)
            .code("L-84EAF187")
            .describe("The maximum total SSD storage capacity of ONTAP file systems."),
        quota("ONTAP total SSD IOPS", 400_000.0)
            .code("L-88479C21")
            .describe("The maximum total SSD IOPS of ONTAP file systems."),
        quota("ONTAP storage virtual machines per file system", 24.0)
            .code("L-901C77F5")
            .describe(
                "The maximum number of storage virtual machines per ONTAP file system.",
            ),
        quota("ONTAP volumes per file system", 500.0)
            .code("L-9AFA1F09")
            .describe("The maximum number of volumes per ONTAP file system."),
        quota("ONTAP backups per volume", 500.0)
            .code("L-AD2FC696")
            .describe("The maximum number of backups per ONTAP volume."),
        // The provisioned SSD read cache of the Lustre Persistent Intelligent-Tiering
        // deployment, under the code the recorded listing binds that plane to, with the
        // name and value that listing carries for it.
        quota(
            "Lustre Persistent Intelligent-Tiering SSD read cache storage capacity",
            100_800.0,
        )
        .code("L-B7391FCE")
        .describe(
            "The maximum amount of provisioned SSD read cache storage capacity (in GiB) \
             that you can configure for all Amazon FSx for Lustre Intelligent-Tiering \
             file systems in this account.",
        ),
        quota("ONTAP file systems", 100.0)
            .code("L-C28C1403")
            .describe(
                "The maximum number of Amazon FSx for NetApp ONTAP file systems that you \
                 can have in this account.",
            ),
        quota("ONTAP backups", 10_000.0)
            .code("L-C431DBA3")
            .describe(
                "The maximum number of user-initiated backups for all Amazon FSx for NetApp \
                 ONTAP file systems that you can have in this account.",
            ),
        // A COUNT of file systems rather than a capacity: the recorded listing binds
        // this code to the Lustre scratch deployment's file-system plane, so the row is
        // published with that plane's name, unit and value.
        quota("Lustre Scratch file systems", 100.0)
            .code("L-C48231E5")
            .describe(
                "The maximum number of Amazon FSx for Lustre Scratch file systems that you \
                 can have in this account.",
            ),
        quota("OpenZFS volumes per file system", 500.0)
            .code("L-C5F860DD")
            .describe("The maximum number of volumes per OpenZFS file system."),
        quota("OpenZFS snapshots per volume", 1000.0)
            .code("L-C8640C82")
            .describe("The maximum number of snapshots per OpenZFS volume."),
        quota("OpenZFS backups per volume", 500.0)
            .code("L-CD5E0524")
            .describe("The maximum number of backups per OpenZFS volume."),
        quota("File caches", 10.0)
            .code("L-DD0F7417")
            .describe("The maximum number of file caches."),
        quota("File cache total storage capacity", 24_576.0)
            .code("L-E24B4DE4")
            .describe("The maximum total storage capacity of file caches."),
        quota("ONTAP SSD storage capacity", 1_048_576.0)
            .code("L-E2C89679")
            .describe(
                "The maximum amount of SSD storage capacity (in GiB) for all Amazon FSx for \
                 NetApp ONTAP file systems that you can have in this account.",
            ),
        quota("Windows SSD storage capacity", 524_288.0)
            .code("L-E43BDB2E")
            .describe(
                "The maximum amount of SSD storage capacity (in GiB) for all Amazon FSx for \
                 Windows File Server file systems that you can have in this account.",
            ),
        rate("L-E94C1C19", "DescribeFileSystems", 10.0),
        rate("L-FD6F2F22", "CreateFileSystem", 5.0),
        // The THROUGHPUT capacity plane of the Windows File Server family, under the code
        // the recorded listing binds it to: a plain measured quota, so its `Unit` is the
        // catalog's `None` rather than a throughput unit.
        quota("Windows throughput capacity", 10_240.0)
            .code("L-FD89CA8A")
            .describe(
                "The maximum amount of throughput capacity (in MBps) for all Amazon FSx for \
                 Windows File Server file systems that you can have in this account.",
            ),
    ]
}

/// Amazon GameLift's published default-quota catalog.
///
/// SOURCE: AWS's published Service Quotas data for the `gamelift` service code (the
/// "Amazon GameLift quotas" page of its developer guide): the hosting resource planes
/// beside the per-API request rates GameLift publishes for its game-session,
/// matchmaking and fleet APIs. Which `QuotaCode` names which quota, each code's
/// adjustability and the unit of the capacity row come from the recorded default-quota
/// listing of `gamelift`, which binds the code this catalog had published the script
/// plane under to the player-session timeout; the script plane names no code that
/// listing states, so it is left to the default-quota-set gap.
pub(super) fn gamelift_quotas() -> Vec<QuotaRow> {
    let rate =
        |code: &str, api: &str, per_second: f64| request_rate(code, "GameLift", api, per_second);
    vec![
        quota("Compute per Anywhere fleet", 100.0)
            .code("L-0536A98D")
            .describe(
                "The maximum number of registered compute resources allowed per Anywhere fleet.",
            ),
        // The BUILD capacity of a Region, beside the script capacity below: the recorded
        // row for this code publishes a size rather than a count, so the row reports the
        // unit that size is published in.
        quota("Build capacity", 100.0)
            .code("L-11948650")
            .unit("Gigabytes")
            .fixed()
            .describe(
                "The maximum capacity (in gigabytes) available for all uploaded builds \
                 combined per region. To free storage space, delete unused builds as \
                 needed.",
            ),
        quota("Builds", 1000.0)
            .code("L-22451070")
            .describe("The maximum number of builds in this account in the current Region."),
        // The player-session TIMEOUT: a length of time rather than a count of
        // resources, so the row reports the unit that time is published in. The recorded
        // listing binds this code to that quota.
        quota("Player session timeout", 60.0)
            .code("L-24BE0A39")
            .unit("Seconds")
            .describe(
                "The maximum length of time, in seconds, that a player session remains in \
                 RESERVED status before it times out.",
            ),
        quota("Game session queues", 25.0)
            .code("L-293B0017")
            .describe("The maximum number of game session queues."),
        quota("Matchmaking configurations", 100.0)
            .code("L-3A43EF3C")
            .describe("The maximum number of matchmaking configurations."),
        quota("Matchmaking rule sets", 100.0)
            .code("L-51AF299A")
            .describe("The maximum number of matchmaking rule sets."),
        quota("Game server groups", 100.0)
            .code("L-55650DB7")
            .describe("The maximum number of game server groups."),
        quota("Fleet locations per fleet", 100.0)
            .code("L-593688D9")
            .describe("The maximum number of locations per fleet."),
        quota("Instances per fleet", 5000.0)
            .code("L-67477D57")
            .fixed()
            .describe("The maximum number of instances per fleet."),
        quota("Instances per location", 5000.0)
            .code("L-73F6E300")
            .describe("The maximum number of instances per fleet location."),
        quota("Matchmaking rule sets per region", 1000.0)
            .code("L-8AE49BBD")
            .describe("The maximum number of matchmaking rule sets per Region."),
        quota("Concurrent game session activations per instance", 2.0)
            .code("L-8D885299")
            .describe(
                "The maximum number of game sessions that can be activated at once on an \
                 instance.",
            ),
        rate("L-8EBC4E87", "CreateGameSession", 10.0).fixed(),
        quota("Builds per region", 1000.0)
            .code("L-90D24F1B")
            .describe("The maximum number of builds per Region."),
        rate("L-9F9DE0B2", "StartGameSessionPlacement", 10.0),
        // The matchmaking player attribute whose value is a STRING-TO-DOUBLE MAP, under
        // the code the recorded listing binds that plane to: a count of the pairs one
        // attribute may carry rather than a request rate, which the service publishes
        // with the name and the adjustability that listing carries for it.
        quota(
            "Key-value pairs per string to double map matchmaking player attribute",
            10.0,
        )
        .code("L-A8C1B434")
        .fixed()
        .describe(
            "The maximum number of key-value pairs per string to double map matchmaking \
             player attribute.",
        ),
        // The ALIAS plane of a Region, under the code the recorded listing binds it to,
        // with the value and the sentence that listing carries for it.
        quota("Aliases per region", 100.0)
            .code("L-AED4A06A")
            .describe("The maximum number of aliases allowed per region."),
        // The destination plane of a game session queue, which the recorded listing
        // publishes under this code: a count of resources rather than a request rate.
        quota("Queue destinations per game session queue", 10.0)
            .code("L-BB62CF1D")
            .describe("The maximum number of destinations per game session queue."),
        rate("L-BFEEB817", "DescribeGameSessionDetails", 10.0),
        rate("L-C30AA854", "SearchGameSessions", 10.0).fixed(),
        quota("Custom locations per region", 20.0)
            .code("L-C6F4238C")
            .describe("The maximum number of custom locations per Region."),
        rate("L-CCBBB4DA", "UpdateFleetCapacity", 10.0).fixed(),
        rate("L-E8D8BD94", "DescribeFleetAttributes", 10.0).fixed(),
        // As with the queue-destination row above: a count plane the recorded listing
        // publishes under this code, which it publishes as not adjustable.
        quota("Player sessions per game session", 200.0)
            .code("L-EC03D793")
            .fixed()
            .describe("The maximum number of player sessions per game session."),
        // A capacity quota measured in GIGABYTES, which AWS publishes as fixed: the
        // unit is part of what the row publishes, so it is never reported as a count.
        quota("Script capacity", 100.0)
            .code("L-ED58E8F8")
            .unit("Gigabytes")
            .fixed()
            .describe(
                "The maximum capacity (in gigabytes) available for all uploaded scripts \
                 combined per region. To free storage space, delete unused builds as \
                 needed.",
            ),
        rate("L-FDDD1260", "CreateGameSessionQueue", 5.0),
    ]
}

/// Amazon S3 Glacier's published default-quota catalog.
///
/// SOURCE: AWS's published Service Quotas data for the `glacier` service code (the
/// "Amazon S3 Glacier quotas" page of its developer guide): the vault plane, the archive
/// and multipart SIZE and COUNT planes, and the provisioned-capacity plane, all of which
/// AWS publishes as not adjustable. Which `QuotaCode` names which quota and the unit and
/// value each size plane is published in come from the recorded default-quota listing of
/// `glacier`, whose rows are limits rather than per-API request rates.
pub(super) fn glacier_quotas() -> Vec<QuotaRow> {
    vec![
        quota("Number of random restore requests.", 35.0)
            .code("L-0D9D2530")
            .fixed()
            .describe("The number of random restore requests per PiB stored per day."),
        quota("Number of vault tags.", 50.0)
            .code("L-153F5D45")
            .fixed()
            .describe("The maximum number of tags that can be applied to a vault."),
        // The part size of a multipart upload, published in GIGABYTES: the unit and the
        // value are one pair, so the row reports AWS's documented maximum part size in
        // the unit AWS declares it in rather than the same magnitude in another.
        quota("Multipart parts size.", 4.0)
            .code("L-4BF4C362")
            .unit("Gigabytes")
            .fixed()
            .describe("The maximum size (in GB) of parts allowed in a multipart upload."),
        // The PART COUNT of a multipart upload, beside the part-size plane above: AWS's
        // documented maximum number of parts per multipart upload (the "Amazon S3
        // Glacier quotas" table), published under the code the recorded listing binds
        // this plane to, in the naming of the service's other count planes.
        quota("Number of multipart parts.", 10_000.0)
            .code("L-982DEA76")
            .fixed()
            .describe("The maximum number of parts allowed in a multipart upload."),
        // The vault plane of an account, which AWS publishes as not adjustable.
        quota("Vaults per account", 1000.0)
            .code("L-D1C67346")
            .fixed()
            .describe("The maximum number of vaults an account can have."),
        // The MINIMUM an archive (or a part of one) may be: the only row of this catalog
        // the recorded listing publishes as a floor rather than a ceiling, so its
        // sentence says so.
        quota("Archive size.", 4.0)
            .code("L-E0D19AD1")
            .unit("Megabytes")
            .fixed()
            .describe("The minimum size (in MB) of an archive (or part)."),
        // The archive SIZE plane published in gigabytes, under the code the recorded
        // listing binds it to: a size rather than a request rate, so it carries neither
        // a window nor an API's call count.
        quota("Archive size in GB.", 40_000.0)
            .code("L-E96017CA")
            .unit("Gigabytes")
            .fixed()
            .describe("The maximum size of an archive."),
        // The provisioned-capacity plane, which the recorded listing publishes under
        // this code.
        quota("Provisioned capacity units", 2.0)
            .code("L-F127DF29")
            .fixed()
            .describe(
                "The maximum number of provisioned capacity units available to purchase \
                 per account.",
            ),
    ]
}

/// AWS Glue's published default-quota catalog.
///
/// SOURCE: AWS's published Service Quotas data for the `glue` service code (the "AWS
/// Glue endpoints and quotas" page of the AWS General Reference): the Data Catalog and
/// job planes beside the per-API request rates Glue publishes for its catalog and job
/// APIs. Which `QuotaCode` names which quota, each code's adjustability, and the unit
/// and description of the rows it states come from the recorded default-quota listing of
/// `glue`, whose Data Catalog rows carry their description in the form
/// `Catalog: <the quota's own name>`.
///
/// That listing binds two of the codes this catalog had published the job-concurrency
/// planes under to the crawler plane of an account and the function plane of one
/// database; the job-concurrency planes name no code it states, so they are left to the
/// default-quota-set gap rather than served under another quota's code, and so is the
/// code this catalog had served the account-wide crawler plane under.
pub(super) fn glue_quotas() -> Vec<QuotaRow> {
    let rate = |code: &str, api: &str, per_second: f64| request_rate(code, "Glue", api, per_second);
    vec![
        quota("Databases per account", 10_000.0)
            .code("L-04CEE988")
            .describe("The maximum number of databases in your account."),
        quota("Tables per database", 200_000.0)
            .code("L-071ABE08")
            .describe("The maximum number of tables per database."),
        quota("Partitions per table", 10_000_000.0)
            .code("L-08F3B322")
            .describe("The maximum number of partitions per table."),
        quota("Table versions per table", 100_000.0)
            .code("L-096DBB95")
            .describe("The maximum number of table versions per table."),
        // The CRAWLER plane of an account and the FUNCTION plane of one database, under
        // the codes the recorded listing binds them to and with the names that listing
        // carries for them.
        quota("Number of crawlers per account", 1000.0)
            .code("L-11FA2C1A")
            .describe("The maximum number of crawlers per account."),
        quota("Max functions per database", 100.0)
            .code("L-1DD415D5")
            .describe("The maximum number of functions per database."),
        quota("Development endpoints per account", 5.0)
            .code("L-2C3F5401")
            .describe("The maximum number of development endpoints in your account."),
        quota("Triggers per account", 25_000.0)
            .code("L-2E9FB93F")
            .describe("The maximum number of triggers in your account."),
        // NOT published here: the code this catalog had served the account-wide crawler
        // plane under. That plane is published under L-11FA2C1A above, the code the
        // listing binds it to, and the listing states no quota this catalog can cite under
        // this one, so it is left to the default-quota-set gap.
        quota("Connections per account", 1000.0)
            .code("L-3890A802")
            .describe("The maximum number of connections in your account."),
        quota("Security configurations per account", 200.0)
            .code("L-4071B0E3")
            .describe("The maximum number of security configurations in your account."),
        quota("Workflows per account", 250.0)
            .code("L-4256D6D2")
            .describe("The maximum number of workflows in your account."),
        // The Data Catalog's global S3 path length limit, published under this code with
        // the catalog's own wording as its description and the name AWS spells it with.
        quota("Global s3 path max length", 2000.0)
            .code("L-4F4B522D")
            .describe("Catalog: global s3 path max length"),
        quota("Machine learning transforms per account", 100.0)
            .code("L-5E4153CA")
            .describe("The maximum number of machine learning transforms in your account."),
        quota("Max jobs per account", 2000.0)
            .code("L-611FDDE4")
            .describe("The maximum number of jobs in your account."),
        quota("Schema registries per account", 100.0)
            .code("L-64A88F0A")
            .fixed()
            .describe("The maximum number of schema registries in your account."),
        quota("Schemas per account", 1000.0)
            .code("L-76EC689B")
            .describe("The maximum number of schemas in your account."),
        quota("Schema versions per account", 10_000.0)
            .code("L-785C1F39")
            .describe("The maximum number of schema versions in your account."),
        quota("Interactive sessions per account", 100.0)
            .code("L-7DD7C33A")
            .describe("The maximum number of interactive sessions in your account."),
        quota("Data quality rulesets per account", 1000.0)
            .code("L-83192DBF")
            .describe("The maximum number of data quality rulesets in your account."),
        rate("L-83A59AA6", "BatchCreatePartition", 50.0),
        rate("L-87F5991C", "BatchDeletePartition", 50.0),
        rate("L-94D025B7", "BatchGetPartition", 50.0),
        // The in-flight completion requests of an account, whose published sentence the
        // service carries WITHOUT a closing period.
        quota("Maximum number of in-flight completion requests allowed per account", 10.0)
            .code("L-958728D1")
            .describe("The maximum number of in-flight completion requests allowed per account"),
        rate("L-A457CBAB", "CreateTable", 10.0),
        // The DATABASE plane of one Data Catalog, under the code the recorded listing
        // binds it to, with the sentence that listing carries for it.
        quota("Max databases per catalog", 10_000.0)
            .code("L-A57B5BCE")
            .describe("The maximum number of databases per catalog."),
        rate("L-ACAC12B6", "CreateCrawler", 10.0),
        rate("L-AD871090", "DeleteDatabase", 10.0).fixed(),
        quota(
            "The maximum number of concurrent materialized view refresh task runs.",
            50.0,
        )
        .code("L-B4840D4D")
        .describe("The maximum number of concurrent materialized view refresh task runs."),
        // A SIZE plane of the machine-learning transforms, published in megabytes.
        quota("Label file size", 10.0)
            .code("L-B78896B9")
            .unit("Megabytes")
            .describe("The maximum size of a label file."),
        rate("L-B8497671", "GetDatabases", 50.0),
        rate("L-C3952F6C", "GetJob", 50.0),
        // The two rate rows the recorded listing publishes as NOT adjustable.
        rate("L-CB69DFA0", "GetJobRun", 100.0).fixed(),
        // The Data Catalog's administrator plane, published under this code with the
        // catalog's own wording as its description.
        quota("Global maximum administrators", 100.0)
            .code("L-D90AC94B")
            .describe("Catalog: global maximum administrators"),
        rate("L-D987EC31", "GetPartition", 100.0),
        rate("L-DBA56E2F", "GetPartitions", 100.0),
        rate("L-E15CE20A", "GetTable", 100.0),
        // The Ray compute plane of the account, measured in M-DPUs, under the code the
        // recorded listing binds it to and with the name that listing carries — which
        // the service publishes with a closing period — and which it publishes as not
        // adjustable.
        quota(
            "Concurrent compute capacity for Ray worker instances in M-DPUs per account.",
            50.0,
        )
        .code("L-E6C807E7")
        .fixed()
        .describe(
            "The maximum concurrent compute capacity for Ray worker instances in M-DPUs \
             per account.",
        ),
        rate("L-EAC30D24", "GetTableVersions", 50.0),
        rate("L-EEC98450", "GetUserDefinedFunction", 50.0),
        rate("L-F1653A6D", "StartCrawler", 10.0),
        rate("L-F1AAB534", "StartJobRun", 50.0),
        rate("L-F545C262", "StartWorkflowRun", 10.0),
        rate("L-F574AED9", "UpdateDatabase", 10.0),
        rate("L-F953935E", "UpdateTable", 10.0),
        rate("L-FB210BD9", "UpdatePartition", 50.0),
        rate("L-FEBBFA7A", "UpdateJob", 10.0),
    ]
}

/// AWS Ground Station's published default-quota catalog.
///
/// SOURCE: AWS's published Service Quotas data for the `groundstation` service code
/// (the "Ground Station quotas" page of its user guide): the mission-profile, config
/// and dataflow planes a contact is scheduled out of, plus the contact planes
/// themselves — the scheduling limits (how far ahead a contact may be booked, how many
/// contacts and how many minutes may be scheduled, how long one contact may run) beside
/// the per-config-type counters. Which `QuotaCode` names which quota, and the value each
/// is published with, come from the recorded default-quota listing of `groundstation`,
/// which binds the code this catalog had published the antenna-uplink-config plane under
/// to the dataflow endpoints of one endpoint group; that config plane names no code it
/// states, so it is left to the default-quota-set gap.
pub(super) fn groundstation_quotas() -> Vec<QuotaRow> {
    vec![
        // A scheduling WINDOW, published in days rather than as a count of resources, in
        // the wording this service publishes a contact window with.
        quota("Contact Lead Time Maximum", 7.0)
            .code("L-09DEC198")
            .describe("Maximum lead time allowed for scheduling a contact in days"),
        quota("Mission profile limit", 100.0)
            .code("L-5342B9BF")
            .describe("The maximum number of mission profiles allowed."),
        quota("Config limit", 100.0)
            .code("L-5CCF0BC2")
            .describe("The maximum number of configs allowed."),
        // The DATAFLOW ENDPOINT plane of one endpoint group, under the code the recorded
        // listing binds it to, with the value and the sentence that listing carries.
        quota("Dataflow endpoints per group limit", 20.0)
            .code("L-98A63A85")
            .describe("The maximum number of dataflow endpoints per group allowed."),
        quota("Enabled Ephemerides limit", 30.0)
            .code("L-BD84767C")
            .describe(
                "Maximum number of enabled customer-provided ephemerides per satellite.",
            ),
        // A contact DURATION, published in minutes rather than as a count of resources.
        quota("Maximum Contact Duration", 20.0)
            .code("L-CCFDE387")
            .describe("The maximum contact duration permitted in minutes"),
        // The DATAFLOW ENDPOINT GROUP plane, under the code the recorded listing binds
        // it to.
        quota("Dataflow endpoint group limit", 100.0)
            .code("L-D6A1915B")
            .describe("The maximum number of dataflow endpoint groups allowed."),
        quota("Ephemeris Validation limit", 10.0)
            .code("L-DE376FC5")
            .describe(
                "Maximum number of customer-provided ephemerides that be validated \
                 simultaneously.",
            ),
        quota("Scheduled Contacts Limit", 100.0)
            .code("L-DF7B6DEC")
            .describe("Maximum number of scheduled contacts allowed"),
        quota("Scheduled Minutes Limit", 1000.0)
            .code("L-FED20749")
            .describe("The maximum number of scheduled minutes allowed"),
    ]
}

/// Amazon GuardDuty's published default-quota catalog.
///
/// SOURCE: AWS's published Service Quotas data for the `guardduty` service code (the
/// "Quotas" page of its user guide): the detector plane of an account, the member-account
/// planes of an administrator account, and the filter, list and retention planes, which
/// AWS publishes as not adjustable. Which `QuotaCode` names which quota comes from the
/// recorded default-quota listing of `guardduty`, which binds the member-account planes
/// to separate codes for accounts added through AWS Organizations and by invitation, and
/// the trusted-IP-set and threat-intel-set planes to separate codes of their own.
pub(super) fn guardduty_quotas() -> Vec<QuotaRow> {
    vec![
        quota("Threat intel sets", 6.0)
            .code("L-2C0E14B9")
            .fixed()
            .describe(
                "The maximum number of Threat intel sets that you can add per AWS account \
                 per region.",
            ),
        // A RETENTION window, published in days rather than as a count of resources.
        quota("Finding retention period", 90.0)
            .code("L-373487E5")
            .fixed()
            .describe(
                "The maximum number of days a finding is retained. After 90 days findings are \
                 deleted.",
            ),
        quota("Member accounts through AWS Organizations", 50_000.0)
            .code("L-8B0D7135")
            .fixed()
            .describe(
                "Your current AWS Organizations member account quota shows the default \
                 maximum number of member accounts that you can associate with an \
                 administrator through AWS Organizations, including members added by \
                 invitation. The number of GuardDuty member accounts added through AWS \
                 Organizations can't exceed the total member accounts in your organization.",
            ),
        quota("Filters", 100.0)
            .code("L-9ABF7A23")
            .fixed()
            .describe("The maximum number of saved filters per AWS account per region."),
        quota("Trusted IP sets", 1.0)
            .code("L-AFBA2260")
            .fixed()
            .describe(
                "The maximum number of Trusted IP sets that you can add per AWS account \
                 per region.",
            ),
        quota("Member accounts by invitation", 5000.0)
            .code("L-F51A2830")
            .fixed()
            .describe(
                "The maximum number of member accounts that can be associated with a \
                 GuardDuty administrator account by invitation.",
            ),
        quota("Detectors", 1.0)
            .code("L-FA6D6E3D")
            .fixed()
            .describe(
                "The maximum number of detector resources that you can create per AWS \
                 account per region.",
            ),
    ]
}

/// EC2 Image Builder's published default-quota catalog.
///
/// SOURCE: AWS's published Service Quotas data for the `imagebuilder` service code (the
/// "Service quotas" page of the EC2 Image Builder user guide): the pipeline, recipe,
/// component and configuration planes, the component SIZE and workflow parameter planes,
/// and the per-API request rates it publishes for its image APIs. Which `QuotaCode`
/// names which quota and the unit of the size plane come from the recorded default-quota
/// listing of `imagebuilder`, which binds the code this catalog had published the
/// `ListImageRecipes` rate under to the infrastructure-configuration plane; that call
/// rate names no code it states, so it is left to the default-quota-set gap, and so is
/// the code this catalog had served that plane under.
pub(super) fn imagebuilder_quotas() -> Vec<QuotaRow> {
    let rate = |code: &str, api: &str, per_second: f64| {
        request_rate(code, "EC2 Image Builder", api, per_second)
    };
    vec![
        quota("Image pipelines", 1000.0)
            .code("L-0DF2752F")
            .describe(
                "The maximum number of image pipelines that you can create in an account in the \
                 current Region.",
            ),
        // NOT published here: the code this catalog had served the
        // infrastructure-configuration plane under. That plane is published under
        // L-D9EF98D9 below, the code the listing binds it to, and the listing states no
        // quota this catalog can cite under this one, so it is left to the
        // default-quota-set gap.
        quota("Image recipes", 1000.0)
            .code("L-1DF98342")
            .describe(
                "The maximum number of image recipes that you can create in an account in the \
                 current Region.",
            ),
        // The SIZE of an image workflow, which the service publishes as the size of the
        // workflow's DATA FIELD, in kilobytes rather than as a count of resources.
        quota("Image workflow size", 64.0)
            .code("L-22EAF8F8")
            .unit("Kilobytes")
            .describe("The maximum size of the data field of an image workflow."),
        quota("Container recipes", 1000.0)
            .code("L-28A502FD")
            .describe(
                "The maximum number of container recipes that you can create in an account in \
                 the current Region.",
            ),
        quota("Distribution configurations", 1000.0)
            .code("L-2BAA05D8")
            .describe(
                "The maximum number of distribution configurations that you can create in an \
                 account in the current Region.",
            ),
        quota("Images", 10_000.0)
            .code("L-31E5726E")
            .describe(
                "The maximum number of images that you can create in an account in the current \
                 Region.",
            ),
        quota("Concurrent image builds", 10.0)
            .code("L-59EC76F6")
            .describe("The maximum number of image builds that can run at once."),
        quota("Lifecycle policies", 100.0)
            .code("L-5E848392")
            .describe(
                "The maximum number of lifecycle policies that you can create in an account in \
                 the current Region.",
            ),
        quota("Workflows", 1000.0)
            .code("L-6105A4EE")
            .describe(
                "The maximum number of workflows that you can create in an account in the \
                 current Region.",
            ),
        quota("Component versions", 1000.0)
            .code("L-69312229")
            .describe("The maximum number of versions of one component."),
        rate("L-7A0E01E3", "CreateImage", 5.0),
        rate("L-84F5AB81", "CreateImagePipeline", 5.0),
        rate("L-9B183655", "CreateImageRecipe", 5.0),
        rate("L-A26B27A7", "CreateComponent", 5.0),
        // The parameter plane of a component, which the recorded listing publishes under
        // this code: a count rather than a request rate.
        quota("Parameters per component", 25.0)
            .code("L-A58CFBED")
            .describe("The maximum number of parameters that you can define in a component."),
        // The SSM parameters one distribution configuration may update in a Region,
        // under the code the recorded listing binds that plane to: a count rather than a
        // request rate.
        quota(
            "SSM Parameters updated per distribution configuration Region",
            10.0,
        )
        .code("L-B588F8CA")
        .describe(
            "The maximum number of SSM parameters that one distribution configuration can \
             update in a Region.",
        ),
        // As above: a concurrency plane the recorded listing publishes under this code.
        quota("Concurrent builds", 100.0)
            .code("L-BA4D191B")
            .describe(
                "The maximum number of concurrent builds that can be in progress in this \
                 account in the current Region.",
            ),
        quota(
            "Launch templates modified per distribution configuration Region",
            5.0,
        )
        .code("L-D5665818")
        .describe(
            "The maximum number of launch templates that one distribution configuration can \
             update in a Region.",
        ),
        // The payload size of a component, which the service publishes as the size of
        // the component's DATA FIELD, in kilobytes rather than as a count.
        quota("Component size", 64.0)
            .code("L-D5DC1FB3")
            .unit("Kilobytes")
            .describe("The maximum size of the data field of a component."),
        rate("L-D9E1BB3C", "ListComponents", 10.0),
        // The INFRASTRUCTURE CONFIGURATION plane of an account, under the code the
        // recorded listing binds it to, with the value and the sentence that listing
        // carries for it.
        quota("Infrastructure configurations", 1000.0)
            .code("L-D9EF98D9")
            .describe(
                "The maximum number of infrastructure configurations that you can create in \
                 an account in the current Region.",
            ),
        quota("Parameters per image workflow", 25.0)
            .code("L-E49825CC")
            .describe(
                "The maximum number of parameters that you can define in an image workflow.",
            ),
    ]
}

/// AWS Lambda's published default-quota catalog.
///
/// SOURCE: AWS's published Service Quotas data for the `lambda` service code — the
/// quota names, default values, units and adjustability of the "Lambda quotas" page —
/// and the recorded applied-quota listing of that code for WHICH `QuotaCode` names
/// which quota and for the `Description` each carries. A `QuotaCode` IDENTIFIES one
/// quota, so a row claims AWS's code only where the recorded response for that pair
/// says which quota the code names; every other row takes the descriptive code
/// ([`super::descriptive_quota_code`]) rather than a pairing this catalog would be
/// guessing at.
///
/// The rows are in the order the service reports them, which is the order a recorded
/// listing of this service carries: the quotas AWS's own codes name — its API rates,
/// its MicroVM quotas and its per-execution-environment throughput — first, then the
/// function-shape quotas whose codes this catalog derives.
pub(super) fn lambda_quotas() -> Vec<QuotaRow> {
    vec![
        // Most of the API request-rate quotas AWS publishes as not adjustable, each
        // with its own `Period`: so many requests per second.
        quota(
            "Rate of control plane API requests (excludes invocation, GetFunction, \
             and GetPolicy requests)",
            15.0,
        )
        .code("L-B2AA0F47")
        .fixed()
        .period(1, "SECOND")
        .describe(
            "The maximum number of API requests per second (excluding invocation, \
             GetFunction, and GetPolicy requests).",
        ),
        // Lambda builds a MicroVM image before it can run one, and publishes
        // how many of those builds may be in flight: a `Count`, adjustable,
        // over the one-second window this row carries.
        quota("Number of concurrent MicroVM image builds", 10.0)
            .code("L-72E0D058")
            .unit("Count")
            .describe("Number of concurrent MicroVM image builds per account per region"),
        // The capacity-provider control plane is rate limited over ALL of its
        // write APIs together, which is what this quota's own description
        // says, so the limit is one combined requests-per-second figure.
        quota("Rate of capacity provider write API requests", 1.0)
            .code("L-F55076EC")
            .fixed()
            .period(1, "SECOND")
            .describe(
                "The maximum combined rate (requests per second) for all capacity \
                 provider write APIs.",
            ),
        // A durable execution reports a completed callback through this API;
        // AWS publishes its rate as adjustable and counted in plain requests.
        quota("Rate of SendDurableExecutionCallbackSuccess API requests", 6000.0)
            .code("L-B82A30EA")
            .describe(&api_rate_description("SendDurableExecutionCallbackSuccess")),
        // The burst allowance of the MicroVM resume API, which AWS publishes beside
        // that API's sustained rate as a quota of its own.
        quota("Burst rate of ResumeMicrovm API requests", 5.0)
            .code("L-25EEC0A4")
            .describe(&api_burst_rate_description("ResumeMicrovm")),
        quota("Rate of GetPolicy API requests", 15.0)
            .code("L-4273958C")
            .fixed()
            .period(1, "SECOND")
            .describe(&api_rate_description("GetPolicy")),
        // How long one MicroVM may run: published in hours, and not adjustable.
        quota("Max Execution Duration of a MicroVM (in Hours)", 8.0)
            .code("L-B430C318")
            .fixed()
            .describe("The maximum execution duration of a MicroVM (in Hours)."),
        // How much network bandwidth ONE execution environment may sustain, which AWS
        // publishes in megabits per second and as adjustable: a throughput quota rather
        // than a count, so it carries that unit and no rate window.
        quota("Network bandwidth per execution environment", 625.0)
            .code("L-14320ECE")
            .unit("Megabits/Second")
            .describe(
                "The maximum sustained network bandwidth, in megabits per second, for a \
                 Lambda function's execution environment that is not attached to a VPC.",
            ),
        // The rate of the API a client reads a durable execution's history through,
        // which AWS publishes as adjustable and counted in plain requests.
        quota("Rate of GetDurableExecutionHistory API requests", 15.0)
            .code("L-BA29C22B")
            .describe(&api_rate_description("GetDurableExecutionHistory")),
        // The published function-shape quotas. AWS's `QuotaCode`s for them are
        // NOT in this catalog: the codes it used to carry on these rows are
        // AWS's codes for other quotas of this service, so each row takes its
        // derived code instead of asserting a pairing nothing here can source.
        quota("Function layers", 5.0)
            .fixed()
            .describe("The maximum number of layers that a function can use."),
        quota("Environment variable size", 4.0)
            .unit("Kilobytes")
            .fixed()
            .describe("The maximum total size of the environment variables of a function."),
        quota("Function resource-based policy", 20.0)
            .unit("Kilobytes")
            .fixed()
            .describe("The maximum size of the resource-based policy of a function."),
        quota("Invocation payload (synchronous)", 6.0)
            .unit("Megabytes")
            .fixed()
            .describe(
                "The maximum size of the request and response payload of a \
                 synchronous invocation.",
            ),
        quota("Invocation payload (asynchronous)", 256.0)
            .unit("Kilobytes")
            .fixed()
            .describe("The maximum size of the event payload of an asynchronous invocation."),
        quota("Function timeout", 900.0)
            .unit("Seconds")
            .fixed()
            .describe(
                "The maximum amount of time that a function can run before Lambda \
                 stops it.",
            ),
        quota("Rate of GetFunction API requests", 100.0)
            .fixed()
            .period(1, "SECOND")
            .describe(&api_rate_description("GetFunction")),
        quota("Function and layer storage", 75.0)
            .code("L-2ACBD22F")
            .unit("Gigabytes")
            .describe(
                "The maximum total storage for the .zip deployment packages and layers \
                 of this account in this Region.",
            ),
        quota("Concurrent executions", 1000.0)
            .code("L-B99A9384")
            .describe(
                "The maximum number of function invocations that can run concurrently \
                 in this Region.",
            )
            .metric(usage_metric(
                "Lambda",
                &[
                    ("Class", "None"),
                    ("Resource", "ConcurrentExecutions"),
                    ("Type", "Resource"),
                ],
            )),
        quota("Elastic network interfaces per VPC", 250.0).describe(
            "The maximum number of elastic network interfaces that the functions of \
             this account can attach to one VPC.",
        ),
        // The rest of the quotas the AWS Lambda quotas page publishes, with
        // the names, values and units it publishes for them. None of them carries
        // AWS's own `QuotaCode` here, so each takes its descriptive one.
        //
        // Their `Description`s are this catalog's own wording — the quotas
        // page publishes the limit, not a Service Quotas description string —
        // so a later fidelity pass that gets AWS's text should replace them.
        quota("Function memory allocation", 10240.0)
            .unit("Megabytes")
            .fixed()
            .describe("The maximum memory that can be allocated to a function."),
        quota("Deployment package size (direct upload)", 50.0)
            .unit("Megabytes")
            .fixed()
            .describe(
                "The maximum size of a .zip deployment package uploaded directly to \
                 Lambda.",
            ),
        quota("Deployment package size (unzipped, including layers)", 250.0)
            .unit("Megabytes")
            .fixed()
            .describe(
                "The maximum unzipped size of a function's deployment package and its \
                 layers.",
            ),
        quota("Container image code package size", 10.0)
            .unit("Gigabytes")
            .fixed()
            .describe("The maximum size of a container image deployment package."),
        quota("/tmp directory storage", 512.0)
            .unit("Megabytes")
            .fixed()
            .describe(
                "The ephemeral storage a function's /tmp directory holds unless the \
                 function configures more.",
            ),
        quota("File descriptors", 1024.0).fixed().describe(
            "The maximum number of file descriptors one execution environment can \
             hold open.",
        ),
        quota("Execution processes/threads", 1024.0).fixed().describe(
            "The maximum number of processes and threads one execution environment \
             can run.",
        ),
        // The AWS Lambda quotas page publishes this one as a rate over a TEN
        // SECOND window ("1,000 execution environment instances every 10
        // seconds"), which is the `Period` it carries.
        quota("Concurrency scaling rate", 1000.0)
            .fixed()
            .period(10, "SECOND")
            .describe("The rate at which a function can add execution environment instances."),
        quota("Maximum event age", 21600.0)
            .unit("Seconds")
            .fixed()
            .describe(
                "The longest an asynchronous invocation request stays in the event \
                 queue before Lambda discards it.",
            ),
        quota("Retry attempts for asynchronous invocation", 2.0)
            .fixed()
            .describe(
                "The maximum number of times Lambda retries an asynchronous invocation \
                 whose function returned an error.",
            ),
        quota("Test events (console editor)", 10.0)
            .fixed()
            .describe("The maximum number of saved test events per function."),
    ]
}

/// Amazon CloudWatch's published quota catalog, under the `ServiceCode` its API
/// endpoint uses.
///
/// CloudWatch publishes most of its quotas as per-API REQUEST RATES, each over a
/// one-second window and measured with the `AWS/Usage` `CallCount` metric of the API
/// it throttles ([`cloudwatch_request_rate`]), beside the rates it puts on the
/// metric-math functions a `GetMetricData` request may use.
///
/// The rows below are the ones whose AWS code this catalog can cite TOGETHER WITH the
/// quota that code names. A row is an IDENTITY — the code a client resolves a limit by —
/// so the rest of the service's published rows are left to the default-quota-set gap
/// rather than served under a code AWS does not mint or a code whose quota this catalog
/// would be guessing at (see [`super::descriptive_quota_code`] for the rule about a
/// row's code).
pub(super) fn monitoring_quotas() -> Vec<QuotaRow> {
    vec![
        // This API states its own rate: "There is a limit of 20 transactions per second
        // for this API" (the `GetMetricWidgetImage` documentation of the CloudWatch
        // model), which is the limit AWS publishes the quota at.
        cloudwatch_request_rate("GetMetricWidgetImage", 20.0).code("L-6FCAAA2E"),
        // Two operations whose own documentation states no rate of their own: the rate
        // the recorded listing carries for each of them
        // ([`CLOUDWATCH_DEFAULT_REQUEST_RATE`]).
        cloudwatch_request_rate("ListDashboards", CLOUDWATCH_DEFAULT_REQUEST_RATE)
            .code("L-69C44FFD"),
        cloudwatch_request_rate(
            "DescribeAnomalyDetectors",
            CLOUDWATCH_DEFAULT_REQUEST_RATE,
        )
        .code("L-6300B446"),
        // A rate CloudWatch puts on ONE METRIC-MATH FUNCTION rather than on an API:
        // the quota counts how often the anomaly-detection band may be computed across
        // all `GetMetricData` requests in a second, and AWS publishes it as not
        // adjustable. It carries no `UsageMetric`, because `AWS/Usage` publishes a
        // `CallCount` series per API operation and no series counts one function's use
        // inside a call of `GetMetricData`.
        quota("Rate of ANOMALY_DETECTION_BAND usage in GetMetricData", 1000.0)
            .code("L-722EAE58")
            .fixed()
            .period(1, "SECOND")
            .describe(
                "The maximum number of times the ANOMALY_DETECTION_BAND function can be \
                 used in all GetMetricData requests, per second, in this account in the \
                 current region.",
            ),
    ]
}

/// Amazon OpenSearch Service's published quota catalog, under the `ServiceCode` its
/// API endpoint uses.
///
/// SOURCE: AWS's published Service Quotas data for the `es` service code (the "Amazon
/// OpenSearch Service quotas" page of its Developer Guide), and the recorded
/// default-quota listing of that code for which `QuotaCode` names which quota. The
/// service publishes the domain plane of the Region beside the planes counted inside
/// one domain.
pub(super) fn es_quotas() -> Vec<QuotaRow> {
    vec![
        quota("Domains per Region", 100.0)
            .code("L-076D529E")
            .describe(
                "The maximum number of OpenSearch Service domains in this account in the \
                 current Region.",
            ),
        // The warm-tier instance plane of a domain, which AWS publishes as fixed.
        quota("Warm instances per domain", 150.0)
            .code("L-1F053E6F")
            .fixed()
            .describe("The maximum number of UltraWarm instances per domain."),
        // The quota `QuotaContextInfo`'s own documentation uses as its worked
        // example of a RESOURCE-level quota: the instance count is applied per
        // OpenSearch Service domain, and can also be set account-wide, so AWS
        // publishes it as adjustable at either level. The scope type names the
        // resource type the per-resource plane applies to, which for this service is
        // its domain resource.
        quota("Instances per domain", 80.0)
            .code("L-6408ABDE")
            .adjustable_at(LEVEL_ALL, "AWS::Elasticsearch::Domain")
            .describe("The maximum number of instances per domain."),
        quota("Cross-cluster search connections per domain", 20.0)
            .code("L-AE676A72")
            .describe("The maximum number of cross-cluster search connections per domain."),
        quota("Packages per domain", 25.0)
            .code("L-E9BC8C95")
            .describe("The maximum number of packages that can be associated with one domain."),
    ]
}

/// AWS Resource Access Manager's published default-quota catalog.
///
/// SOURCE: AWS's published Service Quotas data for the `ram` service code, and the
/// recorded default-quota listing of that code for WHICH `QuotaCode` names which
/// quota: this service publishes its planes in the "Number of …" form the rows below
/// carry, each under a code in AWS's own namespace. The rest of the service's
/// published catalog names no quota this catalog can cite under the remaining codes
/// that listing reports, and a code identifies ONE quota, so those rows are left to
/// the default-quota-set gap rather than published under a binding this catalog would
/// be guessing at (see [`super::descriptive_quota_code`]).
pub(super) fn ram_quotas() -> Vec<QuotaRow> {
    vec![
        quota("Number of custom permissions per resource type", 10.0)
            .code("L-2870BE9D")
            .describe("The maximum number of custom permissions per resource type."),
        quota("Number of resource associations", 25_000.0)
            .code("L-4A6CEE66")
            .describe("The maximum number of resource associations."),
        // This row's `Description` is the service's own published sentence, as the
        // recorded listing carries it; the three beside it publish this catalog's own
        // wording in the form AWS's quota descriptions take ("The maximum number of
        // <what the quota counts>."), because the service's text for them is not
        // among the data available here.
        quota("Number of resource shares", 25_000.0)
            .code("L-595828F9")
            .describe("The maximum number of resource shares."),
        quota("Number of principal associations", 25_000.0)
            .code("L-8491BF81")
            .describe("The maximum number of principal associations."),
    ]
}

/// The `AWS/Usage` display name Amazon RDS publishes its usage counts under, which is
/// the `Service` dimension of every metered RDS quota.
const RDS_USAGE_SERVICE: &str = "RDS";

/// Amazon RDS's published default-quota catalog.
///
/// SOURCE: AWS's published Service Quotas data for the `rds` service code (the
/// "Quotas and constraints for Amazon RDS" page of the Amazon RDS User Guide), and
/// the recorded default-quota listing of that code for which `QuotaCode` names which
/// quota, for the ONE-HOUR window RDS publishes its resource planes over and for the
/// `AWS/Usage` `ResourceCount` metric it meters them with. The rest of the service's
/// published catalog names no quota this catalog can cite under the remaining codes
/// that listing reports, so those rows are left to the default-quota-set gap rather
/// than published under a guessed binding (see [`super::descriptive_quota_code`]).
pub(super) fn rds_quotas() -> Vec<QuotaRow> {
    vec![
        // A per-cluster plane RDS meters: the `Service` dimension is the one the
        // recorded row carries, and the `Resource` dimension is this catalog's naming
        // of the series the quota counts (its resource's published name as one
        // identifier), since no source available here states the value RDS publishes
        // it under.
        quota("Custom endpoints per DB cluster", 5.0)
            .code("L-9372BAB3")
            .period(1, "HOUR")
            .metric(usage_metric(
                RDS_USAGE_SERVICE,
                &[
                    ("Class", "None"),
                    ("Resource", "CustomEndpoints"),
                    ("Type", "Resource"),
                ],
            ))
            .describe("The maximum number of custom endpoints per DB cluster."),
        quota("Parameter groups", 50.0)
            .code("L-DE55804A")
            .period(1, "HOUR")
            .metric(usage_metric(
                RDS_USAGE_SERVICE,
                &[
                    ("Class", "None"),
                    ("Resource", "ParameterGroups"),
                    ("Type", "Resource"),
                ],
            ))
            .describe("The maximum number of parameter groups."),
        // The RDS Data API's concurrency plane, which AWS publishes as not adjustable.
        quota("Data API maximum concurrent cluster-secret pairs", 30.0)
            .code("L-E3721453")
            .fixed()
            .describe(
                "The maximum number of concurrent cluster-secret pairs the Data API \
                 serves.",
            ),
    ]
}

/// Amazon Redshift's published default-quota catalog.
///
/// SOURCE: AWS's published Service Quotas data for the `redshift` service code (the
/// "Quotas and limits in Amazon Redshift" page of the Amazon Redshift Management
/// Guide), and the recorded default-quota listing of that code for which `QuotaCode`
/// names which quota. Redshift publishes planes of the CLUSTER and of the database
/// inside it beside the account's own, which is why the rows are named for the thing
/// the quota counts rather than for a resource type. The rest of the service's
/// published catalog names no quota this catalog can cite under the remaining codes
/// that listing reports, so those rows are left to the default-quota-set gap rather
/// than published under a guessed binding (see [`super::descriptive_quota_code`]).
pub(super) fn redshift_quotas() -> Vec<QuotaRow> {
    vec![
        // The schema plane of a database, which AWS publishes as not adjustable.
        quota("Schemas in each database per cluster", 9900.0)
            .code("L-4E1FE923")
            .fixed()
            .describe("The maximum number of schemas in each database per cluster."),
        quota(
            "Column limit for external tables with pseudocolumns when using AWS Glue Data \
             Catalog",
            1597.0,
        )
        .code("L-8AFA7F04")
        .describe(
            "The maximum number of columns for an external table with pseudocolumns when \
             using the AWS Glue Data Catalog.",
        ),
        quota("Tables for 4xlarge cluster node type", 200_000.0)
            .code("L-99F53FC8")
            .describe("The maximum number of tables for a 4xlarge cluster node type."),
        quota("Nodes in a cluster", 128.0)
            .code("L-BB966966")
            .describe("The maximum number of nodes in a cluster."),
    ]
}

/// Amazon Redshift Serverless's published default-quota catalog.
///
/// SOURCE: AWS's published Service Quotas data for the `redshift-serverless` service
/// code (the "Quotas for Amazon Redshift Serverless" page of the Amazon Redshift
/// Management Guide), and the recorded default-quota listing of that code for which
/// `QuotaCode` names which quota, for the `Count` unit its resource planes are
/// published in and for the `AWS/Usage` `ResourceCount` metric the namespace plane is
/// metered with.
pub(super) fn redshift_serverless_quotas() -> Vec<QuotaRow> {
    vec![
        quota("Redshift Processing Units (RPUs)", 3200.0)
            .code("L-1854027F")
            .unit("Count")
            .metric(usage_metric(
                "RedshiftServerless",
                &[
                    ("Class", "None"),
                    ("Resource", "RedshiftProcessingUnits"),
                    ("Type", "Resource"),
                ],
            ))
            .describe("The maximum number of Redshift Processing Units (RPUs)."),
        quota("Serverless Workgroups", 25.0)
            .code("L-51089E7F")
            .unit("Count")
            .metric(usage_metric(
                "RedshiftServerless",
                &[
                    ("Class", "None"),
                    ("Resource", "ServerlessWorkgroups"),
                    ("Type", "Resource"),
                ],
            ))
            .describe("The maximum number of Redshift Serverless workgroups."),
        // The namespace plane, metered like the two rows above: the `Service`
        // dimension is the one the recorded rows carry, and the `Resource` dimension
        // is this catalog's naming of the series each quota counts, since no source
        // available here states the value the service publishes it under.
        quota("Serverless Namespaces", 25.0)
            .code("L-96772D74")
            .unit("Count")
            .metric(usage_metric(
                "RedshiftServerless",
                &[
                    ("Class", "None"),
                    ("Resource", "ServerlessNamespaces"),
                    ("Type", "Resource"),
                ],
            ))
            .describe("The maximum number of Redshift Serverless namespaces."),
    ]
}

/// Amazon Route 53 Resolver's published default-quota catalog.
///
/// SOURCE: AWS's published Service Quotas data for the `route53resolver` service code
/// (the "Quotas on Route 53 Resolver" page of the Amazon Route 53 Developer Guide,
/// which publishes the Resolver planes beside the DNS Firewall ones), and the recorded
/// default-quota listing of that code for which `QuotaCode` names which quota. The
/// rest of the service's published catalog names no quota this catalog can cite under
/// the remaining codes that listing reports, so those rows are left to the
/// default-quota-set gap rather than published under a guessed binding (see
/// [`super::descriptive_quota_code`]).
pub(super) fn route53resolver_quotas() -> Vec<QuotaRow> {
    vec![
        quota("DNS Firewall rule groups per Region", 1000.0)
            .code("L-02CC8B74")
            .describe("The maximum number of DNS Firewall rule groups per Region."),
        quota("DNS Firewall rule group associations per VPC", 5.0)
            .code("L-15219E1D")
            .fixed()
            .describe("The maximum number of DNS Firewall rule group associations per VPC."),
        // A plane counted in DOMAIN NAMES rather than in resources: the size of the
        // domain list a file imported from Amazon S3 may carry.
        quota("Domains in a file imported from S3", 250_000.0)
            .code("L-1B2BDF0A")
            .describe("The maximum number of domains in a file imported from Amazon S3."),
        quota("Resolver rules per AWS Region", 1000.0)
            .code("L-51D8A1FB")
            .describe("The maximum number of Resolver rules per AWS Region."),
        // The target plane of a rule, which AWS publishes as not adjustable.
        quota("Target IP addresses per resolver rule", 6.0)
            .code("L-D74B6237")
            .fixed()
            .describe("The maximum number of target IP addresses per Resolver rule."),
    ]
}

/// Amazon S3's published default-quota catalog.
///
/// SOURCE: AWS's published Service Quotas data for the `s3` service code (the "Amazon
/// S3 quotas" section of the Amazon S3 User Guide), and the recorded default-quota
/// listing of that code for which `QuotaCode` names which quota, for the units its
/// measured planes are published in and for the one-second window its transfer rate is
/// published over. The rest of the service's published catalog names no quota this
/// catalog can cite under the remaining codes that listing reports, so those rows are
/// left to the default-quota-set gap rather than published under a guessed binding
/// (see [`super::descriptive_quota_code`]).
pub(super) fn s3_quotas() -> Vec<QuotaRow> {
    vec![
        quota("Replication Destinations", 28.0)
            .code("L-1A9B467F")
            .describe("The maximum number of destinations in a replication configuration."),
        // A quota published as a RATE of data moved rather than as a count, so it
        // carries the one-second window AWS publishes it over.
        quota("Replication transfer rate", 1.0)
            .code("L-349AD9CA")
            .unit("Gigabits")
            .period(1, "SECOND")
            .describe("The maximum replication transfer rate, in gigabits per second."),
        // A plane measured in the SIZE of one object rather than in a count of them.
        quota("Object size", 48.828125)
            .code("L-89BABEE8")
            .unit("Terabytes")
            .describe("The maximum size of an object."),
        // The replication-rule plane of a bucket, which AWS publishes as not
        // adjustable.
        quota("Replication rules", 1000.0)
            .code("L-B461D596")
            .fixed()
            .describe("The maximum number of rules in a replication configuration."),
        quota("General purpose buckets", 10_000.0)
            .code("L-DC2B2D3D")
            .describe("The maximum number of general purpose buckets."),
    ]
}

/// One Amazon EventBridge Scheduler API REQUEST-RATE quota: `<api> request rate`, the
/// most requests to that one operation Scheduler serves per second.
///
/// Scheduler publishes a rate quota for each of its API operations, and every member
/// of such a row but the operation's own name and the value is the same across the
/// class: the `QuotaName` is the operation's name followed by `request rate`, the
/// window is one second, the usage metric is that operation's `AWS/Usage` `CallCount`
/// ([`call_count_metric`]) under the service's `AWS/Usage` display name, and the
/// `Description` is the sentence AWS publishes for a request-rate quota with the
/// operation it throttles named in it.
fn scheduler_request_rate(api_name: &str, requests_per_second: f64) -> QuotaRow {
    quota(format!("{api_name} request rate"), requests_per_second)
        .period(1, "SECOND")
        .metric(call_count_metric("EventBridge Scheduler", api_name))
        .describe(&format!(
            "The maximum number of {api_name} requests you can make per second in this \
             account in the current Region."
        ))
}

/// Amazon EventBridge Scheduler's published default-quota catalog.
///
/// SOURCE: AWS's published Service Quotas data for the `scheduler` service code (the
/// "Quotas for EventBridge Scheduler" page of the Amazon EventBridge Scheduler User
/// Guide), and the recorded default-quota listing of that code for which `QuotaCode`
/// names which quota: the account's schedule plane beside a per-API request rate for
/// each throttled operation. The rest of the service's published catalog names no
/// quota this catalog can cite under the remaining codes that listing reports, so
/// those rows are left to the default-quota-set gap rather than published under a
/// guessed binding (see [`super::descriptive_quota_code`]).
pub(super) fn scheduler_quotas() -> Vec<QuotaRow> {
    vec![
        // The read operations of the API share one published rate, and the write
        // operations their own, which is the pair of values these rows carry.
        scheduler_request_rate("ListSchedules", 1000.0).code("L-66629D52"),
        scheduler_request_rate("CreateSchedule", 5000.0).code("L-6F4B17DE"),
        quota("Number of schedules", 10_000_000.0)
            .code("L-EE5D6FF0")
            .describe("The maximum number of schedules."),
        scheduler_request_rate("GetSchedule", 1000.0).code("L-FB03E819"),
    ]
}

/// One AWS Secrets Manager API REQUEST-RATE quota: `Rate of <api> API requests`, the
/// most requests to that one operation Secrets Manager serves per second.
///
/// Secrets Manager publishes a rate quota for each of its API operations, and every
/// member of such a row but the operation's own name and the value is the same across
/// the class: the `QuotaName` names the operation whose requests the rate throttles,
/// the window is one second, the usage metric is that operation's `AWS/Usage`
/// `CallCount` ([`call_count_metric`]) under the service's `AWS/Usage` display name,
/// and the `Description` is the sentence AWS publishes for a request-rate quota.
fn secrets_manager_request_rate(api_name: &str, requests_per_second: f64) -> QuotaRow {
    quota(
        format!("Rate of {api_name} API requests"),
        requests_per_second,
    )
    .period(1, "SECOND")
    .metric(call_count_metric("Secrets Manager", api_name))
    .describe(&format!(
        "The maximum number of {api_name} requests you can make per second in this account \
         in the current Region."
    ))
}

/// AWS Secrets Manager's published default-quota catalog.
///
/// SOURCE: AWS's published Service Quotas data for the `secretsmanager` service code
/// (the "Quotas for AWS Secrets Manager" page of the AWS Secrets Manager User Guide),
/// and the recorded default-quota listing of that code for which `QuotaCode` names
/// which quota and for the adjustability of each: this service publishes its request
/// rates as NOT adjustable, which is why every row below is fixed. One of them is
/// published as a COMBINED rate over four resource-policy operations, so no single
/// operation's call count meters it. The rest of the service's published catalog names
/// no quota this catalog can cite under the remaining codes that listing reports, so
/// those rows are left to the default-quota-set gap rather than published under a
/// guessed binding (see [`super::descriptive_quota_code`]).
pub(super) fn secretsmanager_quotas() -> Vec<QuotaRow> {
    vec![
        secrets_manager_request_rate("GetSecretValue", 10_000.0)
            .code("L-2D877201")
            .fixed(),
        secrets_manager_request_rate("DescribeSecret", 40_000.0)
            .code("L-385CE7B7")
            .fixed(),
        quota(
            "Combined rate of DeleteResourcePolicy, GetResourcePolicy, PutResourcePolicy and \
             ValidateResourcePolicy API requests",
            50.0,
        )
        .code("L-8D82A813")
        .fixed()
        .period(1, "SECOND")
        .describe(
            "The maximum number of DeleteResourcePolicy, GetResourcePolicy, \
             PutResourcePolicy and ValidateResourcePolicy requests you can make per second \
             in this account in the current Region.",
        ),
        secrets_manager_request_rate("CreateSecret", 50.0)
            .code("L-CBA4CAC4")
            .fixed(),
    ]
}

/// AWS Directory Service's published default-quota catalog.
///
/// SOURCE: AWS's published Service Quotas data for the `ds` service code (the "AWS
/// Directory Service quotas" page of its Administration Guide) for each quota's
/// published name, default value and adjustability, and the recorded default-quota
/// listing of that code for which `QuotaCode` names which quota. The service
/// publishes one directory plane per directory TYPE, beside the manual-snapshot plane
/// of each type it snapshots and the domain-controller plane of a managed directory.
pub(super) fn ds_quotas() -> Vec<QuotaRow> {
    vec![
        quota("AD Connector directories", 10.0)
            .code("L-092080C3")
            .describe(
                "The maximum number of AD Connector directories that you can create in this \
                 account in the current Region.",
            ),
        quota("AWS Managed Microsoft AD directories", 20.0)
            .code("L-25146888")
            .describe(
                "The maximum number of AWS Managed Microsoft AD directories that you can \
                 create in this account in the current Region.",
            ),
        quota("Simple AD directories", 10.0)
            .code("L-5B5620DE")
            .describe(
                "The maximum number of Simple AD directories that you can create in this \
                 account in the current Region.",
            ),
        quota("Domain controllers per directory", 20.0)
            .code("L-92A19B29")
            // AWS publishes the domain-controller plane of a managed directory as fixed.
            .fixed()
            .describe(
                "The maximum number of domain controllers that you can deploy for your AWS \
                 Managed Microsoft AD directory.",
            ),
        // The two manual-snapshot planes, which AWS publishes as NOT adjustable: a
        // directory's snapshot allowance is fixed, so no increase can be requested
        // for it.
        quota("Simple AD manual snapshots", 5.0)
            .code("L-A18F1612")
            .fixed()
            .describe(
                "The maximum number of manual snapshots that you can take for your Simple AD \
                 directory.",
            ),
        quota("AWS Managed Microsoft AD manual snapshots", 5.0)
            .code("L-EF86B739")
            .fixed()
            .describe(
                "The maximum number of manual snapshots that you can take for your AWS \
                 Managed Microsoft AD directory.",
            ),
    ]
}

/// The `AWS/Usage` display name Amazon DynamoDB publishes its usage counts under.
const DYNAMODB_USAGE_SERVICE: &str = "DynamoDB";

/// Amazon DynamoDB's published default-quota catalog.
///
/// SOURCE: AWS's published Service Quotas data for the `dynamodb` service code (the
/// "Service, account, and table quotas in Amazon DynamoDB" page of the Amazon
/// DynamoDB Developer Guide), and the recorded default-quota listing of that code for
/// which `QuotaCode` names which quota. DynamoDB publishes its throughput planes
/// TWICE over — once for the account and once for a single table, and once for each of
/// the two capacity modes — which is why the rows are named for the plane and the mode
/// rather than for throughput as a whole.
pub(super) fn dynamodb_quotas() -> Vec<QuotaRow> {
    vec![
        quota(
            "Account-level read throughput limit (Provisioned mode)",
            80_000.0,
        )
        .code("L-1BB77E89")
        .describe(
            "The maximum number of read capacity units that can be provisioned across all \
             tables in this account in the current Region.",
        ),
        quota(
            "Account-level write throughput limit (Provisioned mode)",
            80_000.0,
        )
        .code("L-2A593B99")
        .describe(
            "The maximum number of write capacity units that can be provisioned across all \
             tables in this account in the current Region.",
        ),
        quota("Table-level read throughput limit", 40_000.0)
            .code("L-34F6A552")
            .describe(
                "The maximum number of read capacity units that can be provisioned for a \
                 single table.",
            ),
        // The secondary-index plane of a table, which AWS publishes as fixed.
        quota("Global secondary indexes per table", 20.0)
            .code("L-34F8CCC8")
            .fixed()
            .describe("The maximum number of global secondary indexes for a single table."),
        quota("Concurrent control plane operations", 500.0)
            .code("L-6F30DCE1")
            .fixed()
            .describe(
                "The maximum number of concurrent control plane operations in this account in \
                 the current Region.",
            ),
        quota(
            "Account-level read throughput limit (On-demand mode)",
            40_000.0,
        )
        .code("L-923BEB7A")
        .describe(
            "The maximum number of read request units that all on-demand tables in this \
             account in the current Region can serve per second.",
        ),
        quota("Table-level write throughput limit", 40_000.0)
            .code("L-AB614373")
            .describe(
                "The maximum number of write capacity units that can be provisioned for a \
                 single table.",
            ),
        quota(
            "Account-level write throughput limit (On-demand mode)",
            40_000.0,
        )
        .code("L-C2098644")
        .describe(
            "The maximum number of write request units that all on-demand tables in this \
             account in the current Region can serve per second.",
        ),
        quota(
            "Table-level read throughput limit (On-demand mode)",
            40_000.0,
        )
        .code("L-CF0CBE56")
        .describe(
            "The maximum number of read request units that a single on-demand table can \
             serve per second.",
        ),
        quota(
            "Table-level write throughput limit (On-demand mode)",
            40_000.0,
        )
        .code("L-D98E8184")
        .describe(
            "The maximum number of write request units that a single on-demand table can \
             serve per second.",
        ),
        quota("Concurrent import jobs", 50.0)
            .code("L-F3CA5463")
            .describe(
                "The maximum number of concurrent import jobs in this account in the current \
                 Region.",
            ),
        quota("Concurrent export jobs", 300.0)
            .code("L-F7858A77")
            .describe(
                "The maximum number of concurrent export jobs in this account in the current \
                 Region.",
            ),
        // The table plane of the account, the one DynamoDB publishes a CloudWatch
        // usage metric for (`AWS/Usage`, `ResourceCount`, `Resource=TableCount`).
        quota("Maximum number of tables", 2500.0)
            .code("L-F98FE922")
            .metric(usage_metric(
                DYNAMODB_USAGE_SERVICE,
                &[
                    ("Class", "None"),
                    ("Resource", "TableCount"),
                    ("Type", "Resource"),
                ],
            ))
            .describe(
                "The maximum number of tables that can be created in this account in the \
                 current Region.",
            ),
    ]
}

/// The `AWS/Usage` display name Amazon EBS publishes its API call counts under, which
/// is the `Service` dimension of every metered EBS quota.
const EBS_USAGE_SERVICE: &str = "EBS";

/// Amazon Elastic Block Store's published default-quota catalog.
///
/// SOURCE: AWS's published Service Quotas data for the `ebs` service code (the "Amazon
/// EBS quotas" page of the Amazon EBS User Guide) for the storage, IOPS and snapshot
/// planes it publishes, and the recorded default-quota listing of that code for which
/// `QuotaCode` names which quota and for the per-API request rates it publishes beside
/// them. EBS publishes a storage plane PER VOLUME TYPE — a volume type is what the
/// capacity is bought under — so a storage row is named for the type and carries the
/// `TiB` unit AWS publishes it in; a request-rate row carries the one-second window
/// and the `AWS/Usage` `CallCount` of the operation it throttles ([`request_rate`]).
pub(super) fn ebs_quotas() -> Vec<QuotaRow> {
    let rate =
        |code: &str, api: &str, per_second: f64| request_rate(code, EBS_USAGE_SERVICE, api, per_second);
    // A storage plane of one volume type: the capacity, in TiB, of all volumes of that
    // type in the Region.
    let storage = |code: &str, volume_type: &str, tebibytes: f64| {
        quota(format!("Storage for {volume_type} volumes, in TiB"), tebibytes)
            .code(code)
            .unit("TiB")
            .describe(&format!(
                "The maximum aggregated storage, in TiB, of all {volume_type} volumes in this \
                 account in the current Region."
            ))
    };
    // An IOPS plane of one volume type: the provisioned IOPS of all volumes of that
    // type in the Region.
    let iops = |code: &str, volume_type: &str, operations_per_second: f64| {
        quota(
            format!("IOPS for {volume_type} volumes"),
            operations_per_second,
        )
        .code(code)
        .describe(&format!(
            "The maximum aggregated IOPS provisioned for all {volume_type} volumes in this \
             account in the current Region."
        ))
    };
    vec![
        // The one volume-attachment rate AWS publishes as not adjustable.
        rate("L-028ACFB9", "AttachVolume", 100.0).fixed(),
        rate("L-07399329", "CompleteSnapshot", 100.0),
        rate("L-09BD8365", "CopySnapshot", 10.0),
        rate("L-1774F84A", "CreateSnapshot", 50.0),
        rate("L-18E976AB", "CreateSnapshots", 50.0),
        rate("L-1D4D9345", "CreateVolume", 100.0),
        storage("L-17AF77E8", "Throughput Optimized HDD (st1)", 50.0),
        rate("L-309BACF6", "DeleteSnapshot", 100.0),
        rate("L-35B31D98", "DeleteVolume", 100.0),
        rate("L-39BD5252", "DescribeFastSnapshotRestores", 100.0),
        rate("L-3A0E616D", "DescribeSnapshots", 100.0),
        rate("L-59C8FC87", "DescribeVolumeStatus", 100.0),
        rate("L-5F80CA91", "DescribeVolumes", 100.0),
        rate("L-631ECBD3", "DescribeVolumesModifications", 100.0),
        rate("L-651D1834", "DetachVolume", 100.0),
        rate("L-750405C3", "DisableFastSnapshotRestores", 10.0),
        storage("L-7A658B76", "General Purpose SSD (gp3)", 50.0),
        rate("L-82ACEF56", "EnableFastSnapshotRestores", 10.0),
        // The EBS direct APIs: the block-level read and write rates of a snapshot,
        // which AWS publishes far above its control-plane rates.
        rate("L-835364B2", "GetSnapshotBlock", 1000.0),
        iops("L-8656991D", "Provisioned IOPS SSD (io2)", 100_000.0),
        rate("L-87C9DEA6", "ListChangedBlocks", 100.0),
        storage("L-8D977E7E", "Provisioned IOPS SSD (io2)", 20.0),
        rate("L-915A3DBB", "ListSnapshotBlocks", 100.0),
        rate("L-94D7AB7D", "LockSnapshot", 10.0),
        rate("L-98A0B26D", "ModifySnapshotAttribute", 100.0),
        storage("L-9A0E0F82", "Magnetic (standard)", 20.0),
        storage("L-9CF3C2EB", "Cold HDD (sc1)", 50.0),
        rate("L-9F6E7C4E", "ModifySnapshotTier", 10.0),
        rate("L-A37D9CF3", "ModifyVolume", 100.0),
        rate("L-AFAE1BE8", "PutSnapshotBlock", 1000.0),
        rate("L-AFEBDF7A", "ResetSnapshotAttribute", 100.0),
        iops("L-B3A130E6", "Provisioned IOPS SSD (io1)", 300_000.0),
        rate("L-B9F7C487", "RestoreSnapshotTier", 10.0),
        // The attachment plane of a Multi-Attach enabled volume, which AWS publishes
        // as fixed.
        quota("Attachments per Multi-Attach enabled volume", 16.0)
            .code("L-C125AE42")
            .fixed()
            .describe(
                "The maximum number of instances that one Multi-Attach enabled volume can be \
                 attached to.",
            ),
        quota("In-progress snapshot archives per account", 5.0)
            .code("L-D0291BE3")
            .fixed()
            .describe(
                "The maximum number of snapshot archives that can be in progress at once in \
                 this account in the current Region.",
            ),
        storage("L-D18FCD1D", "General Purpose SSD (gp2)", 50.0),
        rate("L-D8F37C68", "StartSnapshot", 10.0),
        quota("Fast snapshot restores per Region", 5.0)
            .code("L-DB2FBAA1")
            .describe(
                "The maximum number of snapshots that fast snapshot restore can be enabled \
                 for in this account in the current Region.",
            ),
        quota("Concurrent snapshot copies per destination Region", 20.0)
            .code("L-DB70D580")
            .fixed()
            .describe(
                "The maximum number of snapshot copies that can be in progress at once to a \
                 single destination Region.",
            ),
        rate("L-E137849C", "UnlockSnapshot", 10.0),
        quota("Archived snapshots per volume", 25.0)
            .code("L-E20676C1")
            .describe(
                "The maximum number of snapshots of one volume that can be in the archive \
                 storage tier.",
            ),
        quota("Snapshots per Region", 100_000.0)
            .code("L-F06E64A8")
            .describe(
                "The maximum number of snapshots in this account in the current Region.",
            ),
        storage("L-FD252861", "Provisioned IOPS SSD (io1)", 50.0),
    ]
}

/// The `AWS/Usage` display name Amazon ECR publishes its API call counts under.
const ECR_USAGE_SERVICE: &str = "ECR";

/// Amazon Elastic Container Registry's published default-quota catalog.
///
/// SOURCE: AWS's published Service Quotas data for the `ecr` service code (the "Amazon
/// ECR service quotas" page of the Amazon ECR User Guide) for the registry, repository
/// and image planes it publishes, and the recorded default-quota listing of that code
/// for which `QuotaCode` names which quota and for the per-API request rates it
/// publishes beside them. A size plane carries the unit AWS publishes it in rather
/// than being a count, and a request-rate row carries the one-second window and the
/// `AWS/Usage` `CallCount` of the operation it throttles ([`request_rate`]).
pub(super) fn ecr_quotas() -> Vec<QuotaRow> {
    let rate =
        |code: &str, api: &str, per_second: f64| request_rate(code, ECR_USAGE_SERVICE, api, per_second);
    vec![
        quota("Registered repositories", 10_000.0)
            .code("L-03A36CE1")
            .describe(
                "The maximum number of repositories that can be registered in this account in \
                 the current Region.",
            ),
        quota("Images per repository", 10_000.0)
            .code("L-0A556EFC")
            .describe("The maximum number of images in one repository."),
        quota("Lifecycle policy rules per repository", 50.0)
            .code("L-1165B4AE")
            .fixed()
            .describe("The maximum number of rules in one repository's lifecycle policy."),
        quota("Filters per lifecycle policy rule", 100.0)
            .code("L-16E70933")
            .fixed()
            .describe("The maximum number of filters in one lifecycle policy rule."),
        quota("Repository policy length", 10_240.0)
            .code("L-241DEEBA")
            .unit("Bytes")
            .fixed()
            .describe("The maximum length, in bytes, of one repository's policy document."),
        quota("Layer parts per image layer", 10_000.0)
            .code("L-24725E9A")
            .fixed()
            .describe("The maximum number of parts that one image layer can be uploaded in."),
        quota("Maximum image layer size", 52.0)
            .code("L-343F3D94")
            .unit("Gigabytes")
            .fixed()
            .describe("The maximum size, in GiB, of one image layer."),
        quota("Pull through cache rules per registry", 25.0)
            .code("L-44194860")
            .describe("The maximum number of pull through cache rules in one registry."),
        quota("Replication destinations per registry", 25.0)
            .code("L-55A41110")
            .describe(
                "The maximum number of destinations in one registry's replication \
                 configuration.",
            ),
        rate("L-6EF7FC96", "BatchCheckLayerAvailability", 1000.0),
        rate("L-8BE1781E", "BatchDeleteImage", 100.0),
        rate("L-95B28F8D", "BatchGetImage", 10_000.0),
        rate("L-9B60BFFB", "CompleteLayerUpload", 100.0),
        rate("L-9C9D5A08", "CreateRepository", 10.0),
        rate("L-A1670B10", "DeleteRepository", 10.0),
        rate("L-A60A366D", "DescribeImages", 100.0),
        rate("L-A71EAF5A", "DescribeRepositories", 100.0),
        rate("L-AD52DFB2", "GetAuthorizationToken", 500.0),
        rate("L-B9173138", "GetDownloadUrlForLayer", 3000.0),
        rate("L-C0B33BA1", "InitiateLayerUpload", 100.0),
        rate("L-CB63E564", "ListImages", 100.0),
        rate("L-CFEB8E8D", "PutImage", 100.0),
        rate("L-EF8F6F4E", "UploadLayerPart", 1000.0),
    ]
}

/// Amazon Elastic Kubernetes Service's published default-quota catalog.
///
/// SOURCE: AWS's published Service Quotas data for the `eks` service code (the "Amazon
/// EKS service quotas" page of the Amazon EKS User Guide), and the recorded
/// default-quota listing of that code for which `QuotaCode` names which quota and for
/// each code's adjustability. EKS publishes its planes per CLUSTER as well as per
/// Region — a cluster is the unit its node groups, Fargate profiles and access
/// configuration are counted in — so a row names the plane it is counted on.
pub(super) fn eks_quotas() -> Vec<QuotaRow> {
    vec![
        quota("Clusters", 100.0)
            .code("L-11427A54")
            .describe(
                "The maximum number of EKS clusters in this account in the current Region.",
            ),
        quota("Managed node groups per cluster", 30.0)
            .code("L-1194D53C")
            .describe("The maximum number of managed node groups per cluster."),
        quota("Nodes per managed node group", 450.0)
            .code("L-23414FF3")
            .describe("The maximum number of nodes per managed node group."),
        quota("Fargate profiles per cluster", 10.0)
            .code("L-33415657")
            .describe("The maximum number of Fargate profiles per cluster."),
        quota("Selectors per Fargate profile", 5.0)
            .code("L-5C12D558")
            .fixed()
            .describe("The maximum number of selectors per Fargate profile."),
        quota("Label pairs per Fargate profile selector", 5.0)
            .code("L-6AFFD1D8")
            .fixed()
            .describe("The maximum number of label pairs per Fargate profile selector."),
        quota("Control plane security groups per cluster", 4.0)
            .code("L-6D54EA21")
            .fixed()
            .describe("The maximum number of control plane security groups per cluster."),
        quota("Public endpoint access CIDR ranges per cluster", 40.0)
            .code("L-93A74D60")
            .fixed()
            .describe(
                "The maximum number of CIDR ranges that public access to a cluster's endpoint \
                 can be restricted to.",
            ),
        quota("Access entries per cluster", 100.0)
            .code("L-BD136A63")
            .describe("The maximum number of access entries per cluster."),
        quota("Pod identity associations per cluster", 1000.0)
            .code("L-C56B9FC3")
            .describe("The maximum number of EKS Pod Identity associations per cluster."),
        quota("Registered clusters", 10.0)
            .code("L-D78D8AF8")
            .describe(
                "The maximum number of Kubernetes clusters that can be connected to EKS in \
                 this account in the current Region.",
            ),
        quota("Add-ons per cluster", 20.0)
            .code("L-EA277FDC")
            .describe("The maximum number of EKS add-ons per cluster."),
        quota("EKS Anywhere subscriptions", 10.0)
            .code("L-FDFA5F81")
            .describe(
                "The maximum number of EKS Anywhere subscriptions in this account in the \
                 current Region.",
            ),
    ]
}

/// The `AWS/Usage` display name Amazon ElastiCache publishes its resource counts
/// under, which is the `Service` dimension of every metered ElastiCache quota.
const ELASTICACHE_USAGE_SERVICE: &str = "ElastiCache";

/// Amazon ElastiCache's published default-quota catalog.
///
/// SOURCE: AWS's published Service Quotas data for the `elasticache` service code (the
/// "Quotas for ElastiCache" page of the Amazon ElastiCache User Guide), and the
/// recorded default-quota listing of that code for which `QuotaCode` names which
/// quota and for the usage metric each metered row carries. The planes ElastiCache
/// counts PER REGION are the ones CloudWatch publishes an `AWS/Usage` resource count
/// for, so those rows carry that metric and the window those counts are published
/// over, while a plane counted inside one cluster carries neither.
pub(super) fn elasticache_quotas() -> Vec<QuotaRow> {
    let metered = |code: &str, name: &str, value: f64, resource: &str, counted: &str| {
        resource_count_plane(code, name, value, ELASTICACHE_USAGE_SERVICE, resource, counted)
    };
    vec![
        metered(
            "L-3E7F7726",
            "Nodes per Region",
            300.0,
            "NodesPerRegion",
            "nodes",
        ),
        quota("Nodes per cluster (Memcached)", 40.0)
            .code("L-3F15A733")
            .describe("The maximum number of nodes in one Memcached cluster."),
        quota("Replicas per shard", 5.0)
            .code("L-75A7B5A4")
            .describe("The maximum number of read replicas per shard."),
        metered(
            "L-7D6587E6",
            "Parameter groups per Region",
            150.0,
            "ParameterGroupsPerRegion",
            "parameter groups",
        ),
        metered(
            "L-80E085C7",
            "Security groups per Region",
            50.0,
            "SecurityGroupsPerRegion",
            "cache security groups",
        ),
        quota("Shards per cluster (cluster mode enabled)", 500.0)
            .code("L-8C334AD1")
            .describe("The maximum number of shards in one cluster with cluster mode enabled."),
        metered(
            "L-943F0F1C",
            "Subnet groups per Region",
            150.0,
            "SubnetGroupsPerRegion",
            "subnet groups",
        ),
        quota("Subnets per subnet group", 20.0)
            .code("L-A87EE522")
            .describe("The maximum number of subnets in one subnet group."),
        quota("Users", 100.0)
            .code("L-AD484FC5")
            .describe(
                "The maximum number of users in this account in the current Region.",
            ),
        quota("User groups", 100.0)
            .code("L-AF354865")
            .describe(
                "The maximum number of user groups in this account in the current Region.",
            ),
        metered(
            "L-BBCDAECC",
            "Serverless caches per Region",
            40.0,
            "ServerlessCachesPerRegion",
            "serverless caches",
        ),
        metered(
            "L-DFE45DF3",
            "Manual backups per Region",
            1000.0,
            "ManualBackupsPerRegion",
            "manual backups",
        ),
    ]
}

/// AWS Elastic Beanstalk's published default-quota catalog.
///
/// SOURCE: AWS's published Service Quotas data for the `elasticbeanstalk` service code
/// (the "AWS Elastic Beanstalk service quotas" page of its Developer Guide), and the
/// recorded default-quota listing of that code for which `QuotaCode` names which
/// quota. Elastic Beanstalk publishes the planes of the deployment model it serves —
/// the applications of an account, the versions and configuration templates those
/// applications hold, the environments running them, and the custom platform versions
/// they run on.
pub(super) fn elasticbeanstalk_quotas() -> Vec<QuotaRow> {
    vec![
        quota("Applications", 75.0)
            .code("L-1CEABD17")
            .describe(
                "The maximum number of applications in this account in the current Region.",
            ),
        quota("Application versions", 1000.0)
            .code("L-8EFC1C51")
            .describe(
                "The maximum number of application versions in this account in the current \
                 Region.",
            ),
        quota("Configuration templates", 300.0)
            .code("L-9838E43F")
            .describe(
                "The maximum number of configuration templates in this account in the current \
                 Region.",
            ),
        quota("Environments", 200.0)
            .code("L-D64F1F14")
            .describe(
                "The maximum number of environments in this account in the current Region.",
            ),
        quota("Custom platform versions", 50.0)
            .code("L-E593A077")
            .describe(
                "The maximum number of custom platform versions in this account in the current \
                 Region.",
            ),
    ]
}

/// The `AWS/Usage` display name Amazon EFS publishes its API call counts under.
const EFS_USAGE_SERVICE: &str = "EFS";

/// Amazon Elastic File System's published default-quota catalog.
///
/// SOURCE: AWS's published Service Quotas data for the `elasticfilesystem` service
/// code (the "Amazon EFS quotas that you can increase" and "Quotas for Amazon EFS file
/// systems" tables of the Amazon EFS User Guide), and the recorded default-quota
/// listing of that code for which `QuotaCode` names which quota, for each code's
/// adjustability and for the per-API request rates it publishes beside the resource
/// planes. A throughput row carries the unit AWS publishes it in rather than being a
/// count, and a request-rate row carries the one-second window and the `AWS/Usage`
/// `CallCount` of the operation it throttles ([`request_rate`]).
pub(super) fn elasticfilesystem_quotas() -> Vec<QuotaRow> {
    let rate =
        |code: &str, api: &str, per_second: f64| request_rate(code, EFS_USAGE_SERVICE, api, per_second);
    vec![
        quota("File systems per account", 1000.0)
            .code("L-03A6A61D")
            .describe(
                "The maximum number of file systems in this account in the current Region.",
            ),
        quota("Access points per file system", 1000.0)
            .code("L-06519EB8")
            .describe("The maximum number of access points per file system."),
        quota("Mount targets per file system per Availability Zone", 1.0)
            .code("L-1E30FF38")
            .fixed()
            .describe(
                "The maximum number of mount targets that one file system can have in one \
                 Availability Zone.",
            ),
        quota("VPCs per file system", 1.0)
            .code("L-207AE8E0")
            .fixed()
            .describe("The maximum number of VPCs that one file system can be mounted from."),
        quota("Security groups per mount target", 5.0)
            .code("L-29770CF9")
            .fixed()
            .describe("The maximum number of security groups per mount target."),
        quota("Tags per file system", 50.0)
            .code("L-336A920F")
            .fixed()
            .describe("The maximum number of tags per file system."),
        quota("Replication configurations per file system", 1.0)
            .code("L-3D348029")
            .fixed()
            .describe("The maximum number of replication configurations per file system."),
        quota("Provisioned throughput per file system", 1024.0)
            .code("L-48B15094")
            .unit("Megabytes/Second")
            .describe(
                "The maximum throughput, in MiB/s, that can be provisioned for one file \
                 system.",
            ),
        quota("Total provisioned throughput per Region", 3072.0)
            .code("L-4CCB99B3")
            .unit("Megabytes/Second")
            .describe(
                "The maximum throughput, in MiB/s, that can be provisioned across all file \
                 systems in this account in the current Region.",
            ),
        rate("L-509A1582", "CreateAccessPoint", 5.0),
        rate("L-6D380DD0", "CreateFileSystem", 5.0),
        rate("L-7391004C", "CreateMountTarget", 5.0),
        rate("L-7FEABCD7", "CreateReplicationConfiguration", 5.0),
        rate("L-848C634D", "DeleteAccessPoint", 5.0),
        rate("L-A139E5A7", "DeleteFileSystem", 5.0),
        rate("L-B34D07B4", "DeleteMountTarget", 5.0),
        rate("L-B8B14E21", "DescribeAccessPoints", 100.0),
        rate("L-BFB6FB19", "DescribeFileSystems", 100.0),
        rate("L-C8C0D2A0", "DescribeMountTargets", 100.0),
        rate("L-CDCE244F", "DescribeReplicationConfigurations", 100.0),
        rate("L-D6548EE7", "PutBackupPolicy", 10.0),
        rate("L-F3C5D9EF", "PutLifecycleConfiguration", 10.0),
        rate("L-FA0AAA42", "UpdateFileSystem", 10.0),
    ]
}

/// The `AWS/Usage` display name Elastic Load Balancing publishes its resource counts
/// under, which is the `Service` dimension of every metered load-balancing quota.
const ELB_USAGE_SERVICE: &str = "Elastic Load Balancing";

/// Elastic Load Balancing's published default-quota catalog.
///
/// SOURCE: AWS's published Service Quotas data for the `elasticloadbalancing` service
/// code (the "Quotas for your Application Load Balancers", "…Network Load Balancers",
/// "…Gateway Load Balancers" and "…Classic Load Balancers" pages of the Elastic Load
/// Balancing User Guide), and the recorded default-quota listing of that code for
/// which `QuotaCode` names which quota, for each code's adjustability and for the
/// usage metric each metered row carries. Elastic Load Balancing publishes its planes
/// PER LOAD BALANCER TYPE — a type is what a listener, rule or target allowance is
/// bought under — so a row is named for the type it is counted on, and the per-Region
/// planes of each type are the ones CloudWatch publishes an `AWS/Usage` resource count
/// for.
pub(super) fn elasticloadbalancing_quotas() -> Vec<QuotaRow> {
    // One per-Region plane of a load-balancer type, metered like every other counted
    // plane ([`resource_count_plane`]).
    let metered = |code: &str, name: &str, value: f64, resource: &str, counted: &str| {
        resource_count_plane(code, name, value, ELB_USAGE_SERVICE, resource, counted)
    };
    vec![
        metered(
            "L-057ECE54",
            "Application Load Balancers per Region",
            50.0,
            "ApplicationLoadBalancersPerRegion",
            "Application Load Balancers",
        ),
        metered(
            "L-0BB7B635",
            "Classic Load Balancers per Region",
            20.0,
            "ClassicLoadBalancersPerRegion",
            "Classic Load Balancers",
        ),
        metered(
            "L-1A491844",
            "Network Load Balancers per Region",
            50.0,
            "NetworkLoadBalancersPerRegion",
            "Network Load Balancers",
        ),
        metered(
            "L-23568085",
            "Gateway Load Balancers per Region",
            100.0,
            "GatewayLoadBalancersPerRegion",
            "Gateway Load Balancers",
        ),
        metered(
            "L-332EE142",
            "Target groups per Region",
            3000.0,
            "TargetGroupsPerRegion",
            "target groups",
        ),
        quota("Listeners per Application Load Balancer", 50.0)
            .code("L-37E1617B")
            .describe("The maximum number of listeners per Application Load Balancer."),
        quota("Listeners per Network Load Balancer", 50.0)
            .code("L-43FE5A42")
            .describe("The maximum number of listeners per Network Load Balancer."),
        quota("Listeners per Gateway Load Balancer", 1.0)
            .code("L-52964454")
            .fixed()
            .describe("The maximum number of listeners per Gateway Load Balancer."),
        quota("Listeners per Classic Load Balancer", 100.0)
            .code("L-53DA6B97")
            .describe("The maximum number of listeners per Classic Load Balancer."),
        quota("Rules per Application Load Balancer", 100.0)
            .code("L-57A373D6")
            .describe(
                "The maximum number of rules per Application Load Balancer, not counting \
                 default rules.",
            ),
        quota("Certificates per Application Load Balancer", 25.0)
            .code("L-69A177A2")
            .describe(
                "The maximum number of certificates per Application Load Balancer, not \
                 counting default certificates.",
            ),
        quota("Certificates per Network Load Balancer", 25.0)
            .code("L-723DCCB6")
            .describe(
                "The maximum number of certificates per Network Load Balancer, not counting \
                 default certificates.",
            ),
        quota("Target groups per Application Load Balancer", 100.0)
            .code("L-7A15E3C5")
            .fixed()
            .describe("The maximum number of target groups per Application Load Balancer."),
        quota("Target groups per action per Application Load Balancer", 5.0)
            .code("L-7C45F98A")
            .fixed()
            .describe(
                "The maximum number of target groups per action for one Application Load \
                 Balancer.",
            ),
        quota("Targets per Application Load Balancer", 1000.0)
            .code("L-7E6692B2")
            .describe("The maximum number of targets per Application Load Balancer."),
        quota("Targets per Network Load Balancer", 3000.0)
            .code("L-7EED9B64")
            .describe("The maximum number of targets per Network Load Balancer."),
        quota("Targets per Gateway Load Balancer", 300.0)
            .code("L-822D1B1B")
            .describe("The maximum number of targets per Gateway Load Balancer."),
        quota(
            "Targets per Availability Zone per Network Load Balancer",
            500.0,
        )
        .code("L-8A66D0E6")
        .describe(
            "The maximum number of targets per Availability Zone per Network Load Balancer.",
        ),
        quota(
            "Targets per Availability Zone per Gateway Load Balancer",
            300.0,
        )
        .code("L-9365A611")
        .describe(
            "The maximum number of targets per Availability Zone per Gateway Load Balancer.",
        ),
        quota(
            "Targets per target group per Application Load Balancer",
            1000.0,
        )
        .code("L-9E6F9012")
        .describe("The maximum number of targets per target group per Application Load Balancer."),
        quota("Targets per target group per Network Load Balancer", 1000.0)
            .code("L-A012D4E8")
            .describe(
                "The maximum number of targets per target group per Network Load Balancer.",
            ),
        quota("Registered instances per Classic Load Balancer", 1000.0)
            .code("L-A0D0B863")
            .describe("The maximum number of instances registered with one Classic Load Balancer."),
        quota("Condition values per rule", 5.0)
            .code("L-A8F7060B")
            .fixed()
            .describe("The maximum number of condition values per rule."),
        quota("Condition wildcards per rule", 5.0)
            .code("L-B211E961")
            .fixed()
            .describe("The maximum number of condition wildcards per rule."),
        quota("Load balancers per target group", 1.0)
            .code("L-B22855CB")
            .fixed()
            .describe("The maximum number of load balancers one target group can be used by."),
        quota("Subnets per Availability Zone per load balancer", 1.0)
            .code("L-B6DF7632")
            .fixed()
            .describe(
                "The maximum number of subnets per Availability Zone for one load balancer.",
            ),
        quota("Trust stores per Region", 10.0)
            .code("L-CE3125E5")
            .describe(
                "The maximum number of trust stores in this account in the current Region.",
            ),
        quota("Certificate authority certificates per trust store", 25.0)
            .code("L-E9E9831D")
            .describe(
                "The maximum number of certificate authority certificates per trust store.",
            ),
        quota("Revocation lists per trust store", 10.0)
            .code("L-EBC0C08B")
            .describe("The maximum number of certificate revocation lists per trust store."),
        quota("Security groups per load balancer", 5.0)
            .code("L-EEF1AD04")
            .fixed()
            .describe("The maximum number of security groups per load balancer."),
        quota("Tags per load balancer", 50.0)
            .code("L-F31E307C")
            .fixed()
            .describe("The maximum number of tags per load balancer."),
    ]
}

/// A `UsageMetric` in the `AWS/Usage` namespace, the one Service Quotas reports for
/// a quota CloudWatch publishes usage for.
///
/// Every `AWS/Usage` metric carries the `Service` the usage belongs to beside the
/// dimensions the caller names, so that one is added here rather than at each call
/// site; `metric_name` and `statistic` are what distinguish a resource count from a
/// call count.
///
/// `usage_service_name` is the service's DISPLAY name as `AWS/Usage` publishes it
/// (`Certificate Manager`, `Kinesis Data Streams`), never its SDK/model id or its
/// `ServiceCode`: the dimension value is the key a client puts in a
/// `GetMetricData`/`GetMetricStatistics` query, so a metric built from the id selects
/// no data at all. For a few services the two spellings coincide (`EC2`, `Lambda`,
/// `DynamoDB`), and that is the published name rather than the id leaking in.
pub(super) fn aws_usage_metric(
    metric_name: &str,
    statistic: &str,
    usage_service_name: &str,
    dimensions: &[(&str, &str)],
) -> MetricInfo {
    let mut metric_dimensions = std::collections::HashMap::new();
    metric_dimensions.insert("Service".to_string(), usage_service_name.to_string());
    for (name, value) in dimensions {
        metric_dimensions.insert((*name).to_string(), (*value).to_string());
    }
    MetricInfo {
        metric_namespace: Some("AWS/Usage".to_string()),
        metric_name: Some(metric_name.to_string()),
        metric_dimensions: Some(metric_dimensions),
        metric_statistic_recommendation: Some(statistic.to_string()),
    }
}

/// The `UsageMetric` of a quota counted as a number of RESOURCES in use: the largest
/// count seen over the metric's period is the one that matters, so `Maximum` is the
/// statistic recommended for it.
pub(super) fn usage_metric(usage_service_name: &str, dimensions: &[(&str, &str)]) -> MetricInfo {
    aws_usage_metric("ResourceCount", "Maximum", usage_service_name, dimensions)
}

/// The `AWS/Usage` metric CloudWatch publishes the account's VPC count under, which is
/// the usage metric of the `VPCs per Region` quota.
///
/// The VPC resource counts CloudWatch publishes sit under the EC2 service dimension —
/// VPC is served by the EC2 API — which is the dimension the recorded response for that
/// quota carries. `Resource` is the dimension VALUE a client queries usage with, so it
/// is the one CloudWatch publishes the count under (`VPCsPerRegion`), not the resource
/// type.
///
/// It is the one construction of that metric, so the catalog row and the usage reading
/// that fills it in (`usage::measure`) cannot disagree about it.
pub(crate) fn vpcs_per_region_usage_metric() -> MetricInfo {
    usage_metric(
        "EC2",
        &[
            ("Class", "None"),
            ("Resource", "VPCsPerRegion"),
            ("Type", "Resource"),
        ],
    )
}

/// The `AWS/Usage` display name Amazon SES publishes its usage under, which is the
/// `Service` dimension of every metered SES quota.
pub(super) const SES_USAGE_SERVICE: &str = "SES";

/// The `UsageMetric` of the Amazon SES per-second sending rate.
///
/// SES publishes the messages it accepted for an account in its OWN metric namespace
/// rather than in `AWS/Usage`, under one metric that needs no dimension to identify
/// the series the rate is measured on, so the metric carries the EMPTY dimension map
/// the model's `@length 0..10` allows. Its statistic is `Sum`, because a rate is the
/// messages ADDED UP over the window.
pub(super) fn ses_send_metric() -> MetricInfo {
    MetricInfo {
        metric_namespace: Some("AWS/SES".to_string()),
        metric_name: Some("Send".to_string()),
        metric_dimensions: Some(std::collections::HashMap::new()),
        metric_statistic_recommendation: Some("Sum".to_string()),
    }
}

/// The `UsageMetric` of an API REQUEST-RATE quota: the `AWS/Usage` `CallCount`
/// metric of one API.
///
/// `AWS/Usage` identifies a call count by the usage `Type` (`API` for a call count,
/// against `Resource` for a resource count), the `Resource` the count is published
/// under (the API's own name) and the `Class` of usage (`None` where the service
/// publishes no class), beside the `Service` every `AWS/Usage` metric carries. Its
/// statistic is `Sum`, because a rate is the calls ADDED UP over the quota's
/// `Period`, not the largest one seen in it.
pub(super) fn call_count_metric(usage_service_name: &str, api_name: &str) -> MetricInfo {
    aws_usage_metric(
        "CallCount",
        "Sum",
        usage_service_name,
        &[
            ("Class", "None"),
            ("Resource", api_name),
            ("Type", "API"),
        ],
    )
}

/// The `AWS/Usage` display name Amazon CloudWatch publishes its call counts under,
/// which is the `Service` dimension of every metered CloudWatch quota.
const CLOUDWATCH_USAGE_SERVICE: &str = "CloudWatch";

/// The requests per second the rate rows this catalog cites for Amazon CloudWatch carry
/// where the operation they throttle states no rate of its own.
///
/// SOURCE: the recorded default-quota listing of `monitoring`, which carries this value
/// on the rate rows of the two such operations this catalog cites (`ListDashboards` and
/// `DescribeAnomalyDetectors`). It is what THOSE rows are published at, not a default to
/// fill a further operation's row from: an operation that publishes its own rate carries
/// that one (`GetMetricWidgetImage`), and a row whose published rate nothing available
/// here states is left unpublished rather than filled in from this constant.
pub(super) const CLOUDWATCH_DEFAULT_REQUEST_RATE: f64 = 5.0;

/// One Amazon CloudWatch API REQUEST-RATE quota: `Rate of <api> requests`, the most
/// requests to that one operation CloudWatch serves per second.
///
/// CloudWatch publishes a rate quota for each of its API operations, and every member
/// of such a row but the operation's own name and the limit is the same across the
/// class: the `QuotaName` names the operation the rate is of, the window is one second,
/// the usage metric is that operation's `AWS/Usage` `CallCount` ([`call_count_metric`])
/// under the `CloudWatch` service dimension, and the `Description` is the sentence the
/// class is published with, with the throttled operation named in it. Those are the
/// quota's PUBLISHED members, so one constructor fills them and two rows of the class
/// cannot drift apart.
///
/// SOURCE: the recorded default-quota listing of `monitoring` for the class's name
/// form, its one-second window, the call-count metric each row carries and the sentence
/// above. The rate is the operation's own where its documentation states one, else the
/// rate the recording carries for that row ([`CLOUDWATCH_DEFAULT_REQUEST_RATE`]).
pub(super) fn cloudwatch_request_rate(api_name: &str, requests_per_second: f64) -> QuotaRow {
    quota(
        format!("Rate of {api_name} requests"),
        requests_per_second,
    )
    .period(1, "SECOND")
    .metric(call_count_metric(CLOUDWATCH_USAGE_SERVICE, api_name))
    .describe(&format!(
        "The maximum number of {api_name} requests that you can make, per second, in this \
         account in the current region."
    ))
}

/// One AWS KMS API REQUEST-RATE quota: `<api> request rate`, the most requests to that
/// one operation KMS serves per second.
///
/// KMS publishes a rate quota for each of its API operations, and every member of such
/// a row but the operation's own name and the value is the same across the class: the
/// `QuotaName` is the operation's name followed by `request rate`, the window is one
/// second, the usage metric is that operation's `AWS/Usage` `CallCount`
/// ([`call_count_metric`]) under the `KMS` service dimension, and the `Description` is
/// the sentence KMS publishes for the class with the operation it throttles named in
/// it. Those are the quota's PUBLISHED members, so one constructor fills them and two
/// rows of the class cannot drift apart.
///
/// SOURCE: AWS KMS's published request quotas (the per-operation request-quota table of
/// the KMS Developer Guide, which is what the service's default-quota listing reports),
/// and the recorded default-quota listing of `kms` for the wording and for which
/// `QuotaCode` names which operation's rate.
pub(super) fn kms_request_rate(api_name: &str, requests_per_second: f64) -> QuotaRow {
    quota(format!("{api_name} request rate"), requests_per_second)
        .period(1, "SECOND")
        .metric(call_count_metric("KMS", api_name))
        .describe(&format!(
            "Maximum {api_name} requests per second. When you reach this quota, KMS \
             rejects requests for this operation for the remainder of the interval."
        ))
}
