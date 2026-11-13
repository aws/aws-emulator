// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for how a paged EC2 describe ENDS A WALK, driven over the wire the
//! service actually speaks: `ec2Query`, through the crate's shared [`super::ec2_query`]
//! harness.
//!
//! The rule under test is that A WALK REPORTS THE WHOLE SELECTED SET EXACTLY ONCE, IN THE
//! DESCRIBE'S OWN ID ORDER, AND ENDS WHERE THAT OPERATION'S `NextToken` SAYS IT DOES — a
//! token means "resume AFTER what I just gave you", so a caller follows it until it is
//! absent and never decides for itself that a short page was the last one. EC2 does not
//! answer the token the same way for every describe, which is why the operation is a
//! column of the table rather than an assumption:
//!
//! * `DescribeSubnets`, `DescribeVolumes` and `DescribeFlowLogs` answer a token while a
//!   further item follows the page, so the last page of a walk carries none
//!   ([`ec2::state::PageTokenRule::WhenAnotherItemFollows`]);
//! * `DescribeSecurityGroups` answers a token on every page that held groups, whether or
//!   not `MaxResults` filled it, and the walk ends on the trailing EMPTY page that token
//!   leads to ([`ec2::state::PageTokenRule::WhenThePageHeldItems`]) — the behaviour a
//!   vpc-scoped, paged read of that operation was recorded answering.
//!
//! Every table row is a distinct input along what that generalises over — which operation
//! pages, how the set's size relates to the page size (shorter than a page, exactly one
//! page, one past a page, exactly two pages) and which page size the walk uses — so the
//! recorded read is one row among many rather than the specification.

use std::future::Future;

use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, elements, filters};

/// The Region every call here is signed for.
const REGION: &str = "us-west-2";

/// A `(key, value)` request parameter.
fn p(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

/// One row of the paging table: `(case, how many items the scope holds, the page size)`.
///
/// The page sizes are ones every operation here accepts: the smallest the `range 5..1000`
/// operations publish for `MaxResults`, and one above it.
const PAGING_ROWS: [(&str, usize, usize); 6] = [
    ("set shorter than one page", 4, 5),
    ("set exactly one page", 5, 5),
    ("set one past a page", 6, 5),
    ("set exactly two pages", 10, 5),
    ("set exactly one page, page size 6", 6, 6),
    ("set one short of a page, page size 6", 5, 6),
];

/// How many pages a walk of `count` items in pages of `page_size` takes under `rule`.
///
/// Under [`ec2::state::PageTokenRule::WhenThePageHeldItems`] the walk takes one page MORE
/// than it has pages of items, because the last page that held items carries a token and
/// the page that token leads to is empty.
fn expected_pages(rule: ec2::state::PageTokenRule, count: usize, page_size: usize) -> usize {
    let full = count.div_ceil(page_size);
    match rule {
        ec2::state::PageTokenRule::WhenAnotherItemFollows => full.max(1),
        ec2::state::PageTokenRule::WhenThePageHeldItems => full + 1,
    }
}

/// One call that has to succeed, and the body it answered.
async fn ok(
    case: &str,
    router: &MultiServiceRouter,
    action: &str,
    params: &[(String, String)],
) -> String {
    let (status, body) = call(router, REGION, action, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {action}, got {status} with body {body}"
    );
    body
}

/// The text of `element_name` in the answer to a call that has to succeed.
async fn minted(
    case: &str,
    router: &MultiServiceRouter,
    action: &str,
    params: &[(String, String)],
    element_name: &str,
) -> String {
    let body = ok(case, router, action, params).await;
    element(&body, element_name)
        .unwrap_or_else(|| panic!("case {case:?}: expected a {element_name} in {body}"))
}

/// Every item id `action` reports for `scope` in ONE unpaged answer, which is the set a
/// paged walk of the same scope must report.
///
/// A request that names no `MaxResults` carries no token: `DescribeSecurityGroups` is not
/// paged at all without one ("if this parameter is not specified, then all items are
/// returned"), and `DescribeSubnets`/`DescribeVolumes` fall back to the largest page the
/// service serves — far larger than any scope this table seeds, so no page of theirs is
/// filled either.
async fn whole_scope(
    case: &str,
    router: &MultiServiceRouter,
    action: &str,
    id_element: &str,
    scope: &[(String, String)],
) -> Vec<String> {
    let body = ok(case, router, action, scope).await;
    assert!(
        elements(&body, "nextToken").is_empty(),
        "case {case:?}: an answer no MaxResults truncated carries no token, got {body}"
    );
    elements(&body, id_element)
}

/// One walk under test: which operation to page, how to read an item's id out of its
/// answer, the filter that scopes the walk, the page size, how many items the scope holds
/// and the rule that operation mints tokens by.
struct Walk<'a> {
    case: &'a str,
    action: &'a str,
    id_element: &'a str,
    scope: &'a [(String, String)],
    page_size: usize,
    count: usize,
    rule: ec2::state::PageTokenRule,
}

/// Walk the operation `walked` names, asserting on EVERY page that it holds at most
/// `page_size` items and that it carries a token exactly when the rule says one is due for
/// a scope of `count` items. Answers the ids the walk reported, in order, and how many
/// pages it took.
async fn walk(router: &MultiServiceRouter, walked: &Walk<'_>) -> (Vec<String>, usize) {
    let Walk {
        case,
        action,
        id_element,
        scope,
        page_size,
        count,
        rule,
    } = *walked;
    let mut reported: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0usize;
    loop {
        let mut params = scope.to_vec();
        params.push(p("MaxResults", page_size.to_string()));
        if let Some(carried) = &token {
            params.push(p("NextToken", carried.clone()));
        }
        let body = ok(case, router, action, &params).await;
        let page = elements(&body, id_element);
        pages += 1;
        assert!(
            page.len() <= page_size,
            "case {case:?}: page {pages} may hold at most {page_size} items, got {page:?}"
        );
        token = element(&body, "nextToken");
        let held = page.len();
        reported.extend(page);
        let due = match rule {
            // A further item follows this page exactly while the walk has not reported
            // every item of the scope yet.
            ec2::state::PageTokenRule::WhenAnotherItemFollows => reported.len() < count,
            ec2::state::PageTokenRule::WhenThePageHeldItems => held > 0,
        };
        assert_eq!(
            token.is_some(),
            due,
            "case {case:?}: page {pages} reported {held} of at most {page_size} items, \
             {reported_so_far} of {count} in all, so a token {expectation}; got {token:?}",
            reported_so_far = reported.len(),
            expectation = if due { "is due" } else { "is not due" }
        );
        assert!(
            pages <= count + 2,
            "case {case:?}: paging did not terminate, reported {reported:?}"
        );
        if token.is_none() {
            break;
        }
    }
    (reported, pages)
}

/// Run the whole [`PAGING_ROWS`] table against one operation: `seed` puts `count` items
/// into a scope of its own and answers the filter that selects exactly them.
async fn assert_table<Seed, Fut>(
    action: &str,
    id_element: &str,
    rule: ec2::state::PageTokenRule,
    seed: Seed,
) where
    Seed: Fn(MultiServiceRouter, usize, usize) -> Fut,
    Fut: Future<Output = (MultiServiceRouter, Vec<(String, String)>)>,
{
    let mut router = super::ec2_query::emulator().0;
    for (row, (case, count, page_size)) in PAGING_ROWS.into_iter().enumerate() {
        let case = format!("{action}: {case}");
        let scope;
        (router, scope) = seed(router, row, count).await;

        let whole = whole_scope(&case, &router, action, id_element, &scope).await;
        assert_eq!(
            whole.len(),
            count,
            "case {case:?}: the scope must hold {count} items, got {whole:?}"
        );
        let mut ordered = whole.clone();
        ordered.sort();
        assert_eq!(
            whole, ordered,
            "case {case:?}: a describe reports its items in id order, got {whole:?}"
        );

        let walked = Walk {
            case: &case,
            action,
            id_element,
            scope: &scope,
            page_size,
            count,
            rule,
        };
        let (reported, pages) = walk(&router, &walked).await;
        assert_eq!(
            reported, whole,
            "case {case:?}: a walk in pages of {page_size} reports every item exactly once, in \
             the order the unpaged answer reports them, got {reported:?}"
        );
        let due = expected_pages(rule, count, page_size);
        assert_eq!(
            pages, due,
            "case {case:?}: a walk of {count} items in pages of {page_size} takes {due} pages, \
             got {pages}"
        );
    }
}

/// `DescribeSubnets`: `count` subnets in a VPC of this row's own, selected by `vpc-id`.
async fn seed_subnets(
    router: MultiServiceRouter,
    row: usize,
    count: usize,
) -> (MultiServiceRouter, Vec<(String, String)>) {
    let case = format!("seeding {count} subnets");
    let vpc_cidr = format!("10.{row}.0.0/16");
    let vpc_id = minted(
        &case,
        &router,
        "CreateVpc",
        &[p("CidrBlock", vpc_cidr)],
        "vpcId",
    )
    .await;
    for index in 0..count {
        let params = [
            p("VpcId", vpc_id.clone()),
            p("CidrBlock", format!("10.{row}.{index}.0/24")),
        ];
        minted(&case, &router, "CreateSubnet", &params, "subnetId").await;
    }
    let scope = filters(&[("vpc-id", &[vpc_id.as_str()])]);
    (router, scope)
}

/// `DescribeSecurityGroups`: `count` groups of this row's own, selected by a `group-name`
/// pattern — a VPC comes with a default group of its own, and that group's inbound rule
/// NAMES a group, so scoping by name rather than by VPC keeps the row's set to the groups
/// it created.
async fn seed_security_groups(
    router: MultiServiceRouter,
    row: usize,
    count: usize,
) -> (MultiServiceRouter, Vec<(String, String)>) {
    let case = format!("seeding {count} security groups");
    let vpc_cidr = format!("10.{}.0.0/16", row + 100);
    let vpc_id = minted(
        &case,
        &router,
        "CreateVpc",
        &[p("CidrBlock", vpc_cidr)],
        "vpcId",
    )
    .await;
    for index in 0..count {
        let params = [
            p("VpcId", vpc_id.clone()),
            p("GroupName", format!("page-row-{row}-group-{index}")),
            p("GroupDescription", "a group of this rows scope"),
        ];
        minted(&case, &router, "CreateSecurityGroup", &params, "groupId").await;
    }
    // EC2's own `*` wildcard, which a filter value carries ("`?` matches exactly one
    // character and `*` matches zero or more" — `Filter$Values`).
    let pattern = format!("page-row-{row}-group-*");
    let scope = filters(&[("group-name", &[pattern.as_str()])]);
    (router, scope)
}

/// `DescribeFlowLogs`: `count` flow logs, each monitoring a VPC of its own, every one of
/// them publishing to the row's own CloudWatch Logs log group and selected by
/// `log-group-name`.
///
/// A create mints one flow log per monitored resource, and one resource carries one flow log
/// of a given configuration and destination — so the row's set is built one VPC at a time;
/// the log group is what gives the row a scope of its own, flow logs being an account's
/// rather than a VPC's.
async fn seed_flow_logs(
    router: MultiServiceRouter,
    row: usize,
    count: usize,
) -> (MultiServiceRouter, Vec<(String, String)>) {
    let case = format!("seeding {count} flow logs");
    let log_group_name = format!("/aws/vpc/page-row-{row}-flow-logs");
    for index in 0..count {
        let vpc_cidr = format!("10.{}.{index}.0/24", row + 200);
        let vpc_id = minted(
            &case,
            &router,
            "CreateVpc",
            &[p("CidrBlock", vpc_cidr)],
            "vpcId",
        )
        .await;
        // `CreateFlowLogs` answers its minted ids in a `flowLogIdSet`, whose entries are
        // `item` elements; a create that minted none would answer no `item` at all, which
        // `minted` refuses.
        let params = super::flow_logs::cloud_watch_logs_params("VPC", &vpc_id, &log_group_name);
        minted(&case, &router, "CreateFlowLogs", &params, "item").await;
    }
    let scope = filters(&[("log-group-name", &[log_group_name.as_str()])]);
    (router, scope)
}

/// `DescribeVolumes`: `count` volumes of a size of this row's own, selected by `size` —
/// volumes are not VPC-scoped, so the size is what gives each row a scope of its own.
async fn seed_volumes(
    router: MultiServiceRouter,
    row: usize,
    count: usize,
) -> (MultiServiceRouter, Vec<(String, String)>) {
    let case = format!("seeding {count} volumes");
    let size = row + 40;
    for _ in 0..count {
        let params = [
            p("AvailabilityZone", format!("{REGION}a")),
            p("VolumeType", "gp3"),
            p("Size", size.to_string()),
        ];
        minted(&case, &router, "CreateVolume", &params, "volumeId").await;
    }
    let size = size.to_string();
    let scope = filters(&[("size", &[size.as_str()])]);
    (router, scope)
}

#[tokio::test]
async fn describe_subnets_ends_a_walk_when_no_subnet_follows_the_page() {
    assert_table(
        "DescribeSubnets",
        "subnetId",
        ec2::state::PageTokenRule::WhenAnotherItemFollows,
        seed_subnets,
    )
    .await;
}

#[tokio::test]
async fn describe_volumes_ends_a_walk_when_no_volume_follows_the_page() {
    assert_table(
        "DescribeVolumes",
        "volumeId",
        ec2::state::PageTokenRule::WhenAnotherItemFollows,
        seed_volumes,
    )
    .await;
}

#[tokio::test]
async fn describe_flow_logs_ends_a_walk_when_no_flow_log_follows_the_page() {
    assert_table(
        "DescribeFlowLogs",
        "flowLogId",
        ec2::state::PageTokenRule::WhenAnotherItemFollows,
        seed_flow_logs,
    )
    .await;
}

#[tokio::test]
async fn describe_security_groups_ends_a_walk_on_a_trailing_empty_page() {
    assert_table(
        "DescribeSecurityGroups",
        "groupId",
        ec2::state::PageTokenRule::WhenThePageHeldItems,
        seed_security_groups,
    )
    .await;
}
