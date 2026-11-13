// SPDX-License-Identifier: CC0-1.0

//! The role an EKS Pod Identity association binds to a Kubernetes service account
//! is resolved against IAM, and an association is written only for a role the EKS
//! Pod Identity principal may assume.
//!
//! The RULES under test:
//!
//! * a `roleArn` whose trust policy — the one IAM holds for it — does not let
//!   `pods.eks.amazonaws.com` assume it is refused 400 `InvalidParameterException`
//!   with the fixed sentence Amazon EKS answers that class with, and no association
//!   is left behind. EKS Pod Identity "manages credentials to assume this role for
//!   applications in the containers in the Pods that use this service account"
//!   (`CreatePodIdentityAssociationRequest$roleArn`), so a role that does not trust
//!   the principal that assumes it could never issue those credentials;
//! * a `roleArn` of ANOTHER account is refused 403 `AccessDeniedException` with the
//!   fixed cross-account pass-role sentence, whichever call names it: a role handed
//!   to Amazon EKS is passed with the caller's own `iam:PassRole`.
//!
//! Both rules hold for `CreatePodIdentityAssociation` and for
//! `UpdatePodIdentityAssociation`, which hands over a role the same way, so every
//! row below is driven through both.
//!
//! This needs more than one service on one engine — EKS asks IAM over the
//! in-process callback, so a single-service router has nothing to ask — which is
//! why the suite lives here rather than in the EKS crate. The request harness is
//! the test crate's own (`eks_wire`), and the rows differ along which principal the
//! trust document names and how, which actions it allows, whether the statement
//! allows or denies, and which account the role is in.

use std::sync::Arc;

use aws_emulator_router::{build_router, InMemoryEngine};

use crate::eks_wire::{cluster_body, ec2, eks, iam, role_arn, Answer};
use crate::query_wire::elements;

/// Keep this at or above `boot_serve.rs`'s stack: the same assembly runs here.
const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// The principal EKS Pod Identity assumes an association's role as.
const PODS_PRINCIPAL: &str = "\"pods.eks.amazonaws.com\"";

/// A trust policy with one statement, built from the pieces that vary between rows.
///
/// Written compactly and with no `&`, `=`, `+` or `%`, so it travels in an `awsQuery`
/// form body as written.
fn trust_policy(effect: &str, action: &str, service: &str) -> String {
    format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"{effect}\",\
         \"Principal\":{{\"Service\":{service}}},\"Action\":{action}}}]}}"
    )
}

/// The sentence Amazon EKS refuses a pod-identity role whose trust policy does not
/// admit the Pod Identity principal with — read from the one definition of it.
fn trust_refusal() -> &'static str {
    service_com_amazonaws_eks_awswesleyfrontend::state::POD_IDENTITY_TRUST_POLICY_REFUSAL
}

/// The fixed sentence a role of another account earns, from the same place.
fn cross_account_refusal() -> &'static str {
    service_com_amazonaws_eks_awswesleyfrontend::state::CROSS_ACCOUNT_PASS_ROLE_REFUSAL
}

/// Assert a role whose trust policy admits no Pod Identity assume is refused.
fn assert_trust_refusal(case: &str, answer: &Answer) {
    assert_eq!(
        (answer.status, answer.error_type.as_deref()),
        (400, Some("InvalidParameterException")),
        "case {case:?}: a role that does not trust EKS Pod Identity must be refused 400 \
         InvalidParameterException, got {answer:?}"
    );
    assert_eq!(
        answer.message(),
        trust_refusal(),
        "case {case:?}: expected the fixed trust-policy sentence, got {answer:?}"
    );
}

#[test]
fn a_pod_identity_association_is_written_only_for_a_role_that_trusts_pod_identity() {
    std::thread::Builder::new()
        .name("eks-pod-identity-role-trust".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
            let mut router = build_router(engine);
            router.set_request_authentication(false);
            let runtime = tokio::runtime::Builder::new_current_thread()
                .build()
                .expect("a current-thread runtime");

            // The cluster is created in subnets EC2 resolves, so the account's own
            // default subnets are read from EC2 rather than invented.
            let defaults = ec2(
                &router,
                &runtime,
                "DescribeSubnets",
                &[
                    ("Filter.1.Name", "default-for-az"),
                    ("Filter.1.Value.1", "true"),
                ],
            );
            assert_eq!(
                defaults.status, 200,
                "setup: DescribeSubnets must succeed, got {defaults:?}"
            );
            let subnets = elements(&defaults.text, "subnetId");
            assert!(
                subnets.len() >= 2,
                "setup: the account must have default subnets, got {subnets:?}"
            );
            let subnets = vec![subnets[0].clone(), subnets[1].clone()];

            // The roles, each with the trust document its row is about.
            let assume_and_tag = "[\"sts:AssumeRole\",\"sts:TagSession\"]";
            let both_principals = format!("[\"ec2.amazonaws.com\",{PODS_PRINCIPAL}]");
            let fixtures: Vec<(&str, String)> = vec![
                (
                    "pod-identity-role",
                    trust_policy("Allow", assume_and_tag, PODS_PRINCIPAL),
                ),
                (
                    "pod-identity-assume-only",
                    trust_policy("Allow", "\"sts:AssumeRole\"", PODS_PRINCIPAL),
                ),
                (
                    "pod-identity-both-principals",
                    trust_policy("Allow", assume_and_tag, both_principals.as_str()),
                ),
                (
                    "pod-identity-untrusted",
                    trust_policy("Allow", assume_and_tag, "\"ec2.amazonaws.com\""),
                ),
                (
                    "pod-identity-denied",
                    trust_policy("Deny", assume_and_tag, PODS_PRINCIPAL),
                ),
                (
                    "pod-identity-wrong-action",
                    trust_policy("Allow", "\"s3:GetObject\"", PODS_PRINCIPAL),
                ),
            ];
            for (name, policy) in &fixtures {
                let answer = iam(
                    &router,
                    &runtime,
                    "CreateRole",
                    &[
                        ("RoleName", name),
                        ("AssumeRolePolicyDocument", policy.as_str()),
                    ],
                );
                assert_eq!(
                    answer.status, 200,
                    "setup: CreateRole {name} must succeed, got {answer:?}"
                );
            }

            let cluster = "pod-identity-trust-host";
            let created = eks(
                &router,
                &runtime,
                "POST",
                "/clusters",
                Some(cluster_body(cluster, &subnets)),
            );
            assert_eq!(
                created.status, 200,
                "setup: the cluster the associations hang off must be created, got {created:?}"
            );

            // One association to UPDATE, created with a role that does trust the
            // principal: every row below is driven through the create and through an
            // update of this association, because an update hands Amazon EKS a role
            // exactly as a create does.
            let accepted_role = role_arn("pod-identity-role");
            let seed = eks(
                &router,
                &runtime,
                "POST",
                &format!("/clusters/{cluster}/pod-identity-associations"),
                Some(serde_json::json!({
                    "namespace": "default",
                    "serviceAccount": "seed",
                    "roleArn": accepted_role,
                })),
            );
            assert_eq!(
                seed.status, 200,
                "setup: the association to update must be created, got {seed:?} ({})",
                seed.message()
            );
            let association_id = seed.body["association"]["associationId"]
                .as_str()
                .expect("the created association's id")
                .to_string();

            /// What a row's role earns.
            #[derive(Debug, Clone, Copy, PartialEq, Eq)]
            enum Verdict {
                /// The role may be used: the association is written.
                Accepted,
                /// Its trust policy admits no Pod Identity assume: 400 with the fixed
                /// sentence.
                TrustRefused,
                /// It is a role of another account: 403 with the fixed pass-role
                /// sentence.
                CrossAccount,
            }

            // (case, the role, the verdict)
            let rows: Vec<(&str, String, Verdict)> = vec![
                (
                    "the documented pod-identity trust policy: the pods principal with \
                     sts:AssumeRole and sts:TagSession",
                    role_arn("pod-identity-role"),
                    Verdict::Accepted,
                ),
                (
                    "the pods principal with sts:AssumeRole alone",
                    role_arn("pod-identity-assume-only"),
                    Verdict::Accepted,
                ),
                (
                    "a trust policy naming the pods principal in a list beside another",
                    role_arn("pod-identity-both-principals"),
                    Verdict::Accepted,
                ),
                (
                    "a role that trusts Amazon EC2 only",
                    role_arn("pod-identity-untrusted"),
                    Verdict::TrustRefused,
                ),
                (
                    "a role whose statement denies the assume",
                    role_arn("pod-identity-denied"),
                    Verdict::TrustRefused,
                ),
                (
                    "a role whose statement names another action",
                    role_arn("pod-identity-wrong-action"),
                    Verdict::TrustRefused,
                ),
                (
                    "a role of another account",
                    "arn:aws:iam::111122223333:role/no-trust".to_string(),
                    Verdict::CrossAccount,
                ),
            ];

            let mut association_role = accepted_role.clone();
            for (index, (case, role, verdict)) in rows.iter().enumerate() {
                let service_account = format!("row-{index}");
                let created = eks(
                    &router,
                    &runtime,
                    "POST",
                    &format!("/clusters/{cluster}/pod-identity-associations"),
                    Some(serde_json::json!({
                        "namespace": "default",
                        "serviceAccount": service_account,
                        "roleArn": role,
                    })),
                );
                let updated = eks(
                    &router,
                    &runtime,
                    "POST",
                    &format!(
                        "/clusters/{cluster}/pod-identity-associations/{association_id}"
                    ),
                    Some(serde_json::json!({ "roleArn": role })),
                );
                match verdict {
                    Verdict::Accepted => {
                        assert_eq!(
                            created.status, 200,
                            "case {case:?}: a role EKS Pod Identity can assume must be \
                             accepted, got {created:?} ({})",
                            created.message()
                        );
                        assert_eq!(
                            created.body["association"]["roleArn"],
                            serde_json::json!(role),
                            "case {case:?}: the association must carry the role it was \
                             given, got {}",
                            created.text
                        );
                        assert_eq!(
                            updated.status, 200,
                            "case {case:?}: the same role must be accepted on an update, \
                             got {updated:?} ({})",
                            updated.message()
                        );
                    }
                    Verdict::TrustRefused => {
                        assert_trust_refusal(case, &created);
                        assert_trust_refusal(case, &updated);
                    }
                    Verdict::CrossAccount => {
                        for (call, answer) in [("create", &created), ("update", &updated)] {
                            assert_eq!(
                                (answer.status, answer.error_type.as_deref()),
                                (403, Some("AccessDeniedException")),
                                "case {case:?} ({call}): a role of another account must be \
                                 refused 403 AccessDeniedException, got {answer:?}"
                            );
                            assert_eq!(
                                answer.message(),
                                cross_account_refusal(),
                                "case {case:?} ({call}): expected the fixed cross-account \
                                 sentence, got {answer:?}"
                            );
                        }
                    }
                }

                // NOTHING is left behind by a refused create, and a refused update
                // changes nothing: the association the row tried to write is absent,
                // and the one it tried to repoint still carries the role it had.
                let listed = eks(
                    &router,
                    &runtime,
                    "GET",
                    &format!(
                        "/clusters/{cluster}/pod-identity-associations?serviceAccount={service_account}"
                    ),
                    None,
                );
                let written = listed.body["associations"]
                    .as_array()
                    .map_or(0, Vec::len);
                assert_eq!(
                    written,
                    usize::from(*verdict == Verdict::Accepted),
                    "case {case:?}: expected the association to exist only when the role \
                     was accepted, got {}",
                    listed.text
                );
                let seeded = eks(
                    &router,
                    &runtime,
                    "GET",
                    &format!(
                        "/clusters/{cluster}/pod-identity-associations/{association_id}"
                    ),
                    None,
                );
                if *verdict == Verdict::Accepted {
                    association_role = role.clone();
                }
                assert_eq!(
                    seeded.body["association"]["roleArn"],
                    serde_json::json!(association_role),
                    "case {case:?}: only an accepted update may change the stored role, got {}",
                    seeded.text
                );
            }
        })
        .expect("spawn the eks pod-identity-role-trust thread")
        .join()
        .expect("the eks pod-identity-role-trust thread");
}
