// SPDX-License-Identifier: CC0-1.0

//! The DEPLOYMENT STYLE a CodeDeploy deployment group may run by, driven over the
//! wire through the shared [`Emulator`] harness of `codedeploy_lifecycle`.
//!
//! RULE 1: a deployment group's `deploymentStyle` has to describe a deployment the
//! compute platform of the application it belongs to can perform, so the style the
//! group would HAVE is judged against that platform before anything is stored. An
//! Amazon ECS deployment installs a replacement task set and then moves the
//! production traffic route onto it, which is the blue/green, traffic-controlled
//! style and the only one that platform runs: a request that would create or leave
//! an Amazon ECS group running any other style — including the server-side default
//! a request naming no style at all gets — is refused HTTP 400
//! `InvalidDeploymentStyleException` with the fixed sentence
//! [`ECS_DEPLOYMENT_STYLE_REQUIRED`], and nothing is stored for it.
//!
//! RULE 2: the two members of the style have to describe one deployment between
//! them. A blue/green deployment installs the revision on a replacement environment
//! and cuts the production traffic over to it, and whether that traffic moves behind
//! a load balancer is what the traffic option says, so BLUE_GREEN with
//! WITHOUT_TRAFFIC_CONTROL is a combination CodeDeploy does not support on ANY
//! compute platform: it is refused with the same modelled error, stating the
//! combination the request named — `<TYPE> deployment type not supported with
//! <OPTION> option` ([`unsupported_combination`]). Both in-place pairs are
//! supported, so a group on a platform that is not Amazon ECS runs whichever of
//! those it names.
//!
//! The rows differ along what the rules generalise over: the application's compute
//! platform (every member of `ComputePlatform`,
//! `smithy_query(query="shape",shape="ComputePlatform")`: Server | Lambda | ECS),
//! and which style the request names — each pair of `DeploymentType` (IN_PLACE |
//! BLUE_GREEN) and `DeploymentOption` (WITH_TRAFFIC_CONTROL |
//! WITHOUT_TRAFFIC_CONTROL), each member named on its own so the other defaults,
//! and the member omitted entirely — on a create and on an update.

use serde_json::{json, Value};

use crate::codedeploy_lifecycle::{error_code, Emulator};

/// The modelled error a group whose style its platform cannot run earns
/// (`smithy_query(query="shape",shape="InvalidDeploymentStyleException")`: a client
/// error, HTTP 400).
const INVALID_DEPLOYMENT_STYLE: &str = "InvalidDeploymentStyleException";

/// CodeDeploy's sentence for an Amazon ECS deployment group whose style is not the
/// one that platform runs: entirely fixed text, no part of it taken from the
/// request or from state.
const ECS_DEPLOYMENT_STYLE_REQUIRED: &str =
    "For ECS deployment, the deployment type must be BLUE_GREEN, and deployment option must be \
     WITH_TRAFFIC_CONTROL.";

/// CodeDeploy's sentence for a style whose two members name a combination the
/// service does not support: fixed text around the two values the style states.
fn unsupported_combination(deployment_type: &str, deployment_option: &str) -> String {
    format!("{deployment_type} deployment type not supported with {deployment_option} option")
}

/// The sentence a group that states blue/green without traffic control earns, which
/// is the one unsupported pair.
fn blue_green_without_traffic_control() -> String {
    unsupported_combination("BLUE_GREEN", "WITHOUT_TRAFFIC_CONTROL")
}

/// The service role every deployment group here is created with.
const ROLE: &str = "arn:aws:iam::123456789012:role/CodeDeployRole";

/// The style the service materialises for a member a request left unset
/// (`crate::deployment_group_defaults` asserts that defaulting on its own).
const DEFAULT_TYPE: &str = "IN_PLACE";
/// See [`DEFAULT_TYPE`].
const DEFAULT_OPTION: &str = "WITHOUT_TRAFFIC_CONTROL";

/// A `deploymentStyle` naming the members the row states.
fn style(deployment_type: Option<&str>, deployment_option: Option<&str>) -> Value {
    let mut named = json!({});
    if let Some(deployment_type) = deployment_type {
        named["deploymentType"] = json!(deployment_type);
    }
    if let Some(deployment_option) = deployment_option {
        named["deploymentOption"] = json!(deployment_option);
    }
    named
}

/// One row of the create table: the case, the application's compute platform, the
/// `deploymentStyle` the request names, and either the `(deploymentType,
/// deploymentOption)` the created group reports or the sentence the refusal states.
type CreateRow = (
    &'static str,
    &'static str,
    Option<Value>,
    Result<(&'static str, &'static str), String>,
);

/// The `deploymentGroupInfo` `GetDeploymentGroup` reports for `group`.
async fn group_info(emulator: &Emulator, application: &str, group: &str) -> Value {
    emulator
        .ok(
            "GetDeploymentGroup",
            json!({"applicationName": application, "deploymentGroupName": group}),
        )
        .await["deploymentGroupInfo"]
        .clone()
}

/// The deployment group names `ListDeploymentGroups` reports for `application`.
async fn listed_groups(emulator: &Emulator, application: &str) -> Vec<String> {
    emulator
        .ok(
            "ListDeploymentGroups",
            json!({"applicationName": application}),
        )
        .await["deploymentGroups"]
        .as_array()
        .map(|names| {
            names
                .iter()
                .map(|name| name.as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// RULE 1 and RULE 2 on a CREATE: an Amazon ECS deployment group is created only as
/// a blue/green group with traffic control, and no group of any platform is created
/// as a blue/green group WITHOUT it; each refusal states its own sentence and stores
/// nothing. A group on another platform is created with whatever supported style it
/// named.
#[tokio::test]
async fn a_group_is_created_only_with_a_style_its_platform_and_its_pair_support() {
    let emulator = Emulator::new();
    let ecs_required = || Err(ECS_DEPLOYMENT_STYLE_REQUIRED.to_string());
    let rows: Vec<CreateRow> = vec![
        (
            "an ECS group that is blue/green with traffic control",
            "ECS",
            Some(style(Some("BLUE_GREEN"), Some("WITH_TRAFFIC_CONTROL"))),
            Ok(("BLUE_GREEN", "WITH_TRAFFIC_CONTROL")),
        ),
        (
            // The PLATFORM refusal comes first for an Amazon ECS group, whose one
            // admissible style the sentence names.
            "an ECS group that is blue/green WITHOUT traffic control",
            "ECS",
            Some(style(Some("BLUE_GREEN"), Some("WITHOUT_TRAFFIC_CONTROL"))),
            ecs_required(),
        ),
        (
            "an ECS group that is in-place with traffic control",
            "ECS",
            Some(style(Some("IN_PLACE"), Some("WITH_TRAFFIC_CONTROL"))),
            ecs_required(),
        ),
        (
            "an ECS group that is in-place without traffic control",
            "ECS",
            Some(style(Some("IN_PLACE"), Some("WITHOUT_TRAFFIC_CONTROL"))),
            ecs_required(),
        ),
        (
            "an ECS group naming only the blue/green type, so the option defaults",
            "ECS",
            Some(style(Some("BLUE_GREEN"), None)),
            ecs_required(),
        ),
        (
            "an ECS group naming only traffic control, so the type defaults",
            "ECS",
            Some(style(None, Some("WITH_TRAFFIC_CONTROL"))),
            ecs_required(),
        ),
        (
            "an ECS group naming no style at all",
            "ECS",
            None,
            ecs_required(),
        ),
        (
            "an ECS group naming an all-empty style",
            "ECS",
            Some(json!({})),
            ecs_required(),
        ),
        (
            "a Server group that is in-place without traffic control",
            "Server",
            Some(style(Some("IN_PLACE"), Some("WITHOUT_TRAFFIC_CONTROL"))),
            Ok(("IN_PLACE", "WITHOUT_TRAFFIC_CONTROL")),
        ),
        (
            "a Server group that is in-place with traffic control",
            "Server",
            Some(style(Some("IN_PLACE"), Some("WITH_TRAFFIC_CONTROL"))),
            Ok(("IN_PLACE", "WITH_TRAFFIC_CONTROL")),
        ),
        (
            "a Server group that is blue/green with traffic control",
            "Server",
            Some(style(Some("BLUE_GREEN"), Some("WITH_TRAFFIC_CONTROL"))),
            Ok(("BLUE_GREEN", "WITH_TRAFFIC_CONTROL")),
        ),
        (
            "a Server group that is blue/green without traffic control",
            "Server",
            Some(style(Some("BLUE_GREEN"), Some("WITHOUT_TRAFFIC_CONTROL"))),
            Err(blue_green_without_traffic_control()),
        ),
        (
            // The server-side default completes the pair, so naming the blue/green
            // type alone states the unsupported combination.
            "a Server group naming only the blue/green type, so the option defaults",
            "Server",
            Some(style(Some("BLUE_GREEN"), None)),
            Err(blue_green_without_traffic_control()),
        ),
        (
            "a Server group naming no style at all",
            "Server",
            None,
            Ok((DEFAULT_TYPE, DEFAULT_OPTION)),
        ),
        (
            "a Lambda group that is blue/green without traffic control",
            "Lambda",
            Some(style(Some("BLUE_GREEN"), Some("WITHOUT_TRAFFIC_CONTROL"))),
            Err(blue_green_without_traffic_control()),
        ),
        (
            "a Lambda group that is blue/green with traffic control",
            "Lambda",
            Some(style(Some("BLUE_GREEN"), Some("WITH_TRAFFIC_CONTROL"))),
            Ok(("BLUE_GREEN", "WITH_TRAFFIC_CONTROL")),
        ),
        (
            "a Lambda group naming only the in-place type, so the option defaults",
            "Lambda",
            Some(style(Some("IN_PLACE"), None)),
            Ok(("IN_PLACE", DEFAULT_OPTION)),
        ),
        (
            "a Lambda group naming no style at all",
            "Lambda",
            None,
            Ok((DEFAULT_TYPE, DEFAULT_OPTION)),
        ),
    ];

    for (index, (case, platform, named, expected)) in rows.iter().enumerate() {
        let application = format!("style-app-{index:02}");
        let group = format!("{application}-dg");
        emulator
            .ok(
                "CreateApplication",
                json!({"applicationName": application, "computePlatform": platform}),
            )
            .await;
        let mut request = json!({
            "applicationName": application,
            "deploymentGroupName": group,
            "serviceRoleArn": ROLE,
        });
        if let Some(named) = named {
            request["deploymentStyle"] = named.clone();
        }
        if *platform == "ECS" {
            // The load balancer an Amazon ECS group's traffic-controlled style
            // routes traffic behind is a precondition of its own, so every row of
            // this table carries one: what is under test here is the style.
            request["loadBalancerInfo"] = json!({"targetGroupInfoList": [{"name": "tg-blue"}]});
        }

        let (status, body) = emulator.call("CreateDeploymentGroup", request).await;
        let (deployment_type, deployment_option) = match expected {
            Ok(style) => *style,
            Err(sentence) => {
                assert_eq!(
                    (status, error_code(&body).as_str(), body["message"].as_str()),
                    (400, INVALID_DEPLOYMENT_STYLE, Some(sentence.as_str())),
                    "case {case:?}: expected HTTP 400 {INVALID_DEPLOYMENT_STYLE} with \
                     {sentence:?}, got {status} with {body:?}"
                );
                // Nothing is stored for a refused create: the application holds no
                // group at all.
                let listed = listed_groups(&emulator, &application).await;
                assert!(
                    listed.is_empty(),
                    "case {case:?}: expected the refused group not to be stored, got {listed:?}"
                );
                continue;
            }
        };
        assert_eq!(
            status, 200,
            "case {case:?}: expected the group to be created, got {status} with {body:?}"
        );
        // Read the group back through its own Get: the style the group runs by is
        // what it reports.
        let info = group_info(&emulator, &application, &group).await;
        assert_eq!(
            (
                info["deploymentStyle"]["deploymentType"].as_str(),
                info["deploymentStyle"]["deploymentOption"].as_str(),
            ),
            (Some(deployment_type), Some(deployment_option)),
            "case {case:?}: expected the style {deployment_type}/{deployment_option}, got {info:?}"
        );
        let listed = listed_groups(&emulator, &application).await;
        assert_eq!(
            listed,
            vec![group.clone()],
            "case {case:?}: expected the application to hold the created group, got {listed:?}"
        );
    }
}

/// RULE on an UPDATE: the style the group would END UP with is what the
/// precondition is judged on, so an update that would move an Amazon ECS group off
/// the blue/green, traffic-controlled style is refused and changes nothing, while
/// an update that names that style again, or names no style at all, is served.
#[tokio::test]
async fn an_update_may_not_move_an_ecs_group_off_blue_green_with_traffic_control() {
    let emulator = Emulator::new();
    const APPLICATION: &str = "style-update-ecs";
    const GROUP: &str = "style-update-ecs-dg";
    emulator
        .ok(
            "CreateApplication",
            json!({"applicationName": APPLICATION, "computePlatform": "ECS"}),
        )
        .await;
    emulator
        .ok(
            "CreateDeploymentGroup",
            json!({
                "applicationName": APPLICATION,
                "deploymentGroupName": GROUP,
                "serviceRoleArn": ROLE,
                "deploymentStyle": style(Some("BLUE_GREEN"), Some("WITH_TRAFFIC_CONTROL")),
                "loadBalancerInfo": {"targetGroupInfoList": [{"name": "tg-blue"}]},
            }),
        )
        .await;

    // Rows are successive updates on ONE group: each refused row must leave the
    // group running the style it already had.
    let rows: &[(&str, Option<Value>, bool)] = &[
        (
            "off traffic control",
            Some(style(Some("BLUE_GREEN"), Some("WITHOUT_TRAFFIC_CONTROL"))),
            false,
        ),
        (
            "onto an in-place deployment",
            Some(style(Some("IN_PLACE"), Some("WITH_TRAFFIC_CONTROL"))),
            false,
        ),
        (
            "naming only the in-place type, so the option defaults",
            Some(style(Some("IN_PLACE"), None)),
            false,
        ),
        ("an all-empty style", Some(json!({})), false),
        (
            "naming the admissible style again",
            Some(style(Some("BLUE_GREEN"), Some("WITH_TRAFFIC_CONTROL"))),
            true,
        ),
        ("naming no style at all", None, true),
    ];

    for (case, named, served) in rows {
        let mut request = json!({
            "applicationName": APPLICATION,
            "currentDeploymentGroupName": GROUP,
        });
        if let Some(named) = named {
            request["deploymentStyle"] = named.clone();
        }
        let (status, body) = emulator.call("UpdateDeploymentGroup", request).await;
        if *served {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the update to be served, got {status} with {body:?}"
            );
        } else {
            assert_eq!(
                (
                    status,
                    error_code(&body).as_str(),
                    body["message"].as_str()
                ),
                (
                    400,
                    INVALID_DEPLOYMENT_STYLE,
                    Some(ECS_DEPLOYMENT_STYLE_REQUIRED)
                ),
                "case {case:?}: expected HTTP 400 {INVALID_DEPLOYMENT_STYLE} with \
                 {ECS_DEPLOYMENT_STYLE_REQUIRED:?}, got {status} with {body:?}"
            );
        }
        // Served or refused, the group still runs the one style its platform
        // admits.
        let info = group_info(&emulator, APPLICATION, GROUP).await;
        assert_eq!(
            (
                info["deploymentStyle"]["deploymentType"].as_str(),
                info["deploymentStyle"]["deploymentOption"].as_str(),
            ),
            (Some("BLUE_GREEN"), Some("WITH_TRAFFIC_CONTROL")),
            "case {case:?}: expected the group to keep its admissible style, got {info:?}"
        );
    }
}

/// One row of the update table: the case, the `deploymentStyle` the update names,
/// the `(deploymentType, deploymentOption)` the group runs by afterwards, and the
/// sentence the refusal states where the update is refused.
type UpdateRow = (
    &'static str,
    Value,
    (&'static str, &'static str),
    Option<String>,
);

/// RULE 1 and RULE 2 on an UPDATE, the other side: a group on a platform that runs
/// every supported style is moved onto whichever supported style the update names,
/// and reports it — while an update that would leave it on the unsupported pair is
/// refused, stating that pair, and the group keeps the style it had.
#[tokio::test]
async fn an_update_moves_a_group_of_another_platform_onto_any_supported_style() {
    let emulator = Emulator::new();
    const APPLICATION: &str = "style-update-server";
    const GROUP: &str = "style-update-server-dg";
    emulator
        .ok(
            "CreateApplication",
            json!({"applicationName": APPLICATION, "computePlatform": "Server"}),
        )
        .await;
    emulator
        .ok(
            "CreateDeploymentGroup",
            json!({
                "applicationName": APPLICATION,
                "deploymentGroupName": GROUP,
                "serviceRoleArn": ROLE,
            }),
        )
        .await;

    // Rows are successive updates on ONE group: each names a style and states the
    // style the group runs by afterwards, which for a refused row is the one it
    // already had.
    let rows: &[UpdateRow] = &[
        (
            "onto blue/green with traffic control",
            style(Some("BLUE_GREEN"), Some("WITH_TRAFFIC_CONTROL")),
            ("BLUE_GREEN", "WITH_TRAFFIC_CONTROL"),
            None,
        ),
        (
            "onto blue/green without traffic control",
            style(Some("BLUE_GREEN"), Some("WITHOUT_TRAFFIC_CONTROL")),
            ("BLUE_GREEN", "WITH_TRAFFIC_CONTROL"),
            Some(blue_green_without_traffic_control()),
        ),
        (
            // The option the update leaves unnamed is completed by the
            // server-side default, so this names the unsupported pair too.
            "onto the blue/green type alone",
            style(Some("BLUE_GREEN"), None),
            ("BLUE_GREEN", "WITH_TRAFFIC_CONTROL"),
            Some(blue_green_without_traffic_control()),
        ),
        (
            "naming only the type, so the option defaults",
            style(Some("IN_PLACE"), None),
            ("IN_PLACE", DEFAULT_OPTION),
            None,
        ),
        (
            "naming only the option, so the type defaults",
            style(None, Some("WITH_TRAFFIC_CONTROL")),
            (DEFAULT_TYPE, "WITH_TRAFFIC_CONTROL"),
            None,
        ),
    ];

    for (case, named, (deployment_type, deployment_option), refusal) in rows {
        let (status, body) = emulator
            .call(
                "UpdateDeploymentGroup",
                json!({
                    "applicationName": APPLICATION,
                    "currentDeploymentGroupName": GROUP,
                    "deploymentStyle": named,
                }),
            )
            .await;
        match refusal {
            Some(sentence) => assert_eq!(
                (status, error_code(&body).as_str(), body["message"].as_str()),
                (400, INVALID_DEPLOYMENT_STYLE, Some(sentence.as_str())),
                "case {case:?}: expected HTTP 400 {INVALID_DEPLOYMENT_STYLE} with {sentence:?}, \
                 got {status} with {body:?}"
            ),
            None => assert_eq!(
                status, 200,
                "case {case:?}: expected the update to be served, got {status} with {body:?}"
            ),
        }
        let info = group_info(&emulator, APPLICATION, GROUP).await;
        assert_eq!(
            (
                info["deploymentStyle"]["deploymentType"].as_str(),
                info["deploymentStyle"]["deploymentOption"].as_str(),
            ),
            (Some(*deployment_type), Some(*deployment_option)),
            "case {case:?}: expected the style {deployment_type}/{deployment_option}, got {info:?}"
        );
    }
}
