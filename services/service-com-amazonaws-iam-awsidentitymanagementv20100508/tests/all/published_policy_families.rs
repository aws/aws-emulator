// SPDX-License-Identifier: CC0-1.0

//! The reference data every account sees: AWS's own managed-policy catalog, read through
//! `ListPolicies`/`GetPolicy`, and the catalog of service namespaces a service-last-accessed
//! report covers, read through `GenerateServiceLastAccessedDetails` +
//! `GetServiceLastAccessedDetails`.
//!
//! Every assertion drives a `MultiServiceRouter` holding this service over one shared
//! `InMemoryEngine`, exactly as the production router does, so what is under test is the
//! HTTP behaviour (status, awsQuery members) a client on the socket sees. The awsQuery
//! harness (router, call, element, elements) is the one `tests/all/user_lifecycle.rs` owns
//! and every suite in this crate shares.
//!
//! Two rules are asserted:
//!
//! 1. the AWS-managed policy catalog an account enumerates carries every FAMILY AWS
//!    publishes — the policies at the account root, the ones under the path reserved for
//!    service-linked roles, the job functions and the service roles — so a client that
//!    resolves a policy by listing the catalog under its path finds it there, with the
//!    record its own `GetPolicy` read reports;
//! 2. a completed service-last-accessed report enumerates the services the resource's
//!    permissions policies allow an action of out of the published catalog of service
//!    namespaces, ascending by namespace, each entry carrying that service's published name
//!    and the number of principals recorded as having attempted to reach it — and pages over
//!    that enumeration with `MaxItems`, `IsTruncated` and a resumable `Marker`.
//!
//! Each suite is a TABLE whose rows differ along what the rule generalises over: the policy
//! family's path, whether its members may be attached, how wide a grant the report's
//! resource holds (one service, the read-only catalog, the view-only catalog), and the page
//! size the report is read at.

use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_iam_awsidentitymanagementv20100508::state;

use crate::role_lifecycle::optional;
use crate::user_lifecycle::{call, element, elements, router};

/// A trust policy every role here needs to exist at all. It is a resource-based policy, so
/// it puts no service in the role's reach.
const TRUST_POLICY: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"Service":"glue.amazonaws.com"},"Action":"sts:AssumeRole"}]}"#;

/// AWS managed policies as the AWS Managed Policy Reference publishes them:
/// `(path, policy_name, is_attachable)`.
///
/// SOURCE: that reference, read per entry — the path it files the policy under, the friendly
/// name it publishes, and the attachability it publishes for the policy (AWS publishes every
/// policy of this catalog as one that can be attached; the service-linked-role family is
/// RESERVED at attach time instead, which `tests/all/aws_managed_policy_attachability.rs`
/// asserts). The rows span the five paths the catalog carries and deliberately name entries
/// no other fixture in this crate cites.
const PUBLISHED: &[(&str, &str, bool)] = &[
    ("/", "AdministratorAccess", true),
    ("/", "CloudWatchLogsFullAccess", true),
    ("/aws-service-role/", "AWSTrustedAdvisorServiceRolePolicy", true),
    ("/aws-service-role/", "AmazonGuardDutyServiceRolePolicy", true),
    ("/aws-service-role/", "AmazonRedshiftServiceLinkedRolePolicy", true),
    ("/job-function/", "Billing", true),
    ("/job-function/", "NetworkAdministrator", true),
    ("/job-function/", "ViewOnlyAccess", true),
    ("/root-task/", "IAMAuditRootUserCredentials", true),
    ("/root-task/", "S3UnlockBucketPolicy", true),
    ("/root-task/", "SQSUnlockQueuePolicy", true),
    ("/service-role/", "AWSLambdaBasicExecutionRole", true),
];

/// The ARN AWS publishes a managed policy under: `arn:aws:iam::aws:policy<path><name>`.
fn published_arn(path: &str, policy_name: &str) -> String {
    format!("arn:aws:iam::aws:policy{path}{policy_name}")
}

/// Every page of `ListPolicies` for `params`, walked by following the `Marker` the service
/// hands out, answering the members of one named element across all the pages in order.
async fn walk_policies(
    router: &MultiServiceRouter,
    case: &str,
    params: &[(&str, &str)],
    member: &str,
) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
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
        found.extend(elements(&body, member));
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
    found
}

/// RULE: the AWS-managed policy catalog an account enumerates carries every family AWS
/// publishes, each member reported under the path AWS files it at, with the attachability
/// that path implies and the same record its own `GetPolicy` read serves.
#[tokio::test]
async fn the_catalog_enumerates_every_published_policy_family() {
    let router = router();

    for (path, policy_name, is_attachable) in PUBLISHED {
        let case = format!("{policy_name} at {path}");
        let arn = published_arn(path, policy_name);

        // The family is enumerated by its path, with a page size small enough that the walk
        // really pages: a client resolves the policy by listing the catalog, so the listing
        // is what must report it.
        let listed = walk_policies(
            &router,
            &case,
            &[("Scope", "AWS"), ("PathPrefix", path), ("MaxItems", "4")],
            "Arn",
        )
        .await;
        assert!(
            listed.contains(&arn),
            "case {case:?}: the Scope=AWS listing of {path:?} must report {arn:?}, got {listed:?}"
        );

        // Every member of that page set is in the family the prefix names, and the pages
        // together are the catalog's own entries under it, ascending by ARN.
        let mut expected: Vec<String> = state::AWS_MANAGED_POLICIES
            .iter()
            .filter(|entry| entry.path.starts_with(path))
            .map(|entry| entry.arn())
            .collect();
        expected.sort();
        assert_eq!(
            listed, expected,
            "case {case:?}: expected the catalog's entries under {path:?} in one ascending \
             enumeration, got {listed:?}"
        );

        // And the policy reads back as the record AWS publishes.
        let (status, body) = call(&router, "GetPolicy", &[("PolicyArn", arn.as_str())]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetPolicy must serve a published policy, got {status}: {body}"
        );
        assert_eq!(
            element(&body, "PolicyName"),
            *policy_name,
            "case {case:?}: expected the published friendly name, got {body}"
        );
        assert_eq!(
            element(&body, "Path"),
            *path,
            "case {case:?}: expected the published path, got {body}"
        );
        assert_eq!(
            element(&body, "IsAttachable"),
            is_attachable.to_string(),
            "case {case:?}: expected IsAttachable {is_attachable}, got {body}"
        );
        for reported in ["PolicyId", "DefaultVersionId", "CreateDate", "UpdateDate"] {
            assert!(
                !element(&body, reported).is_empty(),
                "case {case:?}: a listed policy carries <{reported}>, got {body}"
            );
        }

        // The version its `DefaultVersionId` names serves the policy's document, so the
        // grant a client reads off the family is the catalog's and not an empty record.
        let default_version = element(&body, "DefaultVersionId");
        let (status, version) = call(
            &router,
            "GetPolicyVersion",
            &[("PolicyArn", arn.as_str()), ("VersionId", &default_version)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetPolicyVersion must serve {default_version:?}, got {status}: \
             {version}"
        );
        assert!(
            state::is_policy_document(&crate::user_lifecycle::document_element(
                &version, "Document"
            )),
            "case {case:?}: expected an IAM policy document, got {version}"
        );
    }
}

/// RULE: every policy the catalog reports carries the publication data of that policy — the
/// attachability AWS publishes for it, its own id, its own default version and its own two
/// instants — rather than one value stamped on the whole catalog, so two entries of one
/// family differ in them.
#[tokio::test]
async fn each_published_policy_carries_its_own_publication_data() {
    let router = router();
    // The paths the catalog carries, taken from the catalog itself, so a family added later
    // is covered too.
    let mut paths: Vec<&str> = state::AWS_MANAGED_POLICIES
        .iter()
        .map(|entry| entry.path)
        .collect();
    paths.sort_unstable();
    paths.dedup();
    assert!(
        paths.len() >= 5,
        "AWS publishes its managed policies under more than one path, got {paths:?}"
    );

    for path in paths {
        // SOURCE: `state::AWS_MANAGED_POLICIES`, the catalog's own per-entry data, selected
        // by the prefix and put in the ARN order the listing walks.
        let mut selected: Vec<(String, &'static state::AwsManagedPolicy)> =
            state::AWS_MANAGED_POLICIES
                .iter()
                .filter(|entry| entry.path.starts_with(path))
                .map(|entry| (entry.arn(), entry))
                .collect();
        selected.sort_by(|(left, _), (right, _)| left.cmp(right));
        assert!(
            !selected.is_empty(),
            "case {path:?}: the catalog must carry at least one policy under this path"
        );

        for member in ["IsAttachable", "PolicyId", "DefaultVersionId"] {
            let reported = walk_policies(
                &router,
                path,
                &[("Scope", "AWS"), ("PathPrefix", path), ("MaxItems", "5")],
                member,
            )
            .await;
            let expected: Vec<String> = selected
                .iter()
                .map(|(_, entry)| match member {
                    "IsAttachable" => entry.is_attachable.to_string(),
                    "PolicyId" => entry.policy_id.to_string(),
                    _ => entry.default_version_id.to_string(),
                })
                .collect();
            assert_eq!(
                reported, expected,
                "case {path:?}: expected each policy's own <{member}>, got {reported:?}"
            );
        }

        // A family is not one instant repeated: the dates a client sorts the catalog by
        // are the policy's own, and a revised policy's UpdateDate is past its CreateDate.
        let created = walk_policies(
            &router,
            path,
            &[("Scope", "AWS"), ("PathPrefix", path), ("MaxItems", "5")],
            "CreateDate",
        )
        .await;
        let updated = walk_policies(
            &router,
            path,
            &[("Scope", "AWS"), ("PathPrefix", path), ("MaxItems", "5")],
            "UpdateDate",
        )
        .await;
        assert_eq!(
            created.len(),
            selected.len(),
            "case {path:?}: every reported policy carries one CreateDate, got {created:?}"
        );
        assert_eq!(
            updated.len(),
            selected.len(),
            "case {path:?}: every reported policy carries one UpdateDate, got {updated:?}"
        );
        for (index, (arn, _)) in selected.iter().enumerate() {
            assert!(
                created[index] <= updated[index],
                "case {arn:?}: a policy is revised no earlier than it is published, got \
                 CreateDate {:?} and UpdateDate {:?}",
                created[index],
                updated[index]
            );
        }
        if selected.len() > 1 {
            let distinct: ::std::collections::BTreeSet<&String> = created.iter().collect();
            assert!(
                distinct.len() > 1,
                "case {path:?}: two policies of a family are published at their own \
                 instants, got {created:?}"
            );
        }
    }
}

/// How long to wait for the generation the clock owes a report to become due.
fn generation_span() -> ::std::time::Duration {
    ::std::time::Duration::from_secs(state::ACCESS_REPORT_GENERATION_SECONDS as u64 + 1)
}

/// The services a report covers, ascending by namespace — the enumeration order a report is
/// sorted in.
///
/// SOURCE: `state::REPORTED_SERVICES`, the catalog of published service prefixes the service
/// itself reports from, so the expectation spans every entry rather than the handful a
/// fixture could write out.
fn every_namespace() -> Vec<String> {
    state::REPORTED_SERVICES
        .iter()
        .map(|service| service.service_namespace.to_string())
        .collect()
}

/// One page of a report: the namespaces it carried, whether it was truncated, and the marker
/// it handed out.
struct Page {
    namespaces: Vec<String>,
    names: Vec<String>,
    counts: Vec<String>,
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
) -> Page {
    let rendered = max_items.map(|size| size.to_string());
    let mut params: Vec<(&str, &str)> = vec![("JobId", job_id)];
    if let Some(size) = rendered.as_deref() {
        params.push(("MaxItems", size));
    }
    if let Some(token) = marker {
        params.push(("Marker", token));
    }
    let (status, body) = call(router, "GetServiceLastAccessedDetails", &params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetServiceLastAccessedDetails must succeed, got {status}: {body}"
    );
    assert_eq!(
        element(&body, "JobStatus"),
        state::JOB_STATUS_COMPLETED,
        "case {case:?}: expected the generation the clock owed to be done, got {body}"
    );
    let is_truncated = element(&body, "IsTruncated") == "true";
    let marker = optional(&body, "Marker");
    Page {
        namespaces: elements(&body, "ServiceNamespace"),
        names: elements(&body, "ServiceName"),
        counts: elements(&body, "TotalAuthenticatedEntities"),
        is_truncated,
        marker,
    }
}

/// RULE: a completed report enumerates, out of the published catalog of service namespaces,
/// every service the resource's permissions policies allow an action of — ascending by
/// namespace, each entry carrying the service's published name and the number of principals
/// recorded as having attempted to reach it — served WHOLE to a read that asks for no page
/// size, and paged over that one enumeration when `MaxItems` names one, reporting
/// `IsTruncated` with a resumable `Marker` exactly when more remain.
#[tokio::test]
async fn a_report_enumerates_the_published_service_catalog_and_pages_over_it() {
    let router = router();

    // Rows spanning how wide a grant the report's resource holds: one service, the read-only
    // policy AWS publishes (whose grant reaches every service), and the view-only job
    // function (same breadth, narrower verbs) — so the expected enumeration runs from a
    // single entry to the whole catalog.
    let rows: &[(&str, Option<&str>, Option<&str>)] = &[
        (
            "a grant of one service",
            Some(
                r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"s3:GetObject","Resource":"*"}]}"#,
            ),
            None,
        ),
        (
            "the published read-only policy",
            None,
            Some("arn:aws:iam::aws:policy/ReadOnlyAccess"),
        ),
        (
            "the published view-only job function",
            None,
            Some("arn:aws:iam::aws:policy/job-function/ViewOnlyAccess"),
        ),
    ];

    let mut jobs: Vec<(&str, String, Vec<String>)> = Vec::new();
    for (index, (case, inline, attached)) in rows.iter().enumerate() {
        let role_name = format!("catalog-report-{index}");
        let (status, body) = call(
            &router,
            "CreateRole",
            &[
                ("RoleName", role_name.as_str()),
                ("AssumeRolePolicyDocument", TRUST_POLICY),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the fixture CreateRole must succeed, got {status}: {body}"
        );
        let role_arn = element(&body, "Arn");

        if let Some(document) = inline {
            let (status, body) = call(
                &router,
                "PutRolePolicy",
                &[
                    ("RoleName", role_name.as_str()),
                    ("PolicyName", "granted"),
                    ("PolicyDocument", document),
                ],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: the fixture PutRolePolicy must succeed, got {status}: {body}"
            );
        }
        if let Some(policy_arn) = attached {
            let (status, body) = call(
                &router,
                "AttachRolePolicy",
                &[("RoleName", role_name.as_str()), ("PolicyArn", policy_arn)],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: the fixture AttachRolePolicy must succeed, got {status}: {body}"
            );
        }

        let (status, body) = call(
            &router,
            "GenerateServiceLastAccessedDetails",
            &[("Arn", role_arn.as_str())],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: GenerateServiceLastAccessedDetails must succeed, got {status}: {body}"
        );
        let expected = if inline.is_some() {
            vec!["s3".to_string()]
        } else {
            every_namespace()
        };
        jobs.push((case, element(&body, "JobId"), expected));
    }

    tokio::time::sleep(generation_span()).await;

    // The published catalog is wide enough that a report of it runs well past any page size
    // a caller is likely to ask for, which is what makes the paging observable.
    assert!(
        state::REPORTED_SERVICES.len() > state::DEFAULT_MAX_ITEMS as usize,
        "the services a report covers must be the published catalog, not a shortlist, got {}",
        state::REPORTED_SERVICES.len()
    );

    for (case, job_id, expected) in &jobs {
        // Page sizes below, around and above the length of the enumeration, plus the read
        // that asks for no size at all — which is served the report whole.
        for max_items in [Some(3_usize), Some(128), Some(1_000), None] {
            let case = format!("{case} at {max_items:?} per page");
            let size = max_items.unwrap_or(usize::MAX);
            let mut walked: Vec<String> = Vec::new();
            let mut marker: Option<String> = None;
            for page in 0..1_000 {
                let read = report_page(&router, &case, job_id, max_items, marker.as_deref()).await;
                assert!(
                    read.namespaces.len() <= size,
                    "case {case:?} page {page}: a page carries at most MaxItems services, got \
                     {}",
                    read.namespaces.len()
                );
                // Each entry carries the service's published name beside its namespace, and
                // the count of principals recorded as having reached it.
                assert_eq!(
                    read.names,
                    read.namespaces
                        .iter()
                        .map(|namespace| state::reported_service(namespace)
                            .unwrap_or_else(|| panic!(
                                "case {case:?}: {namespace} is a service a report covers"
                            ))
                            .service_name
                            .to_string())
                        .collect::<Vec<String>>(),
                    "case {case:?} page {page}: expected each service's published name, got {:?}",
                    read.names
                );
                assert_eq!(
                    read.counts,
                    vec!["0".to_string(); read.namespaces.len()],
                    "case {case:?} page {page}: expected one attempt count per service, zero \
                     where nothing recorded an attempt, got {:?}",
                    read.counts
                );
                walked.extend(read.namespaces.clone());
                // A `Marker` is handed out exactly when the report is truncated, and it is
                // what the next page resumes after.
                assert_eq!(
                    read.marker.is_some(),
                    read.is_truncated,
                    "case {case:?} page {page}: a Marker is reported exactly when the report \
                     is truncated, got truncated={} marker={:?}",
                    read.is_truncated,
                    read.marker
                );
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
            // The pages together are the whole enumeration, in its one ascending order, with
            // nothing repeated and nothing skipped.
            assert_eq!(
                walked, *expected,
                "case {case:?}: expected the services the resource's policies reach, ascending \
                 by namespace"
            );
            // And truncation is a report of whether more remain, not of the page size: it is
            // true exactly while the enumeration is longer than one page.
            let first = report_page(&router, &case, job_id, max_items, None).await;
            assert_eq!(
                first.is_truncated,
                expected.len() > size,
                "case {case:?}: expected IsTruncated {} for {} services at {size} per page, got \
                 {}",
                expected.len() > size,
                expected.len(),
                first.is_truncated
            );
        }
    }
}
