// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the IPv6 address-pool slice — the pool a BYOIP IPv6 provision
//! creates and the `DescribeIpv6Pools` that reads it back — driven over the wire the service
//! actually speaks: `ec2Query`, an `application/x-www-form-urlencoded` POST whose body
//! carries `Action=<Operation>`, answered with XML (README.md / OPERATIONS.md "Wire
//! protocol"). The envelope is selected by the `ec2` signing name in the request's SigV4
//! credential scope, so every call here carries one.
//!
//! The rules under test:
//!
//! 1. PROVISIONING AN IPv6 ADDRESS RANGE CREATES THE POOL THAT HOLDS IT — "provisioning an
//!    IPv4 or IPv6 address range … creates a corresponding address pool"
//!    (`ProvisionByoipCidr`) — under the `ipv6pool-ec2-…` id the range reports as its
//!    `PoolId`, carrying the range in canonical form, the description and the pool tags the
//!    request asked for. An IPv4 range's pool is an `ipv4pool-ec2`, which is NOT an IPv6
//!    pool.
//! 2. THE POOL GOES WITH ITS RANGE: once the clock has finished the release
//!    `DeprovisionByoipCidr` started, the account holds neither the range nor its pool.
//! 3. `DescribeIpv6Pools` REPORTS THE STORE: narrowed by the `PoolIds` and the documented
//!    `tag:<key>` / `tag-key` filters the request names, cut into `MaxResults`-sized pages in
//!    pool-id order behind an opaque `NextToken`, and it is a REPORT — reading changes
//!    nothing a later caller sees.
//! 4. WHAT IT REFUSES: a pool id the account holds no pool for is HTTP 400
//!    `InvalidIpv6PoolID.NotFound` naming that id, a filter name the operation does not
//!    document is `InvalidParameterValue`, a `NextToken` this service did not mint is
//!    `InvalidPaginationToken`, and a `DryRun` is answered from the permission check alone
//!    (HTTP 412 `DryRunOperation`) without reporting a pool.
//!
//! Every table row is a distinct input along what those rules generalise over — which prefix
//! length, which base address, whether the block is written in canonical form, whether the
//! range is publicly advertisable, how many ranges a pool holds, which filter, which page
//! size — so no single recorded call is the specification.
//!
//! A pool holding SEVERAL ranges is seeded straight into the store, the way the sibling
//! `public_ipv4_pools` suite seeds its multi-range pools: one BYOIP provision creates a pool
//! with one range and nothing in the implemented surface provisions a second range to an
//! IPv6 pool, so the multi-range rows have no API path. The creation path a caller takes is
//! covered by rule 1 above, over the wire.
//!
//! The ranges a row PROVISIONS are public address space, as
//! `ProvisionByoipCidrRequest$Cidr` requires ("The public IPv4 or IPv6 address range"), and
//! are the same ranges the `byoip_cidrs` suite brings; the blocks a row SEEDS are RFC 3849's
//! documentation range `2001:db8::/32`, which belongs to nobody.
//!
//! Each refusal's MESSAGE is checked only for the substitution the service makes — the value
//! the request sent, named back — never for wording this emulator has no rule for.

use std::net::Ipv6Addr;
use std::time::Duration;

use ec2::ops::types::Tag;
use ec2::state::{ComAmazonawsEc2Amazonec2State as Ec2State, Ipv6PoolSpec, TRANSITION_DELAY};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, elements, error_code, error_message, filters, indexed};

/// The Region every call here is signed for.
const REGION: &str = "us-west-2";

/// EC2's error code for an IPv6 pool id a read named that the account holds no pool for.
const NOT_FOUND: &str = "InvalidIpv6PoolID.NotFound";

/// EC2's error code for a member value the service cannot accept.
const INVALID_PARAMETER_VALUE: &str = "InvalidParameterValue";

/// EC2's error code for a `NextToken` this service did not issue.
const INVALID_PAGINATION_TOKEN: &str = "InvalidPaginationToken";

/// EC2's error code for a `DryRun` request that would otherwise have been served.
const DRY_RUN_OPERATION: &str = "DryRunOperation";

/// The id prefix EC2 spells an IPv6 address pool with (`ipv6pool-ec2-…`).
const POOL_ID_PREFIX: &str = "ipv6pool-ec2-";

/// The id prefix EC2 spells a public IPv4 address pool with.
const IPV4_POOL_ID_PREFIX: &str = "ipv4pool-ec2-";

/// A `(key, value)` request parameter.
fn p(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

/// One request's form parameters, as the ec2Query harness takes them.
type Params = Vec<(String, String)>;

/// The emulator the tests drive, with the engine the seeded rows write through.
fn emulator() -> (MultiServiceRouter, std::sync::Arc<engine::InMemoryEngine>) {
    super::ec2_query::emulator()
}

/// One `DescribeIpv6Pools` call carrying `params`.
async fn describe(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    call(router, REGION, "DescribeIpv6Pools", params).await
}

/// One `ProvisionByoipCidr` call carrying `params`.
async fn provision(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    call(router, REGION, "ProvisionByoipCidr", params).await
}

/// One `DeprovisionByoipCidr` call for `cidr`.
async fn deprovision(router: &MultiServiceRouter, cidr: &str) -> (u16, String) {
    call(
        router,
        REGION,
        "DeprovisionByoipCidr",
        &[p("Cidr", cidr)],
    )
    .await
}

/// The body of a describe asserted to have been served.
async fn described(router: &MultiServiceRouter, case: &str, params: &[(String, String)]) -> String {
    let (status, body) = describe(router, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeIpv6Pools, got {status} with body {body}"
    );
    body
}

/// The pool ids a successful describe reports, in the order it reported them.
async fn described_ids(
    router: &MultiServiceRouter,
    case: &str,
    params: &[(String, String)],
) -> Vec<String> {
    let body = described(router, case, params).await;
    elements(&body, "poolId")
}

/// Wait until every clock-owed transition started before now has certainly fallen due.
/// Derived from the service's own span, not a literal, so the two cannot drift.
async fn past_the_transition() {
    tokio::time::sleep(TRANSITION_DELAY + Duration::from_millis(200)).await;
}

/// A tag, as the seeded and provisioned pools carry them.
fn tag(key: &str, value: &str) -> Tag {
    Tag {
        key: Some(key.to_string()),
        value: Some(value.to_string()),
    }
}

/// Write one IPv6 pool holding `cidrs` into the store and answer the id minted for it.
///
/// The store's own insert is what mints the id and canonicalises the ranges, so a seeded
/// pool is a pool exactly as a provision would have created it. Shared with the
/// `get_associated_ipv6_pool_cidrs` suite, which needs pools of its own to associate blocks
/// out of.
pub fn seed_pool(
    engine: &engine::InMemoryEngine,
    cidrs: &[&str],
    tags: Vec<Tag>,
    description: Option<&str>,
) -> String {
    let store = engine.state::<Ec2State>();
    let mut guard = store.lock();
    guard
        .insert_ipv6_pool(Ipv6PoolSpec {
            description: description.map(str::to_string),
            tags,
            cidrs: cidrs.iter().map(|cidr| (*cidr).to_string()).collect(),
        })
        .expect("the store has room for the seeded pool")
}

/// The address ranges the store holds for `pool_id`, read straight from the store so it
/// reports what a later caller would be told without a call of its own.
fn stored_cidrs(engine: &engine::InMemoryEngine, pool_id: &str) -> Vec<String> {
    let store = engine.state::<Ec2State>();
    let guard = store.lock();
    guard
        .ipv6_pools
        .get(pool_id)
        .map(|record| record.cidrs.iter().cloned().collect())
        .unwrap_or_default()
}

/// `cidr` with its host bits cleared and its address rendered the one way `Ipv6Addr` renders
/// one — computed here from the standard library's own parser rather than from the service's
/// reader, so the table's expectations are independent of the code under test.
fn canonical(cidr: &str) -> String {
    let (address, prefix_length) = cidr.split_once('/').expect("a block carries a prefix length");
    let base = u128::from(address.parse::<Ipv6Addr>().expect("an IPv6 address"));
    let prefix_length: u32 = prefix_length.parse().expect("a prefix length");
    let mask = u128::MAX << (128 - prefix_length);
    format!("{}/{prefix_length}", Ipv6Addr::from(base & mask))
}

/// One provisioned row: `(case, the Cidr sent, the PubliclyAdvertisable sent)`.
type ProvisionRow = (&'static str, &'static str, Option<bool>);

/// The ranges rule 1 is written against: the most specific prefix each advertisability
/// admits (`/48` publicly advertisable, `/56` not — `ProvisionByoipCidrRequest$Cidr`), less
/// specific blocks, two bases in different global-unicast allocations, and a block written
/// with host bits set.
const PROVISIONED: [ProvisionRow; 5] = [
    (
        "a publicly advertisable /48",
        "2600:1f14:abc::/48",
        Some(true),
    ),
    (
        "a range left at the default advertisability",
        "2a05:d018:1234::/48",
        None,
    ),
    (
        "a /56 that is not publicly advertisable",
        "2a05:d018:1234:5600::/56",
        Some(false),
    ),
    ("a less specific /40", "2606:4700:10::/40", Some(true)),
    (
        "a range given with host bits set",
        "2606:4700:20::dead:beef/48",
        Some(true),
    ),
];

/// RULE 1: a provisioned IPv6 range gives the account one IPv6 pool holding that range,
/// under the id the range reports, carrying the description and the pool tags the request
/// asked for.
#[tokio::test]
async fn a_provisioned_ipv6_range_creates_the_pool_that_holds_it() {
    for (case, requested, publicly_advertisable) in PROVISIONED {
        // One emulator per row: the rows are independent inputs, not a sequence, and an
        // empty store is what makes "exactly this pool is reported" an assertion about the
        // row.
        let (router, engine) = emulator();
        let mut params: Params = vec![
            p("Cidr", requested),
            p("Description", "a documentation range"),
            p("PoolTagSpecification.1.ResourceType", "ipv6pool-ec2"),
            p("PoolTagSpecification.1.Tag.1.Key", "Owner"),
            p("PoolTagSpecification.1.Tag.1.Value", "TeamA"),
        ];
        if let Some(advertisable) = publicly_advertisable {
            params.push(p("PubliclyAdvertisable", advertisable.to_string()));
        }
        let (status, body) = provision(&router, &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from ProvisionByoipCidr, got {status} with body \
             {body}"
        );
        let pool_id = element(&body, "poolId").unwrap_or_default();
        assert!(
            pool_id.starts_with(POOL_ID_PREFIX),
            "case {case:?}: expected a minted IPv6 pool id, got {pool_id:?}"
        );

        let expected = canonical(requested);
        let described_body = described(&router, case, &indexed("PoolId", &[&pool_id])).await;
        assert_eq!(
            elements(&described_body, "poolId"),
            vec![pool_id.clone()],
            "case {case:?}: expected exactly the pool the provision created, got \
             {described_body}"
        );
        assert_eq!(
            elements(&described_body, "poolCidrBlock"),
            vec![expected.clone()],
            "case {case:?}: expected the pool to hold {expected:?}, got {described_body}"
        );
        assert_eq!(
            element(&described_body, "description").unwrap_or_default(),
            "a documentation range",
            "case {case:?}: the description describes the range and the pool, got \
             {described_body}"
        );
        assert_eq!(
            (
                element(&described_body, "key").unwrap_or_default(),
                element(&described_body, "value").unwrap_or_default()
            ),
            ("Owner".to_string(), "TeamA".to_string()),
            "case {case:?}: the pool carries the tags the provision asked for, got \
             {described_body}"
        );
        assert_eq!(
            stored_cidrs(&engine, &pool_id),
            vec![expected.clone()],
            "case {case:?}: the store holds the canonical block for {pool_id}"
        );

        // RULE 3, the reporting half: reading twice reports the same pool — a describe
        // writes nothing. The comparison is of the pool SET, since the envelope's
        // `requestId` is per-call.
        let again = described(&router, case, &indexed("PoolId", &[&pool_id])).await;
        assert_eq!(
            element(&again, "ipv6PoolSet"),
            element(&described_body, "ipv6PoolSet"),
            "case {case:?}: a second describe must report the same pool, got {again}"
        );
    }
}

/// RULE 1, the other side of the family rule: an IPv4 range's corresponding pool is an
/// `ipv4pool-ec2`, so no IPv6 pool appears for it.
#[tokio::test]
async fn an_ipv4_range_creates_no_ipv6_pool() {
    let case = "an IPv4 range";
    let (router, _engine) = emulator();
    let (status, body) = provision(&router, &[p("Cidr", "81.2.69.0/24")]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from ProvisionByoipCidr, got {status} with body {body}"
    );
    let pool_id = element(&body, "poolId").unwrap_or_default();
    assert!(
        pool_id.starts_with(IPV4_POOL_ID_PREFIX),
        "case {case:?}: an IPv4 range's pool is an ipv4pool-ec2, got {pool_id:?}"
    );
    let reported = described_ids(&router, case, &[]).await;
    assert!(
        reported.is_empty(),
        "case {case:?}: an IPv4 range gives the account no IPv6 pool, got {reported:?}"
    );
}

/// RULE 2: the clock finishes the release, and the pool of the released range goes with it.
#[tokio::test]
async fn the_pool_goes_with_the_range_the_clock_releases() {
    let case = "a released IPv6 range";
    let (router, _engine) = emulator();
    let cidr = "2600:1f14:abc::/48";
    let (status, body) = provision(&router, &[p("Cidr", cidr)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from ProvisionByoipCidr, got {status} with body {body}"
    );
    let pool_id = element(&body, "poolId").unwrap_or_default();

    // The range is `pending-provision` until the clock has given it the service's own
    // transition span; a release is accepted only once it is provisioned.
    past_the_transition().await;
    assert_eq!(
        described_ids(&router, case, &[]).await,
        vec![pool_id.clone()],
        "case {case:?}: the pool is the account's while the range is"
    );

    let (status, body) = deprovision(&router, cidr).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DeprovisionByoipCidr, got {status} with body \
         {body}"
    );
    assert_eq!(
        described_ids(&router, case, &[]).await,
        vec![pool_id.clone()],
        "case {case:?}: a release under way has not taken the pool yet"
    );

    past_the_transition().await;
    assert!(
        described_ids(&router, case, &[]).await.is_empty(),
        "case {case:?}: the pool of a released range is no pool of the account"
    );
    let (status, body) = describe(&router, &indexed("PoolId", &[&pool_id])).await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, NOT_FOUND),
        "case {case:?}: the released pool's id names no pool now, got {status} with body {body}"
    );
}

/// The ranges a seeded multi-range pool holds, IN ADDRESS ORDER — which the text order of
/// the blocks is not. Different prefix lengths and bases, so "the pool reports its blocks"
/// is an assertion about the blocks rather than about one of them.
const SEEDED: [&str; 3] = [
    "2001:db8:1000::/48",
    "2001:db8:2000:ab00::/56",
    "2001:db8:ff00::/40",
];

/// RULE 3: a pool reports every CIDR block it holds, in address order whatever order they
/// went in and however they were spelled.
#[tokio::test]
async fn a_pool_reports_every_block_it_holds_in_address_order() {
    let case = "a pool holding three blocks";
    let (router, engine) = emulator();
    // Seeded in an order that is neither address order nor the order the describe must
    // report, and with host bits set on one block, so the reported order is the service's.
    let pool_id = seed_pool(
        &engine,
        &[SEEDED[2], "2001:db8:1000::5/48", SEEDED[1]],
        Vec::new(),
        None,
    );
    let body = described(&router, case, &indexed("PoolId", &[&pool_id])).await;
    let expected: Vec<String> = SEEDED.iter().map(|cidr| canonical(cidr)).collect();
    assert_eq!(
        elements(&body, "poolCidrBlock"),
        expected,
        "case {case:?}: expected the pool's blocks in address order, got {body}"
    );
}

/// One filter row: `(case, the filter name and values, the pools it must select)`.
type FilterRow = (
    &'static str,
    &'static [(&'static str, &'static [&'static str])],
    &'static [usize],
);

/// RULE 3 over the two filters `DescribeIpv6PoolsRequest$Filters` documents, against three
/// seeded pools: the tag key alone, one `tag:<key>` value, several values of one filter
/// (joined with OR), two filters together (joined with AND), and a value nothing carries.
const FILTERS: [FilterRow; 6] = [
    ("no filter at all", &[], &[0, 1, 2]),
    ("tag-key", &[("tag-key", &["Project"])], &[0, 2]),
    ("tag:Owner one value", &[("tag:Owner", &["TeamA"])], &[0]),
    (
        "tag:Owner several values, joined with OR",
        &[("tag:Owner", &["TeamA", "TeamB"])],
        &[0, 1],
    ),
    (
        "two filters, joined with AND",
        &[("tag:Owner", &["TeamA"]), ("tag-key", &["Project"])],
        &[0],
    ),
    (
        "a tag value nothing carries",
        &[("tag:Owner", &["TeamZ"])],
        &[],
    ),
];

/// RULE 3: the describe is narrowed by the documented filters, and by `PoolIds`.
#[tokio::test]
async fn the_describe_is_narrowed_by_the_documented_filters_and_the_pool_ids() {
    let (router, engine) = emulator();
    let pools = [
        seed_pool(
            &engine,
            &["2001:db8:1::/48"],
            vec![tag("Owner", "TeamA"), tag("Project", "apollo")],
            Some("the first pool"),
        ),
        seed_pool(
            &engine,
            &["2001:db8:2::/48"],
            vec![tag("Owner", "TeamB")],
            None,
        ),
        seed_pool(
            &engine,
            &["2001:db8:3::/48"],
            vec![tag("Project", "gemini")],
            None,
        ),
    ];
    for (case, entries, expected) in FILTERS {
        let wanted: Vec<String> = expected.iter().map(|index| pools[*index].clone()).collect();
        let reported = described_ids(&router, case, &filters(entries)).await;
        assert_eq!(
            reported, wanted,
            "case {case:?}: expected the pools {wanted:?}, got {reported:?}"
        );
    }

    // `PoolIds` narrows to the ids named, in pool-id order however they were asked for.
    for (case, asked) in [
        ("one id", vec![1usize]),
        ("two ids, asked for in reverse", vec![2, 0]),
    ] {
        let ids: Vec<String> = asked.iter().map(|index| pools[*index].clone()).collect();
        let mut wanted = ids.clone();
        wanted.sort();
        let selectors: Vec<&str> = ids.iter().map(String::as_str).collect();
        let reported = described_ids(&router, case, &indexed("PoolId", &selectors)).await;
        assert_eq!(
            reported, wanted,
            "case {case:?}: expected the pools {wanted:?}, got {reported:?}"
        );
    }

    // `PoolIds` and a filter together: both must hold.
    let case = "a pool id and a filter that excludes it";
    let reported = described_ids(
        &router,
        case,
        &[
            indexed("PoolId", &[pools[1].as_str()]),
            filters(&[("tag-key", &["Project"])]),
        ]
        .concat(),
    )
    .await;
    assert!(
        reported.is_empty(),
        "case {case:?}: expected no pool, got {reported:?}"
    );
}

/// RULE 3: the store is cut into pages of the asked-for size, in pool-id order, behind a
/// token that always advances — over every page size from one pool to more than the store
/// holds.
#[tokio::test]
async fn the_describe_pages_the_store_in_pool_id_order() {
    let (router, engine) = emulator();
    let mut pools: Vec<String> = (0..5)
        .map(|index| {
            let cidr = format!("2001:db8:{index}00::/48");
            seed_pool(&engine, &[cidr.as_str()], Vec::new(), None)
        })
        .collect();
    pools.sort();

    for page_size in [1usize, 2, 3, 5, 7] {
        let case = format!("MaxResults {page_size}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            let mut params: Params = vec![p("MaxResults", page_size.to_string())];
            if let Some(next) = token.clone() {
                params.push(p("NextToken", next));
            }
            let body = described(&router, &case, &params).await;
            let page = elements(&body, "poolId");
            assert!(
                page.len() <= page_size,
                "case {case:?}: a page holds at most {page_size} pools, got {page:?}"
            );
            seen.extend(page);
            pages += 1;
            token = element(&body, "nextToken");
            assert!(
                pages <= pools.len() + 1,
                "case {case:?}: paging must terminate, got {pages} pages for {} pools",
                pools.len()
            );
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            seen, pools,
            "case {case:?}: expected every pool once, in pool-id order"
        );
        let expected_pages = pools.len().div_ceil(page_size);
        assert_eq!(
            pages, expected_pages,
            "case {case:?}: expected {expected_pages} page(s) for {} pools, got {pages}",
            pools.len()
        );
    }
}

/// One refusal row: `(case, the parameters sent, the expected status, the expected code)`.
type RefusalRow = (&'static str, &'static [(&'static str, &'static str)], u16, &'static str);

/// RULE 4: what the describe refuses. A well-formed id nothing was provisioned for, a value
/// that is no pool id at all, a filter name the operation does not document (including the
/// `pool-id` a sibling pool describe does), an empty `tag:` key, and a token this service
/// never minted.
const REFUSALS: [RefusalRow; 6] = [
    (
        "a pool id nothing was provisioned for",
        &[("PoolId.1", "ipv6pool-ec2-00000000000000000")],
        400,
        NOT_FOUND,
    ),
    (
        "a value that is no pool id at all",
        &[("PoolId.1", "not-a-pool")],
        400,
        NOT_FOUND,
    ),
    (
        "a filter the operation does not document",
        &[("Filter.1.Name", "pool-id"), ("Filter.1.Value.1", "any")],
        400,
        INVALID_PARAMETER_VALUE,
    ),
    (
        "a tag filter naming no key",
        &[("Filter.1.Name", "tag:"), ("Filter.1.Value.1", "any")],
        400,
        INVALID_PARAMETER_VALUE,
    ),
    (
        "a filter name differing only in case",
        &[("Filter.1.Name", "Tag-Key"), ("Filter.1.Value.1", "any")],
        400,
        INVALID_PARAMETER_VALUE,
    ),
    (
        "a token this service did not mint",
        &[("NextToken", "not-a-token")],
        400,
        INVALID_PAGINATION_TOKEN,
    ),
];

/// RULE 4 over that catalog, against a store that HOLDS a pool — so a refusal is a verdict
/// about the request rather than about an empty account.
#[tokio::test]
async fn the_describe_refuses_what_it_cannot_serve() {
    let (router, engine) = emulator();
    let held = seed_pool(&engine, &["2001:db8:4::/48"], Vec::new(), None);
    for (case, sent, expected_status, expected_code) in REFUSALS {
        let params: Params = sent
            .iter()
            .map(|(key, value)| p(key, (*value).to_string()))
            .collect();
        let (status, body) = describe(&router, &params).await;
        assert_eq!(
            (status, error_code(&body)),
            (expected_status, expected_code.to_string()),
            "case {case:?}: expected HTTP {expected_status} {expected_code}, got {status} with \
             body {body}"
        );
        // The refusal names the value the request sent, so a caller can tell which member
        // was rejected from the text alone — and a missing pool is named in the sentence
        // EC2 words that class with, around the id from the request.
        let echoed = sent
            .iter()
            .find(|(key, _)| *key != "Filter.1.Value.1")
            .map(|(_, value)| *value)
            .unwrap_or_default();
        let message = error_message(&body);
        if expected_code == NOT_FOUND {
            assert_eq!(
                message,
                format!("Ipv6Pool '{echoed}' does not exist."),
                "case {case:?}: unexpected not-found message"
            );
        } else if expected_code == INVALID_PAGINATION_TOKEN {
            // The pagination-token refusal is EC2's one fixed sentence: it names no token.
            assert_eq!(
                message,
                ec2::state::INVALID_PAGINATION_TOKEN_MESSAGE,
                "case {case:?}: expected EC2's own pagination-token sentence, got {message:?}"
            );
        } else {
            assert!(
                message.contains(echoed),
                "case {case:?}: expected the message to name {echoed:?}, got {message:?}"
            );
        }
        // Nothing the refusals did took the held pool away.
        assert_eq!(
            described_ids(&router, case, &[]).await,
            vec![held.clone()],
            "case {case:?}: a refused describe leaves the store as it was"
        );
    }
}

/// RULE 4: a dry run is answered from the permission check alone, reporting no pool.
#[tokio::test]
async fn a_dry_run_reports_no_pool() {
    let case = "DryRun";
    let (router, engine) = emulator();
    let held = seed_pool(&engine, &["2001:db8:5::/48"], Vec::new(), None);
    let (status, body) = describe(&router, &[p("DryRun", "true")]).await;
    assert_eq!(
        (status, error_code(&body)),
        (412, DRY_RUN_OPERATION.to_string()),
        "case {case:?}: expected HTTP 412 {DRY_RUN_OPERATION}, got {status} with body {body}"
    );
    assert!(
        elements(&body, "poolId").is_empty(),
        "case {case:?}: a dry run reports no pool, got {body}"
    );
    assert_eq!(
        described_ids(&router, case, &[]).await,
        vec![held],
        "case {case:?}: the dry run left the store as it was"
    );
}
