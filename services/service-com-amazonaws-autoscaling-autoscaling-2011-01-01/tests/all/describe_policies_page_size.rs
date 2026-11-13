// SPDX-License-Identifier: CC0-1.0

//! How a read of the scaling policies pages, over the awsQuery wire.
//!
//! The rule under test: a read bounds its page by the request's `MaxRecords` within the
//! range the model documents — "The maximum number of items to be returned with each
//! call. The default value is 50 and the maximum value is 100"
//! (`smithy_query(query="doc",shape="DescribePoliciesType",member="MaxRecords")`) — so a
//! read naming no page size reports at most the default and the policies past that page
//! are reached through the `NextToken` it answers, a read naming a page size inside the
//! range reports at most that many, and a page size outside the range is refused with
//! HTTP 400 `ValidationError` naming the member and the bound its value missed, leaving
//! what a later caller reads untouched.
//!
//! The tables span what the rule generalises over — the page size: a read naming none,
//! each published bound (1 and 100) and one past each of them (0 and 101) — rather than
//! the single page size any one caller happens to ask for. The group holds one more
//! policy than the default page size, which is what makes the default observable.

use std::sync::Arc;

use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_autoscaling_autoscaling_2011_01_01 as autoscaling;

use crate::support::{borrowed, new_engine, p, query, tag_value, tag_values, xml_unescape};

/// The group every policy in this suite belongs to.
const GROUP: &str = "asg-paged";

/// The launch source the group is created from — a launch TEMPLATE, because no account can
/// create a launch configuration (see `launch_configurations_are_unavailable.rs`).
const LAUNCH_SOURCE: (&str, &str) = ("LaunchTemplate.LaunchTemplateName", "lt-1");

/// How many policies the group holds: ONE MORE than the documented default page size of
/// 50, so a read that names no `MaxRecords` has a policy left over for a second page.
const POLICY_COUNT: usize = 51;

/// The documented default page size: "The default value is 50 and the maximum value is
/// 100" (`DescribePoliciesType$MaxRecords`).
const DEFAULT_PAGE: usize = 50;

/// One accepted read: the case, the `MaxRecords` it carries, how many policies its page
/// must then hold, and whether it must answer a `NextToken`.
struct PageCase {
    case: &'static str,
    max_records: Option<i32>,
    page: usize,
    more: bool,
}

/// One refused read: the case, the `MaxRecords` it carries, and the constraint the
/// refusal must state for it.
struct RefusedCase {
    case: &'static str,
    max_records: i32,
    constraint: &'static str,
}

/// A router carrying Amazon EC2 Auto Scaling alone: a simple-scaling policy mints no
/// CloudWatch alarm, so no other service's store is reached by this suite.
fn router() -> MultiServiceRouter {
    let engine = new_engine();
    let mut router = MultiServiceRouter::new();
    router.register_service(autoscaling::EmulatedService::new(Arc::clone(&engine)));
    router
}

/// The name of the `index`-th policy, zero-padded so the name order a read reports is the
/// order these are written here.
fn policy_name(index: usize) -> String {
    format!("policy-{index:02}")
}

/// POST one `DescribePolicies` for [`GROUP`] and answer its status and body.
async fn describe(
    router: &MultiServiceRouter,
    max_records: Option<i32>,
    next_token: Option<&str>,
) -> (u16, String) {
    let mut params = vec![p("AutoScalingGroupName", GROUP)];
    if let Some(max_records) = max_records {
        params.push(p("MaxRecords", max_records.to_string()));
    }
    if let Some(token) = next_token {
        params.push(p("NextToken", token));
    }
    query(router, "DescribePolicies", &borrowed(&params)).await
}

/// A router holding [`GROUP`] with [`POLICY_COUNT`] simple-scaling policies, plus the
/// names those policies carry in the order a read reports them.
async fn seeded() -> (MultiServiceRouter, Vec<String>) {
    let router = router();
    let (status, body) = query(
        &router,
        "CreateAutoScalingGroup",
        &[
            ("AutoScalingGroupName", GROUP),
            LAUNCH_SOURCE,
            ("MinSize", "0"),
            ("MaxSize", "4"),
            ("DesiredCapacity", "0"),
            ("AvailabilityZones.member.1", "us-west-2a"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "the group {GROUP:?} must be created, got {status} with {body:?}"
    );

    let names: Vec<String> = (0..POLICY_COUNT).map(policy_name).collect();
    for name in &names {
        let (status, body) = query(
            &router,
            "PutScalingPolicy",
            &[
                ("AutoScalingGroupName", GROUP),
                ("PolicyName", name.as_str()),
                ("PolicyType", "SimpleScaling"),
                ("AdjustmentType", "ChangeInCapacity"),
                ("ScalingAdjustment", "1"),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "the put of {name:?} must succeed, got {status} with {body:?}"
        );
    }
    (router, names)
}

/// The `Code` and the decoded `Message` of a refusal body.
fn refusal(body: &str) -> (String, String) {
    (
        tag_value(body, "Code"),
        xml_unescape(&tag_value(body, "Message")),
    )
}

/// A read's page is bounded by the `MaxRecords` it names, inside the documented range, and
/// the policies past that page are the ones its `NextToken` resumes at.
#[tokio::test]
async fn a_read_bounds_its_page_by_the_max_records_it_names() {
    let (router, names) = seeded().await;

    let cases = [
        PageCase {
            case: "no page size at all takes the documented default",
            max_records: None,
            page: DEFAULT_PAGE,
            more: true,
        },
        PageCase {
            case: "the smallest page the service honours",
            max_records: Some(1),
            page: 1,
            more: true,
        },
        PageCase {
            case: "a page size inside the range",
            max_records: Some(2),
            page: 2,
            more: true,
        },
        PageCase {
            case: "a page size equal to the documented default",
            max_records: Some(DEFAULT_PAGE as i32),
            page: DEFAULT_PAGE,
            more: true,
        },
        PageCase {
            case: "a page size that holds every policy exactly",
            max_records: Some(POLICY_COUNT as i32),
            page: POLICY_COUNT,
            more: false,
        },
        PageCase {
            case: "the largest page the service honours",
            max_records: Some(100),
            page: POLICY_COUNT,
            more: false,
        },
    ];

    for PageCase {
        case,
        max_records,
        page,
        more,
    } in cases
    {
        let (status, body) = describe(&router, max_records, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with {body:?}"
        );
        let reported = tag_values(&body, "PolicyName");
        assert_eq!(
            reported,
            names[..page],
            "case {case:?}: expected the first {page} policies, got {reported:?}"
        );
        let token = tag_value(&body, "NextToken");
        assert_eq!(
            !token.is_empty(),
            more,
            "case {case:?}: expected a NextToken present={more}, got {token:?}"
        );
        if !more {
            continue;
        }
        // The token resumes where the page ended: the next page starts at the policy the
        // first one stopped before.
        let (status, body) = describe(&router, max_records, Some(&token)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the resumed read must answer HTTP 200, got {status} with {body:?}"
        );
        let resumed = tag_values(&body, "PolicyName");
        let end = (page + page).min(POLICY_COUNT);
        assert_eq!(
            resumed,
            names[page..end],
            "case {case:?}: the resumed page must carry the policies after the first, got {resumed:?}"
        );
    }
}

/// A page size outside the documented range is refused with the sentence the service
/// states for a member value outside its bounds, and the policies a later caller reads are
/// exactly the ones that were there before.
#[tokio::test]
async fn a_page_size_outside_the_documented_range_is_refused_and_changes_nothing() {
    let (router, names) = seeded().await;

    let cases = [
        RefusedCase {
            case: "one below the smallest page",
            max_records: 0,
            constraint: "Member must have value greater than or equal to 1",
        },
        RefusedCase {
            case: "a negative page size",
            max_records: -1,
            constraint: "Member must have value greater than or equal to 1",
        },
        RefusedCase {
            case: "one past the largest page",
            max_records: 101,
            constraint: "Member must have value less than or equal to 100",
        },
        RefusedCase {
            case: "far past the largest page",
            max_records: 4_096,
            constraint: "Member must have value less than or equal to 100",
        },
    ];

    for RefusedCase {
        case,
        max_records,
        constraint,
    } in cases
    {
        let (status, body) = describe(&router, Some(max_records), None).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {body:?}"
        );
        let (code, message) = refusal(&body);
        assert_eq!(
            code, "ValidationError",
            "case {case:?}: expected the ValidationError code, got {code:?}"
        );
        assert_eq!(
            message,
            format!(
                "1 validation error detected: Value '{max_records}' at 'maxRecords' failed to \
                 satisfy constraint: {constraint}"
            ),
            "case {case:?}: expected the bound the value missed to be stated, got {message:?}"
        );
    }

    // A refused read, and a read of any kind, leaves the collection as it was.
    let (status, body) = describe(&router, Some(100), None).await;
    assert_eq!(
        status, 200,
        "the read after the refusals must answer HTTP 200, got {status} with {body:?}"
    );
    let reported = tag_values(&body, "PolicyName");
    assert_eq!(
        reported, names,
        "every policy put must still be reported, got {reported:?}"
    );
}

/// The page walk reaches every policy of the group exactly once, whatever page size it
/// walks with — including the default no request named.
#[tokio::test]
async fn the_page_walk_reaches_every_policy_exactly_once() {
    let (router, names) = seeded().await;

    for max_records in [None, Some(1), Some(7), Some(DEFAULT_PAGE as i32), Some(100)] {
        let case = format!("page size {max_records:?}");
        let limit = max_records.map_or(DEFAULT_PAGE, |value| value as usize);
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        // A walk of a group holding POLICY_COUNT policies takes at most that many pages,
        // so a walk that has not ended by then is not converging.
        for _ in 0..=POLICY_COUNT {
            let (status, body) = describe(&router, max_records, token.as_deref()).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with {body:?}"
            );
            let page = tag_values(&body, "PolicyName");
            assert!(
                page.len() <= limit,
                "case {case:?}: a page holds at most {limit} policies, got {page:?}"
            );
            walked.extend(page);
            let next = tag_value(&body, "NextToken");
            if next.is_empty() {
                token = None;
                break;
            }
            token = Some(next);
        }
        assert!(
            token.is_none(),
            "case {case:?}: the walk must end, got a NextToken after {} policies",
            walked.len()
        );
        assert_eq!(
            walked, names,
            "case {case:?}: the walk must reach every policy of the group exactly once"
        );
    }
}
