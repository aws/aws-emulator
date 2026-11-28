// SPDX-License-Identifier: CC0-1.0

//! The LOAD BALANCER an Amazon ECS CodeDeploy deployment group has to be wired
//! with, driven over the assembled `MultiServiceRouter` with CodeDeploy and Amazon
//! ECS mounted on one engine.
//!
//! RULE: a deployment group on the Amazon ECS compute platform whose deployment
//! style routes traffic behind a load balancer moves production traffic between
//! task sets over the route its `loadBalancerInfo` names, so a request that would
//! create or leave such a group naming NO load balancer at all is refused HTTP 400
//! `InvalidLoadBalancerInfoException` — "For ECS deployment group,
//! loadBalancerInfo must be specified", fixed text — and nothing is stored for it.
//! The condition is judged before the group's `ecsServices` are resolved against
//! Amazon ECS, so a group that names no load balancer is refused for the load
//! balancer whether the cluster/service pair it targets exists or not. A group that
//! routes no traffic behind a load balancer, and a group on any other compute
//! platform, is unaffected.
//!
//! Two crates have to agree for the ordering rows to hold — CodeDeploy judges the
//! precondition, Amazon ECS owns the cluster and the service a group's `ecsServices`
//! names — which is why this suite lives here. It also lives here rather than beside
//! the CodeDeploy crate's own suites because that crate still roots its integration
//! tests at a hand-declared `tests/all.rs`, which a fix may not edit; the router's
//! `tests/all/` picks a new suite up with nothing to declare.
//!
//! The rows differ along what the rule generalises over: the application's compute
//! platform, which deployment style the request names (both members, one member,
//! none), the form the load balancer is named in (Classic Load Balancer, target
//! group, the Amazon ECS target-group pair, an all-empty configuration, the member
//! omitted), whether the group is being created or updated, and whether the Amazon
//! ECS pair the group targets exists.
//!
//! The Amazon ECS rows all run the blue/green, traffic-controlled style, because
//! that is the only style an application on that compute platform admits at all —
//! a separate precondition, with its own error and its own suite
//! (`services/service-com-amazonaws-codedeploy-codedeploy-20141006/tests/all/deployment_style_precondition.rs`),
//! judged ahead of this one — so the style variations of this table are carried by
//! its rows on the other platforms.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The Region every call here is signed for.
const REGION: &str = "us-east-1";

/// The modelled error an Amazon ECS group with no load balancer earns.
const INVALID_LOAD_BALANCER_INFO: &str = "InvalidLoadBalancerInfoException";

/// The modelled error a group whose `ecsServices` pair could not be loaded earns —
/// the refusal the load balancer precondition is ordered ahead of.
const INVALID_ECS_SERVICE: &str = "InvalidECSServiceException";

/// CodeDeploy's sentence for an Amazon ECS deployment group that names no load
/// balancer: entirely fixed text, no part of it taken from the request or from
/// state.
const LOAD_BALANCER_INFO_REQUIRED: &str =
    "For ECS deployment group, loadBalancerInfo must be specified";

/// The service role every deployment group here is created with.
const SERVICE_ROLE_ARN: &str = "arn:aws:iam::123456789012:role/CodeDeployServiceRole";

/// The emulator with the two services this suite exercises, mounted on ONE engine
/// exactly as `core/router/src/lib.rs` mounts them.
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_codedeploy_codedeploy_20141006::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_ecs_amazonec2containerservicev20141113::EmulatedService::new(engine),
    );
    router
}

/// A SigV4 `Authorization` header whose credential scope names `service` in
/// [`REGION`], which is what routes the request. The signature is never checked:
/// this router runs with request authentication off.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host;x-amz-target, Signature=00"
    )
}

/// Call one operation of an awsJson1_1 service over its own front door.
async fn json_call(
    router: &MultiServiceRouter,
    signing_name: &str,
    target_prefix: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("{target_prefix}.{operation}"))
        .header("authorization", authorization(signing_name))
        .body(Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let answered: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, answered)
}

/// Call one CodeDeploy operation.
async fn codedeploy(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    json_call(router, "codedeploy", "CodeDeploy_20141006", operation, body).await
}

/// Call one CodeDeploy operation, requiring HTTP 200.
async fn codedeploy_ok(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let (status, answered) = codedeploy(router, operation, body).await;
    assert_eq!(
        status, 200,
        "setup: {operation} must succeed, got {status} with {answered}"
    );
    answered
}

/// Call one Amazon ECS operation, requiring HTTP 200.
async fn ecs_ok(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let (status, answered) = json_call(
        router,
        "ecs",
        "AmazonEC2ContainerServiceV20141113",
        operation,
        body,
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: ECS {operation} must succeed, got {status} with {answered}"
    );
    answered
}

/// The wire error code of a failure body: awsJson carries it in `__type`, either
/// bare or namespaced.
fn error_code(body: &Value) -> String {
    body["__type"]
        .as_str()
        .unwrap_or("<no __type>")
        .rsplit('#')
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

/// The deployment group names `ListDeploymentGroups` reports for `application`.
async fn listed_groups(router: &MultiServiceRouter, application: &str) -> Vec<String> {
    let listed = codedeploy_ok(
        router,
        "ListDeploymentGroups",
        &json!({"applicationName": application}),
    )
    .await;
    listed["deploymentGroups"]
        .as_array()
        .map(|names| {
            names
                .iter()
                .map(|name| name.as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// The `deploymentGroupInfo` `GetDeploymentGroup` reports for `group`.
async fn group_info(router: &MultiServiceRouter, application: &str, group: &str) -> Value {
    codedeploy_ok(
        router,
        "GetDeploymentGroup",
        &json!({"applicationName": application, "deploymentGroupName": group}),
    )
    .await["deploymentGroupInfo"]
        .clone()
}

/// A `loadBalancerInfo` naming one Classic Load Balancer, the form
/// `elbInfoList` carries ("If you're using Classic Load Balancers, specify those
/// load balancers in this array").
fn classic_load_balancer() -> Value {
    json!({"elbInfoList": [{"name": "classic-elb"}]})
}

/// A `loadBalancerInfo` naming one target group, the form `targetGroupInfoList`
/// carries.
fn target_group() -> Value {
    json!({"targetGroupInfoList": [{"name": "tg-blue"}]})
}

/// A `loadBalancerInfo` naming the pair an Amazon ECS deployment moves production
/// traffic between, the form `targetGroupPairInfoList` carries.
fn target_group_pair() -> Value {
    json!({"targetGroupPairInfoList": [{
        "targetGroups": [{"name": "tg-blue"}, {"name": "tg-green"}],
        "prodTrafficRoute": {"listenerArns": [
            "arn:aws:elasticloadbalancing:us-east-1:123456789012:listener/app/lb/0/1"
        ]},
    }]})
}

/// The style a request names with both members stated.
fn style(deployment_type: &str, deployment_option: &str) -> Value {
    json!({"deploymentType": deployment_type, "deploymentOption": deployment_option})
}

/// One row of the create table.
struct Row {
    case: &'static str,
    /// The `computePlatform` of the application the group is created under.
    platform: &'static str,
    /// The `deploymentStyle` the request names, absent when `None`.
    style: Option<Value>,
    /// The `loadBalancerInfo` the request names, absent when `None`.
    load_balancer_info: Option<Value>,
    /// Whether the create is refused for the load balancer it names none of.
    refused: bool,
}

/// RULE: an Amazon ECS deployment group whose style routes traffic behind a load
/// balancer is created only when it names a load balancer; it is refused HTTP 400
/// `InvalidLoadBalancerInfoException` with the fixed sentence otherwise, and
/// nothing is stored. Every other platform/style combination is served and reads
/// back.
#[tokio::test]
async fn an_ecs_group_that_routes_traffic_is_created_only_with_load_balancer_info() {
    let rows: Vec<Row> = vec![
        Row {
            case: "an ECS blue/green group with traffic control and no loadBalancerInfo",
            platform: "ECS",
            style: Some(style("BLUE_GREEN", "WITH_TRAFFIC_CONTROL")),
            load_balancer_info: None,
            refused: true,
        },
        Row {
            case: "an ECS group with traffic control and an empty loadBalancerInfo",
            platform: "ECS",
            style: Some(style("BLUE_GREEN", "WITH_TRAFFIC_CONTROL")),
            load_balancer_info: Some(json!({})),
            refused: true,
        },
        Row {
            case: "an ECS group with traffic control whose every list is empty",
            platform: "ECS",
            style: Some(style("BLUE_GREEN", "WITH_TRAFFIC_CONTROL")),
            load_balancer_info: Some(json!({
                "elbInfoList": [],
                "targetGroupInfoList": [],
                "targetGroupPairInfoList": [],
            })),
            refused: true,
        },
        Row {
            case: "an ECS group with traffic control naming the target group pair",
            platform: "ECS",
            style: Some(style("BLUE_GREEN", "WITH_TRAFFIC_CONTROL")),
            load_balancer_info: Some(target_group_pair()),
            refused: false,
        },
        Row {
            case: "an ECS group with traffic control naming a target group",
            platform: "ECS",
            style: Some(style("BLUE_GREEN", "WITH_TRAFFIC_CONTROL")),
            load_balancer_info: Some(target_group()),
            refused: false,
        },
        Row {
            case: "an ECS group with traffic control naming a Classic Load Balancer",
            platform: "ECS",
            style: Some(style("BLUE_GREEN", "WITH_TRAFFIC_CONTROL")),
            load_balancer_info: Some(classic_load_balancer()),
            refused: false,
        },
        Row {
            // Routing no traffic behind a load balancer is what this row is about;
            // the type is IN_PLACE because blue/green WITHOUT traffic control is a
            // combination CodeDeploy does not support at all, which is the sibling
            // precondition on the style itself.
            case: "a Server group that routes no traffic behind a load balancer",
            platform: "Server",
            style: Some(style("IN_PLACE", "WITHOUT_TRAFFIC_CONTROL")),
            load_balancer_info: None,
            refused: false,
        },
        Row {
            case: "a Server group naming no deploymentStyle at all",
            platform: "Server",
            style: None,
            load_balancer_info: None,
            refused: false,
        },
        Row {
            case: "a Server group naming only the traffic option, so the type defaults",
            platform: "Server",
            style: Some(json!({"deploymentOption": "WITH_TRAFFIC_CONTROL"})),
            load_balancer_info: None,
            refused: false,
        },
        Row {
            case: "a Server group with traffic control and no loadBalancerInfo",
            platform: "Server",
            style: Some(style("BLUE_GREEN", "WITH_TRAFFIC_CONTROL")),
            load_balancer_info: None,
            refused: false,
        },
        Row {
            case: "a Lambda group with traffic control and no loadBalancerInfo",
            platform: "Lambda",
            style: Some(style("BLUE_GREEN", "WITH_TRAFFIC_CONTROL")),
            load_balancer_info: None,
            refused: false,
        },
    ];

    let router = build_router();
    for (index, row) in rows.iter().enumerate() {
        let application = format!("lb-info-{index:02}");
        let group = format!("{application}-dg");
        codedeploy_ok(
            &router,
            "CreateApplication",
            &json!({"applicationName": application, "computePlatform": row.platform}),
        )
        .await;
        let mut request = json!({
            "applicationName": application,
            "deploymentGroupName": group,
            "serviceRoleArn": SERVICE_ROLE_ARN,
        });
        if let Some(style) = row.style.as_ref() {
            request["deploymentStyle"] = style.clone();
        }
        if let Some(info) = row.load_balancer_info.as_ref() {
            request["loadBalancerInfo"] = info.clone();
        }

        let (status, body) = codedeploy(&router, "CreateDeploymentGroup", &request).await;
        if row.refused {
            assert_eq!(
                (status, error_code(&body).as_str(), body["message"].as_str()),
                (400, INVALID_LOAD_BALANCER_INFO, Some(LOAD_BALANCER_INFO_REQUIRED)),
                "case {:?}: expected HTTP 400 {INVALID_LOAD_BALANCER_INFO} with \
                 {LOAD_BALANCER_INFO_REQUIRED:?}, got {status} with {body}",
                row.case
            );
            // Nothing is stored for a refused create: the application holds no
            // group of that name.
            let listed = listed_groups(&router, &application).await;
            assert!(
                listed.is_empty(),
                "case {:?}: expected the refused group not to be stored, got {listed:?}",
                row.case
            );
            continue;
        }
        assert_eq!(
            status, 200,
            "case {:?}: expected the group to be created, got {status} with {body}",
            row.case
        );
        // Read the group back through its own Get: the load balancer the request
        // named is what the stored group carries.
        let info = group_info(&router, &application, &group).await;
        // Every served row names either no load balancer at all or one with an
        // entry in it, so what the request carried is what the group must report.
        let expected = row.load_balancer_info.clone().unwrap_or(Value::Null);
        assert_eq!(
            info["loadBalancerInfo"], expected,
            "case {:?}: expected the stored loadBalancerInfo {expected}, got {info}",
            row.case
        );
        let listed = listed_groups(&router, &application).await;
        assert_eq!(
            listed,
            vec![group.clone()],
            "case {:?}: expected the application to hold the created group, got {listed:?}",
            row.case
        );
    }
}

/// RULE applied to an UPDATE: the configuration the group would END UP with is what
/// the precondition is judged on, so an update that would leave an Amazon ECS group
/// routing traffic with no load balancer at all is refused and changes nothing,
/// while one that names a load balancer with it is served — and an update of a group
/// that already carries one needs to name nothing.
///
/// A group on this compute platform always routes its traffic behind a load
/// balancer, because the style it runs by is fixed by the platform's own
/// precondition, so what an update can take away here is the load balancer itself.
#[tokio::test]
async fn an_update_may_not_leave_an_ecs_group_routing_traffic_with_no_load_balancer() {
    let router = build_router();
    const APPLICATION: &str = "lb-info-update";
    const GROUP: &str = "lb-info-update-dg";
    codedeploy_ok(
        &router,
        "CreateApplication",
        &json!({"applicationName": APPLICATION, "computePlatform": "ECS"}),
    )
    .await;
    // A group wired with the target-group pair its deployments move traffic
    // between.
    codedeploy_ok(
        &router,
        "CreateDeploymentGroup",
        &json!({
            "applicationName": APPLICATION,
            "deploymentGroupName": GROUP,
            "serviceRoleArn": SERVICE_ROLE_ARN,
            "deploymentStyle": style("BLUE_GREEN", "WITH_TRAFFIC_CONTROL"),
            "loadBalancerInfo": target_group_pair(),
        }),
    )
    .await;

    // Clearing the load balancer is refused: the group would describe a traffic
    // move it could never perform.
    let case = "an all-empty loadBalancerInfo, clearing the one it carries";
    let (status, body) = codedeploy(
        &router,
        "UpdateDeploymentGroup",
        &json!({
            "applicationName": APPLICATION,
            "currentDeploymentGroupName": GROUP,
            "loadBalancerInfo": {},
        }),
    )
    .await;
    assert_eq!(
        (status, error_code(&body).as_str(), body["message"].as_str()),
        (400, INVALID_LOAD_BALANCER_INFO, Some(LOAD_BALANCER_INFO_REQUIRED)),
        "case {case:?}: expected HTTP 400 {INVALID_LOAD_BALANCER_INFO} with \
         {LOAD_BALANCER_INFO_REQUIRED:?}, got {status} with {body}"
    );
    let info = group_info(&router, APPLICATION, GROUP).await;
    assert_eq!(
        (
            info["deploymentStyle"]["deploymentOption"].as_str(),
            &info["loadBalancerInfo"],
        ),
        (Some("WITH_TRAFFIC_CONTROL"), &target_group_pair()),
        "case {case:?}: expected the refused update to change nothing, got {info}"
    );

    // Naming the style again, WITH a load balancer, is served.
    let case = "the style and the load balancer together";
    codedeploy_ok(
        &router,
        "UpdateDeploymentGroup",
        &json!({
            "applicationName": APPLICATION,
            "currentDeploymentGroupName": GROUP,
            "deploymentStyle": style("BLUE_GREEN", "WITH_TRAFFIC_CONTROL"),
            "loadBalancerInfo": target_group_pair(),
        }),
    )
    .await;
    let info = group_info(&router, APPLICATION, GROUP).await;
    assert_eq!(
        (
            info["deploymentStyle"]["deploymentOption"].as_str(),
            &info["loadBalancerInfo"],
        ),
        (Some("WITH_TRAFFIC_CONTROL"), &target_group_pair()),
        "case {case:?}: expected the group to route traffic through the named pair, got {info}"
    );

    // And an update of that group needs to name neither member again: the stored
    // load balancer is the one the condition is satisfied by.
    let case = "an unrelated update of a group that already carries one";
    codedeploy_ok(
        &router,
        "UpdateDeploymentGroup",
        &json!({
            "applicationName": APPLICATION,
            "currentDeploymentGroupName": GROUP,
            "outdatedInstancesStrategy": "IGNORE",
        }),
    )
    .await;
    let info = group_info(&router, APPLICATION, GROUP).await;
    assert_eq!(
        (
            info["outdatedInstancesStrategy"].as_str(),
            &info["loadBalancerInfo"],
        ),
        (Some("IGNORE"), &target_group_pair()),
        "case {case:?}: expected the stored load balancer to be kept, got {info}"
    );
}

/// One row of the ordering table below: the Amazon ECS pair the group targets, the
/// load balancer it names, and the refusal it earns.
struct OrderRow {
    case: &'static str,
    /// The group's own name, so each row's record is its own.
    group: &'static str,
    /// The `ecsServices` the request names.
    ecs_services: Value,
    /// The `loadBalancerInfo` the request names, absent when `None`.
    load_balancer_info: Option<Value>,
    /// The error code the create is refused with, `None` for a row it serves.
    refusal: Option<&'static str>,
}

/// Create `cluster` and one service named `service` in it through Amazon ECS's OWN
/// front door, so CodeDeploy's `ecsServices` gate resolves the pair against the
/// records that service wrote.
///
/// The smallest service that API accepts: a registered task definition and a
/// desired count of zero, so nothing is placed and no transition is owed. It names
/// no deployment controller, so Amazon ECS deploys it by its own rolling update —
/// which is a pair that LOADS and then fails CodeDeploy's controller precondition,
/// the second of the two `ecsServices` refusals the table below orders the load
/// balancer ahead of.
async fn given_ecs_service(router: &MultiServiceRouter, cluster: &str, service: &str) {
    ecs_ok(router, "CreateCluster", &json!({"clusterName": cluster})).await;
    let family = format!("{cluster}-{service}");
    ecs_ok(
        router,
        "RegisterTaskDefinition",
        &json!({
            "family": family,
            "containerDefinitions": [
                {"name": "app", "image": "public.ecr.aws/nginx/nginx:latest", "memory": 128}
            ],
        }),
    )
    .await;
    ecs_ok(
        router,
        "CreateService",
        &json!({
            "cluster": cluster,
            "serviceName": service,
            "taskDefinition": family,
            "desiredCount": 0,
        }),
    )
    .await;
}

/// RULE (the ORDER the two refusals are served in): the load balancer precondition
/// is judged before the group's `ecsServices` are resolved against Amazon ECS, so a
/// group naming no load balancer is refused for the load balancer whether the pair
/// it targets exists or not — and the Amazon ECS refusal is what a group that DOES
/// name a load balancer and a pair that cannot be targeted earns.
#[tokio::test]
async fn the_load_balancer_precondition_is_served_ahead_of_the_ecs_services_gate() {
    let router = build_router();
    const APPLICATION: &str = "lb-info-order";
    const CLUSTER: &str = "prod";
    const SERVICE: &str = "web";
    given_ecs_service(&router, CLUSTER, SERVICE).await;
    codedeploy_ok(
        &router,
        "CreateApplication",
        &json!({"applicationName": APPLICATION, "computePlatform": "ECS"}),
    )
    .await;

    let rows: Vec<OrderRow> = vec![
        OrderRow {
            case: "a pair the account holds, no load balancer",
            group: "dg-held-no-lb",
            ecs_services: json!([{"clusterName": CLUSTER, "serviceName": SERVICE}]),
            load_balancer_info: None,
            refusal: Some(INVALID_LOAD_BALANCER_INFO),
        },
        OrderRow {
            case: "a pair the account does not hold, no load balancer",
            group: "dg-ghost-no-lb",
            ecs_services: json!([{"clusterName": "ghost-cluster", "serviceName": "ghost-service"}]),
            load_balancer_info: None,
            refusal: Some(INVALID_LOAD_BALANCER_INFO),
        },
        OrderRow {
            case: "a pair the account does not hold, with a load balancer",
            group: "dg-ghost-with-lb",
            ecs_services: json!([{"clusterName": "ghost-cluster", "serviceName": "ghost-service"}]),
            load_balancer_info: Some(target_group_pair()),
            refusal: Some(INVALID_ECS_SERVICE),
        },
        OrderRow {
            case: "a pair the account holds, with a load balancer",
            group: "dg-held-with-lb",
            ecs_services: json!([{"clusterName": CLUSTER, "serviceName": SERVICE}]),
            load_balancer_info: Some(target_group_pair()),
            // The pair LOADS — Amazon ECS holds that service — and is then refused
            // for the controller it is deployed by, which is the precondition the
            // CodeDeploy crate's own `ecs_service_deployment_controller` suite
            // spans. Either way the refusal is the Amazon ECS one, which is what
            // this table orders the load balancer ahead of.
            refusal: Some(INVALID_ECS_SERVICE),
        },
        OrderRow {
            case: "no Amazon ECS pair at all, with a load balancer",
            group: "dg-no-pair-with-lb",
            ecs_services: json!([]),
            load_balancer_info: Some(target_group_pair()),
            refusal: None,
        },
    ];

    let mut served: Vec<String> = Vec::new();
    for OrderRow {
        case,
        group,
        ecs_services,
        load_balancer_info,
        refusal,
    } in rows
    {
        let mut request = json!({
            "applicationName": APPLICATION,
            "deploymentGroupName": group,
            "serviceRoleArn": SERVICE_ROLE_ARN,
            "deploymentStyle": style("BLUE_GREEN", "WITH_TRAFFIC_CONTROL"),
            "ecsServices": ecs_services,
        });
        if let Some(info) = load_balancer_info {
            request["loadBalancerInfo"] = info;
        }
        let (status, body) = codedeploy(&router, "CreateDeploymentGroup", &request).await;
        match refusal {
            None => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected the group to be created, got {status} with {body}"
                );
                served.push(group.to_string());
            }
            Some(INVALID_LOAD_BALANCER_INFO) => {
                assert_eq!(
                    (status, error_code(&body).as_str(), body["message"].as_str()),
                    (
                        400,
                        INVALID_LOAD_BALANCER_INFO,
                        Some(LOAD_BALANCER_INFO_REQUIRED),
                    ),
                    "case {case:?}: expected HTTP 400 {INVALID_LOAD_BALANCER_INFO} with \
                     {LOAD_BALANCER_INFO_REQUIRED:?}, got {status} with {body}"
                );
            }
            Some(expected_code) => {
                assert_eq!(
                    (status, error_code(&body).as_str()),
                    (400, expected_code),
                    "case {case:?}: expected HTTP 400 {expected_code}, got {status} with {body}"
                );
            }
        }
    }

    let listed = listed_groups(&router, APPLICATION).await;
    assert_eq!(
        listed, served,
        "case \"the groups the application holds\": expected only the served rows' groups, got \
         {listed:?}"
    );
}
