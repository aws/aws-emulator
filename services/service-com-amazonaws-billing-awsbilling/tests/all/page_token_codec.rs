// SPDX-License-Identifier: CC0-1.0

//! The crate's ONE page-token codec, driven through the paginated reads that
//! share it.
//!
//! Every response byte these reads answer with is unchanged by the fold onto
//! `state::encode_page_token` / `state::decode_page_token`: what this suite
//! pins is that the reads go through that single pair, so one minting rule and
//! one acceptance rule decide every `nextToken` this service issues and takes
//! back. Each read adds only its own payload on top — a position in a list for
//! `ListSourceViewsForBillingView`, the ARN a page ended at for
//! `ListBillingViews`.
//!
//! The rows differ along what the codec generalises over: the SPELLING of the
//! token bytes a client sends (the hex the encoder mints, the same bytes with
//! their hex digits in upper case, one digit short of a whole byte, characters
//! outside the hex alphabet, hex of bytes that are not UTF-8). Every row is
//! driven through both reads, and an accepted one must resume the walk while a
//! rejected one must draw the very same refusal from either read.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The awsJson1_0 harness — a router over ONE shared engine, plus `call` and
// `error_code` — already exists for the billing-view lifecycle suite, so this
// module reuses it instead of building a second one.
use crate::billing_views::{call, emulator, error_code, ACCOUNT_ID};

/// A pattern-valid `BillingViewArn` for a source view with resource id `id`.
fn source_arn(id: &str) -> String {
    format!("arn:aws:billing::{ACCOUNT_ID}:billingview/{id}")
}

/// How a client spells the token bytes, and whether the crate's one codec
/// accepts that spelling.
struct Spelling {
    case: &'static str,
    /// The token a client sends, given the cursor payload the read's own page
    /// boundary is.
    spell: fn(&str) -> String,
    accepted: bool,
}

/// Exactly what the shared encoder mints for the payload.
fn minted(payload: &str) -> String {
    hex::encode(payload)
}

/// The same bytes, with every alphabetic hex digit in upper case — which the
/// shared decoder reads back to the same payload. A decimal position's hex has
/// no alphabetic digit, so for that read this row coincides with [`minted`];
/// an ARN cursor's hex has several.
fn upper_case_digits(payload: &str) -> String {
    hex::encode(payload).to_uppercase()
}

/// One hex digit short of a whole byte, so the bytes cannot be recovered.
fn half_a_byte(payload: &str) -> String {
    let mut token = hex::encode(payload);
    token.pop();
    token
}

/// Characters the hex alphabet does not hold (inside the modeled `PageToken`
/// pattern `^[-a-zA-Z0-9+=/_]+$`, so the boundary validator passes it through).
fn outside_the_hex_alphabet(_payload: &str) -> String {
    "zzzz".to_string()
}

/// Well-formed hex of a byte sequence that is not UTF-8, so the transcoding
/// fails on its second half rather than its first.
fn not_utf8(_payload: &str) -> String {
    "ff".to_string()
}

fn spellings() -> Vec<Spelling> {
    vec![
        Spelling {
            case: "the hex the encoder mints",
            spell: minted,
            accepted: true,
        },
        Spelling {
            case: "the same bytes with upper-case hex digits",
            spell: upper_case_digits,
            accepted: true,
        },
        Spelling {
            case: "one hex digit short of a whole byte",
            spell: half_a_byte,
            accepted: false,
        },
        Spelling {
            case: "characters outside the hex alphabet",
            spell: outside_the_hex_alphabet,
            accepted: false,
        },
        Spelling {
            case: "hex of bytes that are not UTF-8",
            spell: not_utf8,
            accepted: false,
        },
    ]
}

/// One paginated read of this service: the request fields that select its walk,
/// the payload its own first page of one entry ends at, and the entries a page
/// resuming after that payload reports.
struct Walk {
    operation: &'static str,
    base: Value,
    payload: String,
    expected_after: Vec<String>,
}

/// The entries a page reports, whichever read answered. A protocol that omits
/// an empty list and one that writes `[]` mean the same thing, so both read
/// back as no entries.
fn entries(operation: &str, body: &Value) -> Vec<String> {
    match operation {
        "ListSourceViewsForBillingView" => body
            .get("sourceViews")
            .and_then(Value::as_array)
            .map(|listed| {
                listed
                    .iter()
                    .map(|entry| entry.as_str().unwrap_or_default().to_string())
                    .collect()
            })
            .unwrap_or_default(),
        _ => body
            .get("billingViews")
            .and_then(Value::as_array)
            .map(|listed| {
                listed
                    .iter()
                    .map(|view| view["arn"].as_str().unwrap_or_default().to_string())
                    .collect()
            })
            .unwrap_or_default(),
    }
}

/// The world both reads page over: a `CUSTOM` view holding three source views,
/// beside the account's `PRIMARY` view.
async fn walks(router: &MultiServiceRouter) -> Vec<Walk> {
    let sources = vec![
        source_arn("codec-src-a"),
        source_arn("codec-src-b"),
        source_arn("codec-src-c"),
    ];
    let (status, created) = call(
        router,
        "CreateBillingView",
        &json!({ "name": "page-token-codec", "sourceViews": sources }),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: expected CreateBillingView to answer 200, got {status} {created:?}"
    );
    let view_arn = created["arn"]
        .as_str()
        .unwrap_or_else(|| panic!("setup: CreateBillingView must answer an arn, got {created:?}"))
        .to_string();

    // `ListBillingViews` walks the views in ARN order and its page boundary is
    // the ARN the page ended at, so the unpaginated answer says both what the
    // first page's cursor is and what a page resuming after it must report.
    let (status, all) = call(router, "ListBillingViews", &json!({})).await;
    assert_eq!(
        status, 200,
        "setup: expected ListBillingViews to answer 200, got {status} {all:?}"
    );
    let every_view = entries("ListBillingViews", &all);
    assert!(
        every_view.len() >= 2,
        "setup: expected the created view beside the PRIMARY one, got {every_view:?}"
    );

    vec![
        Walk {
            operation: "ListSourceViewsForBillingView",
            base: json!({ "arn": view_arn }),
            // The positional payload a page of one entry ends at.
            payload: "1".to_string(),
            expected_after: sources[1..].to_vec(),
        },
        Walk {
            operation: "ListBillingViews",
            base: json!({}),
            payload: every_view[0].clone(),
            expected_after: every_view[1..].to_vec(),
        },
    ]
}

#[tokio::test]
async fn every_paginated_read_mints_its_token_with_the_shared_encoder() {
    let router = emulator();

    for walk in walks(&router).await {
        let case = walk.operation;
        let mut request = walk.base.clone();
        request["maxResults"] = json!(1);
        let (status, page) = call(&router, walk.operation, &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 for a first page of one entry, got {status} {page:?}"
        );
        assert_eq!(
            page["nextToken"].as_str(),
            Some(minted(&walk.payload).as_str()),
            "case {case:?}: expected the shared encoder's token for payload {:?}, got {page:?}",
            walk.payload
        );
    }
}

#[tokio::test]
async fn a_token_spelling_the_shared_codec_accepts_resumes_every_paginated_read() {
    let router = emulator();

    for walk in walks(&router).await {
        for spelling in spellings().into_iter().filter(|spelling| spelling.accepted) {
            let case = format!("{} / {}", walk.operation, spelling.case);
            let mut request = walk.base.clone();
            request["nextToken"] = json!((spelling.spell)(&walk.payload));
            let (status, page) = call(&router, walk.operation, &request).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200, got {status} {page:?}"
            );
            assert_eq!(
                entries(walk.operation, &page),
                walk.expected_after,
                "case {case:?}: expected the entries after {:?}, got {page:?}",
                walk.payload
            );
        }
    }
}

#[tokio::test]
async fn a_token_spelling_the_shared_codec_rejects_is_refused_the_same_way_by_every_read() {
    let router = emulator();
    let walks = walks(&router).await;

    for spelling in spellings()
        .into_iter()
        .filter(|spelling| !spelling.accepted)
    {
        let mut refusals: Vec<(String, Value)> = Vec::new();
        for walk in &walks {
            let case = format!("{} / {}", walk.operation, spelling.case);
            let mut request = walk.base.clone();
            request["nextToken"] = json!((spelling.spell)(&walk.payload));
            let (status, refused) = call(&router, walk.operation, &request).await;
            assert_eq!(
                (status, error_code(&refused).as_str()),
                (400, "ValidationException"),
                "case {case:?}: expected 400 ValidationException, got {status} {refused:?}"
            );
            assert!(
                !refused["message"].as_str().unwrap_or_default().is_empty(),
                "case {case:?}: expected a refusal message, got {refused:?}"
            );
            // `cannotParse` is the `ValidationExceptionReason` value for input the
            // service cannot read back (the enum declares `unknownOperation`,
            // `cannotParse`, `fieldValidationFailed` and `other`), and the
            // `fieldList` entry names the member that carried it.
            assert_eq!(
                refused["reason"].as_str(),
                Some("cannotParse"),
                "case {case:?}: expected the reason for input that cannot be read back, \
                 got {refused:?}"
            );
            assert_eq!(
                refused["fieldList"][0]["name"].as_str(),
                Some("nextToken"),
                "case {case:?}: expected the fieldList to name nextToken, got {refused:?}"
            );
            refusals.push((
                case,
                json!({
                    "message": refused["message"].clone(),
                    "reason": refused["reason"].clone(),
                    "fieldList": refused["fieldList"].clone(),
                }),
            ));
        }

        // One codec, one refusal: whichever read answered, the client sees the
        // same message, the same reason and the same offending field.
        let (first_case, first) = &refusals[0];
        for (case, refusal) in &refusals[1..] {
            assert_eq!(
                refusal, first,
                "case {case:?}: expected the refusal {first_case:?} answers with ({first:?}), \
                 got {refusal:?}"
            );
        }
    }
}
