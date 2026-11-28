// SPDX-License-Identifier: CC0-1.0

//! `ListPaymentInstruments` and `GetPaymentInstrument` end to end: what the account
//! holds, how it pages, and how one instrument is identified.

use serde_json::{json, Value};

use crate::payments_harness::{error_code, ids, list, text, Emulator};

/// Both operations of the payment-instrument half of the service.
const LIST: &str = "ListPaymentInstruments";
const GET: &str = "GetPaymentInstrument";

/// Page through the whole listing at `page_size`, reporting the IDs in the order
/// they were returned.
///
/// Asserts the page contract as it goes: no page is larger than asked for, every
/// page before the last is full, and the listing terminates.
async fn paged_ids(emulator: &Emulator, page_size: usize) -> Vec<String> {
    let held = emulator.held_instrument_ids().len();
    let mut seen = Vec::new();
    let mut next_token: Option<String> = None;
    let mut pages = 0usize;
    loop {
        let mut body = json!({ "maxResults": page_size });
        if let Some(token) = &next_token {
            body["nextToken"] = json!(token);
        }
        let payload = emulator.ok(LIST, &body).await;
        let page = list(&payload, "paymentInstrumentSummaries");
        assert!(
            page.len() <= page_size,
            "case page size {page_size}: a page carried {} summaries, more than maxResults",
            page.len()
        );
        seen.extend(page.iter().map(|summary| text(summary, "id")));
        pages += 1;
        assert!(
            pages <= held + 1,
            "case page size {page_size}: paging did not terminate after {pages} pages"
        );
        next_token = payload
            .get("nextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        match next_token {
            None => break,
            Some(_) => assert_eq!(
                page.len(),
                page_size,
                "case page size {page_size}: a page that hands back a nextToken must be full, got {}",
                page.len()
            ),
        }
    }
    seen
}

#[tokio::test]
async fn paging_returns_every_instrument_exactly_once_at_every_page_size() {
    let emulator = Emulator::new();
    let held = emulator.held_instrument_ids();
    assert!(
        held.len() >= 3,
        "case \"account inventory\": the account must hold enough instruments to page, got {held:?}"
    );
    // The modelled `maxResults` range is 1..25: both bounds and the sizes around
    // the inventory's own length.
    for page_size in [1usize, 2, 3, held.len(), held.len() + 1, 25] {
        let seen = paged_ids(&emulator, page_size).await;
        assert_eq!(
            seen, held,
            "case page size {page_size}: expected every held instrument once in insertion order {held:?}, got {seen:?}"
        );
    }
}

#[tokio::test]
async fn a_request_naming_no_page_size_is_served_at_the_modelled_default() {
    let emulator = Emulator::new();
    let held = emulator.held_instrument_ids();
    let payload = emulator.ok(LIST, &json!({})).await;
    assert_eq!(
        ids(&payload, "paymentInstrumentSummaries"),
        held,
        "case \"no maxResults\": the default page size (25) must carry the whole inventory, got {payload}"
    );
    assert!(
        payload.get("nextToken").is_none(),
        "case \"no maxResults\": a listing with nothing left must carry no nextToken, got {payload}"
    );
}

#[tokio::test]
async fn a_page_size_outside_the_modelled_range_is_refused() {
    let emulator = Emulator::new();
    for page_size in [0, 26, 1_000] {
        let (status, payload) = emulator.call(LIST, &json!({ "maxResults": page_size })).await;
        assert_eq!(
            status, 400,
            "case maxResults {page_size}: expected 400 for a page size outside 1..25, got {status} with {payload}"
        );
    }
}

#[tokio::test]
async fn both_forms_of_the_identifier_resolve_to_one_instrument() {
    let emulator = Emulator::new();
    let listing = emulator.ok(LIST, &json!({})).await;
    let summaries = list(&listing, "paymentInstrumentSummaries");
    assert!(
        !summaries.is_empty(),
        "case \"account inventory\": the account must hold an instrument to get"
    );
    for summary in summaries {
        let id = text(summary, "id");
        let arn = text(summary, "arn");
        assert!(
            arn.ends_with(&format!(":payment-instrument:{id}")),
            "case {id}: the ARN must end in the instrument's ID, got {arn}"
        );
        let by_id = emulator.ok(GET, &json!({ "identifier": id })).await;
        let by_arn = emulator.ok(GET, &json!({ "identifier": arn })).await;
        assert_eq!(
            by_id, by_arn,
            "case {id}: the ID and the ARN must name the same instrument, got {by_id} and {by_arn}"
        );
        let instrument = by_id
            .get("paymentInstrument")
            .cloned()
            .unwrap_or(Value::Null);
        for member in ["arn", "paymentMethod", "status", "createdAt", "updatedAt"] {
            assert_eq!(
                instrument.get(member),
                summary.get(member),
                "case {id}: the summary and the instrument must agree on {member}, got {instrument} and {summary}"
            );
        }
    }
}

#[tokio::test]
async fn the_full_instrument_carries_the_method_specific_details_the_summary_omits() {
    let emulator = Emulator::new();
    let listing = emulator.ok(LIST, &json!({})).await;
    let mut methods_with_details = 0usize;
    for summary in list(&listing, "paymentInstrumentSummaries") {
        let id = text(summary, "id");
        let method = text(summary, "paymentMethod");
        assert!(
            summary.get("details").is_none(),
            "case {id}: a summary must carry no details, got {summary}"
        );
        let payload = emulator.ok(GET, &json!({ "identifier": id.clone() })).await;
        let instrument = payload
            .get("paymentInstrument")
            .cloned()
            .unwrap_or(Value::Null);
        // `PaymentInstrument$details` "is absent for a payment method that has no
        // additional details", and PAY_BY_INVOICE is such a method: the caller pays
        // each invoice itself, so there is no card or account to describe.
        match method.as_str() {
            "PAY_BY_INVOICE" => assert!(
                instrument.get("details").is_none(),
                "case {id}: {method} has no details of its own, got {instrument}"
            ),
            _ => {
                let details = instrument
                    .get("details")
                    .and_then(Value::as_object)
                    .unwrap_or_else(|| {
                        panic!("case {id}: {method} must carry its details, got {instrument}")
                    });
                assert_eq!(
                    details.len(),
                    1,
                    "case {id}: exactly one member of the details union is populated, got {instrument}"
                );
                methods_with_details += 1;
            }
        }
    }
    assert!(
        methods_with_details >= 2,
        "case \"account inventory\": the account must hold instruments of more than one detailed payment method, got {methods_with_details}"
    );
}

#[tokio::test]
async fn an_identifier_no_instrument_on_the_account_matches_is_not_found() {
    let emulator = Emulator::new();
    let held = emulator.held_instrument_ids();
    let known = held.first().cloned().unwrap_or_default();
    // Every case satisfies the member's pattern, so the framework passes it to the
    // handler: a bare ID, an ARN for an ID the account does not hold, an ARN for
    // another account, an ARN in another partition, and an ID that is only a prefix
    // of one the account holds.
    let cases = [
        "no-such-instrument".to_string(),
        format!("arn:aws:payments::000000000000:payment-instrument:{known}x"),
        format!("arn:aws:payments::123456789012:payment-instrument:{known}"),
        format!("arn:aws-cn:payments::000000000000:payment-instrument:{known}"),
        known.chars().take(4).collect::<String>(),
    ];
    for identifier in cases {
        let (status, payload) = emulator
            .call(GET, &json!({ "identifier": identifier.clone() }))
            .await;
        assert_eq!(
            status, 404,
            "case {identifier}: expected 404 for an identifier the account does not hold, got {status} with {payload}"
        );
        assert_eq!(
            error_code(&payload),
            "ResourceNotFoundException",
            "case {identifier}: expected the modelled not-found error, got {payload}"
        );
        assert_eq!(
            text(&payload, "resourceId"),
            identifier,
            "case {identifier}: the error names the resource that wasn't found, got {payload}"
        );
        assert_eq!(
            text(&payload, "resourceType"),
            "payment-instrument",
            "case {identifier}: the error names the type of the resource, got {payload}"
        );
    }
}

#[tokio::test]
async fn a_next_token_this_listing_cannot_resume_is_refused() {
    let emulator = Emulator::new();
    let mine = emulator
        .ok(LIST, &json!({ "maxResults": 1 }))
        .await
        .get("nextToken")
        .and_then(Value::as_str)
        .map(str::to_string)
        .expect("a listing with more instruments left hands back a nextToken");
    // A token minted by ANOTHER paginated operation of the same service: it reads
    // as a token — it is authenticated under the same store's key — and it does not
    // resume this listing, because the cursor names the query it was minted for.
    let another_listing = emulator
        .ok(
            "GetAccountBalance",
            &json!({ "balanceType": "INVOICES_DUE", "maxResults": 1 }),
        )
        .await
        .get("nextToken")
        .and_then(Value::as_str)
        .map(str::to_string)
        .expect("a balance with more amounts left hands back a nextToken");
    let cases = [
        ("too short", "0".to_string()),
        ("not base64", "z".repeat(40)),
        ("longer than a token", format!("{mine}00000000")),
        ("minted for another listing", another_listing),
        (
            "a resume position with no authenticator",
            aws_smithy_types::base64::encode(
                [[0_u8; 32].as_slice(), b"\x1f1".as_slice()].concat(),
            ),
        ),
        (
            "a cursor a caller assembled, behind a forged authenticator",
            aws_smithy_types::base64::encode(
                [
                    [0_u8; 32].as_slice(),
                    "0".repeat(64).as_bytes(),
                    b"\x1f1".as_slice(),
                ]
                .concat(),
            ),
        ),
    ];
    for (case, token) in cases {
        let (status, payload) = emulator
            .call(LIST, &json!({ "maxResults": 1, "nextToken": token }))
            .await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400 for a token this listing cannot resume, got {status} with {payload}"
        );
        assert_eq!(
            error_code(&payload),
            "ValidationException",
            "case {case}: expected the modelled validation error, got {payload}"
        );
        assert_eq!(
            text(&payload, "reason"),
            "INVALID_PAGINATION_TOKEN",
            "case {case}: the model gives this refusal its own reason, got {payload}"
        );
    }
    // The token this listing DID mint resumes it.
    let resumed = emulator
        .ok(LIST, &json!({ "maxResults": 1, "nextToken": mine }))
        .await;
    assert_eq!(
        ids(&resumed, "paymentInstrumentSummaries"),
        emulator.held_instrument_ids()[1..2].to_vec(),
        "case \"own token\": the next page continues where the previous one stopped, got {resumed}"
    );
}

#[tokio::test]
async fn an_account_with_no_instrument_lists_nothing_and_gets_nothing() {
    let emulator = Emulator::empty();
    let payload = emulator.ok(LIST, &json!({})).await;
    assert_eq!(
        list(&payload, "paymentInstrumentSummaries").len(),
        0,
        "case \"empty account\": the list is empty when the account has no instrument, got {payload}"
    );
    assert!(
        payload.get("nextToken").is_none(),
        "case \"empty account\": an empty listing carries no nextToken, got {payload}"
    );
    let (status, payload) = emulator
        .call(GET, &json!({ "identifier": "pi-0a1b2c3d4e5f" }))
        .await;
    assert_eq!(
        status, 404,
        "case \"empty account\": an instrument the account no longer holds is not found, got {status} with {payload}"
    );
}
