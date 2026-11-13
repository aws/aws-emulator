// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the SMS sandbox destination-number slice:
//! `CreateSMSSandboxPhoneNumber` adds a number, `ListSMSSandboxPhoneNumbers` reports
//! the numbers the sandbox holds, and `DeleteSMSSandboxPhoneNumber` removes one.
//!
//! The rule under test: a sandbox listing reports ONE entry per destination number
//! this account's sandbox currently holds — each with the verification status its
//! record carries — ordered oldest first and paged at `MaxResults` numbers, with a
//! `NextToken` returned only while numbers remain; the add and the remove are what
//! change that set, adding a number already there changing nothing, and removing one
//! the sandbox does not hold being a lookup miss (404 `ResourceNotFound`).
//!
//! The rows below differ in the phone-number FORM, the number COUNT, the page SIZE,
//! the verification STATUS and the token walked from — which is what the rule
//! generalises over. Every call goes over the wire through the router the emulator
//! serves in production, so nothing but persisted state carries information between
//! calls.

use std::time::{Duration, SystemTime};

use service_com_amazonaws_sns_amazonsimplenotificationservice::ops::types::S_M_S_SANDBOX_PHONE_NUMBER_VERIFICATION_STATUS_VALUES;
use service_com_amazonaws_sns_amazonsimplenotificationservice::state::{
    sms_origination, OriginationNumberRecord, SandboxPhoneNumberRecord, INVALID_NEXT_TOKEN_REFUSAL,
    MAX_RECORDS_PER_STORE, PAGE_SIZE, RESOURCE_DOES_NOT_EXIST, SANDBOX_NUMBER_PENDING,
};

use super::topics_subscriptions_publish::{text_of, Answer, Sns};

/// The page `MaxResults` bounds: `smithy_query(query="shape", shape="MaxItems")`
/// reports `range {min: 1, max: 100}` for the member's target shape.
const MIN_MAX_RESULTS: i64 = 1;
const MAX_MAX_RESULTS: i64 = 100;

/// The body of the response's `<PhoneNumbers>` element, or `None` when the response
/// carries no such member at all — the member is modelled `required`, so an answered
/// listing always has one, even for an empty sandbox.
fn phone_numbers_block(body: &str) -> Option<&str> {
    if let Some(start) = body.find("<PhoneNumbers>") {
        let rest = &body[start + "<PhoneNumbers>".len()..];
        let end = rest.find("</PhoneNumbers>")?;
        return Some(&rest[..end]);
    }
    body.contains("<PhoneNumbers/>").then_some("")
}

/// The `(PhoneNumber, Status)` pairs the listing carries, in document order.
///
/// A list member's open tag carries the envelope's default namespace, so the opening
/// is matched up to its first `>` rather than as a bare `<member>`.
///
/// `pub(crate)` so the sibling verification suite reads a number's status back
/// through the SAME listing parse rather than writing a second one.
pub(crate) fn reported_numbers(answer: &Answer) -> Vec<(String, String)> {
    let block = phone_numbers_block(&answer.body)
        .unwrap_or_else(|| panic!("no PhoneNumbers member in {}", answer.body));
    let mut found = Vec::new();
    let mut rest = block;
    while let Some(start) = rest.find("<member") {
        rest = &rest[start..];
        let Some(open) = rest.find('>') else { break };
        rest = &rest[open + 1..];
        let Some(end) = rest.find("</member>") else {
            break;
        };
        let member = &rest[..end];
        found.push((text_of(member, "PhoneNumber"), text_of(member, "Status")));
        rest = &rest[end + "</member>".len()..];
    }
    found
}

/// One listing call, with the optional paging parameters.
async fn list(sns: &Sns, params: &[(&str, &str)]) -> Answer {
    sns.call("ListSMSSandboxPhoneNumbers", params).await
}

/// Put one SMS-capable origination number in the account's inventory, the way the
/// out-of-band provisioning writes it.
///
/// The sandbox registration SENDS a one-time password, so it is answered only for an
/// account that holds something to send it from; every suite that registers a number
/// gives the account one through this seam (`pub(crate)` so the sibling verification
/// suite uses the same one), and the suites that assert the refusal leave the
/// inventory empty.
pub(crate) fn given_an_origination_number(sns: &Sns) {
    let state = sns.state();
    let mut state = state.lock();
    state.origination_numbers.insert(
        "+12065550199".to_string(),
        OriginationNumberRecord {
            phone_number: "+12065550199".to_string(),
            status: "Active".to_string(),
            iso2_country_code: "US".to_string(),
            route_type: "Transactional".to_string(),
            number_capabilities: vec![sms_origination::SMS_CAPABILITY.to_string()],
            created_at: SystemTime::UNIX_EPOCH + Duration::from_secs(1),
        },
    );
}

/// Add `number` to the sandbox, asserting the call was served.
///
/// `pub(crate)` so the sibling verification suite adds a number the same way.
pub(crate) async fn add(sns: &Sns, case: &str, number: &str) -> Answer {
    given_an_origination_number(sns);
    let answer = sns
        .call("CreateSMSSandboxPhoneNumber", &[("PhoneNumber", number)])
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected CreateSMSSandboxPhoneNumber to answer 200, got {} / {}",
        answer.status, answer.body
    );
    answer
}

/// The destination-number forms the SMS sandbox accepts: `smithy_query(query="shape",
/// shape="PhoneNumberString")` reports `length {min: 0, max: 20}` and pattern
/// `^(\+[0-9]{8,}|[0-9]{0,9})$`, and the member is "The destination phone number to
/// verify … that you can send SMS messages to"
/// (`smithy_query(query="doc",shape="CreateSMSSandboxPhoneNumberInput",
/// member="PhoneNumber")`) — a DESTINATION number, which is one in E.164 format. The forms
/// that shared pattern admits and E.164 does not are refused
/// ([`refused_number_forms`]). The rows cover both bounds of the E.164 digit count (the
/// eight the pattern's own branch publishes, and the fifteen ITU-T E.164 caps a number at)
/// and two country codes.
///
/// `pub(crate)` so the sibling verification suite drives the same accepted forms
/// rather than keeping a second catalog of them.
pub(crate) fn accepted_number_forms() -> Vec<(&'static str, String)> {
    vec![
        ("e164 north american", "+12065550100".to_string()),
        ("e164 international", "+447700900123".to_string()),
        ("e164 at the eight digit minimum", "+12345678".to_string()),
        (
            "e164 at the fifteen digit maximum",
            format!("+{}", "1".repeat(15)),
        ),
    ]
}

/// The destination-number forms the sandbox REFUSES although the modelled pattern admits
/// them: a bare national or short-code form (no `+`, so no country code), an E.164 form
/// whose country code begins with zero — which E.164 assigns to no country — and one digit
/// past the fifteen E.164 caps a number at.
pub(crate) fn refused_number_forms() -> Vec<(&'static str, String)> {
    vec![
        ("a bare short code", "12345".to_string()),
        ("bare digits at the nine digit bound", "123456789".to_string()),
        ("no digits at all", String::new()),
        ("a country code beginning with zero", "+0123456789".to_string()),
        (
            "one digit past the fifteen digit maximum",
            format!("+{}", "1".repeat(16)),
        ),
    ]
}

/// A number added to the sandbox is reported as a PENDING destination number, a
/// repeated add changes nothing, and a delete takes it out again — for every
/// phone-number form the model accepts.
#[tokio::test]
async fn an_added_number_is_listed_as_pending_until_it_is_deleted() {
    let sns = Sns::new();
    let forms = accepted_number_forms();

    let empty = list(&sns, &[]).await;
    assert_eq!(
        empty.status, 200,
        "an empty sandbox: expected 200, got {} / {}",
        empty.status, empty.body
    );
    assert!(
        reported_numbers(&empty).is_empty(),
        "an empty sandbox: expected no numbers, got {:?}",
        reported_numbers(&empty)
    );
    assert!(
        empty.element("NextToken").is_none(),
        "an empty sandbox: expected no NextToken, got {}",
        empty.body
    );

    for (at, (case, number)) in forms.iter().enumerate() {
        add(&sns, case, number).await;
        // Adding a number the sandbox already holds asks for what it already is.
        add(&sns, case, number).await;

        let answer = list(&sns, &[]).await;
        let reported = reported_numbers(&answer);
        assert_eq!(
            reported.len(),
            at + 1,
            "case {case:?}: expected {} numbers after the add, got {reported:?}",
            at + 1
        );
        let held: Vec<&(String, String)> = reported
            .iter()
            .filter(|(listed, _)| listed == number)
            .collect();
        assert_eq!(
            held.len(),
            1,
            "case {case:?}: expected exactly one entry for the number, got {held:?}"
        );
        assert_eq!(
            held[0].1, SANDBOX_NUMBER_PENDING,
            "case {case:?}: expected the status {SANDBOX_NUMBER_PENDING:?}, got {:?}",
            held[0].1
        );
    }

    for (case, number) in &forms {
        let deleted = sns
            .call("DeleteSMSSandboxPhoneNumber", &[("PhoneNumber", number)])
            .await;
        assert_eq!(
            deleted.status, 200,
            "case {case:?}: expected the delete to answer 200, got {} / {}",
            deleted.status, deleted.body
        );
        let reported = reported_numbers(&list(&sns, &[]).await);
        assert!(
            !reported.iter().any(|(listed, _)| listed == number),
            "case {case:?}: expected the number to be gone, got {reported:?}"
        );

        // The same delete again: the sandbox no longer holds the number, so there is
        // no resource to act on.
        let again = sns
            .call("DeleteSMSSandboxPhoneNumber", &[("PhoneNumber", number)])
            .await;
        assert_eq!(
            again.status, 404,
            "case {case:?}: expected a repeated delete to answer 404, got {} / {}",
            again.status, again.body
        );
        assert_eq!(
            again.code(),
            "ResourceNotFound",
            "case {case:?}: expected the ResourceNotFound code, got {}",
            again.body
        );
        assert_eq!(
            again.element("Message").as_deref(),
            Some(RESOURCE_DOES_NOT_EXIST),
            "case {case:?}: expected the fixed missing-resource sentence, got {}",
            again.body
        );
        assert!(
            !again.body.contains(number.as_str()),
            "case {case:?}: the refusal must not echo the sensitive number, got {}",
            again.body
        );
    }

    assert!(
        reported_numbers(&list(&sns, &[]).await).is_empty(),
        "every number was deleted: expected an empty sandbox, got {:?}",
        reported_numbers(&list(&sns, &[]).await)
    );
}

/// Deleting a number this account's sandbox never held is a lookup miss, whatever
/// form the number takes.
#[tokio::test]
async fn deleting_a_number_the_sandbox_does_not_hold_is_a_lookup_miss() {
    let sns = Sns::new();
    add(&sns, "the one number the sandbox holds", "+12065550100").await;

    for (case, number) in accepted_number_forms() {
        if number == "+12065550100" {
            continue;
        }
        let answer = sns
            .call("DeleteSMSSandboxPhoneNumber", &[("PhoneNumber", &number)])
            .await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected 404, got {} / {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.code(),
            "ResourceNotFound",
            "case {case:?}: expected the ResourceNotFound code, got {}",
            answer.body
        );
    }

    let reported = reported_numbers(&list(&sns, &[]).await);
    assert_eq!(
        reported.len(),
        1,
        "the refused deletes must leave the sandbox alone, got {reported:?}"
    );
}

/// `MaxResults` is the page size and `NextToken` walks the pages: whatever the size,
/// the pages concatenate to every number the sandbox holds, oldest first, each number
/// once, and only the last page carries no token.
#[tokio::test]
async fn the_listing_pages_by_max_results_and_the_token_walks_every_number_once() {
    let sns = Sns::new();
    let held: Vec<String> = (0..12).map(|at| format!("+1206555{at:04}")).collect();
    for number in &held {
        add(&sns, "a paged number", number).await;
    }

    // Page sizes spanning both published bounds of `MaxResults`, a size that divides
    // the number count, one that does not, and one larger than the whole set.
    let sizes: &[i64] = &[MIN_MAX_RESULTS, 2, 5, 12, MAX_MAX_RESULTS];
    for size in sizes {
        let case = format!("MaxResults={size}");
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let size_param = size.to_string();
            let mut params: Vec<(&str, &str)> = vec![("MaxResults", size_param.as_str())];
            if let Some(token) = token.as_deref() {
                params.push(("NextToken", token));
            }
            let answer = list(&sns, &params).await;
            assert_eq!(
                answer.status, 200,
                "case {case:?} page {pages}: expected 200, got {} / {}",
                answer.status, answer.body
            );
            let page = reported_numbers(&answer);
            assert!(
                page.len() <= usize::try_from(*size).expect("the page size is positive"),
                "case {case:?} page {pages}: expected at most {size} numbers, got {page:?}"
            );
            assert!(
                !page.is_empty(),
                "case {case:?} page {pages}: a page a token led to must not be empty"
            );
            walked.extend(page.into_iter().map(|(number, _)| number));
            pages += 1;
            token = answer.element("NextToken");
            if token.is_none() {
                break;
            }
            assert!(
                walked.len() < held.len(),
                "case {case:?}: a NextToken was returned with every number already \
                 reported, got {walked:?}"
            );
        }
        assert_eq!(
            walked, held,
            "case {case:?}: expected every number once, oldest first, got {walked:?}"
        );
        let expected_pages = held
            .len()
            .div_ceil(usize::try_from(*size).expect("the page size is positive"));
        assert_eq!(
            pages, expected_pages,
            "case {case:?}: expected {expected_pages} pages, got {pages}"
        );
    }

    // A listing that names no page size reports the page every paginated SNS list
    // reports, which is larger than this set — so one page and no token.
    let whole = list(&sns, &[]).await;
    assert!(
        held.len() < PAGE_SIZE,
        "this suite's number count must sit inside one default page"
    );
    assert_eq!(
        reported_numbers(&whole)
            .into_iter()
            .map(|(number, _)| number)
            .collect::<Vec<String>>(),
        held,
        "no MaxResults: expected the whole sandbox in one page, got {}",
        whole.body
    );
    assert!(
        whole.element("NextToken").is_none(),
        "no MaxResults: expected no NextToken, got {}",
        whole.body
    );
}

/// A `MaxResults` outside the modelled range is refused before the listing is served,
/// at both published bounds plus one past each.
#[tokio::test]
async fn a_max_results_outside_the_modelled_range_is_refused() {
    let sns = Sns::new();
    add(&sns, "one held number", "+12065550100").await;

    let cases: &[(&str, i64, bool)] = &[
        ("one below the published minimum", MIN_MAX_RESULTS - 1, false),
        ("at the published minimum", MIN_MAX_RESULTS, true),
        ("inside the range", 50, true),
        ("at the published maximum", MAX_MAX_RESULTS, true),
        ("one past the published maximum", MAX_MAX_RESULTS + 1, false),
        ("far past the published maximum", 1_000, false),
        ("negative", -1, false),
    ];

    for (case, max_results, accepted) in cases {
        let value = max_results.to_string();
        let answer = list(&sns, &[("MaxResults", value.as_str())]).await;
        if *accepted {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected 200, got {} / {}",
                answer.status, answer.body
            );
            assert_eq!(
                reported_numbers(&answer).len(),
                1,
                "case {case:?}: expected the one held number, got {}",
                answer.body
            );
        } else {
            assert_eq!(
                answer.status, 400,
                "case {case:?}: expected 400, got {} / {}",
                answer.status, answer.body
            );
            assert!(
                phone_numbers_block(&answer.body).is_none(),
                "case {case:?}: a refused request must serve no listing, got {}",
                answer.body
            );
        }
    }
}

/// A `NextToken` the listing did not mint is refused as bad input, and the sandbox is
/// left alone.
#[tokio::test]
async fn a_next_token_the_listing_did_not_mint_is_refused() {
    let sns = Sns::new();
    for number in ["+12065550100", "+12065550101", "+12065550102"] {
        add(&sns, "a held number", number).await;
    }

    let cases: &[(&str, &str)] = &[
        ("not a token at all", "bogus"),
        ("past the end of the set", "9999"),
        ("negative", "-1"),
        ("fractional", "1.5"),
        ("empty-ish whitespace", " "),
    ];
    for (case, token) in cases {
        let answer = list(&sns, &[("NextToken", token)]).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected 400, got {} / {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.code(),
            "InvalidParameter",
            "case {case:?}: expected the InvalidParameter code, got {}",
            answer.body
        );
        // The sentence is per error class, not per listing: every paginated SNS list
        // refuses a token it did not mint with the same one.
        assert_eq!(
            answer.element("Message").as_deref(),
            Some(INVALID_NEXT_TOKEN_REFUSAL),
            "case {case:?}: expected the member's refusal sentence, got {}",
            answer.body
        );
    }

    assert_eq!(
        reported_numbers(&list(&sns, &[]).await).len(),
        3,
        "the refused listings must leave the sandbox alone"
    );
}

/// The sandbox store is BOUNDED: a sandbox already holding
/// [`MAX_RECORDS_PER_STORE`] numbers refuses another one with a modelled user error
/// rather than growing without limit, while a number it already holds is still
/// answered.
#[tokio::test]
async fn a_number_past_the_record_ceiling_is_refused() {
    let sns = Sns::new();
    given_an_origination_number(&sns);
    let filled: Vec<String> = (0..MAX_RECORDS_PER_STORE)
        .map(|at| format!("+1206{at:07}"))
        .collect();
    {
        let state = sns.state();
        let mut state = state.lock();
        for (at, number) in filled.iter().enumerate() {
            state.sandbox_phone_numbers.insert(
                number.clone(),
                SandboxPhoneNumberRecord {
                    phone_number: number.clone(),
                    status: SANDBOX_NUMBER_PENDING.to_string(),
                    one_time_password: format!("{at:06}"),
                    created_at: SystemTime::UNIX_EPOCH + Duration::from_secs(at as u64 + 1),
                },
            );
        }
    }

    let held = sns
        .call(
            "CreateSMSSandboxPhoneNumber",
            &[("PhoneNumber", filled[0].as_str())],
        )
        .await;
    assert_eq!(
        held.status, 200,
        "a number the full sandbox already holds: expected 200, got {} / {}",
        held.status, held.body
    );

    let refused = sns
        .call("CreateSMSSandboxPhoneNumber", &[("PhoneNumber", "+447700900123")])
        .await;
    assert_eq!(
        refused.status, 400,
        "one number past the ceiling: expected 400, got {} / {}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.code(),
        "UserError",
        "one number past the ceiling: expected the UserError code, got {}",
        refused.body
    );
    assert_eq!(
        sns.state().lock().sandbox_phone_numbers.len(),
        MAX_RECORDS_PER_STORE,
        "the refused add must not grow the store"
    );
}

/// The listing reports the verification status each record HOLDS — every value the
/// modelled `SMSSandboxPhoneNumberVerificationStatus` enum accepts — and reading it
/// twice reports the same thing, because a read changes nothing.
#[tokio::test]
async fn the_listing_reports_the_status_each_record_holds() {
    let sns = Sns::new();

    // One record per modelled status value, written the way the operation that moves
    // a number's status writes it. The listing is a report, so the status it carries
    // is the record's, whichever value that is.
    let seeded: Vec<(String, &str)> = S_M_S_SANDBOX_PHONE_NUMBER_VERIFICATION_STATUS_VALUES
        .iter()
        .enumerate()
        .map(|(at, status)| (format!("+1206555{at:04}"), *status))
        .collect();
    {
        let state = sns.state();
        let mut state = state.lock();
        for (at, (number, status)) in seeded.iter().enumerate() {
            state.sandbox_phone_numbers.insert(
                number.clone(),
                SandboxPhoneNumberRecord {
                    phone_number: number.clone(),
                    status: (*status).to_string(),
                    one_time_password: format!("{at:06}"),
                    created_at: SystemTime::UNIX_EPOCH
                        + Duration::from_secs(at as u64 + 1),
                },
            );
        }
    }

    let first = list(&sns, &[]).await;
    let expected: Vec<(String, String)> = seeded
        .iter()
        .map(|(number, status)| (number.clone(), (*status).to_string()))
        .collect();
    assert_eq!(
        reported_numbers(&first),
        expected,
        "expected every seeded number with the status its record holds, got {}",
        first.body
    );

    let second = list(&sns, &[]).await;
    assert_eq!(
        reported_numbers(&second),
        expected,
        "a second read must report the same numbers and statuses, got {}",
        second.body
    );
    assert_eq!(
        sns.state().lock().sandbox_phone_numbers.len(),
        seeded.len(),
        "a listing must not add or drop a record"
    );
}
