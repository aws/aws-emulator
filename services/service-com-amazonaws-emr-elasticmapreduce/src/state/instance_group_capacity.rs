// SPDX-License-Identifier: CC0-1.0

//! How many Amazon EC2 instances ONE instance group may be asked for.
//!
//! One concern of this service's state, and the sibling of the ceiling an instance FLEET's
//! target capacities are held to ([`super::MAX_INSTANCES_PER_FLEET`]): a group states its
//! capacity as "the target number of instances for the instance group"
//! (`InstanceGroupConfig$InstanceCount`), the model puts no `@range` maximum on that member,
//! and every instance a group asks for becomes an Amazon EC2 instance Amazon EMR launches
//! and a record this store keeps for it — so without a bound one request would drive work
//! and resident state proportional to an integer the caller chose.
//!
//! The bound is taken on the configuration, through
//! [`super::validate_instance_group_config`], which every path that persists an
//! `InstanceGroupConfig` goes through — the groups a launch NAMES and the ones its uniform
//! `MasterInstanceType`/`SlaveInstanceType`/`InstanceCount` form IMPLIES
//! ([`super::implied_instance_group_configs`]) alike, since the two state the same capacity
//! in different members.

/// The greatest number of Amazon EC2 instances Amazon EMR provisions for ONE instance group.
///
/// CHOSEN, not modelled, for the same reason [`super::MAX_INSTANCES_PER_FLEET`] is and with
/// the same value: the model publishes no maximum for the member, so what IS the contract is
/// that a request past the ceiling is refused rather than accepted. It is deliberately far
/// above any group a client would really ask for and far below the values a sizing
/// calculation produces when it goes wrong, which is the class the refusal exists for.
pub const MAX_INSTANCES_PER_GROUP: i32 = 65_536;

/// Why the instance count `requested` cannot be accepted for one instance group, if it
/// cannot: it is more than Amazon EMR provisions for one group
/// ([`MAX_INSTANCES_PER_GROUP`]).
///
/// The sentence is this emulator's own, like the other bounds
/// [`super::validate_instance_group_config`] states: it names the count the request carried
/// and the ceiling it exceeded, so a caller reads what to change.
pub fn instance_group_count_violation(requested: i32) -> Option<String> {
    (requested > MAX_INSTANCES_PER_GROUP).then(|| {
        format!(
            "InstanceCount {requested} is above {MAX_INSTANCES_PER_GROUP}, which is the \
             maximum number of instances provisioned per instance group"
        )
    })
}
