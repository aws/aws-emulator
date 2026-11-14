// SPDX-License-Identifier: CC0-1.0

//! `ModifyClusterIamRoles`: the IAM roles a cluster can use to reach other
//! services, end to end over the awsQuery wire.
//!
//! The suite asserts the RULE rather than one recorded request: a cluster's role
//! list is a SET keyed by role ARN, `AddIamRoles` associates and `RemoveIamRoles`
//! disassociates, each change is reported through the `ApplyStatus` the model
//! publishes, and the resulting set is what a later `DescribeClusters` reports.
//! The tables span the cases the rule generalises over — several roles in one
//! call, a role already associated, a role that is not associated, both lists in
//! one call, and ARN forms this service never resolves.

use service_com_amazonaws_redshift_redshiftserviceversion20121201::state;

use crate::cluster_lifecycle::{all_text, Answer, Emulator};

/// The example role ARN the model itself gives for `ClusterIamRole$IamRoleArn`
/// ("for example, `arn:aws:iam::123456789012:role/RedshiftCopyUnload`"), and
/// three more of the same documented form.
const ROLE_COPY_UNLOAD: &str = "arn:aws:iam::123456789012:role/RedshiftCopyUnload";
const ROLE_SPECTRUM: &str = "arn:aws:iam::123456789012:role/RedshiftSpectrum";
const ROLE_OTHER_ACCOUNT: &str = "arn:aws:iam::210987654321:role/CrossAccountCopy";
/// A role ARN carrying a PATH, which is the other form an IAM role ARN takes.
const ROLE_WITH_PATH: &str = "arn:aws:iam::123456789012:role/service-role/NestedName";
/// A role nothing ever created. An IAM role belongs to another service, so this
/// service does not resolve one: associating this ARN is accepted like any other.
const ROLE_NONEXISTENT: &str = "arn:aws:iam::123456789012:role/NoSuchRoleAnywhere";

/// `call` takes borrowed pairs, so a case that builds owned parameters borrows
/// them through this.
fn borrowed(params: &[(String, String)]) -> Vec<(&str, &str)> {
    params
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect()
}

/// The `(IamRoleArn, ApplyStatus)` of every association in a body's `IamRoles`
/// list, in the order the response reports them.
fn associations(body: &str) -> Vec<(String, String)> {
    let open = "<IamRoles>";
    let Some(start) = body.find(open) else {
        return Vec::new();
    };
    let rest = &body[start + open.len()..];
    let end = rest.find("</IamRoles>").unwrap_or(rest.len());
    let block = &rest[..end];
    // Both members are always set on an association this service stores, so the
    // two lists pair up position by position.
    all_text(block, "IamRoleArn")
        .into_iter()
        .zip(all_text(block, "ApplyStatus"))
        .collect()
}

/// Just the role ARNs a body's `IamRoles` list carries, in report order.
fn associated_arns(body: &str) -> Vec<String> {
    associations(body)
        .into_iter()
        .map(|(arn, _)| arn)
        .collect()
}

/// Create one cluster, with `roles` associated at creation, and leave it in the
/// `available` state — the state a configuration change is admitted in.
async fn available_cluster(emulator: &Emulator, identifier: &str, roles: &[&str]) {
    let mut params: Vec<(String, String)> = vec![
        ("ClusterIdentifier".to_string(), identifier.to_string()),
        ("NodeType".to_string(), "ra3.xlplus".to_string()),
        ("MasterUsername".to_string(), "admin".to_string()),
    ];
    for (position, arn) in roles.iter().enumerate() {
        params.push((
            format!("IamRoles.IamRoleArn.{}", position + 1),
            (*arn).to_string(),
        ));
    }
    let created = emulator
        .call("CreateCluster", &borrowed(&params))
        .await;
    assert_eq!(
        created.status, 200,
        "case seed cluster {identifier}: expected 200, got {} with body {}",
        created.status, created.body
    );
    settle(emulator);
}

/// Cross the span a clock-owed change falls due after, by ADVANCING this
/// emulator's own clock across it rather than waiting for it: the transition is
/// then materialised by the next read whatever the host is doing, so no case of
/// this suite can pass on one machine and fail on a loaded one.
fn settle(emulator: &Emulator) {
    emulator.advance_clock(state::PROVISIONING_SPAN);
}

/// `ModifyClusterIamRoles` with these add and remove lists.
async fn modify_roles(
    emulator: &Emulator,
    cluster_identifier: &str,
    add: &[&str],
    remove: &[&str],
) -> Answer {
    let mut params: Vec<(String, String)> = vec![(
        "ClusterIdentifier".to_string(),
        cluster_identifier.to_string(),
    )];
    for (position, arn) in add.iter().enumerate() {
        params.push((
            format!("AddIamRoles.IamRoleArn.{}", position + 1),
            (*arn).to_string(),
        ));
    }
    for (position, arn) in remove.iter().enumerate() {
        params.push((
            format!("RemoveIamRoles.IamRoleArn.{}", position + 1),
            (*arn).to_string(),
        ));
    }
    emulator
        .call("ModifyClusterIamRoles", &borrowed(&params))
        .await
}

/// `DescribeClusters` for one cluster.
async fn describe(emulator: &Emulator, cluster_identifier: &str) -> Answer {
    emulator
        .call(
            "DescribeClusters",
            &[("ClusterIdentifier", cluster_identifier)],
        )
        .await
}

/// One row of the association table: the case name, the `AddIamRoles` and
/// `RemoveIamRoles` the call carries, the associations the CALL must report back
/// (ARN and `ApplyStatus`), and the set a later read must report once the
/// transient statuses have fallen due.
type AssociationCase = (
    &'static str,
    &'static [&'static str],
    &'static [&'static str],
    &'static [(&'static str, &'static str)],
    &'static [&'static str],
);

#[tokio::test]
async fn add_and_remove_move_the_cluster_role_set_through_its_apply_statuses() {
    let emulator = Emulator::new();
    let cluster = "roles-cluster";
    available_cluster(&emulator, cluster, &[ROLE_COPY_UNLOAD]).await;

    let adding = state::IAM_ROLE_APPLY_STATUS_ADDING;
    let removing = state::IAM_ROLE_APPLY_STATUS_REMOVING;
    let in_sync = state::IAM_ROLE_APPLY_STATUS_IN_SYNC;

    // Every row runs against the set the row before it left, so the table is one
    // walk over the rule: several roles at once, a role already associated, a
    // role that is not associated, both lists in one call, and the same ARN in
    // both lists.
    let cases: Vec<AssociationCase> = vec![
        (
            "two roles associated in one call",
            &[ROLE_SPECTRUM, ROLE_WITH_PATH],
            &[],
            &[
                (ROLE_COPY_UNLOAD, "in-sync"),
                (ROLE_SPECTRUM, "adding"),
                (ROLE_WITH_PATH, "adding"),
            ],
            &[ROLE_COPY_UNLOAD, ROLE_SPECTRUM, ROLE_WITH_PATH],
        ),
        (
            "one role disassociated",
            &[],
            &[ROLE_SPECTRUM],
            &[
                (ROLE_COPY_UNLOAD, "in-sync"),
                (ROLE_SPECTRUM, "removing"),
                (ROLE_WITH_PATH, "in-sync"),
            ],
            &[ROLE_COPY_UNLOAD, ROLE_WITH_PATH],
        ),
        (
            "a role already associated is not a second entry",
            &[ROLE_WITH_PATH],
            &[],
            &[(ROLE_COPY_UNLOAD, "in-sync"), (ROLE_WITH_PATH, "in-sync")],
            &[ROLE_COPY_UNLOAD, ROLE_WITH_PATH],
        ),
        (
            "disassociating a role the cluster does not carry changes nothing",
            &[],
            &[ROLE_OTHER_ACCOUNT],
            &[(ROLE_COPY_UNLOAD, "in-sync"), (ROLE_WITH_PATH, "in-sync")],
            &[ROLE_COPY_UNLOAD, ROLE_WITH_PATH],
        ),
        (
            "both lists in one call",
            &[ROLE_NONEXISTENT],
            &[ROLE_COPY_UNLOAD],
            &[
                (ROLE_COPY_UNLOAD, "removing"),
                (ROLE_WITH_PATH, "in-sync"),
                (ROLE_NONEXISTENT, "adding"),
            ],
            &[ROLE_WITH_PATH, ROLE_NONEXISTENT],
        ),
        (
            "an ARN in both lists ends up associated",
            &[ROLE_OTHER_ACCOUNT],
            &[ROLE_OTHER_ACCOUNT],
            &[
                (ROLE_WITH_PATH, "in-sync"),
                (ROLE_NONEXISTENT, "in-sync"),
                (ROLE_OTHER_ACCOUNT, "adding"),
            ],
            &[ROLE_WITH_PATH, ROLE_NONEXISTENT, ROLE_OTHER_ACCOUNT],
        ),
    ];

    for (case, add, remove, reported, settled) in cases {
        let answered = modify_roles(&emulator, cluster, add, remove).await;
        assert_eq!(
            answered.status, 200,
            "case {case}: expected 200, got {} with body {}",
            answered.status, answered.body
        );
        let expected: Vec<(String, String)> = reported
            .iter()
            .map(|(arn, status)| ((*arn).to_string(), (*status).to_string()))
            .collect();
        assert_eq!(
            associations(&answered.body),
            expected,
            "case {case}: expected the call to report {expected:?}, got {:?}",
            associations(&answered.body)
        );
        // The published statuses are the only three a stored association is
        // ever in.
        for (arn, status) in associations(&answered.body) {
            assert!(
                [adding, removing, in_sync].contains(&status.as_str()),
                "case {case}: {arn} carries an unpublished ApplyStatus {status:?}"
            );
        }

        // The call left the service applying the change, so the cluster reports
        // the work in progress and is only intermittently available for queries.
        assert_eq!(
            answered.text("ClusterStatus").as_deref(),
            Some(state::STATUS_MODIFYING),
            "case {case}: expected the cluster to report {:?} while the change is applied, got {:?}",
            state::STATUS_MODIFYING,
            answered.text("ClusterStatus")
        );
        assert_eq!(
            answered.text("ClusterAvailabilityStatus").as_deref(),
            Some("Modifying"),
            "case {case}: expected ClusterAvailabilityStatus=Modifying, got {:?}",
            answered.text("ClusterAvailabilityStatus")
        );

        settle(&emulator);
        let read = describe(&emulator, cluster).await;
        assert_eq!(
            read.status, 200,
            "case {case}: expected the read to answer 200, got {} with body {}",
            read.status, read.body
        );
        assert_eq!(
            read.text("ClusterStatus").as_deref(),
            Some(state::STATUS_AVAILABLE),
            "case {case}: expected the cluster to be {:?} again once the change fell due, got {:?}",
            state::STATUS_AVAILABLE,
            read.text("ClusterStatus")
        );
        let expected_set: Vec<String> = settled.iter().map(|arn| (*arn).to_string()).collect();
        assert_eq!(
            associated_arns(&read.body),
            expected_set,
            "case {case}: expected the cluster to carry {expected_set:?}, got {:?}",
            associated_arns(&read.body)
        );
        for (arn, status) in associations(&read.body) {
            assert_eq!(
                status, in_sync,
                "case {case}: expected {arn} to be {in_sync} once the change fell due, got {status:?}"
            );
        }
    }
}

/// One row of the default-role table: the case name, the `DefaultIamRoleArn` the
/// call carries (`None` when it carries none) and the ARN the cluster must
/// report afterwards.
type DefaultRoleCase = (
    &'static str,
    Option<&'static str>,
    Option<&'static str>,
);

#[tokio::test]
async fn the_default_role_follows_the_member_the_call_carries() {
    let emulator = Emulator::new();
    let cluster = "default-role-cluster";
    available_cluster(&emulator, cluster, &[ROLE_COPY_UNLOAD, ROLE_SPECTRUM]).await;

    let cases: Vec<DefaultRoleCase> = vec![
        ("a default role is set", Some(ROLE_COPY_UNLOAD), Some(ROLE_COPY_UNLOAD)),
        ("another ARN replaces it", Some(ROLE_SPECTRUM), Some(ROLE_SPECTRUM)),
        ("a call carrying none leaves it", None, Some(ROLE_SPECTRUM)),
        ("an empty ARN names no role", Some(""), None),
    ];

    for (case, carried, expected) in cases {
        let mut params: Vec<(String, String)> =
            vec![("ClusterIdentifier".to_string(), cluster.to_string())];
        if let Some(arn) = carried {
            params.push(("DefaultIamRoleArn".to_string(), arn.to_string()));
        }
        let answered = emulator
            .call("ModifyClusterIamRoles", &borrowed(&params))
            .await;
        assert_eq!(
            answered.status, 200,
            "case {case}: expected 200, got {} with body {}",
            answered.status, answered.body
        );
        assert_eq!(
            answered.text("DefaultIamRoleArn").as_deref(),
            expected,
            "case {case}: expected the call to report {expected:?}, got {:?}",
            answered.text("DefaultIamRoleArn")
        );
        // A call that named a change leaves it being applied; one that named
        // none leaves the cluster exactly as it stands.
        let expected_status = if carried.is_some() {
            state::STATUS_MODIFYING
        } else {
            state::STATUS_AVAILABLE
        };
        assert_eq!(
            answered.text("ClusterStatus").as_deref(),
            Some(expected_status),
            "case {case}: expected ClusterStatus={expected_status:?}, got {:?}",
            answered.text("ClusterStatus")
        );
        settle(&emulator);
        let read = describe(&emulator, cluster).await;
        assert_eq!(
            read.text("DefaultIamRoleArn").as_deref(),
            expected,
            "case {case}: expected a later read to report {expected:?}, got {:?}",
            read.text("DefaultIamRoleArn")
        );
        // Nothing the call did not name changes: the role SET is untouched by a
        // call that only names a default.
        assert_eq!(
            associated_arns(&read.body),
            vec![ROLE_COPY_UNLOAD.to_string(), ROLE_SPECTRUM.to_string()],
            "case {case}: expected the association set to be untouched, got {:?}",
            associated_arns(&read.body)
        );
    }

    // The default role is one OF the cluster's roles: disassociating the role
    // set as default leaves the cluster with no default.
    let set_default = emulator
        .call(
            "ModifyClusterIamRoles",
            &[
                ("ClusterIdentifier", cluster),
                ("DefaultIamRoleArn", ROLE_SPECTRUM),
            ],
        )
        .await;
    assert_eq!(
        set_default.status, 200,
        "case default role before its removal: expected 200, got {} with body {}",
        set_default.status, set_default.body
    );
    settle(&emulator);
    let removed = modify_roles(&emulator, cluster, &[], &[ROLE_SPECTRUM]).await;
    assert_eq!(
        removed.text("DefaultIamRoleArn"),
        None,
        "case removing the default role: expected no default to remain, got {:?}",
        removed.text("DefaultIamRoleArn")
    );
    settle(&emulator);
    let read = describe(&emulator, cluster).await;
    assert_eq!(
        read.text("DefaultIamRoleArn"),
        None,
        "case removing the default role: expected a later read to report no default, got {:?}",
        read.text("DefaultIamRoleArn")
    );
    assert_eq!(
        associated_arns(&read.body),
        vec![ROLE_COPY_UNLOAD.to_string()],
        "case removing the default role: expected only the other role to remain, got {:?}",
        associated_arns(&read.body)
    );
}

#[tokio::test]
async fn a_cluster_that_cannot_take_the_change_is_refused() {
    let emulator = Emulator::new();

    // No such cluster: the roles are associated WITH a cluster, so there is
    // nothing to associate them with.
    let unknown = modify_roles(&emulator, "no-such-cluster", &[ROLE_COPY_UNLOAD], &[]).await;
    assert_eq!(
        (unknown.status, unknown.error_code()),
        (404, Some("ClusterNotFound".to_string())),
        "case unknown cluster: expected 404 ClusterNotFound, got {} / {:?} with body {}",
        unknown.status,
        unknown.error_code(),
        unknown.body
    );

    // A cluster still being provisioned is not in the state a configuration
    // change is admitted in. The clock is STOPPED first, so the cluster stays
    // inside its provisioning span however long the calls below take.
    emulator.stop_clock();
    let created = emulator.create_minimal("provisioning-cluster").await;
    assert_eq!(
        created.status, 200,
        "case seed provisioning cluster: expected 200, got {} with body {}",
        created.status, created.body
    );
    let too_early = modify_roles(
        &emulator,
        "provisioning-cluster",
        &[ROLE_COPY_UNLOAD],
        &[],
    )
    .await;
    assert_eq!(
        (too_early.status, too_early.error_code()),
        (400, Some("InvalidClusterState".to_string())),
        "case cluster still creating: expected 400 InvalidClusterState, got {} / {:?} with body {}",
        too_early.status,
        too_early.error_code(),
        too_early.body
    );
    // Refused, so nothing was associated: the cluster still carries no role once
    // it is available.
    settle(&emulator);
    let read = describe(&emulator, "provisioning-cluster").await;
    assert!(
        associated_arns(&read.body).is_empty(),
        "case cluster still creating: expected no association to have been stored, got {:?}",
        associated_arns(&read.body)
    );

    // Once it is available the same call is accepted.
    let accepted = modify_roles(
        &emulator,
        "provisioning-cluster",
        &[ROLE_COPY_UNLOAD],
        &[],
    )
    .await;
    assert_eq!(
        accepted.status, 200,
        "case cluster available: expected 200, got {} with body {}",
        accepted.status, accepted.body
    );
    assert_eq!(
        associated_arns(&accepted.body),
        vec![ROLE_COPY_UNLOAD.to_string()],
        "case cluster available: expected the role to be associated, got {:?}",
        associated_arns(&accepted.body)
    );
}
