// SPDX-License-Identifier: CC0-1.0

//! `SetDefaultPolicyVersion`, end to end over the awsQuery wire.
//!
//! RULE: the call makes one version a customer managed policy ALREADY carries that
//! policy's default (operative) version — exactly one version of a policy is operative,
//! so the one that was operative stops being it, every version's document and creation
//! instant and the policy's own dates are left where they stand, and the operative
//! version is what is in force for the entities the policy is attached to. A request is
//! refused by the KIND of its miss: a value that is not a customer managed policy's ARN
//! is an invalid input parameter, while an ARN the account holds no policy under and a
//! version identifier the policy does not carry are missing entities; a refused call
//! moves nothing.
//!
//! Each suite is a TABLE whose rows differ along what the rule generalises over: the
//! policy's path depth, how many versions it carries, which version was operative before
//! the call, the direction and length of the sequence of versions made operative
//! (including setting the one that is already operative), and the form of an ARN or a
//! version identifier that is refused.
//!
//! The awsQuery harness is the one `tests/all/user_lifecycle.rs` owns and the other
//! suites in this crate share, so every request here is built and read exactly one way.

use service_com_amazonaws_iam_awsidentitymanagementv20100508::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::identity_policy_enforcement::{principal, statement};
use crate::user_lifecycle::{
    account_id, assert_access_denied_message, call, call_signed_by, element, elements, error_code,
    router,
};

/// One IAM JSON policy document per version a case creates, differing in the statement's
/// effect, in how many actions and statements it carries and in its formatting, so what
/// is asserted is the rule and not one document's bytes. The first is the document every
/// fixture `CreatePolicy` sends, so it is the content of each policy's `v1`.
const DOCUMENTS: &[&str] = &[
    r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"s3:GetObject","Resource":"*"}]}"#,
    r#"{"Version":"2012-10-17","Statement":[{"Effect":"Deny","Action":["s3:PutObject","s3:DeleteObject"],"Resource":"arn:aws:s3:::reports/*"}]}"#,
    "{\n\t\"Version\": \"2012-10-17\",\n\t\"Statement\": [\n\t\t{\n\t\t\t\"Effect\": \"Allow\",\n\t\t\t\"Action\": \"sqs:SendMessage\",\n\t\t\t\"Resource\": \"*\"\n\t\t}\n\t]\n}",
    r#"{"Version":"2012-10-17","Id":"two-statements","Statement":[{"Effect":"Allow","Action":"logs:PutLogEvents","Resource":"*"},{"Effect":"Deny","Action":"logs:DeleteLogGroup","Resource":"*"}]}"#,
    r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"dynamodb:*","Resource":"arn:aws:dynamodb:us-east-1:123456789012:table/orders"}]}"#,
];

/// A customer managed policy's ARN is `arn:aws:iam::<account>:policy<path><name>` — IAM
/// is global, so the region segment is EMPTY.
fn customer_policy_arn(path: &str, policy_name: &str) -> String {
    format!("arn:aws:iam::{}:policy{path}{policy_name}", account_id())
}

/// Create one customer managed policy under `path`, carrying `DOCUMENTS[0]` as its `v1`,
/// and answer the ARN its create reported.
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
        "case {policy_name:?} at {path:?}: the fixture CreatePolicy must succeed, \
         got {status}: {body}"
    );
    element(&body, "Arn")
}

/// Add one version carrying `document` to the policy `policy_arn` names, made the
/// policy's default only when `as_default` says so, and answer its version identifier.
async fn add_version(
    router: &MultiServiceRouter,
    policy_arn: &str,
    document: &str,
    as_default: bool,
) -> String {
    let flag = if as_default { "true" } else { "false" };
    let (status, body) = call(
        router,
        "CreatePolicyVersion",
        &[
            ("PolicyArn", policy_arn),
            ("PolicyDocument", document),
            ("SetAsDefault", flag),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {policy_arn:?}: the fixture CreatePolicyVersion must succeed, got {status}: {body}"
    );
    element(&body, "VersionId")
}

/// One `SetDefaultPolicyVersion` call.
async fn set_default(
    router: &MultiServiceRouter,
    policy_arn: &str,
    version_id: &str,
) -> (u16, String) {
    call(
        router,
        "SetDefaultPolicyVersion",
        &[("PolicyArn", policy_arn), ("VersionId", version_id)],
    )
    .await
}

/// What the policy `policy_arn` names reports about its versions right now: the version
/// identifier `GetPolicy` publishes as the default, the identifiers `ListPolicyVersions`
/// reports in the order it reports them, the `IsDefaultVersion` flag beside each, and the
/// policy's own creation and update instants.
struct Reported {
    default_version_id: String,
    version_ids: Vec<String>,
    default_flags: Vec<String>,
    create_date: String,
    update_date: String,
}

/// Read [`Reported`] back through the implemented readers of a managed policy.
async fn reported(router: &MultiServiceRouter, case: &str, policy_arn: &str) -> Reported {
    let (status, listed) = call(
        router,
        "ListPolicyVersions",
        &[("PolicyArn", policy_arn), ("MaxItems", "1000")],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListPolicyVersions must succeed, got {status}: {listed}"
    );
    let (status, metadata) = call(router, "GetPolicy", &[("PolicyArn", policy_arn)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetPolicy must succeed, got {status}: {metadata}"
    );
    Reported {
        default_version_id: element(&metadata, "DefaultVersionId"),
        version_ids: elements(&listed, "VersionId"),
        default_flags: elements(&listed, "IsDefaultVersion"),
        create_date: element(&metadata, "CreateDate"),
        update_date: element(&metadata, "UpdateDate"),
    }
}

/// What `GetPolicyVersion` serves for one version: its document and its default flag.
async fn read_version(
    router: &MultiServiceRouter,
    case: &str,
    policy_arn: &str,
    version_id: &str,
) -> (String, String) {
    let (status, body) = call(
        router,
        "GetPolicyVersion",
        &[("PolicyArn", policy_arn), ("VersionId", version_id)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetPolicyVersion({version_id}) must succeed, got {status}: {body}"
    );
    (
        crate::user_lifecycle::document_element(&body, "Document"),
        element(&body, "IsDefaultVersion"),
    )
}

/// One policy a case drives: how many versions beside its `v1` it carries, whether those
/// creates moved the default as they went, and the sequence of versions the case then
/// makes operative.
struct DefaultCase {
    name: &'static str,
    policy_name: &'static str,
    path: &'static str,
    /// How many versions are added beside the `v1` `CreatePolicy` minted, each carrying
    /// the next entry of [`DOCUMENTS`].
    added: usize,
    /// Whether those creates asked to become the default, which is where the operative
    /// version sits before the first set: the last added version when true, `v1` when
    /// false.
    added_as_default: bool,
    /// The version identifiers made operative, in order.
    sequence: &'static [&'static str],
}

/// The rows: a two-version policy at the root moved forward, back and forward again; a
/// five-version policy under a nested path moved from the top downwards; a policy whose
/// operative version starts at `v1` and is set to it again before being moved; and one
/// whose creates left the LAST version operative, so the first set moves the default
/// backwards. Together they span the path depth, the version count, where the default
/// starts, the direction of a move and the length of the sequence.
fn cases() -> Vec<DefaultCase> {
    vec![
        DefaultCase {
            name: "a two-version policy at the root, moved there and back",
            policy_name: "RootVersioned",
            path: "/",
            added: 1,
            added_as_default: false,
            sequence: &["v2", "v1", "v2"],
        },
        DefaultCase {
            name: "a five-version policy under a nested path, moved from the top down",
            policy_name: "DeeplyVersioned",
            path: "/eng/tools/",
            added: 4,
            added_as_default: false,
            sequence: &["v5", "v3", "v1", "v4"],
        },
        DefaultCase {
            name: "the version that is already operative, twice over",
            policy_name: "AlreadyOperative",
            path: "/eng/",
            added: 2,
            added_as_default: false,
            sequence: &["v1", "v1", "v3", "v3", "v2"],
        },
        DefaultCase {
            name: "a policy whose creates left the last version operative",
            policy_name: "CreatedAsDefault",
            path: "/sales/west/",
            added: 3,
            added_as_default: true,
            sequence: &["v1", "v4", "v2"],
        },
    ]
}

/// RULE: `SetDefaultPolicyVersion` makes the version it names the policy's one operative
/// version — `GetPolicy` publishes it as the default, `ListPolicyVersions` and
/// `GetPolicyVersion` flag exactly that version and no other — while every version's
/// document and the policy's own creation and update instants stay where they stand,
/// because the call creates no version.
#[tokio::test]
async fn each_call_makes_the_named_held_version_the_only_operative_one() {
    let router = router();

    for case in cases() {
        let name = case.name;
        let arn = create_policy(&router, case.policy_name, case.path).await;

        // The documents each version carries, by version identifier, so a later read is
        // compared against what was stored rather than against itself.
        let mut documents = vec![("v1".to_string(), DOCUMENTS[0].to_string())];
        for added in 0..case.added {
            let document = DOCUMENTS[(added + 1) % DOCUMENTS.len()];
            let version_id = add_version(&router, &arn, document, case.added_as_default).await;
            documents.push((version_id, document.to_string()));
        }
        let version_ids: Vec<String> = documents
            .iter()
            .map(|(version_id, _)| version_id.clone())
            .collect();
        // The listing reports a policy's versions NEWEST FIRST, the reverse of the
        // order the creates above added them in.
        let listed_order: Vec<String> = version_ids.iter().rev().cloned().collect();

        // Where the default sits before any set: the last create's version when the
        // creates asked for it, else the `v1` the policy was created with.
        let before = reported(&router, name, &arn).await;
        let expected_initial = if case.added_as_default {
            version_ids.last().expect("at least v1").clone()
        } else {
            "v1".to_string()
        };
        assert_eq!(
            before.default_version_id, expected_initial,
            "case {name:?}: the fixture must leave {expected_initial} operative, got {:?}",
            before.default_version_id
        );

        for operative in case.sequence {
            let (status, body) = set_default(&router, &arn, operative).await;
            assert_eq!(
                status, 200,
                "case {name:?}: SetDefaultPolicyVersion({operative}) must be served, \
                 got {status}: {body}"
            );

            let after = reported(&router, name, &arn).await;
            assert_eq!(
                after.default_version_id, *operative,
                "case {name:?}: expected GetPolicy to publish {operative} as the default, \
                 got {:?}",
                after.default_version_id
            );
            assert_eq!(
                after.version_ids, listed_order,
                "case {name:?}: the set must leave the policy carrying the versions it \
                 carried, newest first, expected {listed_order:?}, got {:?}",
                after.version_ids
            );
            // Exactly one version is operative, and it is the one the call named: the
            // flags are read in the listing's own order, which is the order of
            // `listed_order`.
            let expected_flags: Vec<String> = listed_order
                .iter()
                .map(|version_id| (version_id == operative).to_string())
                .collect();
            assert_eq!(
                after.default_flags, expected_flags,
                "case {name:?}: expected {operative} alone to be flagged as the default, \
                 got {:?} for {listed_order:?}",
                after.default_flags
            );
            assert_eq!(
                (&after.create_date, &after.update_date),
                (&before.create_date, &before.update_date),
                "case {name:?}: the set creates no version, so the policy's dates must not \
                 move, got {:?}",
                (&after.create_date, &after.update_date)
            );

            for (version_id, document) in &documents {
                let (served, flag) = read_version(&router, name, &arn, version_id).await;
                assert_eq!(
                    served, *document,
                    "case {name:?}: the document of {version_id} must be the one it was \
                     created with, got {served:?}"
                );
                assert_eq!(
                    flag,
                    (version_id == operative).to_string(),
                    "case {name:?}: expected GetPolicyVersion({version_id}) to report \
                     IsDefaultVersion={}, got {flag:?}",
                    version_id == operative
                );
            }
        }
    }
}

/// One request a set must refuse, and the answer it must get.
struct MissCase {
    name: &'static str,
    arn: String,
    version_id: String,
    status: u16,
    code: &'static str,
    /// The whole message the refusal must carry, where the operation words one; `None`
    /// for a modelled-constraint refusal, whose wording is the framework's.
    message: Option<String>,
}

/// RULE: a request is refused by the KIND of its miss — an ARN the account holds no
/// policy under, and a version identifier the policy does not carry, are the missing
/// entity `NoSuchEntity` is for, while a value that is not a customer managed policy's
/// ARN at all (another service's, another IAM entity's, one in AWS's own namespace, which
/// names a policy the account cannot modify) is an invalid input parameter, and a value
/// outside the bounds its member declares is refused before the operation sees it. None of
/// them moves any policy's operative version.
#[tokio::test]
async fn a_request_naming_no_held_policy_or_version_is_refused_by_its_kind() {
    let router = router();
    let account = account_id();

    // A policy that WAS held and is then released, which is the state a caller proving a
    // dead ARN unusable is in: the ARN is well formed and named a policy a moment ago.
    let deleted = create_policy(&router, "ReleasedPolicy", "/eng/").await;
    let (status, body) = call(&router, "DeletePolicy", &[("PolicyArn", &deleted)]).await;
    assert_eq!(
        status, 200,
        "the fixture DeletePolicy must succeed, got {status}: {body}"
    );

    // One held policy carrying three versions with the middle one operative, so a
    // refusal is a refusal about THAT policy, and a set that leaked through would be
    // visible whichever direction it moved the default in.
    let held = create_policy(&router, "HeldPolicy", "/eng/").await;
    add_version(&router, &held, DOCUMENTS[1], false).await;
    add_version(&router, &held, DOCUMENTS[2], false).await;
    let (status, body) = set_default(&router, &held, "v2").await;
    assert_eq!(
        status, 200,
        "the fixture SetDefaultPolicyVersion must succeed, got {status}: {body}"
    );
    let before = reported(&router, "the held policy", &held).await;

    let not_valid = |arn: &str| Some(format!("ARN {arn} is not valid."));
    let aws_owned = |arn: &str| {
        Some(format!(
            "Policy {arn} is an AWS managed policy, which cannot be modified."
        ))
    };
    let aws_catalogued = state::AWS_MANAGED_POLICIES[0].arn();
    let other_service = "arn:aws:s3:::a-bucket-named-here".to_string();
    let a_role = format!("arn:aws:iam::{account}:role/a-role-not-a-policy");
    let regional = format!("arn:aws:iam:us-east-1:{account}:policy/HeldPolicy");
    let cases = vec![
        MissCase {
            name: "the ARN of a policy that was just deleted",
            arn: deleted.clone(),
            version_id: "v1".to_string(),
            status: 404,
            code: "NoSuchEntity",
            message: Some(format!("Policy {deleted} was not found.")),
        },
        MissCase {
            name: "an account policy the account never created",
            arn: customer_policy_arn("/", "NeverCreated"),
            version_id: "v1".to_string(),
            status: 404,
            code: "NoSuchEntity",
            message: Some(format!(
                "Policy {} was not found.",
                customer_policy_arn("/", "NeverCreated")
            )),
        },
        MissCase {
            name: "the held policy's name under another path",
            arn: customer_policy_arn("/sales/", "HeldPolicy"),
            version_id: "v2".to_string(),
            status: 404,
            code: "NoSuchEntity",
            message: Some(format!(
                "Policy {} was not found.",
                customer_policy_arn("/sales/", "HeldPolicy")
            )),
        },
        MissCase {
            name: "the held policy's name in another case",
            arn: customer_policy_arn("/eng/", "heldpolicy"),
            version_id: "v2".to_string(),
            status: 404,
            code: "NoSuchEntity",
            message: Some(format!(
                "Policy {} was not found.",
                customer_policy_arn("/eng/", "heldpolicy")
            )),
        },
        MissCase {
            name: "an AWS-managed policy the catalog carries",
            arn: aws_catalogued.clone(),
            version_id: "v1".to_string(),
            status: 400,
            code: "InvalidInput",
            message: aws_owned(&aws_catalogued),
        },
        MissCase {
            name: "an AWS-managed policy the catalog does not carry",
            arn: "arn:aws:iam::aws:policy/job-function/ViewOnlyAccess".to_string(),
            version_id: "v1".to_string(),
            status: 400,
            code: "InvalidInput",
            message: aws_owned("arn:aws:iam::aws:policy/job-function/ViewOnlyAccess"),
        },
        MissCase {
            name: "an ARN of another service",
            arn: other_service.clone(),
            version_id: "v1".to_string(),
            status: 400,
            code: "InvalidInput",
            message: not_valid(&other_service),
        },
        MissCase {
            name: "an IAM ARN naming a role rather than a policy",
            arn: a_role.clone(),
            version_id: "v1".to_string(),
            status: 400,
            code: "InvalidInput",
            message: not_valid(&a_role),
        },
        MissCase {
            name: "a managed-policy ARN carrying a region, which IAM's never do",
            arn: regional.clone(),
            version_id: "v1".to_string(),
            status: 400,
            code: "InvalidInput",
            message: not_valid(&regional),
        },
        MissCase {
            name: "the version one past the greatest the policy carries",
            arn: held.clone(),
            version_id: "v4".to_string(),
            status: 404,
            code: "NoSuchEntity",
            message: Some(format!(
                "Policy {held} version v4 does not exist or is not attachable."
            )),
        },
        MissCase {
            name: "a version far past the greatest the policy carries",
            arn: held.clone(),
            version_id: "v99".to_string(),
            status: 404,
            code: "NoSuchEntity",
            message: Some(format!(
                "Policy {held} version v99 does not exist or is not attachable."
            )),
        },
        MissCase {
            // `policyVersionIdType` admits a sub-identifier after the number, and the
            // policy carries no version under that identifier.
            name: "a held version's number carrying a sub-identifier",
            arn: held.clone(),
            version_id: "v2.1".to_string(),
            status: 404,
            code: "NoSuchEntity",
            message: Some(format!(
                "Policy {held} version v2.1 does not exist or is not attachable."
            )),
        },
        MissCase {
            // `policyVersionIdType`'s pattern is `^v[1-9][0-9]*(\.[A-Za-z0-9-]*)?$`, so
            // a version number starting at zero is outside the member's own constraint.
            name: "a version identifier numbered from zero",
            arn: held.clone(),
            version_id: "v0".to_string(),
            status: 400,
            code: "ValidationError",
            message: None,
        },
        MissCase {
            name: "a version identifier with no leading v",
            arn: held.clone(),
            version_id: "2".to_string(),
            status: 400,
            code: "ValidationError",
            message: None,
        },
        MissCase {
            // "Policy version identifiers always begin with v (always lowercase)"
            // (`PolicyVersion$VersionId`).
            name: "a version identifier spelling the v in upper case",
            arn: held.clone(),
            version_id: "V2".to_string(),
            status: 400,
            code: "ValidationError",
            message: None,
        },
        MissCase {
            // `arnType` declares a length of 20..2048, so a shorter value is refused
            // before the operation sees it.
            name: "an ARN shorter than the member's least length",
            arn: "arn:aws:iam::1:p/x".to_string(),
            version_id: "v1".to_string(),
            status: 400,
            code: "ValidationError",
            message: None,
        },
    ];

    for case in &cases {
        let name = case.name;
        let (status, body) = set_default(&router, &case.arn, &case.version_id).await;
        assert_eq!(
            status, case.status,
            "case {name:?}: SetDefaultPolicyVersion({:?}, {:?}) must answer {}, \
             got {status}: {body}",
            case.arn, case.version_id, case.status
        );
        assert_eq!(
            error_code(&body),
            case.code,
            "case {name:?}: expected the error code {:?}, got {:?}: {body}",
            case.code,
            error_code(&body)
        );
        if let Some(expected) = &case.message {
            assert_eq!(
                &element(&body, "Message"),
                expected,
                "case {name:?}: expected the message {expected:?}, got {:?}",
                element(&body, "Message")
            );
        }

        // Nothing the refusals touched moved: the held policy still carries its three
        // versions with the middle one operative.
        let after = reported(&router, name, &held).await;
        assert_eq!(
            (
                &after.default_version_id,
                &after.version_ids,
                &after.default_flags
            ),
            (
                &before.default_version_id,
                &before.version_ids,
                &before.default_flags
            ),
            "case {name:?}: a refused set must leave the policy's operative version where \
             it was, got {:?}",
            (
                &after.default_version_id,
                &after.version_ids,
                &after.default_flags
            )
        );
    }
}

/// RULE: the operative version of an attached managed policy is the one IN FORCE for the
/// entities it is attached to, so moving the default changes what the principal holding
/// that attachment may do — immediately, in both directions, and without the attachment
/// itself changing.
#[tokio::test]
async fn the_operative_version_is_the_one_in_force_for_an_attached_principal() {
    let router = router();
    let user_name = "operative";
    // No inline policy at all, so the attached managed policy's operative version is the
    // only document in force on this principal.
    let access_key_id = principal(&router, "operative version", user_name, &[]).await;

    // Two versions granting ONE action each, so whichever is operative the other action
    // has no grant: the pair of answers names which version decided.
    let arn = create_policy(&router, "InForce", "/eng/").await;
    let (status, body) = call(
        &router,
        "CreatePolicyVersion",
        &[
            ("PolicyArn", arn.as_str()),
            (
                "PolicyDocument",
                &statement("Allow", "iam:ListGroups", "*"),
            ),
            ("SetAsDefault", "true"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "the fixture CreatePolicyVersion must succeed, got {status}: {body}"
    );
    let (status, body) = call(
        &router,
        "CreatePolicyVersion",
        &[
            ("PolicyArn", arn.as_str()),
            (
                "PolicyDocument",
                &statement("Allow", "iam:ListRoles", "*"),
            ),
            ("SetAsDefault", "false"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "the fixture CreatePolicyVersion must succeed, got {status}: {body}"
    );

    let (status, body) = call(
        &router,
        "AttachUserPolicy",
        &[("UserName", user_name), ("PolicyArn", arn.as_str())],
    )
    .await;
    assert_eq!(
        status, 200,
        "the fixture AttachUserPolicy must succeed, got {status}: {body}"
    );

    // Rows: the version the creates left operative, the other one, and back again twice
    // over — so a grant that only moved once, or only in one direction, fails here. Each
    // row names the version made operative and the action that version grants.
    for (round, (operative, allowed, refused)) in [
        ("v2", "ListGroups", "ListRoles"),
        ("v3", "ListRoles", "ListGroups"),
        ("v2", "ListGroups", "ListRoles"),
        ("v3", "ListRoles", "ListGroups"),
    ]
    .into_iter()
    .enumerate()
    {
        let case = format!("round {round}: {operative} operative");
        let (status, body) = set_default(&router, &arn, operative).await;
        assert_eq!(
            status, 200,
            "case {case:?}: SetDefaultPolicyVersion({operative}) must be served, \
             got {status}: {body}"
        );

        let (status, body) = call_signed_by(&router, Some(&access_key_id), allowed, &[]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected {allowed} to be served by the operative version, \
             got {status}: {body}"
        );
        let (status, body) = call_signed_by(&router, Some(&access_key_id), refused, &[]).await;
        assert_eq!(
            status, 403,
            "case {case:?}: expected {refused} to have no grant in the operative version, \
             got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "AccessDenied",
            "case {case:?}: expected {refused} to be refused AccessDenied, got {:?}: {body}",
            error_code(&body)
        );

        // The attachment is what it was: the set moved the operative version, not the
        // policies the user carries.
        let (status, listed) = call(
            &router,
            "ListAttachedUserPolicies",
            &[("UserName", user_name)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListAttachedUserPolicies must succeed, got {status}: {listed}"
        );
        assert_eq!(
            elements(&listed, "PolicyArn"),
            vec![arn.clone()],
            "case {case:?}: the attachment must be the one it was, got {listed}"
        );
    }
}

/// One principal a set is decided for: the inline policies it holds, the version it asks
/// to make operative, and the answer the rule requires.
struct GateCase {
    name: &'static str,
    /// `(PolicyName, PolicyDocument)` pairs put on the caller before it calls.
    policies: Vec<(String, String)>,
    status: u16,
    asks_for: &'static str,
    /// Whether the refusal is the one a statement about the request explicitly denies,
    /// which AWS words differently from the absence of any grant.
    explicit_deny: bool,
}

/// RULE: IAM authorizes its own API, so a caller whose identity-based policies do not
/// permit `iam:SetDefaultPolicyVersion` on the policy it addresses is refused HTTP 403
/// `AccessDenied` — named by the principal, the action and the policy the decision was
/// made on — and the policy's operative version does not move.
#[tokio::test]
async fn a_caller_whose_policies_forbid_the_call_is_refused() {
    let router = router();
    let arn = create_policy(&router, "GuardedPolicy", "/eng/").await;
    add_version(&router, &arn, DOCUMENTS[1], false).await;

    // Rows: a principal allowed the action, one explicitly denied it, and one holding no
    // grant for it at all.
    let allow = statement("Allow", "iam:SetDefaultPolicyVersion", "*");
    let deny = statement("Deny", "iam:SetDefaultPolicyVersion", "*");
    let elsewhere = statement("Allow", "iam:CreatePolicyVersion", "*");
    // Each refused row asks for the version that is NOT operative, so a call that leaked
    // through the gate would be visible in the policy afterwards.
    let rows = vec![
        GateCase {
            name: "allowed",
            policies: vec![("allow-set".to_string(), allow.clone())],
            status: 200,
            asks_for: "v2",
            explicit_deny: false,
        },
        GateCase {
            name: "explicitly denied over an allow",
            policies: vec![
                ("allow-set".to_string(), allow.clone()),
                ("deny-set".to_string(), deny),
            ],
            status: 403,
            asks_for: "v1",
            explicit_deny: true,
        },
        GateCase {
            name: "allowed only another action",
            policies: vec![("allow-other".to_string(), elsewhere)],
            status: 403,
            asks_for: "v1",
            explicit_deny: false,
        },
        GateCase {
            name: "no identity-based policy at all",
            policies: Vec::new(),
            status: 403,
            asks_for: "v1",
            explicit_deny: false,
        },
    ];

    for row in rows {
        let GateCase {
            name: case,
            policies,
            status: expected,
            asks_for,
            explicit_deny,
        } = row;
        let user_name = format!("caller-{}", case.replace(' ', "-"));
        let access_key_id = principal(&router, case, &user_name, &policies).await;
        let (status, body) = call_signed_by(
            &router,
            Some(&access_key_id),
            "SetDefaultPolicyVersion",
            &[("PolicyArn", arn.as_str()), ("VersionId", asks_for)],
        )
        .await;
        assert_eq!(
            status, expected,
            "case {case:?}: expected {expected}, got {status}: {body}"
        );
        if expected == 403 {
            assert_eq!(
                error_code(&body),
                "AccessDenied",
                "case {case:?}: expected AccessDenied, got {:?}: {body}",
                error_code(&body)
            );
            // The decision is made on the ARN the request addresses, and the refusal
            // names the entity that ARN identifies: a managed policy.
            let reason = if explicit_deny {
                "with an explicit deny in an identity-based policy".to_string()
            } else {
                "because no identity-based policy allows the iam:SetDefaultPolicyVersion \
                 action"
                    .to_string()
            };
            assert_access_denied_message(
                case,
                &element(&body, "Message"),
                &format!("arn:aws:iam::{}:user/{user_name}", account_id()),
                "iam:SetDefaultPolicyVersion",
                &format!("policy {arn}"),
                &reason,
            );
        }
        // Only the allowed row moved the operative version; every refusal left it where
        // that row found it, rather than writing the version it asked for.
        let after = reported(&router, case, &arn).await;
        assert_eq!(
            after.default_version_id, "v2",
            "case {case:?}: expected the operative version to be v2, got {:?}",
            after.default_version_id
        );
    }
}
