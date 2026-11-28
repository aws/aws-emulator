// SPDX-License-Identifier: CC0-1.0

//! What a SPOT INSTANCE REQUEST is admitted with, what the Spot price does to it, and what
//! the instance it launches reports — end to end over the wire the service speaks
//! (`ec2Query`, through the shared [`super::ec2_query`] harness).
//!
//! Three rules, each asked of a set of inputs:
//!
//! 1. INTERRUPTION BEHAVIOUR — stopping or hibernating an interrupted Spot Instance keeps it
//!    to be started again, which only a request that outlives its instance can do, so the two
//!    behaviours are offered on a `persistent` request alone: a `one-time` request naming
//!    either is refused `InvalidParameterCombination` and stores nothing.
//! 2. PRICE — "the maximum price per unit hour that you are willing to pay"
//!    (`RequestSpotInstancesRequest$SpotPrice`) below the Spot price in force buys no
//!    capacity, so the request stays `open` carrying `price-too-low` and launches no
//!    instance; a maximum at or above it is fulfilled. The price a row is judged against is
//!    the one `DescribeSpotPriceHistory` publishes for that zone, type and product, read
//!    through that operation rather than written here.
//! 3. THE INSTANCE — an instance a fulfilled request launched is a Spot Instance: it reports
//!    the `spot` lifecycle and the request that launched it, and the IAM instance profile the
//!    launch specification named.

use ec2::state::{ComAmazonawsEc2Amazonec2State as Ec2State, ImageSpec, TRANSITION_DELAY};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, elements, error_code, error_message, filters, indexed};

/// The Region every call here is signed for.
const REGION: &str = "us-west-2";

/// The instance type every row requests, so one published price applies to the whole suite.
const INSTANCE_TYPE: &str = "m5.large";

/// One `(key, value)` request parameter.
fn p(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

/// One call that must answer 200, and the body it answered.
async fn ok(router: &MultiServiceRouter, case: &str, action: &str, params: &[(String, String)]) -> String {
    let (status, body) = call(router, REGION, action, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {action}, got {status} with body {body}"
    );
    body
}

/// Store one AMI and answer its id, through the store's own writer.
fn seed_image(engine: &engine::InMemoryEngine, name: &str) -> String {
    engine.state::<Ec2State>().lock().insert_image(ImageSpec {
        name: name.to_string(),
        description: "the spot suite's image".to_string(),
        ..ImageSpec::default()
    })
}

/// The Availability Zone every row places its request in: the Region's first, which is where
/// a request that names no zone is placed.
async fn first_zone(router: &MultiServiceRouter) -> String {
    let body = ok(router, "the Region's zones", "DescribeAvailabilityZones", &[]).await;
    elements(&body, "zoneName")
        .first()
        .unwrap_or_else(|| panic!("the Region must publish a zone: {body}"))
        .clone()
}

/// The Spot price in force for `INSTANCE_TYPE` in `zone`, as `DescribeSpotPriceHistory`
/// publishes it: the most recent change point's price.
///
/// Read through the operation that OWNS the price, so no row writes a price of its own.
async fn published_spot_price(router: &MultiServiceRouter, zone: &str) -> f64 {
    let mut params = indexed("InstanceType", &[INSTANCE_TYPE]);
    params.extend(filters(&[("availability-zone", &[zone])]));
    params.push(p("MaxResults", "1"));
    let body = ok(router, "the published Spot price", "DescribeSpotPriceHistory", &params).await;
    let price = elements(&body, "spotPrice")
        .first()
        .unwrap_or_else(|| panic!("the history must price {INSTANCE_TYPE}: {body}"))
        .clone();
    price
        .parse()
        .unwrap_or_else(|_| panic!("a published price must be a number, got {price:?}"))
}

/// A price as a Spot request's `SpotPrice` carries one: six fractional digits, the places AWS
/// publishes a Spot price to.
fn price(amount: f64) -> String {
    format!("{amount:.6}")
}

/// The members of the one Spot Instance request `request_id` names, as
/// `DescribeSpotInstanceRequests` reports them: its state, its status code and the instance it
/// launched.
async fn described_request(
    router: &MultiServiceRouter,
    case: &str,
    request_id: &str,
) -> (String, String, Option<String>) {
    let body = ok(
        router,
        case,
        "DescribeSpotInstanceRequests",
        &indexed("SpotInstanceRequestId", &[request_id]),
    )
    .await;
    let parsed = super::wire::document(case, &body);
    let item = parsed
        .descendants()
        .find(|node| node.is_element() && super::wire::child_text(*node, "spotInstanceRequestId").is_some())
        .unwrap_or_else(|| panic!("case {case:?}: the request must be reported: {body}"));
    let state = super::wire::child_text(item, "state").unwrap_or_default();
    let code = super::wire::child(item, "status")
        .and_then(|status| super::wire::child_text(status, "code"))
        .unwrap_or_default();
    let instance_id = super::wire::child_text(item, "instanceId");
    (state, code, instance_id)
}

/// Look again until the request `request_id` has been evaluated — until its status code is
/// something other than the `pending-evaluation` it is created with — or a generous deadline
/// passes.
///
/// The evaluation is work the CLOCK owes the request, so the condition is looked at AGAIN in a
/// loop rather than waited out, and the deadline is a multiple of the service's own transition
/// span.
async fn poll_until_evaluated(
    router: &MultiServiceRouter,
    case: &str,
    request_id: &str,
) -> (String, String, Option<String>) {
    let deadline = std::time::Instant::now() + TRANSITION_DELAY * 40;
    loop {
        let reported = described_request(router, case, request_id).await;
        if reported.1 != "pending-evaluation" {
            return reported;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "case {case:?}: {request_id} must be evaluated, last read {reported:?}"
        );
        tokio::time::sleep(TRANSITION_DELAY / 10).await;
    }
}

/// The parameters of one Spot Instance request from `image_id` with `extra` beside them.
fn request_params(image_id: &str, extra: &[(String, String)]) -> Vec<(String, String)> {
    let mut params = vec![
        p("LaunchSpecification.ImageId", image_id),
        p("LaunchSpecification.InstanceType", INSTANCE_TYPE),
        p("InstanceCount", "1"),
    ];
    params.extend_from_slice(extra);
    params
}

/// How many Spot Instance requests the account holds, so a row can assert that a refused
/// request stored none.
async fn held_requests(router: &MultiServiceRouter, case: &str) -> usize {
    let body = ok(router, case, "DescribeSpotInstanceRequests", &[]).await;
    elements(&body, "spotInstanceRequestId").len()
}

/// RULE 1: `stop` and `hibernate` are interruption behaviours only a persistent request can
/// perform, so a one-time request naming either is refused and stores nothing.
///
/// The rows span both request types against all three behaviours the model publishes, plus
/// the request that names neither member.
#[tokio::test]
async fn the_interruption_behaviour_is_admitted_against_the_request_type() {
    let (router, engine) = super::ec2_query::emulator();
    let image_id = seed_image(&engine, "spot-interruption");

    // (the request type, the interruption behaviour) — `None` for the member left out.
    let rows: Vec<(Option<&str>, Option<&str>)> = vec![
        (None, None),
        (Some("one-time"), None),
        (Some("one-time"), Some("terminate")),
        (Some("one-time"), Some("stop")),
        (Some("one-time"), Some("hibernate")),
        (Some("persistent"), Some("terminate")),
        (Some("persistent"), Some("stop")),
        (Some("persistent"), Some("hibernate")),
        (None, Some("stop")),
    ];

    for (request_type, behavior) in rows {
        let case = format!("a {request_type:?} request with interruption behaviour {behavior:?}");
        let mut extra = Vec::new();
        if let Some(request_type) = request_type {
            extra.push(p("Type", request_type));
        }
        if let Some(behavior) = behavior {
            extra.push(p("InstanceInterruptionBehavior", behavior));
        }
        let before = held_requests(&router, &case).await;
        let (status, body) = call(
            &router,
            REGION,
            "RequestSpotInstances",
            &request_params(&image_id, &extra),
        )
        .await;

        // A request that names no type is one-time: "Default: one-time"
        // (`RequestSpotInstancesRequest$Type`), and one that names no behaviour terminates.
        let one_time = request_type.unwrap_or("one-time") == "one-time";
        let keeps_the_instance = behavior.unwrap_or("terminate") != "terminate";
        if one_time && keeps_the_instance {
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "InvalidParameterCombination"),
                "case {case:?}: the pair must be refused, got {status} with body {body}"
            );
            let message = error_message(&body);
            assert!(
                message.contains(request_type.unwrap_or("one-time"))
                    && message.contains(behavior.unwrap_or("terminate")),
                "case {case:?}: the refusal must name both values, got {message:?}"
            );
            assert_eq!(
                held_requests(&router, &case).await,
                before,
                "case {case:?}: a refused request must store nothing"
            );
            continue;
        }
        assert_eq!(
            status, 200,
            "case {case:?}: the pair must be served, got {status} with body {body}"
        );
        assert_eq!(
            held_requests(&router, &case).await,
            before + 1,
            "case {case:?}: a served request must be stored"
        );
    }
}

/// RULE 2: a maximum price below the Spot price in force leaves the request `open` carrying
/// `price-too-low` and launches nothing; one at or above it is fulfilled.
///
/// The rows span the maximum price around the published price — well below it, just below it,
/// exactly it, above it — and the request that names no maximum at all, which pays the
/// current price and so is never priced out.
#[tokio::test]
async fn a_maximum_price_below_the_published_spot_price_is_never_fulfilled() {
    let (router, engine) = super::ec2_query::emulator();
    let image_id = seed_image(&engine, "spot-pricing");
    let zone = first_zone(&router).await;
    let market = published_spot_price(&router, &zone).await;
    assert!(
        market > 0.0,
        "the published Spot price of {INSTANCE_TYPE} must be above zero, got {market}"
    );

    // (case, the maximum price the request names)
    let rows: Vec<(&str, Option<String>)> = vec![
        ("no maximum price at all", None),
        ("a maximum far below the price", Some(price(market / 10.0))),
        ("a maximum just below the price", Some(price(market - 0.0001))),
        ("a maximum exactly at the price", Some(price(market))),
        ("a maximum above the price", Some(price(market * 2.0))),
    ];

    for (case, maximum) in rows {
        let mut extra = vec![p("LaunchSpecification.Placement.AvailabilityZone", zone.clone())];
        if let Some(maximum) = maximum.as_deref() {
            extra.push(p("SpotPrice", maximum));
        }
        let body = ok(
            &router,
            case,
            "RequestSpotInstances",
            &request_params(&image_id, &extra),
        )
        .await;
        let request_id = element(&body, "spotInstanceRequestId")
            .unwrap_or_else(|| panic!("case {case:?}: the call must report its request: {body}"));

        let (state, code, instance_id) = poll_until_evaluated(&router, case, &request_id).await;
        let priced_out = maximum
            .as_deref()
            .and_then(|maximum| maximum.parse::<f64>().ok())
            .is_some_and(|maximum| maximum < market);
        if priced_out {
            assert_eq!(
                (state.as_str(), code.as_str()),
                ("open", "price-too-low"),
                "case {case:?}: a maximum below {market} must leave the request open and \
                 priced out, got {state:?} / {code:?}"
            );
            assert_eq!(
                instance_id, None,
                "case {case:?}: a request that was never fulfilled must launch no instance"
            );
            continue;
        }
        assert_eq!(
            (state.as_str(), code.as_str()),
            ("active", "fulfilled"),
            "case {case:?}: a maximum at or above {market} must be fulfilled, got {state:?} / \
             {code:?}"
        );
        assert!(
            instance_id.is_some(),
            "case {case:?}: a fulfilled request must report the instance it launched"
        );
    }
}

/// RULE 3: the instance a fulfilled request launches reports the `spot` lifecycle, the request
/// that launched it, and the IAM instance profile the launch specification named.
///
/// The rows span the specification naming a profile and naming none, so the lifecycle is
/// asserted of both and the profile only where one was asked for.
#[tokio::test]
async fn a_fulfilled_requests_instance_reports_the_spot_lifecycle_and_its_profile() {
    let (router, engine) = super::ec2_query::emulator();
    let image_id = seed_image(&engine, "spot-lifecycle");

    // (case, the instance profile name the specification names)
    let rows: Vec<(&str, Option<&str>)> = vec![
        ("a specification naming no instance profile", None),
        ("a specification naming an instance profile", Some("spot-suite-profile")),
    ];

    for (case, profile) in rows {
        let mut extra = Vec::new();
        if let Some(profile) = profile {
            extra.push(p("LaunchSpecification.IamInstanceProfile.Name", profile));
        }
        let body = ok(
            &router,
            case,
            "RequestSpotInstances",
            &request_params(&image_id, &extra),
        )
        .await;
        let request_id = element(&body, "spotInstanceRequestId")
            .unwrap_or_else(|| panic!("case {case:?}: the call must report its request: {body}"));

        let (state, code, instance_id) = poll_until_evaluated(&router, case, &request_id).await;
        assert_eq!(
            (state.as_str(), code.as_str()),
            ("active", "fulfilled"),
            "case {case:?}: the request must be fulfilled, got {state:?} / {code:?}"
        );
        let instance_id = instance_id
            .unwrap_or_else(|| panic!("case {case:?}: a fulfilled request must name its instance"));

        let described = ok(
            &router,
            case,
            "DescribeInstances",
            &indexed("InstanceId", &[&instance_id]),
        )
        .await;
        assert_eq!(
            element(&described, "instanceLifecycle").as_deref(),
            Some("spot"),
            "case {case:?}: the instance must report the spot lifecycle, got {described}"
        );
        assert_eq!(
            element(&described, "spotInstanceRequestId").as_deref(),
            Some(request_id.as_str()),
            "case {case:?}: the instance must report the request that launched it, got \
             {described}"
        );
        match profile {
            None => assert!(
                element(&described, "iamInstanceProfile").is_none(),
                "case {case:?}: an instance nothing gave a profile must report none, got \
                 {described}"
            ),
            Some(profile) => {
                let arn = super::wire::document(case, &described)
                    .descendants()
                    .find(|node| node.is_element() && node.tag_name().name() == "iamInstanceProfile")
                    .and_then(|node| super::wire::child_text(node, "arn"))
                    .unwrap_or_else(|| {
                        panic!("case {case:?}: the instance must report its profile: {described}")
                    });
                assert!(
                    arn.ends_with(profile),
                    "case {case:?}: the profile must be the one the specification named, got \
                     {arn:?}"
                );
            }
        }
    }
}
