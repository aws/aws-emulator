// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the `domain-name` attribute of an Elastic IP address — its reverse
//! DNS (PTR) record — driven over the wire the service actually speaks (`ec2Query`, through
//! the crate's shared [`super::ec2_query`] harness).
//!
//! The rules under test are:
//!
//! 1. `ResetAddressAttribute` TAKES THE NAMED ADDRESS'S PTR RECORD AWAY and reports the
//!    change as outstanding until it comes due: the attribute it answers is the stored
//!    address's own (its public address and allocation id) with no record in effect and the
//!    change carried beside it (`ptrRecordUpdate/status`), and that change comes due one
//!    [`TRANSITION_DELAY`] later whether or not anybody calls again — after which the address
//!    carries neither a record nor an outstanding change. What an applied change leaves in
//!    effect is the one seam both writers of this attribute go through, so a reset over an
//!    address that HELD a record is [`super::address_attribute_modify`]'s subject;
//! 2. ONE OUTSTANDING REQUEST PER RECORD: while an update on an address has not come due, a
//!    further request to change that address's record is refused with EC2's own
//!    `PendingRequestAlreadyExists`, the refusal neither consumes nor replaces the
//!    outstanding update, and an update on a DIFFERENT address is unaffected;
//! 3. an allocation id that names no address of the account is refused — malformed before
//!    unknown — and a `DryRun` request is answered without starting anything, so neither
//!    stores a thing;
//! 4. `DescribeAddressesAttribute` is a REPORT of that state: it reports every address of
//!    the account whose attribute is SET, narrows by `AllocationIds`, pages in allocation-id
//!    order, and two identical calls answer the same thing. Which addresses carry a row at
//!    all is [`super::describe_addresses_attribute`]'s subject.
//!
//! Each table row is a distinct input along what the rule generalises over — which address
//! of how many is named, how many requests have already been made on it, which id form the
//! request carries, which ids narrow a report and at what page size — so no row is the
//! specification.

use std::time::Duration;

use service_com_amazonaws_ec2_amazonec2::state::{
    ADDRESS_ATTRIBUTE_DOMAIN_NAME, AddressPtrRecord, ComAmazonawsEc2Amazonec2State as Ec2State,
    INVALID_ALLOCATION_ID_MALFORMED, INVALID_ALLOCATION_ID_NOT_FOUND, INVALID_PAGINATION_TOKEN,
    PENDING_REQUEST_ALREADY_EXISTS, PTR_UPDATE_STATUS_PENDING,
};
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, error_code, error_message};
use super::wire::{child, child_text, document, set_items};

/// The Region every call here is signed for.
const REGION: &str = "us-west-2";

/// Long enough past the span the clock owes a PTR record's update that it is certainly
/// overdue: the deadline the polls below give up at, generous because what is under test is
/// the state the record reaches rather than how fast it reaches it.
fn update_deadline() -> Duration {
    Duration::from_secs(60)
}

/// How long a poll below waits between two reads of an address's attribute.
fn poll_interval() -> Duration {
    Duration::from_millis(50)
}

/// A `(key, value)` request parameter.
fn p(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

/// One request's form parameters, as the ec2Query harness takes them.
type Params = Vec<(String, String)>;

/// The `domain-name` attribute of one address as a response reported it.
#[derive(Debug, Default, PartialEq, Eq)]
struct Attribute {
    public_ip: Option<String>,
    allocation_id: Option<String>,
    ptr_record: Option<String>,
    update_status: Option<String>,
    update_value: Option<String>,
}

fn emulator() -> MultiServiceRouter {
    super::ec2_query::emulator().0
}

/// Allocate one Elastic IP address and answer `(allocation id, public address)`.
async fn allocate_one(router: &MultiServiceRouter, case: &str) -> (String, String) {
    let (status, body) = call(router, REGION, "AllocateAddress", &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from AllocateAddress, got {status} with body {body}"
    );
    let parsed = document(case, &body);
    let root = parsed.root_element();
    (
        child_text(root, "allocationId").unwrap_or_default(),
        child_text(root, "publicIp").unwrap_or_default(),
    )
}

/// One `ResetAddressAttribute` call for `allocation_id`, with `extra` form fields beside
/// the attribute every such request names.
async fn reset(
    router: &MultiServiceRouter,
    allocation_id: &str,
    extra: &[(String, String)],
) -> (u16, String) {
    let mut params: Params = vec![
        p("AllocationId", allocation_id),
        p("Attribute", ADDRESS_ATTRIBUTE_DOMAIN_NAME),
    ];
    params.extend_from_slice(extra);
    call(router, REGION, "ResetAddressAttribute", &params).await
}

/// Give `allocation_id` a reverse DNS record in effect, so a reset of it has one to take
/// away.
///
/// A reset acts on the record the address CARRIES, and an address that was never given one
/// has none to reset — so every row below that resets an address sets one first, through the
/// OTHER writer of this attribute (`ModifyAddressAttribute`), and waits for that update to
/// come due: one record carries at most one outstanding request at a time.
async fn give_record(router: &MultiServiceRouter, case: &str, allocation_id: &str) {
    let (status, body) = call(
        router,
        REGION,
        "ModifyAddressAttribute",
        &[
            p("AllocationId", allocation_id),
            p("DomainName", "mail.example.com"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from ModifyAddressAttribute, got {status} with \
         body {body}"
    );
    wait_until_applied(router, case, allocation_id).await;
}

/// Wait until `allocation_id` carries no outstanding update any more — the state the clock
/// owes a record a write has just been started on.
///
/// Polls for that STATE rather than sleeping for the span the clock owes it, so a loaded
/// machine cannot read the record before the move has been applied.
async fn wait_until_applied(router: &MultiServiceRouter, case: &str, allocation_id: &str) {
    let deadline = tokio::time::Instant::now() + update_deadline();
    loop {
        let reported = described_one(router, case, allocation_id).await;
        let outstanding = reported
            .as_ref()
            .and_then(|attribute| attribute.update_status.clone());
        if outstanding.is_none() {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "case {case:?}: the update on {allocation_id} must come due, last read \
             {reported:?}"
        );
        tokio::time::sleep(poll_interval()).await;
    }
}

/// The attribute carried by `node` — the `<address>` of a reset response, or one `<item>` of
/// a report's `addressSet`.
fn attribute_of(node: roxmltree::Node<'_, '_>) -> Attribute {
    let update = child(node, "ptrRecordUpdate");
    Attribute {
        public_ip: child_text(node, "publicIp"),
        allocation_id: child_text(node, "allocationId"),
        ptr_record: child_text(node, "ptrRecord"),
        update_status: update.and_then(|update| child_text(update, "status")),
        update_value: update.and_then(|update| child_text(update, "value")),
    }
}

/// The attribute a successful `ResetAddressAttribute` answered.
fn reset_attribute(case: &str, body: &str) -> Attribute {
    let parsed = document(case, body);
    let address = child(parsed.root_element(), "address")
        .unwrap_or_else(|| panic!("case {case:?}: no <address> in the response {body}"));
    attribute_of(address)
}

/// What `DescribeAddressesAttribute` reports for `params`: the attributes in the order it
/// reported them, and the page token it answered.
async fn described(
    router: &MultiServiceRouter,
    case: &str,
    params: &[(String, String)],
) -> (Vec<Attribute>, Option<String>) {
    let (status, body) = call(router, REGION, "DescribeAddressesAttribute", params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeAddressesAttribute, \
         got {status} with body {body}"
    );
    let parsed = document(case, &body);
    let root = parsed.root_element();
    let attributes = set_items(root, "addressSet")
        .into_iter()
        .map(attribute_of)
        .collect();
    (attributes, child_text(root, "nextToken"))
}

/// The attribute the report carries for `allocation_id`, or `None` when it reports no row
/// for that address.
async fn described_one(
    router: &MultiServiceRouter,
    case: &str,
    allocation_id: &str,
) -> Option<Attribute> {
    let (attributes, _) = described(router, case, &[p("AllocationId.1", allocation_id)]).await;
    attributes
        .into_iter()
        .find(|attribute| attribute.allocation_id.as_deref() == Some(allocation_id))
}

/// RULE 1: a reset starts an outstanding update on exactly the address it names, reported
/// the same way by the reset and by the report that reads it back, and the update reaches
/// the record one `TRANSITION_DELAY` later whether or not anybody calls again.
#[tokio::test]
async fn reset_starts_an_outstanding_update_on_the_named_address() {
    let router = emulator();
    // Four addresses, so "the address named" is a choice among several rather than the only
    // one in the store.
    let mut held: Vec<(String, String)> = Vec::new();
    for index in 0..4 {
        held.push(allocate_one(&router, &format!("the address {index}")).await);
    }
    held.sort_by(|left, right| left.0.cmp(&right.0));

    // Rows differing along which of the held addresses the request names — the first, one in
    // the middle and the last in the order the store reports them.
    let rows: [(&str, usize); 3] = [
        ("the first of four", 0),
        ("one in the middle of four", 2),
        ("the last of four", 3),
    ];
    for (case, index) in rows {
        let (allocation_id, public_ip) = held[index].clone();
        give_record(&router, case, &allocation_id).await;
        let (status, body) = reset(&router, &allocation_id, &[]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from ResetAddressAttribute, \
             got {status} with body {body}"
        );
        let answered = reset_attribute(case, &body);
        assert_eq!(
            answered,
            Attribute {
                public_ip: Some(public_ip.clone()),
                allocation_id: Some(allocation_id.clone()),
                // The address held a record, and the reset names no new one, so the update
                // it started carries no value and there is no record in effect.
                ptr_record: None,
                update_status: Some(PTR_UPDATE_STATUS_PENDING.to_string()),
                update_value: None,
            },
            "case {case:?}: expected the stored address with its update outstanding, \
             got {answered:?} from body {body}"
        );

        // The report reads the SAME state back: the named address carries the outstanding
        // update, and an address with no attribute set carries nothing to report, so it is
        // the ONE row the report answers.
        let (reported, token) = described(&router, case, &[]).await;
        assert_eq!(
            token, None,
            "case {case:?}: one row fits one page, so no token was expected, got {token:?}"
        );
        assert_eq!(
            reported,
            vec![Attribute {
                public_ip: Some(public_ip),
                allocation_id: Some(allocation_id.clone()),
                // The address held a record, and the reset names no new one.
                ptr_record: None,
                update_status: Some(PTR_UPDATE_STATUS_PENDING.to_string()),
                update_value: None,
            }],
            "case {case:?}: expected only the address whose record has an update \
             outstanding, got {reported:?}"
        );

        // The update was owed at its instant whether or not anybody called, so once it has
        // been applied the address carries neither the update nor a record — and an address
        // with no attribute at all is reported no row. Read until that holds rather than once
        // after a span of this test's choosing.
        let deadline = tokio::time::Instant::now() + update_deadline();
        loop {
            let settled = described_one(&router, case, &allocation_id).await;
            if settled.is_none() {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "case {case:?}: expected the update to have been applied, leaving the \
                 address with no attribute to report, last read {settled:?}"
            );
            tokio::time::sleep(poll_interval()).await;
        }
    }
}

/// RULE 2: one outstanding request per record — a further reset while one is outstanding is
/// refused, the refusal leaves that update alone, another address is unaffected, and the
/// address takes a new request once the first has come due.
#[tokio::test]
async fn a_further_reset_while_one_is_outstanding_is_refused() {
    let router = emulator();
    let (first, _) = allocate_one(&router, "the address being reset").await;
    let (second, _) = allocate_one(&router, "another address of the account").await;
    give_record(&router, "the address being reset", &first).await;
    give_record(&router, "another address of the account", &second).await;

    let (status, body) = reset(&router, &first, &[]).await;
    assert_eq!(
        status, 200,
        "the first reset must be accepted, got {status} with body {body}"
    );

    // Rows differing along how many requests have already been refused: the refusal must
    // not consume the outstanding update, so every further attempt gets the same answer.
    let rows: [(&str, Params); 3] = [
        ("the second request", Vec::new()),
        ("the third request", Vec::new()),
        // A dry run asks whether the request WOULD have worked; this one would not, so it is
        // refused for the reason it would have been refused for.
        ("a dry run while one is outstanding", vec![p("DryRun", "true")]),
    ];
    for (case, extra) in rows {
        let (status, body) = reset(&router, &first, &extra).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 while an update is outstanding, \
             got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            PENDING_REQUEST_ALREADY_EXISTS,
            "case {case:?}: expected {PENDING_REQUEST_ALREADY_EXISTS}, got body {body}"
        );
        assert_eq!(
            error_message(&body),
            "A pending request for this record already exists.",
            "case {case:?}: expected the service's own wording, got body {body}"
        );
    }

    // The refusal is about ONE record: another address of the account still takes a request.
    let (status, body) = reset(&router, &second, &[]).await;
    assert_eq!(
        status, 200,
        "a reset of another address must be accepted, got {status} with body {body}"
    );
    let answered = reset_attribute("another address", &body);
    assert_eq!(
        answered.update_status,
        Some(PTR_UPDATE_STATUS_PENDING.to_string()),
        "another address must carry its own outstanding update, got {answered:?}"
    );

    // Once the outstanding update has come due the record takes a new request again — given
    // a record to take away, the first one having been taken away by the reset above.
    wait_until_applied(&router, "the address being reset", &first).await;
    give_record(&router, "the address being reset again", &first).await;
    let (status, body) = reset(&router, &first, &[]).await;
    assert_eq!(
        status, 200,
        "a reset after the outstanding update came due must be accepted, \
         got {status} with body {body}"
    );
}

/// RULE 3: an allocation id that names no address of the account is refused — malformed
/// before unknown — and the refusal stores nothing.
#[tokio::test]
async fn reset_refuses_an_id_the_account_holds_no_address_for() {
    let router = emulator();
    let (held, _) = allocate_one(&router, "the stored address").await;
    // An address allocated and released: a well-formed id of the right shape that names no
    // address of the account any more.
    let (released, _) = allocate_one(&router, "the address to release").await;
    let (status, body) = call(
        &router,
        REGION,
        "ReleaseAddress",
        &[p("AllocationId", released.as_str())],
    )
    .await;
    assert_eq!(
        status, 200,
        "the release must succeed, got {status} with body {body}"
    );

    // An allocation id is `eipalloc-` followed by 8 or 17 lowercase hexadecimal digits (the
    // short and the long EC2 resource id form, as
    // `service_com_amazonaws_ec2_amazonec2::state::allocation_id_is_well_formed` reads one);
    // anything else is refused before it is looked up. Rows differ along the FORM of the
    // value: a word, another resource's id, the right prefix at the wrong length, the right
    // length in the wrong alphabet, and well-formed ids of both lengths that name nothing.
    let rows: [(&str, &str, &str); 7] = [
        ("not an id at all", "domain-name", INVALID_ALLOCATION_ID_MALFORMED),
        (
            "another resource's id",
            "i-0123456789abcdef0",
            INVALID_ALLOCATION_ID_MALFORMED,
        ),
        (
            "the right prefix at the wrong length",
            "eipalloc-0123",
            INVALID_ALLOCATION_ID_MALFORMED,
        ),
        (
            "the right length in the wrong alphabet",
            "eipalloc-0123456Z",
            INVALID_ALLOCATION_ID_MALFORMED,
        ),
        (
            "a well-formed short id nothing holds",
            "eipalloc-0a1b2c3d",
            INVALID_ALLOCATION_ID_NOT_FOUND,
        ),
        (
            "a well-formed long id nothing holds",
            "eipalloc-0a1b2c3d4e5f60718",
            INVALID_ALLOCATION_ID_NOT_FOUND,
        ),
        (
            "the id of a released address",
            released.as_str(),
            INVALID_ALLOCATION_ID_NOT_FOUND,
        ),
    ];
    for (case, allocation_id, expected_code) in rows {
        let (status, body) = reset(&router, allocation_id, &[]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code}, got body {body}"
        );
        let expected_message = if expected_code == INVALID_ALLOCATION_ID_MALFORMED {
            format!("Invalid id: \"{allocation_id}\"")
        } else {
            format!("The allocation ID '{allocation_id}' does not exist")
        };
        assert_eq!(
            error_message(&body),
            expected_message,
            "case {case:?}: expected the service's own wording, got body {body}"
        );
    }

    // Nothing was stored for any of them: the one address the account still holds carries no
    // outstanding update, so the report answers no row for it — and once it is given a record
    // a reset of it is accepted.
    let reported = described_one(&router, "after the refusals", &held).await;
    assert_eq!(
        reported
            .as_ref()
            .and_then(|attribute| attribute.update_status.clone()),
        None,
        "a refused request must store nothing, so the address must carry no outstanding \
         update, got {reported:?}"
    );
    give_record(&router, "the held address", &held).await;
    let (status, body) = reset(&router, &held, &[]).await;
    assert_eq!(
        status, 200,
        "the held address must still take a reset, got {status} with body {body}"
    );
}

/// RULE 3: an attribute other than the one `AddressAttributeName` has is refused by the
/// boundary's own enum check, and the refusal stores nothing.
#[tokio::test]
async fn reset_refuses_an_attribute_the_address_does_not_have() {
    let router = emulator();
    let (allocation_id, _) = allocate_one(&router, "the stored address").await;

    // Rows differing along the FORM of the value: another service's attribute name, the one
    // member in the wrong case, the member of another EC2 attribute enum, and nothing at
    // all. `AddressAttributeName` has exactly one member, `domain-name`
    // (`smithy_query(query="shape",shape="AddressAttributeName")`), so none of these names
    // an attribute of an Elastic IP address.
    let rows: [(&str, &str); 4] = [
        ("another service's attribute", "ptrRecord"),
        ("the one member in the wrong case", "Domain-Name"),
        ("another enum's member", "sourceDestCheck"),
        ("no value at all", ""),
    ];
    for (case, attribute) in rows {
        let params = vec![
            p("AllocationId", allocation_id.as_str()),
            p("Attribute", attribute),
        ];
        let (status, body) = call(&router, REGION, "ResetAddressAttribute", &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for an attribute that is not \
             {ADDRESS_ATTRIBUTE_DOMAIN_NAME}, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationError",
            "case {case:?}: expected the enum constraint's refusal, got body {body}"
        );
        let reported = described_one(&router, case, &allocation_id).await;
        assert_eq!(
            reported
                .as_ref()
                .and_then(|reported| reported.update_status.clone()),
            None,
            "case {case:?}: a refused request must start no update, got {reported:?}"
        );
    }

    // The attribute is the only thing that stopped the request: named as the model spells
    // it, the same request works on an address that carries a record.
    give_record(&router, "the stored address", &allocation_id).await;
    let (status, body) = reset(&router, &allocation_id, &[]).await;
    assert_eq!(
        status, 200,
        "the same request naming the modelled attribute must be accepted, \
         got {status} with body {body}"
    );
}

/// RULE 3: a `DryRun` request that would have succeeded is answered `DryRunOperation` and
/// starts nothing, so the address still takes a real request afterwards.
#[tokio::test]
async fn a_dry_run_starts_nothing() {
    let router = emulator();
    let (first, _) = allocate_one(&router, "the address a dry run names").await;
    let (second, _) = allocate_one(&router, "another address of the account").await;
    // A dry run asks whether the request WOULD have succeeded, so both addresses carry a
    // record for it to take away: a reset of an address that carries none would be refused
    // for that reason instead.
    give_record(&router, "the address a dry run names", &first).await;
    give_record(&router, "another address of the account", &second).await;

    // Rows differing along which address the dry run names and how many have already been
    // made: none of them may start an update on any address.
    let rows: [(&str, &str); 3] = [
        ("the first dry run", first.as_str()),
        ("a second dry run on the same address", first.as_str()),
        ("a dry run on another address", second.as_str()),
    ];
    for (case, allocation_id) in rows {
        let (status, body) = reset(&router, allocation_id, &[p("DryRun", "true")]).await;
        assert_eq!(
            status, 412,
            "case {case:?}: expected HTTP 412 from a dry run that would have succeeded, \
             got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "DryRunOperation",
            "case {case:?}: expected DryRunOperation, got body {body}"
        );
        let reported = described_one(&router, case, allocation_id).await;
        assert_eq!(
            reported
                .as_ref()
                .and_then(|attribute| attribute.update_status.clone()),
            None,
            "case {case:?}: a dry run must start no update, got {reported:?}"
        );
    }

    // The flag is the only thing that stopped the request, so without it the same request
    // works.
    let (status, body) = reset(&router, &first, &[p("DryRun", "false")]).await;
    assert_eq!(
        status, 200,
        "the same request without the flag must be accepted, got {status} with body {body}"
    );
}

/// RULE 4: the report refuses what it cannot READ — a value that is not an allocation id at
/// all, a token it did not issue — and answers a dry run without reporting a row. An
/// allocation id it CAN read narrows the report instead (see
/// [`super::describe_addresses_attribute`]).
#[tokio::test]
async fn the_report_refuses_what_it_cannot_answer() {
    let router = emulator();
    let (held, _) = allocate_one(&router, "the stored address").await;

    // Rows differing along the form of the value the request narrows or pages by: an id that
    // is not an allocation id at all, the right prefix at the wrong length, and a token this
    // service did not issue.
    let rows: [(&str, Params, &str); 3] = [
        (
            "not an allocation id at all",
            vec![p("AllocationId.1", "vpc-0123456789abcdef0")],
            INVALID_ALLOCATION_ID_MALFORMED,
        ),
        (
            "the right prefix at the wrong length",
            vec![p("AllocationId.1", "eipalloc-0123")],
            INVALID_ALLOCATION_ID_MALFORMED,
        ),
        (
            "a token this service did not issue",
            vec![p("NextToken", "not-a-token")],
            INVALID_PAGINATION_TOKEN,
        ),
    ];
    for (case, params, expected_code) in rows {
        let (status, body) = call(&router, REGION, "DescribeAddressesAttribute", &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code}, got body {body}"
        );
    }

    // A dry run of a request that would have succeeded reports no row at all: "If you have
    // the required permissions, the error response is DryRunOperation" (`DryRun`).
    let (status, body) = call(
        &router,
        REGION,
        "DescribeAddressesAttribute",
        &[p("AllocationId.1", held.as_str()), p("DryRun", "true")],
    )
    .await;
    assert_eq!(
        status, 412,
        "a dry run that would have succeeded must be refused 412, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "DryRunOperation",
        "a dry run must answer DryRunOperation, got body {body}"
    );
}

/// RULE 4: the report narrows by `AllocationIds`, pages in allocation-id order, and answers
/// the same thing twice.
#[tokio::test]
async fn the_report_narrows_by_id_and_pages_in_id_order() {
    let (router, engine) = super::ec2_query::emulator();
    let mut held: Vec<String> = Vec::new();
    for index in 0..5 {
        let (allocation_id, _) = allocate_one(&router, &format!("the address {index}")).await;
        held.push(allocation_id);
    }
    held.sort();
    // The report carries a row for an address whose `domain-name` attribute is SET, so each
    // of the five is given a record in effect — the state an applied update leaves behind,
    // which no implemented writer of this service can leave, so it is seeded straight into
    // the store the handler reads.
    for allocation_id in &held {
        engine
            .state::<Ec2State>()
            .lock()
            .address_ptr_records
            .insert(
                allocation_id.clone(),
                AddressPtrRecord {
                    ptr_record: Some(format!("{allocation_id}.example.com")),
                    pending_update: None,
                },
            );
    }

    // Rows differing along how many ids narrow the report and which: none (every address),
    // one, a subset in an order other than the store's, and every id at once. The attribute
    // the request names is the one member `AddressAttributeName` has, so a row naming it and
    // a row leaving it out ask for the same thing.
    let rows: [(&str, Params, Vec<String>); 5] = [
        ("no ids at all", Vec::new(), held.clone()),
        (
            "the attribute named, no ids",
            vec![p("Attribute", ADDRESS_ATTRIBUTE_DOMAIN_NAME)],
            held.clone(),
        ),
        (
            "one id",
            vec![p("AllocationId.1", held[2].as_str())],
            vec![held[2].clone()],
        ),
        (
            "a subset, named out of order",
            vec![
                p("AllocationId.1", held[3].as_str()),
                p("AllocationId.2", held[0].as_str()),
            ],
            vec![held[0].clone(), held[3].clone()],
        ),
        (
            "every id at once",
            (0..held.len())
                .map(|index| p(&format!("AllocationId.{}", index + 1), held[index].as_str()))
                .collect(),
            held.clone(),
        ),
    ];
    for (case, params, expected) in rows {
        let (attributes, token) = described(&router, case, &params).await;
        let reported: Vec<String> = attributes
            .iter()
            .map(|attribute| attribute.allocation_id.clone().unwrap_or_default())
            .collect();
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the addresses {expected:?} in id order, got {reported:?}"
        );
        assert_eq!(
            token, None,
            "case {case:?}: every selected address fits one page, got the token {token:?}"
        );
        // A report changes nothing a later caller sees: the same call answers the same.
        let (again, token_again) = described(&router, case, &params).await;
        assert_eq!(
            again, attributes,
            "case {case:?}: two identical reports must answer the same thing"
        );
        assert_eq!(
            token_again, token,
            "case {case:?}: two identical reports must answer the same token"
        );
    }

    // Rows differing along the page size: each page carries that many addresses and a token
    // naming where the next one starts, until the last page, which carries no token.
    for page_size in [1_usize, 2, 4] {
        let case = format!("pages of {page_size}");
        let mut seen: Vec<String> = Vec::new();
        let mut next: Option<String> = None;
        loop {
            let mut params = vec![p("MaxResults", page_size.to_string())];
            if let Some(token) = next.as_deref() {
                params.push(p("NextToken", token));
            }
            let (attributes, token) = described(&router, &case, &params).await;
            assert!(
                attributes.len() <= page_size,
                "case {case:?}: a page carries at most {page_size} addresses, \
                 got {}",
                attributes.len()
            );
            seen.extend(
                attributes
                    .iter()
                    .map(|attribute| attribute.allocation_id.clone().unwrap_or_default()),
            );
            match token {
                None => break,
                Some(token) => next = Some(token),
            }
        }
        assert_eq!(
            seen, held,
            "case {case:?}: paging must report every address exactly once, in id order"
        );
    }
}
