// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DescribeVpcClassicLinkDnsSupport`, driven over the wire the service
//! actually speaks: `ec2Query` — an `application/x-www-form-urlencoded` POST whose body
//! carries `Action=…`, answered with XML (README.md / OPERATIONS.md "Wire protocol").
//! ec2Query is selected by the `ec2` signing name in the request's SigV4 credential scope, so
//! every call here carries one.
//!
//! The rule under test is that the ClassicLink DNS support of a VPC is STATE THE SERVICE KEEPS
//! PER VPC, and that this operation REPORTS it: for each VPC the request resolves it answers
//! the status the store holds for that VPC — a VPC nobody enabled it for reported as not
//! supporting it, and the status independent of whether that VPC is enabled for ClassicLink —
//! it reports nothing for a VPC the account does not hold but refuses the request instead, and
//! it changes nothing, so two calls report the same thing. A `VpcIds` entry that is no VPC id
//! at all is refused before any lookup, and a well-formed id the account holds no VPC for
//! after it.
//!
//! So every table row below is a distinct input along what that generalises over: the status
//! held, how many times it was written, whether the VPC is also enabled for ClassicLink, which
//! VPC of several the request names, how many times it is named, and how the answer is paged.
//! The one recorded call — a describe naming a value that is no VPC id — is ONE row among them
//! rather than the specification.

use std::collections::BTreeMap;

use ec2::state::ComAmazonawsEc2Amazonec2State as Ec2State;
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, error_code, error_message, indexed, member};
use super::wire::{child_text, document, param, set_items, Param};

/// The Region every call here is signed for. A VPC's ClassicLink DNS support is a fact of the
/// VPC rather than of a Region, so the Region only selects the ec2Query envelope.
const REGION: &str = "us-west-2";

/// A VPC id of the right shape that the account holds no VPC for.
const UNHELD_VPC: &str = "vpc-0000000000000beef";

/// A VPC id in the SHORT (8 hexadecimal digit) form EC2 also issues, which the account holds
/// no VPC for either: a well-formed id is resolved against the store, whichever of the two
/// forms it takes.
const UNHELD_SHORT_VPC: &str = "vpc-0000beef";

// ---------------------------------------------------------------------------
// The table
// ---------------------------------------------------------------------------

/// One VPC this suite seeds: the CIDR it is created with, the ClassicLink DNS support writes
/// made against it in order, and whether it is also enabled for ClassicLink.
#[derive(Debug, Clone, Copy)]
struct Row {
    case: &'static str,
    cidr: &'static str,
    /// What was written through the store's one DNS support seam, in order — which is what
    /// `Enable`/`DisableVpcClassicLinkDnsSupport` must write through. The LAST write is the
    /// status the describe must report, so a repeated or reversed write is a row of its own.
    dns_writes: &'static [bool],
    /// Whether the VPC is enabled for ClassicLink, which is a DIFFERENT fact of the VPC.
    classic_link_enabled: bool,
}

impl Row {
    /// The status the store holds for this row once its writes have been made.
    fn dns_supported(&self) -> bool {
        self.dns_writes.last().copied().unwrap_or(false)
    }
}

/// The VPCs the suite works over: both statuses, both ClassicLink enablements beside them,
/// repeated and reversed writes, and more VPCs than the smallest page the operation serves, so
/// the rule is asserted over a set of inputs rather than over one VPC.
const ROWS: [Row; 7] = [
    Row {
        case: "DNS support on, not enabled for ClassicLink",
        cidr: "10.0.0.0/16",
        dns_writes: &[true],
        classic_link_enabled: false,
    },
    Row {
        case: "DNS support never written, enabled for ClassicLink",
        cidr: "10.1.0.0/16",
        dns_writes: &[],
        classic_link_enabled: true,
    },
    Row {
        case: "DNS support on, enabled for ClassicLink",
        cidr: "10.2.0.0/16",
        dns_writes: &[true],
        classic_link_enabled: true,
    },
    Row {
        case: "DNS support never written, not enabled for ClassicLink",
        cidr: "10.3.0.0/16",
        dns_writes: &[],
        classic_link_enabled: false,
    },
    Row {
        case: "DNS support on and then off again",
        cidr: "10.4.0.0/16",
        dns_writes: &[true, false],
        classic_link_enabled: false,
    },
    Row {
        case: "DNS support on twice",
        cidr: "10.5.0.0/16",
        dns_writes: &[true, true],
        classic_link_enabled: true,
    },
    Row {
        case: "DNS support off without ever being on",
        cidr: "10.6.0.0/16",
        dns_writes: &[false],
        classic_link_enabled: false,
    },
];

// ---------------------------------------------------------------------------
// Driving the service
// ---------------------------------------------------------------------------

/// One `DescribeVpcClassicLinkDnsSupport` call, as its HTTP status and XML body.
async fn describe(router: &MultiServiceRouter, params: &[Param]) -> (u16, String) {
    super::ec2_query::call(router, REGION, "DescribeVpcClassicLinkDnsSupport", params).await
}

/// The form fields naming `VpcIds` entries, as ec2Query flattens that list: `VpcIds.N`, from
/// the member's own name in the model
/// (`smithy_query(query="operation",operation="DescribeVpcClassicLinkDnsSupport")` reports
/// `VpcIds`), which is the PLURAL here where `DescribeVpcClassicLink`'s is the singular
/// `VpcId`.
fn vpc_ids(values: &[&str]) -> Vec<Param> {
    indexed("VpcIds", values)
}

/// Every `vpcs/item` of a response: the `classicLinkDnsSupported` it reports, by the `VpcId`
/// it reports it for.
fn reported(case: &str, body: &str) -> BTreeMap<String, String> {
    let parsed = document(case, body);
    let root = parsed.root_element();
    let mut rows = BTreeMap::new();
    for item in set_items(root, "vpcs") {
        let vpc_id = child_text(item, "vpcId")
            .unwrap_or_else(|| panic!("case {case:?}: an item with no <vpcId> in {body}"));
        let supported = child_text(item, "classicLinkDnsSupported").unwrap_or_else(|| {
            panic!("case {case:?}: {vpc_id} reported with no <classicLinkDnsSupported> in {body}")
        });
        rows.insert(vpc_id, supported);
    }
    rows
}

/// One describe that must succeed, as the rows it reported.
async fn described(
    router: &MultiServiceRouter,
    case: &str,
    params: &[Param],
) -> BTreeMap<String, String> {
    let (status, body) = describe(router, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeVpcClassicLinkDnsSupport, got {status} \
         with body {body}"
    );
    reported(case, &body)
}

/// One describe that must succeed, as the `VpcId`s it reported in the order it reported them
/// and the `NextToken` it closed with.
async fn described_page(
    router: &MultiServiceRouter,
    case: &str,
    params: &[Param],
) -> (Vec<String>, Option<String>) {
    let (status, body) = describe(router, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeVpcClassicLinkDnsSupport, got {status} \
         with body {body}"
    );
    let parsed = document(case, &body);
    let ids: Vec<String> = set_items(parsed.root_element(), "vpcs")
        .into_iter()
        .map(|item| child_text(item, "vpcId").unwrap_or_default())
        .collect();
    (ids, member(&body, "nextToken"))
}

/// A VPC `CreateVpc` makes, as its `VpcId`.
async fn create_vpc(router: &MultiServiceRouter, case: &str, cidr: &str) -> String {
    let (status, body) =
        super::ec2_query::call(router, REGION, "CreateVpc", &[param("CidrBlock", cidr)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateVpc must succeed, got {status} with body {body}"
    );
    element(&body, "vpcId").unwrap_or_else(|| panic!("case {case:?}: no <vpcId> in {body}"))
}

/// The emulator with [`ROWS`] seeded into it: one VPC per row, with its DNS support writes and
/// its ClassicLink enablement made through the store's own seams — which is what
/// `EnableVpcClassicLinkDnsSupport` and `EnableVpcClassicLink` (both still scaffolds) must
/// write through.
async fn emulator() -> (
    MultiServiceRouter,
    std::sync::Arc<engine::InMemoryEngine>,
    Vec<String>,
) {
    let (router, engine) = super::ec2_query::emulator();
    let mut ids = Vec::new();
    for row in ROWS {
        let vpc_id = create_vpc(&router, row.case, row.cidr).await;
        for enabled in row.dns_writes {
            let took = engine
                .state::<Ec2State>()
                .lock()
                .set_vpc_classic_link_dns_enabled(&vpc_id, *enabled);
            assert!(
                took,
                "case {case:?}: the store must take the DNS support write {enabled} for \
                 {vpc_id}",
                case = row.case
            );
        }
        if row.classic_link_enabled {
            let took = engine
                .state::<Ec2State>()
                .lock()
                .set_vpc_classic_link_enabled(&vpc_id, true);
            assert!(
                took,
                "case {case:?}: the store must take the ClassicLink enablement of {vpc_id}",
                case = row.case
            );
        }
        ids.push(vpc_id);
    }
    (router, engine, ids)
}

/// `"true"` / `"false"` as the wire spells a reported boolean.
fn wire_bool(value: bool) -> String {
    if value { "true" } else { "false" }.to_string()
}

/// The status every seeded row must be reported with, by its `VpcId`.
fn expected_statuses(ids: &[String]) -> BTreeMap<String, String> {
    ids.iter()
        .enumerate()
        .map(|(index, vpc_id)| (vpc_id.clone(), wire_bool(ROWS[index].dns_supported())))
        .collect()
}

// ---------------------------------------------------------------------------
// The status the store holds is what a describe reports
// ---------------------------------------------------------------------------

/// Each VPC is reported with the DNS support status the store holds for it — the last value
/// written through the store's seam, and `false` for a VPC nothing was ever written for — and
/// describing changes nothing, so a second call reports exactly the same thing.
#[tokio::test]
async fn each_vpc_is_reported_with_the_dns_support_status_the_store_holds() {
    let (router, engine, ids) = emulator().await;

    let first = described(&router, "every VPC", &[]).await;
    for (index, vpc_id) in ids.iter().enumerate() {
        let row = ROWS[index];
        let want = wire_bool(row.dns_supported());
        let got = first
            .get(vpc_id)
            .unwrap_or_else(|| panic!("case {case:?}: {vpc_id} must be reported, got {first:?}", case = row.case));
        assert_eq!(
            *got, want,
            "case {case:?}: expected {vpc_id} reported as classicLinkDnsSupported {want}, got \
             {got:?}",
            case = row.case
        );
    }

    // A read may not change what a later caller sees: the same report, and the store still
    // holds exactly the VPCs that were enabled.
    let second = described(&router, "described twice", &[]).await;
    assert_eq!(
        second, first,
        "described twice: expected the same report, got {second:?} after {first:?}"
    );
    let mut held: Vec<String> = engine
        .state::<Ec2State>()
        .lock()
        .classic_link_dns_enabled_vpcs
        .iter()
        .cloned()
        .collect();
    held.sort();
    let mut want: Vec<String> = ids
        .iter()
        .enumerate()
        .filter(|(index, _)| ROWS[*index].dns_supported())
        .map(|(_, vpc_id)| vpc_id.clone())
        .collect();
    want.sort();
    assert_eq!(
        held, want,
        "after two describes: expected the store to hold the DNS support of {want:?}, got \
         {held:?}"
    );
}

/// The DNS support status is a fact of its own: a VPC enabled for ClassicLink with no DNS
/// support is reported as not supporting it, and one with DNS support that is not enabled for
/// ClassicLink is reported as supporting it — so neither report can be read off the other.
#[tokio::test]
async fn dns_support_is_independent_of_the_classic_link_enablement() {
    let (router, _engine, ids) = emulator().await;

    let dns = described(&router, "DNS support", &[]).await;

    let (status, body) =
        super::ec2_query::call(&router, REGION, "DescribeVpcClassicLink", &[]).await;
    assert_eq!(
        status, 200,
        "DescribeVpcClassicLink must succeed, got {status} with body {body}"
    );
    let parsed = document("ClassicLink", &body);
    let classic_link: BTreeMap<String, String> = set_items(parsed.root_element(), "vpcSet")
        .into_iter()
        .map(|item| {
            (
                child_text(item, "vpcId").unwrap_or_default(),
                child_text(item, "classicLinkEnabled").unwrap_or_default(),
            )
        })
        .collect();

    for (index, vpc_id) in ids.iter().enumerate() {
        let row = ROWS[index];
        let want_dns = wire_bool(row.dns_supported());
        let want_classic_link = wire_bool(row.classic_link_enabled);
        assert_eq!(
            dns.get(vpc_id).map(String::as_str),
            Some(want_dns.as_str()),
            "case {case:?}: expected {vpc_id} reported as classicLinkDnsSupported {want_dns}, \
             got {actual:?}",
            case = row.case,
            actual = dns.get(vpc_id)
        );
        assert_eq!(
            classic_link.get(vpc_id).map(String::as_str),
            Some(want_classic_link.as_str()),
            "case {case:?}: expected {vpc_id} reported as classicLinkEnabled \
             {want_classic_link}, got {actual:?}",
            case = row.case,
            actual = classic_link.get(vpc_id)
        );
    }
}

// ---------------------------------------------------------------------------
// Narrowing a describe
// ---------------------------------------------------------------------------

/// `VpcIds` narrows the reported set to the ids named, in `VpcId` order, and a repeated id
/// reports that VPC once.
#[tokio::test]
async fn describing_narrows_by_vpc_ids() {
    let (router, _engine, ids) = emulator().await;

    let expected = expected_statuses(&ids);
    let cases: [(&str, Vec<&str>); 5] = [
        ("one id", vec![ids[0].as_str()]),
        ("two ids", vec![ids[4].as_str(), ids[1].as_str()]),
        ("one id twice", vec![ids[2].as_str(), ids[2].as_str()]),
        (
            "three ids, one of them twice",
            vec![ids[6].as_str(), ids[3].as_str(), ids[6].as_str()],
        ),
        (
            "every id",
            ids.iter().map(String::as_str).collect::<Vec<&str>>(),
        ),
    ];
    for (case, wanted) in cases {
        let got = described(&router, case, &vpc_ids(&wanted)).await;
        let mut want: BTreeMap<String, String> = BTreeMap::new();
        for vpc_id in &wanted {
            let status = expected
                .get(*vpc_id)
                .unwrap_or_else(|| panic!("case {case:?}: {vpc_id} is no row of the table"));
            want.insert((*vpc_id).to_string(), status.clone());
        }
        assert_eq!(
            got, want,
            "case {case:?}: expected the statuses {want:?}, got {got:?}"
        );
    }
}

/// A well-formed `VpcIds` entry the account holds no VPC for is a refusal, not an absent row —
/// in either of the two id forms EC2 issues, and including a VPC the account has deleted — and
/// the request is refused as a whole rather than answered with the VPCs it did resolve.
#[tokio::test]
async fn a_vpc_id_the_account_holds_no_vpc_for_is_refused_as_not_found() {
    let (router, _engine, ids) = emulator().await;

    let deleted = create_vpc(&router, "deleted", "10.9.0.0/16").await;
    let (status, body) =
        super::ec2_query::call(&router, REGION, "DeleteVpc", &[param("VpcId", &deleted)]).await;
    assert_eq!(
        status, 200,
        "deleted: DeleteVpc must succeed, got {status} with body {body}"
    );

    for (case, vpc_id) in [
        ("a long id the account never held", UNHELD_VPC),
        ("a short id the account never held", UNHELD_SHORT_VPC),
        ("a VPC the account deleted", deleted.as_str()),
    ] {
        let (status, body) = describe(&router, &vpc_ids(&[ids[0].as_str(), vpc_id])).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidVpcID.NotFound",
            "case {case:?}: unexpected error code in {body}"
        );
        assert_eq!(
            error_message(&body),
            format!("The vpc ID '{vpc_id}' does not exist"),
            "case {case:?}: unexpected message in {body}"
        );
    }
}

/// A `VpcIds` entry that is no VPC id at all is refused before any lookup: the id form is the
/// prefix, a hyphen and 8 or 17 lowercase hexadecimal digits, so every value outside it is
/// malformed whatever the account holds.
#[tokio::test]
async fn a_vpc_ids_entry_that_is_no_vpc_id_is_refused_as_malformed() {
    let (router, _engine, ids) = emulator().await;

    for (case, vpc_id) in [
        ("a value that is no id at all", "not-a-valid-vpc-id"),
        ("the empty value", ""),
        ("the prefix with no digits", "vpc-"),
        ("one digit short of the short form", "vpc-0123456"),
        ("one digit past the short form", "vpc-012345678"),
        ("one digit short of the long form", "vpc-0123456789abcdef"),
        ("one digit past the long form", "vpc-0123456789abcdef01"),
        ("a digit outside hexadecimal", "vpc-0123456g"),
        ("the long form in upper case", "vpc-0123456789ABCDEF0"),
        ("the prefix in upper case", "VPC-01234567"),
        ("an id of another resource kind", "subnet-01234567"),
        ("a well-formed id with trailing space", "vpc-01234567 "),
    ] {
        let (status, body) = describe(&router, &vpc_ids(&[vpc_id])).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidVpcId.Malformed",
            "case {case:?}: unexpected error code in {body}"
        );
        assert_eq!(
            error_message(&body),
            format!("{vpc_id} is not a valid VPC id"),
            "case {case:?}: unexpected message in {body}"
        );
    }

    // The malformed half is answered BEFORE the lookup: a malformed value beside a VPC the
    // account does hold is still malformed.
    let (status, body) = describe(
        &router,
        &vpc_ids(&[ids[0].as_str(), "not-a-valid-vpc-id"]),
    )
    .await;
    assert_eq!(
        status, 400,
        "a malformed id beside a held one: expected HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidVpcId.Malformed",
        "a malformed id beside a held one: unexpected error code in {body}"
    );
}

// ---------------------------------------------------------------------------
// Paging
// ---------------------------------------------------------------------------

/// A paged walk reports every VPC exactly once, in `VpcId` order, with each page carrying a
/// token only while a further VPC follows it — at the smallest page the operation publishes,
/// at the largest, and at a size in between.
#[tokio::test]
async fn paging_walks_every_vpc_once_in_vpc_id_order() {
    let (router, _engine, _ids) = emulator().await;

    let (whole, token) = described_page(&router, "unpaged", &[]).await;
    assert!(
        token.is_none(),
        "unpaged: a page holding every VPC must carry no nextToken, got {token:?}"
    );
    let mut ordered = whole.clone();
    ordered.sort();
    assert_eq!(
        whole, ordered,
        "unpaged: expected the VPCs in VpcId order, got {whole:?}"
    );

    // 5 and 255 are the two sides of the published page-size range
    // (`smithy_query(query="operation",operation="DescribeVpcClassicLinkDnsSupport")`:
    // `MaxResults` range 5..255); 6 is a size in between that does not divide the set.
    for page_size in [5_usize, 6, 255] {
        let case = format!("MaxResults {page_size}");
        let mut walked: Vec<String> = Vec::new();
        let mut next: Option<String> = None;
        for step in 0..=whole.len() {
            let mut params = vec![param("MaxResults", &page_size.to_string())];
            if let Some(token) = next.as_deref() {
                params.push(param("NextToken", token));
            }
            let step_case = format!("{case}, page {step}");
            let (page, token) = described_page(&router, &step_case, &params).await;
            assert!(
                page.len() <= page_size,
                "case {step_case:?}: a page may hold at most {page_size} VPCs, got {page:?}"
            );
            walked.extend(page);
            next = token;
            if next.is_none() {
                break;
            }
        }
        assert!(
            next.is_none(),
            "case {case:?}: the walk must end without a token, got {next:?}"
        );
        assert_eq!(
            walked, whole,
            "case {case:?}: expected the walk to report {whole:?} exactly once, got {walked:?}"
        );
    }
}

/// A `NextToken` this service did not mint is refused rather than read as a position, and a
/// page size outside the published range is refused rather than silently repaired.
#[tokio::test]
async fn a_token_this_service_did_not_mint_and_a_page_size_outside_the_range_are_refused() {
    let (router, _engine, _ids) = emulator().await;

    for (case, token) in [
        ("a token that is not even hex", "not-a-token"),
        ("hex that carries no marker", "deadbeef"),
    ] {
        let (status, body) = describe(&router, &[param("NextToken", token)]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidPaginationToken",
            "case {case:?}: unexpected error code in {body}"
        );
        assert_eq!(
            error_message(&body),
            ec2::state::INVALID_PAGINATION_TOKEN_MESSAGE,
            "case {case:?}: unexpected message in {body}"
        );
    }

    // One below the smallest page the operation publishes and one above the largest: both are
    // refused, and neither reports a VPC.
    for (case, max_results) in [
        ("one below the smallest page", "4"),
        ("one above the largest page", "256"),
    ] {
        let (status, body) = describe(&router, &[param("MaxResults", max_results)]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert!(
            !body.contains("<vpcId>"),
            "case {case:?}: a refused request must report no VPC, got {body}"
        );
    }
}
