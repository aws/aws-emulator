// SPDX-License-Identifier: CC0-1.0

//! A REST API's PRIVATE integration, end to end: the stage a client calls, the VPC link it
//! connects through, the Network Load Balancer that link targets and the internal
//! Application Load Balancer behind it.
//!
//! The rule under test: **a load balancer answers a request addressed to the name it
//! publishes out of the listener the request reached, and a listener that hands the
//! connection on — a `forward` to an `alb` target group, "alb - Register a single
//! Application Load Balancer as a target" — is answered by the load balancer the chosen
//! registration names, out of ITS own listener.** So a private integration of an API
//! Gateway stage, which reaches a backend through a Network Load Balancer, is answered by
//! what is really behind that load balancer: the matched rule's own response for a path a
//! rule of the Application Load Balancer matches, and the default action's for one no rule
//! matches.
//!
//! The rows differ along what the rule generalises over: the path the client asks for (one
//! the internal load balancer has a rule written for, two spellings of it, and one it has
//! none for, which its listener's default action answers), and so the status and the body
//! the answer carries — each of which is the internal load balancer's own, written through
//! its control plane and reachable by nothing else in the chain.
//!
//! The suite lives here because the chain spans two services on one engine: the link and the
//! api are API Gateway's, the two load balancers are Elastic Load Balancing's, and what is
//! under test is only observable through both.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::elbv2_network_resolution::{Answer, ELB_VERSION, REGION};
use crate::query_wire::{element, form_encode};

/// How long a poll here is given before the chain is called a non-convergence, and how long
/// it waits between reads.
///
/// Every transition in this chain is owed one of its service's own transition spans — half a
/// second each — so this is an ample margin on a loaded machine while still failing rather
/// than hanging if a resource never moves at all.
const POLL_TIMEOUT: Duration = Duration::from_secs(10);
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// The body and status the internal load balancer's RULE answers with, and the body and
/// status its listener's DEFAULT action answers with. Two different nonces: what proves the
/// answer came from the rule the request matched rather than from anything else in the
/// chain is that the two never agree.
const MATCHED_BODY: &str = "matched-7f31c2";
const MATCHED_STATUS: u16 = 200;
const DEFAULT_BODY: &str = "default-1a90de";
const DEFAULT_STATUS: u16 = 404;

/// The path pattern the internal load balancer's rule is written for.
const MATCHED_PATTERN: &str = "/orders/*";

/// The stage the REST API is deployed to.
const STAGE: &str = "live";

/// The emulator with the two services this suite exercises, mounted on ONE engine exactly
/// as `core/router/src/lib.rs` mounts them.
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_elasticloadbalancingv2_elasticloadbalancing_v10::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_apigateway_backplanecontrolservice::EmulatedService::new(engine),
    );
    router
}

/// A SigV4 `Authorization` header whose credential scope names `service`, which is what
/// routes the request. The signature is never checked: this router runs with request
/// authentication off.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=deadbeef"
    )
}

/// Drive one awsQuery action against the ELBv2 model, addressed by its own routing key.
async fn elbv2(router: &MultiServiceRouter, action: &str, params: &[(&str, &str)]) -> Answer {
    let mut body = format!("Action={action}&Version={ELB_VERSION}");
    for (name, value) in params {
        body.push('&');
        body.push_str(&form_encode(name));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let host = format!("elasticloadbalancingv2.{REGION}.amazonaws.com");
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://{host}/"))
        .header("host", &host)
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization("elasticloadbalancingv2"))
        .body(bytes::Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    Answer {
        status: response.status().as_u16(),
        text: String::from_utf8_lossy(response.body()).to_string(),
    }
}

/// One ELBv2 action that must succeed, and its body.
async fn elbv2_ok(
    case: &str,
    router: &MultiServiceRouter,
    action: &str,
    params: &[(&str, &str)],
) -> String {
    let answer = elbv2(router, action, params).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected HTTP 200 from {action}, got {answer:?}"
    );
    answer.text
}

/// One API Gateway control-plane call over its restJson1 wire.
async fn apigw(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> Answer {
    let host = format!("apigateway.{REGION}.amazonaws.com");
    let payload = match &body {
        Some(document) => bytes::Bytes::from(document.to_string()),
        None => bytes::Bytes::new(),
    };
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{path}"))
        .header("host", &host)
        .header("content-type", "application/json")
        .header("authorization", authorization("apigateway"))
        .body(payload)
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    Answer {
        status: response.status().as_u16(),
        text: String::from_utf8_lossy(response.body()).to_string(),
    }
}

/// One API Gateway call that must answer `expected`, and its body.
async fn apigw_ok(
    case: &str,
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<Value>,
    expected: u16,
) -> String {
    let answer = apigw(router, method, path, body).await;
    assert_eq!(
        answer.status, expected,
        "case {case:?}: expected HTTP {expected} from {method} {path}, got {answer:?}"
    );
    answer.text
}

/// One string member of a JSON body.
fn json_member(case: &str, body: &str, member: &str) -> String {
    let parsed: Value = serde_json::from_str(body)
        .unwrap_or_else(|error| panic!("case {case:?}: {error} reading {body}"));
    parsed
        .get(member)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: no {member:?} in {body}"))
        .to_string()
}

/// One anonymous invoke of the deployed stage: the data-plane request a client of the
/// published invoke URL sends, carrying no AWS credential.
async fn invoke(router: &MultiServiceRouter, api: &str, path: &str) -> Answer {
    let host = format!("{api}.execute-api.{REGION}.amazonaws.com");
    let request = http::Request::builder()
        .method("GET")
        .uri(format!("https://{host}/{STAGE}{path}"))
        .header("host", &host)
        .body(bytes::Bytes::new())
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    Answer {
        status: response.status().as_u16(),
        text: String::from_utf8_lossy(response.body()).to_string(),
    }
}

/// Poll `read` until `settled` holds of what it answers, and answer that.
///
/// Nothing in the chain is nudged between reads: a state that only ever changes because
/// somebody called again never satisfies the predicate here either.
async fn poll_until<R, F, Fut>(case: &str, mut read: F, settled: impl Fn(&R) -> bool) -> R
where
    R: std::fmt::Debug,
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = R>,
{
    let deadline = Instant::now() + POLL_TIMEOUT;
    loop {
        let answer = read().await;
        if settled(&answer) {
            return answer;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the chain never converged; last read {answer:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Everything the fixture built: the api whose stage the rows call.
struct Estate {
    router: MultiServiceRouter,
    api: String,
}

/// The whole private-integration estate, written through the two control planes: an internal
/// Application Load Balancer with one rule and a default action, a Network Load Balancer
/// whose TCP listener forwards to an `alb` target group holding that load balancer, a VPC
/// link over the Network Load Balancer, and a REST API whose catch-all method is an
/// `HTTP_PROXY` integration through that link.
async fn estate() -> Estate {
    let router = build_router();

    // The INTERNAL Application Load Balancer: the only thing in the chain that answers a
    // request, and the only place the two nonces exist.
    let application = element(
        &elbv2_ok(
            "the internal application load balancer",
            &router,
            "CreateLoadBalancer",
            &[
                ("Name", "private-integration-alb"),
                ("Scheme", "internal"),
                ("Subnets.member.1", "subnet-11111111"),
                ("Subnets.member.2", "subnet-22222222"),
            ],
        )
        .await,
        "LoadBalancerArn",
    );
    let application_listener = element(
        &elbv2_ok(
            "the application load balancer's HTTP listener",
            &router,
            "CreateListener",
            &[
                ("LoadBalancerArn", application.as_str()),
                ("Protocol", "HTTP"),
                ("Port", "80"),
                ("DefaultActions.member.1.Type", "fixed-response"),
                (
                    "DefaultActions.member.1.FixedResponseConfig.StatusCode",
                    &DEFAULT_STATUS.to_string(),
                ),
                (
                    "DefaultActions.member.1.FixedResponseConfig.MessageBody",
                    DEFAULT_BODY,
                ),
            ],
        )
        .await,
        "ListenerArn",
    );
    elbv2_ok(
        "the rule the matched path is answered by",
        &router,
        "CreateRule",
        &[
            ("ListenerArn", application_listener.as_str()),
            ("Priority", "10"),
            ("Conditions.member.1.Field", "path-pattern"),
            ("Conditions.member.1.Values.member.1", MATCHED_PATTERN),
            ("Actions.member.1.Type", "fixed-response"),
            (
                "Actions.member.1.FixedResponseConfig.StatusCode",
                &MATCHED_STATUS.to_string(),
            ),
            (
                "Actions.member.1.FixedResponseConfig.MessageBody",
                MATCHED_BODY,
            ),
        ],
    )
    .await;

    // The `alb` target group holding that load balancer, and the Network Load Balancer whose
    // TCP listener forwards to it — the shape a private integration's backend is built in.
    let target_group = element(
        &elbv2_ok(
            "an alb target group",
            &router,
            "CreateTargetGroup",
            &[
                ("Name", "private-integration-targets"),
                ("TargetType", "alb"),
                ("Protocol", "TCP"),
                ("Port", "80"),
                ("HealthCheckProtocol", "HTTP"),
                ("VpcId", "vpc-11111111"),
            ],
        )
        .await,
        "TargetGroupArn",
    );
    elbv2_ok(
        "the application load balancer registered as the target",
        &router,
        "RegisterTargets",
        &[
            ("TargetGroupArn", target_group.as_str()),
            ("Targets.member.1.Id", application.as_str()),
        ],
    )
    .await;
    let network_body = elbv2_ok(
        "the internal network load balancer",
        &router,
        "CreateLoadBalancer",
        &[
            ("Name", "private-integration-nlb"),
            ("Scheme", "internal"),
            ("Type", "network"),
            ("Subnets.member.1", "subnet-11111111"),
        ],
    )
    .await;
    let network = element(&network_body, "LoadBalancerArn");
    let network_dns = element(&network_body, "DNSName");
    elbv2_ok(
        "the network load balancer's TCP listener",
        &router,
        "CreateListener",
        &[
            ("LoadBalancerArn", network.as_str()),
            ("Protocol", "TCP"),
            ("Port", "80"),
            ("DefaultActions.member.1.Type", "forward"),
            ("DefaultActions.member.1.TargetGroupArn", target_group.as_str()),
        ],
    )
    .await;

    // The VPC link over the network load balancer, which the integration connects through.
    let link = apigw_ok(
        "the VPC link",
        &router,
        "POST",
        "/vpclinks",
        Some(json!({ "name": "private-integration-link", "targetArns": [network] })),
        202,
    )
    .await;
    let link_id = json_member("the VPC link", &link, "id");
    let link_status = poll_until(
        "the VPC link reaches AVAILABLE with nobody calling in between",
        || async {
            let body = apigw_ok(
                "the VPC link's status",
                &router,
                "GET",
                &format!("/vpclinks/{link_id}"),
                None,
                200,
            )
            .await;
            json_member("the VPC link's status", &body, "status")
        },
        |status| status == "AVAILABLE" || status == "FAILED",
    )
    .await;
    assert_eq!(
        link_status, "AVAILABLE",
        "case \"the link's terminal status\": a link over a network load balancer this \
         account holds provisions, got {link_status:?}"
    );

    // The REST API whose catch-all method proxies every path through that link.
    let api_body = apigw_ok(
        "the api",
        &router,
        "POST",
        "/restapis",
        Some(json!({ "name": "private-integration-api" })),
        201,
    )
    .await;
    let api = json_member("the api", &api_body, "id");
    let root = json_member("the api", &api_body, "rootResourceId");
    let proxy = json_member(
        "the catch-all resource",
        &apigw_ok(
            "the catch-all resource",
            &router,
            "POST",
            &format!("/restapis/{api}/resources/{root}"),
            Some(json!({ "pathPart": "{proxy+}" })),
            201,
        )
        .await,
        "id",
    );
    let method_path = format!("/restapis/{api}/resources/{proxy}/methods/GET");
    apigw_ok(
        "the catch-all method",
        &router,
        "PUT",
        &method_path,
        Some(json!({
            "authorizationType": "NONE",
            "requestParameters": { "method.request.path.proxy": true },
        })),
        201,
    )
    .await;
    // "For a private HTTP integration … specify the Network Load Balancer DNS name"
    // (`Integration$uri`), with the greedy path parameter addressing the backend path.
    apigw_ok(
        "the private integration",
        &router,
        "PUT",
        &format!("{method_path}/integration"),
        Some(json!({
            "type": "HTTP_PROXY",
            "httpMethod": "GET",
            "uri": format!("http://{network_dns}/{{proxy}}"),
            "connectionType": "VPC_LINK",
            "connectionId": link_id,
            "requestParameters": {
                "integration.request.path.proxy": "method.request.path.proxy",
            },
        })),
        201,
    )
    .await;
    apigw_ok(
        "the deployment",
        &router,
        "POST",
        &format!("/restapis/{api}/deployments"),
        Some(json!({ "stageName": STAGE })),
        201,
    )
    .await;

    Estate { router, api }
}

#[tokio::test]
async fn a_private_integration_is_answered_by_the_load_balancer_behind_the_link() {
    let estate = estate().await;
    let router = &estate.router;
    let api = &estate.api;

    // The rows: two paths the internal load balancer's rule matches and one it does not,
    // each answered by the action that holds for it.
    let rows: &[(&str, &str, u16, &str)] = &[
        (
            "a path the internal load balancer's rule matches",
            "/orders/42",
            MATCHED_STATUS,
            MATCHED_BODY,
        ),
        (
            "a deeper path under the same matched prefix",
            "/orders/42/items",
            MATCHED_STATUS,
            MATCHED_BODY,
        ),
        (
            "a path no rule matches, answered by the listener's default action",
            "/health",
            DEFAULT_STATUS,
            DEFAULT_BODY,
        ),
    ];

    for (case, path, status, body) in rows {
        // The load balancers' moves to `active` and the registration's to `healthy` are the
        // clock's to make due, so the invoke is asked until the chain carries it.
        let answered = poll_until(
            case,
            || invoke(router, api, path),
            |answered: &Answer| answered.text.contains(MATCHED_BODY) || answered.text.contains(DEFAULT_BODY),
        )
        .await;
        assert_eq!(
            answered.status, *status,
            "case {case:?}: expected HTTP {status} from the load balancer behind the link, \
             got {answered:?}"
        );
        assert_eq!(
            answered.text, *body,
            "case {case:?}: expected the internal load balancer's own body, got {answered:?}"
        );
    }
}
