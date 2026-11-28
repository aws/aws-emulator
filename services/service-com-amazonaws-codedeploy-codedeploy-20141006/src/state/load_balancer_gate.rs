// SPDX-License-Identifier: CC0-1.0

//! The LOAD BALANCER a deployment group routes its traffic through: the one place
//! a group's `loadBalancerInfo` is asked whether it wires the deployments that
//! group would run.
//!
//! # The rule this module implements
//!
//! `deploymentStyle` is "information about the type of deployment, in-place or
//! blue/green, that you want to run and whether to route deployment traffic behind
//! a load balancer"
//! (`smithy_query(query="doc",shape="CreateDeploymentGroupInput",member="deploymentStyle")`),
//! and its `deploymentOption` is the half that answers the second question
//! (`smithy_query(query="shape",shape="DeploymentOption")`:
//! WITHOUT_TRAFFIC_CONTROL | WITH_TRAFFIC_CONTROL). An Amazon ECS deployment moves
//! production traffic BETWEEN TASK SETS and has nothing else to move it with: the
//! route it rewrites is the one `loadBalancerInfo` names
//! (`targetGroupPairInfoList`'s `prodTrafficRoute` is "the path used by a load
//! balancer to route production traffic when an Amazon ECS deployment is
//! complete"), so a group on that compute platform whose style routes traffic
//! behind a load balancer and that names NO load balancer at all describes a
//! deployment that could never be performed.
//!
//! Such a group is therefore refused rather than stored —
//! `InvalidLoadBalancerInfoException` (HTTP 400), "an invalid load balancer name,
//! or no load balancer name, was specified" — by every operation that WRITES a
//! deployment group, so the groups this service holds all satisfy the condition
//! however they were written. The refusal is judged from the configuration the
//! group would HAVE (the compute platform of the application it belongs to, its
//! materialised deployment style, and the load balancer it would end up with),
//! never from which members one request happened to carry.
//!
//! A group that routes no traffic behind a load balancer, and one on any other
//! compute platform, is unaffected: the condition is about the deployment this
//! platform's traffic control performs, not about `loadBalancerInfo` being
//! required in general.
//!
//! # The second question this module answers
//!
//! A `loadBalancerInfo` that DOES name something names it by name, and a name
//! outside Elastic Load Balancing's grammar for one can never name a load
//! balancer resource. So the names a group would be stored with are read at the
//! call that gives them — their EXISTENCE is not resolved, the group may name
//! target groups the account creates afterwards — and a value that is no name is
//! refused with the same `InvalidLoadBalancerInfoException`
//! ([`target_group_name_refusal`]).
//!
//! # The third question this module answers
//!
//! Each of the three members has a documented SIZE — up to 10 Classic Load
//! Balancers in `elbInfoList`, up to 10 target groups in `targetGroupInfoList`, and
//! "a maximum size of one" for `targetGroupPairInfoList`
//! (`smithy_query(query="shape",shape="LoadBalancerInfo")`) — so a configuration
//! that names more of one form than a deployment group may carry is refused with
//! that member's one error as well ([`load_balancer_list_size_refusal`]). The bounds
//! are documented; that refusal's WORDING is not recorded anywhere this emulator
//! could read, so it states the rule in this service's own register and is the one
//! place to correct if a recording of it appears.

use ::service_com_amazonaws_elasticloadbalancingv2_elasticloadbalancing_v10 as elbv2;

use crate::ops::types::{DeploymentStyle, LoadBalancerInfo};

use super::ecs_blue_green::COMPUTE_PLATFORM_ECS;
use super::DEPLOYMENT_OPTION_WITH_TRAFFIC_CONTROL;

/// `InvalidLoadBalancerInfoException` (HTTP 400): the sentence an Amazon ECS
/// deployment group that names no load balancer is refused with.
///
/// Entirely fixed text: what is missing is the member itself, so there is no name,
/// id or value of the request or of this account's state to stand in it.
pub const ECS_LOAD_BALANCER_INFO_REQUIRED: &str =
    "For ECS deployment group, loadBalancerInfo must be specified";

/// The refusal a deployment group of this configuration earns, or `None` when the
/// group describes a deployment that can be performed.
///
/// `compute_platform` is the platform of the application the group belongs to,
/// `deployment_style` the style the group would HAVE (materialised by
/// [`super::effective_deployment_style`], so the server-side default decides for a
/// request that named neither member), and `load_balancer_info` the load balancer
/// configuration it would end up with.
///
/// Call it from every operation that writes a deployment group, BEFORE anything is
/// stored and ahead of the gates that resolve the group's targets against the
/// services owning them: a group that names no load balancer is refused for the
/// load balancer it lacks, whatever those targets resolve to.
#[must_use]
pub fn load_balancer_refusal(
    compute_platform: &str,
    deployment_style: Option<&DeploymentStyle>,
    load_balancer_info: Option<&LoadBalancerInfo>,
) -> Option<&'static str> {
    if compute_platform != COMPUTE_PLATFORM_ECS
        || !routes_traffic_behind_a_load_balancer(deployment_style)
    {
        return None;
    }
    if load_balancer_info.is_some_and(names_a_load_balancer) {
        return None;
    }
    Some(ECS_LOAD_BALANCER_INFO_REQUIRED)
}

/// Whether `deployment_style` routes the deployment's traffic behind a load
/// balancer, which is what its `deploymentOption` states.
fn routes_traffic_behind_a_load_balancer(deployment_style: Option<&DeploymentStyle>) -> bool {
    deployment_style
        .and_then(|style| style.deployment_option.as_deref())
        .is_some_and(|option| option.trim() == DEPLOYMENT_OPTION_WITH_TRAFFIC_CONTROL)
}

/// Whether `load_balancer_info` names a load balancer AT ALL.
///
/// The three member lists are the three forms one can be named in — Classic Load
/// Balancers in `elbInfoList`, Application and Network Load Balancer target groups
/// in `targetGroupInfoList`, and the target-group pair an Amazon ECS deployment
/// moves traffic between in `targetGroupPairInfoList`
/// (`smithy_query(query="shape",shape="LoadBalancerInfo")`) — so a configuration
/// whose every list is absent or empty names none, exactly as omitting the member
/// does. Whether an entry names a load balancer the account HOLDS is another
/// question, and not this one.
fn names_a_load_balancer(load_balancer_info: &LoadBalancerInfo) -> bool {
    let named = load_balancer_info
        .elb_info_list
        .as_deref()
        .map_or(0, <[_]>::len)
        + load_balancer_info
            .target_group_info_list
            .as_deref()
            .map_or(0, <[_]>::len)
        + load_balancer_info
            .target_group_pair_info_list
            .as_deref()
            .map_or(0, <[_]>::len);
    named > 0
}

/// How THIS service names the subject of a target group NAME clause.
///
/// Elastic Load Balancing states the same clause with two subjects, and which one
/// belongs to the operation that reports it: a create that TAKES a `Name` states
/// `Target group name '<value>' …`, a call that is HANDED one states `The target
/// group name '<value>' …` (`elbv2::state::TARGET_GROUP_NAME_SUBJECT` and
/// `…::SELECTED_TARGET_GROUP_NAME_SUBJECT`). A deployment group is handed the name,
/// so this service states the clause with the definite subject — the same register
/// its own value refusals use ("The service role ARN <value> was specified in an
/// invalid format", [`crate::state::invalid_role_message`]).
const TARGET_GROUP_NAME_SUBJECT: &str = "The target group name";

/// The `loadBalancerInfo` member a target group name was carried in: the form an
/// Application or Network Load Balancer's target groups are named in.
const TARGET_GROUP_INFO_LIST_MEMBER: &str = "targetGroupInfoList";

/// The `loadBalancerInfo` member a target group name was carried in: the pair an
/// Amazon ECS deployment moves production traffic between.
const TARGET_GROUP_PAIR_INFO_LIST_MEMBER: &str = "targetGroupPairInfoList";

/// The `loadBalancerInfo` member Classic Load Balancers are named in.
const ELB_INFO_LIST_MEMBER: &str = "elbInfoList";

/// How many Classic Load Balancers one `loadBalancerInfo` may name: "You can add up
/// to 10 load balancers to the array"
/// (`smithy_query(query="shape",shape="LoadBalancerInfo")`, `elbInfoList`).
const MAX_ELB_INFO: usize = 10;

/// How many target groups one `loadBalancerInfo` may name: "You can add up to 10
/// target groups to the array"
/// (`smithy_query(query="shape",shape="LoadBalancerInfo")`, `targetGroupInfoList`).
const MAX_TARGET_GROUP_INFO: usize = 10;

/// How many target group PAIRS one `loadBalancerInfo` may name: "an array of
/// `TargeGroupPairInfo` objects with a maximum size of one"
/// (`smithy_query(query="shape",shape="LoadBalancerInfo")`,
/// `targetGroupPairInfoList`).
const MAX_TARGET_GROUP_PAIR_INFO: usize = 1;

/// The refusal a deployment group earns for naming MORE load balancers of one form
/// than a `loadBalancerInfo` may carry, or `None` when every list is within its
/// bound.
///
/// Each of the three members has a documented size, and each is a bound on the
/// configuration rather than on one value in it, so it is read off the lists the
/// group would be stored with — before their entries are read, since a list too
/// long is refused however well formed its entries are — and answered with the one
/// error this member has, `InvalidLoadBalancerInfoException` (HTTP 400). The lists
/// are read in the order `LoadBalancerInfo` declares them.
///
/// The sentence states the form that was over-named, the bound it exceeded and the
/// member that carries it, in the register this service's other quota refusals use
/// ("The maximum number of deployment groups was exceeded: 100 are allowed per
/// application"): its variable parts are the documented BOUND and the member the
/// request over-named, so the caller is told which list to shorten and to what.
///
/// NOTE on fidelity: the BOUNDS are the member's own documentation, but no recording
/// of this refusal's wording was available, so the sentence states the rule in this
/// service's register; if a recorded one becomes readable, this is the only place it
/// has to change.
///
/// Call it from every operation that writes a deployment group, BEFORE anything is
/// stored.
#[must_use]
pub fn load_balancer_list_size_refusal(
    load_balancer_info: Option<&LoadBalancerInfo>,
) -> Option<String> {
    let load_balancer_info = load_balancer_info?;
    let lists = [
        (
            "load balancers",
            ELB_INFO_LIST_MEMBER,
            load_balancer_info
                .elb_info_list
                .as_deref()
                .map_or(0, <[_]>::len),
            MAX_ELB_INFO,
        ),
        (
            "target groups",
            TARGET_GROUP_INFO_LIST_MEMBER,
            load_balancer_info
                .target_group_info_list
                .as_deref()
                .map_or(0, <[_]>::len),
            MAX_TARGET_GROUP_INFO,
        ),
        (
            "target group pairs",
            TARGET_GROUP_PAIR_INFO_LIST_MEMBER,
            load_balancer_info
                .target_group_pair_info_list
                .as_deref()
                .map_or(0, <[_]>::len),
            MAX_TARGET_GROUP_PAIR_INFO,
        ),
    ];
    lists
        .into_iter()
        .find(|(_, _, carried, bound)| carried > bound)
        .map(|(subject, member, _, bound)| {
            // One is the bound of the pair list, so the clause reads for a bound of
            // one as well as for a bound of ten.
            let verb = if bound == 1 { "is" } else { "are" };
            format!(
                "The maximum number of {subject} was exceeded: {bound} {verb} allowed in {member}."
            )
        })
}

/// THIS service's sentence for a target group name longer than a target group name
/// can be, which is what a `loadBalancerInfo` carrying one is refused with.
///
/// Recorded behaviour: the fixed text states the clause and spells the maximum
/// inline, and names the `loadBalancerInfo` MEMBER the rejected value came from —
/// `InvalidLoadBalancerInfoException` also covers a missing or invalid Classic Load
/// Balancer name, so the sentence is the only thing that tells a caller of a
/// multi-member configuration which list to fix. The variable parts are the value
/// the request gave, copied verbatim and unquoted, and the member it was given in.
///
/// The maximum is the one the service that owns target groups publishes
/// (`CreateTargetGroupInput$Name`: "A maximum of 32 alphanumeric characters
/// including hyphens are allowed"), read through that service's own constant so
/// this emulator states one bound.
fn target_group_name_too_long(member: &str, name: &str) -> String {
    format!(
        "The target group name {name} specified in {member} exceeds the maximum allowed \
         length of {} characters.",
        elbv2::state::TARGET_GROUP_NAME_MAX_LENGTH
    )
}

/// The refusal ONE target group name earns, given the `loadBalancerInfo` member it
/// was carried in, or `None` when the value is a target group name.
///
/// The LENGTH clause is this service's own ([`target_group_name_too_long`]); every
/// other clause of the grammar — the character set, the hyphen rule, an empty value
/// — is read, and stated, through the service that owns target groups, so the rules
/// live in one place. The clauses are read in the order AWS reports them: the length
/// before the character set.
fn target_group_name_clause(member: &str, name: &str) -> Option<String> {
    if name.chars().count() > elbv2::state::TARGET_GROUP_NAME_MAX_LENGTH {
        return Some(target_group_name_too_long(member, name));
    }
    elbv2::state::check_target_group_name_as(TARGET_GROUP_NAME_SUBJECT, name).err()
}

/// The refusal a deployment group earns for a TARGET GROUP NAME its
/// `loadBalancerInfo` carries that is no target group name at all, or `None` when
/// every name it carries is one.
///
/// A target group's EXISTENCE is not resolved here — a group is stored naming
/// target groups the account may create afterwards — but the NAME is read at the
/// call that gives it, because a value outside Elastic Load Balancing's name
/// grammar can never name one: "This name must be unique per region and cannot be
/// changed after creation. A maximum of 32 alphanumeric characters including
/// hyphens are allowed, but the name must not begin or end with a hyphen"
/// (`CreateTargetGroupInput$Name`).
///
/// The grammar and the CLAUSE a value broke belong to the service that owns target
/// groups, so both are read through the ONE place this emulator states them
/// (`elbv2::state::check_target_group_name_as`, which reports the clauses in the
/// order AWS reports them — the length before the character set), EXCEPT the length
/// clause, which this service words itself and whose recorded sentence names the
/// `loadBalancerInfo` member the rejected value came from
/// ([`target_group_name_too_long`]). What this service supplies for the remaining
/// clauses is the SUBJECT they are stated with ([`TARGET_GROUP_NAME_SUBJECT`]), so
/// the refusal names the value the request gave and the one rule it broke rather
/// than reciting every rule.
///
/// Both members that carry such a name are read: `targetGroupInfoList`, the form
/// an Application or Network Load Balancer's target groups are named in, and the
/// `targetGroups` of each `targetGroupPairInfoList` entry, the pair an Amazon ECS
/// deployment moves traffic between. The first value that is no name is the one
/// answered, in the order the request names them, and the sentence names the member
/// it was carried in.
///
/// Call it from every operation that writes a deployment group, BEFORE anything is
/// stored: the refusal is judged from the configuration the group would HAVE,
/// whichever members one request happened to carry.
#[must_use]
pub fn target_group_name_refusal(load_balancer_info: Option<&LoadBalancerInfo>) -> Option<String> {
    let load_balancer_info = load_balancer_info?;
    let named_directly = load_balancer_info
        .target_group_info_list
        .iter()
        .flatten()
        .map(|target_group| (TARGET_GROUP_INFO_LIST_MEMBER, target_group));
    let named_in_a_pair = load_balancer_info
        .target_group_pair_info_list
        .iter()
        .flatten()
        .flat_map(|pair| pair.target_groups.iter().flatten())
        .map(|target_group| (TARGET_GROUP_PAIR_INFO_LIST_MEMBER, target_group));
    named_directly
        .chain(named_in_a_pair)
        .filter_map(|(member, target_group)| {
            target_group.name.as_deref().map(|name| (member, name))
        })
        .find_map(|(member, name)| target_group_name_clause(member, name))
}

#[cfg(test)]
mod tests {
    use super::{load_balancer_refusal, ECS_LOAD_BALANCER_INFO_REQUIRED};
    use crate::ops::types::{
        DeploymentStyle, ELBInfo, LoadBalancerInfo, TargetGroupInfo, TargetGroupPairInfo,
    };
    use crate::state::effective_deployment_style;

    /// The style a request's two members materialise to, which is what the gate
    /// judges.
    fn style(
        deployment_type: Option<&str>,
        deployment_option: Option<&str>,
    ) -> Option<DeploymentStyle> {
        effective_deployment_style(Some(DeploymentStyle {
            deployment_type: deployment_type.map(str::to_string),
            deployment_option: deployment_option.map(str::to_string),
        }))
    }

    /// A `loadBalancerInfo` naming one Classic Load Balancer.
    fn classic() -> LoadBalancerInfo {
        LoadBalancerInfo {
            elb_info_list: Some(vec![ELBInfo {
                name: Some("classic-elb".to_string()),
            }]),
            ..LoadBalancerInfo::default()
        }
    }

    /// A `loadBalancerInfo` naming one target group.
    fn target_group() -> LoadBalancerInfo {
        LoadBalancerInfo {
            target_group_info_list: Some(vec![TargetGroupInfo {
                name: Some("tg-blue".to_string()),
            }]),
            ..LoadBalancerInfo::default()
        }
    }

    /// A `loadBalancerInfo` naming the pair an Amazon ECS deployment moves traffic
    /// between.
    fn pair() -> LoadBalancerInfo {
        LoadBalancerInfo {
            target_group_pair_info_list: Some(vec![TargetGroupPairInfo::default()]),
            ..LoadBalancerInfo::default()
        }
    }

    /// One row: the case, the application's compute platform, the style the request
    /// named, the load balancer configuration it carried, and whether the group is
    /// refused.
    type Row = (
        &'static str,
        &'static str,
        Option<DeploymentStyle>,
        Option<LoadBalancerInfo>,
        bool,
    );

    /// RULE: a deployment group on the Amazon ECS compute platform whose style
    /// routes traffic behind a load balancer must name one; every other
    /// combination is left alone.
    #[test]
    fn only_an_ecs_group_that_routes_traffic_and_names_no_load_balancer_is_refused() {
        let traffic_control = || style(Some("BLUE_GREEN"), Some("WITH_TRAFFIC_CONTROL"));
        let rows: Vec<Row> = vec![
            (
                "an ECS group routing traffic with no loadBalancerInfo",
                "ECS",
                traffic_control(),
                None,
                true,
            ),
            (
                "an ECS group routing traffic with an all-empty loadBalancerInfo",
                "ECS",
                traffic_control(),
                Some(LoadBalancerInfo::default()),
                true,
            ),
            (
                "an ECS group routing traffic whose lists are present but empty",
                "ECS",
                traffic_control(),
                Some(LoadBalancerInfo {
                    elb_info_list: Some(Vec::new()),
                    target_group_info_list: Some(Vec::new()),
                    target_group_pair_info_list: Some(Vec::new()),
                }),
                true,
            ),
            (
                "an ECS group routing traffic, in-place, with no loadBalancerInfo",
                "ECS",
                style(Some("IN_PLACE"), Some("WITH_TRAFFIC_CONTROL")),
                None,
                true,
            ),
            (
                "an ECS group naming the target group pair",
                "ECS",
                traffic_control(),
                Some(pair()),
                false,
            ),
            (
                "an ECS group naming a target group",
                "ECS",
                traffic_control(),
                Some(target_group()),
                false,
            ),
            (
                "an ECS group naming a Classic Load Balancer",
                "ECS",
                traffic_control(),
                Some(classic()),
                false,
            ),
            (
                "an ECS group that routes no traffic behind a load balancer",
                "ECS",
                style(Some("BLUE_GREEN"), Some("WITHOUT_TRAFFIC_CONTROL")),
                None,
                false,
            ),
            (
                "an ECS group whose request named no style at all",
                "ECS",
                effective_deployment_style(None),
                None,
                false,
            ),
            (
                "a Server group routing traffic with no loadBalancerInfo",
                "Server",
                traffic_control(),
                None,
                false,
            ),
            (
                "a Lambda group routing traffic with no loadBalancerInfo",
                "Lambda",
                traffic_control(),
                None,
                false,
            ),
        ];

        for (case, compute_platform, deployment_style, load_balancer_info, refused) in rows {
            let answered = load_balancer_refusal(
                compute_platform,
                deployment_style.as_ref(),
                load_balancer_info.as_ref(),
            );
            let expected = refused.then_some(ECS_LOAD_BALANCER_INFO_REQUIRED);
            assert_eq!(
                answered, expected,
                "case {case:?}: expected {expected:?}, got {answered:?}"
            );
        }
    }
}
