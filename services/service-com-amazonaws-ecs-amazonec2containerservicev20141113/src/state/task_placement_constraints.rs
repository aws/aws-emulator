// SPDX-License-Identifier: CC0-1.0

//! The task placement CONSTRAINTS a placement is held to: which of a cluster's container
//! instances a task may be put on, and the hosts a task that could not be placed was
//! rejected on.
//!
//! # The rule this module implements
//!
//! `PlacementConstraintType`'s `distinctInstance`: "Use `distinctInstance` to ensure that
//! each task in a particular group is running on a different container instance." So a
//! placement made under that constraint puts at most ONE task of the group on any one
//! container instance: a host already running a task of the group is no candidate for
//! another, and the number of tasks such a placement can make is the number of candidate
//! hosts that are still free.
//!
//! Which tasks share the cap is the GROUP's question, not the call's — `RunTask$group`: "The
//! name of the task group to associate with the task. The default value is the family name
//! of the task definition (for example, `family:my-family-name`)" — so two placements of one
//! group share the cap and placements of different groups do not, and a task that has
//! stopped holds no host.
//!
//! A placement asking for more tasks than there are free candidates is not refused: Amazon
//! ECS places the ones it can and reports the rest, which for `RunTask` is `Failures` beside
//! the `Tasks` it placed — `Failure$arn` is "the Amazon Resource Name (ARN) of the failed
//! resource", which for a task the constraint left unplaced is the container instance it was
//! rejected on, and `Failure$reason` is "the reason for the failure"
//! ([`DISTINCT_INSTANCE_UNSATISFIED_REASON`]).
//!
//! Nothing of the cluster query language is needed to evaluate this constraint:
//! `PlacementConstraint$expression` is "a cluster query language expression to apply to the
//! constraint ... You can't specify an expression if the constraint type is
//! `distinctInstance`", so which host a task is on is the whole question. The other
//! constraint type, `memberOf`, is an expression over the candidates' attributes and is NOT
//! decided here.

use super::*;

/// The `PlacementConstraintType` member that caps a group at one task per container
/// instance (the enum's members are `distinctInstance` and `memberOf` —
/// [`PLACEMENT_CONSTRAINT_TYPE_VALUES`]).
pub const PLACEMENT_CONSTRAINT_DISTINCT_INSTANCE: &str = "distinctInstance";

/// What `Failure$reason` carries for a task a `distinctInstance` constraint left unplaced: a
/// FIXED sentence, with nothing of the request in it — the host it names is carried by
/// `Failure$arn`.
pub const DISTINCT_INSTANCE_UNSATISFIED_REASON: &str =
    "DistinctInstance placement constraint unsatisfied.";

/// Whether `constraints` hold a placement to one task per container instance.
///
/// A list carrying the `distinctInstance` type does, whatever else it carries: a constraint
/// list is a conjunction ("an array of placement constraint objects to use for the task"),
/// so one entry asking for distinct instances caps the placement.
pub fn requires_distinct_instances(constraints: &[PlacementConstraint]) -> bool {
    constraints.iter().any(|constraint| {
        constraint.r#type.as_deref() == Some(PLACEMENT_CONSTRAINT_DISTINCT_INSTANCE)
    })
}

/// Whether `constraints` cap a placement of `launch_type` at ONE task per container instance.
///
/// Two halves, both of them necessary. The constraint has to ask for it
/// ([`requires_distinct_instances`]), and the placement has to NAME the infrastructure the
/// cluster's own container instances are for there to be anything to be distinct about: "if
/// you're using the Fargate launch type, task placement constraints aren't supported"
/// (`PlacementConstraint`), so a Fargate placement is not capped
/// ([`needs_registered_capacity`]). A placement that says where it runs with a
/// `capacityProviderStrategy` instead is not capped here either, and that is this reading's
/// own limit rather than a statement about where it runs: the host it is given IS resolved
/// through the provider's class of capacity
/// ([`ComAmazonawsEcsAmazonec2containerservicev20141113State::placement_launch_type`], which
/// is what the scheduler's host choice is made with), so such a placement does take container
/// instances of the cluster — what the cap over them should be for one is the subject of
/// [`DistinctInstancePlacement`]'s own suite and is not decided here. A placement this
/// answers `false` for is made exactly as it would be with no constraint stated: every task
/// asked for is placed.
pub fn caps_distinct_instances(
    constraints: &[PlacementConstraint],
    launch_type: Option<&str>,
) -> bool {
    requires_distinct_instances(constraints) && needs_registered_capacity(launch_type)
}

/// What a placement under a `distinctInstance` constraint can do for one call: the hosts it
/// places a task on, one per task, and the candidate hosts a task it could NOT place was
/// rejected on.
#[derive(Debug, Clone, Default)]
pub struct DistinctInstancePlacement {
    /// The container instance each task of this placement goes on, in order: one task per
    /// entry, so a placement that could place fewer tasks than were asked for has fewer
    /// entries.
    pub hosts: Vec<String>,
    /// The candidate hosts of the cluster that could not take a further task of the group.
    /// Empty when every task asked for was placed — a placement that made them all was
    /// rejected nowhere.
    pub rejected: Vec<String>,
}

impl DistinctInstancePlacement {
    /// The `Failures` this placement reports beside the tasks it placed: one per host it was
    /// rejected on, each carrying that host's ARN and the constraint's fixed sentence.
    pub fn failures(&self) -> Vec<Failure> {
        self.rejected
            .iter()
            .map(|arn| Failure {
                arn: Some(arn.clone()),
                reason: Some(DISTINCT_INSTANCE_UNSATISFIED_REASON.to_string()),
                detail: None,
            })
            .collect()
    }
}

impl ComAmazonawsEcsAmazonec2containerservicev20141113State {
    /// The container instances of `cluster_name` a placement of `launch_type` reserving
    /// `needs` could use at all, in the order the scheduler chooses them.
    ///
    /// The same capacity and the same order [`Self::select_task_host`] picks from — an
    /// instance a placement may be made on at all
    /// ([`ContainerInstanceRecord::accepts_task_placement`]) that has the resources the task
    /// reserves left ([`Self::instance_admits_task`]), fewest tasks first with the
    /// instance's own ARN breaking the tie — so a constrained placement and an unconstrained
    /// one choose hosts the same way.
    fn placement_candidates(
        &self,
        cluster_name: &str,
        launch_type: Option<&str>,
        needs: TaskReservation,
    ) -> Vec<String> {
        if !needs_registered_capacity(launch_type) {
            return Vec::new();
        }
        let mut candidates: Vec<&ContainerInstanceRecord> = self
            .container_instances
            .values()
            .filter(|instance| {
                instance.cluster_name == cluster_name
                    && instance.accepts_task_placement()
                    && self.instance_admits_task(instance, needs)
            })
            .collect();
        candidates.sort_by_key(|instance| {
            let tally = self.container_instance_tally(&instance.arn);
            (tally.running + tally.pending, instance.arn.clone())
        });
        candidates
            .into_iter()
            .map(|instance| instance.arn.clone())
            .collect()
    }

    /// Whether `instance_arn` already runs a task of `group` that has not stopped, which is
    /// what makes it no candidate for another one under a `distinctInstance` constraint.
    ///
    /// A task that has reached `STOPPED` holds nothing ([`TaskRecord::is_active`]), so a
    /// group whose tasks have all stopped leaves every host free again.
    fn hosts_group_task(&self, instance_arn: &str, group: &str) -> bool {
        self.tasks.values().any(|task| {
            task.container_instance_arn.as_deref() == Some(instance_arn)
                && task.group == group
                && task.is_active()
        })
    }

    /// What a placement of up to `count` tasks of `group` into `cluster_name` can do under a
    /// `distinctInstance` constraint: the host of each task it places, and the candidate
    /// hosts it was rejected on when it could not place them all.
    ///
    /// Every candidate host is reported as rejected once a task goes unplaced, because by
    /// then each of them runs a task of the group — the ones that already did and the ones
    /// this placement just filled.
    ///
    /// A reporting-only question: nothing is written here, so two reads at one instant
    /// answer identically.
    pub fn distinct_instance_placement(
        &self,
        cluster_name: &str,
        group: &str,
        launch_type: Option<&str>,
        needs: TaskReservation,
        count: usize,
    ) -> DistinctInstancePlacement {
        let candidates = self.placement_candidates(cluster_name, launch_type, needs);
        let hosts: Vec<String> = candidates
            .iter()
            .filter(|arn| !self.hosts_group_task(arn, group))
            .take(count)
            .cloned()
            .collect();
        let rejected = if hosts.len() < count {
            candidates
        } else {
            Vec::new()
        };
        DistinctInstancePlacement { hosts, rejected }
    }

    /// The container instance the SCHEDULER places the next task of `group` on under a
    /// `distinctInstance` constraint, or `None` when every candidate already runs one.
    pub fn select_distinct_task_host(
        &self,
        cluster_name: &str,
        group: &str,
        launch_type: Option<&str>,
        needs: TaskReservation,
    ) -> Option<String> {
        self.distinct_instance_placement(cluster_name, group, launch_type, needs, 1)
            .hosts
            .into_iter()
            .next()
    }

    /// How many further tasks of `group` a placement under `constraints` may make into
    /// `cluster_name`, out of the `wanted` it would otherwise make.
    ///
    /// `wanted` unchanged for a placement the constraint does not cap
    /// ([`caps_distinct_instances`]), and the number of free candidate hosts for one it does.
    pub fn constrained_placement_room(
        &self,
        cluster_name: &str,
        group: &str,
        launch_type: Option<&str>,
        constraints: &[PlacementConstraint],
        needs: TaskReservation,
        wanted: usize,
    ) -> usize {
        if !caps_distinct_instances(constraints, launch_type) {
            return wanted;
        }
        self.distinct_instance_placement(cluster_name, group, launch_type, needs, wanted)
            .hosts
            .len()
    }
}
