// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DescribeAddressesAttribute` as the REPORT it is, driven over the
//! wire the service actually speaks (`ec2Query`, through the crate's shared
//! [`super::ec2_query`] harness).
//!
//! The rule under test: the report is a projection of an Elastic IP address's ATTRIBUTE
//! state — "the attributes associated with an Elastic IP address" (`AddressAttribute`), the
//! reverse DNS record in effect (`AddressAttribute$PtrRecord`) and the update outstanding on
//! it (`AddressAttribute$PtrRecordUpdate`) — and not a lookup of addresses. So:
//!
//! 1. an address nothing has set the attribute on has no attributes to report and
//!    contributes no row, however many such addresses the account holds and however the
//!    request narrows;
//! 2. an address whose record carries a value or an outstanding update is reported, and
//!    stops being reported once the clock has taken the attribute away again;
//! 3. `AllocationIds` FILTERS those rows and asserts nothing about the account holding the
//!    address, so an id that selects no row contributes none and the answer is HTTP 200 with
//!    an empty `addressSet`;
//! 4. a value that is not an allocation id at all is refused, because the FORM of an id is
//!    not a narrowing this report can be asked for;
//! 5. the rows it reports are paged in allocation-id order, and two identical calls answer
//!    the same thing.
//!
//! Each table row is a distinct input along what the rule generalises over — how many
//! addresses the account holds, how many of them have the attribute set, which ids narrow the
//! report, the FORM of the id, and the page size — so no row is the specification.
//!
//! `ModifyAddressAttribute` is still a scaffold stub, so there is no API path that leaves a
//! PTR record in effect: the records a row needs are seeded straight into the store the
//! handler reads, the way the describe suites of this crate seed theirs. The one writer this
//! service does implement, `ResetAddressAttribute`, is what the clock-owed row below drives.

use std::sync::Arc;
use std::time::Duration;

use service_com_amazonaws_ec2_amazonec2::state::{
    ADDRESS_ATTRIBUTE_DOMAIN_NAME, AddressPtrRecord, ComAmazonawsEc2Amazonec2State as Ec2State,
    INVALID_ALLOCATION_ID_MALFORMED, PTR_UPDATE_STATUS_PENDING,
};
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, error_code, error_message};
use super::wire::{child, child_text, document, set_items};

/// The Region every call here is signed for.
const REGION: &str = "us-west-2";

/// A `(key, value)` request parameter.
fn p(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

/// One request's form parameters, as the ec2Query harness takes them.
type Params = Vec<(String, String)>;

/// Long enough past the span the clock owes a PTR record's update that it is certainly
/// overdue: the deadline the polls below give up at, generous because what is under test is
/// the state the record reaches rather than how fast it reaches it.
fn attribute_deadline() -> Duration {
    Duration::from_secs(60)
}

/// How long a poll below waits between two reads of the report.
fn poll_interval() -> Duration {
    Duration::from_millis(50)
}

/// The emulator the tests drive, and the engine its store is reached through so a row can
/// seed the attribute state no implemented writer can leave in effect.
fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    super::ec2_query::emulator()
}

/// The `domain-name` attribute of one address as a response reported it.
#[derive(Debug, Default, PartialEq, Eq)]
struct Attribute {
    public_ip: Option<String>,
    allocation_id: Option<String>,
    ptr_record: Option<String>,
    update_status: Option<String>,
}

/// The attribute carried by one `<item>` of a report's `addressSet`.
fn attribute_of(node: roxmltree::Node<'_, '_>) -> Attribute {
    Attribute {
        public_ip: child_text(node, "publicIp"),
        allocation_id: child_text(node, "allocationId"),
        ptr_record: child_text(node, "ptrRecord"),
        update_status: child(node, "ptrRecordUpdate")
            .and_then(|update| child_text(update, "status")),
    }
}

/// Allocate `count` Elastic IP addresses and answer their allocation ids in the order the
/// report pages over them.
async fn allocate(router: &MultiServiceRouter, case: &str, count: usize) -> Vec<String> {
    let mut held = Vec::new();
    for index in 0..count {
        let (status, body) = call(router, REGION, "AllocateAddress", &[]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 allocating address {index}, \
             got {status} with body {body}"
        );
        let parsed = document(case, &body);
        held.push(child_text(parsed.root_element(), "allocationId").unwrap_or_default());
    }
    held.sort();
    held
}

/// The PTR record this FIXTURE puts in effect for an address, from the domain RFC 2606 sets
/// aside for documentation, so no row names a domain that belongs to anybody. It is fixture
/// data, not a value the service computes: what the tables assert is that the report carries
/// the record the STORE holds for the address.
fn fixture_ptr_record(allocation_id: &str) -> String {
    format!("{allocation_id}.example.com")
}

/// Seed the `domain-name` attribute of `allocation_id` as a record IN EFFECT, with no update
/// outstanding on it — the state an applied update leaves behind.
fn seed_attribute(engine: &engine::InMemoryEngine, allocation_id: &str) {
    engine.state::<Ec2State>().lock().address_ptr_records.insert(
        allocation_id.to_string(),
        AddressPtrRecord {
            ptr_record: Some(fixture_ptr_record(allocation_id)),
            pending_update: None,
        },
    );
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

/// The allocation ids a report answered, in the order it reported them.
fn reported_ids(attributes: &[Attribute]) -> Vec<String> {
    attributes
        .iter()
        .map(|attribute| attribute.allocation_id.clone().unwrap_or_default())
        .collect()
}

/// `AllocationId.N` form fields for `ids`.
fn narrowed_to(ids: &[&str]) -> Params {
    ids.iter()
        .enumerate()
        .map(|(index, id)| p(&format!("AllocationId.{}", index + 1), *id))
        .collect()
}

/// RULE 1: an address nothing has set the attribute on contributes no row, however many
/// such addresses the account holds and however the request narrows.
#[tokio::test]
async fn an_address_with_no_attribute_set_is_reported_no_row() {
    // Rows differing along how many addresses the account holds — none at all, one, and
    // several — because the report is of the attribute state, not of the addresses.
    for count in [0_usize, 1, 3] {
        let case = format!("{count} address(es), none with the attribute set");
        let (router, _engine) = emulator();
        let held = allocate(&router, &case, count).await;

        // Rows differing along how the request narrows: not at all, by the one member
        // `AddressAttributeName` has, by a single held id, and by every held id at once.
        let mut rows: Vec<(String, Params)> = vec![
            (format!("{case}, no ids at all"), Vec::new()),
            (
                format!("{case}, the attribute named"),
                vec![p("Attribute", ADDRESS_ATTRIBUTE_DOMAIN_NAME)],
            ),
        ];
        if let Some(first) = held.first() {
            rows.push((format!("{case}, one held id"), narrowed_to(&[first])));
            let every: Vec<&str> = held.iter().map(String::as_str).collect();
            rows.push((format!("{case}, every held id"), narrowed_to(&every)));
        }

        for (case, params) in rows {
            let (attributes, token) = described(&router, &case, &params).await;
            assert!(
                attributes.is_empty(),
                "case {case:?}: an address with no attribute set must be reported no row, \
                 got {attributes:?}"
            );
            assert_eq!(
                token, None,
                "case {case:?}: an empty report has no page to follow, got {token:?}"
            );
        }
    }
}

/// RULE 2 and RULE 5: exactly the addresses whose attribute is set are reported, with the
/// record in effect, in allocation-id order, and two identical calls answer the same thing.
#[tokio::test]
async fn only_the_addresses_whose_attribute_is_set_are_reported() {
    // Rows differing along WHICH of four held addresses have the attribute set: the first,
    // the last, two that are not adjacent in id order, and every one of them.
    let rows: [(&str, &[usize]); 4] = [
        ("the first of four", &[0]),
        ("the last of four", &[3]),
        ("two of four, not adjacent", &[0, 2]),
        ("all four", &[0, 1, 2, 3]),
    ];
    for (case, with_attribute) in rows {
        let (router, engine) = emulator();
        let held = allocate(&router, case, 4).await;
        for index in with_attribute {
            seed_attribute(&engine, &held[*index]);
        }
        let expected: Vec<String> = with_attribute
            .iter()
            .map(|index| held[*index].clone())
            .collect();

        let (attributes, token) = described(&router, case, &[]).await;
        assert_eq!(
            reported_ids(&attributes),
            expected,
            "case {case:?}: expected exactly the addresses whose attribute is set, \
             in id order, got {attributes:?}"
        );
        assert_eq!(
            token, None,
            "case {case:?}: four rows fit one page, so no token was expected, got {token:?}"
        );
        for attribute in &attributes {
            let allocation_id = attribute.allocation_id.clone().unwrap_or_default();
            assert_eq!(
                attribute.ptr_record,
                Some(fixture_ptr_record(&allocation_id)),
                "case {case:?}: the row must carry the record in effect, got {attribute:?}"
            );
            assert_eq!(
                attribute.update_status, None,
                "case {case:?}: no update is outstanding on the record, got {attribute:?}"
            );
            assert!(
                attribute.public_ip.is_some(),
                "case {case:?}: the row is the projection of the stored address, so it \
                 carries its public address, got {attribute:?}"
            );
        }

        // A report changes nothing a later caller sees: the same call answers the same.
        let (again, token_again) = described(&router, case, &[]).await;
        assert_eq!(
            (&again, &token_again),
            (&attributes, &token),
            "case {case:?}: two identical reports must answer the same thing"
        );
    }
}

/// RULE 2: an address with an update outstanding on its record is reported, and drops out of
/// the report once the clock has taken the attribute away — the state the report projects,
/// not the call that read it, decides the row.
#[tokio::test]
async fn an_address_drops_out_once_its_attribute_is_gone() {
    let (router, _engine) = emulator();
    let held = allocate(&router, "two addresses", 2).await;

    // `ResetAddressAttribute` takes the record away, which it leaves OUTSTANDING on the
    // address until the clock applies it — so the address has an attribute to report
    // meanwhile. A reset acts on the record the address carries, so one is set first, through
    // the other writer of this attribute, and given its own span.
    let (status, body) = call(
        &router,
        REGION,
        "ModifyAddressAttribute",
        &[
            p("AllocationId", held[0].as_str()),
            p("DomainName", "mail.example.com"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "the record must be set before it can be reset, got {status} with body {body}"
    );
    // Read until the record the modify started is in effect, rather than waiting a span of
    // this test's choosing: one record carries at most one outstanding request, so the reset
    // below needs the first one applied.
    let deadline = tokio::time::Instant::now() + attribute_deadline();
    loop {
        let (attributes, _) = described(&router, "while the record is being set", &[]).await;
        let outstanding = attributes
            .first()
            .and_then(|attribute| attribute.update_status.clone());
        if outstanding.is_none() {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the record set by the modify must come due, last read {attributes:?}"
        );
        tokio::time::sleep(poll_interval()).await;
    }
    let params = vec![
        p("AllocationId", held[0].as_str()),
        p("Attribute", ADDRESS_ATTRIBUTE_DOMAIN_NAME),
    ];
    let (status, body) = call(&router, REGION, "ResetAddressAttribute", &params).await;
    assert_eq!(
        status, 200,
        "the reset must be accepted, got {status} with body {body}"
    );

    let (attributes, _) = described(&router, "while the update is outstanding", &[]).await;
    assert_eq!(
        reported_ids(&attributes),
        vec![held[0].clone()],
        "while the update is outstanding the address is the one row reported, \
         got {attributes:?}"
    );
    assert_eq!(
        attributes[0].update_status.as_deref(),
        Some(PTR_UPDATE_STATUS_PENDING),
        "the row must carry the outstanding update, got {:?}",
        attributes[0]
    );

    // The update was owed at its instant whether or not anybody called, so once it has been
    // applied the address carries no record and no update — no attribute at all, and so no
    // row. Read until that holds rather than once after a span of this test's choosing.
    let deadline = tokio::time::Instant::now() + attribute_deadline();
    let (attributes, token) = loop {
        let (attributes, token) = described(&router, "once the update came due", &[]).await;
        if attributes.is_empty() {
            break (attributes, token);
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "once the update came due the address carries no attribute, so it must be \
             reported no row, last read {attributes:?}"
        );
        tokio::time::sleep(poll_interval()).await;
    };
    assert!(
        attributes.is_empty(),
        "once the update came due the address carries no attribute, so it must be reported \
         no row, got {attributes:?}"
    );
    assert_eq!(
        token, None,
        "an empty report has no page to follow, got {token:?}"
    );
}

/// RULE 3: `AllocationIds` filters the rows and asserts nothing about the account holding
/// the address, so an id that selects no row is answered an empty set rather than refused.
#[tokio::test]
async fn an_id_that_selects_no_row_is_answered_an_empty_set() {
    let (router, engine) = emulator();
    let held = allocate(&router, "three addresses", 3).await;
    seed_attribute(&engine, &held[1]);

    // An address allocated and released: a well-formed id of the right shape that names no
    // address of the account any more.
    let released = allocate(&router, "the address to release", 1).await;
    let released = released[0].clone();
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

    // Rows differing along WHAT the id the request narrows by names: a held address with no
    // attribute set, a well-formed id of each length that names no address at all, the id of
    // a released address, the address whose attribute is set, and a mix of one id that
    // selects a row and two that select none. Both id lengths are tested because an
    // allocation id is `eipalloc-` followed by 8 or 17 lowercase hexadecimal digits
    // (`service_com_amazonaws_ec2_amazonec2::state::allocation_id_is_well_formed`).
    let rows: [(&str, Params, Vec<String>); 6] = [
        (
            "a held address with no attribute set",
            narrowed_to(&[&held[0]]),
            Vec::new(),
        ),
        (
            "a well-formed short id nothing holds",
            narrowed_to(&["eipalloc-0a1b2c3d"]),
            Vec::new(),
        ),
        (
            "a well-formed long id nothing holds",
            narrowed_to(&["eipalloc-0a1b2c3d4e5f60718"]),
            Vec::new(),
        ),
        (
            "the id of a released address",
            narrowed_to(&[released.as_str()]),
            Vec::new(),
        ),
        (
            "the address whose attribute is set",
            narrowed_to(&[&held[1]]),
            vec![held[1].clone()],
        ),
        (
            "one id that selects a row beside two that select none",
            narrowed_to(&[&held[1], "eipalloc-0a1b2c3d4e5f60718", &held[2]]),
            vec![held[1].clone()],
        ),
    ];
    for (case, params, expected) in rows {
        let (attributes, token) = described(&router, case, &params).await;
        assert_eq!(
            reported_ids(&attributes),
            expected,
            "case {case:?}: expected the rows {expected:?}, got {attributes:?}"
        );
        assert_eq!(
            token, None,
            "case {case:?}: every selected row fits one page, got the token {token:?}"
        );
    }
}

/// RULE 4: a value that is not an allocation id at all is refused — the FORM of an id is not
/// a narrowing this report can be asked for.
#[tokio::test]
async fn a_value_that_is_not_an_allocation_id_is_refused() {
    let (router, engine) = emulator();
    let held = allocate(&router, "one address", 1).await;
    seed_attribute(&engine, &held[0]);

    // Rows differing along the FORM of the value: a word, another resource's id, the right
    // prefix at the wrong length, and the right length in the wrong alphabet.
    let rows: [(&str, &str); 4] = [
        ("not an id at all", "domain-name"),
        ("another resource's id", "vpc-0123456789abcdef0"),
        ("the right prefix at the wrong length", "eipalloc-0123"),
        ("the right length in the wrong alphabet", "eipalloc-0123456Z"),
    ];
    for (case, value) in rows {
        let (status, body) = call(
            &router,
            REGION,
            "DescribeAddressesAttribute",
            &narrowed_to(&[value]),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            INVALID_ALLOCATION_ID_MALFORMED,
            "case {case:?}: expected {INVALID_ALLOCATION_ID_MALFORMED}, got body {body}"
        );
        assert_eq!(
            error_message(&body),
            format!("Invalid id: \"{value}\""),
            "case {case:?}: expected the service's own wording, got body {body}"
        );
    }

    // The form of the id is the only thing that stopped the request: the same request
    // narrowing by a well-formed id is answered.
    let (attributes, _) = described(&router, "a well-formed id", &narrowed_to(&[&held[0]])).await;
    assert_eq!(
        reported_ids(&attributes),
        vec![held[0].clone()],
        "the same request naming a well-formed id must be answered, got {attributes:?}"
    );
}

/// RULE 5: the rows the report carries are paged in allocation-id order, a token naming
/// where the next page starts until the last one.
#[tokio::test]
async fn the_report_pages_over_the_rows_it_carries() {
    let (router, engine) = emulator();
    let held = allocate(&router, "four addresses", 4).await;
    // Three of the four have the attribute set, so the page walk is over the ROWS the report
    // carries rather than over the addresses the account holds.
    let with_attribute: Vec<String> = vec![held[0].clone(), held[1].clone(), held[3].clone()];
    for allocation_id in &with_attribute {
        seed_attribute(&engine, allocation_id);
    }

    // Rows differing along the page size: each page carries at most that many rows and a
    // token naming where the next one starts, until the last page, which carries no token.
    for page_size in [1_usize, 2, 3] {
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
                "case {case:?}: a page carries at most {page_size} rows, got {}",
                attributes.len()
            );
            seen.extend(reported_ids(&attributes));
            match token {
                None => break,
                Some(token) => next = Some(token),
            }
        }
        assert_eq!(
            seen, with_attribute,
            "case {case:?}: paging must report every row exactly once, in id order, \
             got {seen:?}"
        );
    }
}
