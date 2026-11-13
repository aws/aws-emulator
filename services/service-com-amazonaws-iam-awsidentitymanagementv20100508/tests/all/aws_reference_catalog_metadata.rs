// SPDX-License-Identifier: CC0-1.0

//! The reference data AWS itself publishes, as this account reads it: the AWS-managed
//! policy catalog (`ListPolicies` with `Scope=AWS`, filtered by `PathPrefix`, cross-read
//! through `GetPolicy` and `GetPolicyVersion`) and the catalog of service namespaces a
//! service-last-accessed report covers (`GenerateServiceLastAccessedDetails` +
//! `GetServiceLastAccessedDetails`).
//!
//! Every assertion drives a `MultiServiceRouter` holding this service over one shared
//! `InMemoryEngine`, exactly as the production router does, so what is under test is the
//! HTTP behaviour (status, awsQuery members) a client on the socket sees. The awsQuery
//! harness (router, call, element, elements) is the one `tests/all/user_lifecycle.rs` owns
//! and every suite in this crate shares.
//!
//! ONE rule, asserted on both catalogs: a built-in reference catalog is reported from the
//! data of the entry being reported — a managed policy's own id, default version,
//! publication and revision instants and published attachability; a report's own one entry
//! per service namespace the catalog covers, in the catalog's order — and the catalog is
//! served whole: every family it publishes is selectable by its path, and a report asked for
//! no page size carries every service it covers. Nothing a reader reports is a value stamped
//! on the catalog as a whole, and nothing is left out of an enumeration of it.
//!
//! Each suite is a TABLE whose rows differ along what the rule generalises over: the policy
//! family's path (all five AWS publishes), the entry inside that family (every one of them,
//! not the handful a fixture cites), the member being reported, the page size an enumeration
//! is read at, and how wide the grant a report is generated for is.

use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_iam_awsidentitymanagementv20100508::state;

use crate::role_lifecycle::{borrowed, optional, param};
use crate::user_lifecycle::{call, element, elements, error_code, router};

/// A trust policy every role here needs to exist at all. It is a resource-based policy, so
/// it puts no service in the role's reach.
const TRUST_POLICY: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"Service":"glue.amazonaws.com"},"Action":"sts:AssumeRole"}]}"#;

/// A grant of every action, so a report generated for the role that carries it covers every
/// service the published namespace catalog carries.
const EVERY_ACTION: &str =
    r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"*","Resource":"*"}]}"#;

/// The paths AWS publishes its managed policies under.
///
/// SOURCE: the AWS Managed Policy Reference, which files every policy under the account
/// root, under `/aws-service-role/` (a service-linked role's own policy), under
/// `/job-function/`, under `/root-task/` (the privileged root tasks a management account
/// takes in a member account) and under `/service-role/`. Written out rather than read off
/// the catalog, so a family the catalog stopped carrying would fail this suite.
const PUBLISHED_PATHS: &[&str] = &[
    "/",
    "/aws-service-role/",
    "/job-function/",
    "/root-task/",
    "/service-role/",
];

/// The AWS-managed policies AWS publishes under `/root-task/`, with the default version and
/// attachability it publishes for each.
///
/// SOURCE: the AWS Managed Policy Reference's `/root-task/` family — the task policies a
/// privileged root session is taken for. Each is published at its first version and as one
/// that can be attached.
const ROOT_TASK_POLICIES: &[&str] = &[
    "IAMAuditRootUserCredentials",
    "IAMCreateRootUserPassword",
    "IAMDeleteRootUserCredentials",
    "S3UnlockBucketPolicy",
    "SQSUnlockQueuePolicy",
];

/// Catalog entries no cited fixture names, one per family, so the rule is asserted away
/// from the policies the recorded trace happened to read.
const UNCITED_ENTRIES: &[(&str, &str)] = &[
    ("/", "CloudWatchAgentServerPolicy"),
    ("/aws-service-role/", "AmazonGuardDutyServiceRolePolicy"),
    ("/job-function/", "DatabaseAdministrator"),
    ("/root-task/", "S3UnlockBucketPolicy"),
    ("/service-role/", "AWSGlueServiceRole"),
];

/// Namespaces a service-last-accessed report covers, with the name AWS publishes for each.
///
/// SOURCE: the Service Authorization Reference, which `ServiceLastAccessed$ServiceNamespace`
/// points at to learn a service's prefix. The first rows are the head of that reference's
/// namespace order — the run a report's first members must spell, in this order — and the
/// rest are prefixes from further in, named here so the catalog is asserted past the entries
/// any one trace reads.
const PUBLISHED_NAMESPACE_HEAD: &[(&str, &str)] = &[
    ("a4b", "Alexa for Business"),
    ("access-analyzer", "AWS IAM Access Analyzer"),
    ("account", "AWS Account Management"),
    ("acm", "AWS Certificate Manager"),
    ("acm-pca", "AWS Private Certificate Authority"),
    ("action-recommendations", "AWS Action Recommendations"),
    ("agent-registry", "AWS Agent Registry"),
    ("aidevops", "AWS DevOps Agent Service"),
    ("aiops", "Amazon AI Operations"),
    ("airflow", "Amazon Managed Workflows for Apache Airflow"),
    ("amplify", "AWS Amplify"),
];

/// Namespaces from further into that reference, each with its published name.
const PUBLISHED_NAMESPACES_ELSEWHERE: &[(&str, &str)] = &[
    ("aoss", "Amazon OpenSearch Serverless"),
    ("q", "Amazon Q"),
    ("s3tables", "Amazon S3 Tables"),
    ("timestream-influxdb", "Amazon Timestream InfluxDB"),
    ("xray", "AWS X-Ray"),
];

/// The ARN AWS publishes a managed policy under: `arn:aws:iam::aws:policy<path><name>`.
fn published_arn(path: &str, policy_name: &str) -> String {
    format!("arn:aws:iam::aws:policy{path}{policy_name}")
}

/// One policy as an enumeration or a read reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ReportedPolicy {
    arn: String,
    path: String,
    policy_id: String,
    default_version_id: String,
    create_date: String,
    update_date: String,
    is_attachable: String,
}

/// Every policy `ListPolicies` reports for `params`, walked over ALL its pages by following
/// the `Marker` the service hands out, with a page size small enough that the walk really
/// pages.
async fn walk_catalog(
    router: &MultiServiceRouter,
    case: &str,
    params: &[(&str, &str)],
) -> Vec<ReportedPolicy> {
    let mut reported: Vec<ReportedPolicy> = Vec::new();
    let mut marker: Option<String> = None;
    // The catalog is finite, so the walk terminates; the cap only keeps a broken `Marker`
    // from looping forever.
    for page in 0..1_000 {
        let mut request: Vec<(&str, &str)> = params.to_vec();
        if let Some(token) = marker.as_deref() {
            request.push(("Marker", token));
        }
        let (status, body) = call(router, "ListPolicies", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?} page {page}: ListPolicies must succeed, got {status}: {body}"
        );
        let arns = elements(&body, "Arn");
        let paths = elements(&body, "Path");
        let ids = elements(&body, "PolicyId");
        let versions = elements(&body, "DefaultVersionId");
        let created = elements(&body, "CreateDate");
        let updated = elements(&body, "UpdateDate");
        let attachable = elements(&body, "IsAttachable");
        for (member, found) in [
            ("Path", paths.len()),
            ("PolicyId", ids.len()),
            ("DefaultVersionId", versions.len()),
            ("CreateDate", created.len()),
            ("UpdateDate", updated.len()),
            ("IsAttachable", attachable.len()),
        ] {
            assert_eq!(
                found,
                arns.len(),
                "case {case:?} page {page}: every reported policy must carry one <{member}>, \
                 got {found} for {} policies: {body}",
                arns.len()
            );
        }
        for index in 0..arns.len() {
            reported.push(ReportedPolicy {
                arn: arns[index].clone(),
                path: paths[index].clone(),
                policy_id: ids[index].clone(),
                default_version_id: versions[index].clone(),
                create_date: created[index].clone(),
                update_date: updated[index].clone(),
                is_attachable: attachable[index].clone(),
            });
        }
        if element(&body, "IsTruncated") == "true" {
            marker = Some(element(&body, "Marker"));
        } else {
            marker = None;
            break;
        }
    }
    assert!(
        marker.is_none(),
        "case {case:?}: the walk must terminate, got a Marker that never ran out"
    );
    reported
}

/// The policy `GetPolicy` reports for `arn`, which must resolve.
async fn read_policy(router: &MultiServiceRouter, arn: &str) -> ReportedPolicy {
    let (status, body) = call(router, "GetPolicy", &[("PolicyArn", arn)]).await;
    assert_eq!(
        status, 200,
        "case {arn:?}: GetPolicy must serve a published policy, got {status}: {body}"
    );
    ReportedPolicy {
        arn: element(&body, "Arn"),
        path: element(&body, "Path"),
        policy_id: element(&body, "PolicyId"),
        default_version_id: element(&body, "DefaultVersionId"),
        create_date: element(&body, "CreateDate"),
        update_date: element(&body, "UpdateDate"),
        is_attachable: element(&body, "IsAttachable"),
    }
}

/// The catalog entry behind `arn`, which every reported policy must have.
fn entry_of(arn: &str) -> &'static state::AwsManagedPolicy {
    state::aws_managed_policy(arn)
        .unwrap_or_else(|| panic!("case {arn:?}: the catalog must carry the policy it reports"))
}

/// `Policy$PolicyId` as AWS spells a policy's unique id: the `ANPA` prefix and 17 further
/// characters, all of the modelled `idType` (len 16..128, pattern `^[\w]+$`).
fn assert_published_policy_id(case: &str, policy_id: &str) {
    assert_eq!(
        policy_id.len(),
        21,
        "case {case:?}: a published policy id is ANPA plus 17 characters, got {policy_id:?}"
    );
    assert!(
        policy_id
            .strip_prefix("ANPA")
            .is_some_and(|suffix| suffix
                .chars()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())),
        "case {case:?}: a published policy id is ANPA plus uppercase alphanumerics, got \
         {policy_id:?}"
    );
}

/// RULE: every member of the enumerated AWS-managed policy catalog is reported from that
/// policy's own publication data — its id, the version it is published at, the instants it
/// was published and last revised, and the attachability AWS publishes for it — and the
/// single-policy read reports the same record.
#[tokio::test]
async fn every_catalog_policy_is_reported_from_its_own_publication_data() {
    let router = router();
    let reported = walk_catalog(&router, "Scope=AWS", &[("Scope", "AWS"), ("MaxItems", "7")]).await;
    assert_eq!(
        reported.len(),
        state::AWS_MANAGED_POLICIES.len(),
        "the enumeration must report every catalog entry, got {} of {}",
        reported.len(),
        state::AWS_MANAGED_POLICIES.len()
    );

    let mut ids: ::std::collections::BTreeSet<String> = ::std::collections::BTreeSet::new();
    for policy in &reported {
        let case = policy.arn.clone();
        let entry = entry_of(&policy.arn);

        assert_published_policy_id(&case, &policy.policy_id);
        assert!(
            ids.insert(policy.policy_id.clone()),
            "case {case:?}: a PolicyId identifies ONE policy, but {:?} repeats",
            policy.policy_id
        );
        assert_eq!(
            policy.policy_id, entry.policy_id,
            "case {case:?}: expected this policy's own id, got {:?}",
            policy.policy_id
        );
        assert_eq!(
            policy.default_version_id, entry.default_version_id,
            "case {case:?}: expected the version this policy is published at, got {:?}",
            policy.default_version_id
        );
        assert_eq!(
            policy.create_date, entry.published,
            "case {case:?}: expected this policy's publication instant, got {:?}",
            policy.create_date
        );
        assert_eq!(
            policy.update_date, entry.revised,
            "case {case:?}: expected this policy's revision instant, got {:?}",
            policy.update_date
        );
        assert_eq!(
            policy.is_attachable,
            entry.is_attachable.to_string(),
            "case {case:?}: expected the attachability AWS publishes for this policy, got {:?}",
            policy.is_attachable
        );
        assert!(
            policy.create_date <= policy.update_date,
            "case {case:?}: a policy is revised no earlier than it is published, got {:?} and \
             {:?}",
            policy.create_date,
            policy.update_date
        );

        // The listing and the single-policy read tell ONE story about the policy.
        let read = read_policy(&router, &policy.arn).await;
        assert_eq!(
            read, *policy,
            "case {case:?}: GetPolicy must report the record the listing reported"
        );
    }

    // The five members are PER POLICY, not one template's values: a client keying the
    // catalog by id, pinning a default version or sorting by either date sees policies that
    // differ.
    for (member, distinct) in [
        (
            "PolicyId",
            reported
                .iter()
                .map(|policy| policy.policy_id.as_str())
                .collect::<::std::collections::BTreeSet<&str>>(),
        ),
        (
            "DefaultVersionId",
            reported
                .iter()
                .map(|policy| policy.default_version_id.as_str())
                .collect(),
        ),
        (
            "CreateDate",
            reported
                .iter()
                .map(|policy| policy.create_date.as_str())
                .collect(),
        ),
        (
            "UpdateDate",
            reported
                .iter()
                .map(|policy| policy.update_date.as_str())
                .collect(),
        ),
    ] {
        assert!(
            distinct.len() > 1,
            "case {member:?}: the catalog must report each policy's own value, got one shared \
             value across {} policies",
            reported.len()
        );
    }
    assert!(
        reported
            .iter()
            .any(|policy| policy.default_version_id != "v1"),
        "AWS bumps a managed policy's default version as it revises the policy, so the \
         catalog must report an entry past its first version"
    );
    assert!(
        reported
            .iter()
            .any(|policy| policy.update_date != policy.create_date),
        "a revised policy's UpdateDate is the instant of its revision, so the catalog must \
         report an entry whose two instants differ"
    );

    // Rows naming entries the recorded trace did not read, so the rule is asserted away
    // from the policies a fixture cited: each carries its own data, and no two of them
    // carry the same id or the same pair of instants.
    let mut seen: Vec<(String, String, String)> = Vec::new();
    for (path, policy_name) in UNCITED_ENTRIES {
        let arn = published_arn(path, policy_name);
        let case = arn.clone();
        let listed = reported
            .iter()
            .find(|policy| policy.arn == arn)
            .unwrap_or_else(|| panic!("case {case:?}: the catalog must enumerate this policy"));
        assert_eq!(
            listed.path, *path,
            "case {case:?}: expected the path AWS publishes it under, got {:?}",
            listed.path
        );
        let row = (
            listed.policy_id.clone(),
            listed.create_date.clone(),
            listed.update_date.clone(),
        );
        assert!(
            !seen.contains(&row),
            "case {case:?}: two published policies must not report the same id and instants, \
             got {row:?}"
        );
        seen.push(row);
    }
}

/// RULE: the version a policy reports as its default is the version a reader is served, so
/// `DefaultVersionId` always names a version `GetPolicyVersion` answers with that policy's
/// document — and a version number the policy is not published at names nothing.
#[tokio::test]
async fn the_default_version_a_policy_reports_is_the_version_served() {
    let router = router();

    for (path, policy_name) in UNCITED_ENTRIES {
        let arn = published_arn(path, policy_name);
        let case = arn.clone();
        let read = read_policy(&router, &arn).await;

        let (status, body) = call(
            &router,
            "GetPolicyVersion",
            &[
                ("PolicyArn", arn.as_str()),
                ("VersionId", read.default_version_id.as_str()),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetPolicyVersion must serve the default version {:?}, got \
             {status}: {body}",
            read.default_version_id
        );
        assert_eq!(
            element(&body, "VersionId"),
            read.default_version_id,
            "case {case:?}: expected the version asked for, got {body}"
        );
        assert_eq!(
            element(&body, "IsDefaultVersion"),
            "true",
            "case {case:?}: the version GetPolicy named is the default one, got {body}"
        );
        assert!(
            state::is_policy_document(&crate::user_lifecycle::document_element(
                &body, "Document"
            )),
            "case {case:?}: expected an IAM policy document, got {body}"
        );

        // One past the version it is published at: a version number the model admits
        // (`^v[1-9][0-9]*$`) that this policy does not carry.
        let number: u32 = read
            .default_version_id
            .trim_start_matches('v')
            .parse()
            .unwrap_or_else(|_| {
                panic!(
                    "case {case:?}: a default version is numbered, got {:?}",
                    read.default_version_id
                )
            });
        let absent = format!("v{}", number + 1);
        let (status, body) = call(
            &router,
            "GetPolicyVersion",
            &[("PolicyArn", arn.as_str()), ("VersionId", absent.as_str())],
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?}: a version the policy does not carry must answer 404, got \
             {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "NoSuchEntity",
            "case {case:?}: the refusal must be NoSuchEntity, got {body}"
        );
    }
}

/// RULE: `PathPrefix` selects the catalog's members by the path AWS files them under, so
/// every family AWS publishes — including the root-task family — is enumerable, and a prefix
/// no policy is published under selects none.
#[tokio::test]
async fn every_published_family_is_enumerable_by_its_path_prefix() {
    let router = router();

    for path in PUBLISHED_PATHS {
        let case = format!("the family at {path}");
        let reported = walk_catalog(
            &router,
            &case,
            &[("Scope", "AWS"), ("PathPrefix", path), ("MaxItems", "4")],
        )
        .await;
        let mut expected: Vec<String> = state::AWS_MANAGED_POLICIES
            .iter()
            .filter(|entry| entry.path.starts_with(path))
            .map(|entry| entry.arn())
            .collect();
        expected.sort();
        assert!(
            !expected.is_empty(),
            "case {case:?}: AWS publishes policies under this path, so the catalog must \
             carry them"
        );
        assert_eq!(
            reported
                .iter()
                .map(|policy| policy.arn.clone())
                .collect::<Vec<String>>(),
            expected,
            "case {case:?}: expected the family's members in one ascending enumeration"
        );
        for policy in &reported {
            assert!(
                policy.path.starts_with(path),
                "case {case:?}: every member selected by a prefix is published under it, got \
                 {:?}",
                policy.path
            );
        }
    }

    // A prefix no policy is published under selects nothing — which is what an empty
    // member list MEANS, and why a published family must never answer one.
    for path in ["/no-such-family/", "/root-task/deeper/"] {
        let case = format!("the unused prefix {path}");
        let reported = walk_catalog(
            &router,
            &case,
            &[("Scope", "AWS"), ("PathPrefix", path), ("MaxItems", "4")],
        )
        .await;
        assert!(
            reported.is_empty(),
            "case {case:?}: no policy is published under this path, got {reported:?}"
        );
    }
}

/// RULE: a published family's members carry the data AWS publishes for them, asserted here
/// of the root-task family written out by hand: each is published under `/root-task/` at its
/// first version, as attachable, with its own ANPA id — and is readable by ARN, at the
/// account's default scope as well as at `Scope=AWS`.
#[tokio::test]
async fn the_root_task_family_is_published_as_aws_publishes_it() {
    let router = router();

    let scoped = walk_catalog(
        &router,
        "the root-task family",
        &[
            ("Scope", "AWS"),
            ("PathPrefix", "/root-task/"),
            ("MaxItems", "2"),
        ],
    )
    .await;
    assert_eq!(
        scoped
            .iter()
            .map(|policy| policy.arn.clone())
            .collect::<Vec<String>>(),
        ROOT_TASK_POLICIES
            .iter()
            .map(|policy_name| published_arn("/root-task/", policy_name))
            .collect::<Vec<String>>(),
        "expected the published root-task policies, ascending by ARN"
    );

    // The account's own view of IAM carries AWS's policies too, so the family is reported
    // at the default scope as well.
    let unscoped = walk_catalog(
        &router,
        "the root-task family at the default scope",
        &[("PathPrefix", "/root-task/"), ("MaxItems", "3")],
    )
    .await;
    assert_eq!(
        unscoped, scoped,
        "a request that names no Scope reports all policies, so the family must be reported \
         there too"
    );

    let mut ids: Vec<String> = Vec::new();
    for policy_name in ROOT_TASK_POLICIES {
        let arn = published_arn("/root-task/", policy_name);
        let case = arn.clone();
        let read = read_policy(&router, &arn).await;
        assert_eq!(
            read.path, "/root-task/",
            "case {case:?}: expected the published path, got {:?}",
            read.path
        );
        assert_eq!(
            read.default_version_id, "v1",
            "case {case:?}: a root-task policy is published at its first version, got {:?}",
            read.default_version_id
        );
        assert_eq!(
            read.is_attachable, "true",
            "case {case:?}: AWS publishes a root-task policy as attachable, got {:?}",
            read.is_attachable
        );
        assert_published_policy_id(&case, &read.policy_id);
        assert!(
            !ids.contains(&read.policy_id),
            "case {case:?}: each member of the family carries its own id, got {:?} twice",
            read.policy_id
        );
        ids.push(read.policy_id.clone());

        // The grant the policy was published for is served, not an empty record.
        let (status, body) = call(
            &router,
            "GetPolicyVersion",
            &[("PolicyArn", arn.as_str()), ("VersionId", "v1")],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetPolicyVersion must serve the published version, got {status}: \
             {body}"
        );
        assert!(
            state::is_policy_document(&crate::user_lifecycle::document_element(
                &body, "Document"
            )),
            "case {case:?}: expected an IAM policy document, got {body}"
        );
    }
}

/// How long to wait for the generation the clock owes a report to become due.
fn generation_span() -> ::std::time::Duration {
    ::std::time::Duration::from_secs(state::ACCESS_REPORT_GENERATION_SECONDS as u64 + 1)
}

/// Create a role carrying `document` as an inline permissions policy, answering its ARN.
async fn role_granting(router: &MultiServiceRouter, role_name: &str, document: &str) -> String {
    let (status, body) = call(
        router,
        "CreateRole",
        &[
            ("RoleName", role_name),
            ("AssumeRolePolicyDocument", TRUST_POLICY),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {role_name:?}: CreateRole must succeed, got {status}: {body}"
    );
    let role_arn = element(&body, "Arn");
    let (status, body) = call(
        router,
        "PutRolePolicy",
        &[
            ("RoleName", role_name),
            ("PolicyName", "granted"),
            ("PolicyDocument", document),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {role_name:?}: PutRolePolicy must succeed, got {status}: {body}"
    );
    role_arn
}

/// Record a report job about `arn`, answering the id it is read back by.
async fn generate_report(router: &MultiServiceRouter, arn: &str) -> String {
    let (status, body) = call(
        router,
        "GenerateServiceLastAccessedDetails",
        &[("Arn", arn)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {arn:?}: GenerateServiceLastAccessedDetails must succeed, got {status}: {body}"
    );
    element(&body, "JobId")
}

/// One page of a completed report: the namespaces and names it carried, and the marker it
/// handed out.
struct ReportPage {
    namespaces: Vec<String>,
    names: Vec<String>,
    is_truncated: bool,
    marker: Option<String>,
}

/// Read one page of the report `job_id` names, at `max_items` when the row names one.
async fn report_page(
    router: &MultiServiceRouter,
    case: &str,
    job_id: &str,
    max_items: Option<usize>,
    marker: Option<&str>,
) -> ReportPage {
    let mut params: Vec<(String, String)> = vec![param("JobId", job_id)];
    if let Some(size) = max_items {
        params.push(param("MaxItems", &size.to_string()));
    }
    if let Some(token) = marker {
        params.push(param("Marker", token));
    }
    let (status, body) = call(
        router,
        "GetServiceLastAccessedDetails",
        &borrowed(&params),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetServiceLastAccessedDetails must succeed, got {status}: {body}"
    );
    assert_eq!(
        element(&body, "JobStatus"),
        state::JOB_STATUS_COMPLETED,
        "case {case:?}: expected the generation the clock owed to be done, got {body}"
    );
    ReportPage {
        namespaces: elements(&body, "ServiceNamespace"),
        names: elements(&body, "ServiceName"),
        is_truncated: element(&body, "IsTruncated") == "true",
        marker: optional(&body, "Marker"),
    }
}

/// RULE: a completed report carries one entry per service namespace the published catalog
/// covers and the resource's policies reach, in the catalog's own namespace order with no
/// namespace left out — so a client reading the list positionally attributes each entry to
/// the service it belongs to — and a read that asks for no page size is served the report
/// whole.
#[tokio::test]
async fn a_report_covers_the_published_namespace_catalog_without_gaps() {
    let router = router();
    let role_arn = role_granting(&router, "catalog-report-role", EVERY_ACTION).await;
    let job_id = generate_report(&router, &role_arn).await;
    tokio::time::sleep(generation_span()).await;

    // A read that asks for no page size is served every service the report covers.
    let whole = report_page(&router, "the whole report", &job_id, None, None).await;
    assert!(
        !whole.is_truncated,
        "a report asked for with no MaxItems is served whole, so nothing remains: got \
         IsTruncated true after {} services",
        whole.namespaces.len()
    );
    assert_eq!(
        whole.marker, None,
        "a report that is not truncated hands out no Marker, got {:?}",
        whole.marker
    );

    let expected_namespaces: Vec<String> = state::REPORTED_SERVICES
        .iter()
        .map(|service| service.service_namespace.to_string())
        .collect();
    let expected_names: Vec<String> = state::REPORTED_SERVICES
        .iter()
        .map(|service| service.service_name.to_string())
        .collect();
    assert_eq!(
        whole.namespaces, expected_namespaces,
        "expected one entry per service the catalog covers, in the catalog's order"
    );
    assert_eq!(
        whole.names, expected_names,
        "expected each entry to name its own service"
    );

    // The head of the published namespace order, written out: a namespace missing from the
    // catalog would shift every later member up one position, which this run catches.
    let head: Vec<String> = whole.namespaces.iter().take(PUBLISHED_NAMESPACE_HEAD.len()).cloned().collect();
    assert_eq!(
        head,
        PUBLISHED_NAMESPACE_HEAD
            .iter()
            .map(|(namespace, _)| (*namespace).to_string())
            .collect::<Vec<String>>(),
        "expected the published namespace order at the head of the report"
    );
    for (position, (namespace, service_name)) in PUBLISHED_NAMESPACE_HEAD.iter().enumerate() {
        assert_eq!(
            whole.names[position], *service_name,
            "case {namespace:?}: expected the name AWS publishes for the service at position \
             {position}, got {:?}",
            whole.names[position]
        );
    }

    // And namespaces from further into the reference are covered too, each under its own
    // name: a report short of them reads as a principal that cannot reach those services.
    for (namespace, service_name) in PUBLISHED_NAMESPACES_ELSEWHERE {
        let position = whole
            .namespaces
            .iter()
            .position(|reported| reported == namespace)
            .unwrap_or_else(|| {
                panic!("case {namespace:?}: the report must cover this published namespace")
            });
        assert_eq!(
            whole.names[position], *service_name,
            "case {namespace:?}: expected the name AWS publishes for it, got {:?}",
            whole.names[position]
        );
    }

    // Asked for a page size, the SAME enumeration is paged: the pages together are the
    // whole report, in one order, with nothing repeated and nothing skipped.
    for size in [1_usize, 50, 400] {
        let case = format!("{size} services per page");
        let mut walked: Vec<String> = Vec::new();
        let mut marker: Option<String> = None;
        for page in 0..1_000 {
            let read = report_page(&router, &case, &job_id, Some(size), marker.as_deref()).await;
            assert!(
                read.namespaces.len() <= size,
                "case {case:?} page {page}: a page carries at most MaxItems services, got {}",
                read.namespaces.len()
            );
            assert_eq!(
                read.marker.is_some(),
                read.is_truncated,
                "case {case:?} page {page}: a Marker is reported exactly when more remain, \
                 got truncated={} marker={:?}",
                read.is_truncated,
                read.marker
            );
            walked.extend(read.namespaces.clone());
            if !read.is_truncated {
                marker = None;
                break;
            }
            marker = read.marker.clone();
        }
        assert!(
            marker.is_none(),
            "case {case:?}: the walk must terminate, got a Marker that never ran out"
        );
        assert_eq!(
            walked, expected_namespaces,
            "case {case:?}: the pages together must be the whole report"
        );
    }
}

/// RULE: the report covers the services the resource's policies reach out of that same
/// catalog, so a narrower grant reports fewer entries — the catalog's breadth is what is
/// enumerated, not what is granted.
#[tokio::test]
async fn a_narrower_grant_reports_fewer_services_from_the_same_catalog() {
    let router = router();

    // Rows spanning how wide the grant is: one service, a run of namespaces, and every
    // action.
    let rows: &[(&str, &str, Option<&[&str]>)] = &[
        (
            "a grant of one service",
            r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"sqs:SendMessage","Resource":"*"}]}"#,
            Some(&["sqs"]),
        ),
        (
            "a grant of a namespace run",
            r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"ssm-*:Get*","Resource":"*"}]}"#,
            Some(&[
                "ssm-contacts",
                "ssm-guiconnect",
                "ssm-incidents",
                "ssm-quicksetup",
                "ssm-sap",
            ]),
        ),
        ("a grant of every action", EVERY_ACTION, None),
    ];

    let mut jobs: Vec<(&str, String, Vec<String>)> = Vec::new();
    for (index, (case, document, expected)) in rows.iter().enumerate() {
        let role_name = format!("grant-width-{index}");
        let role_arn = role_granting(&router, &role_name, document).await;
        let expected: Vec<String> = match expected {
            Some(namespaces) => namespaces.iter().map(|name| (*name).to_string()).collect(),
            None => state::REPORTED_SERVICES
                .iter()
                .map(|service| service.service_namespace.to_string())
                .collect(),
        };
        jobs.push((case, generate_report(&router, &role_arn).await, expected));
    }

    tokio::time::sleep(generation_span()).await;

    for (case, job_id, expected) in &jobs {
        let read = report_page(&router, case, job_id, None, None).await;
        assert_eq!(
            read.namespaces, *expected,
            "case {case:?}: expected the services the grant reaches, out of the published \
             catalog"
        );
        assert!(
            !read.is_truncated,
            "case {case:?}: a report asked for with no MaxItems is served whole"
        );
    }
}
