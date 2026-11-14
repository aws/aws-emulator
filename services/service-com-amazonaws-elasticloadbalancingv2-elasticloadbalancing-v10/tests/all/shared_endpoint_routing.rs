// SPDX-License-Identifier: CC0-1.0

//! This model served through the ONE `elasticloadbalancing` front door, which is where
//! every AWS SDK ELBv2 client arrives.
//!
//! Both emulated Elastic Load Balancing services publish the SAME wire identity —
//! `awsQuery`, `endpointPrefix` / `arnNamespace` / `aws.auth#sigv4` name
//! `elasticloadbalancing`, `cloudTrailEventSource` `elasticloadbalancing.amazonaws.com`
//! (`smithy_query(query="service_info")` on both models) — and differ only in their API
//! version: `2015-12-01` here, `2012-06-01` for the Classic Load Balancer model. While
//! both claimed that shared signing name, the router found two claimants and answered
//! `500 AmbiguousService` — a code no AWS model carries, in a JSON `__type` envelope an
//! `awsQuery` client cannot parse at all — for EVERY Elastic Load Balancing request, so
//! neither API could be reached.
//!
//! The rule this suite pins, over a table of inputs rather than one recorded request:
//!
//! * the `Version` field of the request's own `awsQuery` form body — the only thing that
//!   tells the two APIs apart, and the field AWS resolves together with `Action` — decides
//!   WHICH of the two models mounted behind that front door serves it, so an action BOTH
//!   models declare is served by the one the request names, an action only one of them
//!   declares is served there or refused there, and a version neither declares is refused
//!   `400 InvalidAction`;
//! * a request this model serves answers identically whether it arrived on the shared key
//!   or on this model's own key: the same `xmlns`, the same shapes, the same store;
//! * and whatever a request gets back, it is an Elastic Load Balancing `awsQuery` XML
//!   answer — never the router's JSON refusal.
//!
//! That exactly one service claims the shared key, whichever signal resolves it, is the
//! claim table in `core/router/tests/all/elb_classic_and_v2_signing_name_routing.rs`,
//! which asks each service's `can_handle` over the assembled production router; this
//! suite does not restate it.
//!
//! The dispatch rows run IN ORDER against one router over one engine, so a later row reads
//! what an earlier one wrote: the two `DescribeLoadBalancers` rows are the sharpest case,
//! since both models declare that action and each must report only ITS OWN load balancers.
//!
//! The suite's 18 dispatch rows cover every combination of the three axes that decide what
//! answers — the key the request is signed for, the version its envelope names, and which
//! model (or neither, or both) declares the action — including the asymmetric row where
//! the Classic version is named on this model's own key.

use service_com_amazonaws_elasticloadbalancing_elasticloadbalancing_v7::EmulatedService as ClassicService;
use service_com_amazonaws_elasticloadbalancingv2_elasticloadbalancing_v10::EmulatedService as LoadBalancingV2Service;
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

// The wire helpers this crate's suites share, from the sibling module of this one test
// binary: one form encoder, one field constructor, one answer type and one element reader,
// so the two suites cannot drift on encoding or on how an empty element is reported.
use super::provisioning_loop::{encode, first_text, p, Reply};

/// The signing name, endpoint prefix and ARN namespace both models declare, and so the
/// routing key an AWS SDK client of either API sends.
pub(crate) const SHARED_KEY: &str = "elasticloadbalancing";

/// The routing key that names THIS model directly, without the shared front door.
pub(crate) const V2_KEY: &str = "elasticloadbalancingv2";

/// The region every request below is signed for; the ARNs and DNS names both services
/// mint belong to it.
pub(crate) const REGION: &str = "us-west-2";

/// This model's API version, read from the constant the front door that owns the shared
/// key routes on, so the suite cannot drift from the rule it tests.
const V2_VERSION: &str =
    service_com_amazonaws_elasticloadbalancing_elasticloadbalancing_v7::service::api_version::V2_API_VERSION;

/// The Classic Load Balancer model's API version (`2012-06-01`), read from the same place
/// for the same reason.
const CLASSIC_VERSION: &str =
    service_com_amazonaws_elasticloadbalancing_elasticloadbalancing_v7::service::api_version::CLASSIC_API_VERSION;

/// The emulator under test: ONE shared engine with BOTH Elastic Load Balancing services
/// mounted on the multi-service router, exactly as the production router assembles them.
/// The ambiguity this suite is about exists only when both are mounted.
///
/// `pub(crate)` because `declared_action_set.rs`, in this same test binary, drives the whole
/// declared action set through these same two front doors: one assembly for the crate, so
/// no suite can drift on how the two services are mounted.
pub(crate) fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router
        .set_request_authentication(false)
        .register_service(ClassicService::new(Arc::clone(&engine)))
        .register_service(LoadBalancingV2Service::new(engine));
    router
}

/// Send one `awsQuery` request: the routing key it is signed for, the `Action`, the
/// `Version` its envelope names (or none at all), and the operation's own form fields.
///
/// `pub(crate)` for the same reason [`emulator`] is: one request builder for the two suites
/// that address these front doors.
pub(crate) async fn call(
    router: &MultiServiceRouter,
    key: &str,
    action: &str,
    version: Option<&str>,
    params: &[(String, String)],
) -> Reply {
    let mut fields = vec![p("Action", action)];
    if let Some(version) = version {
        fields.push(p("Version", version));
    }
    fields.extend(params.iter().cloned());
    let body = fields
        .iter()
        .map(|(name, value)| format!("{}={}", encode(name), encode(value)))
        .collect::<Vec<_>>()
        .join("&");
    let request = http::Request::builder()
        .method("POST")
        .uri("https://elasticloadbalancing.us-west-2.amazonaws.com/")
        .header("host", "elasticloadbalancing.us-west-2.amazonaws.com")
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization(key))
        .body(bytes::Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    Reply {
        status: response.status().as_u16(),
        body: String::from_utf8_lossy(response.body()).to_string(),
    }
}

/// The SigV4 `Authorization` header of a request signed for `signing_name`, as every AWS
/// SDK sends it. The signature is never verified here: the claim predicate reads only the
/// credential scope.
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{signing_name}/aws4_request, \
         SignedHeaders=host, Signature=not-verified"
    )
}

/// One dispatch row: how the envelope is addressed and what the ONE model that owns the
/// pair must answer.
struct Call {
    case: &'static str,
    /// The routing key the request is signed for.
    key: &'static str,
    action: &'static str,
    /// The `Version` the envelope names, or `None` for an envelope that names none.
    version: Option<&'static str>,
    params: Vec<(String, String)>,
    status: u16,
    /// The `awsQuery` `<Code>` the answer carries, when the row pins one.
    code: Option<&'static str>,
    /// Text the answer must carry, for a row that pins WHAT was reported.
    contains: Vec<String>,
    /// Text the answer must NOT carry — how a row proves the OTHER model did not serve it.
    absent: Vec<String>,
}

/// The xmlns of a model's success envelope, which the framework stamps on the response
/// root from the service's `@xmlNamespace` trait. It is per-model
/// (`http://elasticloadbalancing.amazonaws.com/doc/<version>/`), so it names which model's
/// protocol serialized the answer.
fn envelope_namespace(version: &str) -> String {
    format!("http://elasticloadbalancing.amazonaws.com/doc/{version}/")
}

#[tokio::test]
async fn the_envelope_version_decides_which_elb_api_serves_a_request() {
    let router = emulator();
    let absent_target_group = format!(
        "arn:aws:elasticloadbalancing:{REGION}:{}:targetgroup/no-such-group/0123456789abcdef",
        emulator_server::emulator_account_id()
    );

    let target_group = vec![
        p("Name", "shared-tg"),
        p("Protocol", "HTTP"),
        p("Port", "80"),
        p("VpcId", "vpc-12345678"),
    ];
    let classic_load_balancer = vec![
        p("LoadBalancerName", "classic-lb"),
        p("AvailabilityZones.member.1", "us-west-2a"),
        p("Listeners.member.1.Protocol", "HTTP"),
        p("Listeners.member.1.LoadBalancerPort", "80"),
        p("Listeners.member.1.InstancePort", "80"),
    ];

    let calls: Vec<Call> = vec![
        Call {
            // The request the issue reports as an unparseable `500 AmbiguousService`: an
            // ELBv2 call as an AWS SDK sends it, signed for the SHARED name.
            case: "CreateTargetGroup on the shared key at this model's version",
            key: SHARED_KEY,
            action: "CreateTargetGroup",
            version: Some(V2_VERSION),
            params: target_group.clone(),
            status: 200,
            code: None,
            contains: vec![
                "<CreateTargetGroupResponse".to_string(),
                envelope_namespace(V2_VERSION),
                "targetgroup/shared-tg/".to_string(),
            ],
            absent: vec![],
        },
        Call {
            // Replayed with identical settings: the operation is idempotent, and the ARN
            // check after the loop pins that every answer names one group.
            case: "CreateTargetGroup replayed with identical settings",
            key: SHARED_KEY,
            action: "CreateTargetGroup",
            version: Some(V2_VERSION),
            params: target_group.clone(),
            status: 200,
            code: None,
            contains: vec!["targetgroup/shared-tg/".to_string()],
            absent: vec![],
        },
        Call {
            // The same group read back through the key that names THIS model directly:
            // one store, reached either way.
            case: "DescribeTargetGroups on this model's own key reads it back",
            key: V2_KEY,
            action: "DescribeTargetGroups",
            version: Some(V2_VERSION),
            params: vec![p("Names.member.1", "shared-tg")],
            status: 200,
            code: None,
            contains: vec![
                envelope_namespace(V2_VERSION),
                "<TargetGroupName>shared-tg</TargetGroupName>".to_string(),
                "<VpcId>vpc-12345678</VpcId>".to_string(),
            ],
            absent: vec![],
        },
        Call {
            // …and through the shared front door, which must answer the same document.
            case: "DescribeTargetGroups on the shared key reads it back",
            key: SHARED_KEY,
            action: "DescribeTargetGroups",
            version: Some(V2_VERSION),
            params: vec![p("Names.member.1", "shared-tg")],
            status: 200,
            code: None,
            contains: vec![
                envelope_namespace(V2_VERSION),
                "<TargetGroupName>shared-tg</TargetGroupName>".to_string(),
                "<VpcId>vpc-12345678</VpcId>".to_string(),
            ],
            absent: vec![],
        },
        Call {
            // The issue's other measured oracle: a well-formed ARN nothing holds, through
            // the shared front door — this model's own modelled error, status and code.
            case: "DescribeTargetGroups for an absent ARN on the shared key",
            key: SHARED_KEY,
            action: "DescribeTargetGroups",
            version: Some(V2_VERSION),
            params: vec![p("TargetGroupArns.member.1", &absent_target_group)],
            status: 400,
            code: Some("TargetGroupNotFound"),
            contains: vec!["<ErrorResponse".to_string()],
            absent: vec![],
        },
        Call {
            case: "CreateLoadBalancer on the shared key at the Classic model's version",
            key: SHARED_KEY,
            action: "CreateLoadBalancer",
            version: Some(CLASSIC_VERSION),
            params: classic_load_balancer.clone(),
            status: 200,
            code: None,
            contains: vec![
                "<CreateLoadBalancerResponse".to_string(),
                envelope_namespace(CLASSIC_VERSION),
                "<DNSName>".to_string(),
            ],
            // The Classic model has no ARN member at all: an answer carrying one would be
            // this model's `CreateLoadBalancer` serving the Classic request.
            absent: vec!["LoadBalancerArn".to_string()],
        },
        Call {
            // The SAME action name at this model's version reaches a different operation,
            // with different required members and a different answer.
            case: "CreateLoadBalancer on the shared key at this model's version",
            key: SHARED_KEY,
            action: "CreateLoadBalancer",
            version: Some(V2_VERSION),
            params: vec![
                p("Name", "shared-alb"),
                p("Subnets.member.1", "subnet-11111111"),
                p("Subnets.member.2", "subnet-22222222"),
            ],
            status: 200,
            code: None,
            contains: vec![
                envelope_namespace(V2_VERSION),
                "<LoadBalancerArn>".to_string(),
                "loadbalancer/app/shared-alb/".to_string(),
            ],
            absent: vec!["<DNSName>classic".to_string()],
        },
        Call {
            // An action BOTH models declare, read back at the Classic version: the
            // Classic store only, projected through the Classic model's own output shape.
            case: "DescribeLoadBalancers at the Classic model's version",
            key: SHARED_KEY,
            action: "DescribeLoadBalancers",
            version: Some(CLASSIC_VERSION),
            params: vec![],
            status: 200,
            code: None,
            contains: vec![
                envelope_namespace(CLASSIC_VERSION),
                "<LoadBalancerDescriptions>".to_string(),
                "<LoadBalancerName>classic-lb</LoadBalancerName>".to_string(),
            ],
            absent: vec!["shared-alb".to_string(), "LoadBalancerArn".to_string()],
        },
        Call {
            // …and at this model's version: this store only, through this output shape.
            case: "DescribeLoadBalancers at this model's version",
            key: SHARED_KEY,
            action: "DescribeLoadBalancers",
            version: Some(V2_VERSION),
            params: vec![],
            status: 200,
            code: None,
            contains: vec![
                envelope_namespace(V2_VERSION),
                "<LoadBalancers>".to_string(),
                "<LoadBalancerName>shared-alb</LoadBalancerName>".to_string(),
                "loadbalancer/app/shared-alb/".to_string(),
            ],
            absent: vec![
                "classic-lb".to_string(),
                "LoadBalancerDescriptions".to_string(),
            ],
        },
        Call {
            // An action only the CLASSIC model declares, asked for at THIS model's
            // version: this model's own routing failure, not a silent hand-back to the
            // front door's model.
            case: "a Classic-only action at this model's version",
            key: SHARED_KEY,
            action: "DescribeInstanceHealth",
            version: Some(V2_VERSION),
            params: vec![p("LoadBalancerName", "classic-lb")],
            status: 400,
            code: Some("InvalidAction"),
            contains: vec!["<ErrorResponse".to_string()],
            absent: vec!["InstanceStates".to_string()],
        },
        Call {
            // …and the mirror image: an action only THIS model declares, at the Classic
            // version, is the front door model's own routing failure.
            case: "an action only this model declares at the Classic model's version",
            key: SHARED_KEY,
            action: "CreateTargetGroup",
            version: Some(CLASSIC_VERSION),
            params: target_group.clone(),
            status: 400,
            code: Some("InvalidAction"),
            contains: vec!["<ErrorResponse".to_string()],
            absent: vec!["TargetGroupArn".to_string()],
        },
        Call {
            // A version NEITHER model declares: the pair resolves nowhere, and the
            // refusal names both halves of it.
            case: "a version neither model declares",
            key: SHARED_KEY,
            action: "CreateTargetGroup",
            version: Some("2099-01-01"),
            params: target_group.clone(),
            status: 400,
            code: Some("InvalidAction"),
            contains: vec![
                "Could not find operation CreateTargetGroup for version 2099-01-01".to_string(),
            ],
            absent: vec!["TargetGroupArn".to_string()],
        },
        Call {
            // A well-formed version belonging to another AWS Query service (Amazon RDS's
            // `2014-10-31`): still no model behind THIS endpoint.
            case: "another service's API version",
            key: SHARED_KEY,
            action: "DescribeLoadBalancers",
            version: Some("2014-10-31"),
            params: vec![],
            status: 400,
            code: Some("InvalidAction"),
            contains: vec![
                "Could not find operation DescribeLoadBalancers for version 2014-10-31".to_string(),
            ],
            absent: vec!["<LoadBalancers>".to_string()],
        },
        Call {
            // No version stated: nothing to disagree with, so the action alone decides
            // and the front door's own model answers — here with what the Classic create
            // above stored.
            case: "no version, an action only the Classic model declares",
            key: SHARED_KEY,
            action: "DescribeInstanceHealth",
            version: None,
            params: vec![p("LoadBalancerName", "classic-lb")],
            status: 200,
            code: None,
            contains: vec![
                envelope_namespace(CLASSIC_VERSION),
                "<InstanceStates".to_string(),
            ],
            absent: vec![],
        },
        Call {
            // …and on THIS model's own key, where the front door is not involved at all,
            // an envelope with no version reaches this model.
            case: "no version on this model's own key",
            key: V2_KEY,
            action: "DescribeTargetGroups",
            version: None,
            params: vec![p("Names.member.1", "shared-tg")],
            status: 200,
            code: None,
            contains: vec![
                envelope_namespace(V2_VERSION),
                "<TargetGroupName>shared-tg</TargetGroupName>".to_string(),
            ],
            absent: vec![],
        },
        Call {
            // The asymmetric case: the CLASSIC version named on THIS model's own key,
            // which the front door is not involved in at all. A request addressed at this
            // model is served by it, whatever version it names — the version decides
            // between the two models only on the key they SHARE.
            case: "the Classic model's version on this model's own key",
            key: V2_KEY,
            action: "DescribeTargetGroups",
            version: Some(CLASSIC_VERSION),
            params: vec![p("Names.member.1", "shared-tg")],
            status: 200,
            code: None,
            contains: vec![
                envelope_namespace(V2_VERSION),
                "<TargetGroupName>shared-tg</TargetGroupName>".to_string(),
            ],
            absent: vec![],
        },
        Call {
            case: "an action neither model declares",
            key: SHARED_KEY,
            action: "NoSuchAction",
            version: Some(V2_VERSION),
            params: vec![],
            status: 400,
            code: Some("InvalidAction"),
            contains: vec!["<ErrorResponse".to_string()],
            absent: vec![],
        },
        Call {
            // This model's modelled not-found through the shared front door, for a name
            // nothing holds: its status and `awsQueryError` code, in its own envelope.
            case: "this model's modelled not-found through the shared front door",
            key: SHARED_KEY,
            action: "DescribeTargetGroups",
            version: Some(V2_VERSION),
            params: vec![p("Names.member.1", "no-such-group")],
            status: 400,
            code: Some("TargetGroupNotFound"),
            contains: vec!["<ErrorResponse".to_string()],
            absent: vec![],
        },
    ];

    // Every row reporting a target group must report the SAME ARN: the create, its
    // idempotent replay and both read-backs all name one stored group.
    let mut target_group_arns: Vec<(&str, String)> = Vec::new();

    for row in &calls {
        let reply = call(&router, row.key, row.action, row.version, &row.params).await;
        let case = row.case;
        let status = reply.status;
        let body = &reply.body;

        // Neither routing failure may happen: `AmbiguousService` (two claimants) nor
        // `UnknownService` (none).
        assert!(
            !body.contains("AmbiguousService") && !body.contains("UnknownService"),
            "case {case:?}: the request must reach exactly one service, got {status} with {body}"
        );
        // Whatever the answer is, it is an Elastic Load Balancing awsQuery document — XML
        // a query-protocol client can parse — and never the router's JSON `__type` body.
        assert!(
            body.trim_start().starts_with('<') && !body.contains("__type"),
            "case {case:?}: an awsQuery client must get XML, got {status} with {body}"
        );
        assert_eq!(
            status, row.status,
            "case {case:?}: expected HTTP {}, got {status} with {body}",
            row.status
        );
        if let Some(expected) = row.code {
            let code = first_text(body, "Code");
            assert_eq!(
                code.as_deref(),
                Some(expected),
                "case {case:?}: expected the error code {expected}, got {status} with {body}"
            );
        }
        for expected in &row.contains {
            assert!(
                body.contains(expected.as_str()),
                "case {case:?}: expected {expected:?} in the answer, got {status} with {body}"
            );
        }
        for unexpected in &row.absent {
            assert!(
                !body.contains(unexpected.as_str()),
                "case {case:?}: {unexpected:?} must not appear in the answer, got {status} \
                 with {body}"
            );
        }
        if let Some(arn) = first_text(body, "TargetGroupArn") {
            target_group_arns.push((case, arn));
        }
    }

    assert!(
        target_group_arns.len() >= 4,
        "expected the create, its replay and both read-backs to report a TargetGroupArn, got \
         {target_group_arns:?}"
    );
    for (case, arn) in &target_group_arns {
        assert_eq!(
            arn, &target_group_arns[0].1,
            "case {case:?}: every answer for one target group must name the same ARN, got {arn} \
             against {}",
            target_group_arns[0].1
        );
    }
}

