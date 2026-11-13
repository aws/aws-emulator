// SPDX-License-Identifier: CC0-1.0

//! What a row of the AWS default-quota catalog may publish, over the wire.
//!
//! `ListAWSDefaultServiceQuotas` / `GetAWSDefaultServiceQuota` report AWS's published
//! default-quota data for a `ServiceCode`, so every field of a row is a FACT about
//! that quota rather than something this emulator may paraphrase or mint. The rules
//! asserted here, each over a TABLE of rows rather than one case:
//!
//! * a published row reports the `QuotaCode`, `Value`, `Adjustable`, `Unit`, `Period`
//!   and `Description` AWS publishes for the quota, with the description in AWS's own
//!   wording (the scope it names — per account, per Region — is part of the text);
//! * `UsageMetric.MetricDimensions.Service` is the service's display name as
//!   `AWS/Usage` publishes it, which is the value a `GetMetricData` query must name —
//!   never the service's SDK/model id and never its `ServiceCode`;
//! * an AWS default quota's `QuotaArn` names the REGION it was read at, for every row
//!   including a quota counted account-wide, and carries no account;
//! * a row the recorded default-quota listing of its service reports as a surplus
//!   entry — a quota that service publishes nothing under — is NOT published, so a
//!   client never reads back a code AWS would reject.
//!
//! The harness (`client`, `walk_pages`) is the one `quota_workflow.rs` owns.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};

// The catalog walks live in the shared harness (`quota_workflow.rs`), which also owns
// `client`/`walk_pages`, so `catalog_identity.rs` and this suite share one copy.
use crate::quota_workflow::{
    client, default_quotas as defaults, emulator, whole_default_catalog as whole_catalog,
};

/// One published default quota, as AWS's default-quota data for its `ServiceCode`
/// reports it.
struct Row {
    service_code: &'static str,
    quota_code: &'static str,
    quota_name: &'static str,
    value: f64,
    unit: &'static str,
    adjustable: bool,
    /// The description AWS publishes, where the evidence states its text; `None`
    /// pins no description (see the assertion loop).
    description: Option<&'static str>,
    /// `(PeriodValue, PeriodUnit)` for a quota AWS publishes over a window.
    period: Option<(i32, &'static str)>,
    /// `(Service, Resource, Type)` of the `AWS/Usage` metric dimensions, for a quota
    /// CloudWatch publishes usage for.
    metric: Option<(&'static str, &'static str, &'static str)>,
}

/// The rows this suite pins.
///
/// SOURCE: AWS's published default-quota data for each `ServiceCode` (what
/// `aws service-quotas list-aws-default-service-quotas --service-code <code>`
/// reports). The table spans the class rather than one shape: a countable quota and a
/// not-adjustable one, units other than `None`, a rate quota with a `Period`, a
/// metered quota with an `AWS/Usage` resource count, a metered API rate with a call
/// count, descriptions that name a per-account and a per-Region scope, values from 1
/// to 100000, and codes in AWS's minted `L-` namespace beside codes AWS publishes
/// descriptively.
fn rows() -> Vec<Row> {
    vec![
        Row {
            service_code: "access-analyzer",
            quota_code: "L-5EFCE71D",
            quota_name: "CloudTrail log files processed per policy generation",
            value: 100000.0,
            unit: "None",
            adjustable: false,
            description: Some(
                "The maximum number of CloudTrail log files that can be processed per policy \
                 generation.",
            ),
            period: None,
            metric: None,
        },
        Row {
            service_code: "access-analyzer",
            quota_code: "AnalyzersPerRegion",
            quota_name: "Analyzers per Region",
            value: 1.0,
            unit: "None",
            adjustable: false,
            description: None,
            period: None,
            metric: None,
        },
        Row {
            service_code: "account-access",
            quota_code: "L-42A1D874",
            quota_name: "Group entitlements per role",
            value: 20.0,
            unit: "None",
            adjustable: true,
            description: Some(
                "The maximum number of group entitlements that can be assigned to a single \
                 role in an application.",
            ),
            period: None,
            metric: None,
        },
        Row {
            service_code: "acm",
            quota_code: "L-DD60D132",
            quota_name: "Rate of ListAcmeDomainValidations API requests",
            value: 50.0,
            unit: "None",
            adjustable: true,
            description: None,
            period: Some((1, "SECOND")),
            metric: Some(("Certificate Manager", "ListAcmeDomainValidations", "API")),
        },
        Row {
            service_code: "acm-pca",
            quota_code: "L-01B8608D",
            quota_name: "Rate of CreateCertificateAuthority requests",
            value: 1.0,
            unit: "None",
            adjustable: false,
            description: Some(
                "The maximum number of CreateCertificateAuthority requests that can be \
                 performed in an account in the current Region each second.",
            ),
            period: Some((1, "SECOND")),
            metric: None,
        },
        Row {
            service_code: "aco-automation",
            quota_code: "L-1501440B",
            quota_name: "Throttle rate limit for ListAutomationRules",
            value: 5.0,
            unit: "None",
            adjustable: false,
            description: Some(
                "Maximum number of ListAutomationRules requests you can make per second per \
                 account in the current region.",
            ),
            period: Some((1, "SECOND")),
            metric: None,
        },
        Row {
            service_code: "agent-registry",
            quota_code: "L-7F384C37",
            quota_name: "InvokeRegistryMcp requests per second",
            value: 10.0,
            unit: "Count/Second",
            adjustable: true,
            description: None,
            period: Some((1, "SECOND")),
            metric: Some(("Agent Registry", "InvokeRegistryMcp", "API")),
        },
        Row {
            service_code: "aidevops",
            quota_code: "L-4FF187E6",
            quota_name: "concurrent-evaluations-per-agent-space",
            value: 1.0,
            unit: "Count",
            adjustable: false,
            description: Some("Concurrent evaluations per agent space"),
            period: None,
            metric: None,
        },
        Row {
            service_code: "aiops",
            quota_code: "L-EF31E0C4",
            quota_name: "Concurrent active investigations",
            value: 2.0,
            unit: "None",
            adjustable: false,
            description: Some(
                "The maximum number of concurrent investigations with active AI analysis per \
                 account per region.",
            ),
            period: Some((5, "MINUTE")),
            metric: Some((
                "Amazon AI Operations",
                "ConcurrentActiveInvestigations",
                "Resource",
            )),
        },
        Row {
            service_code: "airflow",
            quota_code: "L-67D41C6B",
            quota_name: "Environments per account per Region (small, medium, large)",
            value: 10.0,
            unit: "None",
            adjustable: true,
            description: Some("The maximum number of environments per account per Region."),
            period: None,
            metric: None,
        },
        Row {
            service_code: "codecommit",
            quota_code: "L-81790602",
            quota_name: "Repositories per account",
            value: 5000.0,
            unit: "None",
            adjustable: true,
            description: None,
            period: None,
            metric: None,
        },
        Row {
            service_code: "config",
            quota_code: "L-0BE82991",
            quota_name: "AWS Config Rules per region",
            value: 1000.0,
            unit: "None",
            adjustable: false,
            description: Some(
                "The maximum number of AWS Config Rules per region in an AWS account",
            ),
            period: None,
            metric: None,
        },
        // A quota published in BYTES, and not adjustable: the unit is part of what a
        // row publishes, so a value is never reported under the countable `None`.
        Row {
            service_code: "signin",
            quota_code: "L-42359A23",
            quota_name: "Maximum Signin resource based policy statement size",
            value: 10240.0,
            unit: "Bytes",
            adjustable: false,
            description: Some(
                "The maximum size of individual Resource-Based Policy statements in bytes",
            ),
            period: None,
            metric: None,
        },
        Row {
            service_code: "kinesis",
            quota_code: "L-0918CF54",
            quota_name: "Shards per Region",
            value: 20000.0,
            unit: "None",
            adjustable: true,
            description: Some(
                "The maximum number of shards that you can create in this account in the \
                 current Region.",
            ),
            period: Some((5, "MINUTE")),
            metric: Some(("Kinesis Data Streams", "ProvisionedShards", "Resource")),
        },
    ]
}

/// The `Service` dimension value `AWS/Usage` publishes a service's usage metrics
/// under, keyed by the Service Quotas `ServiceCode` of the quota that carries the
/// metric.
///
/// SOURCE: the `Service` dimension of the `AWS/Usage` metrics CloudWatch publishes
/// for each service — the service's display name, which is the value a
/// `GetMetricData` query selects on. `vpc` appears under `EC2` because the VPC
/// resource counts are published by the EC2 API.
///
/// A service code may appear MORE THAN ONCE: `AWS/Usage` publishes some services'
/// resource counts and their API call counts under different display names, and each is
/// the value a query for that series has to name. Every entry of a code is accepted for
/// a row of that service.
///
/// Restated here so a row that drifts back to a service's SDK/model id fails.
const USAGE_METRIC_SERVICE_NAMES: &[(&str, &str)] = &[
    ("acm", "Certificate Manager"),
    ("agent-registry", "Agent Registry"),
    ("aiops", "Amazon AI Operations"),
    ("appconfig", "AppConfig"),
    ("athena", "Athena"),
    ("autoscaling", "AutoScaling"),
    // Amazon Cognito User Pools' call counts are published under the user-pool display
    // name, and AWS Direct Connect's resource counts under the service's own.
    ("cognito-idp", "Cognito User Pool"),
    ("directconnect", "Direct Connect"),
    ("dynamodb", "DynamoDB"),
    // Amazon EBS's API call counts and Amazon ECR's are published under each
    // service's short display name, and the file-system and load-balancing counts
    // under the names `AWS/Usage` publishes those services as.
    ("cloudhsm", "CloudHSM"),
    ("cloudtrail", "CloudTrail"),
    ("ebs", "EBS"),
    ("ec2", "EC2"),
    ("ecr", "ECR"),
    ("elasticache", "ElastiCache"),
    ("elasticfilesystem", "EFS"),
    ("elasticloadbalancing", "Elastic Load Balancing"),
    ("events", "EventBridge"),
    // `AWS/Usage` publishes Amazon Data Firehose's API call counts under the service's
    // current display name, which the recorded default-quota listing of `firehose`
    // carries on every metered rate row — not the former "Kinesis Data Firehose"
    // product name — and its delivery-stream RESOURCE count under the short one.
    ("firehose", "Kinesis Firehose"),
    ("firehose", "Firehose"),
    ("fms", "Firewall Manager"),
    ("fsx", "FSx"),
    ("gamelift", "GameLift"),
    ("glacier", "Glacier"),
    ("glue", "Glue"),
    ("imagebuilder", "EC2 Image Builder"),
    ("kinesis", "Kinesis Data Streams"),
    ("kms", "KMS"),
    ("lambda", "Lambda"),
    ("monitoring", "CloudWatch"),
    ("rds", "RDS"),
    ("redshift-serverless", "RedshiftServerless"),
    ("scheduler", "EventBridge Scheduler"),
    ("secretsmanager", "Secrets Manager"),
    ("ses", "SES"),
    ("sns", "SNS"),
    ("timestream", "Timestream"),
    ("vpc", "EC2"),
    ("wafv2", "WAFV2"),
    ("workspaces", "WorkSpaces"),
    ("xray", "X-Ray"),
];

/// The `(ServiceCode, QuotaCode)` pairs this catalog must NOT publish: the recorded
/// default-quota listing of each of these services carries no row under the code,
/// reporting it as a surplus entry, so the service publishes no quota under it.
///
/// SOURCE: the recorded `ListAWSDefaultServiceQuotas` response for each of these
/// service codes. This is the one exception to the code rule stated on
/// `src/state.rs::descriptive_quota_code`; every other row keeps the code that rule
/// gives it.
const SURPLUS_QUOTA_CODES: &[(&str, &str)] = &[
    ("autoscaling-plans", "ScalingPlansPerRegion"),
    ("batch", "ComputeEnvironments"),
    ("batch", "JobQueues"),
    ("cassandra", "TablesPerRegion"),
    ("cloud9", "Environments"),
    ("cloudhsm", "ClustersPerRegion"),
    ("cloudtrail", "TrailsPerRegion"),
    ("codeartifact", "DomainsPerAccount"),
    ("codedeploy", "ApplicationsPerRegion"),
    ("codedeploy", "DeploymentGroupsPerApplication"),
    ("codepipeline", "PipelinesPerRegion"),
    ("cognito-identity", "IdentityPools"),
    ("cognito-idp", "UserPoolsPerAccount"),
    ("cognito-idp", "AppClientsPerUserPool"),
    ("comprehend", "CustomClassifiersPerAccount"),
    ("config", "DeliveryChannels"),
    ("datasync", "TasksPerAccount"),
    ("datasync", "AgentsPerAccount"),
    ("dax", "NodesPerCluster"),
    ("directconnect", "ConnectionsPerRegion"),
    ("dms", "Endpoints"),
    ("dms", "ReplicationInstances"),
    ("docdb", "Clusters"),
    ("docdb", "InstancesPerCluster"),
    ("emr-serverless", "ApplicationsPerAccount"),
    ("kinesis", "ShardsPerRegion"),
    ("kinesisanalytics", "ApplicationsPerRegion"),
    ("quicksight", "Datasets"),
    ("ram", "ResourceSharesPerAccount"),
    ("rds", "DBInstances"),
    ("rds", "DBClusters"),
    ("rds", "ManualDBInstanceSnapshots"),
    ("redshift", "Clusters"),
    ("redshift", "NodesPerCluster"),
    ("redshift-serverless", "NamespacesPerAccount"),
    ("resource-groups", "ResourceGroupsPerAccount"),
    ("route53resolver", "ResolverEndpointsPerRegion"),
    ("route53resolver", "ResolverRulesPerRegion"),
    ("s3", "AccessPointsPerRegion"),
    ("s3", "MultiRegionAccessPointsPerAccount"),
    ("scheduler", "SchedulesPerAccount"),
    ("secretsmanager", "SecretsPerRegion"),
    ("shield", "ProtectionsPerAccount"),
];

/// Every pinned row reports the facts AWS publishes for it, through the single read
/// and the listing alike.
#[tokio::test]
async fn a_published_default_quota_reports_the_facts_aws_publishes() {
    let engine = emulator();
    let client = client(&engine, "us-west-2");

    for row in rows() {
        let case = format!("{}/{}", row.service_code, row.quota_code);
        let pair = json!({"ServiceCode": row.service_code, "QuotaCode": row.quota_code});
        let quota = client.ok("GetAWSDefaultServiceQuota", pair).await["Quota"].clone();

        assert_eq!(
            quota["QuotaName"],
            json!(row.quota_name),
            "case {case}: expected the published quota name, got {quota}"
        );
        assert_eq!(
            quota["Value"],
            json!(row.value),
            "case {case}: expected the published default value, got {quota}"
        );
        assert_eq!(
            quota["Unit"],
            json!(row.unit),
            "case {case}: expected the published unit, got {quota}"
        );
        assert_eq!(
            quota["Adjustable"],
            json!(row.adjustable),
            "case {case}: expected the published adjustability, got {quota}"
        );
        // `None` means the evidence states no description text for this quota, so the
        // row pins none: a wrong assertion would be worse than no assertion. The rows
        // the evidence does state are pinned byte for byte.
        if let Some(description) = row.description {
            assert_eq!(
                quota["Description"],
                json!(description),
                "case {case}: a description is the service's own published text, got {quota}"
            );
        }
        match row.period {
            Some((value, unit)) => assert_eq!(
                quota["Period"],
                json!({"PeriodValue": value, "PeriodUnit": unit}),
                "case {case}: expected the window AWS publishes the quota over, got {quota}"
            ),
            None => assert_eq!(
                quota["Period"],
                Value::Null,
                "case {case}: a quota AWS publishes no window for reports none, got {quota}"
            ),
        }
        match row.metric {
            Some((service, resource, usage_type)) => {
                assert_eq!(
                    quota["UsageMetric"]["MetricNamespace"],
                    json!("AWS/Usage"),
                    "case {case}: a usage metric is published in AWS/Usage, got {quota}"
                );
                assert_eq!(
                    quota["UsageMetric"]["MetricDimensions"]["Service"],
                    json!(service),
                    "case {case}: the Service dimension is the name AWS/Usage publishes the \
                     service under, got {quota}"
                );
                assert_eq!(
                    quota["UsageMetric"]["MetricDimensions"]["Resource"],
                    json!(resource),
                    "case {case}: the Resource dimension is the one the count is published \
                     under, got {quota}"
                );
                assert_eq!(
                    quota["UsageMetric"]["MetricDimensions"]["Type"],
                    json!(usage_type),
                    "case {case}: expected the usage Type of the metric, got {quota}"
                );
            }
            None => assert_eq!(
                quota["UsageMetric"],
                Value::Null,
                "case {case}: a quota CloudWatch publishes no usage for reports no metric, got \
                 {quota}"
            ),
        }

        // The listing carries exactly the record the single read returned.
        let listed = defaults(&client, row.service_code).await;
        assert!(
            listed.contains(&quota),
            "case {case}: the listing must carry the record the single read returned, got \
             {listed:?}"
        );
    }
}

/// For EVERY row of the whole catalog that carries a usage metric, the `Service`
/// dimension is the name `AWS/Usage` publishes the service under — not its
/// `ServiceCode` and not an SDK/model id.
#[tokio::test]
async fn every_usage_metric_names_the_service_as_aws_usage_publishes_it() {
    let engine = emulator();
    let client = client(&engine, "eu-west-1");

    let mut published: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (service_code, usage_service) in USAGE_METRIC_SERVICE_NAMES {
        published.entry(service_code).or_default().push(usage_service);
    }
    let catalog = whole_catalog(&client).await;
    let mut metered = 0_usize;

    for (service_code, quotas) in &catalog {
        for quota in quotas {
            // The rule is about the metrics `AWS/Usage` publishes: a service that
            // publishes the account's usage in its OWN namespace identifies the
            // series the way that namespace does, so it is not judged here.
            if quota["UsageMetric"]["MetricNamespace"] != json!("AWS/Usage") {
                continue;
            }
            let Some(dimensions) = quota["UsageMetric"]["MetricDimensions"].as_object() else {
                continue;
            };
            let code = quota["QuotaCode"].as_str().unwrap_or_default();
            let case = format!("{service_code}/{code}");
            let expected = published.get(service_code.as_str()).unwrap_or_else(|| {
                panic!(
                    "case {case}: a metered quota's service must have its AWS/Usage name in \
                     USAGE_METRIC_SERVICE_NAMES, got {quota}"
                )
            });
            let dimension = dimensions
                .get("Service")
                .and_then(Value::as_str)
                .unwrap_or_default();
            assert!(
                expected.contains(&dimension),
                "case {case}: expected an AWS/Usage service name of {expected:?}, got \
                 {dimension:?}"
            );
            assert_ne!(
                dimension,
                service_code.as_str(),
                "case {case}: the dimension is a display name, never the ServiceCode, got \
                 {dimension:?}"
            );
            metered += 1;
        }
    }

    assert!(
        metered >= published.len(),
        "case catalog: the catalog must publish usage metrics for the metered quotas, got \
         {metered} rows"
    );
}

/// An AWS default quota's ARN names the Region it was read at and no account — for
/// every row of the whole catalog, including the rows counted account-wide, at two
/// endpoints.
#[tokio::test]
async fn an_aws_default_quota_arn_names_the_region_it_was_read_at() {
    let engine = emulator();

    for region in ["us-west-2", "ap-northeast-1"] {
        let client = client(&engine, region);
        let catalog = whole_catalog(&client).await;
        let mut global_rows = 0_usize;
        let mut rows = 0_usize;

        for (service_code, quotas) in &catalog {
            for quota in quotas {
                let code = quota["QuotaCode"].as_str().unwrap_or_default();
                let case = format!("{region} {service_code}/{code}");
                assert_eq!(
                    quota["QuotaArn"],
                    json!(format!(
                        "arn:aws:servicequotas:{region}::{service_code}/{code}"
                    )),
                    "case {case}: a default quota's ARN names the Region and no account, got \
                     {quota}"
                );
                if quota["GlobalQuota"] == json!(true) {
                    global_rows += 1;
                }
                rows += 1;
            }
        }

        assert!(
            rows > 100 && global_rows > 0,
            "case {region}: the rule must be asserted over the catalog including quotas counted \
             account-wide, got {rows} rows of which {global_rows} are account-wide"
        );
    }
}

/// A quota code the service publishes nothing under is not served: the service still
/// resolves, its listing carries no such row, and a lookup by that code is refused.
#[tokio::test]
async fn a_surplus_quota_code_is_not_published() {
    let engine = emulator();
    let client = client(&engine, "us-east-1");

    for (service_code, quota_code) in SURPLUS_QUOTA_CODES {
        let case = format!("{service_code}/{quota_code}");
        let listed = defaults(&client, service_code).await;
        let codes: BTreeSet<&str> = listed
            .iter()
            .filter_map(|quota| quota["QuotaCode"].as_str())
            .collect();
        assert!(
            !codes.contains(quota_code),
            "case {case}: a code this service publishes nothing under must not be listed, got \
             {codes:?}"
        );

        for operation in ["GetAWSDefaultServiceQuota", "GetServiceQuota"] {
            let (status, error) = client
                .refused(
                    operation,
                    json!({"ServiceCode": service_code, "QuotaCode": quota_code}),
                )
                .await;
            assert_eq!(
                (status, error.as_str()),
                (400, "NoSuchResourceException"),
                "case {case}: {operation} must refuse a code this catalog does not publish"
            );
        }

        // The service code itself still resolves: it is the QUOTA that is unpublished,
        // not the service.
        let response = client
            .ok(
                "ListAWSDefaultServiceQuotas",
                json!({"ServiceCode": service_code}),
            )
            .await;
        assert!(
            response["Quotas"].is_array(),
            "case {case}: the service must still resolve its (possibly empty) listing, got \
             {response}"
        );
    }
}
