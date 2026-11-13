// SPDX-License-Identifier: CC0-1.0

//! Reads of an AWS-managed policy this emulator PUBLISHES, and the refusal a
//! `PolicyArn` that resolves to no policy gets, over the awsQuery wire.
//!
//! Two rules are asserted here.
//!
//! 1. An AWS-managed policy ARN this emulator hands a caller in a response must be
//!    readable through the API that describes such an ARN: a control plane may not
//!    recommend a resource identifier its own service then reports it knows nothing
//!    about. The ARNs the EKS add-on catalog recommends are the rows.
//! 2. An operation answers only the errors its model declares for it. For the reads
//!    addressed by `PolicyArn` the modelled set is `InvalidInputException` (400),
//!    `NoSuchEntityException` (404) and, where the operation declares it,
//!    `ServiceFailureException` (500)
//!    (`smithy_query(query="operation", operation="GetPolicy,GetPolicyVersion,ListPolicyVersions,ListPolicyTags")`),
//!    so an ARN resolving to no policy is the missing entity `NoSuchEntity` names
//!    whichever namespace it is in — never a code outside that set, which no SDK has a
//!    branch for.
//!
//! Each suite is a table whose rows differ along what the rule generalises over: the
//! add-on and service account the recommendation came from, the policy's path depth,
//! the operation doing the reading, and the kind of ARN that resolves to nothing.
//!
//! The awsQuery harness is the one `tests/all/user_lifecycle.rs` owns and the other
//! suites in this crate share.

use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_iam_awsidentitymanagementv20100508::state;

use crate::user_lifecycle::{account_id, call, element, elements, error_code, router};

/// One AWS-managed policy ARN this emulator publishes to callers, and where it
/// publishes it.
///
/// SOURCE: the EKS add-on catalog's pod-identity recommendations
/// (`services/service-com-amazonaws-eks-awswesleyfrontend/src/state.rs`, the
/// `pod_identity` rows of its add-on catalog), which `DescribeAddonConfiguration`
/// reports as `podIdentityConfiguration[].recommendedManagedPolicies`. Every ARN that
/// catalog names is a row, not only the one a recorded suite read.
struct PublishedPolicy {
    /// The add-on whose configuration recommends it.
    addon: &'static str,
    /// The Kubernetes service account the recommendation is for.
    service_account: &'static str,
    /// The ARN as that recommendation spells it.
    arn: &'static str,
}

/// Every ARN the add-on catalog recommends: two at the account root for one add-on,
/// one at the root for another, and two under `/service-role/`, so the table spans
/// both path depths an attachable AWS-managed policy is published under and both
/// single- and multi-policy recommendations.
const PUBLISHED: &[PublishedPolicy] = &[
    PublishedPolicy {
        addon: "amazon-cloudwatch-observability",
        service_account: "cloudwatch-agent",
        arn: "arn:aws:iam::aws:policy/CloudWatchAgentServerPolicy",
    },
    PublishedPolicy {
        addon: "amazon-cloudwatch-observability",
        service_account: "cloudwatch-agent",
        arn: "arn:aws:iam::aws:policy/AWSXrayWriteOnlyAccess",
    },
    PublishedPolicy {
        addon: "vpc-cni",
        service_account: "aws-node",
        arn: "arn:aws:iam::aws:policy/AmazonEKS_CNI_Policy",
    },
    PublishedPolicy {
        addon: "aws-ebs-csi-driver",
        service_account: "ebs-csi-controller-sa",
        arn: "arn:aws:iam::aws:policy/service-role/AmazonEBSCSIDriverPolicy",
    },
    PublishedPolicy {
        addon: "aws-efs-csi-driver",
        service_account: "efs-csi-controller-sa",
        arn: "arn:aws:iam::aws:policy/service-role/AmazonEFSCSIDriverPolicy",
    },
];

/// One `GetPolicy` call.
async fn get_policy(router: &MultiServiceRouter, policy_arn: &str) -> (u16, String) {
    call(router, "GetPolicy", &[("PolicyArn", policy_arn)]).await
}

/// One `GetPolicyVersion` call.
async fn get_policy_version(
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

/// One `ListPolicyVersions` call.
async fn list_policy_versions(router: &MultiServiceRouter, policy_arn: &str) -> (u16, String) {
    call(router, "ListPolicyVersions", &[("PolicyArn", policy_arn)]).await
}

/// One `ListPolicyTags` call.
async fn list_policy_tags(router: &MultiServiceRouter, policy_arn: &str) -> (u16, String) {
    call(router, "ListPolicyTags", &[("PolicyArn", policy_arn)]).await
}

/// The friendly name an AWS-managed policy's ARN spells: the segment after the last
/// `/` of `arn:aws:iam::aws:policy<path><name>`.
fn policy_name_of(arn: &str) -> &str {
    arn.rsplit('/').next().expect("a non-empty ARN")
}

/// The path an AWS-managed policy's ARN spells: everything from the first `/` after
/// `:policy` through the last one.
fn path_of(arn: &str) -> &str {
    let tail = arn
        .split_once(":policy")
        .expect("a managed-policy ARN")
        .1;
    &tail[..tail.rfind('/').expect("a path separator") + 1]
}

/// RULE 1: every AWS-managed policy ARN this emulator recommends to a caller is
/// readable — `GetPolicy` reports its metadata, the version its `DefaultVersionId`
/// names serves its document, and the enumerations of the same policy agree.
#[tokio::test]
async fn every_policy_arn_this_emulator_publishes_is_readable() {
    let router = router();
    assert!(
        PUBLISHED.len() >= 2,
        "the recommendation table must carry more than one row to span anything, got {}",
        PUBLISHED.len()
    );

    for published in PUBLISHED {
        let arn = published.arn;
        let case = format!(
            "{} / {} recommends {arn}",
            published.addon, published.service_account
        );

        let (status, body) = get_policy(&router, arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetPolicy must report a policy this emulator recommends, got \
             {status}: {body}"
        );
        assert_eq!(
            element(&body, "Arn"),
            arn,
            "case {case:?}: the reported Arn must be the ARN asked for: {body}"
        );
        assert_eq!(
            element(&body, "PolicyName"),
            policy_name_of(arn),
            "case {case:?}: the reported PolicyName must be the one the ARN spells: {body}"
        );
        assert_eq!(
            element(&body, "Path"),
            path_of(arn),
            "case {case:?}: the reported Path must be the one the ARN spells: {body}"
        );
        // Every recommended policy is one an identity is meant to take, so none of them
        // sits under the path AWS reserves for service-linked roles
        // (`/aws-service-role/`) and all report `IsAttachable` true.
        assert_eq!(
            element(&body, "IsAttachable"),
            "true",
            "case {case:?}: a policy recommended for a service account must be \
             attachable: {body}"
        );
        assert_eq!(
            element(&body, "AttachmentCount"),
            "0",
            "case {case:?}: nothing is attached in this account yet: {body}"
        );

        // The document round trip: the version GetPolicy names is the one that serves
        // the policy's content.
        let default_version = element(&body, "DefaultVersionId");
        let (status, version) = get_policy_version(&router, arn, &default_version).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetPolicyVersion must serve the default version \
             {default_version:?}, got {status}: {version}"
        );
        let document = crate::user_lifecycle::document_element(&version, "Document");
        let parsed: serde_json::Value = serde_json::from_str(&document)
            .unwrap_or_else(|error| panic!("case {case:?}: the served document must be JSON, got {error}: {document}"));
        let statements = parsed
            .get("Statement")
            .and_then(serde_json::Value::as_array)
            .unwrap_or_else(|| panic!("case {case:?}: the document must carry a Statement list, got {document}"));
        assert!(
            !statements.is_empty(),
            "case {case:?}: a policy that grants nothing recommends nothing, got {document}"
        );
        for (position, statement) in statements.iter().enumerate() {
            assert_eq!(
                statement.get("Effect"),
                Some(&serde_json::json!("Allow")),
                "case {case:?}: statement {position} of a recommended grant must allow, got \
                 {statement}"
            );
            assert!(
                statement.get("Action").is_some() || statement.get("NotAction").is_some(),
                "case {case:?}: statement {position} must name the actions it grants, got \
                 {statement}"
            );
            assert!(
                statement.get("Resource").is_some() || statement.get("NotResource").is_some(),
                "case {case:?}: statement {position} must name the resources it grants on, \
                 got {statement}"
            );
        }

        // The other readers of the same ARN agree with GetPolicy: the version listing
        // reports the default version it named, and the tag listing reports no tags,
        // because AWS owns the policy and tagging is an account operation.
        let (status, versions) = list_policy_versions(&router, arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListPolicyVersions must serve a policy GetPolicy reported, got \
             {status}: {versions}"
        );
        assert_eq!(
            elements(&versions, "VersionId"),
            vec![default_version.clone()],
            "case {case:?}: the listing must report the version GetPolicy named: {versions}"
        );
        let (status, tags) = list_policy_tags(&router, arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListPolicyTags must serve a policy GetPolicy reported, got \
             {status}: {tags}"
        );
        assert!(
            elements(&tags, "Key").is_empty(),
            "case {case:?}: an AWS-owned policy carries none of the account's tags: {tags}"
        );

        // And it is discoverable rather than only addressable: the enumeration of the
        // policies AWS owns reports it too.
        assert!(
            aws_scope_arns(&router).await.iter().any(|listed| listed == arn),
            "case {case:?}: ListPolicies(Scope=AWS) must report a policy this emulator \
             recommends"
        );

        // A read changes nothing a later caller sees: the second read is the first.
        let (repeat_status, repeat) = get_policy(&router, arn).await;
        assert_eq!(
            (repeat_status, element(&repeat, "Arn"), element(&repeat, "PolicyId")),
            (status, element(&body, "Arn"), element(&body, "PolicyId")),
            "case {case:?}: a read must report the same policy the read before it did"
        );
    }
}

/// Every ARN `ListPolicies(Scope=AWS)` reports, across all its pages.
async fn aws_scope_arns(router: &MultiServiceRouter) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    let mut marker: Option<String> = None;
    loop {
        let mut params: Vec<(&str, &str)> = vec![("Scope", "AWS")];
        if let Some(token) = marker.as_deref() {
            params.push(("Marker", token));
        }
        let (status, body) = call(router, "ListPolicies", &params).await;
        assert_eq!(
            status, 200,
            "ListPolicies(Scope=AWS) must succeed, got {status}: {body}"
        );
        seen.extend(elements(&body, "Arn"));
        if element(&body, "IsTruncated") == "true" {
            marker = Some(element(&body, "Marker"));
        } else {
            break;
        }
        assert!(
            seen.len() <= state::AWS_MANAGED_POLICIES.len(),
            "the pagination never ended, saw {seen:?}"
        );
    }
    seen
}

/// One operation addressed by `PolicyArn`, and the error codes its model declares.
struct Reader {
    /// The operation's name, as the `Action` parameter spells it.
    action: &'static str,
    /// Every awsQuery error code the model declares for it — the only codes it may
    /// answer. SOURCE: `smithy_query(query="operation", operation=<action>)`.
    modelled_codes: &'static [&'static str],
}

/// The reads addressed by `PolicyArn`, each with its modelled refusals.
const READERS: &[Reader] = &[
    Reader {
        action: "GetPolicy",
        modelled_codes: &["InvalidInput", "NoSuchEntity", "ServiceFailure"],
    },
    Reader {
        action: "GetPolicyVersion",
        modelled_codes: &["InvalidInput", "NoSuchEntity", "ServiceFailure"],
    },
    Reader {
        action: "ListPolicyVersions",
        modelled_codes: &["InvalidInput", "NoSuchEntity", "ServiceFailure"],
    },
    Reader {
        action: "ListPolicyTags",
        modelled_codes: &["InvalidInput", "NoSuchEntity", "ServiceFailure"],
    },
];

/// One ARN that names no policy any reader can report, and the refusal it must get.
struct MissCase {
    name: &'static str,
    arn: String,
    status: u16,
    code: &'static str,
}

/// RULE 2: a read addressed by a `PolicyArn` it resolves no policy under is refused
/// with a code its model DECLARES — the missing entity `NoSuchEntity` names, carrying
/// its operation family's fixed text (`Policy <arn> was not found.` for a read of the
/// policy resource, `Policy <arn> does not exist or is not attachable.` for the listing
/// of the versions in force for the identities it is attached to, `Policy <arn> version
/// <id> does not exist or is not attachable.` for the read addressed by a version as well
/// as an ARN), for every ARN that is
/// a managed policy's, in the account's namespace or in AWS's; an invalid input for a
/// value that is not a managed-policy ARN at all.
#[tokio::test]
async fn an_unresolvable_policy_arn_is_refused_with_a_modelled_code() {
    let router = router();
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
            name: "an AWS-managed ARN at the root this emulator serves no policy under",
            arn: "arn:aws:iam::aws:policy/NoSuchAwsManagedPolicyHere".to_string(),
            status: 404,
            code: "NoSuchEntity",
        },
        MissCase {
            name: "an AWS-managed ARN under /service-role/ this emulator serves none under",
            arn: "arn:aws:iam::aws:policy/service-role/NoSuchAwsManagedPolicyHere".to_string(),
            status: 404,
            code: "NoSuchEntity",
        },
        MissCase {
            name: "an AWS-managed ARN under /job-function/ this emulator serves none under",
            arn: "arn:aws:iam::aws:policy/job-function/NoSuchAwsManagedPolicyHere".to_string(),
            status: 404,
            code: "NoSuchEntity",
        },
        MissCase {
            name: "a carried policy's name under another path",
            arn: "arn:aws:iam::aws:policy/service-role/AdministratorAccess".to_string(),
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
            name: "a managed-policy ARN with an empty path segment",
            arn: "arn:aws:iam::aws:policy//AdministratorAccess".to_string(),
            status: 400,
            code: "InvalidInput",
        },
    ];

    for reader in READERS {
        let action = reader.action;
        for case in &cases {
            let name = case.name;
            let mut params: Vec<(&str, &str)> = vec![("PolicyArn", &case.arn)];
            // The one reader that needs a second required member; the version named is
            // the one every policy carries, so the refusal is about the ARN.
            if action == "GetPolicyVersion" {
                params.push(("VersionId", "v1"));
            }
            let (status, body) = call(&router, action, &params).await;
            let code = error_code(&body);
            assert!(
                reader.modelled_codes.contains(&code.as_str()),
                "case {name:?}: {action} may answer only the codes its model declares \
                 {:?}, got {code:?} ({status}): {body}",
                reader.modelled_codes
            );
            assert_eq!(
                status, case.status,
                "case {name:?}: {action}({:?}) must answer {}, got {status}: {body}",
                case.arn, case.status
            );
            assert_eq!(
                code, case.code,
                "case {name:?}: {action}({:?}) must answer the error code {:?}: {body}",
                case.arn, case.code
            );
            if case.code == "NoSuchEntity" {
                let expected = match action {
                    "ListPolicyVersions" => {
                        format!("Policy {} does not exist or is not attachable.", case.arn)
                    }
                    // The read addressed by a version as well as an ARN names both.
                    "GetPolicyVersion" => format!(
                        "Policy {} version v1 does not exist or is not attachable.",
                        case.arn
                    ),
                    _ => format!("Policy {} was not found.", case.arn),
                };
                assert_eq!(
                    element(&body, "Message"),
                    expected,
                    "case {name:?}: {action} must word the absence as IAM does, got {:?}",
                    element(&body, "Message")
                );
            }
        }
    }
}
