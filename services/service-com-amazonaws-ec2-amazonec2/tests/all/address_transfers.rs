// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the Elastic IP address transfer slice —
//! `DescribeAddressTransfers` with the two operations that write what it reports,
//! `EnableAddressTransfer` and `DisableAddressTransfer` — driven over the wire the service
//! actually speaks: `ec2Query`, through the crate's shared [`super::ec2_query`] harness.
//!
//! The rules under test are:
//!
//! 1. `DescribeAddressTransfers` REPORTS THE STORED TRANSFERS of the calling account: every
//!    member of a transfer `EnableAddressTransfer` persisted round-trips through it, with
//!    the service deriving the address, the status and the expiration instant rather than
//!    taking them from the request, and an account that has started none is answered with
//!    an empty `addressTransferSet` and HTTP 200;
//! 2. it NARROWS by `AllocationIds` and PAGES by `MaxResults`/`NextToken` in `AllocationId`
//!    order, where an id that names no transfer selects no row and a token this service did
//!    not mint is refused;
//! 3. it is a REPORT: two identical calls answer the same thing, so no later caller can
//!    tell a read was made;
//! 4. the status it reports comes from state: `DisableAddressTransfer` withdraws a `pending`
//!    offer — reporting it `disabled` and leaving the account with no transfer for that
//!    address at all — a later `EnableAddressTransfer` starts a new `pending` one, and a
//!    request the transfer surface has nothing to act on is refused with EC2's own error
//!    code and stores nothing.
//!
//! Each table row is a distinct input along what the rule generalises over — how many
//! transfers, which ids narrow the answer, which page size, which status the record is in,
//! which account id or allocation id is refused — so no row is the specification.

use std::time::{SystemTime, UNIX_EPOCH};

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use service_com_amazonaws_ec2_amazonec2::state::{
    ADDRESS_TRANSFER_OFFER_WINDOW, ADDRESS_TRANSFER_STATUS_DISABLED,
    ADDRESS_TRANSFER_STATUS_PENDING,
};
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, elements, error_code, indexed};

/// The Region every call here is signed for.
const REGION: &str = "us-west-2";

/// Account ids the rows transfer to: twelve decimal digits
/// (`crate::state::AWS_ACCOUNT_ID_DIGITS`), none of them this emulator's own account, and
/// deliberately different from each other so a row's value cannot be confused with another's.
const TRANSFER_ACCOUNTS: [&str; 4] = ["111122223333", "444455556666", "000000000001", "999999999999"];

/// How far a reported expiration instant may sit from the documented seven days after the
/// call, in seconds: the window is measured when the handler runs, so only the test's own
/// latency separates the two.
const EXPIRATION_TOLERANCE_SECONDS: i64 = 300;

/// A `(key, value)` request parameter.
fn p(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

/// One request's form parameters, as the ec2Query harness takes them.
type Params = Vec<(String, String)>;

/// One row of a table that expects a refusal: `(case, the parameters, the status, the
/// error code)`.
type RefusalRow = (&'static str, Params, u16, &'static str);

/// One row of the narrowing table: `(case, the narrowing parameters, the ids it selects)`.
type SelectionRow = (&'static str, Params, Vec<String>);

fn emulator() -> MultiServiceRouter {
    super::ec2_query::emulator().0
}

async fn api(router: &MultiServiceRouter, action: &str, params: &[(String, String)]) -> (u16, String) {
    call(router, REGION, action, params).await
}

async fn describe(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    api(router, "DescribeAddressTransfers", params).await
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

/// Start a transfer of `allocation_id` to `transfer_account_id`.
async fn enable(
    router: &MultiServiceRouter,
    allocation_id: &str,
    transfer_account_id: &str,
) -> (u16, String) {
    api(
        router,
        "EnableAddressTransfer",
        &[
            p("AllocationId", allocation_id),
            p("TransferAccountId", transfer_account_id),
        ],
    )
    .await
}

/// Allocate an address and start a transfer of it, answering `(allocation id, public ip)`.
async fn offer_one(
    router: &MultiServiceRouter,
    case: &str,
    transfer_account_id: &str,
) -> (String, String) {
    let (allocation_id, public_ip) = allocate_one(router, case).await;
    let (status, body) = enable(router, &allocation_id, transfer_account_id).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from EnableAddressTransfer, got {status} with body \
         {body}"
    );
    (allocation_id, public_ip)
}

/// The allocation ids `DescribeAddressTransfers` reports for `params`, in the order it
/// reported them.
async fn described_ids(
    router: &MultiServiceRouter,
    case: &str,
    params: &[(String, String)],
) -> Vec<String> {
    let (status, body) = describe(router, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeAddressTransfers, got {status} with body \
         {body}"
    );
    elements(&body, "allocationId")
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

/// The documented offer window, in whole seconds.
fn offer_window_seconds() -> i64 {
    i64::try_from(ADDRESS_TRANSFER_OFFER_WINDOW.as_secs()).unwrap_or(i64::MAX)
}

// ---------------------------------------------------------------------------
// Rule 1 — the report is the stored transfer
// ---------------------------------------------------------------------------

/// An account that has started no transfer is answered with an EMPTY transfer set and HTTP
/// 200 — not a refusal, and not a missing member.
#[tokio::test]
async fn an_account_with_no_transfer_is_answered_with_an_empty_set() {
    let router = emulator();

    // The rows differ in what else the account holds, because "no transfer" is not "no
    // addresses": an allocated address with no offer on it is still no transfer.
    let (status, body) = describe(&router, &[]).await;
    assert_eq!(
        status, 200,
        "a fresh account: expected HTTP 200, got {status} with body {body}"
    );
    assert!(
        body.contains("addressTransferSet"),
        "a fresh account: the answer carries the (empty) addressTransferSet, got {body}"
    );
    assert!(
        elements(&body, "allocationId").is_empty(),
        "a fresh account: expected no transfer reported, got {body}"
    );

    allocate_one(&router, "an address with no offer").await;
    let reported = described_ids(&router, "an address with no offer", &[]).await;
    assert!(
        reported.is_empty(),
        "an address with no offer: an allocation is not a transfer, got {reported:?}"
    );
}

/// `EnableAddressTransfer` persists the offer and `DescribeAddressTransfers` reports the
/// STORED record: the address is read off the allocation, the status is `pending`, the
/// expiration is the documented seven days out, and nothing is accepted yet.
#[tokio::test]
async fn describe_reports_every_member_of_the_stored_offer() {
    // The rows differ in how many transfers the account has started and which account each
    // goes to, which is what the rule generalises over.
    let rows: [(&str, usize); 3] = [
        ("one transfer", 1),
        ("two transfers", 2),
        ("four transfers", TRANSFER_ACCOUNTS.len()),
    ];

    for (case, count) in rows {
        let router = emulator();
        let mut expected: Vec<(String, String, &str)> = Vec::new();
        for index in 0..count {
            let account = TRANSFER_ACCOUNTS[index % TRANSFER_ACCOUNTS.len()];
            let started_at = now_seconds();
            let (allocation_id, public_ip) = offer_one(&router, case, account).await;

            // The call's own answer is the stored record, read back out of the store.
            let (_, body) = enable(&router, &allocation_id, account).await;
            let reported_expiry = element(&body, "transferOfferExpirationTimestamp")
                .unwrap_or_else(|| panic!("case {case:?}: the offer carries an expiration, {body}"));
            let expiry = epoch_seconds(case, &reported_expiry);
            let due = started_at + offer_window_seconds();
            assert!(
                (expiry - due).abs() <= EXPIRATION_TOLERANCE_SECONDS,
                "case {case:?}: the offer expires the documented window out ({due}), got {expiry}"
            );
            expected.push((allocation_id, public_ip, account));
        }

        // Read each transfer back on its own, so every member lines up with the address it
        // belongs to rather than with a position in a list.
        for (allocation_id, public_ip, account) in &expected {
            let named = indexed("AllocationId", &[allocation_id.as_str()]);
            let (status, body) = describe(&router, &named).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with body {body}"
            );
            assert_eq!(
                element(&body, "allocationId").as_deref(),
                Some(allocation_id.as_str()),
                "case {case:?}: expected the transfer of {allocation_id}, got {body}"
            );
            assert_eq!(
                element(&body, "publicIp").as_deref(),
                Some(public_ip.as_str()),
                "case {case:?}: the address is read off the allocation ({public_ip}), got {body}"
            );
            assert_eq!(
                element(&body, "transferAccountId").as_deref(),
                Some(*account),
                "case {case:?}: the transfer account the request named must round-trip, got {body}"
            );
            assert_eq!(
                element(&body, "addressTransferStatus").as_deref(),
                Some(ADDRESS_TRANSFER_STATUS_PENDING),
                "case {case:?}: an offer nobody has accepted is pending, got {body}"
            );
            assert!(
                element(&body, "transferOfferAcceptedTimestamp").is_none(),
                "case {case:?}: nothing has accepted the offer, so it carries no accepted \
                 timestamp, got {body}"
            );
            let expiry = epoch_seconds(
                case,
                &element(&body, "transferOfferExpirationTimestamp").unwrap_or_default(),
            );
            assert!(
                expiry > now_seconds(),
                "case {case:?}: a pending offer's expiration is still ahead, got {expiry}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 2 — narrowing and paging
// ---------------------------------------------------------------------------

/// `AllocationIds` narrows the answer to the transfers of the addresses it names, in
/// `AllocationId` order however the request ordered them; an id that names no transfer
/// selects no row rather than failing the call.
#[tokio::test]
async fn describe_narrows_by_allocation_ids() {
    let router = emulator();
    let mut offered: Vec<String> = Vec::new();
    for account in TRANSFER_ACCOUNTS.iter().take(3) {
        let (allocation_id, _) = offer_one(&router, "narrowing", account).await;
        offered.push(allocation_id);
    }
    // A fourth address with no offer on it, and a well-formed id the account holds no
    // address for at all.
    let (no_offer, _) = allocate_one(&router, "narrowing").await;
    let unknown = "eipalloc-0123456789abcdef0";
    let mut all = offered.clone();
    all.sort();

    let rows: Vec<SelectionRow> = vec![
        ("no ids reports every transfer", Vec::new(), all.clone()),
        (
            "one named id",
            indexed("AllocationId", &[offered[1].as_str()]),
            vec![offered[1].clone()],
        ),
        (
            "two named ids, reported in id order",
            indexed(
                "AllocationId",
                &[offered[2].as_str(), offered[0].as_str()],
            ),
            {
                let mut selected = vec![offered[0].clone(), offered[2].clone()];
                selected.sort();
                selected
            },
        ),
        (
            "an address with no offer selects nothing",
            indexed("AllocationId", &[no_offer.as_str()]),
            Vec::new(),
        ),
        (
            "a well-formed id the account holds no address for selects nothing",
            indexed("AllocationId", &[unknown]),
            Vec::new(),
        ),
        (
            "a named id with no transfer does not hide one that has",
            indexed("AllocationId", &[no_offer.as_str(), offered[0].as_str()]),
            vec![offered[0].clone()],
        ),
    ];

    for (case, params, expected) in rows {
        let reported = described_ids(&router, case, &params).await;
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
    }
}

/// `MaxResults` bounds the page and the token resumes from where it ended, so walking the
/// pages reports every transfer exactly once in `AllocationId` order; a request with no
/// `MaxResults` is answered in one page.
#[tokio::test]
async fn max_results_pages_the_set_and_the_token_resumes_it() {
    let router = emulator();
    let mut offered: Vec<String> = Vec::new();
    for index in 0..12 {
        let account = TRANSFER_ACCOUNTS[index % TRANSFER_ACCOUNTS.len()];
        let (allocation_id, _) = offer_one(&router, "paging", account).await;
        offered.push(allocation_id);
    }
    offered.sort();

    // `DescribeAddressTransfersMaxResults` is modelled `range 5..1000`, so the rows are its
    // lower bound, sizes that do and do not divide the set, the set's own size and the
    // published upper bound.
    for page_size in [5, 6, 7, 12, 1000] {
        let case = format!("MaxResults={page_size}");
        let mut params = vec![p("MaxResults", page_size.to_string())];
        let mut walked: Vec<String> = Vec::new();
        loop {
            let (status, body) = describe(&router, &params).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with body {body}"
            );
            let page = elements(&body, "allocationId");
            assert!(
                page.len() <= page_size,
                "case {case:?}: a page holds at most MaxResults transfers, got {page:?}"
            );
            walked.extend(page);
            match element(&body, "nextToken") {
                None => break,
                Some(token) => {
                    params = vec![p("MaxResults", page_size.to_string()), p("NextToken", token)];
                }
            }
        }
        assert_eq!(
            walked, offered,
            "case {case:?}: the walk must report every transfer exactly once in id order, got \
             {walked:?}"
        );
    }

    let unpaged = described_ids(&router, "no MaxResults", &[]).await;
    assert_eq!(
        unpaged, offered,
        "no MaxResults: expected every transfer in one answer, got {unpaged:?}"
    );
}

/// `MaxResults` outside the range the model publishes is refused at the boundary; each
/// published bound itself is accepted.
#[tokio::test]
async fn max_results_is_accepted_at_its_bounds_and_refused_past_them() {
    let router = emulator();
    offer_one(&router, "bounds", TRANSFER_ACCOUNTS[0]).await;

    // The published bounds (`range 5..1000`) and one past each.
    let rows = [(4, 400), (5, 200), (1000, 200), (1001, 400)];
    for (max_results, expected_status) in rows {
        let params = vec![p("MaxResults", max_results.to_string())];
        let (status, body) = describe(&router, &params).await;
        assert_eq!(
            status, expected_status,
            "case MaxResults={max_results}: expected HTTP {expected_status}, got {status} with \
             body {body}"
        );
    }
}

/// A `NextToken` this service did not mint is refused with `InvalidPaginationToken` rather
/// than read as a cursor, and a malformed `AllocationId` with
/// `InvalidAllocationID.Malformed` — neither is answered as "no results".
#[tokio::test]
async fn a_value_this_operation_cannot_read_is_refused() {
    let router = emulator();
    let (offered, _) = offer_one(&router, "refusals", TRANSFER_ACCOUNTS[0]).await;

    let rows: Vec<RefusalRow> = vec![
        (
            "a token that is not one of ours",
            vec![p("NextToken", "not-a-token")],
            400,
            "InvalidPaginationToken",
        ),
        (
            "a token shaped like our hex cursor",
            vec![p("NextToken", "0123456789abcdef0123456789abcdef")],
            400,
            "InvalidPaginationToken",
        ),
        (
            "an allocation id passed off as a token",
            vec![p("NextToken", offered.as_str())],
            400,
            "InvalidPaginationToken",
        ),
        (
            "an id of another resource kind",
            indexed("AllocationId", &["vpc-0123456789abcdef0"]),
            400,
            "InvalidAllocationID.Malformed",
        ),
        (
            "an allocation id with no digits",
            indexed("AllocationId", &["eipalloc-"]),
            400,
            "InvalidAllocationID.Malformed",
        ),
        (
            "an allocation id of the wrong length",
            indexed("AllocationId", &["eipalloc-0123"]),
            400,
            "InvalidAllocationID.Malformed",
        ),
        (
            "an allocation id with a non-hex digit",
            indexed("AllocationId", &["eipalloc-0123456z"]),
            400,
            "InvalidAllocationID.Malformed",
        ),
        (
            "DryRun on a request that would have succeeded",
            vec![p("DryRun", "true")],
            412,
            "DryRunOperation",
        ),
    ];

    for (case, params, expected_status, expected_code) in rows {
        let (status, body) = describe(&router, &params).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code}, got {body}"
        );
    }

    // A refusal changed nothing: the one transfer is still reported.
    let reported = described_ids(&router, "after the refusals", &[]).await;
    assert_eq!(
        reported,
        vec![offered.clone()],
        "after the refusals: expected the one transfer to still be reported, got {reported:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule 3 — the describe is a report
// ---------------------------------------------------------------------------

/// Two identical reads answer the same transfers, with the same timestamps and statuses: a
/// read cannot change what a later caller sees.
#[tokio::test]
async fn two_identical_reads_answer_the_same_thing() {
    let router = emulator();
    for account in TRANSFER_ACCOUNTS.iter().take(2) {
        offer_one(&router, "repeat reads", account).await;
    }

    let members = [
        "allocationId",
        "publicIp",
        "transferAccountId",
        "addressTransferStatus",
        "transferOfferExpirationTimestamp",
    ];
    let (_, first) = describe(&router, &[]).await;
    let (_, second) = describe(&router, &[]).await;
    for member in members {
        assert_eq!(
            elements(&first, member),
            elements(&second, member),
            "a second read reports the same {member}: {first} then {second}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 4 — the reported status comes from state
// ---------------------------------------------------------------------------

/// `DisableAddressTransfer` withdraws a `pending` offer: the call answers it `disabled` and
/// the account has no transfer for that address left to report; a second disable has nothing
/// to act on and is refused; a later enable starts a fresh `pending` offer on the same
/// address.
#[tokio::test]
async fn disable_withdraws_the_offer_and_enable_starts_a_new_one() {
    let router = emulator();
    let (allocation_id, public_ip) = offer_one(&router, "withdraw", TRANSFER_ACCOUNTS[0]).await;
    let named = indexed("AllocationId", &[allocation_id.as_str()]);
    let disable = vec![p("AllocationId", allocation_id.as_str())];

    let (status, body) = api(&router, "DisableAddressTransfer", &disable).await;
    assert_eq!(
        status, 200,
        "the disable: expected HTTP 200, got {status} with body {body}"
    );
    assert_eq!(
        element(&body, "addressTransferStatus").as_deref(),
        Some(ADDRESS_TRANSFER_STATUS_DISABLED),
        "the disable answers the withdrawn offer, got {body}"
    );

    assert_eq!(
        element(&body, "publicIp").as_deref(),
        Some(public_ip.as_str()),
        "the withdrawn offer names the address it was started for, got {body}"
    );

    let (_, body) = describe(&router, &named).await;
    assert!(
        elements(&body, "addressTransferStatus").is_empty(),
        "a withdrawn offer is no transfer of the account, got {body}"
    );
    assert!(
        elements(&body, "allocationId").is_empty(),
        "a withdrawn offer is no transfer of the account, got {body}"
    );

    // Nothing left to withdraw: EC2 answers the transfer surface's own code for an address
    // it has no transfer to act on.
    let (status, body) = api(&router, "DisableAddressTransfer", &disable).await;
    assert_eq!(
        status, 400,
        "a second disable: expected HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidAddress.NotFound",
        "a second disable: expected InvalidAddress.NotFound, got {body}"
    );

    // Enabling again starts a new offer on the same address, to a different account.
    let (status, body) = enable(&router, &allocation_id, TRANSFER_ACCOUNTS[1]).await;
    assert_eq!(
        status, 200,
        "the second enable: expected HTTP 200, got {status} with body {body}"
    );
    let (_, body) = describe(&router, &named).await;
    assert_eq!(
        element(&body, "addressTransferStatus").as_deref(),
        Some(ADDRESS_TRANSFER_STATUS_PENDING),
        "the second enable leaves a pending offer, got {body}"
    );
    assert_eq!(
        element(&body, "transferAccountId").as_deref(),
        Some(TRANSFER_ACCOUNTS[1]),
        "the new offer names the account the second enable asked for, got {body}"
    );
    assert_eq!(
        elements(&body, "allocationId").len(),
        1,
        "an address has at most one transfer, got {body}"
    );
}

/// `EnableAddressTransfer` refuses an allocation id it cannot resolve and a transfer
/// account the address cannot go to, and stores nothing when it does; `DryRun` on a request
/// that would have succeeded stores nothing either.
#[tokio::test]
async fn enable_refuses_what_it_cannot_transfer_and_stores_nothing() {
    let router = emulator();
    let (allocation_id, _) = allocate_one(&router, "enable refusals").await;
    let own_account = engine::emulator_account_id().to_string();

    let rows: Vec<RefusalRow> = vec![
        (
            "an allocation id no address of the account has",
            vec![
                p("AllocationId", "eipalloc-0123456789abcdef0"),
                p("TransferAccountId", TRANSFER_ACCOUNTS[0]),
            ],
            400,
            "InvalidAllocationID.NotFound",
        ),
        (
            "an allocation id of another resource kind",
            vec![
                p("AllocationId", "subnet-0123456789abcdef0"),
                p("TransferAccountId", TRANSFER_ACCOUNTS[0]),
            ],
            400,
            "InvalidAllocationID.Malformed",
        ),
        (
            "a transfer account id with too few digits",
            vec![
                p("AllocationId", allocation_id.as_str()),
                p("TransferAccountId", "11112222333"),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            "a transfer account id with too many digits",
            vec![
                p("AllocationId", allocation_id.as_str()),
                p("TransferAccountId", "1111222233334"),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            "a transfer account id that is not digits",
            vec![
                p("AllocationId", allocation_id.as_str()),
                p("TransferAccountId", "aws-account1"),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            "the account that already owns the address",
            vec![
                p("AllocationId", allocation_id.as_str()),
                p("TransferAccountId", own_account.as_str()),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            "DryRun on an enable that would have succeeded",
            vec![
                p("AllocationId", allocation_id.as_str()),
                p("TransferAccountId", TRANSFER_ACCOUNTS[0]),
                p("DryRun", "true"),
            ],
            412,
            "DryRunOperation",
        ),
    ];

    for (case, params, expected_status, expected_code) in rows {
        let (status, body) = api(&router, "EnableAddressTransfer", &params).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code}, got {body}"
        );
        let reported = described_ids(&router, case, &[]).await;
        assert!(
            reported.is_empty(),
            "case {case:?}: a refused enable stores nothing, got {reported:?}"
        );
    }
}

/// `DisableAddressTransfer` refuses an address it has no `pending` offer to withdraw, and a
/// `DryRun` disable leaves the offer standing.
#[tokio::test]
async fn disable_refuses_an_address_with_no_pending_offer() {
    let router = emulator();
    let (no_offer, _) = allocate_one(&router, "disable refusals").await;
    let (offered, _) = offer_one(&router, "disable refusals", TRANSFER_ACCOUNTS[2]).await;

    let rows: Vec<RefusalRow> = vec![
        (
            "an address with no offer on it",
            vec![p("AllocationId", no_offer.as_str())],
            400,
            "InvalidAddress.NotFound",
        ),
        (
            "an allocation id no address of the account has",
            vec![p("AllocationId", "eipalloc-0123456789abcdef0")],
            400,
            // This operation answers the OTHER of the two codes EC2 publishes for an
            // allocation id that resolves to no address of the account.
            "InvalidElasticIpID.NotFound",
        ),
        (
            "an allocation id of another resource kind",
            vec![p("AllocationId", "igw-0123456789abcdef0")],
            400,
            "InvalidAllocationID.Malformed",
        ),
        (
            "DryRun on a disable that would have succeeded",
            vec![p("AllocationId", offered.as_str()), p("DryRun", "true")],
            412,
            "DryRunOperation",
        ),
    ];

    for (case, params, expected_status, expected_code) in rows {
        let (status, body) = api(&router, "DisableAddressTransfer", &params).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code}, got {body}"
        );
    }

    // Every refusal above left the one standing offer exactly as it was.
    let named = indexed("AllocationId", &[offered.as_str()]);
    let (_, body) = describe(&router, &named).await;
    assert_eq!(
        element(&body, "addressTransferStatus").as_deref(),
        Some(ADDRESS_TRANSFER_STATUS_PENDING),
        "after the refusals: the offer is still pending, got {body}"
    );
}
