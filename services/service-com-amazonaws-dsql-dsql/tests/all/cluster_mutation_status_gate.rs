// SPDX-License-Identifier: CC0-1.0

//! The one status gate every mutation of a cluster record passes through.
//!
//! RULE: a request that WRITES a stored cluster's record is refused with 409
//! `ConflictException` unless the cluster's status accepts a mutation — only a
//! settled cluster does, not one the clock still owes a transition and not one
//! already on its way out — and a refused request leaves the record exactly as it
//! was. The one exemption is the delete of a cluster whose deletion is already
//! under way — either leg of that chain, the peering tear-down
//! (`PENDING_DELETE`) or the removal (`DELETING`) — which answers with the delete
//! already in flight.
//!
//! The first table is the whole class: every `ClusterStatus` the model declares
//! crossed with every operation that writes a cluster record, each case on its own
//! cluster, driven over the wire through the `Dsql` harness in
//! `tests/all/harness.rs` — this crate's one test client. Statuses no caller can
//! drive a cluster into over the wire are reached by seeding the store, which is
//! what makes the table span the catalog rather than the three statuses
//! `CreateCluster`, `UpdateCluster` and `DeleteCluster` happen to produce.
//!
//! The second case is the same rule driven entirely over the wire, through the
//! clock's own `CREATING -> ACTIVE` and `UPDATING -> ACTIVE` windows.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::json;
use service_com_amazonaws_dsql_dsql::ops::types::CLUSTER_STATUS_VALUES;
use service_com_amazonaws_dsql_dsql::state::{
    new_vpc_endpoint_service_suffix, Cluster, ClusterPolicy, ComAmazonawsDsqlDsqlState,
    PendingTransition, STATUS_ACTIVE, STATUS_DELETED, STATUS_DELETING, STATUS_IDLE,
    STATUS_PENDING_DELETE, STATUS_UPDATING,
};

use crate::cluster_lifecycle::cluster_id_for;
use crate::cluster_policy::allow;
use crate::harness::{expected_arn, policy_document, wait_for_the_clock, Answer, Dsql};
use crate::stream_lifecycle::kinesis_target;

/// The region every case in this suite signs for.
const REGION: &str = "us-east-1";

/// The `PolicyVersion` the seeded cluster's attached policy carries: a token of the
/// form the service mints one in (`state::new_policy_version`), which a refused write
/// must leave attached as it is.
const SEEDED_POLICY_VERSION: &str = "4f6b8c2e-0a1d-4e7f-9b35-2c6d8e1f4a70";

/// Far enough ahead that no case in this suite reaches it: a seeded cluster in a
/// transient status carries the transition the clock owes it, and the gate has to
/// refuse while that transition is still outstanding.
const NOT_DUE_YET: Duration = Duration::from_secs(3600);

/// An operation that MUTATES a cluster record, as a client calls it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mutation {
    UpdateCluster,
    DeleteCluster,
    CreateStream,
    PutClusterPolicy,
    DeleteClusterPolicy,
}

impl Mutation {
    /// Every operation of this crate that writes a cluster's record and models
    /// `ConflictException`. `TagResource` and `UntagResource` are not here: they
    /// model no `ConflictException` at all (`smithy_query(query="operation",
    /// operation="TagResource,UntagResource")`), so the gate's refusal is not an
    /// answer they can give.
    const ALL: [Mutation; 5] = [
        Mutation::UpdateCluster,
        Mutation::DeleteCluster,
        Mutation::CreateStream,
        Mutation::PutClusterPolicy,
        Mutation::DeleteClusterPolicy,
    ];

    /// Drive this operation against `identifier` with a request that is valid in
    /// every other respect, so the only thing a refusal can be about is the
    /// cluster's status.
    async fn call(self, dsql: &Dsql, identifier: &str) -> Answer {
        match self {
            Mutation::UpdateCluster => {
                dsql.update(identifier, json!({"deletionProtectionEnabled": true}))
                    .await
            }
            Mutation::DeleteCluster => dsql.delete(identifier).await,
            Mutation::CreateStream => {
                dsql.create_stream(
                    identifier,
                    json!({
                        "format": "JSON",
                        "ordering": "UNORDERED",
                        "targetDefinition": kinesis_target(dsql.region(), "changes", "dsql-cdc"),
                    }),
                )
                .await
            }
            Mutation::PutClusterPolicy => {
                dsql.put_policy(identifier, json!({"policy": allow("dsql:DbConnect")}))
                    .await
            }
            Mutation::DeleteClusterPolicy => dsql.delete_policy(identifier, "").await,
        }
    }
}

/// A stored cluster in `status`, with a policy already attached so a policy write
/// is a write the gate is the only thing standing in front of, and with the
/// transition the clock owes a transient status still outstanding.
fn stored_cluster(identifier: &str, status: &str) -> Cluster {
    let settles_to = match status {
        // The legs of the deletion chain are owed the next leg of that chain, not
        // a return to service.
        STATUS_PENDING_DELETE => Some(STATUS_DELETING),
        STATUS_DELETING => Some(STATUS_DELETED),
        status if status == STATUS_ACTIVE || status == STATUS_IDLE => None,
        _ => Some(STATUS_ACTIVE),
    };
    Cluster {
        identifier: identifier.to_string(),
        vpc_endpoint_service_suffix: new_vpc_endpoint_service_suffix(),
        arn: expected_arn(REGION, identifier),
        status: status.to_string(),
        creation_time: SystemTime::now(),
        deletion_protection_enabled: false,
        kms_encryption_key: None,
        encryption_details: None,
        multi_region_properties: None,
        tags: Default::default(),
        metric_groups: Default::default(),
        policy: Some(ClusterPolicy {
            document: allow("dsql:DbConnect"),
            version: SEEDED_POLICY_VERSION.to_string(),
        }),
        streams: Default::default(),
        pending: settles_to.map(|next_status| PendingTransition {
            due_at: SystemTime::now() + NOT_DUE_YET,
            next_status,
        }),
        // Seeded with its encryption configuration already in place, so the
        // clock owes it nothing on that member.
        encryption_pending: None,
    }
}

/// Whether `status` is one a mutation is accepted in: the rule's own condition,
/// spelled here so the table states the expectation rather than reading it off the
/// code under test.
fn accepts_a_mutation(status: &str) -> bool {
    status == STATUS_ACTIVE || status == STATUS_IDLE
}

/// RULE: a write of a cluster's record is refused with 409 `ConflictException`
/// while the cluster's status does not accept a mutation, whichever operation
/// carries the write — and the refused write changes nothing about the record.
#[tokio::test]
async fn every_cluster_write_passes_the_same_status_gate() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let dsql = Dsql::sharing(&engine, REGION);

    let mut case_number = 0usize;
    for status in CLUSTER_STATUS_VALUES {
        for mutation in Mutation::ALL {
            case_number += 1;
            let identifier = cluster_id_for(case_number);
            let case = format!("{mutation:?} on a {status} cluster");
            engine
                .state::<ComAmazonawsDsqlDsqlState>()
                .lock()
                .clusters
                .insert(identifier.clone(), stored_cluster(&identifier, status));

            let answer = mutation.call(&dsql, &identifier).await;

            // The one exemption: a delete of a cluster whose deletion is already
            // under way answers with that delete rather than refusing it, in
            // either leg of the chain the deletion reports — the peering
            // tear-down of a multi-Region cluster and the removal itself.
            let delete_already_in_flight = [STATUS_PENDING_DELETE, STATUS_DELETING]
                .contains(status)
                && mutation == Mutation::DeleteCluster;
            if accepts_a_mutation(status) || delete_already_in_flight {
                assert_eq!(
                    answer.status, 200,
                    "case {case:?}: the write must be accepted, got {} {:?}",
                    answer.status, answer.body
                );
                continue;
            }

            assert_eq!(
                answer.status, 409,
                "case {case:?}: the write must be refused with 409, got {} {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error.as_deref(),
                Some("ConflictException"),
                "case {case:?}: the refusal must be a ConflictException, got {:?}",
                answer.error
            );
            assert!(
                answer.message().contains(status) && answer.message().contains(&identifier),
                "case {case:?}: the refusal must name the cluster and the status \
                 that refused it, got {:?}",
                answer.message()
            );

            // Nothing about the record moved: not its status, not the policy it
            // carries, not the version counter a policy write would bump, and no
            // stream was minted on it.
            let state = engine.state::<ComAmazonawsDsqlDsqlState>();
            let state = state.lock();
            let stored = state
                .clusters
                .get(&identifier)
                .unwrap_or_else(|| panic!("case {case:?}: the cluster must still be stored"));
            assert_eq!(
                stored.status, *status,
                "case {case:?}: a refused write must leave the status alone, got {:?}",
                stored.status
            );
            assert_eq!(
                stored.policy.as_ref().map(|policy| policy.version.as_str()),
                Some(SEEDED_POLICY_VERSION),
                "case {case:?}: a refused write must leave the attached policy alone, got {:?}",
                stored.policy
            );
            assert!(
                stored.streams.is_empty(),
                "case {case:?}: a refused write must mint no stream, got {:?}",
                stored.streams.keys().collect::<Vec<_>>()
            );
        }
    }
}

/// RULE (over the wire, through the clock's own windows): the policy operations
/// answer the status gate exactly as their siblings do — refused while the clock
/// still owes the cluster a transition, accepted once it is settled — and the
/// policy a refused call carried is never attached.
#[tokio::test]
async fn the_policy_operations_answer_the_gate_the_clock_opens() {
    let dsql = Dsql::new();
    let created = dsql.create(json!({"deletionProtectionEnabled": false})).await;
    let identifier = created.str("identifier");
    assert_eq!(
        created.str("status"),
        "CREATING",
        "CreateCluster must answer CREATING, got {:?}",
        created.body
    );

    // Inside the CREATING window: both policy writes are refused, exactly as
    // UpdateCluster is.
    for mutation in [
        Mutation::PutClusterPolicy,
        Mutation::DeleteClusterPolicy,
        Mutation::UpdateCluster,
    ] {
        let answer = mutation.call(&dsql, &identifier).await;
        assert_eq!(
            (answer.status, answer.error.as_deref()),
            (409, Some("ConflictException")),
            "case {mutation:?} on a CREATING cluster: must be refused with 409 \
             ConflictException, got {} {:?}",
            answer.status,
            answer.body
        );
    }
    assert_eq!(
        dsql.get_policy(&identifier).await.status,
        404,
        "case PutClusterPolicy on a CREATING cluster: the refused document must not be attached"
    );

    wait_for_the_clock().await;
    let attached = dsql
        .put_policy(&identifier, json!({"policy": allow("dsql:DbConnect")}))
        .await;
    assert_eq!(
        attached.status, 200,
        "case PutClusterPolicy on an ACTIVE cluster: must be accepted, got {} {:?}",
        attached.status, attached.body
    );
    let read = dsql.get_policy(&identifier).await;
    assert_eq!(
        (read.status, policy_document(&read.str("policy"))),
        (200, policy_document(&allow("dsql:DbConnect"))),
        "the attached document must read back, got {} {:?}",
        read.status,
        read.body
    );
    let first_version = attached.str("policyVersion");

    // UpdateCluster puts the cluster into UPDATING, which is another transition the
    // clock owes it — so the same two policy writes are refused again.
    let updating = dsql
        .update(&identifier, json!({"deletionProtectionEnabled": false}))
        .await;
    assert_eq!(
        (updating.status, updating.str("status")),
        (200, STATUS_UPDATING.to_string()),
        "UpdateCluster must answer UPDATING, got {} {:?}",
        updating.status,
        updating.body
    );
    for mutation in [Mutation::PutClusterPolicy, Mutation::DeleteClusterPolicy] {
        let answer = mutation.call(&dsql, &identifier).await;
        assert_eq!(
            (answer.status, answer.error.as_deref()),
            (409, Some("ConflictException")),
            "case {mutation:?} on an UPDATING cluster: must be refused with 409 \
             ConflictException, got {} {:?}",
            answer.status,
            answer.body
        );
    }
    let read = dsql.get_policy(&identifier).await;
    assert_eq!(
        (read.status, read.str("policyVersion")),
        (200, first_version.clone()),
        "a refused policy write must leave the attached version alone, got {} {:?}",
        read.status,
        read.body
    );

    // Settled again: the removal is accepted, and it removes the version the
    // accepted write issued.
    wait_for_the_clock().await;
    let removed = dsql.delete_policy(&identifier, "").await;
    assert_eq!(
        (removed.status, removed.str("policyVersion")),
        (200, first_version),
        "case DeleteClusterPolicy on a settled cluster: must remove the attached version, \
         got {} {:?}",
        removed.status,
        removed.body
    );

    // On its way out: a cluster that is deleting takes no policy either.
    let deleting = dsql.delete(&identifier).await;
    assert_eq!(
        (deleting.status, deleting.str("status")),
        (200, STATUS_DELETING.to_string()),
        "DeleteCluster must answer DELETING, got {} {:?}",
        deleting.status,
        deleting.body
    );
    for mutation in [Mutation::PutClusterPolicy, Mutation::DeleteClusterPolicy] {
        let answer = mutation.call(&dsql, &identifier).await;
        assert_eq!(
            (answer.status, answer.error.as_deref()),
            (409, Some("ConflictException")),
            "case {mutation:?} on a DELETING cluster: must be refused with 409 \
             ConflictException, got {} {:?}",
            answer.status,
            answer.body
        );
    }
}
