// SPDX-License-Identifier: CC0-1.0

//! `CreatePolicyVersion`, end to end over the awsQuery wire.
//!
//! RULE: a create ADDS a version, carrying the document the request supplied, to the
//! customer managed policy its `PolicyArn` names — numbered one past the greatest
//! version the policy holds, made the operative version exactly when `SetAsDefault`
//! says so — and the version is then readable through `GetPolicyVersion` and listed by
//! `ListPolicyVersions`; a request whose ARN names no policy the account holds is
//! refused by the KIND of the miss (a missing entity, an invalid parameter for an ARN
//! that is not an account policy's), a document that is not an IAM policy or is past
//! the policy size quota is refused, and a policy already carrying its full complement
//! of versions takes no further one.
//!
//! A create is also DECIDED before it writes: IAM authorizes its own API, so a signed
//! create is answered from the signing principal's identity-based policies before the
//! store is consulted for the policy its ARN names; and a create that is served moves
//! the addressed policy's update instant to the instant the version it added was
//! created, leaving that policy's creation instant and every other policy alone.
//!
//! Each suite is a TABLE whose rows differ along what the rule generalises over: the
//! policy's path depth, how many versions it already holds and whether their numbers
//! are contiguous, the document's content, shape and size, whether `SetAsDefault` is
//! absent, false or true, the form of an ARN or a document that is refused, and which
//! statements are in force on the caller.
//!
//! The awsQuery harness is the one `tests/all/user_lifecycle.rs` owns and the other
//! suites in this crate share, so every request here is built and read exactly one way;
//! the signing principal and the documents in force on it are
//! `tests/all/identity_policy_enforcement.rs`'s, as in the sibling signed-write suite
//! `tests/all/policy_tag_authorization.rs`.

use service_com_amazonaws_iam_awsidentitymanagementv20100508::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::identity_policy_enforcement::{principal, statement};
use crate::user_lifecycle::{
    account_id, assert_access_denied_message, call, call_signed_by, element, elements, error_code,
    router,
};

/// A trust policy every fixture `CreateRole` sends.
const TRUST_POLICY: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"Service":"lambda.amazonaws.com"},"Action":"sts:AssumeRole"}]}"#;

/// One policy document per row of the document table: an IAM JSON policy, differing in
/// its statement's content, in its formatting (one line against pretty printed with
/// tabs and newlines) and in how many statements it carries, so what is asserted is the
/// rule and not one document's bytes.
const DOCUMENTS: &[&str] = &[
    r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"s3:GetObject","Resource":"*"}]}"#,
    r#"{"Version":"2012-10-17","Statement":[{"Effect":"Deny","Action":["s3:PutObject","s3:DeleteObject"],"Resource":"arn:aws:s3:::reports/*"}]}"#,
    "{\n\t\"Version\": \"2012-10-17\",\n\t\"Statement\": [\n\t\t{\n\t\t\t\"Effect\": \"Allow\",\n\t\t\t\"Action\": \"sqs:SendMessage\",\n\t\t\t\"Resource\": \"*\"\n\t\t}\n\t]\n}",
    r#"{"Version":"2012-10-17","Id":"two-statements","Statement":[{"Effect":"Allow","Action":"logs:PutLogEvents","Resource":"*"},{"Effect":"Deny","Action":"logs:DeleteLogGroup","Resource":"*"}]}"#,
];

/// A customer managed policy's ARN is `arn:aws:iam::<account>:policy<path><name>` —
/// IAM is global, so the region segment is EMPTY.
fn customer_policy_arn(path: &str, policy_name: &str) -> String {
    format!("arn:aws:iam::{}:policy{path}{policy_name}", account_id())
}

/// Create one customer managed policy under `path` and answer the ARN its create
/// reported, which is the handle every call below addresses it by.
async fn create_policy(router: &MultiServiceRouter, policy_name: &str, path: &str) -> String {
    let (status, body) = call(
        router,
        "CreatePolicy",
        &[
            ("PolicyName", policy_name),
            ("PolicyDocument", DOCUMENTS[0]),
            ("Path", path),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {policy_name:?} at {path:?}: the fixture create must succeed, got {status}: {body}"
    );
    element(&body, "Arn")
}

/// One `CreatePolicyVersion` call: the `SetAsDefault` member is sent only when
/// `set_as_default` names a value, so an absent one exercises the modelled default.
async fn create_version(
    router: &MultiServiceRouter,
    policy_arn: &str,
    document: &str,
    set_as_default: Option<&str>,
) -> (u16, String) {
    let mut params = vec![("PolicyArn", policy_arn), ("PolicyDocument", document)];
    if let Some(flag) = set_as_default {
        params.push(("SetAsDefault", flag));
    }
    call(router, "CreatePolicyVersion", &params).await
}

/// The version identifiers `ListPolicyVersions` reports for `policy_arn`, in the order
/// it reports them, beside the one `GetPolicy` publishes as the policy's default.
async fn versions_and_default(
    router: &MultiServiceRouter,
    policy_arn: &str,
) -> (Vec<String>, Vec<String>, String) {
    let (status, listed) = call(
        router,
        "ListPolicyVersions",
        &[("PolicyArn", policy_arn), ("MaxItems", "1000")],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {policy_arn:?}: ListPolicyVersions must succeed, got {status}: {listed}"
    );
    let (status, metadata) = call(router, "GetPolicy", &[("PolicyArn", policy_arn)]).await;
    assert_eq!(
        status, 200,
        "case {policy_arn:?}: GetPolicy must succeed, got {status}: {metadata}"
    );
    (
        elements(&listed, "VersionId"),
        elements(&listed, "IsDefaultVersion"),
        element(&metadata, "DefaultVersionId"),
    )
}

/// The document `GetPolicyVersion` serves for one version of a policy.
async fn read_document(router: &MultiServiceRouter, policy_arn: &str, version_id: &str) -> String {
    let (status, body) = call(
        router,
        "GetPolicyVersion",
        &[("PolicyArn", policy_arn), ("VersionId", version_id)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {policy_arn:?} {version_id:?}: the created version must be readable, got \
         {status}: {body}"
    );
    crate::user_lifecycle::document_element(&body, "Document")
}

/// A one-statement policy document whose `Resource` carries `filler`, so a document of
/// any measured size can be built from it.
fn padded_document(filler: &str) -> String {
    format!(
        r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"Allow","Action":"s3:GetObject","Resource":"arn:aws:s3:::{filler}/*"}}]}}"#
    )
}

/// A policy document whose measured size is EXACTLY `characters`.
///
/// The measure is the crate's own (`state::policy_document_characters`), the one every
/// policy size quota is taken against, so a row built here sits exactly on a
/// published bound rather than near it.
fn document_of_size(characters: usize) -> String {
    let base = state::policy_document_characters(&padded_document(""));
    let document = padded_document(&"a".repeat(characters - base));
    assert_eq!(
        state::policy_document_characters(&document),
        characters,
        "the fixture document must measure exactly {characters} characters"
    );
    document
}

/// One policy whose versions are created through the API, and the default it must
/// report afterwards.
struct CreateCase {
    name: &'static str,
    policy_name: &'static str,
    path: &'static str,
    /// One row per `CreatePolicyVersion`: the document it sends and the `SetAsDefault`
    /// it carries (`None` omits the member).
    creates: Vec<(&'static str, Option<&'static str>)>,
    /// The version the policy must report as its default once every create has run.
    default_after: &'static str,
}

/// RULE: each create adds a version numbered one past the greatest the policy holds,
/// carrying the document the request supplied and flagged default exactly when
/// `SetAsDefault` asked for it; the response withholds the document, which
/// `GetPolicyVersion` serves, and `ListPolicyVersions` reports every version with
/// exactly one of them default.
///
/// Rows: a single create with `SetAsDefault` absent, false and true; a run of creates
/// that moves the default more than once; and a policy under a nested path, so the rule
/// is exercised on more than one path depth.
#[tokio::test]
async fn each_create_adds_the_next_version_carrying_the_document_it_was_given() {
    let cases = vec![
        CreateCase {
            name: "one create with SetAsDefault absent",
            policy_name: "VersionedAbsentFlag",
            path: "/",
            creates: vec![(DOCUMENTS[1], None)],
            default_after: "v1",
        },
        CreateCase {
            name: "one create with SetAsDefault false",
            policy_name: "VersionedFalseFlag",
            path: "/",
            creates: vec![(DOCUMENTS[2], Some("false"))],
            default_after: "v1",
        },
        CreateCase {
            name: "one create with SetAsDefault true",
            policy_name: "VersionedTrueFlag",
            path: "/",
            creates: vec![(DOCUMENTS[3], Some("true"))],
            default_after: "v2",
        },
        CreateCase {
            name: "a run of creates moving the default twice",
            policy_name: "Versioned_Run+1=a,b.c@x-2",
            path: "/service-role/",
            creates: vec![
                (DOCUMENTS[1], Some("true")),
                (DOCUMENTS[2], Some("false")),
                (DOCUMENTS[3], Some("true")),
                (DOCUMENTS[0], None),
            ],
            default_after: "v4",
        },
        CreateCase {
            name: "a policy under a nested path",
            policy_name: "nested",
            path: "/eng/tools/iam/",
            creates: vec![(DOCUMENTS[2], Some("true")), (DOCUMENTS[3], Some("true"))],
            default_after: "v3",
        },
    ];

    for case in &cases {
        let name = case.name;
        let router = router();
        let arn = create_policy(&router, case.policy_name, case.path).await;

        for (index, (document, set_as_default)) in case.creates.iter().enumerate() {
            // The create minted `v1`, so the nth create of this run is version n+1.
            let expected_id = format!("v{}", index + 2);
            let (status, body) = create_version(&router, &arn, document, *set_as_default).await;
            assert_eq!(
                status, 200,
                "case {name:?} {expected_id:?}: the create must succeed, got {status}: {body}"
            );
            assert_eq!(
                element(&body, "VersionId"),
                expected_id,
                "case {name:?}: the version must be numbered one past the greatest held: {body}"
            );
            assert_eq!(
                element(&body, "IsDefaultVersion"),
                set_as_default.unwrap_or("false"),
                "case {name:?} {expected_id:?}: the version is the operative one exactly when \
                 SetAsDefault asked for it: {body}"
            );
            assert!(
                elements(&body, "Document").is_empty(),
                "case {name:?} {expected_id:?}: this operation does not return the document, \
                 got {:?}",
                elements(&body, "Document")
            );
            assert_eq!(
                elements(&body, "CreateDate").len(),
                1,
                "case {name:?} {expected_id:?}: the version must report a CreateDate: {body}"
            );
            assert_eq!(
                read_document(&router, &arn, &expected_id).await,
                *document,
                "case {name:?} {expected_id:?}: the stored version must carry the document the \
                 request supplied, byte for byte"
            );
        }

        // The listing reports a policy's versions NEWEST FIRST, so the run of creates
        // is expected back in the reverse of the order it ran in.
        let expected_ids: Vec<String> = (1..=case.creates.len() + 1)
            .rev()
            .map(|number| format!("v{number}"))
            .collect();
        let (listed, defaults, default_version) = versions_and_default(&router, &arn).await;
        assert_eq!(
            listed, expected_ids,
            "case {name:?}: the policy must carry every created version, newest first, got \
             {listed:?}"
        );
        assert_eq!(
            default_version, case.default_after,
            "case {name:?}: the policy's operative version must be {:?}, got {default_version:?}",
            case.default_after
        );
        assert_eq!(
            defaults,
            expected_ids
                .iter()
                .map(|version_id| (version_id == case.default_after).to_string())
                .collect::<Vec<String>>(),
            "case {name:?}: exactly the default version must be flagged as it, got {defaults:?}"
        );
        // The version that was default before the last `SetAsDefault` is still there,
        // with its own document: moving the operative version replaces nothing.
        for (index, (document, _)) in case.creates.iter().enumerate() {
            let version_id = format!("v{}", index + 2);
            assert_eq!(
                read_document(&router, &arn, &version_id).await,
                *document,
                "case {name:?} {version_id:?}: every version keeps the document it was created \
                 with"
            );
        }
    }
}

/// RULE: the number a new version carries is one past the GREATEST the policy holds, so
/// a policy whose version numbers are not contiguous still numbers its next version
/// after all of them rather than after however many it happens to carry.
///
/// The gap is made the way AWS's own is: versions the policy carried are removed through
/// `DeletePolicyVersion`, so the fixture arranges the sparse numbering exactly as a
/// client would.
#[tokio::test]
async fn the_next_version_is_numbered_past_the_greatest_the_policy_holds() {
    let router = router();
    let policy_name = "SparselyVersioned";
    let arn = create_policy(&router, policy_name, "/").await;
    for document in DOCUMENTS {
        let (status, body) = create_version(&router, &arn, document, Some("false")).await;
        assert_eq!(
            status, 200,
            "the fixture create must succeed, got {status}: {body}"
        );
    }
    let (listed, _, _) = versions_and_default(&router, &arn).await;
    assert_eq!(
        listed,
        vec!["v5", "v4", "v3", "v2", "v1"],
        "the fixture policy must carry a contiguous run, got {listed:?}"
    );

    // Everything between the first and the last version goes away, leaving `v1` and
    // `v5` held and three numbers unused. Each of them is non-default (every fixture
    // create asked not to become the operative version), so each delete must succeed.
    for version_id in ["v2", "v3", "v4"] {
        let (status, body) = call(
            &router,
            "DeletePolicyVersion",
            &[("PolicyArn", &arn), ("VersionId", version_id)],
        )
        .await;
        assert_eq!(
            status, 200,
            "the fixture delete of {version_id:?} must succeed, got {status}: {body}"
        );
    }

    // Rows: the first create after the gap, and the one after that, so the numbering is
    // asserted to keep climbing rather than to land on one value.
    for expected_id in ["v6", "v7"] {
        let (status, body) = create_version(&router, &arn, DOCUMENTS[0], Some("false")).await;
        assert_eq!(
            status, 200,
            "case {expected_id:?}: the create must succeed, got {status}: {body}"
        );
        assert_eq!(
            element(&body, "VersionId"),
            expected_id,
            "case {expected_id:?}: the new version must be numbered past every version the \
             policy holds: {body}"
        );
    }
    let (listed, _, _) = versions_and_default(&router, &arn).await;
    assert_eq!(
        listed,
        vec!["v7", "v6", "v5", "v1"],
        "the policy must carry the versions it was left with plus the new ones, got {listed:?}"
    );
}

/// RULE: a policy carries at most the published number of versions, so a create past
/// that quota is refused with the modelled limit failure and the policy is left exactly
/// as it was.
///
/// Rows: the creates that fill the quota (each of which must succeed), the first one
/// past it, and a second attempt past it — none of which may add a version.
#[tokio::test]
async fn a_create_past_the_published_version_quota_is_refused() {
    let router = router();
    let arn = create_policy(&router, "QuotaVersioned", "/").await;
    // The create minted one version, so the quota admits that many less one more.
    for filled in 2..=state::MAX_POLICY_VERSIONS {
        let document = DOCUMENTS[filled % DOCUMENTS.len()];
        let (status, body) = create_version(&router, &arn, document, Some("true")).await;
        assert_eq!(
            status, 200,
            "case v{filled}: a create inside the quota must succeed, got {status}: {body}"
        );
    }
    let (before, _, default_before) = versions_and_default(&router, &arn).await;
    assert_eq!(
        before.len(),
        state::MAX_POLICY_VERSIONS,
        "the fixture policy must sit exactly on the quota, got {before:?}"
    );

    for attempt in ["first attempt past the quota", "second attempt past the quota"] {
        let (status, body) = create_version(&router, &arn, DOCUMENTS[0], Some("true")).await;
        assert_eq!(
            status, 409,
            "case {attempt:?}: a create past the quota must answer 409, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "LimitExceeded",
            "case {attempt:?}: the refusal must be LimitExceeded: {body}"
        );
        assert!(
            element(&body, "Message").contains(&state::MAX_POLICY_VERSIONS.to_string()),
            "case {attempt:?}: the message must describe the limit exceeded: {body}"
        );
        let (after, _, default_after) = versions_and_default(&router, &arn).await;
        assert_eq!(
            (after, default_after),
            (before.clone(), default_before.clone()),
            "case {attempt:?}: a refused create must leave the policy exactly as it was"
        );
    }
}

/// One ARN that names no policy of this account that can take a version, and the answer
/// it must get.
struct MissCase {
    name: &'static str,
    arn: String,
    status: u16,
    code: &'static str,
}

/// RULE: an ARN naming no managed policy the account holds is refused by the KIND of the
/// miss — an account policy ARN the account does not hold is the missing entity
/// `NoSuchEntity` is for, named by the ARN the request carried, while a value that is not
/// a customer managed policy's ARN at all (another service's, another IAM entity's, one
/// in AWS's own namespace, which names a policy the account cannot change) is an invalid
/// input parameter.
#[tokio::test]
async fn an_arn_naming_no_account_policy_is_refused_by_its_kind() {
    let router = router();
    // A policy that WAS held and is then released, which is the state a caller proving a
    // dead ARN unusable is in: the ARN is well formed and named a policy a moment ago.
    let deleted = create_policy(&router, "ReleasedPolicy", "/eng/").await;
    let (status, body) = call(&router, "DeletePolicy", &[("PolicyArn", &deleted)]).await;
    assert_eq!(
        status, 200,
        "the fixture delete must succeed, got {status}: {body}"
    );
    // One held policy, so a miss is a miss about THAT ARN rather than about an empty
    // account: the rows below differ from it only in path, name case or name.
    let held = create_policy(&router, "HeldPolicy", "/eng/").await;

    let account = account_id();
    let cases = vec![
        MissCase {
            name: "the ARN of a policy that was just deleted",
            arn: deleted.clone(),
            status: 404,
            code: "NoSuchEntity",
        },
        MissCase {
            name: "an account policy the account never created",
            arn: customer_policy_arn("/", "NeverCreated"),
            status: 404,
            code: "NoSuchEntity",
        },
        MissCase {
            name: "a held policy's name under another path",
            arn: customer_policy_arn("/sales/", "HeldPolicy"),
            status: 404,
            code: "NoSuchEntity",
        },
        MissCase {
            name: "a held policy's name in another case",
            arn: customer_policy_arn("/eng/", "heldpolicy"),
            status: 404,
            code: "NoSuchEntity",
        },
        MissCase {
            name: "an AWS-managed policy the catalog carries",
            arn: state::AWS_MANAGED_POLICIES[0].arn(),
            status: 400,
            code: "InvalidInput",
        },
        MissCase {
            name: "an AWS-managed policy the catalog does not carry",
            arn: "arn:aws:iam::aws:policy/job-function/ViewOnlyAccess".to_string(),
            status: 400,
            code: "InvalidInput",
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
            arn: format!("arn:aws:iam:us-east-1:{account}:policy/HeldPolicy"),
            status: 400,
            code: "InvalidInput",
        },
    ];

    for case in &cases {
        let name = case.name;
        let (status, body) = create_version(&router, &case.arn, DOCUMENTS[0], Some("true")).await;
        assert_eq!(
            status, case.status,
            "case {name:?}: CreatePolicyVersion({:?}) must answer {}, got {status}: {body}",
            case.arn, case.status
        );
        assert_eq!(
            error_code(&body),
            case.code,
            "case {name:?}: CreatePolicyVersion({:?}) must answer the error code {:?}: {body}",
            case.arn,
            case.code
        );
        if case.code == "NoSuchEntity" {
            assert_eq!(
                element(&body, "Message"),
                format!("Policy {} does not exist or is not attachable.", case.arn),
                "case {name:?}: the message must name the policy the request referenced: {body}"
            );
        }
    }

    // The held policy took no version from any of those refusals.
    let (listed, _, default_version) = versions_and_default(&router, &held).await;
    assert_eq!(
        (listed, default_version),
        (vec!["v1".to_string()], "v1".to_string()),
        "a refused create must write nothing to any policy"
    );
}

/// One document a create must refuse, and the answer it must get.
struct DocumentCase {
    name: &'static str,
    document: String,
    status: u16,
    code: &'static str,
}

/// RULE: the document a version carries must be an IAM JSON policy and must sit inside
/// the published managed policy size quota — the same two rules a policy's first
/// document is held to — so a document that is neither is refused and no version is
/// written.
///
/// Rows: text that is not JSON, JSON that is not an object, an object carrying no
/// `Statement`, and a document one character past the quota; and, on the accepting side,
/// a document sitting exactly on the quota.
#[tokio::test]
async fn a_document_that_is_not_an_iam_policy_or_is_past_the_size_quota_is_refused() {
    let router = router();
    let arn = create_policy(&router, "DocumentChecked", "/").await;

    let cases = vec![
        DocumentCase {
            name: "text that is not JSON at all",
            document: "this is not a policy".to_string(),
            status: 400,
            code: "MalformedPolicyDocument",
        },
        DocumentCase {
            name: "JSON that is not an object",
            document: r#"["Statement"]"#.to_string(),
            status: 400,
            code: "MalformedPolicyDocument",
        },
        DocumentCase {
            name: "a JSON object carrying no Statement",
            document: r#"{"Version":"2012-10-17"}"#.to_string(),
            status: 400,
            code: "MalformedPolicyDocument",
        },
        DocumentCase {
            name: "a JSON literal rather than an object",
            document: "null".to_string(),
            status: 400,
            code: "MalformedPolicyDocument",
        },
        DocumentCase {
            name: "one character past the size quota",
            document: document_of_size(state::MAX_MANAGED_POLICY_CHARACTERS + 1),
            status: 409,
            code: "LimitExceeded",
        },
    ];

    for case in &cases {
        let name = case.name;
        let (status, body) = create_version(&router, &arn, &case.document, Some("true")).await;
        assert_eq!(
            status, case.status,
            "case {name:?}: the create must answer {}, got {status}: {body}",
            case.status
        );
        assert_eq!(
            error_code(&body),
            case.code,
            "case {name:?}: the refusal must be {:?}: {body}",
            case.code
        );
        let (listed, _, default_version) = versions_and_default(&router, &arn).await;
        assert_eq!(
            (listed, default_version),
            (vec!["v1".to_string()], "v1".to_string()),
            "case {name:?}: a refused create must add no version"
        );
    }

    // The quota is a bound, not a ban: a document sitting exactly on it is stored, and
    // served back byte for byte.
    let at_quota = document_of_size(state::MAX_MANAGED_POLICY_CHARACTERS);
    let (status, body) = create_version(&router, &arn, &at_quota, Some("true")).await;
    assert_eq!(
        status, 200,
        "a document exactly on the quota must be accepted, got {status}: {body}"
    );
    assert_eq!(
        read_document(&router, &arn, &element(&body, "VersionId")).await,
        at_quota,
        "a document exactly on the quota must be stored as it was supplied"
    );
}

/// RULE: a released policy's ARN names no entity, so every operation addressed by it —
/// a read of the policy and a create of a version alike — reports the missing entity
/// rather than acting on an ARN nothing is held under; and the refusal is the same every
/// time it is asked, so a caller reconciling the deletion can act on it.
///
/// This is the managed-policy lifecycle a client reconciling infrastructure drives:
/// create, attach, detach, delete, then prove the dead ARN unusable. The rows are the
/// paths a policy can live under, so the lifecycle is exercised at more than one depth.
#[tokio::test]
async fn a_version_cannot_be_added_to_a_policy_that_was_deleted() {
    for (policy_name, path) in [
        ("LifecyclePolicy", "/"),
        ("lifecycle-nested", "/eng/tools/"),
        ("Lifecycle_1+a=b,c.d@x-2", "/service-role/"),
    ] {
        let case = format!("{policy_name} at {path}");
        let router = router();
        let role_name = "service-role-under-test";
        let (status, body) = call(
            &router,
            "CreateRole",
            &[
                ("RoleName", role_name),
                ("AssumeRolePolicyDocument", TRUST_POLICY),
                ("Path", path),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the fixture role must be created, got {status}: {body}"
        );
        let arn = create_policy(&router, policy_name, path).await;

        // Attached, then detached: the policy is related to an entity and then is not,
        // which is what lets it be deleted at all.
        for (action, expected_count) in [("AttachRolePolicy", "1"), ("DetachRolePolicy", "0")] {
            let (status, body) = call(
                &router,
                action,
                &[("RoleName", role_name), ("PolicyArn", &arn)],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?} {action:?}: the fixture call must succeed, got {status}: {body}"
            );
            let (status, read) = call(&router, "GetPolicy", &[("PolicyArn", &arn)]).await;
            assert_eq!(
                status, 200,
                "case {case:?} {action:?}: GetPolicy must succeed, got {status}: {read}"
            );
            assert_eq!(
                element(&read, "AttachmentCount"),
                expected_count,
                "case {case:?} {action:?}: the attachment count must follow the relation: {read}"
            );
        }

        let (status, body) = call(&router, "DeletePolicy", &[("PolicyArn", &arn)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the delete must succeed, got {status}: {body}"
        );

        // The ARN resolves nowhere afterwards: the policy reads as a missing entity and
        // is no longer listed under its path.
        let (status, body) = call(&router, "GetPolicy", &[("PolicyArn", &arn)]).await;
        assert_eq!(
            status, 404,
            "case {case:?}: the released policy must read as a missing entity, got \
             {status}: {body}"
        );
        let (status, listed) = call(
            &router,
            "ListPolicies",
            &[("Scope", "Local"), ("PathPrefix", path), ("MaxItems", "1000")],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListPolicies must succeed, got {status}: {listed}"
        );
        assert!(
            !elements(&listed, "Arn").contains(&arn),
            "case {case:?}: the released policy must no longer be listed: {listed}"
        );

        // So a version cannot be added to it — twice, because a reconciliation loop asks
        // again and must get the same answer.
        for attempt in ["first attempt", "second attempt"] {
            let (status, body) = create_version(&router, &arn, DOCUMENTS[1], Some("true")).await;
            assert_eq!(
                status, 404,
                "case {case:?} {attempt}: the dead ARN must answer 404, got {status}: {body}"
            );
            assert_eq!(
                error_code(&body),
                "NoSuchEntity",
                "case {case:?} {attempt}: the refusal must be NoSuchEntity: {body}"
            );
            assert_eq!(
                element(&body, "Message"),
                format!("Policy {arn} does not exist or is not attachable."),
                "case {case:?} {attempt}: the message must name the policy referenced: {body}"
            );
        }

        // A policy created again under that same ARN takes versions as any other does,
        // so the refusal was about the entity being gone and nothing else.
        let recreated = create_policy(&router, policy_name, path).await;
        assert_eq!(
            recreated, arn,
            "case {case:?}: the re-created policy must carry the same ARN"
        );
        let (status, body) = create_version(&router, &arn, DOCUMENTS[1], Some("true")).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the re-created policy must take a version, got {status}: {body}"
        );
        assert_eq!(
            element(&body, "VersionId"),
            "v2",
            "case {case:?}: the version must be numbered after the one the create minted: {body}"
        );
    }
}

/// One `CreatePolicyVersion` presented as signed by `access_key_id`, which is the only
/// place a request carries the credential the caller speaks for.
async fn create_version_signed(
    router: &MultiServiceRouter,
    access_key_id: &str,
    policy_arn: &str,
    document: &str,
) -> (u16, String) {
    call_signed_by(
        router,
        Some(access_key_id),
        "CreatePolicyVersion",
        &[
            ("PolicyArn", policy_arn),
            ("PolicyDocument", document),
            ("SetAsDefault", "true"),
        ],
    )
    .await
}

/// What a signed row expects the create to be answered with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Decided {
    /// HTTP 200: the documents in force permit the create.
    Served,
    /// HTTP 403 `AccessDenied` because a statement about the request says `Deny`.
    ExplicitDeny,
    /// HTTP 403 `AccessDenied` because no statement about the request says `Allow`.
    NoAllow,
}

/// The documents in force on the row's caller, built from the row's own policy ARN —
/// which is only known once that policy exists.
type Policies = Box<dyn Fn(&str) -> Vec<(String, String)>>;

/// RULE: IAM authorizes its own API, so a signed create is decided against the signing
/// principal's identity-based policies before a version is written — a statement about
/// the request that says `Deny` refuses it whatever any `Allow` says, an `Allow` with no
/// such `Deny` serves it, and a principal with neither is refused — and a refusal is HTTP
/// 403 `AccessDenied` naming the principal, the action and the policy the decision was
/// made on, with no version added.
///
/// Rows: whether a statement is in force at all, its effect, whether its action element
/// is a literal, a wildcard run or another policy-writing action, and whether its
/// resource element covers the policy the request addresses.
#[tokio::test]
async fn a_signed_create_is_decided_against_the_callers_identity_policies() {
    let router = router();

    let rows: Vec<(&str, Policies, Decided)> = vec![
        (
            "no identity-based policy at all",
            Box::new(|_arn: &str| Vec::new()),
            Decided::NoAllow,
        ),
        (
            "an Allow naming the action on every resource",
            Box::new(|_arn: &str| {
                vec![(
                    "allow-create-version".to_string(),
                    statement("Allow", "iam:CreatePolicyVersion", "*"),
                )]
            }),
            Decided::Served,
        ),
        (
            "an Allow scoped to the policy the request addresses",
            Box::new(|arn: &str| {
                vec![(
                    "allow-this-policy".to_string(),
                    statement("Allow", "iam:CreatePolicyVersion", arn),
                )]
            }),
            Decided::Served,
        ),
        (
            "an Allow whose action element is a wildcard run",
            Box::new(|_arn: &str| {
                vec![(
                    "allow-create-star".to_string(),
                    statement("Allow", "iam:CreatePolicy*", "*"),
                )]
            }),
            Decided::Served,
        ),
        (
            "an Allow for the deleting action only",
            Box::new(|_arn: &str| {
                vec![(
                    "allow-delete-version".to_string(),
                    statement("Allow", "iam:DeletePolicyVersion", "*"),
                )]
            }),
            Decided::NoAllow,
        ),
        (
            "an Allow scoped to another policy's ARN",
            Box::new(|_arn: &str| {
                vec![(
                    "allow-other-policy".to_string(),
                    statement(
                        "Allow",
                        "iam:CreatePolicyVersion",
                        &customer_policy_arn("/", "SomeOtherPolicy"),
                    ),
                )]
            }),
            Decided::NoAllow,
        ),
        (
            "a Deny naming the action beside an Allow",
            Box::new(|_arn: &str| {
                vec![
                    (
                        "allow-create-version".to_string(),
                        statement("Allow", "iam:CreatePolicyVersion", "*"),
                    ),
                    (
                        "deny-create-version".to_string(),
                        statement("Deny", "iam:CreatePolicyVersion", "*"),
                    ),
                ]
            }),
            Decided::ExplicitDeny,
        ),
        (
            "a Deny scoped to the policy the request addresses",
            Box::new(|arn: &str| {
                vec![
                    (
                        "allow-create-version".to_string(),
                        statement("Allow", "iam:CreatePolicyVersion", "*"),
                    ),
                    (
                        "deny-this-policy".to_string(),
                        statement("Deny", "iam:CreatePolicyVersion", arn),
                    ),
                ]
            }),
            Decided::ExplicitDeny,
        ),
    ];

    for (index, (case, policies, expect)) in rows.iter().enumerate() {
        let policy_name = format!("SignedVersioned{index}");
        let arn = create_policy(&router, &policy_name, "/").await;
        let user_name = format!("versioner{index}");
        let access_key_id = principal(&router, case, &user_name, &policies(&arn)).await;

        let (status, body) = create_version_signed(&router, &access_key_id, &arn, DOCUMENTS[1]).await;

        match expect {
            Decided::Served => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected the create to be served, got {status}: {body}"
                );
                let (listed, _, default_version) = versions_and_default(&router, &arn).await;
                assert_eq!(
                    (listed, default_version),
                    (vec!["v2".to_string(), "v1".to_string()], "v2".to_string()),
                    "case {case:?}: the served create must add the next version and make it \
                     operative"
                );
                assert_eq!(
                    read_document(&router, &arn, "v2").await,
                    DOCUMENTS[1],
                    "case {case:?}: the version stored by a signed create must carry the \
                     document the request supplied"
                );
            }
            Decided::ExplicitDeny | Decided::NoAllow => {
                assert_eq!(
                    status, 403,
                    "case {case:?}: expected HTTP 403, got {status}: {body}"
                );
                assert_eq!(
                    error_code(&body),
                    "AccessDenied",
                    "case {case:?}: expected the error code AccessDenied, got {body}"
                );
                // The message is built from the request: the principal that signed it, the
                // action it named, the policy the decision was made on, and which of the
                // two refusals this is — asserted through the one reader of that wording
                // this crate's test tree holds.
                let reason = if *expect == Decided::ExplicitDeny {
                    "with an explicit deny in an identity-based policy".to_string()
                } else {
                    "because no identity-based policy allows the iam:CreatePolicyVersion action"
                        .to_string()
                };
                assert_access_denied_message(
                    case,
                    &element(&body, "Message"),
                    &format!("arn:aws:iam::{}:user/{user_name}", account_id()),
                    "iam:CreatePolicyVersion",
                    &format!("policy {arn}"),
                    &reason,
                );
                let (listed, _, default_version) = versions_and_default(&router, &arn).await;
                assert_eq!(
                    (listed, default_version),
                    (vec!["v1".to_string()], "v1".to_string()),
                    "case {case:?}: a refused create must add no version"
                );
            }
        }
    }
}

/// RULE: the decision precedes the store lookup, so a caller whose documents do not allow
/// the create is refused with HTTP 403 `AccessDenied` for an ARN the account holds no
/// policy under — learning nothing about whether that policy exists — while the same
/// request from a caller the documents DO allow reaches the lookup and is answered with
/// the missing entity.
///
/// One row per ARN the account holds no policy under, differing in the name and the path
/// the ARN carries.
#[tokio::test]
async fn an_unauthorized_create_is_refused_before_the_policy_is_looked_up() {
    let router = router();

    let absent = [
        (
            "a name at the account root",
            customer_policy_arn("/", "AbsentPolicy"),
        ),
        (
            "a name under a nested path",
            customer_policy_arn("/a/b/c/", "AbsentPolicy"),
        ),
        (
            "a name whose only unusual feature is its case",
            customer_policy_arn("/", "absentpolicy"),
        ),
    ];

    // The caller with no grant: every absent ARN answers the authorization refusal rather
    // than the missing entity, so the refusal reveals nothing about the account's
    // contents.
    let refused_key = principal(&router, "no grant", "unauthorized-versioner", &[]).await;
    for (case, arn) in &absent {
        let (status, body) = create_version_signed(&router, &refused_key, arn, DOCUMENTS[0]).await;
        assert_eq!(
            status, 403,
            "case {case:?}: expected HTTP 403 for {arn:?}, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "AccessDenied",
            "case {case:?}: expected the error code AccessDenied, got {body}"
        );
    }

    // The caller the documents allow reaches the lookup, which is where an ARN naming no
    // held policy becomes the missing entity.
    let allowed_key = principal(
        &router,
        "an Allow on every resource",
        "authorized-versioner",
        &[(
            "allow-create-version".to_string(),
            statement("Allow", "iam:CreatePolicyVersion", "*"),
        )],
    )
    .await;
    for (case, arn) in &absent {
        let (status, body) = create_version_signed(&router, &allowed_key, arn, DOCUMENTS[0]).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected HTTP 404 for {arn:?}, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "NoSuchEntity",
            "case {case:?}: expected the error code NoSuchEntity, got {body}"
        );
    }

    // And that same allowed caller adds a version to a policy the account does hold, so
    // the grant is what the two answers above differ by rather than the request.
    let arn = create_policy(&router, "HeldByTheAccount", "/").await;
    let (status, body) = create_version_signed(&router, &allowed_key, &arn, DOCUMENTS[2]).await;
    assert_eq!(
        status, 200,
        "an allowed caller must add a version to a held policy, got {status}: {body}"
    );
    assert_eq!(
        read_document(&router, &arn, &element(&body, "VersionId")).await,
        DOCUMENTS[2],
        "the version the allowed caller created must carry the document it supplied"
    );
}

/// The instants the policy `policy_arn` names reports right now: its creation instant and
/// its update instant, as `GetPolicy` publishes them.
async fn policy_instants(router: &MultiServiceRouter, policy_arn: &str) -> (String, String) {
    let (status, body) = call(router, "GetPolicy", &[("PolicyArn", policy_arn)]).await;
    assert_eq!(
        status, 200,
        "case {policy_arn:?}: GetPolicy must succeed, got {status}: {body}"
    );
    (element(&body, "CreateDate"), element(&body, "UpdateDate"))
}

/// RULE: `Policy$UpdateDate` is "the date and time when the most recent policy version
/// was created", so a served create moves the addressed policy's update instant to the
/// instant of the version it added, leaves that policy's creation instant where it
/// stands, and touches no other policy — and a refused create moves nothing at all.
///
/// Rows: creates whose `SetAsDefault` is true, false and absent (the update instant
/// follows the creation of a version, not the moving of the operative one), then the
/// refusals — a document that is not an IAM policy, one past the policy size quota, and
/// a create past the version quota.
#[tokio::test]
async fn a_served_create_moves_only_the_addressed_policys_update_instant() {
    let router = router();
    let arn = create_policy(&router, "UpdatedPolicy", "/").await;
    // A bystander policy of the same account: what a create addressed elsewhere must
    // leave untouched.
    let bystander = create_policy(&router, "BystanderPolicy", "/eng/").await;
    let bystander_before = policy_instants(&router, &bystander).await;

    let (created_at, updated_at) = policy_instants(&router, &arn).await;
    assert_eq!(
        created_at, updated_at,
        "a policy carrying only the version its create minted must report its update \
         instant as its creation instant, got {updated_at:?} against {created_at:?}"
    );

    for (case, set_as_default) in [
        ("a create that becomes the operative version", Some("true")),
        ("a create that does not", Some("false")),
        ("a create leaving SetAsDefault absent", None),
    ] {
        let (status, body) = create_version(&router, &arn, DOCUMENTS[1], set_as_default).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the create must succeed, got {status}: {body}"
        );
        let version_created = element(&body, "CreateDate");
        let (reported_create, reported_update) = policy_instants(&router, &arn).await;
        assert_eq!(
            reported_update, version_created,
            "case {case:?}: the policy's update instant must be the instant its most recent \
             version was created, got {reported_update:?} against {version_created:?}"
        );
        assert_eq!(
            reported_create, created_at,
            "case {case:?}: a create must leave the policy's creation instant where it \
             stands, got {reported_create:?}"
        );
    }

    // Fill the policy to the version quota, so one of the refusal rows below is a create
    // the quota refuses rather than the document.
    let (held, _, _) = versions_and_default(&router, &arn).await;
    for filled in held.len() + 1..=state::MAX_POLICY_VERSIONS {
        let (status, body) = create_version(&router, &arn, DOCUMENTS[0], Some("false")).await;
        assert_eq!(
            status, 200,
            "the fixture create of v{filled} must succeed, got {status}: {body}"
        );
    }
    let expected = policy_instants(&router, &arn).await;

    let refusals: Vec<(&str, String)> = vec![
        (
            "a document that is not an IAM policy",
            "not a policy at all".to_string(),
        ),
        (
            "a document past the policy size quota",
            document_of_size(state::MAX_MANAGED_POLICY_CHARACTERS + 1),
        ),
        ("a create past the version quota", DOCUMENTS[3].to_string()),
    ];
    for (case, document) in &refusals {
        let (status, body) = create_version(&router, &arn, document, Some("true")).await;
        assert!(
            status >= 400,
            "case {case:?}: the create must be refused, got {status}: {body}"
        );
        assert_eq!(
            policy_instants(&router, &arn).await,
            expected,
            "case {case:?}: a refused create must leave the policy's instants where they \
             stand"
        );
    }

    assert_eq!(
        policy_instants(&router, &bystander).await,
        bystander_before,
        "a create addressed to another policy must leave this one's instants where they \
         stand"
    );
    let (listed, _, default_version) = versions_and_default(&router, &bystander).await;
    assert_eq!(
        (listed, default_version),
        (vec!["v1".to_string()], "v1".to_string()),
        "a create addressed to another policy must add no version to this one"
    );
}
