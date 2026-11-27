// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the ONE sentence a change to a replication group whose
//! published status is not `available` is refused with.
//!
//! The rule under test: a request that CHANGES a replication group's
//! configuration, refused because the status the group has PUBLISHED is not
//! `available`, is answered HTTP 400 `InvalidReplicationGroupState` with the one
//! fixed sentence this service states for that gate — `Replication group must be
//! in available state to modify.` — whichever operation asked for the change and
//! whatever status the group actually carries. The sentence names neither the
//! group nor its status: it states the precondition the caller has to wait for,
//! so a client branching on the text of an `InvalidReplicationGroupState`
//! refusal reads the same statement from every operation that reconfigures a
//! group.
//!
//! The one refusal of that class that is NOT about a published status is a
//! shard reconfiguration that collided with another one admitted at the same
//! instant: while the winner's own call has not answered, the `modifying` status
//! it left has been handed to nobody, so the loser is told the reshard could not
//! be carried out and to try again (`Could not reshard replication group. Please
//! try again later.`). The second test below pins that boundary from both sides.
//!
//! The first table spans the operations that change a group's configuration —
//! the shard count in each direction, the replica count in each direction, and
//! the group's own settings — against each status that is not `available`, and
//! then shows the SAME request being accepted once the group reads `available`,
//! so a row proves the refusal came from the state gate rather than from
//! anything else about the request.
//!
//! Every assertion goes over the wire through the same router the production
//! emulator mounts. A row that needs a group in a status the clock would move
//! holds the record there through [`crate::harness::hold_replication_group_status`]
//! — the same store the handlers read — so no row races a transition span, and
//! the rows that need a settled group LOOK AGAIN until it reports settled.

use service_com_amazonaws_elasticache_amazonelasticachev9::state;

use crate::harness::{
    assert_refusal, hold_replication_group_status, params, poll_replication_group_available,
    Emulator, TEST_NODE_TYPE,
};

/// AWS's state sentence for a change to a replication group whose published
/// status is not `available`: wholly fixed text, naming neither the group nor the
/// status it is in.
const MODIFY_STATE_SENTENCE: &str = "Replication group must be in available state to modify.";

/// AWS's sentence for a shard reconfiguration that collided with another one
/// admitted at the same instant: wholly fixed text too.
const RESHARD_COLLISION_SENTENCE: &str =
    "Could not reshard replication group. Please try again later.";

/// The statuses a replication group of this service reads while work on it is
/// outstanding or while it is on its way out — every one of them a status a change
/// to the group is refused in. The spellings are the service's own
/// (`crate::state`), so a row never writes a status string of its own.
const NOT_AVAILABLE_STATUSES: &[&str] = &[
    state::STATUS_CREATING,
    state::STATUS_MODIFYING,
    state::STATUS_SNAPSHOTTING,
    state::STATUS_DELETING,
];

/// One change to a replication group: the case name, the operation that asks for
/// it, the pairs that make the request beside `ReplicationGroupId`, and the
/// statuses the change is refused in.
type Change = (
    &'static str,
    &'static str,
    &'static [(&'static str, &'static str)],
    &'static [&'static str],
);

/// Create one cluster mode enabled replication group with two shards of two
/// replicas each: a shape every change in the table applies to — its shard count
/// can go up or down, and its replica count can go up or down.
async fn create_group(emulator: &Emulator, case: &str, id: &str) {
    let created = emulator
        .call(
            "CreateReplicationGroup",
            &params(&[
                ("ReplicationGroupId", id),
                ("ReplicationGroupDescription", "a group a change is refused on"),
                ("Engine", state::DEFAULT_ENGINE),
                ("CacheNodeType", TEST_NODE_TYPE),
                ("NumNodeGroups", "2"),
                ("ReplicasPerNodeGroup", "2"),
            ]),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "case {case:?}: the group a row needs must be creatable, got {} {}",
        created.status, created.body
    );
}

/// Put the stored replication group `id` back at `available`, with nothing
/// outstanding on it.
fn release_group(emulator: &Emulator, id: &str) {
    emulator.with_state(|store| {
        let group = store
            .replication_groups
            .get_mut(id)
            .expect("the group a row created must be stored");
        group.status = state::STATUS_AVAILABLE.to_string();
        group.pending_status = None;
        for shard in &mut group.node_groups {
            shard.status = state::STATUS_AVAILABLE.to_string();
        }
    });
}

#[tokio::test]
async fn every_change_to_a_group_that_is_not_available_states_the_one_modification_sentence() {
    // The axis: WHICH change is asked for (the shard count up and down, the
    // replica count up and down, the group's own settings) against WHICH status
    // the group is held in. `ModifyReplicationGroup` is the one change accepted
    // while the group is already `modifying` — "the modifications in this request
    // and any pending modifications" — so its row leaves that status out.
    let rows: &[Change] = &[
        (
            "a scale-out of the shard count",
            "ModifyReplicationGroupShardConfiguration",
            &[("NodeGroupCount", "3"), ("ApplyImmediately", "true")],
            NOT_AVAILABLE_STATUSES,
        ),
        (
            "a scale-in of the shard count",
            "ModifyReplicationGroupShardConfiguration",
            &[
                ("NodeGroupCount", "1"),
                ("ApplyImmediately", "true"),
                ("NodeGroupsToRetain.NodeGroupToRetain.1", "0001"),
            ],
            NOT_AVAILABLE_STATUSES,
        ),
        (
            "an increase of the replica count",
            "IncreaseReplicaCount",
            &[("NewReplicaCount", "3"), ("ApplyImmediately", "true")],
            NOT_AVAILABLE_STATUSES,
        ),
        (
            "a decrease of the replica count",
            "DecreaseReplicaCount",
            &[("NewReplicaCount", "1"), ("ApplyImmediately", "true")],
            NOT_AVAILABLE_STATUSES,
        ),
        (
            "a change to the group's own settings",
            "ModifyReplicationGroup",
            &[
                ("ReplicationGroupDescription", "the description this change writes"),
                ("ApplyImmediately", "true"),
            ],
            &[
                state::STATUS_CREATING,
                state::STATUS_SNAPSHOTTING,
                state::STATUS_DELETING,
            ],
        ),
    ];

    let emulator = Emulator::new();
    for (index, (case, action, extra, statuses)) in rows.iter().enumerate() {
        let id = format!("change-gate-{index}");
        create_group(&emulator, case, &id).await;
        poll_replication_group_available(&emulator, case, &id).await;

        let mut request = params(&[("ReplicationGroupId", &id)]);
        request.extend(params(extra));

        for status in statuses.iter() {
            hold_replication_group_status(&emulator, &id, status);
            let refused = emulator.call(action, &request).await;
            assert_refusal(
                &format!("{case} on a group reading {status}"),
                &refused,
                400,
                "InvalidReplicationGroupState",
                MODIFY_STATE_SENTENCE,
            );
            // The sentence states the precondition and nothing of this run: a
            // client matching it byte for byte is handed neither the group it
            // named nor the status the group happens to carry.
            let message = refused.error_message().to_string();
            assert!(
                !message.contains(&id) && !message.contains(status),
                "case {case:?} on a group reading {status}: the sentence names neither the group \
                 nor its status, got {message:?}"
            );
        }

        // The very same request, on the very same group, once it reads
        // `available`: accepted. So every refusal above came from the state gate
        // rather than from anything else about the request.
        release_group(&emulator, &id);
        let accepted = emulator.call(action, &request).await;
        assert_eq!(
            accepted.status, 200,
            "case {case:?}: the same change must be accepted once the group reads available, got \
             {} {}",
            accepted.status, accepted.body
        );
    }
}

#[tokio::test]
async fn a_reshard_states_the_collision_sentence_only_while_a_reshard_call_is_unanswered() {
    // The boundary, driven from the store so neither side races the other: the
    // group is held `modifying` either way, and the only difference is whether a
    // reshard of it has been ADMITTED and not answered yet. One such call
    // outstanding is a collision — the status it left is published to nobody — and
    // none outstanding is the published-status gate.
    //
    // Each row: the case, how many reshard calls are outstanding on the group,
    // and the sentence the next reshard is refused with.
    type Row = (&'static str, usize, &'static str);
    let rows: &[Row] = &[
        ("no reshard call outstanding", 0, MODIFY_STATE_SENTENCE),
        (
            "one reshard call admitted and unanswered",
            1,
            RESHARD_COLLISION_SENTENCE,
        ),
        (
            "several reshard calls admitted and unanswered",
            3,
            RESHARD_COLLISION_SENTENCE,
        ),
    ];

    let emulator = Emulator::new();
    create_group(&emulator, "the collision boundary", "collision-group").await;
    poll_replication_group_available(&emulator, "the collision boundary", "collision-group").await;

    for (case, outstanding, sentence) in rows.iter() {
        hold_replication_group_status(&emulator, "collision-group", state::STATUS_MODIFYING);
        emulator.with_state(|store| {
            let group = store
                .replication_groups
                .get_mut("collision-group")
                .expect("the group this test created must be stored");
            group.reshard_admissions = *outstanding;
        });

        let refused = emulator
            .call(
                "ModifyReplicationGroupShardConfiguration",
                &params(&[
                    ("ReplicationGroupId", "collision-group"),
                    ("NodeGroupCount", "3"),
                    ("ApplyImmediately", "true"),
                ]),
            )
            .await;
        assert_refusal(
            case,
            &refused,
            400,
            "InvalidReplicationGroupState",
            sentence,
        );
    }
}
