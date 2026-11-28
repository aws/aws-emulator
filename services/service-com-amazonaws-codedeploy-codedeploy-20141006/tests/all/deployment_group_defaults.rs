// SPDX-License-Identifier: CC0-1.0

//! The server-side defaults a deployment group runs by, driven over the wire.
//!
//! RULE under test: a deployment group always runs one deployment type with one
//! traffic option and one outdated-instances strategy, so `GetDeploymentGroup`
//! reports `deploymentStyle` and `outdatedInstancesStrategy` as the values the
//! group HAS — the request's where it named them, the service's default for every
//! member it left unset — rather than omitting the member. The model's own
//! documentation states the strategy default ("If this option is set to
//! `UPDATE` or is unspecified, CodeDeploy initiates one or more 'auto-update
//! outdated instances' deployments",
//! `smithy_query(query="doc",shape="DeploymentGroupInfo",member="outdatedInstancesStrategy")`).
//!
//! Every case goes through the same front door a client uses — the shared
//! [`Emulator`] harness of `codedeploy_lifecycle`.

use serde_json::{json, Value};

use crate::codedeploy_lifecycle::Emulator;

/// The `deploymentStyle` the service materialises when a request names neither
/// member: an in-place deployment with no traffic control. Both values are
/// members of the modelled enums
/// (`smithy_query(query="shape",shape="DeploymentType")`: IN_PLACE | BLUE_GREEN;
/// `smithy_query(query="shape",shape="DeploymentOption")`: WITH_TRAFFIC_CONTROL
/// | WITHOUT_TRAFFIC_CONTROL).
const DEFAULT_TYPE: &str = "IN_PLACE";
/// See [`DEFAULT_TYPE`].
const DEFAULT_OPTION: &str = "WITHOUT_TRAFFIC_CONTROL";
/// The `OutdatedInstancesStrategy` an unspecified strategy behaves as, per the
/// member's own documentation (`smithy_query(query="shape",shape="OutdatedInstancesStrategy")`:
/// UPDATE | IGNORE).
const DEFAULT_STRATEGY: &str = "UPDATE";

/// The service role every group here is created with.
const ROLE: &str = "arn:aws:iam::123456789012:role/CodeDeployRole";

/// One row of the defaults table: the case name, the compute platform of the
/// application, the members the create request adds, and the
/// `(deploymentType, deploymentOption, outdatedInstancesStrategy)` the group
/// must report.
type DefaultsCase<'a> = (&'a str, &'a str, Value, (&'a str, &'a str, &'a str));

/// Create an application on `platform` and one deployment group on it carrying
/// `extra`, answering the `deploymentGroupInfo` the group reads back as.
async fn group_info(
    emulator: &Emulator,
    application: &str,
    platform: &str,
    extra: &Value,
) -> Value {
    emulator
        .ok(
            "CreateApplication",
            json!({"applicationName": application, "computePlatform": platform}),
        )
        .await;
    let group = format!("{application}-dg");
    let mut request = json!({
        "applicationName": application,
        "deploymentGroupName": group,
        "serviceRoleArn": ROLE,
    });
    for (member, value) in extra.as_object().into_iter().flatten() {
        request[member] = value.clone();
    }
    emulator.ok("CreateDeploymentGroup", request).await;
    read_group(emulator, application, &group).await
}

/// The `deploymentGroupInfo` `GetDeploymentGroup` answers for `group`.
async fn read_group(emulator: &Emulator, application: &str, group: &str) -> Value {
    emulator
        .ok(
            "GetDeploymentGroup",
            json!({"applicationName": application, "deploymentGroupName": group}),
        )
        .await["deploymentGroupInfo"]
        .clone()
}

/// RULE: every member of `deploymentStyle` and `outdatedInstancesStrategy` is
/// reported, the request's value where it named one and the service's default
/// where it did not.
///
/// Rows differ along what the rule generalises over: which of the two
/// `deploymentStyle` members the request names (neither, each one alone, both),
/// every value of the `OutdatedInstancesStrategy` enum, and the compute platform
/// of the application the group belongs to.
#[tokio::test]
async fn a_deployment_group_reports_the_style_and_strategy_it_runs_by() {
    let emulator = Emulator::new();
    let cases: &[DefaultsCase<'_>] = &[
        (
            "neither member named",
            "Server",
            json!({}),
            (DEFAULT_TYPE, DEFAULT_OPTION, DEFAULT_STRATEGY),
        ),
        (
            // The type is named on its own, so the server-side default completes
            // the option. It names the IN_PLACE type: a half-stated BLUE_GREEN
            // style completes to the one pair CodeDeploy does not support, which
            // is refused rather than stored (`deployment_style_precondition`).
            "only the deployment type named",
            "Server",
            json!({"deploymentStyle": {"deploymentType": "IN_PLACE"}}),
            ("IN_PLACE", DEFAULT_OPTION, DEFAULT_STRATEGY),
        ),
        (
            "only the traffic option named",
            "Server",
            json!({"deploymentStyle": {"deploymentOption": "WITH_TRAFFIC_CONTROL"}}),
            (DEFAULT_TYPE, "WITH_TRAFFIC_CONTROL", DEFAULT_STRATEGY),
        ),
        (
            "both members named",
            "Server",
            json!({"deploymentStyle": {
                "deploymentType": "BLUE_GREEN",
                "deploymentOption": "WITH_TRAFFIC_CONTROL",
            }}),
            ("BLUE_GREEN", "WITH_TRAFFIC_CONTROL", DEFAULT_STRATEGY),
        ),
        (
            "the strategy named as the default value",
            "Server",
            json!({"outdatedInstancesStrategy": "UPDATE"}),
            (DEFAULT_TYPE, DEFAULT_OPTION, "UPDATE"),
        ),
        (
            "the strategy named as the other value",
            "Server",
            json!({"outdatedInstancesStrategy": "IGNORE"}),
            (DEFAULT_TYPE, DEFAULT_OPTION, "IGNORE"),
        ),
        (
            "a Lambda application",
            "Lambda",
            json!({}),
            (DEFAULT_TYPE, DEFAULT_OPTION, DEFAULT_STRATEGY),
        ),
        (
            // The Amazon ECS compute platform admits only the blue/green,
            // traffic-controlled style, so this row names it (and the load
            // balancer that style routes traffic behind) and reads it back; the
            // strategy is still the service's default.
            "an ECS application",
            "ECS",
            crate::codedeploy_lifecycle::ecs_group_members(),
            ("BLUE_GREEN", "WITH_TRAFFIC_CONTROL", DEFAULT_STRATEGY),
        ),
    ];

    for (index, (case, platform, extra, expected)) in cases.iter().enumerate() {
        let application = format!("defaults-app-{index:02}");
        let info = group_info(&emulator, &application, platform, extra).await;
        let (deployment_type, deployment_option, strategy) = *expected;
        assert_eq!(
            info["deploymentStyle"]["deploymentType"].as_str(),
            Some(deployment_type),
            "case {case:?}: expected deploymentType {deployment_type}, got {info:?}"
        );
        assert_eq!(
            info["deploymentStyle"]["deploymentOption"].as_str(),
            Some(deployment_option),
            "case {case:?}: expected deploymentOption {deployment_option}, got {info:?}"
        );
        assert_eq!(
            info["outdatedInstancesStrategy"].as_str(),
            Some(strategy),
            "case {case:?}: expected outdatedInstancesStrategy {strategy}, got {info:?}"
        );
        // A selection or trigger list the group holds is reported as the list it
        // holds, empty included, so a client can index the member.
        for member in [
            "ec2TagFilters",
            "onPremisesInstanceTagFilters",
            "autoScalingGroups",
            "triggerConfigurations",
        ] {
            assert_eq!(
                info[member],
                json!([]),
                "case {case:?}: expected {member} to be reported as [], got {info:?}"
            );
        }
        // Reading the group again reports the same record: materialising a
        // default is something the WRITE did, not something a read does.
        let again = read_group(&emulator, &application, &format!("{application}-dg")).await;
        assert_eq!(
            again, info,
            "case {case:?}: a read reports the same record, got {again:?}"
        );
    }
}

/// RULE: an update materialises the same defaults — a `deploymentStyle` naming
/// one member leaves the group with both — and a member the update does not name
/// keeps the value it had.
#[tokio::test]
async fn an_update_materialises_the_same_defaults() {
    let emulator = Emulator::new();
    let application = "defaults-updated";
    let group = format!("{application}-dg");
    let created = group_info(&emulator, application, "Server", &json!({})).await;
    assert_eq!(
        created["deploymentStyle"]["deploymentType"].as_str(),
        Some(DEFAULT_TYPE),
        "case \"created\": expected the default style, got {created:?}"
    );

    // Rows are successive updates on ONE group: a half-stated style, the other
    // half-stated style, a strategy on its own (which leaves the style alone),
    // and a fully stated style.
    let cases: &[(&str, Value, (&str, &str, &str))] = &[
        (
            // See the create's row: a half-stated BLUE_GREEN style completes to
            // the pair CodeDeploy does not support, so the half-stated row here
            // names the in-place type too.
            "a style naming only the type",
            json!({"deploymentStyle": {"deploymentType": "IN_PLACE"}}),
            ("IN_PLACE", DEFAULT_OPTION, DEFAULT_STRATEGY),
        ),
        (
            "a style naming only the option",
            json!({"deploymentStyle": {"deploymentOption": "WITH_TRAFFIC_CONTROL"}}),
            (DEFAULT_TYPE, "WITH_TRAFFIC_CONTROL", DEFAULT_STRATEGY),
        ),
        (
            "a strategy on its own",
            json!({"outdatedInstancesStrategy": "IGNORE"}),
            (DEFAULT_TYPE, "WITH_TRAFFIC_CONTROL", "IGNORE"),
        ),
        (
            "a fully stated style",
            json!({"deploymentStyle": {
                "deploymentType": "BLUE_GREEN",
                "deploymentOption": "WITH_TRAFFIC_CONTROL",
            }}),
            ("BLUE_GREEN", "WITH_TRAFFIC_CONTROL", "IGNORE"),
        ),
    ];

    for (case, patch, expected) in cases {
        let mut request = json!({
            "applicationName": application,
            "currentDeploymentGroupName": group,
        });
        for (member, value) in patch.as_object().into_iter().flatten() {
            request[member] = value.clone();
        }
        emulator.ok("UpdateDeploymentGroup", request).await;

        let info = read_group(&emulator, application, &group).await;
        let (deployment_type, deployment_option, strategy) = *expected;
        assert_eq!(
            info["deploymentStyle"]["deploymentType"].as_str(),
            Some(deployment_type),
            "case {case:?}: expected deploymentType {deployment_type}, got {info:?}"
        );
        assert_eq!(
            info["deploymentStyle"]["deploymentOption"].as_str(),
            Some(deployment_option),
            "case {case:?}: expected deploymentOption {deployment_option}, got {info:?}"
        );
        assert_eq!(
            info["outdatedInstancesStrategy"].as_str(),
            Some(strategy),
            "case {case:?}: expected outdatedInstancesStrategy {strategy}, got {info:?}"
        );
        assert_eq!(
            info["deploymentGroupId"], created["deploymentGroupId"],
            "case {case:?}: an update leaves the group's identity alone, got {info:?}"
        );
    }
}

/// RULE: an `onPremisesInstanceTagFilters` replacement an update names is what
/// the group holds afterwards, so the read reports the NEW selector — whatever
/// the shape of the replacement (a value changed, a filter type changed, the
/// list grown, the list shortened, the list cleared).
#[tokio::test]
async fn an_on_premises_filter_replacement_is_what_the_group_reports() {
    let emulator = Emulator::new();
    let application = "onprem-filters";
    let group = format!("{application}-dg");
    group_info(
        &emulator,
        application,
        "Server",
        &json!({"onPremisesInstanceTagFilters": [
            {"Key": "Stage", "Value": "filter-a", "Type": "KEY_AND_VALUE"}
        ]}),
    )
    .await;

    let cases: &[(&str, Value)] = &[
        (
            "a value replaced",
            json!([{"Key": "Stage", "Value": "filter-b", "Type": "KEY_AND_VALUE"}]),
        ),
        (
            "a filter type replaced",
            json!([{"Key": "Stage", "Type": "KEY_ONLY"}]),
        ),
        (
            "the list grown to two filters",
            json!([
                {"Key": "Stage", "Value": "filter-c", "Type": "KEY_AND_VALUE"},
                {"Value": "filter-d", "Type": "VALUE_ONLY"},
            ]),
        ),
        (
            "the list shortened to one filter",
            json!([{"Key": "Owner", "Value": "team", "Type": "KEY_AND_VALUE"}]),
        ),
        ("the list cleared", json!([])),
    ];

    for (case, filters) in cases {
        emulator
            .ok(
                "UpdateDeploymentGroup",
                json!({
                    "applicationName": application,
                    "currentDeploymentGroupName": group,
                    "onPremisesInstanceTagFilters": filters,
                }),
            )
            .await;
        let info = read_group(&emulator, application, &group).await;
        assert_eq!(
            info["onPremisesInstanceTagFilters"], *filters,
            "case {case:?}: expected the group to report {filters:?}, got {info:?}"
        );
    }
}
