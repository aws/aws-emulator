// SPDX-License-Identifier: CC0-1.0

//! What this service answers when a certificate-authority rotation step or an EKS
//! Pod Identity call cannot be carried out: the error CLASS, and the SENTENCE that
//! goes with it.
//!
//! The rules pinned here, each over a TABLE whose rows differ along what the rule
//! generalises over rather than over one recorded request:
//!
//! * a rotation step that asks for the state the cluster is already in — promoting
//!   the certificate authority that already signs, appending past the ceiling of
//!   certificate authorities a cluster holds — is refused as an invalid REQUEST
//!   (`InvalidRequestException`, 400), not as a conflict or a service limit, and
//!   changes nothing;
//! * deleting the certificate authority that signs is refused `ResourceInUseException`
//!   (409) with the remediation that lifts the refusal;
//! * a `(namespace, serviceAccount)` pair holds at most one association, and the
//!   refusal of a second one names the IAM role that pair is ALREADY bound to, read
//!   from the stored association;
//! * an `associationId` that resolves to nothing is `ResourceNotFoundException` (404)
//!   with one sentence, whichever operation resolved it;
//! * `CreatePodIdentityAssociation` authorizes the pass-role of `roleArn` BEFORE it
//!   judges the body and before it resolves the cluster;
//! * a member that must be an IAM role ARN is refused with the operation's own
//!   sentence, naming the member and the value;
//! * the association every read answers with reports `disableSessionTags`;
//! * an add-on reports owning exactly the associations the pod identity store
//!   still holds for it.
//!
//! The wire harness is the crate's one copy (`crate::wire`) and the per-operation
//! helpers are the ones the `certificate_authority*` and `pod_identity_associations`
//! suites already own: no suite here grows a second client.

use service_com_amazonaws_eks_awswesleyfrontend::state::{
    CA_DISTRIBUTION_DELAY, CLUSTER_ACTIVATION_DELAY, MAX_CERTIFICATE_AUTHORITIES_PER_CLUSTER,
    UPDATE_COMPLETION_DELAY,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::addon_deletion::delete_addon;
use crate::addon_lifecycle::{create_addon, describe_addon};
use crate::certificate_authority::{
    append_authority, create_cluster, describe_authority, describe_cluster, settle,
};
use crate::certificate_authority_activation::{
    activate_authority, cluster_with_a_distributed_successor,
};
use crate::certificate_authority_deletion::delete_authority;
use crate::certificate_authority_listing::signer_id;
use crate::pod_identity_associations::{
    association_id_of, cluster_fixture, create_association, delete_association,
    describe_association, minimal_create, update_association, CLUSTER, OTHER_ROLE_ARN, REGION,
    ROLE_ARN, TARGET_ROLE_ARN,
};
use crate::wire::{call, encode_label, error_code, Answer};

/// A well-formed association id this emulator never minted: the documented prefix
/// and a body of the documented length.
const NEVER_MINTED_ASSOCIATION_ID: &str = "a-11111111111111111";

/// `ListPodIdentityAssociations`, for proving that a refused create wrote nothing.
///
/// The listing's own suite owns the filtered and paged forms
/// (`crate::list_pod_identity_associations`); what is needed here is the whole of
/// one cluster's set, which is this call with no query at all.
async fn list_associations(router: &MultiServiceRouter, region: &str, cluster: &str) -> Answer {
    call(
        router,
        region,
        "GET",
        &format!(
            "/clusters/{}/pod-identity-associations",
            encode_label(cluster)
        ),
        "",
    )
    .await
}

/// How many associations a cluster holds, as its own listing reports them.
async fn association_count(router: &MultiServiceRouter, region: &str, cluster: &str) -> usize {
    list_associations(router, region, cluster).await.body["associations"]
        .as_array()
        .map_or(0, Vec::len)
}

// ---------------------------------------------------------------------------
// RULE: a rotation step that asks for the state the cluster is already in is
// refused as a request that is invalid GIVEN THAT STATE — 400
// `InvalidRequestException` — and leaves every record it named untouched.
// ---------------------------------------------------------------------------

/// The rows differ along WHICH certificate authority is the signer the request
/// names: the one Amazon EKS provisioned with the cluster, and one a caller's own
/// activation promoted (so the refusal does not depend on who created the CA), on
/// clusters of their own, with and without an idempotency token.
#[tokio::test]
async fn activating_the_certificate_authority_that_already_signs_is_an_invalid_request() {
    let (router, _engine) = crate::wire::emulator_with_engine();

    // A cluster whose signer is the CA Amazon EKS provisioned with it.
    let eks_cluster = "already-active-eks";
    let created = create_cluster(&router, REGION, eks_cluster).await;
    assert_eq!(
        created.status, 200,
        "setup: expected 200 from CreateCluster, got {created:?}"
    );

    // A cluster whose signer is a successor a caller activated, so the CA the
    // refusal names was created and promoted through this API.
    let promoted_cluster = "already-active-promoted";
    let (_, successor) =
        cluster_with_a_distributed_successor(&router, "promoted", promoted_cluster).await;
    let promotion = activate_authority(&router, REGION, promoted_cluster, &successor, None).await;
    assert_eq!(
        promotion.status, 200,
        "setup: expected 200 from ActivateCertificateAuthority, got {promotion:?}"
    );
    settle(UPDATE_COMPLETION_DELAY).await;
    settle(CLUSTER_ACTIVATION_DELAY).await;

    let rows: Vec<(&str, &str, Option<&str>)> = vec![
        (
            "the certificate authority Amazon EKS provisioned with the cluster",
            eks_cluster,
            None,
        ),
        (
            "the same call again, carrying an idempotency token",
            eks_cluster,
            Some("tok-already-active"),
        ),
        (
            "a successor this caller activated, which now signs",
            promoted_cluster,
            None,
        ),
    ];

    for (case, cluster, token) in rows {
        let signer = signer_id(&router, case, cluster).await;
        let before = describe_authority(&router, REGION, cluster, &signer).await;

        let refused = activate_authority(&router, REGION, cluster, &signer, token).await;
        assert_eq!(
            (refused.status, error_code(&refused).as_str()),
            (400, "InvalidRequestException"),
            "case {case:?}: expected 400 InvalidRequestException, got {refused:?}"
        );
        assert_eq!(
            refused.message(),
            format!("Certificate authority {signer} is already active for cluster {cluster}"),
            "case {case:?}: expected the already-active sentence, got {refused:?}"
        );

        // The refusal started no update and moved nothing: the signer is the same
        // CA, with the same activation recorded, and the cluster is still ACTIVE.
        let after = describe_authority(&router, REGION, cluster, &signer).await;
        for member in ["signingStatus", "activatedBy", "activatedAt"] {
            assert_eq!(
                after.body["certificateAuthority"][member],
                before.body["certificateAuthority"][member],
                "case {case:?}: a refused activation must not touch {member}, got {:?}",
                after.body["certificateAuthority"][member]
            );
        }
        let cluster_after = describe_cluster(&router, REGION, cluster).await;
        assert_eq!(
            cluster_after.body["cluster"]["status"],
            serde_json::json!("ACTIVE"),
            "case {case:?}: a refused activation must start no update, got {:?}",
            cluster_after.body["cluster"]["status"]
        );
        assert_eq!(
            signer_id(&router, case, cluster).await,
            signer,
            "case {case:?}: the cluster must still report the same signer"
        );
    }
}

/// The rows differ along the cluster the ceiling is reached on and whether the
/// refused call carries an idempotency token (one identifies no earlier call, so
/// it is not absorbed and still meets the ceiling).
#[tokio::test]
async fn appending_past_the_certificate_authority_ceiling_is_an_invalid_request() {
    let router = crate::wire::emulator();
    let clusters = ["ceiling-invalid-a", "ceiling-invalid-b"];
    for cluster in clusters {
        let created = create_cluster(&router, REGION, cluster).await;
        assert_eq!(
            created.status, 200,
            "setup: expected 200 from CreateCluster for {cluster}, got {created:?}"
        );
    }
    settle(CLUSTER_ACTIVATION_DELAY).await;

    // Fill each cluster to the ceiling: it already holds the CA Amazon EKS
    // provisioned with it, so it takes one fewer append than the ceiling itself.
    for cluster in clusters {
        for index in 1..MAX_CERTIFICATE_AUTHORITIES_PER_CLUSTER {
            let appended =
                append_authority(&router, REGION, cluster, Some(&format!("fill-{index}"))).await;
            assert_eq!(
                appended.status, 200,
                "setup: append {index} on {cluster} is within the ceiling, got {appended:?}"
            );
            settle(CA_DISTRIBUTION_DELAY).await;
        }
    }

    let expected = format!(
        "Cluster already has the maximum number of certificate authorities \
         ({MAX_CERTIFICATE_AUTHORITIES_PER_CLUSTER}). Delete an existing CA before creating a \
         new one."
    );
    for cluster in clusters {
        for token in [None, Some("tok-past-the-ceiling")] {
            let case = (cluster, token);
            let held_before = describe_cluster(&router, REGION, cluster).await;
            let refused = append_authority(&router, REGION, cluster, token).await;
            assert_eq!(
                (refused.status, error_code(&refused).as_str()),
                (400, "InvalidRequestException"),
                "case {case:?}: expected 400 InvalidRequestException, got {refused:?}"
            );
            assert_eq!(
                refused.message(),
                expected,
                "case {case:?}: expected the ceiling sentence, got {refused:?}"
            );
            // The refusal wrote nothing: no update was started.
            let held_after = describe_cluster(&router, REGION, cluster).await;
            assert_eq!(
                held_after.body["cluster"]["status"],
                held_before.body["cluster"]["status"],
                "case {case:?}: a refused append must start no update, got {:?}",
                held_after.body["cluster"]["status"]
            );
        }
    }
}

// ---------------------------------------------------------------------------
// RULE: the certificate authority that signs for a cluster cannot be deleted; the
// refusal is 409 `ResourceInUseException` and carries the remediation that lifts
// it — activate another certificate authority first.
// ---------------------------------------------------------------------------

/// The rows differ along which certificate authority is signing when the delete
/// arrives: the one Amazon EKS provisioned, and the successor a caller's own
/// activation promoted — after which the CA that was signing becomes deletable,
/// which is the other half of the rule.
#[tokio::test]
async fn the_signing_certificate_authority_cannot_be_deleted() {
    let router = crate::wire::emulator();
    let cluster = "delete-active-ca";
    let (outgoing, successor) =
        cluster_with_a_distributed_successor(&router, "signing", cluster).await;

    const EXPECTED: &str = "Cannot delete the active certificate authority. Activate a \
                            different CA first.";

    let refused = delete_authority(&router, REGION, cluster, &outgoing, None).await;
    assert_eq!(
        (refused.status, error_code(&refused).as_str()),
        (409, "ResourceInUseException"),
        "case \"the CA Amazon EKS provisioned, while it signs\": expected 409 \
         ResourceInUseException, got {refused:?}"
    );
    assert_eq!(
        refused.message(),
        EXPECTED,
        "case \"the CA Amazon EKS provisioned, while it signs\": expected the remediation \
         sentence, got {refused:?}"
    );

    // Promote the successor: the CA that is signing is now the other one, so the
    // refusal follows the SIGNER rather than the id the first call named.
    let promotion = activate_authority(&router, REGION, cluster, &successor, None).await;
    assert_eq!(
        promotion.status, 200,
        "setup: expected 200 from ActivateCertificateAuthority, got {promotion:?}"
    );
    settle(UPDATE_COMPLETION_DELAY).await;
    settle(CLUSTER_ACTIVATION_DELAY).await;

    let refused = delete_authority(&router, REGION, cluster, &successor, None).await;
    assert_eq!(
        (refused.status, error_code(&refused).as_str()),
        (409, "ResourceInUseException"),
        "case \"the successor, now that it signs\": expected 409 ResourceInUseException, got \
         {refused:?}"
    );
    assert_eq!(
        refused.message(),
        EXPECTED,
        "case \"the successor, now that it signs\": expected the remediation sentence, got \
         {refused:?}"
    );

    // And the remediation the sentence names works: the retired CA is deletable.
    let deleted = delete_authority(&router, REGION, cluster, &outgoing, None).await;
    assert_eq!(
        deleted.status, 200,
        "case \"the retired CA, once another signs\": the remediation must lift the refusal, \
         got {deleted:?}"
    );
}

// ---------------------------------------------------------------------------
// RULE: one `(namespace, serviceAccount)` pair holds at most one EKS Pod Identity
// association, and the refusal of a second one names the IAM role that pair is
// ALREADY bound to — taken from the stored association, never from the request.
// ---------------------------------------------------------------------------

/// The rows differ along the relationship between the roles of the two calls (a
/// different role, the very same role, a role reached through a target role) and
/// along whether the stored role is the one the create wrote or one an UPDATE
/// since changed it to — which is what shows the role is read from state.
#[tokio::test]
async fn a_second_association_for_one_pair_names_the_role_already_bound() {
    let router = cluster_fixture(REGION, CLUSTER).await;

    struct Row {
        case: &'static str,
        namespace: &'static str,
        service_account: &'static str,
        /// The role the FIRST create binds the pair to.
        first_role: &'static str,
        /// The role the second create offers, which the refusal must NOT name.
        second_role: &'static str,
        /// A role an update moves the association to before the second create, when
        /// the row exercises that.
        updated_to: Option<&'static str>,
    }

    let rows = vec![
        Row {
            case: "a second create with a different role",
            namespace: "default",
            service_account: "conflicting-sa",
            first_role: ROLE_ARN,
            second_role: OTHER_ROLE_ARN,
            updated_to: None,
        },
        Row {
            case: "a second create with the very same role",
            namespace: "default",
            service_account: "duplicate-sa",
            first_role: ROLE_ARN,
            second_role: ROLE_ARN,
            updated_to: None,
        },
        Row {
            case: "a second create in another namespace's name-alike pair",
            namespace: "kube-system",
            service_account: "conflicting-sa",
            first_role: OTHER_ROLE_ARN,
            second_role: ROLE_ARN,
            updated_to: None,
        },
        Row {
            case: "a second create after an update changed the bound role",
            namespace: "updated",
            service_account: "moved-sa",
            first_role: ROLE_ARN,
            second_role: ROLE_ARN,
            updated_to: Some(OTHER_ROLE_ARN),
        },
    ];

    for row in rows {
        let case = row.case;
        let first = create_association(
            &router,
            REGION,
            CLUSTER,
            &serde_json::json!({
                "namespace": row.namespace,
                "serviceAccount": row.service_account,
                "roleArn": row.first_role,
            }),
        )
        .await;
        assert_eq!(
            first.status, 200,
            "case {case:?}: the first create must succeed, got {first:?}"
        );
        let association_id = association_id_of(&first);

        let bound_role = match row.updated_to {
            None => row.first_role.to_string(),
            Some(role) => {
                let updated = update_association(
                    &router,
                    REGION,
                    CLUSTER,
                    &association_id,
                    &serde_json::json!({ "roleArn": role }),
                )
                .await;
                assert_eq!(
                    updated.status, 200,
                    "case {case:?}: the update must succeed, got {updated:?}"
                );
                role.to_string()
            }
        };

        let refused = create_association(
            &router,
            REGION,
            CLUSTER,
            &serde_json::json!({
                "namespace": row.namespace,
                "serviceAccount": row.service_account,
                "roleArn": row.second_role,
            }),
        )
        .await;
        assert_eq!(
            (refused.status, error_code(&refused).as_str()),
            (409, "ResourceInUseException"),
            "case {case:?}: expected 409 ResourceInUseException, got {refused:?}"
        );
        assert_eq!(
            refused.message(),
            format!(
                "The service account is already associated with a different IAM role: \
                 {bound_role}"
            ),
            "case {case:?}: the refusal names the role the pair is bound to, got {refused:?}"
        );

        // The refused create wrote nothing: the pair still resolves to the one
        // association, with the role it was bound to.
        let described = describe_association(&router, REGION, CLUSTER, &association_id).await;
        assert_eq!(
            described.association()["roleArn"],
            serde_json::json!(bound_role),
            "case {case:?}: the stored association is untouched, got {:?}",
            described.association()
        );
    }

    // The same pair in ANOTHER namespace is a different pair, so it is created
    // rather than refused: the conflict is about the pair, not the name.
    let other_namespace = create_association(
        &router,
        REGION,
        CLUSTER,
        &serde_json::json!({
            "namespace": "elsewhere",
            "serviceAccount": "conflicting-sa",
            "roleArn": ROLE_ARN,
        }),
    )
    .await;
    assert_eq!(
        other_namespace.status, 200,
        "case \"the same service account in another namespace\": expected a created \
         association, got {other_namespace:?}"
    );
}

// ---------------------------------------------------------------------------
// RULE: an `associationId` that resolves to no association is 404
// `ResourceNotFoundException` with ONE sentence, naming the id the request
// carried, whichever operation resolved it.
// ---------------------------------------------------------------------------

/// The rows differ along WHY the id resolves to nothing (never minted, or minted
/// and then deleted) and along which operation resolved it, because the sentence
/// is the same in every combination.
#[tokio::test]
async fn an_association_id_that_names_nothing_is_reported_with_one_sentence() {
    let router = cluster_fixture(REGION, CLUSTER).await;

    let created =
        create_association(&router, REGION, CLUSTER, &minimal_create("default", "gone-sa")).await;
    assert_eq!(
        created.status, 200,
        "setup: the association to delete must be created, got {created:?}"
    );
    let deleted_id = association_id_of(&created);
    let deleted = delete_association(&router, REGION, CLUSTER, &deleted_id).await;
    assert_eq!(
        deleted.status, 200,
        "setup: the association must delete, got {deleted:?}"
    );

    for (why, association_id) in [
        ("an id this emulator never minted", NEVER_MINTED_ASSOCIATION_ID.to_string()),
        ("the id of an association just deleted", deleted_id.clone()),
    ] {
        let calls = vec![
            (
                "describe",
                describe_association(&router, REGION, CLUSTER, &association_id).await,
            ),
            (
                "update",
                update_association(
                    &router,
                    REGION,
                    CLUSTER,
                    &association_id,
                    &serde_json::json!({ "roleArn": OTHER_ROLE_ARN }),
                )
                .await,
            ),
            (
                "delete",
                delete_association(&router, REGION, CLUSTER, &association_id).await,
            ),
        ];
        for (operation, answer) in calls {
            let case = (why, operation);
            assert_eq!(
                (answer.status, error_code(&answer).as_str()),
                (404, "ResourceNotFoundException"),
                "case {case:?}: expected 404 ResourceNotFoundException, got {answer:?}"
            );
            assert_eq!(
                answer.message(),
                format!("No service account role association found for ID: {association_id}"),
                "case {case:?}: expected the one not-found sentence, got {answer:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// RULE: `CreatePodIdentityAssociation` authorizes the pass-role of `roleArn`
// FIRST — before it judges any other member of the body and before it resolves
// the cluster — so a value that names no role of this account is 403
// `AccessDeniedException` whatever else the request gets wrong, and nothing is
// written.
// ---------------------------------------------------------------------------

/// The rows differ along what ELSE the request gets wrong (an absent cluster, a
/// malformed cluster name, a malformed namespace, a malformed `targetRoleArn`,
/// nothing at all) and along the form of the unpassable `roleArn` (another
/// account, another partition, not an ARN at all, a bare name, the empty string),
/// because the authorization precedes every one of those answers.
#[tokio::test]
async fn create_authorizes_the_pass_role_before_the_body_and_the_cluster() {
    let router = cluster_fixture(REGION, CLUSTER).await;
    const CROSS_ACCOUNT: &str = "arn:aws:iam::111122223333:role/anything";
    const FIXED_SENTENCE: &str = "Cross-account pass role is not allowed.";

    // Each row: the case, the cluster the call is addressed to, and the body. The
    // `roleArn` of every row names no role of this account.
    let rows: Vec<(&str, &str, serde_json::Value)> = vec![
        (
            "a role of another account, on a cluster that does not exist",
            "no-such-cluster",
            serde_json::json!({
                "namespace": "default", "serviceAccount": "pass-1",
                "roleArn": CROSS_ACCOUNT,
            }),
        ),
        (
            "a value that is not an ARN at all",
            CLUSTER,
            serde_json::json!({
                "namespace": "default", "serviceAccount": "pass-2",
                "roleArn": "not-an-arn-at-all",
            }),
        ),
        (
            "a value that is not an ARN, on a cluster that does not exist",
            "no-such-cluster",
            serde_json::json!({
                "namespace": "default", "serviceAccount": "pass-3",
                "roleArn": "not-an-arn-at-all",
            }),
        ),
        (
            "a bare role name beside a malformed namespace",
            CLUSTER,
            serde_json::json!({
                "namespace": "Not-A-Namespace", "serviceAccount": "pass-4",
                "roleArn": "eksPodIdentityRole",
            }),
        ),
        (
            "the empty string beside a malformed cluster name",
            "-not-a-cluster-name",
            serde_json::json!({
                "namespace": "default", "serviceAccount": "pass-5",
                "roleArn": "",
            }),
        ),
        (
            "a role of another account beside a malformed targetRoleArn",
            CLUSTER,
            serde_json::json!({
                "namespace": "default", "serviceAccount": "pass-6",
                "roleArn": CROSS_ACCOUNT, "targetRoleArn": "not-an-arn",
            }),
        ),
        (
            "another vendor's ARN",
            CLUSTER,
            serde_json::json!({
                "namespace": "default", "serviceAccount": "pass-7",
                "roleArn": "arn:aws:eks:us-west-2:000000000000:cluster/other",
            }),
        ),
    ];

    let held_before = association_count(&router, REGION, CLUSTER).await;
    for (case, cluster, body) in rows {
        let refused = create_association(&router, REGION, cluster, &body).await;
        assert_eq!(
            (refused.status, error_code(&refused).as_str()),
            (403, "AccessDeniedException"),
            "case {case:?}: expected 403 AccessDeniedException, got {refused:?}"
        );
        assert_eq!(
            refused.message(),
            FIXED_SENTENCE,
            "case {case:?}: expected the fixed cross-account sentence, got {refused:?}"
        );
    }
    assert_eq!(
        association_count(&router, REGION, CLUSTER).await,
        held_before,
        "a refused create must leave no association behind"
    );

    // A role of this account passes the authorization, so each row below is
    // answered by the layer BEHIND it — which is what makes the 403 above an
    // ordering fact rather than a blanket refusal.
    let behind: Vec<(&str, &str, serde_json::Value, u16, &str)> = vec![
        (
            "a passable role on a cluster that does not exist",
            "no-such-cluster",
            serde_json::json!({
                "namespace": "default", "serviceAccount": "behind-1", "roleArn": ROLE_ARN,
            }),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a passable role with a malformed namespace",
            CLUSTER,
            serde_json::json!({
                "namespace": "Not-A-Namespace", "serviceAccount": "behind-2",
                "roleArn": ROLE_ARN,
            }),
            400,
            "InvalidParameterException",
        ),
        (
            "a passable role with a malformed targetRoleArn",
            CLUSTER,
            serde_json::json!({
                "namespace": "default", "serviceAccount": "behind-3",
                "roleArn": ROLE_ARN, "targetRoleArn": "not-an-arn",
            }),
            400,
            "InvalidParameterException",
        ),
    ];
    for (case, cluster, body, status, code) in behind {
        let answer = create_association(&router, REGION, cluster, &body).await;
        assert_eq!(
            (answer.status, error_code(&answer).as_str()),
            (status, code),
            "case {case:?}: expected {status} {code}, got {answer:?}"
        );
    }

    // And the whole request being right is still a created association.
    let created = create_association(
        &router,
        REGION,
        CLUSTER,
        &minimal_create("default", "behind-ok"),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case \"a passable role and a well-formed body\": expected a created association, got \
         {created:?}"
    );
}

// ---------------------------------------------------------------------------
// RULE: a member of the EKS Pod Identity surface that must be an IAM role ARN is
// refused with the operation's OWN sentence, naming the member as the model
// spells it and the value the request carried.
// ---------------------------------------------------------------------------

/// The rows span the members that carry such a value and are reported this way
/// (`UpdatePodIdentityAssociation`'s `targetRoleArn` and
/// `CreatePodIdentityAssociation`'s `targetRoleArn`) and the ways a value leaves
/// the form (not an ARN, an IAM ARN naming something other than a role, an IAM
/// ARN carrying a Region, another vendor's ARN, an account that is not 12
/// digits).
///
/// `roleArn` is on NEITHER operation's list: that member is passed to Amazon EKS,
/// so a value outside the form names no role of this account and is answered by
/// the pass-role authorization (403), which the row below pins for the update the
/// way `create_authorizes_the_pass_role_before_the_body_and_the_cluster` pins it
/// for the create.
#[tokio::test]
async fn a_member_that_is_not_an_iam_role_arn_earns_the_operations_own_sentence() {
    let router = cluster_fixture(REGION, CLUSTER).await;
    let created =
        create_association(&router, REGION, CLUSTER, &minimal_create("default", "arn-sa")).await;
    assert_eq!(
        created.status, 200,
        "setup: the association to update must be created, got {created:?}"
    );
    let association_id = association_id_of(&created);

    let malformed = [
        "not-a-valid-iam-role-arn",
        "arn:aws:iam::000000000000:user/someone",
        "arn:aws:iam:us-west-2:000000000000:role/regional",
        "arn:aws:eks:us-west-2:000000000000:cluster/other",
        "arn:aws:iam::12345:role/short-account",
        "arn:aws:iam::000000000000:role/",
    ];

    for value in malformed {
        // The update reports its target role this way.
        for member in ["targetRoleArn"] {
            let case = ("update", member, value);
            let refused = update_association(
                &router,
                REGION,
                CLUSTER,
                &association_id,
                &serde_json::json!({ member: value }),
            )
            .await;
            assert_eq!(
                (refused.status, error_code(&refused).as_str()),
                (400, "InvalidParameterException"),
                "case {case:?}: expected 400 InvalidParameterException, got {refused:?}"
            );
            assert_eq!(
                refused.message(),
                format!("The provided {member} {value} is invalid."),
                "case {case:?}: expected the operation's own sentence, got {refused:?}"
            );
        }

        // The same value as the update's `roleArn` is the PASS-ROLE answer
        // instead: a value outside the form names no role of this account, and
        // passing a role is what an update does with that member too.
        let case = ("update", "roleArn", value);
        let refused = update_association(
            &router,
            REGION,
            CLUSTER,
            &association_id,
            &serde_json::json!({ "roleArn": value }),
        )
        .await;
        assert_eq!(
            (refused.status, error_code(&refused).as_str()),
            (403, "AccessDeniedException"),
            "case {case:?}: expected the pass-role refusal, got {refused:?}"
        );

        // The create reports `targetRoleArn` the same way. Its `roleArn` is NOT
        // here: that one is passed to Amazon EKS, so it is answered by the
        // pass-role authorization above.
        let case = ("create", "targetRoleArn", value);
        let refused = create_association(
            &router,
            REGION,
            CLUSTER,
            &serde_json::json!({
                "namespace": "default",
                "serviceAccount": "arn-create-sa",
                "roleArn": ROLE_ARN,
                "targetRoleArn": value,
            }),
        )
        .await;
        assert_eq!(
            (refused.status, error_code(&refused).as_str()),
            (400, "InvalidParameterException"),
            "case {case:?}: expected 400 InvalidParameterException, got {refused:?}"
        );
        assert_eq!(
            refused.message(),
            format!("The provided targetRoleArn {value} is invalid."),
            "case {case:?}: expected the operation's own sentence, got {refused:?}"
        );
    }

    // The stored association took none of it.
    let described = describe_association(&router, REGION, CLUSTER, &association_id).await;
    assert_eq!(
        described.association()["roleArn"],
        serde_json::json!(ROLE_ARN),
        "a refused update must change nothing, got {:?}",
        described.association()
    );
}

// ---------------------------------------------------------------------------
// RULE: `Addon$podIdentityAssociations` is "an array of EKS Pod Identity
// associations OWNED BY the add-on", so it reports exactly the associations the
// pod identity store still holds for that add-on: one a caller deleted directly
// is owned by nothing and is not reported any more.
// ---------------------------------------------------------------------------

/// The rows differ along how many of the add-on's associations are deleted and in
/// which order, because the rule is about the SET the add-on owns rather than
/// about one association: the reported list shrinks by exactly the one that went,
/// and the rest keep both their order and their ARNs.
#[tokio::test]
async fn an_addon_reports_only_the_associations_the_store_still_holds() {
    let router = crate::wire::emulator();
    let cluster = "addon-owned-associations";
    let created_cluster = create_cluster(&router, REGION, cluster).await;
    assert_eq!(
        created_cluster.status, 200,
        "setup: expected 200 from CreateCluster, got {created_cluster:?}"
    );
    settle(CLUSTER_ACTIVATION_DELAY).await;

    let addon_name = "amazon-cloudwatch-observability";
    let installed = create_addon(
        &router,
        cluster,
        &serde_json::json!({
            "addonName": addon_name,
            "podIdentityAssociations": [
                { "serviceAccount": "owned-a", "roleArn": ROLE_ARN },
                { "serviceAccount": "owned-b", "roleArn": OTHER_ROLE_ARN },
            ],
        }),
    )
    .await;
    assert_eq!(
        installed.status, 200,
        "setup: expected 200 from CreateAddon, got {installed:?}"
    );

    /// The association ARNs an add-on reports owning, in the order it reports them.
    fn reported(addon: &serde_json::Value) -> Vec<String> {
        addon["podIdentityAssociations"]
            .as_array()
            .map(|arns| {
                arns.iter()
                    .filter_map(|arn| arn.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default()
    }

    let owned = reported(installed.addon());
    assert_eq!(
        owned.len(),
        2,
        "setup: the add-on must own both associations it asked for, got {installed:?}"
    );

    // Delete the FIRST one directly through the pod identity slice: the add-on
    // then owns only the second, and that is what it reports.
    let first_id = owned[0].rsplit('/').next().unwrap_or_default().to_owned();
    let deleted = delete_association(&router, REGION, cluster, &first_id).await;
    assert_eq!(
        deleted.status, 200,
        "case \"one of two deleted directly\": the association must delete, got {deleted:?}"
    );
    let described = describe_addon(&router, cluster, addon_name).await;
    assert_eq!(
        reported(described.addon()),
        vec![owned[1].clone()],
        "case \"one of two deleted directly\": the add-on reports only what the store still \
         holds, got {:?}",
        described.addon()["podIdentityAssociations"]
    );

    // Delete the second one too: the add-on owns none, so it reports none at all —
    // the same ABSENT member (not an empty array) an add-on that was never asked
    // for an association reports, and the form `crate::addon_update` already pins
    // for an update that empties the set.
    let second_id = owned[1].rsplit('/').next().unwrap_or_default().to_owned();
    let deleted = delete_association(&router, REGION, cluster, &second_id).await;
    assert_eq!(
        deleted.status, 200,
        "case \"both deleted directly\": the second association must delete, got {deleted:?}"
    );
    let described = describe_addon(&router, cluster, addon_name).await;
    assert!(
        described.addon()["podIdentityAssociations"].is_null(),
        "case \"both deleted directly\": an add-on that owns nothing reports no \
         associations, got {:?}",
        described.addon()["podIdentityAssociations"]
    );

    // And the delete of the add-on answers the record that left: it owned none by
    // then, so it reports none.
    let removed = delete_addon(&router, cluster, addon_name, None).await;
    assert_eq!(
        removed.status, 200,
        "case \"both deleted directly\": expected 200 from DeleteAddon, got {removed:?}"
    );
    assert!(
        removed.addon()["podIdentityAssociations"].is_null(),
        "case \"both deleted directly\": the delete answers the record that left, got {:?}",
        removed.addon()["podIdentityAssociations"]
    );
}

// ---------------------------------------------------------------------------
// RULE: an association that carries a `policy` must have the automatic session
// tags disabled ("When using this policy, disableSessionTags must be set to
// true"), and a request that leaves the pair broken is refused with the one
// sentence that states the pairing — whichever call supplies which half.
// ---------------------------------------------------------------------------

/// The rows span which call breaks the pairing and how: a create that sends the
/// policy with the flag absent or explicitly `false`, and an update that sends a
/// policy onto an association whose flag is off, or turns the flag back on under a
/// policy that is already there.
#[tokio::test]
async fn a_policy_without_disabled_session_tags_is_refused_with_one_sentence() {
    let router = cluster_fixture(REGION, CLUSTER).await;
    const EXPECTED: &str = "When policy is specified, disableSessionTags must be set to true";
    const POLICY: &str = "{\"Version\":\"2012-10-17\",\"Statement\":[]}";

    // The create half.
    let creates: Vec<(&str, serde_json::Value)> = vec![
        (
            "a create with a policy and no flag at all",
            serde_json::json!({
                "namespace": "default", "serviceAccount": "pairing-1",
                "roleArn": ROLE_ARN, "policy": POLICY,
            }),
        ),
        (
            "a create with a policy and the flag explicitly false",
            serde_json::json!({
                "namespace": "default", "serviceAccount": "pairing-2",
                "roleArn": ROLE_ARN, "policy": POLICY, "disableSessionTags": false,
            }),
        ),
    ];
    for (case, body) in creates {
        let refused = create_association(&router, REGION, CLUSTER, &body).await;
        assert_eq!(
            (refused.status, error_code(&refused).as_str()),
            (400, "InvalidParameterException"),
            "case {case:?}: expected 400 InvalidParameterException, got {refused:?}"
        );
        assert_eq!(
            refused.message(),
            EXPECTED,
            "case {case:?}: expected the pairing sentence, got {refused:?}"
        );
    }

    // The update half: the rule holds for the association the request LEAVES
    // behind, so both halves of the pair can come from either call.
    let plain =
        create_association(&router, REGION, CLUSTER, &minimal_create("default", "pairing-3")).await;
    assert_eq!(
        plain.status, 200,
        "setup: the association to update must be created, got {plain:?}"
    );
    let plain_id = association_id_of(&plain);

    let restricted = create_association(
        &router,
        REGION,
        CLUSTER,
        &serde_json::json!({
            "namespace": "default", "serviceAccount": "pairing-4",
            "roleArn": ROLE_ARN, "policy": POLICY, "disableSessionTags": true,
        }),
    )
    .await;
    assert_eq!(
        restricted.status, 200,
        "setup: the restricted association must be created, got {restricted:?}"
    );
    let restricted_id = association_id_of(&restricted);

    let updates: Vec<(&str, &str, serde_json::Value)> = vec![
        (
            "an update adding a policy to an association whose flag is off",
            plain_id.as_str(),
            serde_json::json!({ "policy": POLICY }),
        ),
        (
            "an update turning the flag back on under a stored policy",
            restricted_id.as_str(),
            serde_json::json!({ "disableSessionTags": false }),
        ),
    ];
    for (case, association_id, body) in updates {
        let refused = update_association(&router, REGION, CLUSTER, association_id, &body).await;
        assert_eq!(
            (refused.status, error_code(&refused).as_str()),
            (400, "InvalidParameterException"),
            "case {case:?}: expected 400 InvalidParameterException, got {refused:?}"
        );
        assert_eq!(
            refused.message(),
            EXPECTED,
            "case {case:?}: expected the pairing sentence, got {refused:?}"
        );
    }

    // And the pair supplied together on one update is accepted, so the refusals
    // above are about the PAIR and not about either member.
    let accepted = update_association(
        &router,
        REGION,
        CLUSTER,
        &plain_id,
        &serde_json::json!({ "policy": POLICY, "disableSessionTags": true }),
    )
    .await;
    assert_eq!(
        accepted.status, 200,
        "case \"an update supplying both halves\": expected the update to be accepted, got \
         {accepted:?}"
    );
}

// ---------------------------------------------------------------------------
// RULE: `disableSessionTags` is a flag with two states, so every association this
// service answers with reports it: `false` — EKS Pod Identity appends its session
// tags — unless the association was created or updated to disable them.
// ---------------------------------------------------------------------------

/// The rows differ along whether the create supplied the flag and with which
/// value, and the assertion is made on every call that answers an association
/// (create, describe, update, delete), because the flag is a member of the
/// association rather than of one response.
#[tokio::test]
async fn every_association_reports_whether_session_tags_are_disabled() {
    let router = cluster_fixture(REGION, CLUSTER).await;

    let rows: Vec<(&str, serde_json::Value, bool)> = vec![
        (
            "a create that does not mention the flag",
            minimal_create("default", "tags-default-sa"),
            false,
        ),
        (
            "a create that leaves session tags on explicitly",
            serde_json::json!({
                "namespace": "default", "serviceAccount": "tags-on-sa",
                "roleArn": ROLE_ARN, "disableSessionTags": false,
            }),
            false,
        ),
        (
            "a create that disables them",
            serde_json::json!({
                "namespace": "default", "serviceAccount": "tags-off-sa",
                "roleArn": ROLE_ARN, "disableSessionTags": true,
            }),
            true,
        ),
        (
            "a create that disables them for a policy, with a target role",
            serde_json::json!({
                "namespace": "default", "serviceAccount": "tags-policy-sa",
                "roleArn": ROLE_ARN, "targetRoleArn": TARGET_ROLE_ARN,
                "policy": "{\"Version\":\"2012-10-17\",\"Statement\":[]}",
                "disableSessionTags": true,
            }),
            true,
        ),
    ];

    for (case, request, expected) in rows {
        let created = create_association(&router, REGION, CLUSTER, &request).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected a created association, got {created:?}"
        );
        let association_id = association_id_of(&created);
        assert_eq!(
            created.association()["disableSessionTags"],
            serde_json::json!(expected),
            "case {case:?}: the create reports the flag, got {:?}",
            created.association()["disableSessionTags"]
        );

        let described = describe_association(&router, REGION, CLUSTER, &association_id).await;
        assert_eq!(
            described.association()["disableSessionTags"],
            serde_json::json!(expected),
            "case {case:?}: a describe reports the same flag, got {:?}",
            described.association()["disableSessionTags"]
        );

        // An update that says nothing about the flag leaves it where it was, and
        // one that flips it is reported flipped.
        let untouched = update_association(
            &router,
            REGION,
            CLUSTER,
            &association_id,
            &serde_json::json!({ "roleArn": OTHER_ROLE_ARN }),
        )
        .await;
        assert_eq!(
            untouched.status, 200,
            "case {case:?}: an update that does not mention the flag is accepted, got \
             {untouched:?}"
        );
        assert_eq!(
            untouched.association()["disableSessionTags"],
            serde_json::json!(expected),
            "case {case:?}: an update that does not mention the flag leaves it, got {:?}",
            untouched.association()["disableSessionTags"]
        );

        let flipped = update_association(
            &router,
            REGION,
            CLUSTER,
            &association_id,
            &serde_json::json!({ "disableSessionTags": !expected }),
        )
        .await;
        if request.get("policy").is_some() && expected {
            // "When using this policy, disableSessionTags must be set to true": an
            // update that would enable session tags under a policy is refused, so
            // the flag stays as it was.
            assert_eq!(
                (flipped.status, error_code(&flipped).as_str()),
                (400, "InvalidParameterException"),
                "case {case:?}: enabling session tags under a policy is refused, got \
                 {flipped:?}"
            );
        } else {
            assert_eq!(
                flipped.association()["disableSessionTags"],
                serde_json::json!(!expected),
                "case {case:?}: an update that flips the flag reports it flipped, got {:?}",
                flipped.association()["disableSessionTags"]
            );
            // The delete answers "the full description of the association that was
            // deleted", so it reports the flag too.
            let deleted = delete_association(&router, REGION, CLUSTER, &association_id).await;
            assert_eq!(
                deleted.association()["disableSessionTags"],
                serde_json::json!(!expected),
                "case {case:?}: the delete reports the flag of the record it removed, got {:?}",
                deleted.association()["disableSessionTags"]
            );
        }
    }
}
