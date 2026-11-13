// SPDX-License-Identifier: CC0-1.0

//! A PRIVATE REST API reached through an `execute-api` interface VPC endpoint, over the
//! whole estate a client provisions for it: EC2's VPC, subnets, security group and
//! endpoint, Elastic Load Balancing's internal network load balancer with the endpoint's
//! network interfaces as its `ip` targets, and API Gateway's VPC link and private api.
//!
//! The rule under test: a resource a create answers in a NON-TERMINAL status reaches its
//! terminal one from the CLOCK alone — no further call by the caller — so every readiness
//! poll in that chain converges and the gate in front of the private data plane opens.
//! It holds for each resource of the chain, each owned by a different service and each
//! polled by a different read: the interface endpoint's `State` (`pending` -> `available`,
//! in the lowercase spelling EC2 puts on the wire), its network interfaces' health as
//! Elastic Load Balancing targets (`initial` -> `healthy`), and the VPC link's `status`
//! (`PENDING` -> `AVAILABLE`). Behind the gate, the api's own resource policy decides the
//! invoke: a request that arrived through the endpoint the policy's `aws:SourceVpce`
//! condition names is served, and one that arrived through any other endpoint is refused
//! `403` with the `execute-api:Invoke` denial and none of the body. WHICH endpoint a request
//! arrived through is decided by the LINK it was carried over — the endpoint whose network
//! interfaces that link's load balancer forwards to — and never by the host the integration's
//! `uri` names, because for a private integration "the URI is not used for routing"
//! (`Integration$uri`): the rows below spell that address both ways and expect one answer.
//!
//! This suite lives here rather than in one service's crate because the chain spans three
//! services on one engine, and the gate is only observable end to end: the endpoint is
//! EC2's, its targets' health is Elastic Load Balancing's, and the link and the api are
//! API Gateway's.
//!
//! Assembling the whole router costs stack (see `boot_serve.rs`), so it is built — and the
//! requests driven — on a sized thread.

use std::sync::Arc;
use std::time::{Duration, Instant};

use aws_emulator_router::{build_router, InMemoryEngine};
use serde_json::Value;
use smithy_server_runtime::MultiServiceRouter;

use crate::elbv2_network_resolution::{ec2, ec2_ok, elbv2, Answer, BOOT_STACK_SIZE, REGION};
use crate::query_wire::{block_of, element, elements};

/// How long a readiness poll here is given before it is called a non-convergence.
///
/// Every transition in this chain is owed one of its service's own transition spans — half
/// a second each — so a second and a half is an ample margin on a loaded machine while
/// still failing fast if a resource never moves at all.
const POLL_TIMEOUT: Duration = Duration::from_millis(1_500);

/// How long a poll waits between reads. Short, because the spans under test are
/// sub-second; nothing here sleeps for a transition, so this only bounds how often the
/// store is asked.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// The one value the private api's `MOCK` integration answers with — what proves a request
/// reached the integration, so a refusal must never carry it.
const NONCE: &str = "b7e21d";

/// Another endpoint id of the same account, in EC2's own id form: the one the policy is
/// re-pinned to so the SAME request becomes a refusal. It names no endpoint this account
/// holds, which is exactly the point — an endpoint the request did not arrive through.
const OTHER_ENDPOINT: &str = "vpce-00000000000000000";

/// The stage the private api is deployed to.
const STAGE: &str = "s";

/// The resource path the private api serves.
const RESOURCE_PATH: &str = "whoami";

/// One API Gateway control-plane call, driven over the service's own restJson1 endpoint.
fn apigw(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    method: &str,
    path: &str,
    body: Option<&str>,
) -> Answer {
    let host = format!("apigateway.{REGION}.amazonaws.com");
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{path}"))
        .header("host", &host)
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/apigateway\
                 /aws4_request, SignedHeaders=host, Signature=deadbeef"
            ),
        )
        .body(bytes::Bytes::from(body.unwrap_or_default().to_string()))
        .expect("a well-formed request");
    let response = runtime.block_on(router.dispatch(request));
    Answer {
        status: response.status().as_u16(),
        text: String::from_utf8_lossy(response.body()).to_string(),
    }
}

/// One API Gateway control-plane call that has to succeed with `expected`, and its body.
fn apigw_ok(
    case: &str,
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    method: &str,
    path: &str,
    body: Option<&str>,
    expected: u16,
) -> String {
    let answer = apigw(router, runtime, method, path, body);
    assert_eq!(
        answer.status, expected,
        "case {case:?}: expected HTTP {expected} from {method} {path}, got {answer:?}"
    );
    answer.text
}

/// One member of a JSON body, or a panic naming the case.
fn json_member(case: &str, body: &str, member: &str) -> String {
    let parsed: Value = serde_json::from_str(body)
        .unwrap_or_else(|error| panic!("case {case:?}: {error} reading {body}"));
    parsed
        .get(member)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: no {member:?} in {body}"))
        .to_string()
}

/// One anonymous invoke of a deployed stage, addressed on `host`, carrying `headers`.
///
/// Anonymous on purpose: the method under test authorizes nobody (`authorizationType`
/// `NONE`), which is the case a private api's resource policy is the only grant for.
fn invoke(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    host: &str,
    path: &str,
    headers: &[(&str, &str)],
) -> Answer {
    let mut builder = http::Request::builder()
        .method("GET")
        .uri(format!("https://{host}{path}"))
        .header("host", host);
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let request = builder
        .body(bytes::Bytes::new())
        .expect("a well-formed request");
    let response = runtime.block_on(router.dispatch(request));
    Answer {
        status: response.status().as_u16(),
        text: String::from_utf8_lossy(response.body()).to_string(),
    }
}

/// Poll `read` until `settled` holds of what it answers, and answer that.
///
/// This is the client-side half of the rule under test: nothing in the chain is nudged
/// between reads, so a value that only ever changes because somebody called again never
/// satisfies the predicate here either.
fn poll_until<R>(case: &str, mut read: impl FnMut() -> R, settled: impl Fn(&R) -> bool) -> R
where
    R: std::fmt::Debug,
{
    let deadline = Instant::now() + POLL_TIMEOUT;
    loop {
        let answer = read();
        if settled(&answer) {
            return answer;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the clock owed this move and it never came; last read {answer:?}"
        );
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// The endpoint-specific DNS name EC2 reports in an interface endpoint's `DnsEntries`,
/// which is how a client addresses a PRIVATE api through one endpoint rather than another.
fn endpoint_host(endpoint: &str) -> String {
    format!("{endpoint}.execute-api.{REGION}.vpce.amazonaws.com")
}

/// The resource policy that admits `execute-api:Invoke` only from `endpoint`.
fn pinned_policy(endpoint: &str) -> String {
    format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"Allow\",\
         \"Principal\":\"*\",\"Action\":\"execute-api:Invoke\",\
         \"Resource\":\"execute-api:/*\",\"Condition\":{{\"StringEquals\":\
         {{\"aws:SourceVpce\":\"{endpoint}\"}}}}}}]}}"
    )
}

/// `UpdateRestApi` re-pinning the api's `/policy` to `endpoint`, then a redeploy of the
/// stage — the two calls a developer makes to change what the endpoint admits.
fn repin_policy(
    case: &str,
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    api: &str,
    endpoint: &str,
) {
    let patch = serde_json::json!({
        "patchOperations": [{
            "op": "replace",
            "path": "/policy",
            "value": pinned_policy(endpoint),
        }]
    })
    .to_string();
    apigw_ok(
        case,
        router,
        runtime,
        "PATCH",
        &format!("/restapis/{api}"),
        Some(&patch),
        200,
    );
    apigw_ok(
        case,
        router,
        runtime,
        "POST",
        &format!("/restapis/{api}/deployments"),
        Some(&serde_json::json!({ "stageName": STAGE }).to_string()),
        201,
    );
}

#[test]
fn a_private_api_is_reached_through_the_endpoint_its_policy_names() {
    std::thread::Builder::new()
        .name("apigateway-private-vpce-invoke".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
            let mut router = build_router(engine);
            router.set_request_authentication(false);
            let runtime = tokio::runtime::Builder::new_current_thread()
                // The IO and TIME drivers a service's background work needs: this suite
                // stands a load balancer up and invokes a function, and both reach work that
                // panics inside tokio when the runtime has neither.
                .enable_all()
                .build()
                .expect("a current-thread runtime");

            // -----------------------------------------------------------------
            // The network the endpoint is created in.
            // -----------------------------------------------------------------
            let vpc = element(
                &ec2_ok(&router, &runtime, "CreateVpc", &[("CidrBlock", "10.70.0.0/16")]),
                "vpcId",
            );
            let subnets: Vec<String> = [("10.70.1.0/24", "us-west-2a"), ("10.70.2.0/24", "us-west-2b")]
                .iter()
                .map(|(cidr, zone)| {
                    element(
                        &ec2_ok(
                            &router,
                            &runtime,
                            "CreateSubnet",
                            &[
                                ("VpcId", vpc.as_str()),
                                ("CidrBlock", cidr),
                                ("AvailabilityZone", zone),
                            ],
                        ),
                        "subnetId",
                    )
                })
                .collect();
            let group = element(
                &ec2_ok(
                    &router,
                    &runtime,
                    "CreateSecurityGroup",
                    &[
                        ("GroupName", "private-vpce-invoke"),
                        ("GroupDescription", "the execute-api endpoint"),
                        ("VpcId", vpc.as_str()),
                    ],
                ),
                "groupId",
            );
            let ingress = ec2(
                &router,
                &runtime,
                "AuthorizeSecurityGroupIngress",
                &[
                    ("GroupId", group.as_str()),
                    ("IpPermissions.1.IpProtocol", "tcp"),
                    ("IpPermissions.1.FromPort", "443"),
                    ("IpPermissions.1.ToPort", "443"),
                    ("IpPermissions.1.IpRanges.1.CidrIp", "10.70.0.0/16"),
                ],
            );
            assert_eq!(
                ingress.status, 200,
                "case \"443 from the VPC's own range\": expected 200, got {ingress:?}"
            );

            // -----------------------------------------------------------------
            // The gate: the interface endpoint reaches `available` on the clock.
            // -----------------------------------------------------------------
            let created = ec2_ok(
                &router,
                &runtime,
                "CreateVpcEndpoint",
                &[
                    ("VpcId", vpc.as_str()),
                    ("ServiceName", &format!("com.amazonaws.{REGION}.execute-api")),
                    ("VpcEndpointType", "Interface"),
                    ("SubnetId.1", subnets[0].as_str()),
                    ("SubnetId.2", subnets[1].as_str()),
                    ("SecurityGroupId.1", group.as_str()),
                    ("PrivateDnsEnabled", "false"),
                ],
            );
            let endpoint = element(&created, "vpcEndpointId");
            assert_eq!(
                element(&created, "state"),
                "pending",
                "case \"the create's own answer\": an interface endpoint is answered in its \
                 non-terminal state, got {created}"
            );

            let describe_endpoint = || {
                let body = ec2_ok(
                    &router,
                    &runtime,
                    "DescribeVpcEndpoints",
                    &[("VpcEndpointId.1", endpoint.as_str())],
                );
                (element(&body, "state"), body)
            };
            let (state, described) = poll_until(
                "the endpoint reaches available with nobody calling in between",
                describe_endpoint,
                |(state, _)| state == "available",
            );
            assert_eq!(
                state, "available",
                "case \"the endpoint's terminal state\": got {described}"
            );

            // The materialised value is the one every reader of the store sees, the
            // `vpc-endpoint-state` filter included.
            let filtered = ec2_ok(
                &router,
                &runtime,
                "DescribeVpcEndpoints",
                &[
                    ("Filter.1.Name", "vpc-endpoint-state"),
                    ("Filter.1.Value.1", "available"),
                ],
            );
            assert!(
                elements(&filtered, "vpcEndpointId").contains(&endpoint),
                "case \"the vpc-endpoint-state filter\": expected {endpoint} to be selected \
                 by its materialised state, got {filtered}"
            );

            // "An endpoint network interface is created … in an Availability Zone"
            // (`SubnetIds`): one per subnet, each with a private address of that subnet.
            let interfaces: Vec<String> = block_of(&described, "networkInterfaceIdSet")
                .map(|set| elements(set, "item"))
                .unwrap_or_default();
            assert_eq!(
                interfaces.len(),
                subnets.len(),
                "case \"one endpoint network interface per subnet\": got {described}"
            );
            let mut addresses: Vec<String> = Vec::new();
            for network_interface in &interfaces {
                let reported = ec2_ok(
                    &router,
                    &runtime,
                    "DescribeNetworkInterfaces",
                    &[("NetworkInterfaceId.1", network_interface.as_str())],
                );
                let address = element(&reported, "privateIpAddress");
                assert!(
                    address.starts_with("10.70."),
                    "case \"the interface's address comes out of its subnet\": got {reported}"
                );
                addresses.push(address);
            }

            // -----------------------------------------------------------------
            // The network load balancer in front of those interfaces.
            // -----------------------------------------------------------------
            let load_balancer = elbv2(
                &router,
                &runtime,
                "CreateLoadBalancer",
                &[
                    ("Name", "private-vpce-nlb"),
                    ("Scheme", "internal"),
                    ("Type", "network"),
                    ("Subnets.member.1", subnets[0].as_str()),
                    ("Subnets.member.2", subnets[1].as_str()),
                ],
            );
            assert_eq!(
                load_balancer.status, 200,
                "case \"an internal network load balancer\": expected 200, got {load_balancer:?}"
            );
            let load_balancer_arn = element(&load_balancer.text, "LoadBalancerArn");

            let target_group = elbv2(
                &router,
                &runtime,
                "CreateTargetGroup",
                &[
                    ("Name", "private-vpce-targets"),
                    ("TargetType", "ip"),
                    ("Protocol", "TCP"),
                    ("Port", "443"),
                    ("HealthCheckProtocol", "TCP"),
                    ("VpcId", vpc.as_str()),
                ],
            );
            assert_eq!(
                target_group.status, 200,
                "case \"an ip target group in the endpoint's VPC\": expected 200, got \
                 {target_group:?}"
            );
            let target_group_arn = element(&target_group.text, "TargetGroupArn");

            let registered = elbv2(
                &router,
                &runtime,
                "RegisterTargets",
                &[
                    ("TargetGroupArn", target_group_arn.as_str()),
                    ("Targets.member.1.Id", addresses[0].as_str()),
                    ("Targets.member.1.Port", "443"),
                    ("Targets.member.2.Id", addresses[1].as_str()),
                    ("Targets.member.2.Port", "443"),
                ],
            );
            assert_eq!(
                registered.status, 200,
                "case \"the endpoint's own addresses as ip targets\": expected 200, got \
                 {registered:?}"
            );
            let listener = elbv2(
                &router,
                &runtime,
                "CreateListener",
                &[
                    ("LoadBalancerArn", load_balancer_arn.as_str()),
                    ("Protocol", "TCP"),
                    ("Port", "443"),
                    ("DefaultActions.member.1.Type", "forward"),
                    (
                        "DefaultActions.member.1.TargetGroupArn",
                        target_group_arn.as_str(),
                    ),
                ],
            );
            assert_eq!(
                listener.status, 200,
                "case \"a TCP listener forwarding to those targets\": expected 200, got \
                 {listener:?}"
            );

            let describe_health = || {
                let health = elbv2(
                    &router,
                    &runtime,
                    "DescribeTargetHealth",
                    &[("TargetGroupArn", target_group_arn.as_str())],
                );
                assert_eq!(
                    health.status, 200,
                    "case \"the targets' health\": expected 200, got {health:?}"
                );
                elements(&health.text, "State")
            };
            let states = poll_until(
                "both endpoint targets become healthy with nobody calling in between",
                describe_health,
                |states| states.iter().all(|state| state == "healthy"),
            );
            assert_eq!(
                states,
                vec!["healthy".to_string(), "healthy".to_string()],
                "case \"the targets' terminal health\": got {states:?}"
            );

            // -----------------------------------------------------------------
            // The VPC link over that load balancer.
            // -----------------------------------------------------------------
            let link_request = serde_json::json!({
                "name": "private-vpce-link",
                "targetArns": [load_balancer_arn],
            })
            .to_string();
            let link = apigw_ok(
                "the VPC link over the network load balancer",
                &router,
                &runtime,
                "POST",
                "/vpclinks",
                Some(&link_request),
                202,
            );
            let link_id = json_member("the VPC link", &link, "id");
            assert_eq!(
                json_member("the VPC link", &link, "status"),
                "PENDING",
                "case \"the create's own answer\": a link is created in its non-terminal \
                 status, got {link}"
            );
            let read_link = || {
                let body = apigw_ok(
                    "the VPC link's status",
                    &router,
                    &runtime,
                    "GET",
                    &format!("/vpclinks/{link_id}"),
                    None,
                    200,
                );
                json_member("the VPC link's status", &body, "status")
            };
            let link_status = poll_until(
                "the VPC link reaches AVAILABLE with nobody calling in between",
                read_link,
                |status| status == "AVAILABLE" || status == "FAILED",
            );
            assert_eq!(
                link_status, "AVAILABLE",
                "case \"the link's terminal status\": a link over a network load balancer \
                 this account holds provisions, got {link_status:?}"
            );

            // -----------------------------------------------------------------
            // The PRIVATE api behind the endpoint, and the three invokes.
            // -----------------------------------------------------------------
            let api_request = serde_json::json!({
                "name": "private-vpce-api",
                "endpointConfiguration": { "types": ["PRIVATE"] },
            })
            .to_string();
            let api_body = apigw_ok(
                "the private api",
                &router,
                &runtime,
                "POST",
                "/restapis",
                Some(&api_request),
                201,
            );
            let api = json_member("the private api", &api_body, "id");
            let root = json_member("the private api", &api_body, "rootResourceId");

            let resource = apigw_ok(
                "the api's whoami resource",
                &router,
                &runtime,
                "POST",
                &format!("/restapis/{api}/resources/{root}"),
                Some(&serde_json::json!({ "pathPart": RESOURCE_PATH }).to_string()),
                201,
            );
            let resource_id = json_member("the api's whoami resource", &resource, "id");
            let method_path =
                format!("/restapis/{api}/resources/{resource_id}/methods/GET");
            apigw_ok(
                "the method",
                &router,
                &runtime,
                "PUT",
                &method_path,
                Some(&serde_json::json!({ "authorizationType": "NONE" }).to_string()),
                201,
            );
            apigw_ok(
                "the MOCK integration",
                &router,
                &runtime,
                "PUT",
                &format!("{method_path}/integration"),
                Some(
                    &serde_json::json!({
                        "type": "MOCK",
                        "requestTemplates": { "application/json": "{\"statusCode\": 200}" },
                    })
                    .to_string(),
                ),
                201,
            );
            apigw_ok(
                "the method response",
                &router,
                &runtime,
                "PUT",
                &format!("{method_path}/responses/200"),
                Some("{}"),
                201,
            );
            let served_body = format!("{{\"m\":\"{NONCE}\"}}");
            apigw_ok(
                "the integration response",
                &router,
                &runtime,
                "PUT",
                &format!("{method_path}/integration/responses/200"),
                Some(
                    &serde_json::json!({
                        "responseTemplates": { "application/json": served_body },
                    })
                    .to_string(),
                ),
                201,
            );
            repin_policy(
                "the policy pinned to the endpoint in the path",
                &router,
                &runtime,
                &api,
                &endpoint,
            );

            // -----------------------------------------------------------------
            // The REGIONAL front apis that reach the private one through the link.
            //
            // Three of them, differing ONLY in the host their private integration's `uri`
            // names: the endpoint's own DNS name, the private api's own `execute-api` name,
            // and the stage's address at the emulator's own endpoint. For such an
            // integration "the URI is not used for routing" (`Integration$uri`) — the
            // request is carried over the LINK, whose targets are this endpoint's own
            // network interfaces — so each arrives through the same endpoint and the api's
            // policy decides all three the same way.
            // -----------------------------------------------------------------
            let front_api = |name: &str,
                             case: &str,
                             proxy_uri: String,
                             uri_names_the_endpoint: bool|
             -> String {
                let front_body = apigw_ok(
                    case,
                    &router,
                    &runtime,
                    "POST",
                    "/restapis",
                    Some(
                        &serde_json::json!({
                            "name": name,
                            "endpointConfiguration": { "types": ["REGIONAL"] },
                        })
                        .to_string(),
                    ),
                    201,
                );
                let front = json_member(case, &front_body, "id");
                let front_root = json_member(case, &front_body, "rootResourceId");
                let front_method = format!("/restapis/{front}/resources/{front_root}/methods/GET");
                apigw_ok(
                    case,
                    &router,
                    &runtime,
                    "PUT",
                    &front_method,
                    Some(&serde_json::json!({ "authorizationType": "NONE" }).to_string()),
                    201,
                );
                let mut integration = serde_json::json!({
                    "type": "HTTP_PROXY",
                    "httpMethod": "GET",
                    "uri": proxy_uri,
                    "connectionType": "VPC_LINK",
                    "connectionId": link_id,
                });
                if uri_names_the_endpoint {
                    // An endpoint's own DNS name says which ENDPOINT, not which api, so
                    // that addressing carries the api in the header AWS documents it in,
                    // mapped on by the integration. A `uri` naming the api needs none.
                    integration["requestParameters"] = serde_json::json!({
                        "integration.request.header.x-apigw-api-id": format!("'{api}'"),
                    });
                }
                apigw_ok(
                    case,
                    &router,
                    &runtime,
                    "PUT",
                    &format!("{front_method}/integration"),
                    Some(&integration.to_string()),
                    201,
                );
                apigw_ok(
                    case,
                    &router,
                    &runtime,
                    "POST",
                    &format!("/restapis/{front}/deployments"),
                    Some(&serde_json::json!({ "stageName": "live" }).to_string()),
                    201,
                );
                format!("{front}.execute-api.{REGION}.amazonaws.com")
            };
            let endpoint_front_host = front_api(
                "private-vpce-front-endpoint",
                "the front api whose uri names the endpoint",
                format!(
                    "https://{host}/{STAGE}/{RESOURCE_PATH}",
                    host = endpoint_host(&endpoint)
                ),
                true,
            );
            let api_front_host = front_api(
                "private-vpce-front-api",
                "the front api whose uri names the private api",
                format!(
                    "https://{api}.execute-api.{REGION}.amazonaws.com/{STAGE}/{RESOURCE_PATH}"
                ),
                false,
            );
            // The third spelling of the same stage: the prefixed path-style address this
            // emulator announces an invoke URL at, which is the address a client of an
            // emulator holds for a stage it cannot reach by its AWS host name.
            let emulator_front_host = front_api(
                "private-vpce-front-emulator",
                "the front api whose uri names the stage at the emulator's own endpoint",
                format!("https://localhost/_aws/execute-api/{api}/{STAGE}/{RESOURCE_PATH}"),
                false,
            );

            // Rows: the endpoint the policy names, another endpoint, and the first one
            // again — the same request each time, differing only in what the policy admits.
            let invoke_path = format!("/{STAGE}/{RESOURCE_PATH}");
            for (case, pinned, expected) in [
                ("the endpoint the request arrived through", None, 200),
                ("another endpoint of the account", Some(OTHER_ENDPOINT), 403),
                ("the original endpoint, restored", Some(endpoint.as_str()), 200),
            ] {
                if let Some(pinned) = pinned {
                    repin_policy(case, &router, &runtime, &api, pinned);
                }
                // The same outcome whether the endpoint is addressed directly or reached
                // through a front api's private integration, which passes the request on
                // over the link to that same endpoint whatever host its `uri` names: what
                // decides it is the endpoint the request arrived through, not who sent it
                // and not how the integration spelled the api's address.
                let addressings = [
                    (
                        "addressed on the endpoint itself",
                        endpoint_host(&endpoint),
                        invoke_path.clone(),
                        vec![("x-apigw-api-id", api.as_str())],
                    ),
                    (
                        "reached through a VPC link whose uri names the endpoint",
                        endpoint_front_host.clone(),
                        "/live/".to_string(),
                        Vec::new(),
                    ),
                    (
                        "reached through a VPC link whose uri names the api",
                        api_front_host.clone(),
                        "/live/".to_string(),
                        Vec::new(),
                    ),
                    (
                        "reached through a VPC link whose uri names the emulator's own \
                         invoke address",
                        emulator_front_host.clone(),
                        "/live/".to_string(),
                        Vec::new(),
                    ),
                ];
                for (how, host, path, headers) in addressings {
                    let case = format!("{case}, {how}");
                    let answered = invoke(&router, &runtime, &host, &path, &headers);
                    assert_eq!(
                        answered.status, expected,
                        "case {case:?}: expected HTTP {expected}, got {answered:?}"
                    );
                    if expected == 200 {
                        assert_eq!(
                            answered.text, served_body,
                            "case {case:?}: expected the integration's own body, got \
                             {answered:?}"
                        );
                    } else {
                        assert!(
                            !answered.text.contains(NONCE),
                            "case {case:?}: a refusal must carry none of the body, got \
                             {answered:?}"
                        );
                        let message = json_member(&case, &answered.text, "Message");
                        assert!(
                            message.contains("not authorized to perform: execute-api:Invoke"),
                            "case {case:?}: expected the invoke denial, got {message:?}"
                        );
                    }
                }
            }
        })
        .expect("spawn the private-endpoint thread")
        .join()
        .expect("the private-endpoint chain must converge and the policy must gate it");
}
