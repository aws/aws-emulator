// SPDX-License-Identifier: CC0-1.0

//! The templates this service answers a `ReplicationGroupNotFoundFault` with BESIDE the
//! service-wide one, and which operations take them.
//!
//! ElastiCache words that fault PER OPERATION, and the wordings are observable apart on the
//! live service, so one service-wide template cannot serve them all:
//!
//! ```text
//! ReplicationGroup <id> not found.           <- the shape's spelling, a trailing stop
//! Replication group '<id>' not found         <- prose, the id quoted, no trailing stop
//! Replication group '<id>' not found.        <- the same, with a trailing stop
//! Replication group <id> does not exist.     <- prose, unquoted, a trailing stop
//! Specified replication group does not exist <- the delete's own, naming no id at all
//! ```
//!
//! The first is [`crate::state::replication_group_not_found_message`], which the describe
//! answers. [`quoted_message`] is the second, which the settings change
//! (`ModifyReplicationGroup`), the shard reconfiguration
//! (`ModifyReplicationGroupShardConfiguration`), the Global datastore create
//! (`CreateGlobalReplicationGroup`) and the migration CUTOVER (`CompleteMigration`) answer
//! for the group each resolves. [`quoted_sentence`] is the third, which the two operations
//! that OPEN or test an online migration (`StartMigration`, `TestMigration`) answer.
//! [`does_not_exist_message`] is the fourth, which the failover test answers, and
//! [`DELETE_MESSAGE`] the fifth, which `DeleteReplicationGroup` answers.
//!
//! Stated here once so the handlers answering one cannot drift apart about its text: the
//! message is the only place a client reads WHY a call was refused, and for four of the five
//! WHICH group could not be resolved.

/// The template `Replication group '<id>' not found` — the id single-quoted, and no
/// trailing stop.
///
/// The id is the one variable part and comes from the request, as the request spelled it,
/// because the message is built from the request and from nothing else.
pub fn quoted_message(replication_group_id: &str) -> String {
    format!("Replication group '{replication_group_id}' not found")
}

/// How the two operations that OPEN or test an online migration word a replication group
/// this account does not hold: `Replication group '<id>' not found.` — [`quoted_message`]
/// with a trailing stop.
///
/// `StartMigration` and `TestMigration` state the same sentence, so a client that tests the
/// connection and then starts the sync reads one wording for the group it names; the cutover
/// that finishes the migration words it without the stop ([`quoted_message`]). The id is the
/// one variable part and comes from the request, as the request spelled it.
pub fn quoted_sentence(replication_group_id: &str) -> String {
    format!("{}.", quoted_message(replication_group_id))
}

/// How `TestFailover` words a replication group this account does not hold:
/// `Replication group <id> does not exist.`
///
/// Prose, the id unquoted, and a trailing stop. The id is the one variable part and comes
/// from the request, as the request spelled it.
pub fn does_not_exist_message(replication_group_id: &str) -> String {
    format!("Replication group {replication_group_id} does not exist.")
}

/// How `DeleteReplicationGroup` words a replication group this account does not hold:
/// `Specified replication group does not exist`.
///
/// WHOLLY FIXED — the sentence has no variable part at all. The delete states the class of
/// resource that could not be resolved and nothing else, as
/// [`crate::state::REPLICATION_GROUP_ALREADY_EXISTS_MESSAGE`] does for the name a create
/// collides on, so interpolating the requested id would change the string for every caller.
pub const DELETE_MESSAGE: &str = "Specified replication group does not exist";
