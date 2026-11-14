// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `AcceptAddressTransfer` — the accepting half of the Elastic IP
//! address transfer handshake — driven over the wire the service actually speaks:
//! `ec2Query`, through the crate's shared [`super::ec2_query`] harness.
//!
//! The rules under test are:
//!
//! 1. an acceptance TAKES THE STANDING OFFER ON THE NAMED ADDRESS: the stored record moves
//!    to `accepted`, carries the instant it was taken, keeps the address, the allocation and
//!    the transfer account the offer was started with, and every later reader
//!    (`DescribeAddressTransfers`) sees exactly that — while no other address's offer moves;
//! 2. an acceptance is refused when NO STORED TRANSFER HAS THAT ADDRESS ON OFFER — none was
//!    ever started on it, the source account withdrew it, or it has already been taken — with
//!    the transfer surface's own code and sentence, and stores nothing; a value that is not
//!    an IPv4 address at all is refused as the value it is, and a `DryRun` acceptance leaves
//!    the offer standing;
//! 3. the tags the call carries land ON THE ADDRESS, so `DescribeAddresses` reports them,
//!    and a specification naming any other resource type is refused instead of discarded.
//!
//! Each table row is a distinct input along what the rule generalises over — how many offers
//! the store holds, which one is accepted, which account it was offered to, which status the
//! record is in, which address value is refused, how many tags the acceptance carries — so no
//! row is the specification.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use service_com_amazonaws_ec2_amazonec2 as ec2;
use service_com_amazonaws_ec2_amazonec2::state::{
    ADDRESS_TRANSFER_ACCEPTED_VISIBILITY, ADDRESS_TRANSFER_OFFER_WINDOW,
    ADDRESS_TRANSFER_STATUS_ACCEPTED, ADDRESS_TRANSFER_STATUS_PENDING,
};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

use super::ec2_query::{call, element, elements, error_code, error_message, indexed};

/// The Region every call here is signed for.
const REGION: &str = "us-west-2";

/// Account ids the rows offer their addresses to: twelve decimal digits, none of them this
/// emulator's own account, and different from each other so one row's value cannot be
/// confused with another's.
const TRANSFER_ACCOUNTS: [&str; 3] = ["111122223333", "444455556666", "999999999999"];

/// How far a reported instant may sit from the documented span after the call, in seconds:
/// the span is measured when the handler runs, so only the test's own latency separates the
/// two.
const TOLERANCE_SECONDS: i64 = 300;

/// A `(key, value)` request parameter.
fn p(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

/// One request's form parameters, as the ec2Query harness takes them.
type Params = Vec<(String, String)>;

/// One row of a table that expects a refusal: `(case, the parameters, the status, the error
/// code, the message)`.
type RefusalRow = (&'static str, Params, u16, &'static str, String);

/// One row of the tagging table: `(case, the tags the acceptance carries, the tags the
/// address ends up with)`.
type TaggingRow = (&'static str, Vec<(&'static str, &'static str)>, Vec<(&'static str, &'static str)>);

fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    super::ec2_query::emulator()
}

async fn api(
    router: &MultiServiceRouter,
    action: &str,
    params: &[(String, String)],
) -> (u16, String) {
    call(router, REGION, action, params).await
}

/// Allocate one Elastic IP address and answer `(allocation id, public ip)`.
async fn allocate_one(router: &MultiServiceRouter, case: &str) -> (String, String) {
    let (status, body) = api(router, "AllocateAddress", &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from AllocateAddress, got {status} with body {body}"
    );
    (
        element(&body, "allocationId").unwrap_or_default(),
        element(&body, "publicIp").unwrap_or_default(),
    )
}

/// Allocate an address and start a transfer of it, answering `(allocation id, public ip)`.
async fn offer_one(
    router: &MultiServiceRouter,
    case: &str,
    transfer_account_id: &str,
) -> (String, String) {
    let (allocation_id, public_ip) = allocate_one(router, case).await;
    let (status, body) = api(
        router,
        "EnableAddressTransfer",
        &[
            p("AllocationId", allocation_id.as_str()),
            p("TransferAccountId", transfer_account_id),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from EnableAddressTransfer, got {status} with body \
         {body}"
    );
    (allocation_id, public_ip)
}

/// Accept the transfer of `address`, with `extra` beside it.
async fn accept(
    router: &MultiServiceRouter,
    address: &str,
    extra: &[(String, String)],
) -> (u16, String) {
    let mut params = vec![p("Address", address)];
    params.extend_from_slice(extra);
    api(router, "AcceptAddressTransfer", &params).await
}

/// The transfer `DescribeAddressTransfers` reports for `allocation_id`, as its response body.
async fn described_transfer(
    router: &MultiServiceRouter,
    case: &str,
    allocation_id: &str,
) -> String {
    let (status, body) = api(
        router,
        "DescribeAddressTransfers",
        &indexed("AllocationId", &[allocation_id]),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeAddressTransfers, got {status} with body \
         {body}"
    );
    body
}

/// The status `DescribeAddressTransfers` reports for `allocation_id`, or the empty string
/// when it reports no transfer of that address at all.
async fn reported_status(router: &MultiServiceRouter, case: &str, allocation_id: &str) -> String {
    let body = described_transfer(router, case, allocation_id).await;
    element(&body, "addressTransferStatus").unwrap_or_default()
}

/// A timestamp the wire carries, as whole seconds since the Unix epoch.
fn epoch_seconds(case: &str, rendered: &str) -> i64 {
    DateTime::from_str(rendered, Format::DateTime)
        .unwrap_or_else(|error| panic!("case {case:?}: {rendered:?} is not a timestamp: {error}"))
        .secs()
}

/// Now, in whole seconds since the Unix epoch.
fn now_seconds() -> i64 {
    let since_epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    i64::try_from(since_epoch.as_secs()).unwrap_or(i64::MAX)
}

/// A documented span, in whole seconds.
fn span_seconds(span: Duration) -> i64 {
    i64::try_from(span.as_secs()).unwrap_or(i64::MAX)
}

/// The tag `(key, value)` pairs `DescribeAddresses` reports for `allocation_id`.
async fn described_tags(
    router: &MultiServiceRouter,
    case: &str,
    allocation_id: &str,
) -> Vec<(String, String)> {
    let (status, body) = api(
        router,
        "DescribeAddresses",
        &indexed("AllocationId", &[allocation_id]),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeAddresses, got {status} with body {body}"
    );
    elements(&body, "key")
        .into_iter()
        .zip(elements(&body, "value"))
        .collect()
}

/// `TagSpecification.1.*` form fields for one resource type and its tags.
fn tag_specification(resource_type: &str, tags: &[(&str, &str)]) -> Params {
    let mut params = vec![p("TagSpecification.1.ResourceType", resource_type)];
    for (index, (key, value)) in tags.iter().enumerate() {
        params.push(p(
            &format!("TagSpecification.1.Tag.{}.Key", index + 1),
            *key,
        ));
        params.push(p(
            &format!("TagSpecification.1.Tag.{}.Value", index + 1),
            *value,
        ));
    }
    params
}

/// The sentence the transfer surface answers for an address it has no transfer to act on.
fn not_available(address: &str) -> String {
    format!("The Elastic IP address '{address}' does not exist or is not available.")
}

// ---------------------------------------------------------------------------
// Rule 1 — the acceptance takes the standing offer, and every reader sees it
// ---------------------------------------------------------------------------

/// Accepting the offer on an address moves the STORED record to `accepted` with the instant
/// it was taken, keeps every member the offer was started with, and leaves every other
/// address's offer exactly where it was.
#[tokio::test]
async fn accept_takes_the_standing_offer_and_the_report_reads_it_back() {
    // The rows differ in how many offers the store holds and WHICH of them is accepted —
    // the first, a middle one, the last — because the acceptance is named by address and
    // must find that address's offer among the others.
    let rows: [(&str, usize, usize); 4] = [
        ("the only offer", 1, 0),
        ("the first of two", 2, 0),
        ("the last of two", 2, 1),
        ("the middle of three", 3, 1),
    ];

    for (case, count, accepted_index) in rows {
        let (router, engine) = emulator();
        let mut offers: Vec<(String, String, &str)> = Vec::new();
        for index in 0..count {
            let account = TRANSFER_ACCOUNTS[index % TRANSFER_ACCOUNTS.len()];
            let (allocation_id, public_ip) = offer_one(&router, case, account).await;
            offers.push((allocation_id, public_ip, account));
        }
        let (allocation_id, public_ip, account) = offers[accepted_index].clone();

        let offered_expiry = epoch_seconds(
            case,
            &element(
                &described_transfer(&router, case, &allocation_id).await,
                "transferOfferExpirationTimestamp",
            )
            .unwrap_or_default(),
        );

        let accepted_at = now_seconds();
        let (status, body) = accept(&router, &public_ip, &[]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from AcceptAddressTransfer, got {status} with body \
             {body}"
        );
        assert_eq!(
            element(&body, "publicIp").as_deref(),
            Some(public_ip.as_str()),
            "case {case:?}: expected the accepted address {public_ip}, got {body}"
        );
        assert_eq!(
            element(&body, "allocationId").as_deref(),
            Some(allocation_id.as_str()),
            "case {case:?}: expected the allocation the offer was started on, got {body}"
        );
        assert_eq!(
            element(&body, "transferAccountId").as_deref(),
            Some(account),
            "case {case:?}: expected the transfer account the offer carries, got {body}"
        );
        assert_eq!(
            element(&body, "addressTransferStatus").as_deref(),
            Some(ADDRESS_TRANSFER_STATUS_ACCEPTED),
            "case {case:?}: a taken offer is accepted, got {body}"
        );
        let reported_accepted = element(&body, "transferOfferAcceptedTimestamp")
            .unwrap_or_else(|| panic!("case {case:?}: an accepted transfer carries the instant it was taken, got {body}"));
        let accepted_seconds = epoch_seconds(case, &reported_accepted);
        assert!(
            (accepted_seconds - accepted_at).abs() <= TOLERANCE_SECONDS,
            "case {case:?}: expected the acceptance instant near {accepted_at}, got \
             {accepted_seconds}"
        );
        assert_eq!(
            epoch_seconds(
                case,
                &element(&body, "transferOfferExpirationTimestamp").unwrap_or_default()
            ),
            offered_expiry,
            "case {case:?}: taking the offer does not move when it would have expired, got {body}"
        );

        // The round trip: what a later reader sees is the stored record, not the answer the
        // acceptance built.
        let described = described_transfer(&router, case, &allocation_id).await;
        assert_eq!(
            element(&described, "addressTransferStatus").as_deref(),
            Some(ADDRESS_TRANSFER_STATUS_ACCEPTED),
            "case {case:?}: the report reads the accepted record back, got {described}"
        );
        assert_eq!(
            element(&described, "transferOfferAcceptedTimestamp").as_deref(),
            Some(reported_accepted.as_str()),
            "case {case:?}: the acceptance instant is stored once, got {described}"
        );
        assert_eq!(
            element(&described, "publicIp").as_deref(),
            Some(public_ip.as_str()),
            "case {case:?}: the report carries the same address, got {described}"
        );

        // Reading it twice reports the same thing: a report cannot move the record.
        let again = described_transfer(&router, case, &allocation_id).await;
        assert_eq!(
            element(&again, "addressTransferStatus"),
            element(&described, "addressTransferStatus"),
            "case {case:?}: two reads of one accepted transfer answer the same, got {again}"
        );

        // Every OTHER address's offer is untouched: the acceptance acted on one address.
        for (other_id, _, _) in offers
            .iter()
            .enumerate()
            .filter(|(index, _)| *index != accepted_index)
            .map(|(_, offer)| offer)
        {
            let status = reported_status(&router, case, other_id).await;
            assert_eq!(
                status, ADDRESS_TRANSFER_STATUS_PENDING,
                "case {case:?}: the offer on {other_id} still stands, got {status:?}"
            );
        }

        // The documented fourteen days the accepted record stays visible for is the one
        // fact the modelled shape carries no member for, so it is read off the store.
        let visible_in = {
            let state = ec2::state::service_state(&*engine).expect("the EC2 store");
            let store = state.lock();
            let record = store
                .address_transfers
                .get(&allocation_id)
                .unwrap_or_else(|| panic!("case {case:?}: the accepted transfer is stored"));
            record
                .visible_until
                .duration_since(SystemTime::now())
                .unwrap_or_default()
        };
        let expected = span_seconds(ADDRESS_TRANSFER_ACCEPTED_VISIBILITY);
        let actual = span_seconds(visible_in);
        assert!(
            (expected - actual).abs() <= TOLERANCE_SECONDS,
            "case {case:?}: an accepted transfer stays visible for {expected} seconds, got \
             {actual}"
        );
        // The visibility of an accepted transfer is a LONGER span than the offer window it
        // replaces, so the two cannot be confused.
        assert!(
            actual > span_seconds(ADDRESS_TRANSFER_OFFER_WINDOW),
            "case {case:?}: the accepted window outlasts the offer window, got {actual}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 2 — nothing on offer at that address, nothing to accept
// ---------------------------------------------------------------------------

/// An acceptance is refused for every address no stored transfer has ON OFFER, and for a
/// value that is not an address at all; a `DryRun` acceptance leaves the offer standing.
#[tokio::test]
async fn accept_refuses_an_address_nothing_has_on_offer() {
    let (router, _engine) = emulator();

    // An address the account holds with no offer on it at all.
    let (_, no_offer) = allocate_one(&router, "accept refusals").await;
    // An address whose offer the source account withdrew.
    let (withdrawn_id, withdrawn) = offer_one(&router, "accept refusals", TRANSFER_ACCOUNTS[0]).await;
    let (status, body) = api(
        &router,
        "DisableAddressTransfer",
        &[p("AllocationId", withdrawn_id.as_str())],
    )
    .await;
    assert_eq!(
        status, 200,
        "withdrawing one offer must succeed, got {status} with body {body}"
    );
    // An address whose offer has already been taken.
    let (taken_id, taken) = offer_one(&router, "accept refusals", TRANSFER_ACCOUNTS[1]).await;
    let (status, body) = accept(&router, &taken, &[]).await;
    assert_eq!(
        status, 200,
        "the first acceptance must succeed, got {status} with body {body}"
    );
    // An offer that stays standing: every refusal below must leave it exactly as it is.
    let (standing_id, standing) = offer_one(&router, "accept refusals", TRANSFER_ACCOUNTS[2]).await;

    // An address outside the pool this emulator allocates from, so the account holds no
    // allocation for it (`203.0.113.0/24` is the documentation range RFC 5737 reserves).
    let unheld = "203.0.113.9";

    let rows: Vec<RefusalRow> = vec![
        (
            "an address the account holds no allocation for",
            vec![p("Address", unheld)],
            400,
            "InvalidAddress.NotFound",
            not_available(unheld),
        ),
        (
            "an allocated address with no offer on it",
            vec![p("Address", no_offer.as_str())],
            400,
            "InvalidAddress.NotFound",
            not_available(&no_offer),
        ),
        (
            "an address whose offer was withdrawn",
            vec![p("Address", withdrawn.as_str())],
            400,
            "InvalidAddress.NotFound",
            not_available(&withdrawn),
        ),
        (
            "an address whose offer has already been taken",
            vec![p("Address", taken.as_str())],
            400,
            "InvalidAddress.NotFound",
            not_available(&taken),
        ),
        (
            "a value that is not an address at all",
            vec![p("Address", "not-an-address")],
            400,
            "InvalidParameterValue",
            "Invalid value 'not-an-address' for address. It must be an IPv4 address".to_string(),
        ),
        (
            "an IPv6 address, which is not an Elastic IP address",
            vec![p("Address", "2001:db8::1")],
            400,
            "InvalidParameterValue",
            "Invalid value '2001:db8::1' for address. It must be an IPv4 address".to_string(),
        ),
        (
            "a dotted quad with an octet out of range",
            vec![p("Address", "54.0.0.256")],
            400,
            "InvalidParameterValue",
            "Invalid value '54.0.0.256' for address. It must be an IPv4 address".to_string(),
        ),
        (
            "a tag specification naming another resource type",
            {
                let mut params = vec![p("Address", standing.as_str())];
                params.extend(tag_specification("vpc", &[("Name", "misdirected")]));
                params
            },
            400,
            "InvalidParameterValue",
            "'vpc' is not a valid resource type for tagging on AcceptAddressTransfer".to_string(),
        ),
        (
            "a tag key in the reserved namespace",
            {
                let mut params = vec![p("Address", standing.as_str())];
                params.extend(tag_specification("elastic-ip", &[("aws:owner", "me")]));
                params
            },
            400,
            "InvalidParameterValue",
            "Tag key 'aws:owner' is invalid: the 'aws:' prefix is reserved".to_string(),
        ),
        (
            "DryRun on an acceptance that would have succeeded",
            vec![p("Address", standing.as_str()), p("DryRun", "true")],
            412,
            "DryRunOperation",
            "Request would have succeeded, but DryRun flag is set.".to_string(),
        ),
    ];

    for (case, params, expected_status, expected_code, expected_message) in rows {
        let (status, body) = api(&router, "AcceptAddressTransfer", &params).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code}, got {body}"
        );
        assert_eq!(
            error_message(&body),
            expected_message,
            "case {case:?}: expected the refusal's own sentence, got {body}"
        );

        // A refusal stored nothing: the standing offer is still standing, the withdrawn one
        // is still gone — withdrawing an offer leaves the account no transfer of that
        // address to report — and the taken one is still taken.
        for (allocation_id, expected) in [
            (&standing_id, ADDRESS_TRANSFER_STATUS_PENDING),
            (&withdrawn_id, ""),
            (&taken_id, ADDRESS_TRANSFER_STATUS_ACCEPTED),
        ] {
            let status = reported_status(&router, case, allocation_id).await;
            assert_eq!(
                status, expected,
                "case {case:?}: the transfer of {allocation_id} is still {expected}, got \
                 {status:?}"
            );
        }
        let tags = described_tags(&router, case, &standing_id).await;
        assert!(
            tags.is_empty(),
            "case {case:?}: a refused acceptance tags nothing, got {tags:?}"
        );
    }

    // The offer that stood through every refusal can still be accepted.
    let (status, body) = accept(&router, &standing, &[]).await;
    assert_eq!(
        status, 200,
        "the standing offer is still acceptable, got {status} with body {body}"
    );
}

// ---------------------------------------------------------------------------
// Rule 3 — the tags the acceptance carries land on the address
// ---------------------------------------------------------------------------

/// The tags an acceptance carries are assigned to the Elastic IP address it takes, so
/// `DescribeAddresses` reports them beside the tags the address already carried.
#[tokio::test]
async fn accept_tags_the_address_it_takes() {
    // The rows differ in how many tags the acceptance carries and whether one of them
    // replaces a tag the address already had.
    let rows: Vec<TaggingRow> = vec![
        ("no tags at all", Vec::new(), vec![("Owner", "TeamA")]),
        (
            "one tag",
            vec![("Name", "edge")],
            vec![("Owner", "TeamA"), ("Name", "edge")],
        ),
        (
            "two tags, one with an empty value",
            vec![("Name", "edge"), ("empty", "")],
            vec![("Owner", "TeamA"), ("Name", "edge"), ("empty", "")],
        ),
        (
            "a tag the address already carries, overwritten",
            vec![("Owner", "TeamB")],
            vec![("Owner", "TeamB")],
        ),
    ];

    for (case, accept_tags, expected) in rows {
        let (router, _engine) = emulator();
        // The address starts out with a tag of its own, so the acceptance's tags are seen to
        // MERGE rather than to replace the list.
        let (status, body) = api(
            &router,
            "AllocateAddress",
            &tag_specification("elastic-ip", &[("Owner", "TeamA")]),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from AllocateAddress, got {status} with body {body}"
        );
        let allocation_id = element(&body, "allocationId").unwrap_or_default();
        let public_ip = element(&body, "publicIp").unwrap_or_default();
        let (status, body) = api(
            &router,
            "EnableAddressTransfer",
            &[
                p("AllocationId", allocation_id.as_str()),
                p("TransferAccountId", TRANSFER_ACCOUNTS[0]),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from EnableAddressTransfer, got {status} with body \
             {body}"
        );

        let extra = if accept_tags.is_empty() {
            Params::new()
        } else {
            tag_specification("elastic-ip", &accept_tags)
        };
        let (status, body) = accept(&router, &public_ip, &extra).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from AcceptAddressTransfer, got {status} with body \
             {body}"
        );

        let reported = described_tags(&router, case, &allocation_id).await;
        let expected_pairs: Vec<(String, String)> = expected
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        assert_eq!(
            reported, expected_pairs,
            "case {case:?}: expected the address to carry {expected_pairs:?}, got {reported:?}"
        );
    }
}
