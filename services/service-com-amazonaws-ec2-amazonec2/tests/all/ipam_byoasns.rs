// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DescribeIpamByoasn`, driven over the wire the service actually
//! speaks: `ec2Query` — an `application/x-www-form-urlencoded` POST whose body carries
//! `Action=DescribeIpamByoasn`, answered with XML (README.md / OPERATIONS.md "Wire
//! protocol").
//!
//! The rule under test: `DescribeIpamByoasn` REPORTS THE ASNs THE ACCOUNT HAS BROUGHT TO
//! IPAM OUT OF THE STORE — every member each one holds, in one stable order, one
//! `MaxResults`-sized page at a time behind an opaque `NextToken` that walks the whole set
//! exactly once; a token the service did not mint is refused rather than read as a
//! position; a `DryRun` request reports the permission check instead of reading; and the
//! report changes nothing a later caller sees, the only state it shows moving being the
//! provisioning the CLOCK already owed.
//!
//! Each table row is a distinct input along what that generalises over — how many ASNs the
//! account holds (none, one, several), which `AsnState` each sits in (the whole published
//! catalog), whether an ASN carries a status message, the page size at each published bound
//! and one past it, and the shape of a token the service did not mint — so no single
//! recorded request is the specification.
//!
//! The ASNs used are from the blocks RFC 5398 sets aside for documentation (64496-64511 for
//! 2-byte ASNs, 65536-65551 for 4-byte ASNs), so no row names an ASN that belongs to
//! anybody.
//!
//! The store is seeded directly, the way `ipam_byoasn_associations` and `describe_vpcs`
//! seed theirs: `ProvisionIpamByoasn` — the operation that brings an ASN to IPAM — is still
//! a scaffold, so the API has no path to an ASN in a chosen `AsnState` yet.

use std::time::Duration;

use ec2::ops::types::ASN_STATE_VALUES;
use ec2::state::{
    ByoasnRecord, ComAmazonawsEc2Amazonec2State as Ec2State, PendingStateMove, TRANSITION_DELAY,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, error_code, error_message, member};
use super::wire::{child_text, document, set_items};

/// The Region every call here is signed for. A BYOASN is named by itself and this store
/// holds the account's, not a Region's, so the Region only selects the ec2Query envelope.
const REGION: &str = "us-west-2";

/// The IPAM a seeded ASN was brought to (`Byoasn$IpamId`).
const IPAM_ID: &str = "ipam-00000000000000001";

/// The `AsnState` of an ASN IPAM has finished bringing to the account. Spelled out here
/// rather than imported, so the service cannot make this suite pass by agreeing with
/// itself.
const PROVISIONED: &str = "provisioned";

/// The `AsnState` of an ASN whose provision is still under way.
const PENDING_PROVISION: &str = "pending-provision";

/// EC2's code for a `NextToken` the service did not issue.
const INVALID_PAGINATION_TOKEN: &str = "InvalidPaginationToken";

/// EC2's code, and the status, for a request that only asked whether it would have been
/// permitted.
const DRY_RUN_OPERATION: &str = "DryRunOperation";
const DRY_RUN_STATUS: u16 = 412;

/// The largest page the model publishes for this operation
/// (`DescribeIpamByoasnMaxResults`, `range 1..100`), the size a case that is not about
/// paging asks for.
const WHOLE_PAGE: &str = "100";

/// A `(key, value)` request parameter.
fn p(key: &str, value: &str) -> (String, String) {
    (key.to_string(), value.to_string())
}

/// One `DescribeIpamByoasn` call carrying `params`, as its HTTP status and XML body.
async fn describe(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    call(router, REGION, "DescribeIpamByoasn", params).await
}

/// One ASN as the response reports it: every member `Byoasn` carries, so a member the
/// store holds and the answer drops cannot pass unnoticed.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Reported {
    asn: Option<String>,
    ipam_id: Option<String>,
    state: Option<String>,
    status_message: Option<String>,
}

/// One page of `DescribeIpamByoasn`: every ASN it reports, in the order it reported them,
/// plus the page's `NextToken`.
async fn described_page(
    router: &MultiServiceRouter,
    case: &str,
    params: &[(String, String)],
) -> (Vec<Reported>, Option<String>) {
    let (status, body) = describe(router, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeIpamByoasn, got {status} with body {body}"
    );
    let parsed = document(case, &body);
    let reported = set_items(parsed.root_element(), "byoasnSet")
        .into_iter()
        .map(|item| Reported {
            asn: child_text(item, "asn"),
            ipam_id: child_text(item, "ipamId"),
            state: child_text(item, "state"),
            status_message: child_text(item, "statusMessage"),
        })
        .collect();
    (reported, member(&body, "nextToken"))
}

/// Every ASN the account holds, as one page of the published maximum.
async fn described(router: &MultiServiceRouter, case: &str) -> Vec<Reported> {
    described_page(router, case, &[p("MaxResults", WHOLE_PAGE)])
        .await
        .0
}

/// Seed one ASN the account has brought to IPAM, sitting in `state` with `status_message`
/// and nothing owed to it by the clock.
fn seed_asn(
    engine: &engine::InMemoryEngine,
    asn: &str,
    state: &str,
    status_message: Option<&str>,
) {
    let mut record = ByoasnRecord::in_state(asn, IPAM_ID, state);
    record.byoasn.status_message = status_message.map(str::to_string);
    let stored = engine.state::<Ec2State>().lock().insert_byoasn(record);
    assert!(stored, "the store has room for the seeded ASN {asn}");
}

/// Seed one ASN whose provision is still under way, with the clock owing it `provisioned`.
fn seed_asn_being_provisioned(engine: &engine::InMemoryEngine, asn: &str) {
    let mut record = ByoasnRecord::in_state(asn, IPAM_ID, PENDING_PROVISION);
    record.pending = Some(PendingStateMove::starting_now(PROVISIONED));
    let stored = engine.state::<Ec2State>().lock().insert_byoasn(record);
    assert!(stored, "the store has room for the seeded ASN {asn}");
}

/// The `AsnState` the STORE holds for `asn`, read straight from it so it reports what a
/// later caller would be told without a call of its own materialising anything.
fn stored_state(engine: &engine::InMemoryEngine, asn: &str) -> Option<String> {
    let store = engine.state::<Ec2State>();
    let guard = store.lock();
    guard
        .byoasns
        .get(asn)
        .and_then(|record| record.byoasn.state.clone())
}

/// An account that has brought no ASN to IPAM is reported as holding none — a 200 with an
/// empty set, not a refusal, and no token to follow.
#[tokio::test]
async fn an_account_holding_no_asn_is_reported_empty() {
    let (router, _engine) = super::ec2_query::emulator();

    let (page, token) = described_page(&router, "a fresh account", &[]).await;
    assert!(
        page.is_empty(),
        "a fresh account has brought no ASN to IPAM, got {page:?}"
    );
    assert_eq!(
        token, None,
        "nothing follows an empty page, so no token is due, got {token:?}"
    );
}

/// Every member a stored ASN holds is reported, in each `AsnState` the model publishes and
/// with or without a status message.
#[tokio::test]
async fn every_member_a_stored_asn_holds_is_reported() {
    // The states come from the published catalog
    // (`smithy_query(query="shape",shape="AsnState")`, re-exported as `ASN_STATE_VALUES`),
    // so every state the model admits is a row — including the ones the issue did not cite.
    for state in ASN_STATE_VALUES {
        for (case, asn, status_message) in [
            ("a 2-byte ASN with no status message", "64496", None),
            (
                "a 4-byte ASN with a status message",
                "65536",
                Some("ASN brought to IPAM"),
            ),
        ] {
            let case = format!("{case} in {state}");
            let (router, engine) = super::ec2_query::emulator();
            seed_asn(&engine, asn, state, status_message);

            let reported = described(&router, &case).await;
            let expected = vec![Reported {
                asn: Some(asn.to_string()),
                ipam_id: Some(IPAM_ID.to_string()),
                state: Some((*state).to_string()),
                status_message: status_message.map(str::to_string),
            }];
            assert_eq!(
                reported, expected,
                "case {case:?}: expected {expected:?}, got {reported:?}"
            );
        }
    }
}

/// The ASNs the account holds are reported in the store's own key order — ascending by the
/// ASN as the API spells it — whatever order they were brought in, and two reads agree.
#[tokio::test]
async fn the_asns_are_reported_in_one_stable_order() {
    let (router, engine) = super::ec2_query::emulator();

    // Brought in neither ascending nor descending order, and spanning both ASN widths, so
    // the order reported can be neither the insertion order nor one width's.
    for asn in ["65540", "64496", "65536", "64510", "64497"] {
        seed_asn(&engine, asn, PROVISIONED, None);
    }
    let expected: Vec<String> = ["64496", "64497", "64510", "65536", "65540"]
        .iter()
        .map(|asn| (*asn).to_string())
        .collect();

    let first: Vec<String> = described(&router, "first read")
        .await
        .into_iter()
        .filter_map(|reported| reported.asn)
        .collect();
    assert_eq!(
        first, expected,
        "case \"first read\": expected {expected:?}, got {first:?}"
    );

    let second: Vec<String> = described(&router, "second read")
        .await
        .into_iter()
        .filter_map(|reported| reported.asn)
        .collect();
    assert_eq!(
        second, expected,
        "case \"second read\": a report changes nothing, so the order holds: expected \
         {expected:?}, got {second:?}"
    );
}

/// Every page size walks the whole set exactly once: a page is never longer than asked
/// for, a page that another ASN follows is full and carries a token, and the walk ends
/// without one.
#[tokio::test]
async fn pages_of_every_size_walk_the_whole_set_exactly_once() {
    let asns = ["64496", "64497", "64510", "65536", "65540"];

    // Page sizes below, at and above the number of ASNs held, one that does not divide it,
    // and the published bounds themselves (`range 1..100`).
    for page_size in [1_usize, 2, 3, 5, 6, 100] {
        let case = format!("pages of {page_size}");
        let (router, engine) = super::ec2_query::emulator();
        for asn in asns {
            seed_asn(&engine, asn, PROVISIONED, None);
        }

        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0_usize;
        loop {
            let mut params = vec![p("MaxResults", &page_size.to_string())];
            if let Some(token) = &token {
                params.push(p("NextToken", token));
            }
            let (page, next) = described_page(&router, &case, &params).await;
            pages += 1;
            assert!(
                pages <= asns.len() + 2,
                "case {case:?}: the walk must terminate, it has taken {pages} pages"
            );
            assert!(
                page.len() <= page_size,
                "case {case:?}: a page is never longer than asked for, got {} entries",
                page.len()
            );
            if next.is_some() {
                assert_eq!(
                    page.len(),
                    page_size,
                    "case {case:?}: a page another ASN follows is full, got {} entries",
                    page.len()
                );
            }
            walked.extend(page.into_iter().filter_map(|reported| reported.asn));
            match next {
                None => break,
                Some(next) => token = Some(next),
            }
        }

        let expected: Vec<String> = asns.iter().map(|asn| (*asn).to_string()).collect();
        assert_eq!(
            walked, expected,
            "case {case:?}: the walk reports each ASN once, in order: expected {expected:?}, \
             got {walked:?}"
        );
    }
}

/// `MaxResults` outside the range the model publishes is refused at the boundary; each
/// published bound itself is accepted.
#[tokio::test]
async fn max_results_is_accepted_at_its_bounds_and_refused_past_them() {
    let (router, engine) = super::ec2_query::emulator();
    seed_asn(&engine, "64496", PROVISIONED, None);

    // The published bounds (`DescribeIpamByoasnMaxResults`, `range 1..100`) and one past
    // each, plus a negative value no bound admits.
    for (max_results, expected_status) in [("-1", 400), ("0", 400), ("1", 200), ("100", 200), ("101", 400)] {
        let (status, body) = describe(&router, &[p("MaxResults", max_results)]).await;
        assert_eq!(
            status, expected_status,
            "case MaxResults={max_results:?}: expected HTTP {expected_status}, got {status} \
             with body {body}"
        );
    }
}

/// A `NextToken` this service did not mint is refused with `InvalidPaginationToken`, in a
/// message naming no token, rather than read as a position.
#[tokio::test]
async fn a_next_token_this_service_did_not_mint_is_refused() {
    let (router, engine) = super::ec2_query::emulator();
    seed_asn(&engine, "64496", PROVISIONED, None);

    for token in ["64496", "not-a-token", "eyJhIjoxfQ==", "////"] {
        let (status, body) = describe(&router, &[p("NextToken", token)]).await;
        assert_eq!(
            status, 400,
            "case {token:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            INVALID_PAGINATION_TOKEN,
            "case {token:?}: expected {INVALID_PAGINATION_TOKEN}, got {body}"
        );
        let message = error_message(&body);
        assert_eq!(
            message,
            ec2::state::INVALID_PAGINATION_TOKEN_MESSAGE,
            "case {token:?}: expected EC2's own pagination-token sentence, got {message:?}"
        );
    }
}

/// A `DryRun` request reports the permission check and reads nothing; the ASNs the account
/// holds are still reported afterwards.
#[tokio::test]
async fn a_dry_run_reports_the_permission_check() {
    let (router, engine) = super::ec2_query::emulator();
    seed_asn(&engine, "64496", PROVISIONED, None);

    for params in [
        vec![p("DryRun", "true")],
        vec![p("DryRun", "true"), p("MaxResults", WHOLE_PAGE)],
    ] {
        let case = format!("{params:?}");
        let (status, body) = describe(&router, &params).await;
        assert_eq!(
            status, DRY_RUN_STATUS,
            "case {case:?}: expected HTTP {DRY_RUN_STATUS}, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            DRY_RUN_OPERATION,
            "case {case:?}: expected {DRY_RUN_OPERATION}, got {body}"
        );
    }

    // `DryRun=false` is a request like any other, and the dry runs above left the store
    // alone.
    let reported = described(&router, "after the dry runs").await;
    assert_eq!(
        reported.len(),
        1,
        "case \"after the dry runs\": the account still holds its one ASN, got {reported:?}"
    );
}

/// An ASN whose provision is still under way is reported in `pending-provision` until the
/// instant the clock owed it, and in `provisioned` from then on — the report itself moving
/// nothing before that instant.
#[tokio::test]
async fn the_clock_not_a_caller_finishes_a_provision() {
    let (router, engine) = super::ec2_query::emulator();
    let asn = "64497";
    seed_asn_being_provisioned(&engine, asn);

    for case in ["the first read", "the second read"] {
        let reported = described(&router, case).await;
        assert_eq!(
            reported.first().and_then(|entry| entry.state.clone()),
            Some(PENDING_PROVISION.to_string()),
            "case {case:?}: the provision is not due yet, got {reported:?}"
        );
        assert_eq!(
            stored_state(&engine, asn),
            Some(PENDING_PROVISION.to_string()),
            "case {case:?}: a report moves nothing the clock does not owe, got {:?}",
            stored_state(&engine, asn)
        );
    }

    tokio::time::sleep(TRANSITION_DELAY + Duration::from_millis(50)).await;

    let reported = described(&router, "once the provision is due").await;
    assert_eq!(
        reported.first().and_then(|entry| entry.state.clone()),
        Some(PROVISIONED.to_string()),
        "case \"once the provision is due\": expected {PROVISIONED}, got {reported:?}"
    );
    assert_eq!(
        stored_state(&engine, asn),
        Some(PROVISIONED.to_string()),
        "case \"once the provision is due\": the move the clock owed is materialised, got {:?}",
        stored_state(&engine, asn)
    );
}
