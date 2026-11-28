// SPDX-License-Identifier: CC0-1.0

//! The deployment configuration a deployment group created WITHOUT one runs by,
//! driven over the wire.
//!
//! RULE under test: `deploymentConfigName` is optional on a creating request and a
//! group that names none still runs under one — "It is used if a configuration
//! isn't specified for the deployment or deployment group"
//! (`smithy_query(query="doc",shape="CreateDeploymentGroupInput",member="deploymentConfigName")`)
//! — so the service stores the configuration its compute platform and its
//! `deploymentStyle` default to: a group whose style has the service move the
//! production traffic between the two environments (blue/green WITH traffic
//! control) defaults to its platform's time-based canary where the platform has
//! one, every other group to its platform's single cutover. The stored value is
//! what `GetDeploymentGroup` and `BatchGetDeploymentGroups` both report, and it
//! names a configuration the service holds.
//!
//! Every case goes through the same front door a client uses — the shared
//! [`Emulator`] harness of `codedeploy_lifecycle`.

use serde_json::{json, Value};

use crate::codedeploy_lifecycle::Emulator;

/// The service role every group here is created with.
const ROLE: &str = "arn:aws:iam::123456789012:role/CodeDeployRole";

/// A `deploymentStyle` naming both members.
fn style(deployment_type: &str, deployment_option: &str) -> Value {
    json!({"deploymentType": deployment_type, "deploymentOption": deployment_option})
}

/// Create an application on `platform` and one deployment group on it carrying
/// `extra`, answering the group's name.
async fn given_group(
    emulator: &Emulator,
    application: &str,
    platform: &str,
    extra: &Value,
) -> String {
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
    group
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

/// One row: the case, the application's compute platform, the members the create
/// request adds beside the required ones, and the configuration the group must run
/// by.
type Row<'a> = (&'a str, &'a str, Value, &'a str);

/// RULE: a group created with no `deploymentConfigName` runs by the configuration
/// its compute platform and its deployment style default to, and every view of the
/// group reports that one value.
///
/// Rows differ along what the rule generalises over: the compute platform of the
/// application (`smithy_query(query="shape",shape="ComputePlatform")`: Server |
/// Lambda | ECS) crossed with which deployment the group's style describes — each
/// pair of `DeploymentType` (IN_PLACE | BLUE_GREEN) and `DeploymentOption`
/// (WITH_TRAFFIC_CONTROL | WITHOUT_TRAFFIC_CONTROL) the platform admits, a
/// half-stated style and no style at all.
#[tokio::test]
async fn a_group_that_names_no_configuration_runs_by_its_platform_and_style_default() {
    let emulator = Emulator::new();
    let rows: Vec<Row<'_>> = vec![
        (
            "a Server group naming no style",
            "Server",
            json!({}),
            "CodeDeployDefault.OneAtATime",
        ),
        (
            // The EC2/on-premises platform ships no traffic-shifting
            // configuration, so its traffic-controlled group defaults to the
            // configuration the model documents for the platform.
            "a Server group that is blue/green with traffic control",
            "Server",
            json!({"deploymentStyle": style("BLUE_GREEN", "WITH_TRAFFIC_CONTROL")}),
            "CodeDeployDefault.OneAtATime",
        ),
        (
            "a Lambda group naming no style",
            "Lambda",
            json!({}),
            "CodeDeployDefault.LambdaAllAtOnce",
        ),
        (
            "a Lambda group that is in-place with traffic control",
            "Lambda",
            json!({"deploymentStyle": style("IN_PLACE", "WITH_TRAFFIC_CONTROL")}),
            "CodeDeployDefault.LambdaAllAtOnce",
        ),
        (
            "a Lambda group that is in-place without traffic control",
            "Lambda",
            json!({"deploymentStyle": style("IN_PLACE", "WITHOUT_TRAFFIC_CONTROL")}),
            "CodeDeployDefault.LambdaAllAtOnce",
        ),
        (
            // Only the type is named, so the service's own default completes the
            // style — and the group it completes to moves no traffic itself.
            "a Lambda group naming only the in-place type",
            "Lambda",
            json!({"deploymentStyle": {"deploymentType": "IN_PLACE"}}),
            "CodeDeployDefault.LambdaAllAtOnce",
        ),
        (
            "a Lambda group that is blue/green with traffic control",
            "Lambda",
            json!({"deploymentStyle": style("BLUE_GREEN", "WITH_TRAFFIC_CONTROL")}),
            "CodeDeployDefault.LambdaCanary10Percent5Minutes",
        ),
        (
            // The Amazon ECS platform admits only the blue/green,
            // traffic-controlled style, so this row names it and the load
            // balancer that style routes traffic behind.
            "an ECS group, which is always blue/green with traffic control",
            "ECS",
            crate::codedeploy_lifecycle::ecs_group_members(),
            "CodeDeployDefault.ECSAllAtOnce",
        ),
    ];

    for (index, (case, platform, extra, expected)) in rows.iter().enumerate() {
        let application = format!("default-config-{index:02}");
        let group = given_group(&emulator, &application, platform, extra).await;

        let info = read_group(&emulator, &application, &group).await;
        assert_eq!(
            info["deploymentConfigName"].as_str(),
            Some(*expected),
            "case {case:?}: expected deploymentConfigName {expected:?}, got {info:?}"
        );

        // The defaulted name is a configuration the service HOLDS, so the group
        // does not reference one no read resolves.
        let config = emulator
            .ok(
                "GetDeploymentConfig",
                json!({"deploymentConfigName": expected}),
            )
            .await["deploymentConfigInfo"]
            .clone();
        assert_eq!(
            config["deploymentConfigName"].as_str(),
            Some(*expected),
            "case {case:?}: expected the defaulted configuration to resolve, got {config:?}"
        );

        // The batch read of the same name answers exactly one record, and the
        // same record the single read answers.
        let batch = emulator
            .ok(
                "BatchGetDeploymentGroups",
                json!({
                    "applicationName": application,
                    "deploymentGroupNames": [group.clone()],
                }),
            )
            .await;
        let records = batch["deploymentGroupsInfo"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            records.len(),
            1,
            "case {case:?}: expected exactly one record from BatchGetDeploymentGroups, got \
             {batch:?}"
        );
        assert_eq!(
            records[0], info,
            "case {case:?}: expected the batch record to equal the single read {info:?}, got {:?}",
            records[0]
        );

        // The group is listed under the application it was created on.
        let listed = emulator
            .ok(
                "ListDeploymentGroups",
                json!({"applicationName": application}),
            )
            .await;
        assert_eq!(
            listed["applicationName"].as_str(),
            Some(application.as_str()),
            "case {case:?}: expected ListDeploymentGroups to name the application, got {listed:?}"
        );
        assert_eq!(
            listed["deploymentGroups"],
            json!([group]),
            "case {case:?}: expected the group to be listed, got {listed:?}"
        );
    }
}

/// RULE: the batch reader answers one record per name the application holds a
/// group under, in the order the request named them, and is refused for the
/// request as a whole — for the application it names, for naming no group at all
/// — rather than for a name the application holds nothing under.
///
/// Rows differ along the shape of the batch: one name, several in a different
/// order from the one they were created in, a repeated name, and a name no group
/// was created under.
#[tokio::test]
async fn the_batch_reader_answers_the_groups_the_application_holds_in_request_order() {
    let emulator = Emulator::new();
    let application = "batched-groups";
    emulator
        .ok(
            "CreateApplication",
            json!({"applicationName": application, "computePlatform": "Lambda"}),
        )
        .await;
    let created = ["dg-alpha", "dg-beta", "dg-gamma"];
    for group in created {
        emulator
            .ok(
                "CreateDeploymentGroup",
                json!({
                    "applicationName": application,
                    "deploymentGroupName": group,
                    "serviceRoleArn": ROLE,
                }),
            )
            .await;
    }

    // (case, the names the batch asks for, the names it must answer with)
    let rows: &[(&str, &[&str], &[&str])] = &[
        ("one name", &["dg-beta"], &["dg-beta"]),
        (
            "every name, in a different order from the creates",
            &["dg-gamma", "dg-alpha", "dg-beta"],
            &["dg-gamma", "dg-alpha", "dg-beta"],
        ),
        (
            "a repeated name, answered at each position",
            &["dg-alpha", "dg-alpha"],
            &["dg-alpha", "dg-alpha"],
        ),
        (
            "a name no group was created under, which this operation models no \
             does-not-exist error for",
            &["dg-alpha", "dg-missing"],
            &["dg-alpha"],
        ),
        ("only unknown names", &["dg-missing"], &[]),
    ];

    for (case, asked, expected) in rows {
        let answered = emulator
            .ok(
                "BatchGetDeploymentGroups",
                json!({"applicationName": application, "deploymentGroupNames": asked}),
            )
            .await;
        let names: Vec<String> = answered["deploymentGroupsInfo"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|record| {
                record["deploymentGroupName"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        assert_eq!(
            names, *expected,
            "case {case:?}: expected the records {expected:?}, got {answered:?}"
        );
        // Each record is the one the single reader answers, so the two views of
        // one group cannot disagree.
        for record in answered["deploymentGroupsInfo"]
            .as_array()
            .cloned()
            .unwrap_or_default()
        {
            let name = record["deploymentGroupName"].as_str().unwrap_or_default();
            let single = read_group(&emulator, application, name).await;
            assert_eq!(
                record, single,
                "case {case:?}: expected the batch record of {name:?} to equal the single read \
                 {single:?}, got {record:?}"
            );
        }
    }

    // The refusals of the request AS A WHOLE: the application it names, and a
    // batch that names no deployment group at all.
    let (status, body) = emulator
        .call(
            "BatchGetDeploymentGroups",
            json!({"applicationName": "no-such-app", "deploymentGroupNames": ["dg-alpha"]}),
        )
        .await;
    assert_eq!(
        (status, crate::codedeploy_lifecycle::error_code(&body).as_str()),
        (400, "ApplicationDoesNotExistException"),
        "case \"an application this account does not hold\": expected the application's own \
         refusal, got {status} {body:?}"
    );
    let (status, body) = emulator
        .call(
            "BatchGetDeploymentGroups",
            json!({"applicationName": application, "deploymentGroupNames": []}),
        )
        .await;
    assert_eq!(
        (status, crate::codedeploy_lifecycle::error_code(&body).as_str()),
        (400, "DeploymentGroupNameRequiredException"),
        "case \"a batch naming no deployment group\": expected the missing-name refusal, got \
         {status} {body:?}"
    );
}

/// RULE: the default is only reached when the request names NO configuration — a
/// request that names one runs by the one it named, whatever its style would have
/// defaulted to.
///
/// Rows span the Lambda configurations a client picks for the very style whose
/// default this emulator now resolves to a canary: the all-at-once cutover, a
/// different canary interval, a linear shift and a configuration the client
/// created.
#[tokio::test]
async fn a_named_configuration_is_what_a_traffic_controlled_group_runs_by() {
    let emulator = Emulator::new();
    // A configuration of the client's own, so a named value that is not one of the
    // predefined ones is covered too.
    emulator
        .ok(
            "CreateDeploymentConfig",
            json!({
                "deploymentConfigName": "orders-canary",
                "computePlatform": "Lambda",
                "trafficRoutingConfig": {
                    "type": "TimeBasedCanary",
                    "timeBasedCanary": {"canaryPercentage": 25, "canaryInterval": 2},
                },
            }),
        )
        .await;

    let named = [
        "CodeDeployDefault.LambdaAllAtOnce",
        "CodeDeployDefault.LambdaCanary10Percent30Minutes",
        "CodeDeployDefault.LambdaLinear10PercentEvery1Minute",
        "orders-canary",
    ];

    for (index, config) in named.iter().enumerate() {
        let application = format!("named-config-{index:02}");
        let mut extra = json!({"deploymentConfigName": config});
        extra["deploymentStyle"] = style("BLUE_GREEN", "WITH_TRAFFIC_CONTROL");
        let group = given_group(&emulator, &application, "Lambda", &extra).await;
        let info = read_group(&emulator, &application, &group).await;
        assert_eq!(
            info["deploymentConfigName"].as_str(),
            Some(*config),
            "case {config:?}: expected the named configuration, got {info:?}"
        );
    }
}
