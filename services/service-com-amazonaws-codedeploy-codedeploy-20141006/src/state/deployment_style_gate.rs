// SPDX-License-Identifier: CC0-1.0

//! The DEPLOYMENT STYLE a deployment group runs by: the one place a group's
//! `deploymentStyle` is asked whether the compute platform it belongs to can run
//! deployments of that kind.
//!
//! # The rule this module implements
//!
//! `deploymentStyle` states "whether to run an in-place deployment or a
//! blue/green deployment" and "whether to route deployment traffic behind a load
//! balancer" (`smithy_query(query="shape",shape="DeploymentStyle")`), over
//! `DeploymentType` (IN_PLACE | BLUE_GREEN) and `DeploymentOption`
//! (WITH_TRAFFIC_CONTROL | WITHOUT_TRAFFIC_CONTROL). Not every pair describes a
//! deployment every compute platform can PERFORM: an Amazon ECS deployment
//! installs a replacement task set beside the original one and then moves the
//! production traffic route onto it, which is a blue/green deployment that routes
//! its traffic behind a load balancer and is the only deployment that platform
//! runs.
//!
//! So a group's style is admissible only where the platform of the application it
//! belongs to can run it, and a group whose style that platform cannot run is
//! refused rather than stored — `InvalidDeploymentStyleException` (HTTP 400), "an
//! invalid deployment style was specified" — by every operation that WRITES a
//! deployment group, so the groups this service holds all satisfy the condition
//! however they were written. The verdict is judged from the style the group would
//! HAVE (materialised by [`super::effective_deployment_style`], so the
//! server-side default decides for a request that named neither member), never
//! from which members one request happened to carry.
//!
//! # The second rule: a pair no platform runs
//!
//! The two members are not independent either. A BLUE/GREEN deployment installs the
//! revision on a REPLACEMENT environment and then moves the production traffic from
//! the original one onto it, and moving that traffic is exactly what the traffic
//! option decides ("indicates whether to route deployment traffic behind a load
//! balancer",
//! `smithy_query(query="doc",shape="DeploymentStyle",member="deploymentOption")`):
//! a blue/green deployment that routes no traffic behind a load balancer would
//! never cut over, so the pair BLUE_GREEN with WITHOUT_TRAFFIC_CONTROL is a
//! combination CodeDeploy does not support — on any compute platform — and a group
//! stating it is refused with the same `InvalidDeploymentStyleException`, naming the
//! combination it stated ([`unsupported_combination`]). The in-place pairs are both
//! supported: an in-place deployment runs with a load balancer in front of its
//! instances, and without one.
//!
//! An Amazon ECS group states that pair too, and its refusal is the PLATFORM one
//! above ([`ECS_DEPLOYMENT_STYLE_REQUIRED`]): that platform runs one style, and the
//! recorded sentence for a group of it that states any other names the pair it must
//! state rather than the one it did.

use crate::ops::types::DeploymentStyle;

use super::ecs_blue_green::COMPUTE_PLATFORM_ECS;
use super::{DEPLOYMENT_OPTION_WITH_TRAFFIC_CONTROL, DEPLOYMENT_TYPE_BLUE_GREEN};

/// `InvalidDeploymentStyleException` (HTTP 400): the sentence a deployment group
/// on the Amazon ECS compute platform whose style is not the one that platform
/// runs is refused with.
///
/// Entirely fixed text: it states the only admissible pair, so neither the style
/// the request asked for nor any other name, id or value of the request stands in
/// it.
pub const ECS_DEPLOYMENT_STYLE_REQUIRED: &str =
    "For ECS deployment, the deployment type must be BLUE_GREEN, and deployment option must be \
     WITH_TRAFFIC_CONTROL.";

/// `InvalidDeploymentStyleException` (HTTP 400): the sentence a deployment group
/// whose style states a combination of the two members CodeDeploy does not support
/// is refused with.
///
/// Recorded behaviour: the fixed text is " deployment type not supported with " …
/// " option", and the two variable parts are the values the style states — the
/// deployment type first, then the deployment option — so the sentence names the
/// combination the caller has to change rather than reciting the admissible ones.
/// There is no trailing period.
fn unsupported_combination(deployment_type: &str, deployment_option: &str) -> String {
    format!("{deployment_type} deployment type not supported with {deployment_option} option")
}

/// The refusal a deployment group of this configuration earns, or `None` when its
/// style describes a deployment its compute platform can perform.
///
/// `compute_platform` is the platform of the application the group belongs to and
/// `deployment_style` the style the group would HAVE.
///
/// Call it from every operation that writes a deployment group, BEFORE anything is
/// stored and ahead of the gates that ask what the style REQUIRES (the load
/// balancer a traffic-controlled style needs,
/// [`super::load_balancer_gate`]) or resolve the group's targets against the
/// services owning them: a style the platform cannot run at all is refused for
/// itself, whatever the rest of the request names.
#[must_use]
pub fn deployment_style_refusal(
    compute_platform: &str,
    deployment_style: Option<&DeploymentStyle>,
) -> Option<String> {
    if compute_platform == COMPUTE_PLATFORM_ECS {
        if is_blue_green_with_traffic_control(deployment_style) {
            return None;
        }
        return Some(ECS_DEPLOYMENT_STYLE_REQUIRED.to_string());
    }
    // The pair no platform runs: a blue/green deployment with no traffic to move
    // onto the replacement environment.
    let style = deployment_style?;
    let deployment_type = style.deployment_type.as_deref()?.trim();
    let deployment_option = style.deployment_option.as_deref()?.trim();
    if deployment_type == DEPLOYMENT_TYPE_BLUE_GREEN
        && deployment_option != DEPLOYMENT_OPTION_WITH_TRAFFIC_CONTROL
    {
        return Some(unsupported_combination(deployment_type, deployment_option));
    }
    None
}

/// Whether `deployment_style` is the blue/green, traffic-controlled pair an Amazon
/// ECS deployment is.
fn is_blue_green_with_traffic_control(deployment_style: Option<&DeploymentStyle>) -> bool {
    let Some(style) = deployment_style else {
        return false;
    };
    let names =
        |member: Option<&String>, value: &str| member.is_some_and(|named| named.trim() == value);
    names(style.deployment_type.as_ref(), DEPLOYMENT_TYPE_BLUE_GREEN)
        && names(
            style.deployment_option.as_ref(),
            DEPLOYMENT_OPTION_WITH_TRAFFIC_CONTROL,
        )
}

#[cfg(test)]
mod tests {
    use super::{deployment_style_refusal, unsupported_combination, ECS_DEPLOYMENT_STYLE_REQUIRED};
    use crate::ops::types::DeploymentStyle;
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

    /// One row: the case, the application's compute platform, the style the
    /// request named, and the sentence the group is refused with — `None` where it
    /// is admissible.
    type Row = (
        &'static str,
        &'static str,
        Option<DeploymentStyle>,
        Option<String>,
    );

    /// The sentence a group of any other platform that states the unsupported pair
    /// earns.
    fn unsupported_blue_green() -> Option<String> {
        Some(unsupported_combination(
            "BLUE_GREEN",
            "WITHOUT_TRAFFIC_CONTROL",
        ))
    }

    /// The sentence an Amazon ECS group whose style is not that platform's earns.
    fn ecs_style_required() -> Option<String> {
        Some(ECS_DEPLOYMENT_STYLE_REQUIRED.to_string())
    }

    /// RULE 1: a deployment group on the Amazon ECS compute platform may only run
    /// the blue/green, traffic-controlled deployment that platform performs.
    /// RULE 2: on every other platform, the one pair CodeDeploy does not support is
    /// blue/green WITHOUT traffic control — a blue/green deployment with no traffic
    /// to move onto the replacement environment — and the refusal names the
    /// combination the style stated.
    ///
    /// The platforms are `ComputePlatform`'s own values
    /// (`smithy_query(query="shape",shape="ComputePlatform")`: Server | Lambda |
    /// ECS) and the styles every pair of `DeploymentType` (IN_PLACE | BLUE_GREEN)
    /// and `DeploymentOption` (WITH_TRAFFIC_CONTROL | WITHOUT_TRAFFIC_CONTROL),
    /// plus the half-stated and omitted forms the server-side default completes.
    #[test]
    fn a_style_is_refused_where_its_platform_or_its_own_pair_cannot_run_it() {
        let rows: Vec<Row> = vec![
            (
                "an ECS group that is blue/green with traffic control",
                "ECS",
                style(Some("BLUE_GREEN"), Some("WITH_TRAFFIC_CONTROL")),
                None,
            ),
            (
                "an ECS group that is blue/green without traffic control",
                "ECS",
                style(Some("BLUE_GREEN"), Some("WITHOUT_TRAFFIC_CONTROL")),
                ecs_style_required(),
            ),
            (
                "an ECS group that is in-place with traffic control",
                "ECS",
                style(Some("IN_PLACE"), Some("WITH_TRAFFIC_CONTROL")),
                ecs_style_required(),
            ),
            (
                "an ECS group that is in-place without traffic control",
                "ECS",
                style(Some("IN_PLACE"), Some("WITHOUT_TRAFFIC_CONTROL")),
                ecs_style_required(),
            ),
            (
                "an ECS group naming only the blue/green type",
                "ECS",
                style(Some("BLUE_GREEN"), None),
                ecs_style_required(),
            ),
            (
                "an ECS group naming only traffic control",
                "ECS",
                style(None, Some("WITH_TRAFFIC_CONTROL")),
                ecs_style_required(),
            ),
            (
                "an ECS group whose request named no style at all",
                "ECS",
                crate::state::effective_deployment_style(None),
                ecs_style_required(),
            ),
            ("an ECS group holding no style", "ECS", None, ecs_style_required()),
            (
                "a Server group that is in-place without traffic control",
                "Server",
                style(Some("IN_PLACE"), Some("WITHOUT_TRAFFIC_CONTROL")),
                None,
            ),
            (
                "a Server group that is in-place with traffic control",
                "Server",
                style(Some("IN_PLACE"), Some("WITH_TRAFFIC_CONTROL")),
                None,
            ),
            (
                "a Server group that is blue/green with traffic control",
                "Server",
                style(Some("BLUE_GREEN"), Some("WITH_TRAFFIC_CONTROL")),
                None,
            ),
            (
                "a Server group that is blue/green without traffic control",
                "Server",
                style(Some("BLUE_GREEN"), Some("WITHOUT_TRAFFIC_CONTROL")),
                unsupported_blue_green(),
            ),
            (
                // The server-side default completes the pair, so a group naming
                // only the blue/green type states the unsupported one.
                "a Server group naming only the blue/green type",
                "Server",
                style(Some("BLUE_GREEN"), None),
                unsupported_blue_green(),
            ),
            (
                "a Lambda group that is in-place without traffic control",
                "Lambda",
                style(Some("IN_PLACE"), Some("WITHOUT_TRAFFIC_CONTROL")),
                None,
            ),
            (
                "a Lambda group that is blue/green without traffic control",
                "Lambda",
                style(Some("BLUE_GREEN"), Some("WITHOUT_TRAFFIC_CONTROL")),
                unsupported_blue_green(),
            ),
            (
                "a Lambda group whose request named no style at all",
                "Lambda",
                crate::state::effective_deployment_style(None),
                None,
            ),
            ("a Server group holding no style", "Server", None, None),
        ];

        for (case, compute_platform, deployment_style, expected) in rows {
            let answered = deployment_style_refusal(compute_platform, deployment_style.as_ref());
            assert_eq!(
                answered, expected,
                "case {case:?}: expected {expected:?}, got {answered:?}"
            );
        }
    }
}
