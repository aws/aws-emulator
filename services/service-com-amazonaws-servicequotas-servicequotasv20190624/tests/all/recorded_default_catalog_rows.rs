// SPDX-License-Identifier: CC0-1.0

//! What the default-quota catalog publishes for a service whose rows AWS keys on its
//! OWN `QuotaCode`, over the wire.
//!
//! `QuotaCode` is the only key Service Quotas exposes — `GetAWSDefaultServiceQuota`,
//! `GetServiceQuota` and `RequestServiceQuotaIncrease` all take it — so the rules
//! asserted here are about the identity and the content of a published row rather than
//! about one recorded listing:
//!
//! * a published row is keyed on the `L-XXXXXXXX` code AWS mints for that quota and
//!   reports the `QuotaName`, `Value`, `Unit`, `Adjustable`, `GlobalQuota`,
//!   `Description`, `Period` and `UsageMetric` AWS publishes UNDER THAT CODE,
//!   identically through the listing and through the single read, with a `QuotaArn`
//!   naming the Region it was read at and no account;
//! * a code built out of a quota's NAME is published nowhere and resolves nowhere, and
//!   neither does a code in AWS's namespace this catalog cannot cite a quota for — so a
//!   client never reads back a quota this emulator invented;
//! * a quota AWS publishes as a quota OF A RESOURCE reports the whole `QuotaContext` it
//!   publishes — the plane the quota is counted on, that plane's resource type and the
//!   level an increase may be asked for at — and its value resolves on that plane as
//!   well as account-wide.
//!
//! The tables span rate quotas published with no window, category rate quotas published
//! over a one-second window with an `AWS/Usage` call count — on one user pool's plane and
//! on the account's — resource planes metered with an `AWS/Usage` resource count over the
//! five-minute window those counts are published over, plain counts, a quota measured in
//! megabytes rather than counted, adjustable and fixed rows, and values from 1 to 1000 —
//! and the last table drives the same rules through two services this suite's issue never
//! cited.
//!
//! The harness (`client`, `emulator`, `default_quotas`, `walk_pages`) is the one
//! `quota_workflow.rs` owns, and "is this a code AWS mints" is asked through
//! `catalog_identity.rs`'s single copy of that question.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::catalog_identity::is_aws_minted_shape;
use crate::quota_workflow::{client, default_quotas as defaults, emulator, walk_pages, Client};

/// The `UsageMetric` a published row carries, where it carries one.
enum Metric {
    /// No usage metric at all.
    None,
    /// The `AWS/Usage` `CallCount` of the API calls a rate quota throttles, under the
    /// `Service` dimension that count is published with.
    Calls {
        usage_service: &'static str,
        resource: &'static str,
    },
    /// The `AWS/Usage` `ResourceCount` of a counted plane, under the `Service` dimension
    /// and the `Resource` series that count is published with.
    Resources {
        usage_service: &'static str,
        resource: &'static str,
    },
}

/// The `QuotaContext` a published row carries: the plane the quota is counted on.
///
/// `ContextScope` is the plane a record that names no resource answers for, which is the
/// RESOURCE for a quota AWS publishes as a quota of one and the ACCOUNT for a quota whose
/// value is managed account-wide; `ContextScopeType` is the resource type that plane
/// applies to, and `AdjustableAtLevel` the level an increase may be asked for at.
struct Context {
    scope: &'static str,
    scope_type: &'static str,
    adjustable_at_level: &'static str,
}

/// One published row, with every member AWS publishes for it.
pub(crate) struct Row {
    service_code: &'static str,
    service_name: &'static str,
    quota_code: &'static str,
    quota_name: &'static str,
    value: f64,
    unit: &'static str,
    adjustable: bool,
    description: &'static str,
    /// `(PeriodValue, PeriodUnit)` — the window the quota is published over, for a row
    /// published over one.
    period: Option<(i32, &'static str)>,
    metric: Metric,
    /// The `QuotaContext` every record of the quota reports, for a quota AWS applies to
    /// an individual resource; `None` for a quota managed at the account level only,
    /// which reports none.
    context: Option<Context>,
}

/// The rows the eight service codes below publish.
///
/// SOURCE: AWS's published Service Quotas data for each `ServiceCode` (what
/// `aws service-quotas list-aws-default-service-quotas --service-code <code>` reports),
/// and the recorded default-quota listing of each code for WHICH `QuotaCode` names which
/// quota, for the window and usage metric a metered row carries, and for the wording of
/// the sentences a service publishes for a whole class of rows (the Cognito category
/// rates and the identity-pool API rates). A quota of one of these services whose
/// published name or value no source available here states is NOT published at all: a
/// code identifies one quota, so a row under a guessed binding would resolve the wrong
/// record.
const ROWS: &[Row] = &[
    // Amazon Cognito Federated Identities: one request-rate quota per identity-pool API
    // operation, published with the rate named in its own text and no window block.
    Row {
        service_code: "cognito-identity",
        service_name: "Amazon Cognito Federated Identities",
        quota_code: "L-0BF99E8D",
        quota_name: "Rate of GetOpenIdTokenForDeveloperIdentity requests",
        value: 50.0,
        unit: "None",
        adjustable: true,
        description: "The maximum number of GetOpenIdTokenForDeveloperIdentity requests that you \
                      can make per second.",
        period: None,
        metric: Metric::None,
        context: None,
    },
    Row {
        service_code: "cognito-identity",
        service_name: "Amazon Cognito Federated Identities",
        quota_code: "L-0D31D6D2",
        quota_name: "Rate of UntagResource requests",
        value: 5.0,
        unit: "None",
        adjustable: true,
        description: "The maximum number of UntagResource requests that you can make per second.",
        period: None,
        metric: Metric::None,
        context: None,
    },
    Row {
        service_code: "cognito-identity",
        service_name: "Amazon Cognito Federated Identities",
        quota_code: "L-14403AD5",
        quota_name: "User pool providers per identity pool",
        value: 50.0,
        unit: "None",
        adjustable: true,
        description: "The maximum number of Amazon Cognito user pool providers per identity pool.",
        period: None,
        metric: Metric::None,
        context: None,
    },
    Row {
        service_code: "cognito-identity",
        service_name: "Amazon Cognito Federated Identities",
        quota_code: "L-50C9AAB8",
        quota_name: "Rate of ListTagsForResource requests",
        value: 10.0,
        unit: "None",
        adjustable: true,
        description: "The maximum number of ListTagsForResource requests that you can make per \
                      second.",
        period: None,
        metric: Metric::None,
        context: None,
    },
    Row {
        service_code: "cognito-identity",
        service_name: "Amazon Cognito Federated Identities",
        quota_code: "L-5AE9F13D",
        quota_name: "Rate of GetCredentialsForIdentity requests",
        value: 200.0,
        unit: "None",
        adjustable: true,
        description: "The maximum number of GetCredentialsForIdentity requests that you can make \
                      per second.",
        period: None,
        metric: Metric::None,
        context: None,
    },
    Row {
        service_code: "cognito-identity",
        service_name: "Amazon Cognito Federated Identities",
        quota_code: "L-A38ADA84",
        quota_name: "Rate of GetId requests",
        value: 25.0,
        unit: "None",
        adjustable: true,
        description: "The maximum number of GetId requests that you can make per second.",
        period: None,
        metric: Metric::None,
        context: None,
    },
    Row {
        service_code: "cognito-identity",
        service_name: "Amazon Cognito Federated Identities",
        quota_code: "L-DAF3EAFF",
        quota_name: "Rate of TagResource requests",
        value: 5.0,
        unit: "None",
        adjustable: true,
        description: "The maximum number of TagResource requests that you can make per second.",
        period: None,
        metric: Metric::None,
        context: None,
    },
    Row {
        service_code: "cognito-identity",
        service_name: "Amazon Cognito Federated Identities",
        quota_code: "L-DBF99350",
        quota_name: "Rate of ListIdentities requests",
        value: 5.0,
        unit: "None",
        adjustable: true,
        description: "The maximum number of ListIdentities requests that you can make per second.",
        period: None,
        metric: Metric::None,
        context: None,
    },
    Row {
        service_code: "cognito-identity",
        service_name: "Amazon Cognito Federated Identities",
        quota_code: "L-E97D135E",
        quota_name: "Rate of GetOpenIdToken requests",
        value: 200.0,
        unit: "None",
        adjustable: true,
        description: "The maximum number of GetOpenIdToken requests that you can make per \
                      second.",
        period: None,
        metric: Metric::None,
        context: None,
    },
    // Beside those rates the service publishes the COUNTS and the SIZES of an identity
    // pool and of the account that holds it: an adjustable count, a fixed count, a page
    // size and two sizes measured in bytes rather than counted.
    Row {
        service_code: "cognito-identity",
        service_name: "Amazon Cognito Federated Identities",
        quota_code: "L-1F1BC060",
        quota_name: "Role-based access control rules",
        value: 25.0,
        unit: "None",
        adjustable: false,
        description: "The maximum number of rules for role-based access control (RBAC)",
        period: None,
        metric: Metric::None,
        context: None,
    },
    Row {
        service_code: "cognito-identity",
        service_name: "Amazon Cognito Federated Identities",
        quota_code: "L-29CA570C",
        quota_name: "Identity pool name size",
        value: 128.0,
        unit: "Bytes",
        adjustable: false,
        description: "The maximum size of an identity pool name in bytes.",
        period: None,
        metric: Metric::None,
        context: None,
    },
    Row {
        service_code: "cognito-identity",
        service_name: "Amazon Cognito Federated Identities",
        quota_code: "L-8692CE1C",
        quota_name: "Identity pools per account",
        value: 1000.0,
        unit: "None",
        adjustable: true,
        description: "The maximum number of identity pools per account.",
        period: None,
        metric: Metric::None,
        context: None,
    },
    Row {
        service_code: "cognito-identity",
        service_name: "Amazon Cognito Federated Identities",
        quota_code: "L-B08F14B9",
        quota_name: "Login provider name size",
        value: 2048.0,
        unit: "Bytes",
        adjustable: false,
        description: "The maximum size of a login provider name in bytes.",
        period: None,
        metric: Metric::None,
        context: None,
    },
    Row {
        service_code: "cognito-identity",
        service_name: "Amazon Cognito Federated Identities",
        quota_code: "L-EAF710D2",
        quota_name: "List API call results",
        value: 60.0,
        unit: "None",
        adjustable: false,
        description: "The maximum number of results from a list or lookup API call.",
        period: None,
        metric: Metric::None,
        context: None,
    },
    // Amazon Cognito User Pools throttles its API by CATEGORY: a one-second window, the
    // category's own `AWS/Usage` call count, and the sentence the service publishes for
    // the class — the provisionable wording for an adjustable category, the shared-quota
    // wording for a fixed one.
    Row {
        service_code: "cognito-idp",
        service_name: "Amazon Cognito User Pools",
        quota_code: "L-12C4D74A",
        quota_name: "Rate of UserPoolClientUpdate requests per account",
        value: 15.0,
        unit: "None",
        adjustable: false,
        description: "The maximum combined call rate (requests per second) for all API operations \
                      in the UserPoolClientUpdate category. All operations within a category \
                      share the same quota. For the list of included operations, see \
                      https://docs.aws.amazon.com/cognito/latest/developerguide/\
                      limits.html#category_operations.",
        period: Some((1, "SECOND")),
        metric: Metric::Calls {
            usage_service: "Cognito User Pool",
            resource: "UserPoolClientUpdate",
        },
        context: None,
    },
    Row {
        service_code: "cognito-idp",
        service_name: "Amazon Cognito User Pools",
        quota_code: "L-60A0B411",
        quota_name: "Rate of UserPoolUpdate requests",
        value: 15.0,
        unit: "None",
        adjustable: false,
        description: "The maximum combined call rate (requests per second) for all API operations \
                      in the UserPoolUpdate category. All operations within a category \
                      share the same quota. For the list of included operations, see \
                      https://docs.aws.amazon.com/cognito/latest/developerguide/\
                      limits.html#category_operations.",
        period: Some((1, "SECOND")),
        metric: Metric::Calls {
            usage_service: "Cognito User Pool",
            resource: "UserPoolUpdate",
        },
        context: None,
    },
    Row {
        service_code: "cognito-idp",
        service_name: "Amazon Cognito User Pools",
        quota_code: "L-5987B8A0",
        quota_name: "Rate of UserCreation requests",
        value: 50.0,
        unit: "None",
        adjustable: true,
        description: "The maximum combined call rate (requests per second) you can request for \
                      all API operations in the UserCreation category. Provision your rate limit \
                      within this quota from the Amazon Cognito console or APIs. For the list of \
                      included operations, see https://docs.aws.amazon.com/cognito/latest/\
                      developerguide/limits.html#category_operations.",
        period: Some((1, "SECOND")),
        metric: Metric::Calls {
            usage_service: "Cognito User Pool",
            resource: "UserCreation",
        },
        context: None,
    },
    Row {
        service_code: "cognito-idp",
        service_name: "Amazon Cognito User Pools",
        quota_code: "L-574C86AE",
        quota_name: "Rate of UserResourceUpdate requests",
        value: 25.0,
        unit: "None",
        adjustable: false,
        description: "The maximum combined call rate (requests per second) for all API operations \
                      in the UserResourceUpdate category. All operations within a category \
                      share the same quota. For the list of included operations, see \
                      https://docs.aws.amazon.com/cognito/latest/developerguide/\
                      limits.html#category_operations.",
        period: Some((1, "SECOND")),
        metric: Metric::Calls {
            usage_service: "Cognito User Pool",
            resource: "UserResourceUpdate",
        },
        context: None,
    },
    // The service's RESOURCE COUNTS: a count of the account's user pools and a count of
    // one pool's resource servers, neither of them a rate, so neither carries the
    // category window or the category call count. Each carries the sentence the service
    // publishes for that count — the quota's own text rather than the category sentence.
    Row {
        service_code: "cognito-idp",
        service_name: "Amazon Cognito User Pools",
        quota_code: "L-66E6DF30",
        quota_name: "User pools per account",
        value: 1000.0,
        unit: "None",
        adjustable: true,
        description: "The maximum number of user pools that you can create in this account per \
                      region.",
        period: None,
        metric: Metric::None,
        context: None,
    },
    Row {
        service_code: "cognito-idp",
        service_name: "Amazon Cognito User Pools",
        quota_code: "L-7CDAF993",
        quota_name: "Resource servers per user pool",
        value: 25.0,
        unit: "None",
        adjustable: true,
        description: "The maximum number of resource servers per user pool. A resource server is \
                      a server for access-protected resources.",
        period: None,
        metric: Metric::None,
        context: None,
    },
    Row {
        service_code: "cognito-idp",
        service_name: "Amazon Cognito User Pools",
        quota_code: "L-7D6E8ED3",
        quota_name: "Rate of UserAccountRecovery requests",
        value: 30.0,
        unit: "None",
        adjustable: false,
        description: "The maximum combined call rate (requests per second) for all API operations \
                      in the UserAccountRecovery category. All operations within a category \
                      share the same quota. For the list of included operations, see \
                      https://docs.aws.amazon.com/cognito/latest/developerguide/\
                      limits.html#category_operations.",
        period: Some((1, "SECOND")),
        metric: Metric::Calls {
            usage_service: "Cognito User Pool",
            resource: "UserAccountRecovery",
        },
        context: None,
    },
    // A category whose rate AWS serves for the WHOLE ACCOUNT rather than for one user
    // pool, which is the plane its own name states.
    Row {
        service_code: "cognito-idp",
        service_name: "Amazon Cognito User Pools",
        quota_code: "L-B7575496",
        quota_name: "Rate of UserPoolResourceUpdate requests per account",
        value: 15.0,
        unit: "None",
        adjustable: false,
        description: "The maximum combined call rate (requests per second) for all API operations \
                      in the UserPoolResourceUpdate category. All operations within a category \
                      share the same quota. For the list of included operations, see \
                      https://docs.aws.amazon.com/cognito/latest/developerguide/\
                      limits.html#category_operations.",
        period: Some((1, "SECOND")),
        metric: Metric::Calls {
            usage_service: "Cognito User Pool",
            resource: "UserPoolResourceUpdate",
        },
        context: None,
    },
    Row {
        service_code: "cognito-idp",
        service_name: "Amazon Cognito User Pools",
        quota_code: "L-D6BD5178",
        quota_name: "Rate of UserRead requests",
        value: 120.0,
        unit: "None",
        adjustable: true,
        description: "The maximum combined call rate (requests per second) you can request for \
                      all API operations in the UserRead category. Provision your rate limit \
                      within this quota from the Amazon Cognito console or APIs. For the list of \
                      included operations, see https://docs.aws.amazon.com/cognito/latest/\
                      developerguide/limits.html#category_operations.",
        period: Some((1, "SECOND")),
        metric: Metric::Calls {
            usage_service: "Cognito User Pool",
            resource: "UserRead",
        },
        context: None,
    },
    // Amazon Comprehend publishes a per-operation throttle in transactions per second,
    // and the number of jobs one operation may have running.
    Row {
        service_code: "comprehend",
        service_name: "Amazon Comprehend",
        quota_code: "L-1F3990DA",
        quota_name: "DetectDominantLanguage throttle limit in transactions per second",
        value: 50.0,
        unit: "None",
        adjustable: true,
        description: "The maximum number of DetectDominantLanguage requests allowed per account \
                      per second, in the current Region",
        period: None,
        metric: Metric::None,
        context: None,
    },
    Row {
        service_code: "comprehend",
        service_name: "Amazon Comprehend",
        quota_code: "L-2B8ECCAB",
        quota_name: "StartEntitiesDetectionJob max active jobs",
        value: 10.0,
        unit: "None",
        adjustable: true,
        description: "The maximum number of active jobs for API StartEntitiesDetectionJob \
                      allowed per account, in the current Region",
        period: None,
        metric: Metric::None,
        context: None,
    },
    Row {
        service_code: "comprehend",
        service_name: "Amazon Comprehend",
        quota_code: "L-9A845B3A",
        quota_name: "TagResource throttle limit in transactions per second",
        value: 1.0,
        unit: "None",
        adjustable: false,
        description: "The maximum number of TagResource requests allowed per account per second, \
                      in the current Region",
        period: None,
        metric: Metric::None,
        context: None,
    },
    Row {
        service_code: "comprehend",
        service_name: "Amazon Comprehend",
        quota_code: "L-E7F70588",
        quota_name: "DeleteEntityRecognizer throttle limit in transactions per second",
        value: 1.0,
        unit: "None",
        adjustable: false,
        description: "The maximum number of DeleteEntityRecognizer requests allowed per account \
                      per second, in the current Region",
        period: None,
        metric: Metric::None,
        context: None,
    },
    Row {
        service_code: "comprehend",
        service_name: "Amazon Comprehend",
        quota_code: "L-F039F1E3",
        quota_name: "DescribeEntityRecognizer throttle limit in transactions per second",
        value: 10.0,
        unit: "None",
        adjustable: false,
        description: "The maximum number of DescribeEntityRecognizer requests allowed per account \
                      per second, in the current Region",
        period: None,
        metric: Metric::None,
        context: None,
    },
    Row {
        service_code: "comprehend",
        service_name: "Amazon Comprehend",
        quota_code: "L-C7E12CA2",
        quota_name: "StopKeyPhrasesDetectionJob throttle limit in transactions per second",
        value: 1.0,
        unit: "None",
        adjustable: true,
        description: "The maximum number of StopKeyPhrasesDetectionJob requests allowed per \
                      account per second, in the current Region",
        period: None,
        metric: Metric::None,
        context: None,
    },
    Row {
        service_code: "comprehend",
        service_name: "Amazon Comprehend",
        quota_code: "L-CF92EEE2",
        quota_name: "StopTrainingEntityRecognizer throttle limit in transactions per second",
        value: 1.0,
        unit: "None",
        adjustable: false,
        description: "The maximum number of StopTrainingEntityRecognizer requests allowed per \
                      account per second, in the current Region",
        period: None,
        metric: Metric::None,
        context: None,
    },
    // Beside the throttles, the number of RESOURCES of a kind the account may have active
    // at once — neither a rate nor a job count, and the sentence the service publishes for
    // each is one this catalog cannot cite, so the rows publish no `Description` at all.
    Row {
        service_code: "comprehend",
        service_name: "Amazon Comprehend",
        quota_code: "L-55642075",
        quota_name: "Endpoints max active endpoints",
        value: 20.0,
        unit: "None",
        adjustable: true,
        description: "",
        period: None,
        metric: Metric::None,
        context: None,
    },
    Row {
        service_code: "comprehend",
        service_name: "Amazon Comprehend",
        quota_code: "L-AE5B911F",
        quota_name: "CreateFlywheel max active flywheels",
        value: 20.0,
        unit: "None",
        adjustable: true,
        description: "",
        period: None,
        metric: Metric::None,
        context: None,
    },
    // Amazon DynamoDB Accelerator: the node and subnet planes of the Region beside the
    // planes counted inside one cluster or subnet group.
    Row {
        service_code: "dax",
        service_name: "Amazon DynamoDB Accelerator (DAX)",
        quota_code: "L-315AFD08",
        quota_name: "Parameter groups",
        value: 20.0,
        unit: "None",
        adjustable: false,
        description: "The maximum number of parameter groups in a single AWS region.",
        period: None,
        metric: Metric::None,
        context: None,
    },
    Row {
        service_code: "dax",
        service_name: "Amazon DynamoDB Accelerator (DAX)",
        quota_code: "L-87AEEBB5",
        quota_name: "Nodes per cluster",
        value: 11.0,
        unit: "None",
        adjustable: false,
        description: "The maximum number of nodes per cluster, including the primary node as well \
                      as any read replica nodes.",
        period: None,
        metric: Metric::None,
        context: None,
    },
    Row {
        service_code: "dax",
        service_name: "Amazon DynamoDB Accelerator (DAX)",
        quota_code: "L-AB139030",
        quota_name: "Total number of nodes",
        value: 50.0,
        unit: "None",
        adjustable: true,
        description: "The maximum total number of nodes per AWS account in a single AWS region.",
        period: None,
        metric: Metric::None,
        context: None,
    },
    Row {
        service_code: "dax",
        service_name: "Amazon DynamoDB Accelerator (DAX)",
        quota_code: "L-E34C284B",
        quota_name: "Subnets per subnet group",
        value: 20.0,
        unit: "None",
        adjustable: false,
        description: "The maximum number of subnets per subnet group in a single AWS region.",
        period: None,
        metric: Metric::None,
        context: None,
    },
    Row {
        service_code: "dax",
        service_name: "Amazon DynamoDB Accelerator (DAX)",
        quota_code: "L-F55BD408",
        quota_name: "Subnet groups",
        value: 50.0,
        unit: "None",
        adjustable: false,
        description: "The maximum number of subnet groups in a single AWS region.",
        period: None,
        metric: Metric::None,
        context: None,
    },
    // AWS Direct Connect: a plain per-connection count beside the planes AWS applies to
    // an individual resource — a dedicated connection, a LAG, a transit gateway and a
    // Direct Connect gateway — the metered ones counted with the service's own
    // `AWS/Usage` resource count.
    Row {
        service_code: "directconnect",
        service_name: "AWS Direct Connect",
        quota_code: "L-0B7EBFD7",
        quota_name: "Public or private virtual interfaces per dedicated connection",
        value: 50.0,
        unit: "None",
        adjustable: false,
        description: "",
        period: Some((5, "MINUTE")),
        metric: Metric::Resources {
            usage_service: "Direct Connect",
            resource: "PublicOrPrivateVIFsPerDedicatedConnection",
        },
        context: Some(Context {
            scope: "RESOURCE",
            scope_type: "AWS::DirectConnect::Connections",
            adjustable_at_level: "ALL",
        }),
    },
    Row {
        service_code: "directconnect",
        service_name: "AWS Direct Connect",
        quota_code: "L-A1004C93",
        quota_name: "Prefixes per transit gateway",
        value: 200.0,
        unit: "None",
        adjustable: false,
        description: "",
        period: None,
        metric: Metric::None,
        context: Some(Context {
            scope: "RESOURCE",
            scope_type: "AWS::EC2::TransitGateways",
            adjustable_at_level: "ALL",
        }),
    },
    Row {
        service_code: "directconnect",
        service_name: "AWS Direct Connect",
        quota_code: "L-3745876E",
        quota_name: "Private or public or transit virtual interfaces per hosted connection",
        value: 1.0,
        unit: "None",
        adjustable: false,
        description: "The maximum number of private, public, or transit virtual interfaces per \
                      AWS Direct Connect hosted connection.",
        period: None,
        metric: Metric::None,
        context: None,
    },
    Row {
        service_code: "directconnect",
        service_name: "AWS Direct Connect",
        quota_code: "L-5A971579",
        quota_name: "Members per LAG",
        value: 4.0,
        unit: "None",
        adjustable: false,
        description: "The maximum number of dedicated connections or interconnects per link \
                      aggregation group (LAG).",
        period: Some((5, "MINUTE")),
        metric: Metric::Resources {
            usage_service: "Direct Connect",
            resource: "MembersPerLag",
        },
        context: Some(Context {
            scope: "RESOURCE",
            scope_type: "AWS::DirectConnect::Lags",
            adjustable_at_level: "ALL",
        }),
    },
    Row {
        service_code: "directconnect",
        service_name: "AWS Direct Connect",
        quota_code: "L-FDE06981",
        quota_name: "Virtual private gateways per Direct Connect gateway",
        value: 20.0,
        unit: "None",
        adjustable: false,
        description: "The maximum number of virtual private gateways per AWS Direct Connect \
                      gateway.",
        period: Some((5, "MINUTE")),
        metric: Metric::Resources {
            usage_service: "Direct Connect",
            resource: "VirtualPrivateGatewaysPerDirectConnectGateway",
        },
        context: Some(Context {
            scope: "RESOURCE",
            scope_type: "AWS::DirectConnect::Gateways",
            adjustable_at_level: "ALL",
        }),
    },
    // AWS Database Migration Service: the endpoint, replication and subscription planes
    // of the Region beside the endpoint plane of one replication instance, every row of
    // them reporting the display name the inventory gives the service code.
    Row {
        service_code: "dms",
        service_name: "AWS Database Migration Service (AWS DMS)",
        quota_code: "L-2146F1FD",
        quota_name: "Endpoints per instance",
        value: 100.0,
        unit: "None",
        adjustable: true,
        description: "The maximum number of endpoints allowed per replication instance.",
        period: None,
        metric: Metric::None,
        context: None,
    },
    Row {
        service_code: "dms",
        service_name: "AWS Database Migration Service (AWS DMS)",
        quota_code: "L-D97343A2",
        quota_name: "Event subscriptions",
        value: 60.0,
        unit: "None",
        adjustable: true,
        description: "The maximum number of AWS DMS events that you can subscribe to.",
        period: None,
        metric: Metric::None,
        context: None,
    },
    Row {
        service_code: "dms",
        service_name: "AWS Database Migration Service (AWS DMS)",
        quota_code: "L-E17328E9",
        quota_name: "Endpoint count",
        value: 1000.0,
        unit: "None",
        adjustable: true,
        description: "The maximum number of endpoints allowed in this account in the current \
                      Region.",
        period: None,
        metric: Metric::None,
        context: None,
    },
    Row {
        service_code: "dms",
        service_name: "AWS Database Migration Service (AWS DMS)",
        quota_code: "L-E569F59D",
        quota_name: "Serverless replications",
        value: 100.0,
        unit: "None",
        adjustable: true,
        description: "The maximum number of serverless replications allowed in this account in \
                      the current Region.",
        period: None,
        metric: Metric::None,
        context: None,
    },
    Row {
        service_code: "dms",
        service_name: "AWS Database Migration Service (AWS DMS)",
        quota_code: "L-FBEA20FB",
        quota_name: "Number of running Data Migrations",
        value: 500.0,
        unit: "None",
        adjustable: true,
        description: "Maximum number of running data migrations simultaneously.",
        period: None,
        metric: Metric::None,
        context: None,
    },
    Row {
        service_code: "dms",
        service_name: "AWS Database Migration Service (AWS DMS)",
        quota_code: "L-1AB41EE0",
        quota_name: "Number of Data Migrations",
        value: 1000.0,
        unit: "None",
        adjustable: true,
        description: "Maximum number of data migrations",
        period: None,
        metric: Metric::None,
        context: None,
    },
    Row {
        service_code: "dms",
        service_name: "AWS Database Migration Service (AWS DMS)",
        quota_code: "L-62EFB27A",
        quota_name: "Number of Data Migrations per Migration Project",
        value: 10.0,
        unit: "None",
        adjustable: false,
        description: "",
        period: None,
        metric: Metric::None,
        context: None,
    },
    Row {
        service_code: "dms",
        service_name: "AWS Database Migration Service (AWS DMS)",
        quota_code: "L-DE63148C",
        quota_name: "Number of Migration Projects",
        value: 50.0,
        unit: "None",
        adjustable: true,
        description: "",
        period: None,
        metric: Metric::None,
        context: None,
    },
    Row {
        service_code: "dms",
        service_name: "AWS Database Migration Service (AWS DMS)",
        quota_code: "L-FE918D88",
        quota_name: "Certificate count",
        value: 100.0,
        unit: "None",
        adjustable: true,
        description: "The maximum number of certificates allowed in this account in the current \
                      Region.",
        period: None,
        metric: Metric::None,
        context: None,
    },
    // Amazon DocumentDB: the cluster, replica, snapshot, subnet-group and subscription
    // planes of the Region beside the security-group plane of a VPC, which AWS publishes
    // as fixed, each described in the terse form this service words its descriptions in.
    Row {
        service_code: "docdb",
        service_name: "Amazon DocumentDB (with MongoDB compatibility)",
        quota_code: "L-02DEA053",
        quota_name: "Subnet groups",
        value: 50.0,
        unit: "None",
        adjustable: true,
        description: "The maximum number of DB subnet groups",
        period: None,
        metric: Metric::None,
        context: None,
    },
    Row {
        service_code: "docdb",
        service_name: "Amazon DocumentDB (with MongoDB compatibility)",
        quota_code: "L-13F31459",
        quota_name: "Clusters",
        value: 40.0,
        unit: "None",
        adjustable: true,
        description: "The maximum number of clusters allowed in this account in the current \
                      Region",
        period: None,
        metric: Metric::None,
        context: None,
    },
    Row {
        service_code: "docdb",
        service_name: "Amazon DocumentDB (with MongoDB compatibility)",
        quota_code: "L-5BA57179",
        quota_name: "Read replicas per cluster",
        value: 15.0,
        unit: "None",
        adjustable: true,
        description: "The maximum number of read replicas per cluster",
        period: None,
        metric: Metric::None,
        context: None,
    },
    Row {
        service_code: "docdb",
        service_name: "Amazon DocumentDB (with MongoDB compatibility)",
        quota_code: "L-B2551F83",
        quota_name: "Manual cluster snapshots",
        value: 100.0,
        unit: "None",
        adjustable: true,
        description: "The maximum number of manual snapshots",
        period: None,
        metric: Metric::None,
        context: None,
    },
    Row {
        service_code: "docdb",
        service_name: "Amazon DocumentDB (with MongoDB compatibility)",
        quota_code: "L-D02D85EA",
        quota_name: "VPC security groups per instance",
        value: 5.0,
        unit: "None",
        adjustable: false,
        description: "The maximum number of DB security groups per Amazon VPC",
        period: None,
        metric: Metric::None,
        context: None,
    },
    Row {
        service_code: "docdb",
        service_name: "Amazon DocumentDB (with MongoDB compatibility)",
        quota_code: "L-F7FABF71",
        quota_name: "Event subscriptions",
        value: 20.0,
        unit: "None",
        adjustable: true,
        description: "The maximum number of event subscriptions",
        period: None,
        metric: Metric::None,
        context: None,
    },
    Row {
        service_code: "docdb",
        service_name: "Amazon DocumentDB (with MongoDB compatibility)",
        quota_code: "L-3B0E1499",
        quota_name: "Tags per resource",
        value: 50.0,
        unit: "None",
        adjustable: false,
        description: "The maximum number of tags per Amazon RDS resource",
        period: None,
        metric: Metric::None,
        context: None,
    },
    Row {
        service_code: "docdb",
        service_name: "Amazon DocumentDB (with MongoDB compatibility)",
        quota_code: "L-2A542E16",
        quota_name: "Cluster parameter groups",
        value: 50.0,
        unit: "None",
        adjustable: false,
        description: "The maximum number of DB cluster parameter groups",
        period: None,
        metric: Metric::None,
        context: None,
    },
    Row {
        service_code: "docdb",
        service_name: "Amazon DocumentDB (with MongoDB compatibility)",
        quota_code: "L-36C7F3F8",
        quota_name: "Subnets per subnet group",
        value: 20.0,
        unit: "None",
        adjustable: false,
        description: "The maximum number of subnets per DB subnet group",
        period: None,
        metric: Metric::None,
        context: None,
    },
    Row {
        service_code: "docdb",
        service_name: "Amazon DocumentDB (with MongoDB compatibility)",
        quota_code: "L-739A3A85",
        quota_name: "Instances",
        value: 40.0,
        unit: "None",
        adjustable: true,
        description: "The maximum number of DB instances allowed in this account in the current \
                      Region",
        period: None,
        metric: Metric::None,
        context: None,
    },
    // Amazon QuickSight: the planes of a space and of the account beside the retention,
    // size and nesting limits of its agent features.
    Row {
        service_code: "quicksight",
        service_name: "Amazon QuickSight",
        quota_code: "L-12A1F6CA",
        quota_name: "Amazon Quick Automate Maximum Nesting Depth",
        value: 6.0,
        unit: "None",
        adjustable: false,
        description: "Maximum nesting depth allowed in an automation in Amazon Quick Automate",
        period: None,
        metric: Metric::None,
        context: None,
    },
    // A size rather than a count, so the row carries the unit AWS measures it in.
    Row {
        service_code: "quicksight",
        service_name: "Amazon QuickSight",
        quota_code: "L-26532294",
        quota_name: "Maximum total size of attached reference documents per chat agent",
        value: 50.0,
        unit: "Megabytes",
        adjustable: false,
        description: "The maximum size in megabytes (MBs) of all reference documents attached to \
                      a chat agent.",
        period: None,
        metric: Metric::None,
        context: None,
    },
    Row {
        service_code: "quicksight",
        service_name: "Amazon QuickSight",
        quota_code: "L-39A09B2E",
        quota_name: "Maximum number of resources per space",
        value: 100.0,
        unit: "None",
        adjustable: false,
        description: "The maximum number of resources that can be linked to a space.",
        period: None,
        metric: Metric::None,
        context: None,
    },
    Row {
        service_code: "quicksight",
        service_name: "Amazon QuickSight",
        quota_code: "L-7DB5CFE0",
        quota_name: "Scheduled task history retention in days",
        value: 365.0,
        unit: "None",
        adjustable: false,
        description: "The number of days scheduled task run history is retained.",
        period: None,
        metric: Metric::None,
        context: None,
    },
    Row {
        service_code: "quicksight",
        service_name: "Amazon QuickSight",
        quota_code: "L-90F283DF",
        quota_name: "Custom agent artifact retention in days",
        value: 365.0,
        unit: "None",
        adjustable: false,
        description: "The number of days custom agent artifacts are retained before deletion.",
        period: None,
        metric: Metric::None,
        context: None,
    },
    Row {
        service_code: "quicksight",
        service_name: "Amazon QuickSight",
        quota_code: "L-D75C2D48",
        quota_name: "Maximum number of approval policies per account",
        value: 500.0,
        unit: "None",
        adjustable: true,
        description: "The maximum number of approval policies that can be created in a single \
                      account across all asset types.",
        period: None,
        metric: Metric::None,
        context: None,
    },
    Row {
        service_code: "quicksight",
        service_name: "Amazon QuickSight",
        quota_code: "L-3B893AF4",
        quota_name: "Session approval expiry in hours",
        value: 12.0,
        unit: "None",
        adjustable: false,
        description: "",
        period: None,
        metric: Metric::None,
        context: None,
    },
    Row {
        service_code: "quicksight",
        service_name: "Amazon QuickSight",
        quota_code: "L-5FE00BED",
        quota_name: "Amazon Quick Automate Maximum Automations",
        value: 1000.0,
        unit: "None",
        adjustable: true,
        description: "",
        period: None,
        metric: Metric::None,
        context: None,
    },
    // A size measured in gigabytes rather than counted.
    Row {
        service_code: "quicksight",
        service_name: "Amazon QuickSight",
        quota_code: "L-8926DD84",
        quota_name: "Maximum custom agent files size",
        value: 1.0,
        unit: "Gigabytes",
        adjustable: true,
        description: "",
        period: None,
        metric: Metric::None,
        context: None,
    },
];

/// Rows of the SAME class published by two service codes this suite's issue never cited,
/// so the rules above are asserted beyond the reported instances: a re-keyed resource
/// plane, and the quota whose value AWS applies to an individual resource as well as
/// account-wide.
///
/// SOURCE: AWS's published Service Quotas data for the `ram` and `es` service codes, as
/// `src/state/quota_rows.rs` carries them.
const UNCITED_ROWS: &[Row] = &[
    Row {
        service_code: "ram",
        service_name: "AWS Resource Access Manager",
        quota_code: "L-595828F9",
        quota_name: "Number of resource shares",
        value: 25_000.0,
        unit: "None",
        adjustable: true,
        description: "The maximum number of resource shares.",
        period: None,
        metric: Metric::None,
        context: None,
    },
    Row {
        service_code: "es",
        service_name: "Amazon OpenSearch Service",
        quota_code: "L-6408ABDE",
        quota_name: "Instances per domain",
        value: 200.0,
        unit: "None",
        adjustable: true,
        description: "The maximum number of instances in a single Amazon OpenSearch Service \
                      domain. You can request an increase up to 1002 instances per domain.",
        period: None,
        metric: Metric::None,
        context: Some(Context {
            scope: "RESOURCE",
            scope_type: "AWS::OpenSearchService::Domain",
            adjustable_at_level: "ALL",
        }),
    },
];

/// The published rows of `service_codes`, out of the one table of them this suite owns.
///
/// `pub(crate)` with [`assert_row`] so a sibling suite can drive the SAME rows through
/// another endpoint rather than keeping a second copy of the table, which could drift from
/// this one.
pub(crate) fn rows_of<'codes>(
    service_codes: &'codes [&'codes str],
) -> impl Iterator<Item = &'static Row> + 'codes {
    ROWS.iter()
        .chain(UNCITED_ROWS)
        .filter(move |row| service_codes.contains(&row.service_code))
}

/// The whole published catalog of each covered service: its `ServiceCode` and the codes
/// its rows are keyed on, so a row that silently appears or disappears fails here.
///
/// SOURCE: the `ROWS` table above, which is the set of quotas this catalog can cite for
/// these services.
fn published_codes(service_code: &str) -> BTreeSet<&'static str> {
    ROWS.iter()
        .filter(|row| row.service_code == service_code)
        .map(|row| row.quota_code)
        .collect()
}

/// The service codes `ROWS` covers, in the order they first appear.
fn covered_service_codes() -> Vec<&'static str> {
    let mut codes: Vec<&'static str> = Vec::new();
    for row in ROWS {
        if !codes.contains(&row.service_code) {
            codes.push(row.service_code);
        }
    }
    codes
}

/// Codes these services publish NOTHING under: the descriptive stand-ins their quota
/// names yield (`src/state.rs::descriptive_quota_code`), which this catalog used to key
/// their rows on.
///
/// SOURCE: the recorded `ListAWSDefaultServiceQuotas` response of each service code,
/// which reports each of these as a surplus entry.
const WITHDRAWN_CODES: &[(&str, &str)] = &[
    ("cognito-identity", "IdentityPools"),
    ("cognito-idp", "UserPoolsPerAccount"),
    ("cognito-idp", "AppClientsPerUserPool"),
    ("comprehend", "CustomClassifiersPerAccount"),
    ("dax", "NodesPerCluster"),
    ("directconnect", "ConnectionsPerRegion"),
    ("dms", "Endpoints"),
    ("dms", "ReplicationInstances"),
    ("docdb", "Clusters"),
    ("docdb", "InstancesPerCluster"),
    ("quicksight", "Datasets"),
];

/// The `UsageMetric` a row's [`Metric`] publishes, as the wire reports it.
fn metric_json(metric: &Metric) -> Value {
    match metric {
        Metric::None => Value::Null,
        Metric::Calls {
            usage_service,
            resource,
        } => json!({
            "MetricNamespace": "AWS/Usage",
            "MetricName": "CallCount",
            "MetricStatisticRecommendation": "Sum",
            "MetricDimensions": {
                "Service": usage_service,
                "Class": "None",
                "Resource": resource,
                "Type": "API",
            },
        }),
        Metric::Resources {
            usage_service,
            resource,
        } => json!({
            "MetricNamespace": "AWS/Usage",
            "MetricName": "ResourceCount",
            "MetricStatisticRecommendation": "Maximum",
            "MetricDimensions": {
                "Service": usage_service,
                "Class": "None",
                "Resource": resource,
                "Type": "Resource",
            },
        }),
    }
}

/// Every `QuotaCode` a listing of `service_code` reports.
async fn listed_codes(caller: &Client, operation: &str, service_code: &str) -> BTreeSet<String> {
    walk_pages(
        caller,
        operation,
        &json!({"ServiceCode": service_code}),
        "Quotas",
        None,
    )
    .await
    .iter()
    .filter_map(|quota| quota["QuotaCode"].as_str().map(str::to_string))
    .collect()
}

/// Assert one row reports the facts AWS publishes for it, through the single read and
/// through the listing alike.
pub(crate) async fn assert_row(caller: &Client, region: &str, row: &Row) {
    let case = format!("{}/{}", row.service_code, row.quota_code);
    let pair = json!({"ServiceCode": row.service_code, "QuotaCode": row.quota_code});
    let quota = caller.ok("GetAWSDefaultServiceQuota", pair.clone()).await["Quota"].clone();

    assert!(
        is_aws_minted_shape(row.quota_code),
        "case {case}: this suite covers the rows AWS keys on its own code, got \
         {:?}",
        row.quota_code
    );
    assert_eq!(
        (
            quota["ServiceCode"].as_str(),
            quota["ServiceName"].as_str(),
            quota["QuotaCode"].as_str()
        ),
        (
            Some(row.service_code),
            Some(row.service_name),
            Some(row.quota_code)
        ),
        "case {case}: expected the service's own code and published name, got {quota}"
    );
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
    assert_eq!(
        quota["GlobalQuota"],
        json!(false),
        "case {case}: AWS counts these quotas per Region, got {quota}"
    );
    assert_eq!(
        quota["Description"],
        if row.description.is_empty() {
            // A row whose published text this catalog cannot cite publishes no
            // `Description` member at all, rather than wording authored here.
            Value::Null
        } else {
            json!(row.description)
        },
        "case {case}: a description is the quota's own published text, got {quota}"
    );
    assert_eq!(
        quota["QuotaArn"],
        json!(format!(
            "arn:aws:servicequotas:{region}::{}/{}",
            row.service_code, row.quota_code
        )),
        "case {case}: an AWS default quota's ARN names the Region and no account, got {quota}"
    );
    match row.period {
        Some((value, unit)) => assert_eq!(
            quota["Period"],
            json!({"PeriodValue": value, "PeriodUnit": unit}),
            "case {case}: expected the window AWS publishes the quota over, got {quota}"
        ),
        None => assert!(
            quota.get("Period").is_none_or(Value::is_null),
            "case {case}: this quota is published over no window, got {quota}"
        ),
    }
    let metric = metric_json(&row.metric);
    if metric.is_null() {
        assert!(
            quota.get("UsageMetric").is_none_or(Value::is_null),
            "case {case}: this quota publishes no usage metric, got {quota}"
        );
    } else {
        assert_eq!(
            quota["UsageMetric"], metric,
            "case {case}: expected the usage metric the service publishes, got {quota}"
        );
    }

    // The listing carries exactly the record the single read returned.
    let listed = defaults(caller, row.service_code).await;
    assert!(
        listed.contains(&quota),
        "case {case}: the listing must carry the record the single read returned, got {listed:?}"
    );

    // The applied plane is keyed on the same code, so a client that read a code out of the
    // default catalog resolves its value in force too.
    let applied = caller.ok("GetServiceQuota", pair).await["Quota"].clone();
    assert_eq!(
        (
            applied["QuotaCode"].clone(),
            applied["QuotaName"].clone(),
            applied["Value"].clone()
        ),
        (
            json!(row.quota_code),
            json!(row.quota_name),
            json!(row.value)
        ),
        "case {case}: the applied read resolves the same quota at its default value, got {applied}"
    );
}

/// Every published row reports the facts AWS publishes for that quota code.
#[tokio::test]
async fn a_published_row_reports_the_facts_aws_publishes() {
    let engine = emulator();
    let region = "us-west-2";
    let caller = client(&engine, region);

    for row in ROWS.iter().chain(UNCITED_ROWS) {
        assert_row(&caller, region, row).await;
    }
}

/// Each covered service publishes exactly the rows this catalog can cite, every one of
/// them keyed on a code AWS mints and reachable by a key of its own, at two endpoints.
#[tokio::test]
async fn a_covered_service_publishes_exactly_the_rows_it_cites() {
    let engine = emulator();

    // The catalog is published per Region and the rule is about the catalog rather than
    // about one Region's view of it.
    for region in ["us-east-1", "eu-central-1"] {
        let caller = client(&engine, region);

        for service_code in covered_service_codes() {
            let case = format!("{region} {service_code}");
            let expected = published_codes(service_code);
            let quotas = defaults(&caller, service_code).await;
            let codes: BTreeSet<&str> = quotas
                .iter()
                .filter_map(|quota| quota["QuotaCode"].as_str())
                .collect();

            assert_eq!(
                codes, expected,
                "case {case}: expected exactly the cited published rows, got {codes:?}"
            );
            assert_eq!(
                codes.len(),
                quotas.len(),
                "case {case}: every published quota has a key of its own, got {codes:?}"
            );
            for quota in &quotas {
                let code = quota["QuotaCode"].as_str().unwrap_or_default();
                // Every published row carries the quota's description, except the rows
                // whose published text this catalog cannot cite, which the table above
                // names with an empty `description` and which publish no member at all.
                let citable = ROWS
                    .iter()
                    .chain(UNCITED_ROWS)
                    .any(|row| row.quota_code == code && !row.description.is_empty());
                let served = quota["Description"].as_str().unwrap_or_default();
                assert_eq!(
                    !served.is_empty(),
                    citable,
                    "case {case} {code}: a row publishes the quota's description where this \
                     catalog cites one and none where it does not, got {quota}"
                );
            }
        }
    }
}

/// A code these services publish nothing under resolves nowhere, while the service
/// itself still resolves.
#[tokio::test]
async fn a_code_the_service_publishes_nothing_under_resolves_nowhere() {
    let engine = emulator();
    let caller = client(&engine, "ap-northeast-1");

    for (service_code, withdrawn) in WITHDRAWN_CODES {
        let case = format!("{service_code}/{withdrawn}");

        for operation in ["ListAWSDefaultServiceQuotas", "ListServiceQuotas"] {
            let listed = listed_codes(&caller, operation, service_code).await;
            assert!(
                !listed.contains(*withdrawn),
                "case {case}: {operation} must not publish a code the service publishes nothing \
                 under, got {listed:?}"
            );
        }

        for operation in [
            "GetAWSDefaultServiceQuota",
            "GetServiceQuota",
            "RequestServiceQuotaIncrease",
        ] {
            let mut body = json!({"ServiceCode": service_code, "QuotaCode": withdrawn});
            if operation == "RequestServiceQuotaIncrease" {
                body["DesiredValue"] = json!(10_000.0);
            }
            let (status, error) = caller.refused(operation, body).await;
            assert_eq!(
                (status, error.as_str()),
                (400, "NoSuchResourceException"),
                "case {case}: {operation} must refuse a code this service publishes nothing under"
            );
        }

        // The SERVICE still resolves: it is the withdrawn quota that is unpublished.
        let response = caller
            .ok(
                "ListAWSDefaultServiceQuotas",
                json!({"ServiceCode": service_code}),
            )
            .await;
        assert!(
            response["Quotas"].is_array(),
            "case {case}: the service code must still resolve its listing, got {response}"
        );
    }
}

/// A quota AWS publishes as a quota OF A RESOURCE reports the whole `QuotaContext` AWS
/// publishes for it — the plane it is counted on, that plane's resource type and the
/// level an increase may be asked for at — through every read and the listing alike, and
/// its value resolves on that plane as well as account-wide; a quota managed at the
/// account level only reports no context at all.
#[tokio::test]
async fn a_resource_plane_quota_reports_the_context_aws_publishes() {
    let engine = emulator();
    let caller = client(&engine, "us-east-1");

    let mut with_context = 0_usize;
    for row in ROWS.iter().chain(UNCITED_ROWS) {
        let case = format!("{}/{}", row.service_code, row.quota_code);
        let pair = json!({"ServiceCode": row.service_code, "QuotaCode": row.quota_code});
        let default = caller
            .ok("GetAWSDefaultServiceQuota", pair.clone())
            .await["Quota"]
            .clone();
        let applied = caller.ok("GetServiceQuota", pair).await["Quota"].clone();
        let listed = defaults(&caller, row.service_code).await;
        let in_listing = listed
            .iter()
            .find(|quota| quota["QuotaCode"] == json!(row.quota_code))
            .cloned()
            .unwrap_or(Value::Null);

        match &row.context {
            None => {
                for (read, record) in [
                    ("GetAWSDefaultServiceQuota", &default),
                    ("GetServiceQuota", &applied),
                    ("ListAWSDefaultServiceQuotas", &in_listing),
                ] {
                    assert!(
                        record.get("QuotaContext").is_none_or(Value::is_null),
                        "case {case}: a quota managed at the account level reports no context, \
                         got {record} from {read}"
                    );
                }
            }
            Some(context) => {
                with_context += 1;
                // The whole block, not one member of it: the scope is the plane the
                // quota is counted on, which is a published fact of the quota rather
                // than of the read.
                let published = json!({
                    "ContextScope": context.scope,
                    "ContextScopeType": context.scope_type,
                    "AdjustableAtLevel": context.adjustable_at_level,
                });
                for (read, record) in [
                    ("GetAWSDefaultServiceQuota", &default),
                    ("GetServiceQuota", &applied),
                    ("ListAWSDefaultServiceQuotas", &in_listing),
                ] {
                    assert_eq!(
                        record["QuotaContext"], published,
                        "case {case}: expected the context AWS publishes for the quota, got \
                         {record} from {read}"
                    );
                }
                // And the value resolves on the resource plane too, reported at the level
                // the read named, with the resource the read asked for named in the
                // context.
                let on_resource = caller
                    .ok(
                        "GetServiceQuota",
                        json!({
                            "ServiceCode": row.service_code,
                            "QuotaCode": row.quota_code,
                            "ContextId": "arn:aws:servicequotas:us-east-1::context/probe",
                        }),
                    )
                    .await["Quota"]
                    .clone();
                assert_eq!(
                    (
                        on_resource["Value"].clone(),
                        on_resource["QuotaAppliedAtLevel"].clone(),
                        on_resource["QuotaContext"]["ContextScope"].clone(),
                        on_resource["QuotaContext"]["ContextId"].clone()
                    ),
                    (
                        json!(row.value),
                        json!("RESOURCE"),
                        json!("RESOURCE"),
                        json!("arn:aws:servicequotas:us-east-1::context/probe")
                    ),
                    "case {case}: a read for one resource answers that plane at the published \
                     default, got {on_resource}"
                );
            }
        }
    }

    assert!(
        with_context >= 2,
        "case catalog: the rule must be asserted on the quotas that HAVE a resource plane, \
         got {with_context}"
    );
}
