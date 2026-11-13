// SPDX-License-Identifier: CC0-1.0

//! `ListPolicyVersions`, end to end over the awsQuery wire.
//!
//! Every test drives a `MultiServiceRouter` holding this service over one shared
//! `InMemoryEngine`, exactly as the production router does, so what is asserted is
//! the HTTP behaviour (status, awsQuery error code, XML members) a client on the
//! socket sees. The awsQuery harness is the one `tests/all/user_lifecycle.rs` owns
//! and the other suites in this crate share.
//!
//! The rule being asserted: the listing reports the versions the policy its
//! `PolicyArn` RESOLVES to carries — the account's own policies and AWS's catalog
//! alike — in one total order (NEWEST FIRST: descending by version number, so the
//! most recently created version is the first member and `v1` the last), flags as
//! the default exactly the version the policy's stored default-version id names,
//! withholds each version's `Document`, honours `MaxItems` as a real page size with
//! an opaque `Marker` a follow-up call resumes strictly after, and refuses an ARN
//! that names no readable policy by its kind.
//!
//! Each suite is a TABLE whose rows differ along what that rule generalises over:
//! how many versions a policy carries, which of them is the default, whether the
//! version numbers are contiguous, the policy's owner, the page size (both bounds of
//! `maxItemsType` and the sizes that do and do not divide the set), and the form of
//! a `Marker` or an ARN that names nothing.

use service_com_amazonaws_iam_awsidentitymanagementv20100508::ops::types::PolicyVersion;
use service_com_amazonaws_iam_awsidentitymanagementv20100508::state;
use smithy_server_runtime::MultiServiceRouter;

// `router_with_engine` is the shared harness's router whose engine the test also
// holds: this suite seeds the version sets it reads over — including the sparse
// numbering a run of creates and deletes leaves behind — directly in the service's
// own store, so a listing case needs no fixed order of writes to arrange.
use crate::user_lifecycle::{account_id, call, element, elements, error_code, router_with_engine};

/// The document a policy this account creates carries — an IAM JSON policy, the form
/// `CreatePolicy$PolicyDocument` documents.
const CUSTOMER_DOCUMENT: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"s3:GetObject","Resource":"*"}]}"#;

/// A trust policy with a single statement, which every role fixture needs to exist.
const TRUST_POLICY: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"Service":"glue.amazonaws.com"},"Action":"sts:AssumeRole"}]}"#;

/// One `ListPolicyVersions` call.
async fn list_versions(
    router: &MultiServiceRouter,
    policy_arn: &str,
    params: &[(&str, &str)],
) -> (u16, String) {
    let mut all = vec![("PolicyArn", policy_arn)];
    all.extend_from_slice(params);
    call(router, "ListPolicyVersions", &all).await
}

/// One `ListPolicyVersions` call that must succeed.
async fn listed(router: &MultiServiceRouter, policy_arn: &str, params: &[(&str, &str)]) -> String {
    let (status, body) = list_versions(router, policy_arn, params).await;
    assert_eq!(
        status, 200,
        "case {policy_arn:?} {params:?}: the listing must succeed, got {status}: {body}"
    );
    body
}

/// Create a policy in the ACCOUNT, answering the ARN it reported.
async fn create_policy(router: &MultiServiceRouter, policy_name: &str) -> String {
    let (status, body) = call(
        router,
        "CreatePolicy",
        &[
            ("PolicyName", policy_name),
            ("PolicyDocument", CUSTOMER_DOCUMENT),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {policy_name:?}: CreatePolicy must succeed, got {status}: {body}"
    );
    element(&body, "Arn")
}

/// The number a policy version identifier carries: `policyVersionIdType`'s pattern
/// is `^v[1-9][0-9]*(\.[A-Za-z0-9-]*)?$`
/// (`smithy_query(query="operation", operation="GetPolicyVersion")`), so the number
/// is the digit run after the `v`.
fn version_number(version_id: &str) -> u64 {
    version_id
        .trim_start_matches('v')
        .chars()
        .take_while(char::is_ascii_digit)
        .collect::<String>()
        .parse()
        .unwrap_or_else(|_| panic!("{version_id:?} is not a policy version identifier"))
}

/// Seed the policy `policy_name` names with a version per number in `numbers`,
/// beside the `v1` its `CreatePolicy` minted, and make `default_version_id` the
/// policy's default.
///
/// This is the state a run of `CreatePolicyVersion` ("creates a new version of the
/// specified managed policy") plus a `SetDefaultPolicyVersion` leaves behind, written
/// into the service's OWN store here rather than into a second copy, so a listing case
/// can be arranged at any version numbers — including the sparse numbering only a run of
/// deletes reaches, and `DeletePolicyVersion` is still a scaffolded 501 with its own stub
/// issue. Answers every version identifier the policy
/// then carries, in the order the listing must report them: descending by version
/// number, the newest created first.
fn seed_versions(
    engine: &engine::InMemoryEngine,
    policy_name: &str,
    numbers: &[u64],
    default_version_id: &str,
) -> Vec<String> {
    let store = state::service_state(engine).expect("the IAM store");
    let mut store = store.lock();
    let held = store
        .policies
        .get_mut(&state::name_key(policy_name))
        .unwrap_or_else(|| panic!("the created policy {policy_name:?} must be held"));
    for number in numbers {
        let version_id = format!("v{number}");
        held.versions.insert(
            version_id.clone(),
            PolicyVersion {
                document: Some(format!(
                    r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"Allow","Action":"s3:GetObject","Resource":"arn:aws:s3:::bucket-{number}/*"}}]}}"#
                )),
                version_id: Some(version_id),
                is_default_version: false,
                create_date: Some(state::now()),
            },
        );
    }
    // "The default version is the operative version" — exactly one of a policy's
    // versions is it (`SetDefaultPolicyVersion`).
    for (version_id, version) in &mut held.versions {
        version.is_default_version = version_id == default_version_id;
    }
    held.policy.default_version_id = Some(default_version_id.to_string());
    assert!(
        held.versions.contains_key(default_version_id),
        "case {policy_name:?}: the default version {default_version_id:?} must be one the \
         policy carries"
    );
    let mut ordered: Vec<String> = held.versions.keys().cloned().collect();
    ordered.sort_by_key(|version_id| std::cmp::Reverse(version_number(version_id)));
    ordered
}

/// One policy whose versions the listing must report.
struct VersionsCase {
    name: &'static str,
    /// The version numbers seeded beside the `v1` the create minted.
    seeded: Vec<u64>,
    default_version_id: &'static str,
}

/// RULE: the listing reports every version the policy carries, newest created
/// first, flags exactly the default one, and withholds the document — which
/// `GetPolicyVersion` still serves.
///
/// Rows: the lone version a create minted, a contiguous run, a run whose default is
/// not the first version, and sparse numbers that span the decade boundary (so the
/// order asserted is the version order and not the order the identifiers happen to
/// sort in as text).
#[tokio::test]
async fn the_listing_reports_every_version_in_order_and_withholds_the_document() {
    let cases = vec![
        VersionsCase {
            name: "only the version the create minted",
            seeded: vec![],
            default_version_id: "v1",
        },
        VersionsCase {
            name: "a contiguous run, the first version default",
            seeded: vec![2, 3],
            default_version_id: "v1",
        },
        VersionsCase {
            name: "a contiguous run, a later version default",
            seeded: vec![2, 3, 4, 5],
            default_version_id: "v3",
        },
        VersionsCase {
            name: "sparse numbers across the decade boundary",
            seeded: vec![2, 9, 10, 11, 20, 100],
            default_version_id: "v11",
        },
    ];

    for case in &cases {
        let name = case.name;
        let (router, engine) = router_with_engine();
        let policy_name = "VersionedReader";
        let arn = create_policy(&router, policy_name).await;
        let expected = seed_versions(
            &engine,
            policy_name,
            &case.seeded,
            case.default_version_id,
        );

        let body = listed(&router, &arn, &[]).await;
        assert_eq!(
            elements(&body, "VersionId"),
            expected,
            "case {name:?}: the listing must report every version newest first, got {:?}",
            elements(&body, "VersionId")
        );
        assert_eq!(
            elements(&body, "IsDefaultVersion"),
            expected
                .iter()
                .map(|version_id| (version_id == case.default_version_id).to_string())
                .collect::<Vec<String>>(),
            "case {name:?}: exactly the default version must be flagged as it: {body}"
        );
        assert_eq!(
            elements(&body, "CreateDate").len(),
            expected.len(),
            "case {name:?}: every version must report a CreateDate: {body}"
        );
        assert!(
            elements(&body, "Document").is_empty(),
            "case {name:?}: the document is not returned by this listing, got {:?}",
            elements(&body, "Document")
        );
        assert_eq!(
            element(&body, "IsTruncated"),
            "false",
            "case {name:?}: a page holding every version is not truncated: {body}"
        );
        assert!(
            elements(&body, "Marker").is_empty(),
            "case {name:?}: an untruncated page carries no Marker, got {:?}",
            elements(&body, "Marker")
        );

        // The content the listing withheld is still served by the reader that
        // publishes it, so nothing was lost: one row per version the policy carries.
        for version_id in &expected {
            let (status, read) = call(
                &router,
                "GetPolicyVersion",
                &[("PolicyArn", &arn), ("VersionId", version_id)],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {name:?} {version_id:?}: the listed version must be readable, got \
                 {status}: {read}"
            );
            assert!(
                state::is_policy_document(&crate::user_lifecycle::document_element(
                    &read, "Document"
                )),
                "case {name:?} {version_id:?}: the reader must serve the version's document, \
                 got {read}"
            );
        }
    }
}

/// RULE: a read addressed by `PolicyArn` resolves AWS's own catalog beside the
/// account's policies, so the versions of a policy AWS owns are listed too.
///
/// The table is the catalog itself, so an entry added later is covered as well.
#[tokio::test]
async fn every_aws_managed_policy_lists_the_version_the_catalog_carries() {
    let (router, _engine) = router_with_engine();
    assert!(
        state::AWS_MANAGED_POLICIES.len() >= 2,
        "the catalog must carry more than one policy for this table to span anything, got {}",
        state::AWS_MANAGED_POLICIES.len()
    );

    for entry in state::AWS_MANAGED_POLICIES {
        let arn = entry.arn();
        let case = arn.clone();
        let body = listed(&router, &arn, &[]).await;
        let listed_versions = elements(&body, "VersionId");
        assert_eq!(
            listed_versions.len(),
            1,
            "case {case:?}: the catalog carries one version per policy, got \
             {listed_versions:?}"
        );
        assert_eq!(
            element(&body, "IsDefaultVersion"),
            "true",
            "case {case:?}: the version the catalog carries is the policy's default: {body}"
        );
        assert!(
            elements(&body, "Document").is_empty(),
            "case {case:?}: the document is not returned by this listing, got {:?}",
            elements(&body, "Document")
        );

        // The version this listing named is the one `GetPolicy` publishes as default,
        // so two readers tell one story about the policy.
        let (status, metadata) = call(&router, "GetPolicy", &[("PolicyArn", &arn)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetPolicy must succeed, got {status}: {metadata}"
        );
        assert_eq!(
            listed_versions,
            vec![element(&metadata, "DefaultVersionId")],
            "case {case:?}: the listed version must be the one reported as default: {body}"
        );
    }
}

/// RULE: `MaxItems` is a real page size — absent it defaults to the published
/// default — and the `Marker` a truncated page hands out resumes strictly after the
/// version it was cut at, so a walk sees every version exactly once.
#[tokio::test]
async fn the_listing_pages_through_every_version_exactly_once() {
    let (router, engine) = router_with_engine();
    let policy_name = "PagedPolicy";
    let arn = create_policy(&router, policy_name).await;
    // One version past the default page size, so the default is observable as a page
    // size rather than as "everything".
    let default_page = usize::try_from(state::DEFAULT_MAX_ITEMS).expect("a positive default");
    let seeded: Vec<u64> = (2..=(default_page as u64 + 1)).collect();
    let expected = seed_versions(&engine, policy_name, &seeded, "v1");
    assert_eq!(
        expected.len(),
        default_page + 1,
        "the fixture must carry one version more than a default page"
    );

    // Rows: the smallest page, sizes that do and do not divide the set, the set
    // itself, one past it, and `maxItemsType`'s upper bound of 1000.
    for page_size in [1, 2, 7, default_page, expected.len(), expected.len() + 1, 1000] {
        let mut seen: Vec<String> = Vec::new();
        let mut marker: Option<String> = None;
        loop {
            let size = page_size.to_string();
            let mut params = vec![("MaxItems", size.as_str())];
            if let Some(token) = &marker {
                params.push(("Marker", token.as_str()));
            }
            let body = listed(&router, &arn, &params).await;
            let page = elements(&body, "VersionId");
            assert!(
                page.len() <= page_size,
                "case MaxItems={page_size}: at most {page_size} versions per page, got {}",
                page.len()
            );
            seen.extend(page);
            marker = match element(&body, "IsTruncated").as_str() {
                "true" => Some(element(&body, "Marker")),
                _ => None,
            };
            if marker.is_none() {
                break;
            }
        }
        assert_eq!(
            seen, expected,
            "case MaxItems={page_size}: every version exactly once, newest first, got \
             {seen:?}"
        );
    }

    // "If you do not include this parameter, the number of items defaults to 100"
    // (`ListPolicyVersions$MaxItems`), and the rest is reachable from the Marker.
    let first = listed(&router, &arn, &[]).await;
    assert_eq!(
        elements(&first, "VersionId"),
        expected[..default_page],
        "an absent MaxItems must page at the published default of {default_page}: {first}"
    );
    assert_eq!(
        element(&first, "IsTruncated"),
        "true",
        "a page short of the set must say so: {first}"
    );
    let rest = listed(&router, &arn, &[("Marker", &element(&first, "Marker"))]).await;
    assert_eq!(
        elements(&rest, "VersionId"),
        expected[default_page..],
        "the Marker must resume strictly after the version the page was cut at: {rest}"
    );
    assert_eq!(
        element(&rest, "IsTruncated"),
        "false",
        "the last page is not truncated: {rest}"
    );
}

/// RULE: `MaxItems` outside its published range is refused before the operation
/// runs, so a page size the model forbids never reaches the store.
#[tokio::test]
async fn a_page_size_outside_the_published_range_is_refused() {
    let (router, _engine) = router_with_engine();
    let arn = create_policy(&router, "BoundedPaging").await;
    // One past each published bound of `maxItemsType` (range 1..1000).
    for out_of_range in ["0", "1001", "-1"] {
        let (status, body) = list_versions(&router, &arn, &[("MaxItems", out_of_range)]).await;
        assert_eq!(
            status, 400,
            "case {out_of_range:?}: expected 400, got {status}: {body}"
        );
    }
    // And each bound itself is accepted.
    for in_range in ["1", "1000"] {
        let (status, body) = list_versions(&router, &arn, &[("MaxItems", in_range)]).await;
        assert_eq!(
            status, 200,
            "case {in_range:?}: expected 200, got {status}: {body}"
        );
    }
}

/// RULE: a `Marker` this service did not issue for THIS policy's versions names no
/// position in them, so the call is refused rather than silently answered from the
/// beginning; a token that does decode names a POSITION, which stays usable even
/// when the version it was cut at is gone.
#[tokio::test]
async fn a_marker_this_collection_did_not_issue_is_refused() {
    let (router, engine) = router_with_engine();
    let arn = create_policy(&router, "MarkedPolicy").await;
    let other_arn = create_policy(&router, "OtherMarkedPolicy").await;
    let expected = seed_versions(&engine, "MarkedPolicy", &[2, 3, 4], "v1");
    seed_versions(&engine, "OtherMarkedPolicy", &[2, 3, 4], "v1");

    // A marker this service DID issue — while paging ANOTHER policy's versions, a
    // different position space.
    let other_page = listed(&router, &other_arn, &[("MaxItems", "1")]).await;
    let other_marker = element(&other_page, "Marker");
    // And one it issued for another collection entirely: `ListRoles` pages over the
    // account's roles.
    for role in ["marker.one", "marker.two"] {
        let (status, body) = call(
            &router,
            "CreateRole",
            &[("RoleName", role), ("AssumeRolePolicyDocument", TRUST_POLICY)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {role:?}: CreateRole must succeed, got {status}: {body}"
        );
    }
    let roles_page = call(&router, "ListRoles", &[("MaxItems", "1")]).await.1;
    let roles_marker = element(&roles_page, "Marker");

    for bogus in [
        "not-a-marker",
        "zz",
        "0g",
        other_marker.as_str(),
        roles_marker.as_str(),
    ] {
        let (status, body) = list_versions(&router, &arn, &[("Marker", bogus)]).await;
        assert_eq!(
            status, 400,
            "case {bogus:?}: a foreign marker must be refused, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidInput",
            "case {bogus:?}: the refusal must be InvalidInput: {body}"
        );
    }

    // A marker names a position, not a version: the version it was cut at can go away
    // and the next page is still the versions after that position.
    let page = listed(&router, &arn, &[("MaxItems", "2")]).await;
    assert_eq!(
        elements(&page, "VersionId"),
        expected[..2],
        "the first page must carry the two newest versions: {page}"
    );
    let marker = element(&page, "Marker");
    {
        let store = state::service_state(&*engine).expect("the IAM store");
        let mut store = store.lock();
        let held = store
            .policies
            .get_mut(&state::name_key("MarkedPolicy"))
            .expect("the created policy");
        held.versions
            .remove(&expected[1])
            .expect("the version the page was cut at");
    }
    let resumed = listed(&router, &arn, &[("Marker", &marker)]).await;
    assert_eq!(
        elements(&resumed, "VersionId"),
        expected[2..],
        "a marker whose version is gone still resumes from its position: {resumed}"
    );
}

/// One ARN that names no policy whose versions can be reported, and the answer it
/// must get.
struct MissCase {
    name: &'static str,
    arn: String,
    status: u16,
    code: &'static str,
}

/// RULE: an ARN naming no readable policy is refused by its KIND, always with a code
/// this operation MODELS — an ARN naming no policy the service can report, the
/// account's own or an entry of AWS's catalog, is the missing entity `NoSuchEntity` is
/// for, and a value that is not a managed-policy ARN at all is an invalid input.
#[tokio::test]
async fn an_arn_naming_no_readable_policy_is_refused_by_its_kind() {
    let (router, _engine) = router_with_engine();
    let account = account_id();
    let cases = vec![
        MissCase {
            name: "an account policy the account never created",
            arn: format!("arn:aws:iam::{account}:policy/NeverCreated"),
            status: 404,
            code: "NoSuchEntity",
        },
        MissCase {
            name: "an account policy under a nested path, never created",
            arn: format!("arn:aws:iam::{account}:policy/eng/tools/NeverCreated"),
            status: 404,
            code: "NoSuchEntity",
        },
        MissCase {
            name: "an AWS-managed policy under /job-function/ the catalog does not carry",
            arn: "arn:aws:iam::aws:policy/job-function/NoSuchJobFunctionHere".to_string(),
            status: 404,
            code: "NoSuchEntity",
        },
        MissCase {
            name: "an AWS-managed policy at the root the catalog does not carry",
            arn: "arn:aws:iam::aws:policy/AmazonDynamoDBFullAccess".to_string(),
            status: 404,
            code: "NoSuchEntity",
        },
        MissCase {
            name: "an AWS-managed policy under /service-role/ the catalog does not carry",
            arn: "arn:aws:iam::aws:policy/service-role/NoSuchAwsManagedPolicyHere".to_string(),
            status: 404,
            code: "NoSuchEntity",
        },
        MissCase {
            name: "an ARN of another service",
            arn: "arn:aws:s3:::a-bucket-named-here".to_string(),
            status: 400,
            code: "InvalidInput",
        },
        MissCase {
            name: "an IAM ARN naming a role rather than a policy",
            arn: format!("arn:aws:iam::{account}:role/a-role-not-a-policy"),
            status: 400,
            code: "InvalidInput",
        },
        MissCase {
            name: "a managed-policy ARN carrying a region, which IAM's never do",
            arn: "arn:aws:iam:us-east-1:aws:policy/AdministratorAccess".to_string(),
            status: 400,
            code: "InvalidInput",
        },
        MissCase {
            name: "a managed-policy ARN with an empty path segment",
            arn: "arn:aws:iam::aws:policy//AdministratorAccess".to_string(),
            status: 400,
            code: "InvalidInput",
        },
    ];

    for case in &cases {
        let name = case.name;
        let (status, body) = list_versions(&router, &case.arn, &[]).await;
        assert_eq!(
            status, case.status,
            "case {name:?}: ListPolicyVersions({:?}) must answer {}, got {status}: {body}",
            case.arn, case.status
        );
        assert_eq!(
            error_code(&body),
            case.code,
            "case {name:?}: ListPolicyVersions({:?}) must answer the error code {:?}: {body}",
            case.arn,
            case.code
        );
        // A policy whose versions cannot be reported is worded the way IAM words an
        // unresolvable `PolicyArn` for the family this listing belongs to: the fixed
        // text around the ARN the request supplied, echoed byte for byte.
        if case.code == "NoSuchEntity" {
            assert_eq!(
                element(&body, "Message"),
                format!("Policy {} does not exist or is not attachable.", case.arn),
                "case {name:?}: the refusal must carry IAM's sentence for this family: {body}"
            );
        }
    }
}

/// RULE: a listing is a REPORT — it writes nothing, so what the next caller sees is
/// what the store already held.
#[tokio::test]
async fn listing_changes_nothing_a_later_caller_sees() {
    let (router, engine) = router_with_engine();
    let policy_name = "UnchangedByReads";
    let arn = create_policy(&router, policy_name).await;
    let expected = seed_versions(&engine, policy_name, &[2, 3, 4, 5], "v4");

    let before = listed(&router, &arn, &[]).await;
    // Rows: a whole page, a page size that truncates, and a resumed page — none of
    // them may move the store.
    let truncated = listed(&router, &arn, &[("MaxItems", "2")]).await;
    let _ = listed(&router, &arn, &[("Marker", &element(&truncated, "Marker"))]).await;
    let after = listed(&router, &arn, &[]).await;
    // Every member the listing reports, rather than the whole envelope: the request
    // id is per-call metadata, not part of what the policy carries.
    for member in ["VersionId", "IsDefaultVersion", "CreateDate", "IsTruncated"] {
        assert_eq!(
            elements(&after, member),
            elements(&before, member),
            "case {member:?}: the versions a reader reports must not change because \
             something was read: {after}"
        );
    }
    assert_eq!(
        elements(&after, "VersionId"),
        expected,
        "the store must still carry every version: {after}"
    );

    // And the documents are still there to be read, in full.
    for version_id in &expected {
        let (status, read) = call(
            &router,
            "GetPolicyVersion",
            &[("PolicyArn", &arn), ("VersionId", version_id)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {version_id:?}: the version must still be readable, got {status}: {read}"
        );
    }
}

/// One policy driven to a version set through the API alone, and the version that is
/// operative once every call has run.
struct WireCase {
    name: &'static str,
    policy_name: &'static str,
    /// One row per `CreatePolicyVersion` after the create that minted `v1`: whether it
    /// asked to become the operative version (`CreatePolicyVersion$SetAsDefault`,
    /// default false).
    creates: &'static [bool],
    /// The version a `SetDefaultPolicyVersion` call then selects, when the row makes
    /// one.
    then_set_default: Option<&'static str>,
    /// The version the policy's default-version id must name afterwards.
    operative: &'static str,
}

/// RULE: a reader of a managed policy's versions reports them NEWEST FIRST — the most
/// recently created version is the first member and `v1` the last, on every page of a
/// `Marker` walk — and flags as the default exactly the version the policy's stored
/// default-version id names, whatever position that version holds in the list.
///
/// Every version here is created over the wire, so the order and the flag are asserted
/// against what the operations themselves persisted rather than against a seeded store.
/// Rows differ along the version count (one up to the published per-policy quota),
/// along which version ends up operative (the oldest, one in the middle, the newest)
/// and along how it got there (a create asking for it, a create that did not, and a
/// later set moving it backwards) — a create without `SetAsDefault` must leave the
/// default where it stood.
#[tokio::test]
async fn the_listing_is_newest_first_and_flags_the_stored_default() {
    // "A managed policy can have up to five versions" — the longest row sits exactly on
    // that quota (`crate::state::MAX_POLICY_VERSIONS`).
    let quota = state::MAX_POLICY_VERSIONS;
    let cases = [
        WireCase {
            name: "the lone version the create minted",
            policy_name: "WireOnlyV1",
            creates: &[],
            then_set_default: None,
            operative: "v1",
        },
        WireCase {
            name: "a second version that did not ask to be operative",
            policy_name: "WireSecondNotDefault",
            creates: &[false],
            then_set_default: None,
            operative: "v1",
        },
        WireCase {
            name: "a second version that asked to be operative",
            policy_name: "WireSecondDefault",
            creates: &[true],
            then_set_default: None,
            operative: "v2",
        },
        WireCase {
            name: "a full policy whose newest create took the default",
            policy_name: "WireFullNewestDefault",
            creates: &[false, false, false, true],
            then_set_default: None,
            operative: "v5",
        },
        WireCase {
            name: "a full policy whose default was set back to the middle",
            policy_name: "WireFullMiddleDefault",
            creates: &[true, true, true, true],
            then_set_default: Some("v3"),
            operative: "v3",
        },
        WireCase {
            name: "a full policy whose default never moved off the oldest version",
            policy_name: "WireFullOldestDefault",
            creates: &[false, false, false, false],
            then_set_default: None,
            operative: "v1",
        },
    ];

    for case in &cases {
        let name = case.name;
        let (router, _engine) = router_with_engine();
        let arn = create_policy(&router, case.policy_name).await;
        let held_versions = case.creates.len() + 1;
        assert!(
            held_versions <= quota,
            "case {name:?}: the row must stay inside the published quota of {quota}, asks \
             for {held_versions}"
        );

        for (index, set_as_default) in case.creates.iter().enumerate() {
            let flag = set_as_default.to_string();
            let document = format!(
                r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"Allow","Action":"s3:GetObject","Resource":"arn:aws:s3:::wire-{index}/*"}}]}}"#
            );
            let (status, body) = call(
                &router,
                "CreatePolicyVersion",
                &[
                    ("PolicyArn", &arn),
                    ("PolicyDocument", &document),
                    ("SetAsDefault", &flag),
                ],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {name:?}: the create of version {} must succeed, got {status}: {body}",
                index + 2
            );
        }
        if let Some(operative) = case.then_set_default {
            let (status, body) = call(
                &router,
                "SetDefaultPolicyVersion",
                &[("PolicyArn", &arn), ("VersionId", operative)],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {name:?}: the set of {operative:?} must succeed, got {status}: {body}"
            );
        }

        // What the policy itself publishes as its operative version: the one the flag
        // in the listing has to follow.
        let (status, metadata) = call(&router, "GetPolicy", &[("PolicyArn", &arn)]).await;
        assert_eq!(
            status, 200,
            "case {name:?}: GetPolicy must succeed, got {status}: {metadata}"
        );
        assert_eq!(
            element(&metadata, "DefaultVersionId"),
            case.operative,
            "case {name:?}: the fixture must leave {:?} operative: {metadata}",
            case.operative
        );

        let newest_first: Vec<String> = (1..=case.creates.len() + 1)
            .rev()
            .map(|number| format!("v{number}"))
            .collect();
        let expected_flags: Vec<String> = newest_first
            .iter()
            .map(|version_id| (version_id == case.operative).to_string())
            .collect();

        let body = listed(&router, &arn, &[]).await;
        assert_eq!(
            elements(&body, "VersionId"),
            newest_first,
            "case {name:?}: expected the versions newest first as {newest_first:?}, got {:?}",
            elements(&body, "VersionId")
        );
        assert_eq!(
            elements(&body, "IsDefaultVersion"),
            expected_flags,
            "case {name:?}: expected {:?} alone flagged as the default ({expected_flags:?}), \
             got {:?}",
            case.operative,
            elements(&body, "IsDefaultVersion")
        );

        // The same order and the same flag on every page of a walk at the smallest page
        // size the model admits: a client taking the first member of page one reads the
        // newest version, not the oldest.
        let mut walked: Vec<String> = Vec::new();
        let mut flags: Vec<String> = Vec::new();
        let mut marker: Option<String> = None;
        loop {
            let mut params = vec![("MaxItems", "1")];
            if let Some(token) = &marker {
                params.push(("Marker", token.as_str()));
            }
            let page = listed(&router, &arn, &params).await;
            walked.extend(elements(&page, "VersionId"));
            flags.extend(elements(&page, "IsDefaultVersion"));
            marker = match element(&page, "IsTruncated").as_str() {
                "true" => Some(element(&page, "Marker")),
                _ => None,
            };
            if marker.is_none() {
                break;
            }
        }
        assert_eq!(
            walked, newest_first,
            "case {name:?}: a MaxItems=1 walk must hand out the versions newest first, got \
             {walked:?}"
        );
        assert_eq!(
            flags, expected_flags,
            "case {name:?}: the default flag must travel with its version across the pages, \
             got {flags:?}"
        );

        // The reader that serves a single version tells the same story about which one
        // is operative, so the two readers cannot disagree.
        for (version_id, flag) in newest_first.iter().zip(&expected_flags) {
            let (status, read) = call(
                &router,
                "GetPolicyVersion",
                &[("PolicyArn", &arn), ("VersionId", version_id)],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {name:?} {version_id:?}: the listed version must be readable, got \
                 {status}: {read}"
            );
            assert_eq!(
                &element(&read, "IsDefaultVersion"),
                flag,
                "case {name:?} {version_id:?}: expected IsDefaultVersion {flag:?}, got {read}"
            );
        }
    }
}
