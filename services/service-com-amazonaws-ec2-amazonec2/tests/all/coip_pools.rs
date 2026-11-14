// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DescribeCoipPools`, driven over the wire the service actually
//! speaks: `ec2Query` — an `application/x-www-form-urlencoded` POST whose body carries
//! `Action=DescribeCoipPools`, answered with XML (README.md / OPERATIONS.md "Wire
//! protocol"). ec2Query is selected by the `ec2` signing name in the request's SigV4
//! credential scope, so every call here carries one.
//!
//! The rule under test is that the operation REPORTS THE STORE: the set it answers with is
//! the account's customer-owned address pools narrowed by the `PoolIds` and `Filters` the
//! request names and cut into `MaxResults`-sized pages, and it writes nothing. Every table
//! row is a distinct input along what that generalises over — which documented filter, how
//! many values, how many filters, which ids, which page size — so no single recorded call
//! is the specification.

use ec2::ops::types::Tag;
use ec2::state::{
    CoipPoolSpec, ComAmazonawsEc2Amazonec2State as Ec2State, MAX_ECHOED_PAGE_TOKEN_CHARS,
    MAX_FILTERS, MAX_FILTER_VALUES, MAX_FILTER_VALUE_BYTES, MAX_RESOURCE_IDS,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, elements, error_code, filters, indexed, unescaped};

/// The Region every call here is signed for, and the Region the seeded pools' ARNs name.
///
/// `pub` because the `DeleteCoipPool` suite drives the same pools through the same Region:
/// one definition of the CoIP surface's fixtures, so a delete is asserted against exactly
/// the pools this describe reports.
pub const REGION: &str = "us-east-1";

/// The local gateway route tables the fixtures below hang off: `lgw-rtb-` plus 17 hex
/// digits, the id shape `CoipPool$LocalGatewayRouteTableId` carries.
const ROUTE_TABLE_A: &str = "lgw-rtb-0aaaaaaaaaaaaaaaa";
const ROUTE_TABLE_B: &str = "lgw-rtb-0bbbbbbbbbbbbbbbb";
const ROUTE_TABLE_C: &str = "lgw-rtb-0cccccccccccccccc";
const ROUTE_TABLE_D: &str = "lgw-rtb-0dddddddddddddddd";
const ROUTE_TABLE_E: &str = "lgw-rtb-0eeeeeeeeeeeeeeee";

/// One `DescribeCoipPools` call carrying `params`, as its HTTP status and XML body.
async fn describe(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    super::ec2_query::call(router, REGION, "DescribeCoipPools", params).await
}

/// The pool ids a successful response reports, in the order it reported them.
///
/// `pub`, like [`seed`] and [`REGION`], so the `DeleteCoipPool` suite reads the store back
/// through the ONE describe wrapper of this test tree rather than a second copy of it.
pub async fn described_ids(
    router: &MultiServiceRouter,
    case: &str,
    params: &[(String, String)],
) -> Vec<String> {
    let (status, body) = describe(router, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );
    elements(&body, "poolId")
}

/// The XML of the one pool `pool_id` names. `pub` for the same reason as [`described_ids`].
pub async fn describe_one(router: &MultiServiceRouter, case: &str, pool_id: &str) -> String {
    let (status, body) = describe(router, &indexed("PoolId", &[pool_id])).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );
    body
}

/// One `MaxResults` parameter.
fn max_results_param(max_results: i64) -> Vec<(String, String)> {
    vec![("MaxResults".to_string(), max_results.to_string())]
}

/// A tag, as the seeded pools carry them.
fn tag(key: &str, value: &str) -> Tag {
    Tag {
        key: Some(key.to_string()),
        value: Some(value.to_string()),
    }
}

/// The `<item>` texts of the one `<set>…</set>` element the body carries, or none when the
/// service wrote the set empty (a self-closing element has no inner text).
///
/// `pub`, like [`describe_one`] and [`described_ids`], so the `CreateCoipPool` suite reads a
/// pool's address ranges and tags out of a body the one way this test tree does.
pub fn set_items(xml: &str, set: &str) -> Vec<String> {
    match element(xml, set) {
        Some(inner) => elements(&inner, "item"),
        None => Vec::new(),
    }
}

/// One filter-table row: the case name, the `Filters` entries the request carries as
/// `(name, values)` pairs, and the seeded positions the response must report.
type FilterCase<'a> = (&'a str, Vec<(&'a str, Vec<&'a str>)>, Vec<usize>);

/// What one seeded pool is created with, beyond the id and ARN the service mints itself.
struct PoolFixture {
    local_gateway_route_table_id: &'static str,
    pool_cidrs: Vec<&'static str>,
    tags: Vec<Tag>,
}

/// The pools the filter and paging tables are written against. They differ along every
/// attribute the documented filters select on and along what the modelled `CoipPool`
/// carries: which local gateway route table (two pools share one), how many address ranges
/// (none, one, two, three), and how many tags.
fn seeded_fixtures() -> Vec<PoolFixture> {
    vec![
        PoolFixture {
            local_gateway_route_table_id: ROUTE_TABLE_A,
            pool_cidrs: vec!["10.0.0.0/26"],
            tags: vec![tag("Name", "outpost-a")],
        },
        PoolFixture {
            local_gateway_route_table_id: ROUTE_TABLE_A,
            pool_cidrs: vec!["10.0.1.0/26", "10.0.2.0/26"],
            tags: Vec::new(),
        },
        PoolFixture {
            local_gateway_route_table_id: ROUTE_TABLE_B,
            pool_cidrs: Vec::new(),
            tags: vec![tag("Name", "outpost-b"), tag("Tier", "prod")],
        },
        PoolFixture {
            local_gateway_route_table_id: ROUTE_TABLE_C,
            pool_cidrs: vec!["192.168.10.0/24"],
            tags: vec![tag("Name", "outpost-c")],
        },
        PoolFixture {
            local_gateway_route_table_id: ROUTE_TABLE_D,
            pool_cidrs: vec!["172.16.0.0/28", "172.16.0.16/28", "172.16.0.32/28"],
            tags: Vec::new(),
        },
        PoolFixture {
            local_gateway_route_table_id: ROUTE_TABLE_E,
            pool_cidrs: vec!["10.10.0.0/24"],
            tags: vec![tag("Tier", "dev")],
        },
    ]
}

/// Write the seeded pools into the engine's store, the way a creating operation would, and
/// report the ids they were given, in store order.
pub fn seed(engine: &engine::InMemoryEngine) -> Vec<String> {
    let store = engine.state::<Ec2State>();
    let mut guard = store.lock();
    seeded_fixtures()
        .into_iter()
        .map(|fixture| {
            guard
                .insert_coip_pool(CoipPoolSpec {
                    local_gateway_route_table_id: fixture
                        .local_gateway_route_table_id
                        .to_string(),
                    region: REGION.to_string(),
                    pool_cidrs: fixture
                        .pool_cidrs
                        .iter()
                        .map(|cidr| (*cidr).to_string())
                        .collect(),
                    tags: fixture.tags,
                })
                .expect("the store holds the seeded pool")
        })
        .collect()
}

/// The seeded ids at `positions`, as the expectation tables name them.
fn ids_at(seeded: &[String], positions: &[usize]) -> Vec<String> {
    positions
        .iter()
        .map(|position| seeded[*position].clone())
        .collect()
}

// ---------------------------------------------------------------------------
// The store is what is reported
// ---------------------------------------------------------------------------

/// A fresh account holds no customer-owned address pool — a CoIP pool exists only on an
/// Outpost local gateway — so the operation answers HTTP 200 with an empty set and no
/// token, rather than refusing the call.
#[tokio::test]
async fn a_fresh_account_reports_no_coip_pool() {
    let (router, _engine) = super::ec2_query::emulator();

    let (status, body) = describe(&router, &[]).await;
    assert_eq!(
        status, 200,
        "fresh account: expected HTTP 200, got {status} with body {body}"
    );
    assert!(
        elements(&body, "poolId").is_empty(),
        "fresh account: expected no pool, got {body}"
    );
    assert!(
        elements(&body, "nextToken").is_empty(),
        "fresh account: nothing follows an empty set, so no token is due, got {body}"
    );
}

/// Every pool the store holds is reported, in store order, and a pool written after a call
/// shows up in the next one — the answer is computed from the store, not fixed.
#[tokio::test]
async fn every_stored_pool_is_reported_and_a_later_write_is_visible() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    let reported = described_ids(&router, "all pools", &[]).await;
    assert_eq!(
        reported, seeded,
        "all pools: expected every stored pool in store order, got {reported:?}"
    );

    let late_id = engine
        .state::<Ec2State>()
        .lock()
        .insert_coip_pool(CoipPoolSpec {
            local_gateway_route_table_id: ROUTE_TABLE_E.to_string(),
            region: REGION.to_string(),
            pool_cidrs: vec!["10.20.0.0/24".to_string()],
            tags: vec![tag("Name", "late")],
        })
        .expect("the store holds the later pool");

    let after = described_ids(&router, "after a later write", &[]).await;
    let mut expected = seeded.clone();
    expected.push(late_id.clone());
    assert_eq!(
        after, expected,
        "after a later write: expected {late_id:?} to be reported too, got {after:?}"
    );
}

/// Each member the modelled `CoipPool` carries round-trips out of the store: the pool id,
/// the local gateway route table, the ARN built from the id, every address range and every
/// tag it was stored with.
#[tokio::test]
async fn every_stored_member_round_trips() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    for (position, fixture) in seeded_fixtures().into_iter().enumerate() {
        let pool_id = &seeded[position];
        let case = format!("pool {position} ({pool_id})");
        let body = describe_one(&router, &case, pool_id).await;

        let reported_id = element(&body, "poolId").unwrap_or_default();
        assert_eq!(
            &reported_id, pool_id,
            "case {case:?}: expected poolId {pool_id}, got {reported_id:?}"
        );

        let route_table = element(&body, "localGatewayRouteTableId").unwrap_or_default();
        assert_eq!(
            route_table, fixture.local_gateway_route_table_id,
            "case {case:?}: expected localGatewayRouteTableId {:?}, got {route_table:?}",
            fixture.local_gateway_route_table_id
        );

        // `CoipPool$PoolArn` is "The ARN of the address pool", built from the pool's own id
        // in the Region the call was signed for and the emulator's canonical account.
        let expected_arn = ::engine::emulator_arn("ec2", REGION, &format!("coip-pool/{pool_id}"));
        let reported_arn = element(&body, "poolArn").unwrap_or_default();
        assert_eq!(
            reported_arn, expected_arn,
            "case {case:?}: expected poolArn {expected_arn}, got {reported_arn:?}"
        );

        let reported_cidrs = set_items(&body, "poolCidrSet");
        assert_eq!(
            reported_cidrs, fixture.pool_cidrs,
            "case {case:?}: expected poolCidrSet {:?}, got {reported_cidrs:?}",
            fixture.pool_cidrs
        );

        let tag_set = element(&body, "tagSet").unwrap_or_default();
        let reported_tags: Vec<(String, String)> = elements(&tag_set, "key")
            .into_iter()
            .zip(elements(&tag_set, "value"))
            .map(|(key, value)| (unescaped(&key), unescaped(&value)))
            .collect();
        let expected_tags: Vec<(String, String)> = fixture
            .tags
            .iter()
            .map(|entry| {
                (
                    entry.key.clone().unwrap_or_default(),
                    entry.value.clone().unwrap_or_default(),
                )
            })
            .collect();
        assert_eq!(
            reported_tags, expected_tags,
            "case {case:?}: expected tagSet {expected_tags:?}, got {reported_tags:?}"
        );
    }
}

/// Describing is reporting: two identical calls report the same pools and the store is left
/// as it was.
#[tokio::test]
async fn describing_does_not_change_what_a_later_caller_sees() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    let first = described_ids(&router, "first read", &[]).await;
    let second = described_ids(&router, "second read", &[]).await;
    assert_eq!(
        second, first,
        "a read must not change what a later caller sees: {first:?} then {second:?}"
    );
    assert_eq!(
        engine.state::<Ec2State>().lock().coip_pools.len(),
        seeded.len(),
        "describing pools must not write to the store"
    );
}

// ---------------------------------------------------------------------------
// PoolIds
// ---------------------------------------------------------------------------

/// `PoolIds` narrows the store to the pools it names, in store order however the request
/// ordered them and however many times it named one.
#[tokio::test]
async fn pool_ids_narrow_the_report_to_the_ids_named() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    // Rows: one id, two in store order, the same two reversed, a repeat of one id, and
    // every id at once.
    let cases: Vec<(&str, Vec<usize>, Vec<usize>)> = vec![
        ("one id", vec![0], vec![0]),
        ("two ids", vec![1, 3], vec![1, 3]),
        ("two ids, reversed", vec![3, 1], vec![1, 3]),
        ("the same id twice", vec![2, 2], vec![2]),
        ("every id", vec![0, 1, 2, 3, 4, 5], vec![0, 1, 2, 3, 4, 5]),
    ];
    for (case, requested, expected) in cases {
        let asked = ids_at(&seeded, &requested);
        let params = indexed(
            "PoolId",
            &asked.iter().map(String::as_str).collect::<Vec<&str>>(),
        );
        let reported = described_ids(&router, case, &params).await;
        assert_eq!(
            reported,
            ids_at(&seeded, &expected),
            "case {case:?}: expected the pools at {expected:?}, got {reported:?}"
        );
    }
}

/// An id that names no pool of the account is refused — whatever else the request names —
/// rather than reported as an absent row, and WHICH refusal says whether the id could name
/// a pool at all: a well-formed id nothing was created for is `InvalidPoolID.NotFound`,
/// while a value that is not a pool id at all is `InvalidPoolID.Malformed`, answered before
/// any lookup.
#[tokio::test]
async fn a_pool_id_that_names_no_pool_is_refused() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    // Rows: a well-formed id nothing was created for, a known id beside an unknown one
    // (order reversed in the next row, so the answer cannot depend on position), and then
    // one row per way a value can fail to be a pool id at all — the prefix of another
    // resource, a prefix that is no EC2 id prefix, the wrong suffix length, a non-hex digit,
    // an upper-case digit and no prefix at all.
    let unknown = "ipv4pool-coip-00000000000000000";
    let known = seeded[0].clone();
    let cases: Vec<(&str, Vec<&str>, &str)> = vec![
        ("only an unknown id", vec![unknown], "InvalidPoolID.NotFound"),
        (
            "a known id then an unknown one",
            vec![&known, unknown],
            "InvalidPoolID.NotFound",
        ),
        (
            "an unknown id then a known one",
            vec![unknown, &known],
            "InvalidPoolID.NotFound",
        ),
        (
            "an EC2 address pool id, not a customer-owned one",
            vec!["ipv4pool-ec2-00000000000000000"],
            "InvalidPoolID.Malformed",
        ),
        (
            "a prefix EC2 mints no id with",
            vec!["coip-pool-00000000000000000"],
            "InvalidPoolID.Malformed",
        ),
        (
            "a suffix of neither published length",
            vec!["ipv4pool-coip-000000000"],
            "InvalidPoolID.Malformed",
        ),
        (
            "a non-hexadecimal digit",
            vec!["ipv4pool-coip-00000000000000g"],
            "InvalidPoolID.Malformed",
        ),
        (
            "an upper-case digit",
            vec!["ipv4pool-coip-0000000000000000A"],
            "InvalidPoolID.Malformed",
        ),
        ("not an id at all", vec!["not-a-pool"], "InvalidPoolID.Malformed"),
        (
            "a malformed id beside a known one",
            vec![&known, "not-a-pool"],
            "InvalidPoolID.Malformed",
        ),
    ];
    for (case, asked, expected_code) in cases {
        let (status, body) = describe(&router, &indexed("PoolId", &asked)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code}, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// Filters
// ---------------------------------------------------------------------------

/// Each documented filter selects on the member it names, with EC2's `*`/`?` wildcards,
/// case-sensitively; several values are joined with OR and several filters with AND.
#[tokio::test]
async fn each_documented_filter_selects_on_its_member() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    // The two filter names `DescribeCoipPoolsRequest$Filters` documents:
    // smithy_query(query="doc",shape="DescribeCoipPoolsRequest",member="Filters",
    //   model_path="services/service-com-amazonaws-ec2-amazonec2/smithy-model/com-amazonaws-ec2-amazonec2.json")
    // Rows vary the value count, wildcards, case and the number of filters; two pools share
    // route table A, so a route-table filter is not just another way of naming one pool.
    let pool_0 = seeded[0].clone();
    let pool_2 = seeded[2].clone();
    let cases: Vec<FilterCase<'_>> = vec![
        (
            "pool-id, one value",
            vec![("coip-pool.pool-id", vec![pool_0.as_str()])],
            vec![0],
        ),
        (
            "pool-id, two values",
            vec![(
                "coip-pool.pool-id",
                vec![pool_2.as_str(), pool_0.as_str()],
            )],
            vec![0, 2],
        ),
        (
            "pool-id, wildcard over every pool",
            vec![("coip-pool.pool-id", vec!["ipv4pool-coip-*"])],
            vec![0, 1, 2, 3, 4, 5],
        ),
        (
            "pool-id, a value no pool carries",
            vec![(
                "coip-pool.pool-id",
                vec!["ipv4pool-coip-00000000000000000"],
            )],
            Vec::new(),
        ),
        (
            "route-table shared by two pools",
            vec![(
                "coip-pool.local-gateway-route-table-id",
                vec![ROUTE_TABLE_A],
            )],
            vec![0, 1],
        ),
        (
            "route-table, two values",
            vec![(
                "coip-pool.local-gateway-route-table-id",
                vec![ROUTE_TABLE_B, ROUTE_TABLE_A],
            )],
            vec![0, 1, 2],
        ),
        (
            "route-table, prefix wildcard",
            vec![(
                "coip-pool.local-gateway-route-table-id",
                vec!["lgw-rtb-0a*"],
            )],
            vec![0, 1],
        ),
        (
            "route-table, single-character wildcard",
            vec![(
                "coip-pool.local-gateway-route-table-id",
                vec!["lgw-rtb-0ccccccccccccccc?"],
            )],
            vec![3],
        ),
        (
            "route-table, upper case: filter values are case-sensitive",
            vec![(
                "coip-pool.local-gateway-route-table-id",
                vec!["LGW-RTB-0AAAAAAAAAAAAAAAA"],
            )],
            Vec::new(),
        ),
        (
            "a filter carrying no value matches nothing",
            vec![("coip-pool.local-gateway-route-table-id", Vec::new())],
            Vec::new(),
        ),
        (
            "two filters are joined with AND",
            vec![
                (
                    "coip-pool.local-gateway-route-table-id",
                    vec![ROUTE_TABLE_A],
                ),
                ("coip-pool.pool-id", vec![pool_0.as_str()]),
            ],
            vec![0],
        ),
        (
            "two filters that no pool satisfies together",
            vec![
                (
                    "coip-pool.local-gateway-route-table-id",
                    vec![ROUTE_TABLE_A],
                ),
                ("coip-pool.pool-id", vec![pool_2.as_str()]),
            ],
            Vec::new(),
        ),
    ];

    for (case, entries, expected) in cases {
        let as_slices: Vec<(&str, &[&str])> = entries
            .iter()
            .map(|(name, values)| (*name, values.as_slice()))
            .collect();
        let reported = described_ids(&router, case, &filters(&as_slices)).await;
        assert_eq!(
            reported,
            ids_at(&seeded, &expected),
            "case {case:?}: expected the pools at {expected:?}, got {reported:?}"
        );
    }
}

/// A filter name this operation does not document is refused with `InvalidParameterValue`
/// naming it, rather than silently matching nothing.
#[tokio::test]
async fn an_undocumented_filter_name_is_refused() {
    let (router, engine) = super::ec2_query::emulator();
    seed(&engine);

    // Rows: the unqualified spellings of the two documented names, a name documented for
    // other EC2 describes but not for this one, a tag filter this operation's `Filters`
    // member does not list, a wrong-case spelling, and free text.
    let cases = [
        "pool-id",
        "local-gateway-route-table-id",
        "tag-key",
        "tag:Name",
        "coip-pool.Pool-Id",
        "nonsense",
    ];
    for name in cases {
        let (status, body) = describe(&router, &filters(&[(name, &["anything"])])).await;
        assert_eq!(
            status, 400,
            "case {name:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValue",
            "case {name:?}: expected InvalidParameterValue, got {body}"
        );
        let message = unescaped(&element(&body, "Message").unwrap_or_default());
        assert!(
            message.contains(name),
            "case {name:?}: expected the message to name the filter, got {message:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Pagination
// ---------------------------------------------------------------------------

/// Whatever the page size, walking the pages covers every pool exactly once, in store
/// order, and the last page carries no token.
#[tokio::test]
async fn paging_covers_every_pool_exactly_once_for_each_page_size() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    // `CoipPoolMaxResults` is modelled `range 5..1000`, so the rows are its published
    // lower bound, a size that divides the store exactly, one that does not, and the size
    // that holds everything.
    for max_results in [5, 6, 1000] {
        let case = format!("MaxResults={max_results}");
        let mut params = max_results_param(max_results);
        let mut walked: Vec<String> = Vec::new();
        let mut pages = 0;
        loop {
            let (status, body) = describe(&router, &params).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with body {body}"
            );
            let page = elements(&body, "poolId");
            assert!(
                page.len() <= usize::try_from(max_results).unwrap_or_default(),
                "case {case:?}: a page may not exceed MaxResults, got {page:?}"
            );
            walked.extend(page);
            pages += 1;
            assert!(
                pages <= seeded.len() + 1,
                "case {case:?}: paging did not terminate, walked {walked:?}"
            );
            match elements(&body, "nextToken").first() {
                Some(token) => {
                    params = max_results_param(max_results);
                    params.push(("NextToken".to_string(), token.clone()));
                }
                None => break,
            }
        }
        assert_eq!(
            walked, seeded,
            "case {case:?}: expected every pool exactly once in store order, got {walked:?}"
        );
    }
}

/// A token is issued exactly when a pool follows the page, resuming from it starts at that
/// pool, and a page is cut from the FILTERED set rather than from the whole store.
#[tokio::test]
async fn a_token_is_issued_only_when_a_pool_follows_the_page() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    let (status, body) = describe(&router, &max_results_param(5)).await;
    assert_eq!(status, 200, "first page: got {status} {body}");
    let first = elements(&body, "poolId");
    assert_eq!(
        first,
        ids_at(&seeded, &[0, 1, 2, 3, 4]),
        "first page: expected the first five pools, got {first:?}"
    );
    let token = elements(&body, "nextToken")
        .first()
        .cloned()
        .unwrap_or_else(|| panic!("first page: expected a NextToken in {body}"));

    let mut params = max_results_param(5);
    params.push(("NextToken".to_string(), token));
    let (status, body) = describe(&router, &params).await;
    assert_eq!(status, 200, "second page: got {status} {body}");
    let second = elements(&body, "poolId");
    assert_eq!(
        second,
        ids_at(&seeded, &[5]),
        "second page: expected the pool after the first five, got {second:?}"
    );
    assert!(
        elements(&body, "nextToken").is_empty(),
        "second page: nothing follows it, so no token is due, got {body}"
    );

    // The same page size over a filtered set that ends inside the page: no token is due.
    let mut filtered = filters(&[(
        "coip-pool.local-gateway-route-table-id",
        &[ROUTE_TABLE_A][..],
    )]);
    filtered.extend(max_results_param(5));
    let (status, body) = describe(&router, &filtered).await;
    assert_eq!(status, 200, "filtered page: got {status} {body}");
    assert_eq!(
        elements(&body, "poolId"),
        ids_at(&seeded, &[0, 1]),
        "filtered page: expected route table A's two pools, got {body}"
    );
    assert!(
        elements(&body, "nextToken").is_empty(),
        "filtered page: the filtered set ends here, so no token is due, got {body}"
    );
}

/// `MaxResults` outside the range the model publishes is refused at the boundary; each
/// published bound itself is accepted.
#[tokio::test]
async fn max_results_is_accepted_at_its_bounds_and_refused_past_them() {
    let (router, engine) = super::ec2_query::emulator();
    seed(&engine);

    // The published bounds (`CoipPoolMaxResults`, `range 5..1000`) and one past each.
    let cases = [(4, 400), (5, 200), (1000, 200), (1001, 400)];
    for (max_results, expected_status) in cases {
        let (status, body) = describe(&router, &max_results_param(max_results)).await;
        assert_eq!(
            status, expected_status,
            "case MaxResults={max_results}: expected HTTP {expected_status}, got {status} with \
             body {body}"
        );
    }
}

/// A `NextToken` this service did not issue is refused with `InvalidPaginationToken`,
/// rather than read as a cursor.
#[tokio::test]
async fn a_next_token_this_service_did_not_issue_is_refused() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    // Rows: free text, punctuation, a hex string this service never minted, the bare
    // resource id a caller might guess the token is, and a token AT the length the message
    // echoes in full.
    let cases = [
        "not-a-token".to_string(),
        "###".to_string(),
        "6161".to_string(),
        seeded[0].clone(),
        "z".repeat(MAX_ECHOED_PAGE_TOKEN_CHARS),
    ];
    for token in cases {
        let params = vec![("NextToken".to_string(), token.clone())];
        let (status, body) = describe(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {token:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidPaginationToken",
            "case {token:?}: expected InvalidPaginationToken, got {body}"
        );
        // EC2's message for this refusal is one fixed sentence: it does not repeat the token
        // the request sent back.
        let message = unescaped(&element(&body, "Message").unwrap_or_default());
        assert_eq!(
            message,
            ec2::state::INVALID_PAGINATION_TOKEN_MESSAGE,
            "case {token:?}: expected EC2's own pagination-token sentence, got {message:?}"
        );
    }

    // A token far longer than any this service mints earns the same fixed sentence, so a
    // request's own bytes are never reflected at all.
    let oversize = "z".repeat(MAX_ECHOED_PAGE_TOKEN_CHARS * 4);
    let params = vec![("NextToken".to_string(), oversize.clone())];
    let (status, body) = describe(&router, &params).await;
    assert_eq!(
        status, 400,
        "an oversize token: expected HTTP 400, got {status} with body {body}"
    );
    let message = unescaped(&element(&body, "Message").unwrap_or_default());
    assert_eq!(
        message,
        ec2::state::INVALID_PAGINATION_TOKEN_MESSAGE,
        "an oversize token: expected EC2's own pagination-token sentence, got {message:?}"
    );
}

// ---------------------------------------------------------------------------
// The ceilings on one request's filtering work
// ---------------------------------------------------------------------------

/// Matching cost is the PRODUCT of how many filters a request carries, how many values each
/// carries, how long a value is and how many pools the store holds, and the model bounds
/// none of them (`FilterList` and `Filter$Values` carry no `@length`). So a request AT each
/// of this crate's ceilings is answered, and one PAST any of them is refused with
/// `InvalidParameterValue` and reports no pool. Each row is a distinct factor of that
/// product, named by the constant it is derived from rather than by a literal.
#[tokio::test]
async fn a_request_at_each_filtering_ceiling_is_answered_and_past_it_refused() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);
    let known = seeded[0].clone();

    let at_filter_ceiling: Vec<(&str, &[&str])> = (0..MAX_FILTERS)
        .map(|_| ("coip-pool.pool-id", &["ipv4pool-coip-*"][..]))
        .collect();
    let many_values: Vec<String> = (0..MAX_FILTER_VALUES)
        .map(|n| format!("ipv4pool-coip-{n:017x}"))
        .collect();
    let many_values: Vec<&str> = many_values.iter().map(String::as_str).collect();
    let long_value = "a".repeat(MAX_FILTER_VALUE_BYTES);
    let many_ids = vec![known.as_str(); MAX_RESOURCE_IDS];
    let accepted: Vec<(&str, Vec<(String, String)>)> = vec![
        ("MAX_FILTERS filters", filters(&at_filter_ceiling)),
        (
            "MAX_FILTER_VALUES values in one filter",
            filters(&[("coip-pool.pool-id", &many_values)]),
        ),
        (
            "a value of MAX_FILTER_VALUE_BYTES bytes",
            filters(&[("coip-pool.pool-id", &[long_value.as_str()])]),
        ),
        ("MAX_RESOURCE_IDS ids", indexed("PoolId", &many_ids)),
    ];
    for (case, params) in accepted {
        let (status, body) = describe(&router, &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 at the ceiling, got {status} with body {body}"
        );
    }

    let past_filter_ceiling: Vec<(&str, &[&str])> = (0..=MAX_FILTERS)
        .map(|_| ("coip-pool.pool-id", &["ipv4pool-coip-*"][..]))
        .collect();
    let past_value_ceiling: Vec<String> = (0..=MAX_FILTER_VALUES)
        .map(|n| format!("ipv4pool-coip-{n:017x}"))
        .collect();
    let past_value_ceiling: Vec<&str> = past_value_ceiling.iter().map(String::as_str).collect();
    let past_byte_ceiling = "a".repeat(MAX_FILTER_VALUE_BYTES + 1);
    let past_id_ceiling = vec![known.as_str(); MAX_RESOURCE_IDS + 1];
    let refused: Vec<(&str, Vec<(String, String)>)> = vec![
        ("one filter past MAX_FILTERS", filters(&past_filter_ceiling)),
        (
            "one value past MAX_FILTER_VALUES",
            filters(&[("coip-pool.pool-id", &past_value_ceiling)]),
        ),
        (
            "one byte past MAX_FILTER_VALUE_BYTES",
            filters(&[("coip-pool.pool-id", &[past_byte_ceiling.as_str()])]),
        ),
        (
            "one id past MAX_RESOURCE_IDS",
            indexed("PoolId", &past_id_ceiling),
        ),
    ];
    for (case, params) in refused {
        let (status, body) = describe(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValue",
            "case {case:?}: expected InvalidParameterValue, got {body}"
        );
        assert!(
            elements(&body, "poolId").is_empty(),
            "case {case:?}: a refused request reports no pool, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// DryRun
// ---------------------------------------------------------------------------

/// `DryRun` reports the permitted outcome — `DryRunOperation`, HTTP 412 — and describes
/// nothing; `DryRun=false` is an ordinary call.
#[tokio::test]
async fn dry_run_reports_the_permitted_outcome_without_describing() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    let (status, body) = describe(&router, &[("DryRun".to_string(), "true".to_string())]).await;
    assert_eq!(
        status, 412,
        "DryRun=true: expected HTTP 412, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "DryRunOperation",
        "DryRun=true: expected DryRunOperation, got {body}"
    );
    assert!(
        elements(&body, "poolId").is_empty(),
        "DryRun=true: a dry run reports no pool, got {body}"
    );

    let reported = described_ids(
        &router,
        "DryRun=false",
        &[("DryRun".to_string(), "false".to_string())],
    )
    .await;
    assert_eq!(
        reported, seeded,
        "DryRun=false: expected the ordinary report, got {reported:?}"
    );
}
