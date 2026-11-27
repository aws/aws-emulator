// SPDX-License-Identifier: CC0-1.0

//! The pairing an ECS service's `loadBalancers` entry has to satisfy: the ELBv2 target
//! group it names must carry the target type the service's task-definition revision
//! registers its tasks as.
//!
//! The rule under test, over the whole class rather than one request:
//!
//! > A `loadBalancers` entry of `CreateService` or `UpdateService` names an Elastic Load
//! > Balancing v2 target group that must EXIST and whose `TargetType` must be the one the
//! > task definition's `networkMode` registers tasks as — `ip` for `awsvpc`, whose tasks
//! > each get their own elastic network interface, and `instance` for every other mode,
//! > whose tasks are reached at the container instance that runs them — and any other
//! > pairing is refused `400 InvalidParameterException` with nothing persisted.
//!
//! `LoadBalancer$targetGroupArn`'s own doc is the `awsvpc` half: "If your service's task
//! definition uses the `awsvpc` network mode, you must choose `ip` as the target type, not
//! `instance`. Do this when creating your target groups because tasks that use the
//! `awsvpc` network mode are associated with an elastic network interface, not an Amazon
//! EC2 instance."
//!
//! The target type is not a value the request carries: it lives in the ELBv2 service's own
//! store. So every row here creates its target group through the ELBv2 API
//! (`CreateTargetGroup` at `Version=2015-12-01`, awsQuery) on the SAME engine the ECS
//! service runs on, and the ECS call is made through the ECS front door — a cross-service
//! READ, pinned end to end rather than at the state struct.
//!
//! The `TargetType` catalog each row draws from is that service's `TargetTypeEnum`
//! (`smithy_query(query="shape", shape="TargetTypeEnum",
//! model_path="services/service-com-amazonaws-elasticloadbalancingv2-elasticloadbalancing-v10/smithy-model/com-amazonaws-elasticloadbalancingv2-elasticloadbalancing-v10.json")`:
//! `instance`, `ip`, `lambda`, `alb`), and the network modes are the ECS model's
//! `NetworkMode` enum (`bridge`, `host`, `awsvpc`, `none`).
//!
//! The pairing is decided by the REVISION's network mode alone, so it is independent of
//! how the service's tasks are PLACED: the last suite here crosses it with the placement
//! the request asks for (a `launchType`, or a `capacityProviderStrategy`), where the
//! tables above name no placement member at all.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ecs_wire::{list, ok, refusal, string, SIGNED_REGION};
use crate::task_lifecycle::container;

/// The ELBv2 API version every awsQuery request below carries, from that model's service
/// version.
const ELB_VERSION: &str = "2015-12-01";

/// The VPC every target group below is created in.
///
/// `pub(crate)` for the same reason the helpers are: a sibling suite that creates its own
/// target group through the ELBv2 API puts it in the SAME VPC as the load balancers this
/// harness builds.
pub(crate) const VPC: &str = "vpc-0abc1234ecs0lb";

/// A router carrying the ECS service AND the Elastic Load Balancing v2 service over ONE
/// shared engine — the two stores this rule reads across, mounted the way the production
/// router mounts them.
///
/// `pub(crate)` because every suite in this one test binary whose service binds a target
/// group needs BOTH stores: a binding naming a group ELBv2 does not hold is refused, so a
/// suite that fabricates an ARN would be asserting a refusal it did not mean.
pub(crate) fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    // The services of this suite are placed in `awsvpc` subnets, which a placement is
    // resolved against ([`crate::ecs_wire::seed_awsvpc_subnets`]).
    crate::ecs_wire::seed_awsvpc_subnets(&engine);
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_ecs_amazonec2containerservicev20141113::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_elasticloadbalancingv2_elasticloadbalancing_v10::EmulatedService::new(
            engine,
        ),
    );
    router
}

/// Percent-encode one awsQuery form component. Escaping every non-alphanumeric byte is
/// always a valid form encoding.
fn encode(value: &str) -> String {
    let mut out = String::new();
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' => out.push(char::from(*byte)),
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// The text of the first element named `tag` in an awsQuery answer.
///
/// `pub(crate)`: shared with the sibling suites that read an ARN out of an ELBv2 answer.
pub(crate) fn first_text(xml: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = xml.find(&open)? + open.len();
    let end = xml[start..].find(&close)? + start;
    Some(xml[start..end].to_string())
}

/// The text of EVERY element named `tag` in an awsQuery answer, in document order.
///
/// The repeated-element counterpart of [`first_text`], for a sibling suite reading one leaf
/// of each member of a LIST (every `Id`, every `State`, every `Port` of a
/// `DescribeTargetHealth` answer). The responses are the framework's own awsQuery serializer
/// output — elements, text, and an `xmlns` attribute on the response root and on each
/// `<member>` — so a leaf element is read by its own tag and nothing here has to understand
/// the list wrapper.
///
/// `pub(crate)` for the reason [`first_text`] is: it lives beside the ELBv2 call it reads the
/// answer of, so the suites of this binary share ONE reader instead of each keeping its own,
/// which is how two of them would come to disagree about what they are asserting.
pub(crate) fn all_texts(xml: &str, tag: &str) -> Vec<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let mut found = Vec::new();
    let mut cursor = 0usize;
    while let Some(at) = xml[cursor..].find(&open) {
        let start = cursor + at + open.len();
        let Some(end) = xml[start..].find(&close) else {
            break;
        };
        found.push(xml[start..start + end].to_string());
        cursor = start + end + close.len();
    }
    found
}

/// Each `<member>` block of an awsQuery list, in document order — the blocks a
/// `DescribeTargetHealth` answer's target health descriptions come in.
///
/// The serializer writes the first member of a list with the service's namespace attribute
/// (`<member xmlns="…">`) and the rest bare, so a block starts past its own tag's closing
/// bracket.
pub(crate) fn member_blocks(xml: &str) -> Vec<&str> {
    xml.split("<member")
        .skip(1)
        .filter_map(|block| block.split_once('>'))
        .filter_map(|(_, rest)| rest.split("</member>").next())
        .collect()
}

/// The `TargetHealth` block of one target health description — the element a reported
/// health's own `State`, `Reason` and `Description` are read out of.
///
/// Scoped for the reason the readers above are shared: a description also carries an
/// `AdministrativeOverride`, whose members are named `State`, `Reason` and `Description`
/// too, so a read that is not scoped to the health's own block takes the override's values
/// for a `healthy` target — whose health carries neither a reason nor a description.
pub(crate) fn target_health_of(description: &str) -> &str {
    description
        .split_once("<TargetHealth")
        .and_then(|(_, rest)| rest.split_once('>'))
        .and_then(|(_, inner)| inner.split_once("</TargetHealth>"))
        .map(|(inner, _)| inner)
        .unwrap_or_default()
}

/// One `tag` of each described target's own `TargetHealth`, in the order the answer reports
/// the descriptions — empty for a description whose health carries no such member.
///
/// The health counterpart of [`all_texts`]: one entry per DESCRIPTION, so it pairs with the
/// leaves read off the descriptions themselves ([`target_health_of`]).
pub(crate) fn health_texts(xml: &str, tag: &str) -> Vec<String> {
    member_blocks(xml)
        .into_iter()
        .map(|description| first_text(target_health_of(description), tag).unwrap_or_default())
        .collect()
}

/// One ELBv2 awsQuery call through the shared router, returning the status and the body.
///
/// `pub(crate)`: shared with the sibling suites that drive the Elastic Load Balancing v2
/// side of a cross-service rule through calls this module has no fixture for.
pub(crate) async fn elb_call(
    router: &MultiServiceRouter,
    action: &str,
    params: &[(&str, &str)],
) -> (u16, String) {
    let mut form = vec![
        format!("Action={}", encode(action)),
        format!("Version={}", encode(ELB_VERSION)),
    ];
    for (key, value) in params {
        form.push(format!("{}={}", encode(key), encode(value)));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("https://elasticloadbalancingv2.us-west-2.amazonaws.com/")
        .header("content-type", "application/x-www-form-urlencoded")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDELBTEST/20260101/{SIGNED_REGION}\
                 /elasticloadbalancingv2/aws4_request, SignedHeaders=host, Signature=00"
            ),
        )
        .body(bytes::Bytes::from(form.join("&")))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let body = String::from_utf8_lossy(response.body()).to_string();
    (status, body)
}

/// Create one target group of `target_type` through the ELBv2 API, give it a load balancer
/// that forwards to it, and return its ARN, as that service minted it.
///
/// The listener is part of the fixture because a target group no load balancer forwards to
/// cannot be bound to a service AT ALL — `CreateService` refuses it, whatever its target
/// type — so a group a row means to be USABLE has to have one. That refusal is pinned by
/// [`create_unattached_target_group`]'s callers.
///
/// A `lambda` or `alb` target group gets no listener: no row binds one to a service
/// successfully (ECS registers neither a Lambda function nor a load balancer as a task
/// target), so the target-type refusal is what those rows are about.
///
/// `pub(crate)`: shared with the other suites in this binary that bind a service to a
/// target group.
pub(crate) async fn create_target_group(
    router: &MultiServiceRouter,
    name: &str,
    target_type: &str,
    case: &str,
) -> String {
    let arn = create_unattached_target_group(router, name, target_type, case).await;
    if target_type == "instance" || target_type == "ip" {
        attach_load_balancer(router, name, &arn, case).await;
    }
    arn
}

/// Create one target group of `target_type` through the ELBv2 API and return its ARN,
/// with NO load balancer forwarding to it — which `DescribeTargetGroups` is asserted to
/// confirm, since it is the premise of the orphan-target-group refusal.
pub(crate) async fn create_unattached_target_group(
    router: &MultiServiceRouter,
    name: &str,
    target_type: &str,
    case: &str,
) -> String {
    // A `lambda` target group takes no protocol, port or VPC; every other type does, and the
    // protocol has to be one that target type is reached over. An `alb` target is a whole
    // Application Load Balancer, which the load balancer in front of it passes TCP
    // connections to; a host target (`instance`, `ip`) takes the HTTP protocol these rows
    // use. Which target types an ECS service may bind is what the rows below are about, and
    // that is unchanged by the protocol its group speaks.
    let mut params: Vec<(&str, &str)> = vec![("Name", name), ("TargetType", target_type)];
    if target_type != "lambda" {
        let protocol = if target_type == "alb" { "TCP" } else { "HTTP" };
        params.extend([("Protocol", protocol), ("Port", "80"), ("VpcId", VPC)]);
    }
    let (status, body) = elb_call(router, "CreateTargetGroup", &params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateTargetGroup for a {target_type} target group must succeed, \
         got {status} with {body}"
    );
    let arn = first_text(&body, "TargetGroupArn")
        .unwrap_or_else(|| panic!("case {case:?}: no TargetGroupArn in {body}"));
    // The read back through the ELBv2 model confirms the stored target type the ECS check
    // will read — the row's own premise — and that nothing forwards to the group yet.
    let (status, described) =
        elb_call(router, "DescribeTargetGroups", &[("Names.member.1", name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeTargetGroups must read it back, got {status} with {described}"
    );
    assert_eq!(
        first_text(&described, "TargetType").as_deref(),
        Some(target_type),
        "case {case:?}: expected the stored target type {target_type}, got {described}"
    );
    assert!(
        first_text(&described, "LoadBalancerArns")
            .unwrap_or_default()
            .is_empty(),
        "case {case:?}: a group with no listener forwarding to it has no load balancer, \
         got {described}"
    );
    arn
}

/// Create a load balancer and a listener that forwards to `target_group`, so the group is
/// one a service may bind to, and assert ELBv2 then reports the association.
pub(crate) async fn attach_load_balancer(
    router: &MultiServiceRouter,
    name: &str,
    target_group: &str,
    case: &str,
) {
    let load_balancer_name = format!("{name}-alb");
    let (status, body) = elb_call(
        router,
        "CreateLoadBalancer",
        &[
            ("Name", &load_balancer_name),
            ("Subnets.member.1", "subnet-0abc1234ecs0lb0"),
            ("Subnets.member.2", "subnet-0abc1234ecs0lb1"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateLoadBalancer must succeed, got {status} with {body}"
    );
    let load_balancer = first_text(&body, "LoadBalancerArn")
        .unwrap_or_else(|| panic!("case {case:?}: no LoadBalancerArn in {body}"));
    let (status, body) = elb_call(
        router,
        "CreateListener",
        &[
            ("LoadBalancerArn", &load_balancer),
            ("Protocol", "HTTP"),
            ("Port", "80"),
            ("DefaultActions.member.1.Type", "forward"),
            ("DefaultActions.member.1.TargetGroupArn", target_group),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateListener must succeed, got {status} with {body}"
    );
    let (status, described) = elb_call(
        router,
        "DescribeTargetGroups",
        &[("TargetGroupArns.member.1", target_group)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeTargetGroups must read the group back, got {status} with \
         {described}"
    );
    assert!(
        first_text(&described, "LoadBalancerArns")
            .unwrap_or_default()
            .contains(&load_balancer),
        "case {case:?}: ELBv2 must report the association the listener made, got \
         {described}"
    );
}

/// Register one revision of `family` in `network_mode` and return its ARN. A revision in
/// the `none` network mode gets a container with no port mappings, which is what that mode
/// allows ("If the network mode is set to none, you cannot specify port mappings").
///
/// `pub(crate)`: the sibling suite that pins the WORDING of the pairing refusal needs a
/// revision in each network mode, which is exactly this.
pub(crate) async fn register_definition(
    router: &MultiServiceRouter,
    family: &str,
    network_mode: &str,
    case: &str,
) -> String {
    let mut web = container("web", "public.ecr.aws/nginx/nginx:latest");
    if network_mode != "none" {
        web["portMappings"] = json!([{ "containerPort": 80, "protocol": "tcp" }]);
    }
    let body = ok(
        router,
        "RegisterTaskDefinition",
        json!({
            "family": family,
            "networkMode": network_mode,
            "containerDefinitions": [web],
        }),
        case,
    )
    .await;
    string(&body["taskDefinition"], "taskDefinitionArn", case)
}

/// The `CreateService` request body of a service in `cluster` attached to `target_group`.
///
/// `pub(crate)` for the reason [`register_definition`] gives: the wording suite drives the
/// same requests through the same front door.
pub(crate) fn create_body(
    cluster: &str,
    service_name: &str,
    task_definition: &str,
    network_mode: &str,
    load_balancers: Value,
) -> Value {
    let mut body = json!({
        "cluster": cluster,
        "serviceName": service_name,
        "taskDefinition": task_definition,
        "desiredCount": 1,
        "loadBalancers": load_balancers,
    });
    if network_mode == "awsvpc" {
        // "This parameter is required for task definitions that use the awsvpc network
        // mode."
        body["networkConfiguration"] = json!({
            "awsvpcConfiguration": { "subnets": ["subnet-0abc1234ecs0lb"] },
        });
    }
    body
}

/// One `loadBalancers` entry naming `target_group`.
///
/// `pub(crate)` for the reason [`register_definition`] gives.
pub(crate) fn attachment(target_group: &str) -> Value {
    json!([{
        "targetGroupArn": target_group,
        "containerName": "web",
        "containerPort": 80,
    }])
}

/// The `loadBalancers` of the service `cluster`/`service_name` holds, read back through
/// `DescribeServices`.
async fn stored_load_balancers(
    router: &MultiServiceRouter,
    cluster: &str,
    service_name: &str,
    case: &str,
) -> Vec<Value> {
    let body = ok(
        router,
        "DescribeServices",
        json!({ "cluster": cluster, "services": [service_name] }),
        case,
    )
    .await;
    let services = list(&body, "services", case);
    assert_eq!(
        services.len(),
        1,
        "case {case:?}: expected exactly one described service, got {body}"
    );
    list(&services[0], "loadBalancers", case)
}

/// Whether `DescribeServices` reports the named service at all.
///
/// `pub(crate)` for the reason [`register_definition`] gives: a refused create must leave
/// nothing behind, which the wording suite checks too.
pub(crate) async fn service_exists(
    router: &MultiServiceRouter,
    cluster: &str,
    service_name: &str,
    case: &str,
) -> bool {
    let body = ok(
        router,
        "DescribeServices",
        json!({ "cluster": cluster, "services": [service_name] }),
        case,
    )
    .await;
    !list(&body, "services", case).is_empty()
}

/// One row of the pairing table: a network mode, a target type, and whether a service
/// pairing the two is created or refused.
struct Row {
    case: &'static str,
    network_mode: &'static str,
    target_type: &'static str,
    created: bool,
}

#[tokio::test]
async fn create_service_pairs_a_target_group_with_the_task_definitions_network_mode() {
    let router = emulator();
    let cluster = "lb-pairing";
    ok(
        &router,
        "CreateCluster",
        json!({ "clusterName": cluster }),
        "the cluster every row creates its service in",
    )
    .await;

    // The rows span the two halves of the rule — both directions of the `awsvpc`/`ip`
    // pairing and of the instance-registered modes — plus the target types ECS registers
    // nothing as (`lambda` and `alb` carry a Lambda function and a load balancer, not a
    // task). The `none` mode appears only as a refusal: a load-balanced service in that
    // mode also needs a port mapping it cannot have, which is a different rule.
    let rows: [Row; 10] = [
        Row {
            case: "awsvpc with an ip target group",
            network_mode: "awsvpc",
            target_type: "ip",
            created: true,
        },
        Row {
            // The issue's own case.
            case: "awsvpc with an instance target group",
            network_mode: "awsvpc",
            target_type: "instance",
            created: false,
        },
        Row {
            case: "awsvpc with a lambda target group",
            network_mode: "awsvpc",
            target_type: "lambda",
            created: false,
        },
        Row {
            case: "awsvpc with an alb target group",
            network_mode: "awsvpc",
            target_type: "alb",
            created: false,
        },
        Row {
            case: "bridge with an instance target group",
            network_mode: "bridge",
            target_type: "instance",
            created: true,
        },
        Row {
            case: "bridge with an ip target group",
            network_mode: "bridge",
            target_type: "ip",
            created: false,
        },
        Row {
            case: "host with an instance target group",
            network_mode: "host",
            target_type: "instance",
            created: true,
        },
        Row {
            case: "host with an ip target group",
            network_mode: "host",
            target_type: "ip",
            created: false,
        },
        Row {
            case: "host with an alb target group",
            network_mode: "host",
            target_type: "alb",
            created: false,
        },
        Row {
            case: "none with an ip target group",
            network_mode: "none",
            target_type: "ip",
            created: false,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let group_name = format!("pair-tg-{index}");
        let family = format!("pair-family-{index}");
        let service_name = format!("pair-svc-{index}");
        let target_group = create_target_group(&router, &group_name, row.target_type, case).await;
        let definition = register_definition(&router, &family, row.network_mode, case).await;
        let body = create_body(
            cluster,
            &service_name,
            &definition,
            row.network_mode,
            attachment(&target_group),
        );

        if row.created {
            ok(&router, "CreateService", body, case).await;
            let stored = stored_load_balancers(&router, cluster, &service_name, case).await;
            assert_eq!(
                stored.len(),
                1,
                "case {case:?}: expected the stored service to hold its one attachment, \
                 got {stored:?}"
            );
            let stored_arn = string(&stored[0], "targetGroupArn", case);
            assert_eq!(
                stored_arn, target_group,
                "case {case:?}: expected the stored attachment to name {target_group}, got \
                 {stored_arn}"
            );
        } else {
            let (code, message) = refusal(&router, "CreateService", body, case).await;
            assert_eq!(
                code, "InvalidParameterException",
                "case {case:?}: expected the modelled InvalidParameterException, got {code} \
                 with {message:?}"
            );
            for expected in [row.target_type, row.network_mode] {
                assert!(
                    message.contains(expected),
                    "case {case:?}: expected the refusal to name {expected:?}, got {message:?}"
                );
            }
            assert!(
                !service_exists(&router, cluster, &service_name, case).await,
                "case {case:?}: the refused service must not be persisted, but \
                 DescribeServices reports {service_name}"
            );
        }
    }
}

#[tokio::test]
async fn create_service_refuses_a_target_group_no_load_balancer_holds() {
    let router = emulator();
    let cluster = "lb-unknown";
    ok(
        &router,
        "CreateCluster",
        json!({ "clusterName": cluster }),
        "the cluster",
    )
    .await;

    // An ARN of the right shape for every field a target-group ARN has, naming a target
    // group the ELBv2 store does not hold — and one that names a target group only after
    // it is created, so the refusal is about the RESOURCE and not about the text.
    let real = create_target_group(&router, "unknown-real-tg", "ip", "the real group").await;
    let definition = register_definition(&router, "unknown-family", "awsvpc", "the revision").await;
    let cases: [(&str, String); 2] = [
        (
            "a target group ARN nothing created",
            real.replace("unknown-real-tg", "never-created-tg"),
        ),
        (
            "a target group ARN with an id no mint produced",
            format!("{real}deadbeef"),
        ),
    ];

    for (index, (case, arn)) in cases.iter().enumerate() {
        let service_name = format!("unknown-svc-{index}");
        let (code, message) = refusal(
            &router,
            "CreateService",
            create_body(
                cluster,
                &service_name,
                &definition,
                "awsvpc",
                attachment(arn),
            ),
            case,
        )
        .await;
        assert_eq!(
            code, "InvalidParameterException",
            "case {case:?}: expected the modelled InvalidParameterException, got {code} with \
             {message:?}"
        );
        assert!(
            message.contains(arn),
            "case {case:?}: expected the refusal to name {arn:?}, got {message:?}"
        );
        assert!(
            !service_exists(&router, cluster, &service_name, case).await,
            "case {case:?}: the refused service must not be persisted, but DescribeServices \
             reports {service_name}"
        );
    }

    // The same request against the target group that DOES exist is created: the refusals
    // above are the resource's absence, not the attachment itself.
    ok(
        &router,
        "CreateService",
        create_body(
            cluster,
            "unknown-svc-ok",
            &definition,
            "awsvpc",
            attachment(&real),
        ),
        "the attachment that names a stored target group",
    )
    .await;
}

#[tokio::test]
async fn an_alternate_target_group_is_held_to_the_same_pairing() {
    let router = emulator();
    let cluster = "lb-alternate";
    ok(
        &router,
        "CreateCluster",
        json!({ "clusterName": cluster }),
        "the cluster",
    )
    .await;

    // `AdvancedConfiguration$alternateTargetGroupArn` is the second target group a
    // blue/green deployment registers the SAME tasks into, so the pairing rule covers it.
    let primary = create_target_group(&router, "alt-primary-tg", "ip", "the primary group").await;
    let good = create_target_group(&router, "alt-ip-tg", "ip", "the alternate ip group").await;
    let bad =
        create_target_group(&router, "alt-instance-tg", "instance", "the instance group").await;
    let definition = register_definition(&router, "alt-family", "awsvpc", "the revision").await;

    let entry = |alternate: &str| {
        json!([{
            "targetGroupArn": primary.clone(),
            "containerName": "web",
            "containerPort": 80,
            "advancedConfiguration": { "alternateTargetGroupArn": alternate },
        }])
    };

    let case = "an awsvpc service whose alternate target group is instance-typed";
    let (code, message) = refusal(
        &router,
        "CreateService",
        create_body(cluster, "alt-svc-bad", &definition, "awsvpc", entry(&bad)),
        case,
    )
    .await;
    assert_eq!(
        code, "InvalidParameterException",
        "case {case:?}: expected the modelled InvalidParameterException, got {code} with \
         {message:?}"
    );
    assert!(
        message.contains(&bad) && message.contains("instance"),
        "case {case:?}: expected the refusal to name the alternate group and its target \
         type, got {message:?}"
    );

    let case = "an awsvpc service whose alternate target group is ip-typed";
    ok(
        &router,
        "CreateService",
        create_body(cluster, "alt-svc-good", &definition, "awsvpc", entry(&good)),
        case,
    )
    .await;
    let stored = stored_load_balancers(&router, cluster, "alt-svc-good", case).await;
    let alternate = string(
        &stored[0]["advancedConfiguration"],
        "alternateTargetGroupArn",
        case,
    );
    assert_eq!(
        alternate, good,
        "case {case:?}: expected the stored alternate target group {good}, got {alternate}"
    );
}

#[tokio::test]
async fn the_number_of_attachments_one_request_may_name_is_bounded() {
    let router = emulator();
    let cluster = "lb-quota";
    ok(
        &router,
        "CreateCluster",
        json!({ "clusterName": cluster }),
        "the cluster",
    )
    .await;

    let definition = register_definition(&router, "quota-family", "awsvpc", "the revision").await;
    // `state::MAX_LOAD_BALANCERS` is the Amazon ECS quota for target groups per service.
    // The rows are the bound itself and one past it: a list AT the bound is created, one
    // PAST it is refused before any of its entries is looked up.
    let cap = service_com_amazonaws_ecs_amazonec2containerservicev20141113::state::MAX_LOAD_BALANCERS;
    let mut groups = Vec::new();
    for index in 0..cap {
        groups.push(
            create_target_group(
                &router,
                &format!("quota-tg-{index}"),
                "ip",
                "the groups at the bound",
            )
            .await,
        );
    }
    let entries = |count: usize| -> Value {
        Value::Array(
            (0..count)
                .map(|index| {
                    json!({
                        "targetGroupArn": groups[index % groups.len()].clone(),
                        "containerName": "web",
                        "containerPort": 80,
                    })
                })
                .collect(),
        )
    };

    let case = "a service with as many attachments as the quota allows";
    ok(
        &router,
        "CreateService",
        create_body(cluster, "quota-svc-ok", &definition, "awsvpc", entries(cap)),
        case,
    )
    .await;
    let stored = stored_load_balancers(&router, cluster, "quota-svc-ok", case).await;
    assert_eq!(
        stored.len(),
        cap,
        "case {case:?}: expected the stored service to hold {cap} attachments, got \
         {stored:?}"
    );

    let case = "a service with one attachment more than the quota allows";
    let (code, message) = refusal(
        &router,
        "CreateService",
        create_body(
            cluster,
            "quota-svc-over",
            &definition,
            "awsvpc",
            entries(cap + 1),
        ),
        case,
    )
    .await;
    assert_eq!(
        code, "ClientException",
        "case {case:?}: expected the modelled ClientException, got {code} with {message:?}"
    );
    assert!(
        message.contains(&cap.to_string()),
        "case {case:?}: expected the refusal to name the bound {cap}, got {message:?}"
    );
    assert!(
        !service_exists(&router, cluster, "quota-svc-over", case).await,
        "case {case:?}: the refused service must not be persisted"
    );

    // The same bound on the update path.
    let case = "an update that names one attachment more than the quota allows";
    let (code, message) = refusal(
        &router,
        "UpdateService",
        json!({
            "cluster": cluster,
            "service": "quota-svc-ok",
            "loadBalancers": entries(cap + 1),
        }),
        case,
    )
    .await;
    assert_eq!(
        code, "ClientException",
        "case {case:?}: expected the modelled ClientException, got {code} with {message:?}"
    );
    let still = stored_load_balancers(&router, cluster, "quota-svc-ok", case).await;
    assert_eq!(
        still.len(),
        cap,
        "case {case:?}: expected the refused update to leave {cap} attachments, got \
         {still:?}"
    );
}

#[tokio::test]
async fn update_service_holds_the_new_pairing_to_the_same_rule() {
    let router = emulator();
    let cluster = "lb-update";
    ok(
        &router,
        "CreateCluster",
        json!({ "clusterName": cluster }),
        "the cluster",
    )
    .await;

    let instance_group =
        create_target_group(&router, "update-instance-tg", "instance", "the instance group").await;
    let ip_group = create_target_group(&router, "update-ip-tg", "ip", "the ip group").await;
    let bridge = register_definition(&router, "update-bridge", "bridge", "the bridge revision")
        .await;
    let awsvpc = register_definition(&router, "update-awsvpc", "awsvpc", "the awsvpc revision")
        .await;

    // A bridge service on the instance target group: the pairing the rule allows.
    let service_name = "update-svc";
    ok(
        &router,
        "CreateService",
        create_body(
            cluster,
            service_name,
            &bridge,
            "bridge",
            attachment(&instance_group),
        ),
        "the service the updates below act on",
    )
    .await;

    // Each row is one UPDATE of that service: what it names, and whether it is refused.
    let rows: [(&str, Value, bool); 4] = [
        (
            "moving a bridge service onto an awsvpc revision, keeping its instance target \
             group",
            json!({ "taskDefinition": awsvpc.clone() }),
            false,
        ),
        (
            "attaching an ip target group to a service that still runs a bridge revision",
            json!({ "loadBalancers": attachment(&ip_group) }),
            false,
        ),
        (
            "attaching a target group nothing created",
            json!({ "loadBalancers": attachment(&format!("{ip_group}-gone")) }),
            false,
        ),
        (
            "moving onto the awsvpc revision AND its ip target group together",
            json!({
                "taskDefinition": awsvpc.clone(),
                "loadBalancers": attachment(&ip_group),
                "networkConfiguration": {
                    "awsvpcConfiguration": { "subnets": ["subnet-0abc1234ecs0lb"] },
                },
            }),
            true,
        ),
    ];

    for (case, mut request, accepted) in rows {
        request["cluster"] = json!(cluster);
        request["service"] = json!(service_name);
        if accepted {
            ok(&router, "UpdateService", request, case).await;
        } else {
            let (code, message) = refusal(&router, "UpdateService", request, case).await;
            assert_eq!(
                code, "InvalidParameterException",
                "case {case:?}: expected the modelled InvalidParameterException, got {code} \
                 with {message:?}"
            );
            // Nothing is half-applied: the service still runs the revision and holds the
            // attachment it had.
            let body = ok(
                &router,
                "DescribeServices",
                json!({ "cluster": cluster, "services": [service_name] }),
                case,
            )
            .await;
            let described = list(&body, "services", case);
            let running = string(&described[0], "taskDefinition", case);
            assert_eq!(
                running, bridge,
                "case {case:?}: expected the refused update to leave the service on \
                 {bridge}, got {running}"
            );
            let stored = list(&described[0], "loadBalancers", case);
            let stored_arn = string(&stored[0], "targetGroupArn", case);
            assert_eq!(
                stored_arn, instance_group,
                "case {case:?}: expected the refused update to leave the attachment on \
                 {instance_group}, got {stored_arn}"
            );
        }
    }

    // The accepted update is what the service reports afterwards.
    let case = "the service after the accepted update";
    let stored = stored_load_balancers(&router, cluster, service_name, case).await;
    let stored_arn = string(&stored[0], "targetGroupArn", case);
    assert_eq!(
        stored_arn, ip_group,
        "case {case:?}: expected the stored attachment to name {ip_group}, got {stored_arn}"
    );
}

/// The task-level `cpu` and `memory` a Fargate-compatible revision declares (each member's
/// own doc: "This field is required for Fargate"), at a legal Fargate size.
const FARGATE_CPU: &str = "256";
const FARGATE_MEMORY: &str = "512";

/// How a request may ask for a service's tasks to be PLACED — the dimension the pairing
/// above must be independent of.
///
/// `CreateService$launchType` (`LaunchType`: `EC2 | EXTERNAL | FARGATE |
/// MANAGED_INSTANCES`) and `CreateService$capacityProviderStrategy` are the two members
/// that carry it; `MANAGED_INSTANCES` is reachable only through the latter ("If you want to
/// use Amazon ECS Managed Instances, you must use the capacityProviderStrategy request
/// parameter and omit the launchType request parameter"), and naming both at once is
/// refused outright ("If a capacityProviderStrategy is specified, the launchType parameter
/// must be omitted"), so a row names exactly one. No row names `MANAGED_INSTANCES`: its
/// capacity provider is created by `CreateCapacityProvider` with a `managedInstancesProvider`,
/// which this slice does not model yet, and the pairing a row would check is the one the
/// `awsvpc` rows below already cross.
#[derive(Debug)]
enum Placement {
    /// A `launchType` the request names outright.
    LaunchType(&'static str),
    /// A `capacityProviderStrategy` naming one capacity provider of the cluster.
    CapacityProvider(&'static str),
}

impl Placement {
    /// Add the member this placement is asked for by to a `CreateService` body.
    fn apply(&self, body: &mut Value) {
        match self {
            Self::LaunchType(launch_type) => body["launchType"] = json!(launch_type),
            Self::CapacityProvider(provider) => {
                body["capacityProviderStrategy"] =
                    json!([{ "capacityProvider": provider, "weight": 1 }]);
            }
        }
    }
}

/// Register one `awsvpc` revision of `family` that every placement below may legally run:
/// Fargate requires `awsvpc` plus task-level `cpu` and `memory`, and the revision declares
/// the launch types it is validated against ("Amazon ECS validates the task definition
/// parameters with those supported by the launch type"), so the SAME revision is placed on
/// Fargate, on EC2 and through a Fargate capacity provider.
async fn register_fargate_definition(
    router: &MultiServiceRouter,
    family: &str,
    case: &str,
) -> String {
    let mut web = container("web", "public.ecr.aws/nginx/nginx:latest");
    web["portMappings"] = json!([{ "containerPort": 80, "protocol": "tcp" }]);
    let body = ok(
        router,
        "RegisterTaskDefinition",
        json!({
            "family": family,
            "networkMode": "awsvpc",
            "requiresCompatibilities": ["EC2", "FARGATE"],
            "cpu": FARGATE_CPU,
            "memory": FARGATE_MEMORY,
            "containerDefinitions": [web],
        }),
        case,
    )
    .await;
    string(&body["taskDefinition"], "taskDefinitionArn", case)
}

/// One row of the placement table: how the tasks are placed, the revision's network mode,
/// the target group's type, and whether the service is created.
struct PlacementRow {
    case: &'static str,
    placement: Placement,
    network_mode: &'static str,
    target_type: &'static str,
    created: bool,
}

#[tokio::test]
async fn the_pairing_holds_whatever_placement_the_request_asks_for() {
    let router = emulator();
    let cluster = "lb-placement";
    // The Fargate capacity providers are ASSOCIATED with the cluster, since a
    // `capacityProviderStrategy` may only name a provider the cluster holds. The cluster
    // gets no `defaultCapacityProviderStrategy`: every row names its own placement.
    ok(
        &router,
        "CreateCluster",
        json!({ "clusterName": cluster, "capacityProviders": ["FARGATE", "FARGATE_SPOT"] }),
        "the cluster every row creates its service in",
    )
    .await;

    // The rows cross the placement dimension with both halves of the pairing: one
    // `awsvpc` revision placed four ways against an `ip` and an `instance` group, and a
    // `bridge` revision placed on the launch types its tasks can take, where the required
    // type is the other one. The table above covers the request that names NO placement.
    let rows: [PlacementRow; 9] = [
        PlacementRow {
            // The pairing of the recorded request this suite's rule was filed from.
            case: "FARGATE launch type, awsvpc revision, instance target group",
            placement: Placement::LaunchType("FARGATE"),
            network_mode: "awsvpc",
            target_type: "instance",
            created: false,
        },
        PlacementRow {
            case: "FARGATE launch type, awsvpc revision, ip target group",
            placement: Placement::LaunchType("FARGATE"),
            network_mode: "awsvpc",
            target_type: "ip",
            created: true,
        },
        PlacementRow {
            case: "EC2 launch type, awsvpc revision, instance target group",
            placement: Placement::LaunchType("EC2"),
            network_mode: "awsvpc",
            target_type: "instance",
            created: false,
        },
        PlacementRow {
            case: "EC2 launch type, awsvpc revision, ip target group",
            placement: Placement::LaunchType("EC2"),
            network_mode: "awsvpc",
            target_type: "ip",
            created: true,
        },
        PlacementRow {
            case: "a FARGATE capacity-provider strategy, awsvpc revision, instance target \
                   group",
            placement: Placement::CapacityProvider("FARGATE"),
            network_mode: "awsvpc",
            target_type: "instance",
            created: false,
        },
        PlacementRow {
            case: "a FARGATE capacity-provider strategy, awsvpc revision, ip target group",
            placement: Placement::CapacityProvider("FARGATE"),
            network_mode: "awsvpc",
            target_type: "ip",
            created: true,
        },
        PlacementRow {
            case: "a FARGATE_SPOT capacity-provider strategy, awsvpc revision, instance \
                   target group",
            placement: Placement::CapacityProvider("FARGATE_SPOT"),
            network_mode: "awsvpc",
            target_type: "instance",
            created: false,
        },
        PlacementRow {
            case: "EXTERNAL launch type, bridge revision, ip target group",
            placement: Placement::LaunchType("EXTERNAL"),
            network_mode: "bridge",
            target_type: "ip",
            created: false,
        },
        PlacementRow {
            case: "EXTERNAL launch type, bridge revision, instance target group",
            placement: Placement::LaunchType("EXTERNAL"),
            network_mode: "bridge",
            target_type: "instance",
            created: true,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let group_name = format!("place-tg-{index}");
        let target_group =
            create_target_group(&router, &group_name, row.target_type, case).await;
        let family = format!("place-family-{index}");
        let definition = match row.network_mode {
            "awsvpc" => register_fargate_definition(&router, &family, case).await,
            mode => register_definition(&router, &family, mode, case).await,
        };
        let service_name = format!("place-svc-{index}");
        let mut body = create_body(
            cluster,
            &service_name,
            &definition,
            row.network_mode,
            attachment(&target_group),
        );
        row.placement.apply(&mut body);

        if row.created {
            let created = ok(&router, "CreateService", body, case).await;
            let stored = stored_load_balancers(&router, cluster, &service_name, case).await;
            assert_eq!(
                stored.len(),
                1,
                "case {case:?}: expected the stored service to hold its one attachment, \
                 got {stored:?}"
            );
            let stored_arn = string(&stored[0], "targetGroupArn", case);
            assert_eq!(
                stored_arn, target_group,
                "case {case:?}: expected the stored attachment to name {target_group}, got \
                 {stored_arn}"
            );
            // The service runs on the placement the row asked for, so the accept is this
            // row's placement and not another one.
            let service = &created["service"];
            match row.placement {
                Placement::LaunchType(launch_type) => {
                    let reported = string(service, "launchType", case);
                    assert_eq!(
                        reported, launch_type,
                        "case {case:?}: expected the service to report launchType \
                         {launch_type}, got {reported}"
                    );
                }
                Placement::CapacityProvider(provider) => {
                    let strategy = list(service, "capacityProviderStrategy", case);
                    let reported = strategy
                        .first()
                        .map(|entry| string(entry, "capacityProvider", case));
                    assert_eq!(
                        reported.as_deref(),
                        Some(provider),
                        "case {case:?}: expected the service to report the strategy on \
                         {provider}, got {strategy:?}"
                    );
                }
            }
        } else {
            let (code, message) = refusal(&router, "CreateService", body, case).await;
            assert_eq!(
                code, "InvalidParameterException",
                "case {case:?}: expected the modelled InvalidParameterException, got {code} \
                 with {message:?}"
            );
            // The refusal is the PAIRING one — it names the group, the target type it has
            // and the network mode it cannot carry — and not one about the placement, which
            // is legal in every row here.
            for expected in [target_group.as_str(), row.target_type, row.network_mode] {
                assert!(
                    message.contains(expected),
                    "case {case:?}: expected the refusal to name {expected:?}, got {message:?}"
                );
            }
            assert!(
                !service_exists(&router, cluster, &service_name, case).await,
                "case {case:?}: the refused service must not be persisted, but \
                 DescribeServices reports {service_name}"
            );
        }
    }
}
