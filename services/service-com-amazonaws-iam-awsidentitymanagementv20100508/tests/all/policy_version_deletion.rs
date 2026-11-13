// SPDX-License-Identifier: CC0-1.0

//! `DeletePolicyVersion`, end to end over the awsQuery wire.
//!
//! RULE: a delete removes EXACTLY the one version its `VersionId` names from the
//! customer managed policy its `PolicyArn` names, leaving the policy, its other
//! versions and its operative version as they were, and freeing the number for no
//! reuse; the policy's operative (default) version is not removable this way, so a
//! request naming it is refused as a delete of a resource that still has a subordinate
//! entity; a version the policy does not carry, and an ARN naming no policy the account
//! holds, are refused by the KIND of the miss.
//!
//! Every suite is a TABLE whose rows differ along what the rule generalises over: how
//! many versions the policy holds, which of them is the operative one, the order the
//! deletes are issued in, the policy's path depth, and the form of a version identifier
//! or an ARN that names nothing.
//!
//! The awsQuery harness is the one `tests/all/user_lifecycle.rs` owns and the other
//! suites in this crate share, so every request here is built and read exactly one way.

use service_com_amazonaws_iam_awsidentitymanagementv20100508::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::user_lifecycle::{account_id, call, element, elements, error_code, router};

/// One policy document per fixture version: IAM JSON policies differing in their
/// statement's content, in their formatting and in how many statements they carry, so
/// what a surviving version is asserted to keep is its OWN document rather than one
/// shared string.
const DOCUMENTS: &[&str] = &[
    r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"s3:GetObject","Resource":"*"}]}"#,
    r#"{"Version":"2012-10-17","Statement":[{"Effect":"Deny","Action":["s3:PutObject","s3:DeleteObject"],"Resource":"arn:aws:s3:::reports/*"}]}"#,
    "{\n\t\"Version\": \"2012-10-17\",\n\t\"Statement\": [\n\t\t{\n\t\t\t\"Effect\": \"Allow\",\n\t\t\t\"Action\": \"sqs:SendMessage\",\n\t\t\t\"Resource\": \"*\"\n\t\t}\n\t]\n}",
    r#"{"Version":"2012-10-17","Id":"two-statements","Statement":[{"Effect":"Allow","Action":"logs:PutLogEvents","Resource":"*"},{"Effect":"Deny","Action":"logs:DeleteLogGroup","Resource":"*"}]}"#,
    r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"dynamodb:GetItem","Resource":"arn:aws:dynamodb:::table/orders"}]}"#,
];

/// A customer managed policy's ARN is `arn:aws:iam::<account>:policy<path><name>` —
/// IAM is global, so the region segment is EMPTY.
fn customer_policy_arn(path: &str, policy_name: &str) -> String {
    format!("arn:aws:iam::{}:policy{path}{policy_name}", account_id())
}

/// Create one customer managed policy under `path`, answering the ARN its create
/// reported, which is the handle every call below addresses it by. The create mints
/// `v1` carrying `DOCUMENTS[0]`.
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

/// Add one version to `policy_arn`, which becomes the policy's operative version
/// exactly when `set_as_default` says so, and answer the identifier it was given.
async fn create_version(
    router: &MultiServiceRouter,
    policy_arn: &str,
    document: &str,
    set_as_default: bool,
) -> String {
    let (status, body) = call(
        router,
        "CreatePolicyVersion",
        &[
            ("PolicyArn", policy_arn),
            ("PolicyDocument", document),
            ("SetAsDefault", if set_as_default { "true" } else { "false" }),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {policy_arn:?}: the fixture CreatePolicyVersion must succeed, got {status}: {body}"
    );
    element(&body, "VersionId")
}

/// One `DeletePolicyVersion` call.
async fn delete_version(
    router: &MultiServiceRouter,
    policy_arn: &str,
    version_id: &str,
) -> (u16, String) {
    call(
        router,
        "DeletePolicyVersion",
        &[("PolicyArn", policy_arn), ("VersionId", version_id)],
    )
    .await
}

/// The version identifiers `ListPolicyVersions` reports for `policy_arn`, in the order
/// it reports them.
async fn listed_versions(router: &MultiServiceRouter, policy_arn: &str) -> Vec<String> {
    let (status, body) = call(
        router,
        "ListPolicyVersions",
        &[("PolicyArn", policy_arn), ("MaxItems", "1000")],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {policy_arn:?}: ListPolicyVersions must succeed, got {status}: {body}"
    );
    elements(&body, "VersionId")
}

/// The version `GetPolicy` publishes as the policy's operative one.
async fn default_version(router: &MultiServiceRouter, policy_arn: &str) -> String {
    let (status, body) = call(router, "GetPolicy", &[("PolicyArn", policy_arn)]).await;
    assert_eq!(
        status, 200,
        "case {policy_arn:?}: GetPolicy must succeed, got {status}: {body}"
    );
    element(&body, "DefaultVersionId")
}

/// One `GetPolicyVersion` call, which is how a delete is read back: a version that is
/// gone is no longer served, and one that is still held still carries its document.
async fn get_version(
    router: &MultiServiceRouter,
    policy_arn: &str,
    version_id: &str,
) -> (u16, String) {
    call(
        router,
        "GetPolicyVersion",
        &[("PolicyArn", policy_arn), ("VersionId", version_id)],
    )
    .await
}

/// The message IAM refuses a request naming a version a policy does not carry with.
fn version_not_found(policy_arn: &str, version_id: &str) -> String {
    format!("Policy {policy_arn} version {version_id} does not exist or is not attachable.")
}

/// Assert that `version_id` is not served for `policy_arn`, by the refusal a missing
/// version gets.
async fn assert_version_gone(
    router: &MultiServiceRouter,
    case: &str,
    policy_arn: &str,
    version_id: &str,
) {
    let (status, body) = get_version(router, policy_arn, version_id).await;
    assert_eq!(
        status, 404,
        "case {case:?}: the deleted version {version_id:?} must no longer be served, got \
         {status}: {body}"
    );
    assert_eq!(
        error_code(&body),
        "NoSuchEntity",
        "case {case:?}: reading the deleted version {version_id:?} must answer NoSuchEntity: \
         {body}"
    );
    assert_eq!(
        element(&body, "Message"),
        version_not_found(policy_arn, version_id),
        "case {case:?}: the message must name the policy and the version referenced: {body}"
    );
}

/// One policy the suite arranges: the versions it is given beyond the `v1` its create
/// minted, and the versions a run of deletes then removes.
struct DeleteCase {
    name: &'static str,
    policy_name: &'static str,
    path: &'static str,
    /// One row per fixture `CreatePolicyVersion`: the document it sends and whether it
    /// asks to become the operative version.
    creates: &'static [(usize, bool)],
    /// The version the policy must report as its operative one once the fixture is
    /// arranged — and still, unchanged, after every delete below.
    default_version: &'static str,
    /// The versions the suite deletes, in the order it issues the deletes.
    deletes: &'static [&'static str],
    /// The versions the policy must carry afterwards, newest first — the order the
    /// listing reports them in.
    remaining: &'static [&'static str],
}

/// RULE: a delete removes exactly the version it names and nothing else — the policy
/// stays, its operative version stays what it was, and every other version still
/// carries its own document — while the deleted version is served to nobody afterwards.
///
/// Rows: the newest version of two; the version that was operative until the operative
/// one moved; two non-default versions taken lowest-first and highest-first; a version
/// in the middle of a policy under a nested path; and every version but the operative
/// one, which is the state `DeletePolicy` then takes over from.
#[tokio::test]
async fn a_delete_removes_exactly_the_version_it_names() {
    let cases = &[
        DeleteCase {
            name: "the newest version of two",
            policy_name: "DeleteNewest",
            path: "/",
            creates: &[(1, false)],
            default_version: "v1",
            deletes: &["v2"],
            remaining: &["v1"],
        },
        DeleteCase {
            name: "the version that was operative until the operative one moved",
            policy_name: "DeleteFormerDefault",
            path: "/",
            creates: &[(1, true)],
            default_version: "v2",
            deletes: &["v1"],
            remaining: &["v2"],
        },
        DeleteCase {
            name: "two non-default versions, lowest first",
            policy_name: "DeleteAscending",
            path: "/eng/",
            creates: &[(1, false), (2, false)],
            default_version: "v1",
            deletes: &["v2", "v3"],
            remaining: &["v1"],
        },
        DeleteCase {
            name: "two non-default versions, highest first",
            policy_name: "Delete_Descending+1=a,b.c@x-2",
            path: "/eng/",
            creates: &[(1, false), (2, false)],
            default_version: "v1",
            deletes: &["v3", "v2"],
            remaining: &["v1"],
        },
        DeleteCase {
            name: "the versions around the operative one, under a nested path",
            policy_name: "DeleteAroundDefault",
            path: "/eng/tools/iam/",
            creates: &[(1, false), (2, true), (3, false)],
            default_version: "v3",
            deletes: &["v2", "v4"],
            remaining: &["v3", "v1"],
        },
        DeleteCase {
            name: "every version but the operative one",
            policy_name: "DeleteAllButDefault",
            path: "/sales/west/",
            creates: &[(1, false), (2, false), (3, false), (4, false)],
            default_version: "v1",
            deletes: &["v3", "v5", "v2", "v4"],
            remaining: &["v1"],
        },
    ];

    for case in cases {
        let name = case.name;
        let router = router();
        let arn = create_policy(&router, case.policy_name, case.path).await;
        // The document each held version carries, keyed by its identifier, so a
        // surviving version is asserted to keep its own rather than any document.
        let mut documents = vec![("v1".to_string(), DOCUMENTS[0])];
        for (document, set_as_default) in case.creates {
            let version_id =
                create_version(&router, &arn, DOCUMENTS[*document], *set_as_default).await;
            documents.push((version_id, DOCUMENTS[*document]));
        }
        assert_eq!(
            default_version(&router, &arn).await,
            case.default_version,
            "case {name:?}: the fixture must leave {:?} operative",
            case.default_version
        );

        for version_id in case.deletes {
            let before = listed_versions(&router, &arn).await;
            let (status, body) = delete_version(&router, &arn, version_id).await;
            assert_eq!(
                status, 200,
                "case {name:?} {version_id:?}: deleting a non-default version must succeed, got \
                 {status}: {body}"
            );
            assert_version_gone(&router, name, &arn, version_id).await;

            let after = listed_versions(&router, &arn).await;
            let expected: Vec<String> = before
                .iter()
                .filter(|held| held.as_str() != *version_id)
                .cloned()
                .collect();
            assert_eq!(
                after, expected,
                "case {name:?} {version_id:?}: exactly the deleted version may leave the policy, \
                 got {after:?}"
            );
            assert_eq!(
                default_version(&router, &arn).await,
                case.default_version,
                "case {name:?} {version_id:?}: a delete must leave the operative version where \
                 it was"
            );
        }

        let remaining = listed_versions(&router, &arn).await;
        assert_eq!(
            remaining,
            case.remaining
                .iter()
                .map(|version_id| version_id.to_string())
                .collect::<Vec<String>>(),
            "case {name:?}: the policy must carry the versions no delete named, newest \
             first, got {remaining:?}"
        );
        for (version_id, document) in &documents {
            if case.deletes.contains(&version_id.as_str()) {
                continue;
            }
            let (status, body) = get_version(&router, &arn, version_id).await;
            assert_eq!(
                status, 200,
                "case {name:?} {version_id:?}: a version no delete named must still be served, \
                 got {status}: {body}"
            );
            assert_eq!(
                crate::user_lifecycle::document_element(&body, "Document"),
                *document,
                "case {name:?} {version_id:?}: a surviving version keeps the document it was \
                 created with: {body}"
            );
        }
    }
}

/// One attempt to delete the operative version of a policy, and the state the policy is
/// in when it is made.
struct DefaultCase {
    name: &'static str,
    policy_name: &'static str,
    /// One row per fixture `CreatePolicyVersion`, as in [`DeleteCase`].
    creates: &'static [(usize, bool)],
    /// The versions the fixture deletes before the attempt, which must all succeed.
    deletes_first: &'static [&'static str],
    /// The version the attempt names, which is the policy's operative one.
    attempt: &'static str,
}

/// RULE: the operative version of a policy is not removable through this operation —
/// it belongs to the policy, which `DeletePolicy` removes — so a request naming it is
/// refused with the modelled delete conflict and leaves the policy exactly as it was.
///
/// Rows: the only version a policy has; the operative version of a policy that also
/// carries others; one the operative flag was moved onto; and the last version left
/// after every other has been deleted.
#[tokio::test]
async fn the_operative_version_is_not_removable() {
    let cases = &[
        DefaultCase {
            name: "the only version a policy has",
            policy_name: "DefaultOnlyVersion",
            creates: &[],
            deletes_first: &[],
            attempt: "v1",
        },
        DefaultCase {
            name: "the operative version of a policy carrying others",
            policy_name: "DefaultAmongMany",
            creates: &[(1, false), (2, false)],
            deletes_first: &[],
            attempt: "v1",
        },
        DefaultCase {
            name: "a version the operative flag was moved onto",
            policy_name: "DefaultMoved",
            creates: &[(1, false), (2, true)],
            deletes_first: &[],
            attempt: "v3",
        },
        DefaultCase {
            name: "the last version left after the others were deleted",
            policy_name: "DefaultLastStanding",
            creates: &[(1, true), (2, false)],
            deletes_first: &["v1", "v3"],
            attempt: "v2",
        },
    ];

    for case in cases {
        let name = case.name;
        let router = router();
        let arn = create_policy(&router, case.policy_name, "/").await;
        for (document, set_as_default) in case.creates {
            create_version(&router, &arn, DOCUMENTS[*document], *set_as_default).await;
        }
        for version_id in case.deletes_first {
            let (status, body) = delete_version(&router, &arn, version_id).await;
            assert_eq!(
                status, 200,
                "case {name:?}: the fixture delete of {version_id:?} must succeed, got {status}: \
                 {body}"
            );
        }
        let before = listed_versions(&router, &arn).await;
        assert_eq!(
            default_version(&router, &arn).await,
            case.attempt,
            "case {name:?}: the fixture must leave {:?} operative",
            case.attempt
        );

        // The refusal is the same however often it is asked for, and it changes nothing.
        for attempt in ["first attempt", "second attempt"] {
            let (status, body) = delete_version(&router, &arn, case.attempt).await;
            assert_eq!(
                status, 409,
                "case {name:?} {attempt}: deleting the operative version must answer 409, got \
                 {status}: {body}"
            );
            assert_eq!(
                error_code(&body),
                "DeleteConflict",
                "case {name:?} {attempt}: the refusal must be DeleteConflict: {body}"
            );
            assert_eq!(
                element(&body, "Message"),
                "Cannot delete the default version of a policy.",
                "case {name:?} {attempt}: the message must say the operative version cannot be \
                 deleted: {body}"
            );
            assert_eq!(
                listed_versions(&router, &arn).await,
                before,
                "case {name:?} {attempt}: a refused delete must leave the policy's versions as \
                 they were"
            );
            assert_eq!(
                default_version(&router, &arn).await,
                case.attempt,
                "case {name:?} {attempt}: a refused delete must leave the operative version \
                 where it was"
            );
        }
    }
}

/// RULE: a `VersionId` the policy carries no version under referenced an entity that
/// does not exist, named by the policy and the version the request supplied.
///
/// Rows: a number past every version held; a number inside the run that was never
/// created; a version deleted a moment ago; and an identifier carrying the optional
/// suffix `policyVersionIdType`'s pattern admits, which names no version either.
#[tokio::test]
async fn a_version_the_policy_does_not_carry_is_a_missing_entity() {
    let router = router();
    let arn = create_policy(&router, "MissingVersions", "/eng/").await;
    // Two more versions, the second of which is deleted again: `v1` and `v2` are held,
    // `v3` was held a moment ago and nothing else ever was.
    create_version(&router, &arn, DOCUMENTS[1], false).await;
    create_version(&router, &arn, DOCUMENTS[2], false).await;
    let (status, body) = delete_version(&router, &arn, "v3").await;
    assert_eq!(
        status, 200,
        "the fixture delete must succeed, got {status}: {body}"
    );

    // `policyVersionIdType`'s pattern is `^v[1-9][0-9]*(\.[A-Za-z0-9-]*)?$`
    // (`smithy_query(query="operation", operation="DeletePolicyVersion")`), so each row
    // here is a well-formed identifier that reaches the handler.
    let cases = &[
        ("a version deleted a moment ago", "v3"),
        ("the next number the policy would mint", "v4"),
        ("a number far past every version held", "v97"),
        ("a held version's number with the optional suffix", "v2.1"),
        ("a two-digit number nothing was created under", "v10"),
    ];
    for (name, version_id) in cases {
        let (status, body) = delete_version(&router, &arn, version_id).await;
        assert_eq!(
            status, 404,
            "case {name:?}: deleting {version_id:?} must answer 404, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "NoSuchEntity",
            "case {name:?}: the refusal must be NoSuchEntity: {body}"
        );
        assert_eq!(
            element(&body, "Message"),
            version_not_found(&arn, version_id),
            "case {name:?}: the message must name the policy and the version referenced: {body}"
        );
        assert_eq!(
            listed_versions(&router, &arn).await,
            vec!["v2", "v1"],
            "case {name:?}: a refused delete must leave the policy's versions as they were"
        );
    }
}

/// One ARN that names no policy of this account a version can be deleted from, and the
/// answer it must get.
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
    create_version(&router, &held, DOCUMENTS[1], false).await;

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
        let (status, body) = delete_version(&router, &case.arn, "v2").await;
        assert_eq!(
            status, case.status,
            "case {name:?}: DeletePolicyVersion({:?}) must answer {}, got {status}: {body}",
            case.arn, case.status
        );
        assert_eq!(
            error_code(&body),
            case.code,
            "case {name:?}: DeletePolicyVersion({:?}) must answer the error code {:?}: {body}",
            case.arn,
            case.code
        );
        if case.code == "NoSuchEntity" {
            assert_eq!(
                element(&body, "Message"),
                format!("Policy {} was not found.", case.arn),
                "case {name:?}: the message must name the policy the request referenced: {body}"
            );
        }
        // The held policy is untouched by every row, whichever way the row was refused.
        assert_eq!(
            listed_versions(&router, &held).await,
            vec!["v2", "v1"],
            "case {name:?}: a refused delete must leave the account's policies as they were"
        );
    }
}

/// RULE: a delete frees a slot against the published per-policy version quota, so a
/// policy that was full takes a new version after one is deleted — and that version is
/// numbered one past the GREATEST number the policy still holds, never by reusing the
/// deleted one.
#[tokio::test]
async fn a_delete_frees_a_slot_against_the_version_quota() {
    let router = router();
    let arn = create_policy(&router, "QuotaFreedByDelete", "/").await;
    // The create minted one version, so the quota admits that many less one more.
    for filled in 2..=state::MAX_POLICY_VERSIONS {
        create_version(&router, &arn, DOCUMENTS[filled % DOCUMENTS.len()], false).await;
    }
    let full = listed_versions(&router, &arn).await;
    assert_eq!(
        full.len(),
        state::MAX_POLICY_VERSIONS,
        "the fixture policy must sit exactly on the quota, got {full:?}"
    );

    // Rows: the version in the middle of the run, then the greatest one, so the next
    // number is asserted against what the policy still holds rather than against one
    // arrangement.
    for (case, deleted, expected_next) in [
        ("a version in the middle of the run", "v3", "v6"),
        ("another version in the middle of the run", "v4", "v7"),
    ] {
        let (status, body) = call(
            &router,
            "CreatePolicyVersion",
            &[
                ("PolicyArn", &arn),
                ("PolicyDocument", DOCUMENTS[1]),
                ("SetAsDefault", "false"),
            ],
        )
        .await;
        assert_eq!(
            status, 409,
            "case {case:?}: a create against a full policy must answer 409, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "LimitExceeded",
            "case {case:?}: the refusal must be LimitExceeded: {body}"
        );

        let (status, body) = delete_version(&router, &arn, deleted).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the delete of {deleted:?} must succeed, got {status}: {body}"
        );
        let minted = create_version(&router, &arn, DOCUMENTS[2], false).await;
        assert_eq!(
            minted, expected_next,
            "case {case:?}: the version a freed slot takes must be numbered past every version \
             the policy holds, got {minted:?}"
        );
        assert_eq!(
            listed_versions(&router, &arn).await.len(),
            state::MAX_POLICY_VERSIONS,
            "case {case:?}: the policy must sit on the quota again"
        );
    }
}
