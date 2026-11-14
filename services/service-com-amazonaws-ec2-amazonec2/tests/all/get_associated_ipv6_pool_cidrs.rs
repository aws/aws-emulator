// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `GetAssociatedIpv6PoolCidrs`, driven over the wire the service
//! actually speaks: `ec2Query` — an `application/x-www-form-urlencoded` POST whose body
//! carries `Action=GetAssociatedIpv6PoolCidrs`, answered with XML (README.md /
//! OPERATIONS.md "Wire protocol"). The envelope is selected by the `ec2` signing name in the
//! request's SigV4 credential scope, so every call here carries one.
//!
//! The rules under test:
//!
//! 1. IT REPORTS THE STORE: "information about the IPv6 CIDR block associations for a
//!    specified IPv6 address pool" is every block a resource of this Region holds that was
//!    allocated out of THAT pool (`VpcIpv6CidrBlockAssociation$Ipv6Pool`), paired with the
//!    resource holding it (`Ipv6CidrAssociation$AssociatedResource`) — however the
//!    association was made, and whichever resource made it. A block nothing took out of the
//!    pool, and a block taken out of ANOTHER pool or provided by Amazon, is reported by no
//!    association of this pool. It is a REPORT: reading changes nothing a later caller sees.
//! 2. ORDER AND PAGING: the associations come back in address order — the order the pool
//!    reports its own blocks in (`Ipv6Pool$PoolCidrBlocks`) — cut into `MaxResults`-sized
//!    pages behind an opaque `NextToken` that always advances.
//! 3. REGIONAL: EC2 is reached through a regional endpoint, so an association held by a VPC
//!    of another Region is invisible to this request.
//! 4. WHAT IT REFUSES, each class with the code a client branches on: a value that is no
//!    IPv6 address pool id at all is refused on its FORM, before any lookup, with HTTP 400
//!    `InvalidIpv6PoolID.Malformed` naming the value; a pool id of the issued form the
//!    account holds no pool for is `InvalidIpv6PoolID.NotFound`; a request carrying an empty
//!    pool id is `MissingParameter`; a `NextToken` this service did not mint — or minted
//!    while paging another pool — is `InvalidPaginationToken`; and a `DryRun` is answered
//!    from the permission check alone (HTTP 412 `DryRunOperation`) without reporting an
//!    association.
//!
//! Every table row is a distinct input along what those rules generalise over — which pool,
//! how the association was made (at create or afterwards), how many associations one pool and
//! one VPC hold, which prefix length and base address, which page size, which Region — so no
//! single recorded call is the specification.
//!
//! The blocks here are RFC 3849's documentation range `2001:db8::/32`, which belongs to
//! nobody, and the pools that hold them are seeded through the store's own insert (the
//! sibling `ipv6_pools` suite's [`super::ipv6_pools::seed_pool`]), because one BYOIP
//! provision creates a pool holding one range and these rows need pools with room for
//! several sub-blocks.
//!
//! Each refusal's MESSAGE is checked only for the substitution the service makes — the value
//! the request sent, named back — never for wording this emulator has no rule for.

use std::net::Ipv6Addr;

use ec2::state::MAX_ECHOED_PAGE_TOKEN_CHARS;
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, elements, error_code, error_message};
use super::ipv6_pools::seed_pool;
use super::wire::{Param, param};

/// The ec2Query action under test.
const ACTION: &str = "GetAssociatedIpv6PoolCidrs";

/// The Region every call here is signed for unless a row names another.
const REGION: &str = "us-west-2";

/// A second Region, for the rule that an association of another Region is invisible.
const OTHER_REGION: &str = "eu-west-1";

/// EC2's error code for an IPv6 pool id of the right form that the account holds no pool
/// for.
const NOT_FOUND: &str = "InvalidIpv6PoolID.NotFound";

/// EC2's error code for a value that is no IPv6 address pool id at all, answered before any
/// lookup.
const MALFORMED: &str = "InvalidIpv6PoolID.Malformed";

/// EC2's error code for a request that left out a parameter the action requires.
const MISSING_PARAMETER: &str = "MissingParameter";

/// EC2's error code for a `NextToken` this service did not issue.
const INVALID_PAGINATION_TOKEN: &str = "InvalidPaginationToken";

/// EC2's answer to an authorized dry run, and the status it carries it on.
const DRY_RUN_OPERATION: &str = "DryRunOperation";
const DRY_RUN_STATUS: u16 = 412;

/// The XML elements one reported association arrives in (`Ipv6CidrAssociation`'s member
/// `xmlName`s).
const CIDR_ELEMENT: &str = "ipv6Cidr";
const RESOURCE_ELEMENT: &str = "associatedResource";

/// The emulator the tests drive, with the engine the seeded pools are written through.
fn emulator() -> (MultiServiceRouter, std::sync::Arc<engine::InMemoryEngine>) {
    super::ec2_query::emulator()
}

/// One association, as a response reports it: the block and the resource holding it.
type Association = (String, String);

/// One `GetAssociatedIpv6PoolCidrs` call in `region` carrying `params`.
async fn get_in(
    router: &MultiServiceRouter,
    region: &str,
    params: &[Param],
) -> (u16, String) {
    call(router, region, ACTION, params).await
}

/// One `GetAssociatedIpv6PoolCidrs` call carrying `params`, in the suite's Region.
async fn get(router: &MultiServiceRouter, params: &[Param]) -> (u16, String) {
    get_in(router, REGION, params).await
}

/// The body of a call asserted to have been served.
async fn served(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    params: &[Param],
) -> String {
    let (status, body) = get_in(router, region, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {ACTION}, got {status} with body {body}"
    );
    body
}

/// The associations a response reports, in the order it reported them.
fn reported(body: &str) -> Vec<Association> {
    let cidrs = elements(body, CIDR_ELEMENT);
    let resources = elements(body, RESOURCE_ELEMENT);
    assert_eq!(
        cidrs.len(),
        resources.len(),
        "every reported association carries both members, got {body}"
    );
    cidrs.into_iter().zip(resources).collect()
}

/// The associations `pool_id` reports in `region`, with `extra` form fields beside it.
async fn associations_in(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    pool_id: &str,
    extra: &[Param],
) -> Vec<Association> {
    let mut params = vec![param("PoolId", pool_id)];
    params.extend_from_slice(extra);
    reported(&served(router, case, region, &params).await)
}

/// The associations `pool_id` reports in the suite's Region.
async fn associations(
    router: &MultiServiceRouter,
    case: &str,
    pool_id: &str,
    extra: &[Param],
) -> Vec<Association> {
    associations_in(router, case, REGION, pool_id, extra).await
}

/// `expected` in ADDRESS order: by the block's base address, then its prefix length, then
/// the resource holding it — the order the report is paged in, of which the resource is the
/// tiebreak, since one resource cannot hold one block twice.
///
/// Computed from the standard library's own parser rather than from the service's reader, so
/// a table's expectation is independent of the code under test — and from the ORDER RULE
/// rather than by hand, so a row that adds a block cannot silently expect text order.
fn address_order(mut expected: Vec<Association>) -> Vec<Association> {
    expected.sort_by_key(|(cidr, resource)| {
        let (address, prefix_length) = cidr
            .split_once('/')
            .expect("a block carries a prefix length");
        let base = u128::from(address.parse::<Ipv6Addr>().expect("an IPv6 address"));
        let prefix_length: u8 = prefix_length.parse().expect("a prefix length");
        (base, prefix_length, resource.clone())
    });
    expected
}

/// Create one VPC in `region` and answer its id. `ipv6` names the block to take out of
/// `pool_id` at create, when the row asks for one.
async fn create_vpc(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    cidr_block: &str,
    ipv6: Option<(&str, &str)>,
) -> String {
    let mut params = vec![param("CidrBlock", cidr_block)];
    if let Some((ipv6_cidr_block, pool_id)) = ipv6 {
        params.push(param("Ipv6CidrBlock", ipv6_cidr_block));
        params.push(param("Ipv6Pool", pool_id));
    }
    let (status, body) = call(router, region, "CreateVpc", &params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateVpc, got {status} with body {body}"
    );
    element(&body, "vpcId").unwrap_or_else(|| panic!("case {case:?}: no <vpcId> in {body}"))
}

/// Associate `ipv6_cidr_block` out of `pool_id` with the VPC `vpc_id`, in `region`.
async fn associate(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    vpc_id: &str,
    ipv6_cidr_block: &str,
    pool_id: &str,
) {
    let (status, body) = call(
        router,
        region,
        "AssociateVpcCidrBlock",
        &[
            param("VpcId", vpc_id),
            param("Ipv6CidrBlock", ipv6_cidr_block),
            param("Ipv6Pool", pool_id),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from AssociateVpcCidrBlock, got {status} with body \
         {body}"
    );
}

/// The ranges the seeded pools hold. Each pool is a `/40`, so the rows have room for several
/// sub-blocks of their own.
const POOL_A_RANGE: &str = "2001:db8:a000::/40";
const POOL_B_RANGE: &str = "2001:db8:b000::/40";
const POOL_C_RANGE: &str = "2001:db8:c000::/40";

/// RULE 1: a pool's associations are exactly the blocks taken out of IT, paired with the
/// resource holding them — whether the association was made at create or afterwards, and
/// whether one resource holds one block or several.
///
/// RULE 2's order half comes with it: the blocks differ in base address and in prefix length,
/// and they are associated in an order that is neither address order nor the order they must
/// be reported in.
#[tokio::test]
async fn a_pool_reports_the_blocks_taken_out_of_it_and_who_holds_them() {
    let case = "three pools, one Region";
    let (router, engine) = emulator();
    let pool_a = seed_pool(&engine, &[POOL_A_RANGE], Vec::new(), None);
    let pool_b = seed_pool(&engine, &[POOL_B_RANGE], Vec::new(), None);
    let pool_c = seed_pool(&engine, &[POOL_C_RANGE], Vec::new(), None);

    // A VPC that took a block out of pool A at create, and a second block out of the same
    // pool afterwards: one resource, two associations, and the later block sorts BEFORE the
    // earlier one in address order.
    let first = create_vpc(
        &router,
        case,
        REGION,
        "10.71.0.0/16",
        Some(("2001:db8:a010::/56", &pool_a)),
    )
    .await;
    associate(&router, case, REGION, &first, "2001:db8:a001::/48", &pool_a).await;

    // A VPC with an Amazon-provided block — which comes out of no pool of the account — that
    // afterwards took a block out of pool B.
    let second = create_vpc(&router, case, REGION, "10.72.0.0/16", None).await;
    let (status, body) = call(
        &router,
        REGION,
        "AssociateVpcCidrBlock",
        &[
            param("VpcId", &second),
            param("AmazonProvidedIpv6CidrBlock", "true"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from AssociateVpcCidrBlock, got {status} with body \
         {body}"
    );
    associate(&router, case, REGION, &second, "2001:db8:b0ff::/56", &pool_b).await;

    // A third VPC on pool A, so a pool's associations span resources.
    let third = create_vpc(
        &router,
        case,
        REGION,
        "10.73.0.0/16",
        Some(("2001:db8:a020::/56", &pool_a)),
    )
    .await;

    let expected_a = address_order(vec![
        ("2001:db8:a010::/56".to_string(), first.clone()),
        ("2001:db8:a001::/48".to_string(), first.clone()),
        ("2001:db8:a020::/56".to_string(), third.clone()),
    ]);
    let got_a = associations(&router, case, &pool_a, &[]).await;
    assert_eq!(
        got_a, expected_a,
        "case {case:?}: expected pool A's associations {expected_a:?}, got {got_a:?}"
    );

    let expected_b = vec![("2001:db8:b0ff::/56".to_string(), second.clone())];
    let got_b = associations(&router, case, &pool_b, &[]).await;
    assert_eq!(
        got_b, expected_b,
        "case {case:?}: expected pool B's associations {expected_b:?}, got {got_b:?}"
    );

    // A pool nothing took a block out of reports no association — not the associations of
    // another pool, and not the range it holds.
    let got_c = associations(&router, case, &pool_c, &[]).await;
    assert!(
        got_c.is_empty(),
        "case {case:?}: expected no association for a pool nothing drew on, got {got_c:?}"
    );

    // RULE 1's reporting half: reading twice reports the same thing — the call writes
    // nothing a later caller would see.
    let again = associations(&router, case, &pool_a, &[]).await;
    assert_eq!(
        again, expected_a,
        "case {case:?}: a second read must report the same associations, got {again:?}"
    );
}

/// RULE 3: an association held by a VPC of another Region is invisible to a request signed
/// for this one, and visible to a request signed for its own — the pool itself resolves in
/// both.
#[tokio::test]
async fn an_association_of_another_region_is_not_reported_here() {
    let case = "one pool, two Regions";
    let (router, engine) = emulator();
    let pool = seed_pool(&engine, &[POOL_A_RANGE], Vec::new(), None);

    let here = create_vpc(
        &router,
        case,
        REGION,
        "10.74.0.0/16",
        Some(("2001:db8:a030::/56", &pool)),
    )
    .await;
    let there = create_vpc(
        &router,
        case,
        OTHER_REGION,
        "10.75.0.0/16",
        Some(("2001:db8:a040::/56", &pool)),
    )
    .await;

    for (region, expected) in [
        (REGION, vec![("2001:db8:a030::/56".to_string(), here)]),
        (
            OTHER_REGION,
            vec![("2001:db8:a040::/56".to_string(), there)],
        ),
    ] {
        let got = associations_in(&router, case, region, &pool, &[]).await;
        assert_eq!(
            got, expected,
            "case {case:?}: expected {region}'s associations {expected:?}, got {got:?}"
        );
    }
}

/// RULE 2: the associations are cut into pages of the asked-for size, in address order,
/// behind a token that always advances — over every page size from one association to more
/// than the pool has.
#[tokio::test]
async fn the_report_pages_the_associations_in_address_order() {
    let case_setup = "five associations of one pool";
    let (router, engine) = emulator();
    let pool = seed_pool(&engine, &[POOL_A_RANGE], Vec::new(), None);

    // Five associations over three VPCs, so a page can end inside one resource's blocks, and
    // with bases that are not in the order they are made in.
    let mut expected: Vec<Association> = Vec::new();
    let blocks = [
        "2001:db8:a0f0::/56",
        "2001:db8:a050::/56",
        "2001:db8:a0a0::/60",
        "2001:db8:a001::/48",
        "2001:db8:a0b0::/56",
    ];
    let mut vpcs: Vec<String> = Vec::new();
    for index in 0..3 {
        vpcs.push(
            create_vpc(
                &router,
                case_setup,
                REGION,
                &format!("10.8{index}.0.0/16"),
                None,
            )
            .await,
        );
    }
    for (index, block) in blocks.iter().enumerate() {
        let vpc = &vpcs[index % vpcs.len()];
        associate(&router, case_setup, REGION, vpc, block, &pool).await;
        expected.push(((*block).to_string(), vpc.clone()));
    }
    let expected = address_order(expected);

    for page_size in [1usize, 2, 3, 5, 7] {
        let case = format!("MaxResults {page_size}");
        let mut seen: Vec<Association> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            let mut extra = vec![param("MaxResults", &page_size.to_string())];
            if let Some(next) = token.clone() {
                extra.push(param("NextToken", &next));
            }
            let mut params = vec![param("PoolId", &pool)];
            params.extend_from_slice(&extra);
            let body = served(&router, &case, REGION, &params).await;
            let page = reported(&body);
            assert!(
                page.len() <= page_size,
                "case {case:?}: a page holds at most {page_size} associations, got {page:?}"
            );
            seen.extend(page);
            pages += 1;
            token = element(&body, "nextToken");
            assert!(
                pages <= expected.len() + 1,
                "case {case:?}: paging must terminate, got {pages} pages for {} associations",
                expected.len()
            );
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            seen, expected,
            "case {case:?}: expected every association once, in address order"
        );
        let expected_pages = expected.len().div_ceil(page_size);
        assert_eq!(
            pages, expected_pages,
            "case {case:?}: expected {expected_pages} page(s) for {} associations, got {pages}",
            expected.len()
        );
    }
}

/// RULE 4: what the report refuses, against a store that HOLDS a pool with an association —
/// so a refusal is a verdict about the request rather than about an empty account.
#[tokio::test]
async fn the_report_refuses_what_it_cannot_serve() {
    let case = "refusals";
    let (router, engine) = emulator();
    let pool = seed_pool(&engine, &[POOL_A_RANGE], Vec::new(), None);
    let other = seed_pool(&engine, &[POOL_B_RANGE], Vec::new(), None);
    let vpc = create_vpc(
        &router,
        case,
        REGION,
        "10.76.0.0/16",
        Some(("2001:db8:a060::/56", &pool)),
    )
    .await;

    // A token this pool really minted, to show the cross-pool refusal below is about the
    // SCOPE and not about the token.
    let paged = served(
        &router,
        case,
        REGION,
        &[param("PoolId", &pool), param("MaxResults", "1")],
    )
    .await;
    let minted = element(&paged, "nextToken");
    assert!(
        minted.is_none(),
        "case {case:?}: one association fills a page of one, so no token is due, got {minted:?}"
    );

    // Two associations, so a page of one leaves one behind and a token IS due.
    associate(&router, case, REGION, &vpc, "2001:db8:a070::/56", &pool).await;
    let paged = served(
        &router,
        case,
        REGION,
        &[param("PoolId", &pool), param("MaxResults", "1")],
    )
    .await;
    let minted = element(&paged, "nextToken")
        .unwrap_or_else(|| panic!("case {case:?}: expected a token on a short page, got {paged}"));

    // (case, the parameters sent, the expected status, the expected code, the value the
    // message must name)
    let refusals: Vec<(&str, Vec<Param>, u16, &str, String)> = vec![
        (
            "a pool id of the issued form nothing was provisioned for",
            vec![param("PoolId", "ipv6pool-ec2-0fedcba987654321f")],
            400,
            NOT_FOUND,
            "ipv6pool-ec2-0fedcba987654321f".to_string(),
        ),
        (
            "the all-zero placeholder no mint can produce",
            vec![param("PoolId", "ipv6pool-ec2-00000000000000000")],
            400,
            MALFORMED,
            "ipv6pool-ec2-00000000000000000".to_string(),
        ),
        (
            "a value that is no pool id at all",
            vec![param("PoolId", "not-a-pool")],
            400,
            MALFORMED,
            "not-a-pool".to_string(),
        ),
        (
            "a pool id of the other address family",
            vec![param("PoolId", "ipv4pool-ec2-0fedcba987654321f")],
            400,
            MALFORMED,
            "ipv4pool-ec2-0fedcba987654321f".to_string(),
        ),
        (
            "a suffix that is not hexadecimal",
            vec![param("PoolId", "ipv6pool-ec2-0zzzzzzzzzzzzzzzz")],
            400,
            MALFORMED,
            "ipv6pool-ec2-0zzzzzzzzzzzzzzzz".to_string(),
        ),
        (
            "a suffix of neither published digit count",
            vec![param("PoolId", "ipv6pool-ec2-0fedcba")],
            400,
            MALFORMED,
            "ipv6pool-ec2-0fedcba".to_string(),
        ),
        (
            "an empty pool id",
            vec![param("PoolId", "")],
            400,
            MISSING_PARAMETER,
            "poolId".to_string(),
        ),
        (
            "a token this service did not mint",
            vec![param("PoolId", &pool), param("NextToken", "not-a-token")],
            400,
            INVALID_PAGINATION_TOKEN,
            ec2::state::INVALID_PAGINATION_TOKEN_MESSAGE.to_string(),
        ),
        (
            "a token minted while paging another pool",
            vec![param("PoolId", &other), param("NextToken", &minted)],
            400,
            INVALID_PAGINATION_TOKEN,
            ec2::state::INVALID_PAGINATION_TOKEN_MESSAGE.to_string(),
        ),
        (
            "a dry run",
            vec![param("PoolId", &pool), param("DryRun", "true")],
            DRY_RUN_STATUS,
            DRY_RUN_OPERATION,
            String::new(),
        ),
    ];

    for (row, params, expected_status, expected_code, named) in refusals {
        let (status, body) = get(&router, &params).await;
        assert_eq!(
            (status, error_code(&body)),
            (expected_status, expected_code.to_string()),
            "case {row:?}: expected HTTP {expected_status} {expected_code}, got {status} with \
             body {body}"
        );
        assert!(
            reported(&body).is_empty(),
            "case {row:?}: a refusal reports no association, got {body}"
        );
        // The refusal names the value the request sent, so a caller can tell which member
        // was rejected from the text alone — each class in the sentence EC2 words it with,
        // around the id from the request.
        let message = error_message(&body);
        if expected_code == MALFORMED {
            assert_eq!(
                message,
                format!("The ipv6pool-ec2 ID '{named}' is malformed"),
                "case {row:?}: unexpected malformed-id message"
            );
        } else if expected_code == NOT_FOUND {
            assert_eq!(
                message,
                format!("Ipv6Pool '{named}' does not exist."),
                "case {row:?}: unexpected not-found message"
            );
        } else if expected_code == INVALID_PAGINATION_TOKEN {
            // The pagination-token refusal is EC2's one fixed sentence: it names no token,
            // so no part of a caller-chosen value is reflected at all.
            assert_eq!(
                message, named,
                "case {row:?}: expected EC2's own pagination-token sentence, got {message:?}"
            );
        } else if !named.is_empty() {
            // A refusal that echoes a caller-chosen value bounds that echo, so the message
            // names as much of the value as the service repeats back — read from the
            // service's own ceiling rather than from a literal here, so the two cannot drift.
            let echoed: String = named.chars().take(MAX_ECHOED_PAGE_TOKEN_CHARS).collect();
            assert!(
                message.contains(&echoed),
                "case {row:?}: expected the message to name {echoed:?}, got {message:?}"
            );
        }
    }

    // Nothing the refusals did took the held associations away.
    let expected = address_order(vec![
        ("2001:db8:a060::/56".to_string(), vpc.clone()),
        ("2001:db8:a070::/56".to_string(), vpc.clone()),
    ]);
    let got = associations(&router, case, &pool, &[]).await;
    assert_eq!(
        got, expected,
        "case {case:?}: a refused read leaves the store as it was, got {got:?}"
    );
}
