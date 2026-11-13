// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DescribeInstanceStatus`, driven over the wire the service actually
//! speaks — `ec2Query`, through the crate's shared [`super::ec2_query`] harness.
//!
//! The rules under test are:
//!
//! 1. the operation reports each instance's STATUS CHECKS — the system's and the instance's
//!    own, each a summary with the reachability detail it was decided from — beside that
//!    instance's state and the zone it is in; and a check is owed to the CLOCK, so an
//!    instance whose guest has just come up is `initializing` and passes a span later,
//!    whether or not anybody polls;
//! 2. "by default, only running instances are described, unless you specifically indicate to
//!    return the status of all instances" (`IncludeAllInstances`), and an instance that is
//!    not running has no check that applies: its status is `not-applicable`, with no detail
//!    beside it, because `StatusType` models no value for a check that does not apply;
//! 3. the filters select on what the status REPORTS, a filter naming a fact no instance has
//!    selects nothing, and a filter name the operation does not document is refused rather
//!    than ignored;
//! 4. the answer is PAGED: `MaxResults` bounds the page, its `NextToken` resumes after it, a
//!    token this service did not mint is refused, a page size below the published minimum is
//!    refused, and `MaxResults` cannot be combined with explicit instance ids;
//! 5. an instance id the operation cannot resolve is refused, and `DryRun` answers the
//!    permission probe without reporting a status.
//!
//! Each table row is a distinct input along what the rule generalises over — the state the
//! instance is parked in, which member a filter selects on, and the page sizes a walk is
//! asked for — so no row is the specification.

use std::time::{Duration, Instant};

use ec2::state::{ComAmazonawsEc2Amazonec2State as Ec2State, ImageSpec};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, elements, error_code, member};

/// The Region every call here is signed for.
const REGION: &str = "us-west-2";

/// How long a poll waits for a status the clock owes: generously above the service's own
/// spans, so a loaded host reaches the same answer.
const SETTLE_BUDGET: Duration = Duration::from_secs(30);

/// How often that poll looks again.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// The smallest page size this operation serves, as the EC2 API Reference publishes it for
/// `MaxResults`. Spelled out here rather than imported, so a changed floor fails this suite.
const MIN_MAX_RESULTS: usize = 5;

/// A `(key, value)` request parameter.
fn p(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

/// The extra request parameters one row sends.
type Params = Vec<(String, String)>;

/// One row of the include-all table: what it covers, what it asks for, and the instances it
/// must report.
type SelectionRow = (&'static str, Params, Vec<String>);

/// One row of the page-refusal table: what it covers, what it asks for, and the code the
/// refusal carries.
type RefusalRow = (&'static str, Params, &'static str);

/// One ec2Query call through the shared harness.
async fn api(
    router: &MultiServiceRouter,
    action: &str,
    params: &[(String, String)],
) -> (u16, String) {
    call(router, REGION, action, params).await
}

/// One emulator over its own engine, with one AMI of the calling account seeded the way a
/// register operation would store it — answered beside its image id.
fn emulator() -> (MultiServiceRouter, String) {
    let (router, engine) = super::ec2_query::emulator();
    let image_id = {
        let store = engine.state::<Ec2State>();
        let mut guard = store.lock();
        guard.insert_image(ImageSpec {
            name: "instance-status".to_string(),
            ..ImageSpec::default()
        })
    };
    (router, image_id)
}

/// Launch `count` instances from `image_id`, answering their ids in launch order.
async fn launch(router: &MultiServiceRouter, image_id: &str, count: usize) -> Vec<String> {
    let (status, body) = api(
        router,
        "RunInstances",
        &[
            p("ImageId", image_id),
            p("InstanceType", "t3.micro"),
            p("MinCount", count.to_string()),
            p("MaxCount", count.to_string()),
        ],
    )
    .await;
    assert_eq!(status, 200, "RunInstances must succeed, got {status}: {body}");
    let ids = elements(&body, "instanceId");
    assert_eq!(
        ids.len(),
        count,
        "RunInstances must report the {count} instance(s) it minted: {body}"
    );
    ids
}

/// One reported instance status, as the response carries it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Reported {
    instance_id: String,
    state: String,
    zone: String,
    zone_id: String,
    instance_status: String,
    instance_details: Vec<(String, String)>,
    system_status: String,
    system_details: Vec<(String, String)>,
}

/// The first child element of `node` named `name`.
fn child<'a, 'input>(
    node: roxmltree::Node<'a, 'input>,
    name: &str,
) -> Option<roxmltree::Node<'a, 'input>> {
    node.children()
        .find(|child| child.is_element() && child.tag_name().name() == name)
}

/// The text of that child, or the empty string when there is none.
fn text(node: roxmltree::Node<'_, '_>, name: &str) -> String {
    child(node, name)
        .and_then(|found| found.text().map(str::to_string))
        .unwrap_or_default()
}

/// One status summary of a reported item: its status and its details as `(name, status)`.
fn summary(node: roxmltree::Node<'_, '_>, name: &str) -> (String, Vec<(String, String)>) {
    let Some(found) = child(node, name) else {
        return (String::new(), Vec::new());
    };
    let details = child(found, "details")
        .map(|details| {
            details
                .children()
                .filter(|item| item.is_element() && item.tag_name().name() == "item")
                .map(|item| (text(item, "name"), text(item, "status")))
                .collect::<Vec<(String, String)>>()
        })
        .unwrap_or_default();
    (text(found, "status"), details)
}

/// Every instance status a response reports, in the order it reports them.
fn reported(case: &str, body: &str) -> Vec<Reported> {
    let document = roxmltree::Document::parse(body)
        .unwrap_or_else(|error| panic!("case {case:?}: the body must be XML ({error}): {body}"));
    let Some(set) = document
        .root_element()
        .children()
        .find(|node| node.is_element() && node.tag_name().name() == "instanceStatusSet")
    else {
        return Vec::new();
    };
    set.children()
        .filter(|node| node.is_element() && node.tag_name().name() == "item")
        .map(|item| {
            let (instance_status, instance_details) = summary(item, "instanceStatus");
            let (system_status, system_details) = summary(item, "systemStatus");
            Reported {
                instance_id: text(item, "instanceId"),
                state: child(item, "instanceState")
                    .map(|state| text(state, "name"))
                    .unwrap_or_default(),
                zone: text(item, "availabilityZone"),
                zone_id: text(item, "availabilityZoneId"),
                instance_status,
                instance_details,
                system_status,
                system_details,
            }
        })
        .collect()
}

/// One `DescribeInstanceStatus` call, having pinned it answered 200: the whole body.
async fn describe(router: &MultiServiceRouter, case: &str, params: &[(String, String)]) -> String {
    let (status, body) = api(router, "DescribeInstanceStatus", params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeInstanceStatus must succeed, got {status}: {body}"
    );
    body
}

/// The status one instance reports now, or `None` when the answer carries none for it.
async fn status_of(
    router: &MultiServiceRouter,
    case: &str,
    instance_id: &str,
    extra: &[(String, String)],
) -> Option<Reported> {
    let mut params = vec![p("InstanceId.1", instance_id)];
    params.extend_from_slice(extra);
    let body = describe(router, case, &params).await;
    reported(case, &body).into_iter().next()
}

/// Look again until `instance_id` reports `ok` for both checks, or the budget runs out —
/// which is a failure, since the clock owes that status.
async fn poll_until_checks_pass(
    router: &MultiServiceRouter,
    case: &str,
    instance_id: &str,
) -> Reported {
    let deadline = Instant::now() + SETTLE_BUDGET;
    loop {
        let held = status_of(router, case, instance_id, &[]).await;
        if let Some(held) = held.clone() {
            if held.instance_status == "ok" && held.system_status == "ok" {
                return held;
            }
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the status checks the clock owes never passed; last read {held:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Look again until `instance_id` reports the instance state `wanted`.
async fn poll_until_state(
    router: &MultiServiceRouter,
    case: &str,
    instance_id: &str,
    wanted: &str,
) {
    let deadline = Instant::now() + SETTLE_BUDGET;
    loop {
        let state = super::ec2_query::described_instance_state(router, REGION, instance_id).await;
        if state == wanted {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected the instance to reach {wanted:?}, last read {state:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// RULE 1: the status checks of an instance whose guest has just come up are `initializing`,
/// and the clock owes them passing a span later — reported beside the instance's state and
/// the zone it is in.
#[tokio::test]
async fn the_status_checks_the_clock_owes_settle_from_initializing_to_ok() {
    let (router, image_id) = emulator();
    let instance_id = launch(&router, &image_id, 1).await.remove(0);

    // Read straight away, while the launch's own clock-owed move is still outstanding: a
    // check cannot have passed before the guest it is about has come up, and an instance
    // that is still starting is only reported when all instances are asked for.
    let fresh = status_of(
        &router,
        "straight after the launch",
        &instance_id,
        &[p("IncludeAllInstances", "true")],
    )
    .await
    .unwrap_or_else(|| panic!("straight after the launch: the instance must be reported"));
    assert_eq!(
        (fresh.instance_status.as_str(), fresh.system_status.as_str()),
        ("initializing", "initializing"),
        "straight after the launch: expected both checks to be initializing, got {fresh:?}"
    );
    assert_eq!(
        fresh.instance_details,
        [("reachability".to_string(), "initializing".to_string())],
        "straight after the launch: expected the reachability detail to be initializing, \
         got {fresh:?}"
    );

    let settled = poll_until_checks_pass(&router, "once the checks have passed", &instance_id).await;
    assert_eq!(
        settled.instance_details,
        [("reachability".to_string(), "passed".to_string())],
        "once the checks have passed: expected the reachability detail to have passed, got \
         {settled:?}"
    );
    assert_eq!(
        settled.system_details, settled.instance_details,
        "once the checks have passed: expected both checks to report the same detail, got \
         {settled:?}"
    );
    assert_eq!(
        settled.instance_id, instance_id,
        "the status must name its own instance, got {settled:?}"
    );
    assert_eq!(
        settled.state, "running",
        "the status reports the state the instance is in, got {settled:?}"
    );
    assert!(
        settled.zone.starts_with(REGION),
        "the status reports the zone the instance is in, got {settled:?}"
    );
    assert!(
        !settled.zone_id.is_empty(),
        "the status reports that zone's id, got {settled:?}"
    );
    // Reading a status is a READ: two reads of a settled instance answer the same thing.
    let again = status_of(&router, "a second read", &instance_id, &[]).await;
    assert_eq!(
        again.as_ref(),
        Some(&settled),
        "two reads of one settled instance must answer the same status, got {again:?}"
    );
}

/// RULE 2: only running instances are described unless the request asks for all of them, and
/// a check that does not apply reports `not-applicable` with no detail.
#[tokio::test]
async fn only_running_instances_are_described_unless_all_are_asked_for() {
    let (router, image_id) = emulator();
    let ids = launch(&router, &image_id, 2).await;
    let (running, stopped) = (ids[0].clone(), ids[1].clone());
    poll_until_checks_pass(&router, "the running instance", &running).await;

    let (status, body) = api(&router, "StopInstances", &[p("InstanceId.1", &stopped)]).await;
    assert_eq!(status, 200, "StopInstances must succeed, got {status}: {body}");
    poll_until_state(&router, "the stopped instance", &stopped, "stopped").await;

    // (case, what the request asks for, the instances it must report)
    let rows: [SelectionRow; 3] = [
        ("the default", vec![], vec![running.clone()]),
        (
            "IncludeAllInstances=false",
            vec![p("IncludeAllInstances", "false")],
            vec![running.clone()],
        ),
        (
            "IncludeAllInstances=true",
            vec![p("IncludeAllInstances", "true")],
            vec![running.clone(), stopped.clone()],
        ),
    ];
    for (case, extra, expected) in rows {
        let body = describe(&router, case, &extra).await;
        let found: Vec<String> = reported(case, &body)
            .into_iter()
            .map(|status| status.instance_id)
            .collect();
        assert_eq!(
            found, expected,
            "case {case:?}: expected {expected:?} to be reported, got {found:?}"
        );
    }

    let not_applicable = status_of(
        &router,
        "a stopped instance",
        &stopped,
        &[p("IncludeAllInstances", "true")],
    )
    .await
    .unwrap_or_else(|| panic!("a stopped instance must be reported when all are asked for"));
    assert_eq!(
        (
            not_applicable.instance_status.as_str(),
            not_applicable.system_status.as_str()
        ),
        ("not-applicable", "not-applicable"),
        "a stopped instance has no check that applies, got {not_applicable:?}"
    );
    assert!(
        not_applicable.instance_details.is_empty() && not_applicable.system_details.is_empty(),
        "a check that does not apply reports no detail, got {not_applicable:?}"
    );
    assert_eq!(
        not_applicable.state, "stopped",
        "the status reports the state the instance is in, got {not_applicable:?}"
    );
}

/// RULE 3: a filter selects on what the status reports; one naming a fact no instance has
/// selects nothing; a name the operation does not document is refused.
#[tokio::test]
async fn a_filter_selects_on_what_the_status_reports() {
    let (router, image_id) = emulator();
    let ids = launch(&router, &image_id, 2).await;
    let (running, stopped) = (ids[0].clone(), ids[1].clone());
    poll_until_checks_pass(&router, "the running instance", &running).await;
    let (status, body) = api(&router, "StopInstances", &[p("InstanceId.1", &stopped)]).await;
    assert_eq!(status, 200, "StopInstances must succeed, got {status}: {body}");
    poll_until_state(&router, "the stopped instance", &stopped, "stopped").await;

    // (case, the filter name, its value, whether all instances are asked for, the instances
    //  the filter must select)
    let rows: [(&str, &str, String, bool, Vec<String>); 8] = [
        (
            "a passing instance check",
            "instance-status.status",
            "ok".to_string(),
            true,
            vec![running.clone()],
        ),
        (
            "a passing system check",
            "system-status.status",
            "ok".to_string(),
            true,
            vec![running.clone()],
        ),
        (
            "the reachability detail",
            "instance-status.reachability",
            "passed".to_string(),
            true,
            vec![running.clone()],
        ),
        (
            "a check that does not apply",
            "instance-status.status",
            "not-applicable".to_string(),
            true,
            vec![stopped.clone()],
        ),
        (
            "the instance state",
            "instance-state-name",
            "stopped".to_string(),
            true,
            vec![stopped.clone()],
        ),
        (
            "the instance state code",
            "instance-state-code",
            "16".to_string(),
            true,
            vec![running.clone()],
        ),
        (
            "a status no instance is in",
            "instance-status.status",
            "impaired".to_string(),
            true,
            vec![],
        ),
        (
            "a scheduled event no instance has",
            "event.code",
            "instance-reboot".to_string(),
            true,
            vec![],
        ),
    ];
    for (case, name, value, all, expected) in rows {
        let mut params = vec![
            p("Filter.1.Name", name),
            p("Filter.1.Value.1", value.clone()),
        ];
        if all {
            params.push(p("IncludeAllInstances", "true"));
        }
        let body = describe(&router, case, &params).await;
        let found: Vec<String> = reported(case, &body)
            .into_iter()
            .map(|status| status.instance_id)
            .collect();
        assert_eq!(
            found, expected,
            "case {case:?}: expected the filter {name}={value} to select {expected:?}, got \
             {found:?}"
        );
    }

    // A zone filter selects on the zone the instance reports, read off the status itself.
    let zone = status_of(&router, "the zone", &running, &[])
        .await
        .map(|status| status.zone)
        .unwrap_or_default();
    let body = describe(
        &router,
        "the zone the instance is in",
        &[p("Filter.1.Name", "availability-zone"), p("Filter.1.Value.1", &zone)],
    )
    .await;
    assert!(
        reported("the zone the instance is in", &body)
            .iter()
            .any(|status| status.instance_id == running),
        "a zone filter must select the instance in that zone, got {body}"
    );

    // A filter name this operation does not document is refused, never ignored.
    let (status, body) = api(
        &router,
        "DescribeInstanceStatus",
        &[
            p("Filter.1.Name", "not-a-filter"),
            p("Filter.1.Value.1", "x"),
        ],
    )
    .await;
    assert_eq!(status, 400, "an undocumented filter must be refused, got {status}: {body}");
    assert_eq!(
        error_code(&body),
        "InvalidParameterValue",
        "expected InvalidParameterValue, got {body}"
    );
    assert!(
        super::ec2_query::error_message(&body).contains("not-a-filter"),
        "the refusal must name the filter, got {body}"
    );
}

/// RULE 4: the answer is paged — `MaxResults` bounds the page, its token resumes after it,
/// and the ways a page can be asked for wrongly are each refused.
#[tokio::test]
async fn the_page_is_bounded_and_resumed_by_its_token() {
    let (router, image_id) = emulator();
    let ids = launch(&router, &image_id, MIN_MAX_RESULTS + 1).await;
    for instance_id in &ids {
        poll_until_checks_pass(&router, "every launched instance", instance_id).await;
    }

    let body = describe(
        &router,
        "the first page",
        &[p("MaxResults", MIN_MAX_RESULTS.to_string())],
    )
    .await;
    let first: Vec<String> = reported("the first page", &body)
        .into_iter()
        .map(|status| status.instance_id)
        .collect();
    assert_eq!(
        first.len(),
        MIN_MAX_RESULTS,
        "the page must carry the size the request asked for, got {first:?}"
    );
    let token = member(&body, "nextToken")
        .unwrap_or_else(|| panic!("a page with more to come must carry a token: {body}"));
    let body = describe(
        &router,
        "the second page",
        &[
            p("MaxResults", MIN_MAX_RESULTS.to_string()),
            p("NextToken", &token),
        ],
    )
    .await;
    let second: Vec<String> = reported("the second page", &body)
        .into_iter()
        .map(|status| status.instance_id)
        .collect();
    assert_eq!(
        second.len(),
        ids.len() - MIN_MAX_RESULTS,
        "the next page must carry what the first left, got {second:?}"
    );
    assert!(
        first.iter().all(|held| !second.contains(held)),
        "the two pages must not overlap, got {first:?} then {second:?}"
    );
    assert_eq!(
        member(&body, "nextToken"),
        None,
        "the last page of the walk must carry no token: {body}"
    );

    // (case, the parameters, the code the refusal carries)
    let rows: [RefusalRow; 3] = [
        (
            "a token this service did not mint",
            vec![p("NextToken", "not-a-token")],
            "InvalidPaginationToken",
        ),
        (
            "a page size below the published minimum",
            vec![p("MaxResults", (MIN_MAX_RESULTS - 1).to_string())],
            "InvalidParameterValue",
        ),
        (
            "a page size beside explicit instance ids",
            vec![
                p("InstanceId.1", ids[0].clone()),
                p("MaxResults", MIN_MAX_RESULTS.to_string()),
            ],
            "InvalidParameterCombination",
        ),
    ];
    for (case, params, expected) in rows {
        let (status, body) = api(&router, "DescribeInstanceStatus", &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            expected,
            "case {case:?}: expected {expected}, got {body}"
        );
    }
}

/// RULE 5: an instance id the operation cannot resolve is refused, and `DryRun` answers the
/// probe without reporting a status.
#[tokio::test]
async fn an_unresolvable_id_is_refused_and_a_dry_run_reports_nothing() {
    let (router, image_id) = emulator();
    let known = launch(&router, &image_id, 1).await.remove(0);

    // (case, the id, the code the refusal carries)
    //
    // The two halves of EC2's unresolvable-id reading: a LONG-form id this service never
    // issued is one it cannot decode, which is the malformed half, and a short-form id it
    // holds no instance for is the not-found half.
    let rows: [(&str, &str, &str); 3] = [
        (
            "a long-form id this service never issued",
            "i-0123456789abcdef0",
            "InvalidInstanceID.Malformed",
        ),
        (
            "the short form, nothing launched for it",
            "i-0abc1234",
            "InvalidInstanceID.NotFound",
        ),
        (
            "a value that is no instance id at all",
            "not-an-id",
            "InvalidInstanceID.Malformed",
        ),
    ];
    for (case, instance_id, expected) in rows {
        let (status, body) = api(
            &router,
            "DescribeInstanceStatus",
            &[p("InstanceId.1", instance_id)],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            expected,
            "case {case:?}: expected {expected}, got {body}"
        );
    }

    let (status, body) = api(
        &router,
        "DescribeInstanceStatus",
        &[p("InstanceId.1", &known), p("DryRun", "true")],
    )
    .await;
    assert_eq!(
        status, 412,
        "a DryRun must answer the probe, got {status}: {body}"
    );
    assert_eq!(
        error_code(&body),
        "DryRunOperation",
        "expected DryRunOperation, got {body}"
    );
    assert!(
        element(&body, "instanceStatusSet").is_none(),
        "a probe must report no status, got {body}"
    );
}
