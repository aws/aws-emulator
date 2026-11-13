// SPDX-License-Identifier: CC0-1.0

//! The Elastic Load Balancing v2 TARGETS an Auto Scaling group's instances occupy in the
//! target groups it is attached to — the registration that makes a load-balanced group
//! serve traffic, and the drain that takes an instance leaving the group out of rotation.
//!
//! `CreateAutoScalingGroup$TargetGroupARNs` is "the Amazon Resource Names (ARN) of the
//! target groups to associate with the Auto Scaling group. Instances are registered as
//! targets in a target group, and traffic is routed to the target group", and the same
//! association is what `AttachLoadBalancerTargetGroups` adds and
//! `DetachLoadBalancerTargetGroups` removes ("when you detach a target group, it enters the
//! Removing state while deregistering the instances in the group"). So an instance of an
//! attached group is a target of that target group, at the target group's own port, and an
//! instance on its way out of the group leaves it — through `draining`, for the group's
//! deregistration delay, exactly as `DeregisterTargets` takes a target out (ELBv2
//! `state::target_health`).
//!
//! The registrations live in the Elastic Load Balancing v2 service's OWN store, written
//! through its own seam, so a target this service registers is one that service returns
//! from its own read (`DescribeTargetHealth`), counts against its own target quota and
//! health-checks exactly as it checks a `RegisterTargets` caller's own instance target.
//! Nothing about the instance is declared here beyond where it is: an instance id names an
//! Amazon EC2 instance whose listening ports this emulator does not know, and the security
//! groups governing it are the ones its own network interface reports.
//!
//! # Reconciled, never appended
//!
//! The registrations this service owns in a target group are exactly the ones a group's
//! live instances need, so the set is RECOMPUTED from this store ([`wanted_of`]) and the
//! target groups are brought in line with it ([`reconcile`]): an instance that launched is
//! registered, an instance that is terminating or gone is deregistered, an attachment that
//! was detached takes its registrations with it, and a registration a `RegisterTargets`
//! CALLER made is never touched (it carries no `RegisteredTarget::registered_for`, which is
//! what tells the two apart).
//!
//! Only a target group whose `TargetType` is `instance` receives anything: an Auto Scaling
//! group's instances are Amazon EC2 instances, and an `ip`, `lambda` or `alb` target group
//! takes a different kind of target altogether, so none is invented for it.
//!
//! # One group at a time
//!
//! A reconcile is scoped to ONE Auto Scaling group ([`settle_group`]): a write to a group
//! owes the registrations of that group and of no other, and a pass claims only the
//! registrations whose workload names it ([`Scope::claims`]), so one group's reconcile
//! cannot touch another's. The clock pump, whose wake-up can move an instance of any group,
//! walks the groups one at a time ([`settle_every_group`]) for the same reason — so the
//! registrations ONE group's instances need is the most that is ever computed at once,
//! bounded by that group's own attachments ([`crate::state::MAX_TARGET_GROUPS_PER_GROUP`])
//! times its own instances ([`crate::state::MAX_GROUP_CAPACITY`]).
//!
//! # An attachment whose target group is gone
//!
//! Registering the instance is PART of bringing it into service, so an attachment whose
//! target group Elastic Load Balancing v2 no longer holds ([`unresolved_attachments`])
//! stops the launch that would have to register there: the group launches nothing and
//! records the refusal that names the target group instead
//! ([`launch_blocked_message`]), which is what a client reads off the group's scaling
//! activities. The attachment then ends with the target group it named
//! (`AutoScalingState::drop_attachments`), because a target group ARN carries the id that
//! service minted for that one target group and so can never be registered into again.
//!
//! LOCKING. A reconcile reads the wanted set under this service's own lock, RELEASES it,
//! and only then takes the Elastic Load Balancing v2 store's lock for the one pass that
//! registers and deregisters; nothing awaits while either is held. A read of that store
//! ([`unresolved_attachments`]) takes its lock for the read alone and locks nothing of
//! this service inside it, so the order is the same one. Elastic Load Balancing v2
//! never reaches this service, so the two locks are never taken in the other order and no
//! cycle exists.

use std::collections::BTreeMap;
use std::time::SystemTime;

use ::service_com_amazonaws_elasticloadbalancingv2_elasticloadbalancing_v10::state as elbv2;

use crate::state::{AutoScalingState, StoredGroup, LIFECYCLE_TERMINATING};

/// The `TargetTypeEnum` value of a target group whose targets are Amazon EC2 instances,
/// which is the only kind an Auto Scaling group's instances can be registered in
/// (`TargetGroup$TargetType`: "`instance` - Targets are specified by instance ID").
const TARGET_TYPE_INSTANCE: &str = "instance";

/// The `TrafficSourceIdentifier$Type` of a traffic source whose `Identifier` is a target
/// group ARN of Elastic Load Balancing v2 — "`elbv2` if `Identifier` is the ARN of an
/// Application Load Balancer, Gateway Load Balancer, or Network Load Balancer target group".
const TRAFFIC_SOURCE_TYPE_ELBV2: &str = "elbv2";

/// One registration an Auto Scaling group owes in one Elastic Load Balancing v2 target
/// group.
///
/// Every field is read off a stored group and one of its stored instances, so a registration
/// stands for an instance a `DescribeAutoScalingGroups` of the same instant reports — and an
/// Amazon EC2 instance of the account — and never a second, invented copy of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetRegistration {
    /// The target group the instance is registered in.
    pub target_group_arn: String,
    /// The instance's own id, which is what an `instance` target group's targets are
    /// identified by.
    pub instance_id: String,
    /// The Availability Zone the instance runs in, as the group holds it.
    pub availability_zone: String,
    /// The workload this registration stands for, which is what lets this service take its
    /// own registrations out again and leave every other registration alone
    /// ([`workload`]).
    pub workload: String,
}

/// The workload identifier a registration of `instance_id` for the group `group_arn`
/// carries: the group the instance belongs to and the instance itself.
///
/// Built from the group's ARN, so it names a resource of THIS service and two groups never
/// claim one another's registrations ([`Scope::claims`]).
pub fn workload(group_arn: &str, instance_id: &str) -> String {
    format!("{group_arn}/{instance_id}")
}

/// The group ARN a workload [`workload`] built names: everything before the instance id it
/// ends with.
///
/// Read from the RIGHT, because a group name may itself contain a `/`
/// (`AutoScalingGroupName` admits every ASCII character 33 to 126 but the colon), so only
/// the last separator is the one this form added.
fn group_arn_of(workload: &str) -> Option<&str> {
    workload.rsplit_once('/').map(|(group_arn, _)| group_arn)
}

/// Which registrations one reconcile may add and remove.
///
/// A registration another emulated service's scheduler made through the same seam, and one a
/// `RegisterTargets` caller made, are claimed by neither variant: both are left exactly as
/// they are.
#[derive(Debug, Clone)]
enum Scope {
    /// The registrations of ONE Auto Scaling group, named by the suffix its ARN ends with
    /// ([`crate::state::group_arn`]) — what a write to that group owes, and nothing else.
    /// It claims the group's registrations whether or not the group still exists, so a
    /// deleted group's targets drain.
    OneGroup { arn_suffix: String },
}

impl Scope {
    /// The scope of a reconcile of the group `group_name`.
    fn of_group(group_name: &str) -> Self {
        Self::OneGroup {
            arn_suffix: format!(":autoScalingGroupName/{group_name}"),
        }
    }

    /// Whether a registration standing for `workload` is one this reconcile may act on.
    fn claims(&self, workload: &str) -> bool {
        match self {
            Self::OneGroup { arn_suffix } => {
                group_arn_of(workload).is_some_and(|group_arn| group_arn.ends_with(arn_suffix))
            }
        }
    }
}

/// Every Elastic Load Balancing v2 target group the group `group` is ATTACHED to, in
/// attachment order and once each however many times it is named.
///
/// A target group is counted from both forms the attachment takes — the group's own
/// `TargetGroupARNs` and an `elbv2` traffic source — because either one associates that
/// target group with the group.
pub fn attached_arns(group: &StoredGroup) -> Vec<&str> {
    let mut arns: Vec<&str> = Vec::new();
    let attached = group.attached_target_groups().iter().map(String::as_str);
    let from_traffic_sources = group
        .traffic_sources
        .iter()
        .flatten()
        .filter(|source| {
            source
                .r#type
                .as_deref()
                .is_some_and(|kind| kind == TRAFFIC_SOURCE_TYPE_ELBV2)
        })
        .map(|source| source.identifier.as_str());
    for arn in attached.chain(from_traffic_sources) {
        if !arns.contains(&arn) {
            arns.push(arn);
        }
    }
    arns
}

/// The target groups `group` is attached to that Elastic Load Balancing v2 no longer
/// holds, in attachment order: the attachments that can take no registration at all,
/// because the target group they name has been deleted in that service since it was
/// attached.
///
/// A READ of the other service's store, under that store's lock alone — this module's
/// header states the order, and nothing of this service is locked inside it.
///
/// Answers none when that store is unreachable (the engine is not the shared in-memory
/// one): there is then no target group this service could have registered into, so no
/// attachment can be said to have lost one.
pub fn unresolved_attachments<Eng: Send + Sync + 'static>(
    engine: &Eng,
    group: &StoredGroup,
) -> Vec<String> {
    let arns = attached_arns(group);
    if arns.is_empty() {
        return Vec::new();
    }
    let Some(state) = elbv2::service_state(engine) else {
        return Vec::new();
    };
    let store = state.lock();
    arns.into_iter()
        .filter(|arn| !store.target_groups.contains_key(*arn))
        .map(str::to_string)
        .collect()
}

/// How Elastic Load Balancing v2 names ITSELF in the refusal a caller of it reports —
/// the service's own `cloudFormationName`
/// (`smithy_query(query="service_info")` of that service's model), which is the name an
/// AWS SDK renders in the exception it raises for a failed call.
const ELBV2_SERVICE_NAME: &str = "ElasticLoadBalancingV2";

/// The wire error code and HTTP status Elastic Load Balancing v2 answers a call naming a
/// target group it does not hold with: its `TargetGroupNotFoundException`'s own
/// `awsQueryError` code and `httpError`
/// (`smithy_query(query="shape",shape="TargetGroupNotFoundException")`).
const TARGET_GROUP_NOT_FOUND_CODE: &str = "TargetGroupNotFound";
const TARGET_GROUP_NOT_FOUND_STATUS: u16 = 400;

/// What the scaling activity of a launch that could not be registered reports as its
/// `StatusMessage`: the refusal Elastic Load Balancing v2 answered for the target groups
/// that are gone, and the launch that therefore did not happen.
///
/// The refusal is that service's own — its `TargetGroupNotFound` wording for the
/// identifiers a call names (`elbv2::target_groups_not_found_message`), with the service,
/// status and code an AWS SDK renders beside the message of a failed call, so a client
/// reads here exactly what it would read from a call of its own against the deleted target
/// group. The sentence that ends it is Amazon EC2 Auto Scaling's own: the launch it did not
/// make.
pub fn launch_blocked_message(unresolved: &[String]) -> String {
    format!(
        "{} (Service: {ELBV2_SERVICE_NAME}, Status Code: {TARGET_GROUP_NOT_FOUND_STATUS}, Error \
         Code: {TARGET_GROUP_NOT_FOUND_CODE}). Launching EC2 instance failed.",
        elbv2::target_groups_not_found_message(unresolved.iter().map(String::as_str))
    )
}

/// Every registration the group `group_name` owes right now, in attachment order and, within
/// an attachment, in instance order. Empty for a name this store holds no group for, which is
/// what a deleted group owes.
///
/// An instance counts while it is not on its way out of the group: an instance that is still
/// `Pending` has launched and is being brought into service, which is what the registration
/// is part of, and one that is `Terminating` is leaving. A target group is counted from both
/// forms the attachment takes — the group's own `TargetGroupARNs` and an `elbv2` traffic
/// source — because either one associates that target group with the group, and each target
/// group is counted once however many times it is named.
pub fn wanted_of(store: &AutoScalingState, group_name: &str) -> Vec<TargetRegistration> {
    let Some(group) = store.groups.get(group_name) else {
        return Vec::new();
    };
    let mut registrations = Vec::new();
    for arn in attached_arns(group) {
        for instance in &group.instances {
            if instance.lifecycle_state == LIFECYCLE_TERMINATING {
                continue;
            }
            registrations.push(TargetRegistration {
                target_group_arn: arn.to_string(),
                instance_id: instance.instance_id.clone(),
                availability_zone: instance.availability_zone.clone(),
                workload: workload(&group.arn, &instance.instance_id),
            });
        }
    }
    registrations
}

/// Bring the Elastic Load Balancing v2 target groups in line with what the group
/// `group_name` now holds: the ONE call a write that moved that group's instances or its
/// attachments makes for it.
///
/// Called with NO guard of this store held: it takes this store's lock to read the set that
/// group needs ([`wanted_of`]), releases it, and only then writes the other service's store
/// ([`reconcile`]) — the lock order this module's header states.
///
/// Reconciling is idempotent, so a call that moved nothing writes nothing: the set it reads
/// is the set already in place.
pub fn settle_group<Eng: Send + Sync + 'static>(engine: &Eng, group_name: &str) {
    let Some(state) = crate::state::service_state(engine) else {
        return;
    };
    // The guard this statement takes is a TEMPORARY: it is released at the end of the
    // statement, so nothing below holds it.
    let wanted = wanted_of(&state.lock(), group_name);
    reconcile(engine, group_name, &wanted, SystemTime::now());
}

/// Settle every group this store holds, ONE GROUP AT A TIME: what the clock pump owes, whose
/// wake-up can have moved an instance of any group.
///
/// The group names are read under this store's lock and the guard released before the first
/// reconcile, and each group is then settled on its own — so the registrations of one group
/// is the most this pass ever computes at once, however many groups the store holds (itself
/// bounded by [`crate::state::MAX_GROUPS`]).
pub fn settle_every_group<Eng: Send + Sync + 'static>(engine: &Eng) {
    let Some(state) = crate::state::service_state(engine) else {
        return;
    };
    let names: Vec<String> = state.lock().groups.keys().cloned().collect();
    for name in names {
        settle_group(engine, &name);
    }
}

/// Make the Elastic Load Balancing v2 registrations of the group `group_name` exactly
/// `wanted`, in that service's own store through the one shared engine.
///
/// A registration of that group that `wanted` no longer holds is DEREGISTERED the way
/// `DeregisterTargets` deregisters one: it enters `draining` and leaves the target group once
/// that group's `deregistration_delay.timeout_seconds` has passed, which is the contract a
/// client that drains connections before an instance goes away waits on. A registration of
/// any other group, of another emulated service's scheduler, or of a `RegisterTargets`
/// caller is left exactly as it is ([`Scope::claims`]).
///
/// Nothing happens when the Elastic Load Balancing v2 store is unreachable (the engine is
/// not the shared in-memory one): there is then no target group to register into and none
/// this service could have registered into, so there is nothing to add and nothing to clean
/// up — which is why this answers nothing rather than failing the request that called it.
pub fn reconcile<Eng: Send + Sync + 'static>(
    engine: &Eng,
    group_name: &str,
    wanted: &[TargetRegistration],
    now: SystemTime,
) {
    let Some(state) = elbv2::service_state(engine) else {
        return;
    };
    let scope = Scope::of_group(group_name);
    // The wanted set INDEXED by the target group it belongs to, once, so the pass below
    // costs each target group only what that target group itself holds rather than a walk
    // over every registration of the group.
    let mut by_target_group: BTreeMap<&str, Vec<&TargetRegistration>> = BTreeMap::new();
    for registration in wanted {
        by_target_group
            .entry(registration.target_group_arn.as_str())
            .or_default()
            .push(registration);
    }
    let mut store = state.lock();
    // The registrations this reconcile ADDED, each as the key the target group holds it
    // under: the write that creates a registration owes it its first health check, and
    // nothing else is checked for it.
    let mut added: Vec<(String, elbv2::TargetKey)> = Vec::new();
    for (arn, group) in store.target_groups.iter_mut() {
        // What the clock already owes this group: a registration whose drain is over has
        // left it, so it occupies neither a key nor the group's quota.
        group.settle_draining(now);
        // The registrations wanted in THIS target group, each under the key the group would
        // store it at — an instance target takes the target group's own port, which is where
        // "the load balancer routes requests to registered targets using the protocol and
        // port for the target group".
        let keys: Vec<(elbv2::TargetKey, &TargetRegistration)> = by_target_group
            .get(arn.as_str())
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .map(|registration| {
                (
                    (registration.instance_id.clone(), group.port),
                    *registration,
                )
            })
            .collect();
        // Every registration of THIS group that the set above no longer holds.
        for (key, workload) in group.scheduler_registrations() {
            if !scope.claims(&workload) {
                continue;
            }
            if !keys.iter().any(|(held, _)| *held == key) {
                group.deregister(&key, now);
            }
        }
        // An Amazon EC2 instance is a target of an `instance` group alone.
        if group.target_type != TARGET_TYPE_INSTANCE {
            continue;
        }
        for (key, registration) in keys {
            // Whether the group ALREADY holds this registration decides how to read the
            // answer below: a key that is already there is the steady state, while a key
            // that is not and still is not afterwards is a registration the group had no
            // room for.
            let held = group.targets.contains_key(&key);
            let registered = group.register_for(
                &elbv2::SchedulerRegistration {
                    id: registration.instance_id.clone(),
                    // The target group's own port, which `register_for` fills in.
                    port: None,
                    availability_zone: Some(registration.availability_zone.clone()),
                    workload: registration.workload.clone(),
                    // An instance id names no workload whose listening ports this emulator
                    // knows, and the security groups governing it are the ones its own
                    // network interface reports — the same two unknowns a `RegisterTargets`
                    // caller's instance target carries.
                    serves_ports: Vec::new().into(),
                    guarded_by: Vec::new().into(),
                },
                now,
            );
            // A group already holding its quota of targets takes no further registration, so
            // the instance is reachable through no target: said rather than left to be read
            // as an instance nothing routes to.
            if !held && !registered {
                ::tracing::warn!(
                    target_group = %arn,
                    instance = %registration.instance_id,
                    "an Auto Scaling instance could not be registered as a target: the target \
                     group holds its quota of targets"
                );
            }
            if registered {
                added.push((arn.clone(), key));
            }
        }
    }
    // The registrations this reconcile made are CHECKED at once, the way the Elastic Load
    // Balancing v2 write that registers a caller's own targets checks them: the state a
    // registration is reported in must follow those checks from the moment it is registered
    // rather than wait for a clock wake-up. Only the registrations this call added, so a
    // reconcile that moved nothing checks nothing; with the guard this reconcile held
    // RELEASED, so the check takes the store's lock itself.
    drop(store);
    elbv2::check_registrations(engine, &added);
}
