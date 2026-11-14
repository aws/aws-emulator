// SPDX-License-Identifier: CC0-1.0

//! The service tokens a `ResourceTypeFilters` entry may name.
//!
//! A filter entry is `service[:resourceType]`, and "the string for each service
//! name and resource type is the same as that embedded in a resource's Amazon
//! Resource Name (ARN)" (`GetResourcesInput.ResourceTypeFilters`
//! documentation) — so the service half is the lower-case namespace an ARN's
//! third field carries, never a display spelling (`Lambda`), an upper-case one
//! (`EC2`) or a misspelling. A filter naming a token that is not one of the
//! supported services is therefore a request-level validation failure, not a
//! query that happens to select nothing: the service refuses it with
//! `InvalidParameterException` and [`unsupported_service_message`].
//!
//! Only the SERVICE half is checked. The resource-type half selects within a
//! supported service, so a type that names nothing (`s3:Bucket`) is a query that
//! matches no resource — an empty page, not an error — exactly as a supported
//! service holding no tagged resource is.

/// Every service namespace a `ResourceTypeFilters` entry may name.
///
/// Source: the list the member's own documentation points at — "For the list of
/// services whose resources you can tag using the Resource Groups Tagging API,
/// see Services that support the Resource Groups Tagging API"
/// (`GetResourcesInput.ResourceTypeFilters`, which also gives
/// `ec2:instance,s3:bucket,auditmanager` as a worked example of the three
/// spellings) — as the namespace each of those services embeds in its ARNs.
///
/// Ordered and deduplicated by construction so the table reads as the reference
/// page does; membership is asked through [`is_supported_service`] rather than
/// by indexing it.
pub const SUPPORTED_SERVICES: &[&str] = &[
    "a4b",
    "access-analyzer",
    "acm",
    "acm-pca",
    "airflow",
    "amplify",
    "amplifyuibuilder",
    "aoss",
    "apigateway",
    "appconfig",
    "appflow",
    "application-autoscaling",
    "applicationinsights",
    "appmesh",
    "apprunner",
    "appstream",
    "appsync",
    "aps",
    "athena",
    "auditmanager",
    "autoscaling",
    "backup",
    "backup-gateway",
    "batch",
    "bedrock",
    "braket",
    "budgets",
    "cases",
    "cassandra",
    "ce",
    "chime",
    "cleanrooms",
    "cloud9",
    "clouddirectory",
    "cloudformation",
    "cloudfront",
    "cloudhsm",
    "cloudtrail",
    "cloudwatch",
    "codeartifact",
    "codebuild",
    "codecommit",
    "codedeploy",
    "codeguru-profiler",
    "codeguru-reviewer",
    "codepipeline",
    "codestar-connections",
    "cognito-identity",
    "cognito-idp",
    "comprehend",
    "comprehendmedical",
    "config",
    "connect",
    "databrew",
    "dataexchange",
    "datapipeline",
    "datasync",
    "dax",
    "detective",
    "devicefarm",
    "devops-guru",
    "directconnect",
    "dlm",
    "dms",
    "docdb-elastic",
    "drs",
    "ds",
    "dynamodb",
    "ebs",
    "ec2",
    "ecr",
    "ecr-public",
    "ecs",
    "eks",
    "elastic-inference",
    "elasticache",
    "elasticbeanstalk",
    "elasticfilesystem",
    "elasticloadbalancing",
    "elasticmapreduce",
    "elastictranscoder",
    "emr-containers",
    "emr-serverless",
    "es",
    "events",
    "evidently",
    "finspace",
    "firehose",
    "fis",
    "forecast",
    "frauddetector",
    "fsx",
    "gamelift",
    "glacier",
    "globalaccelerator",
    "glue",
    "grafana",
    "greengrass",
    "groundstation",
    "guardduty",
    // AWS HealthScribe's namespace: its Domains are tagged through its own API and
    // returned by `GetResources` under this token (the behaviour this emulator is held to
    // for the `health-agent` owner in `crate::state::owners`).
    "health-agent",
    "healthlake",
    "iam",
    "imagebuilder",
    "inspector",
    "inspector2",
    "iot",
    "iotanalytics",
    "iotevents",
    "iotfleetwise",
    "iotsitewise",
    "iottwinmaker",
    "iotwireless",
    "ivs",
    "ivschat",
    "kafka",
    "kafkaconnect",
    "kendra",
    "kinesis",
    "kinesisanalytics",
    "kinesisvideo",
    "kms",
    "lakeformation",
    "lambda",
    "lex",
    "license-manager",
    "lightsail",
    "logs",
    "lookoutequipment",
    "lookoutmetrics",
    "lookoutvision",
    "m2",
    "macie2",
    "managedblockchain",
    "mediaconnect",
    "mediaconvert",
    "medialive",
    "mediapackage",
    "mediapackage-vod",
    "mediastore",
    "mediatailor",
    "memorydb",
    "mgn",
    "mobiletargeting",
    "monitron",
    "mq",
    "network-firewall",
    "networkmanager",
    "oam",
    "omics",
    "opsworks",
    "organizations",
    "osis",
    "outposts",
    "panorama",
    "personalize",
    "pipes",
    "proton",
    "qldb",
    "quicksight",
    "ram",
    "rbin",
    "rds",
    "redshift",
    "redshift-serverless",
    "refactor-spaces",
    "rekognition",
    "resiliencehub",
    "resource-explorer-2",
    "resource-groups",
    "robomaker",
    "route53",
    "route53-recovery-control-config",
    "route53-recovery-readiness",
    "route53resolver",
    "rum",
    "s3",
    "s3-outposts",
    "sagemaker",
    "savingsplans",
    "scheduler",
    "schemas",
    "sdb",
    "secretsmanager",
    "securityhub",
    "serverlessrepo",
    "servicecatalog",
    "servicediscovery",
    "servicequotas",
    "ses",
    "shield",
    "signer",
    "simspaceweaver",
    "sms",
    "snowball",
    "sns",
    "sqs",
    "ssm",
    "ssm-contacts",
    "ssm-incidents",
    "sso",
    "states",
    "storagegateway",
    "swf",
    "synthetics",
    "textract",
    "timestream",
    "transcribe",
    "transfer",
    "translate",
    "voiceid",
    "vpc-lattice",
    "waf",
    "waf-regional",
    "wafv2",
    "wellarchitected",
    "wisdom",
    "workspaces",
    "workspaces-web",
    "xray",
];

/// Whether `service` is a namespace a `ResourceTypeFilters` entry may name.
///
/// Compared byte for byte: the token has to be the namespace as an ARN spells
/// it, so a differently cased spelling of a real service is as unsupported as a
/// misspelling of one.
pub fn is_supported_service(service: &str) -> bool {
    SUPPORTED_SERVICES.contains(&service)
}

/// The service half of a `service[:resourceType]` filter entry: everything
/// before the first `:`, or the whole entry when it carries none (a bare service
/// token, which selects every resource of that service).
pub fn filter_service_token(filter: &str) -> &str {
    filter.split_once(':').map_or(filter, |(service, _)| service)
}

/// The `InvalidParameterException` message for a filter entry whose service
/// token is not a supported service.
///
/// Two variable parts, both taken from the request: the service token that was
/// not recognised, and the whole filter entry it was read out of, echoed as the
/// caller wrote it.
pub fn unsupported_service_message(service: &str, filter: &str) -> String {
    format!("Unsupported service={service} in ResourceTypeFilter - {filter}")
}

/// The refusal message for the FIRST entry of `filters` naming an unsupported
/// service, or `None` when every entry names a supported one.
///
/// Every entry is checked, in the order the request listed them, so one bad
/// entry alongside good ones refuses the call and the message names that entry.
pub fn unsupported_service_refusal(filters: &[String]) -> Option<String> {
    filters.iter().find_map(|filter| {
        let service = filter_service_token(filter);
        (!is_supported_service(service))
            .then(|| unsupported_service_message(service, filter))
    })
}
