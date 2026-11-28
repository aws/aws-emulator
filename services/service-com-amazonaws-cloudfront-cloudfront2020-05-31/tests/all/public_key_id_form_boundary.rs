// SPDX-License-Identifier: CC0-1.0

//! The WIDTH of a public key identifier, and which of the one 404 code's two sentences a value
//! at each width earns, end to end over HTTP.
//!
//! RULE under test: `GetPublicKey`, `GetPublicKeyConfig`, `UpdatePublicKey` and
//! `DeletePublicKey` model one 404 error for an `Id` they cannot resolve — `NoSuchPublicKey`
//! (`smithy_query(query="operation",operation="GetPublicKey,GetPublicKeyConfig,UpdatePublicKey,DeletePublicKey")`
//! reports it and `AccessDenied` and no argument error) — so the MESSAGE tells two conditions
//! apart, and what decides it is whether the value IS an identifier CloudFront issued for
//! another kind of this service: a value written as an identifier is ISSUED (the identifier
//! alphabet at the issued width) under ANOTHER kind's namespace letter names that other kind's
//! record, so no public key could ever answer to it and it is the identifier the route reports
//! it cannot READ. Every other value is carried through to the STORE, whatever width and
//! whatever characters it is written in, and the key the store holds none under is the key
//! reported ABSENT: `Id` is a plain `string` `@httpLabel` the model constrains no further, and
//! the width this service MINTS at is one width CloudFront writes an identifier in rather than
//! the only one.
//!
//! The sibling suites span the NAMESPACE the value belongs to
//! ([`crate::public_key_id_is_resolved_against_the_store`],
//! [`crate::refusal_message_fidelity`]); what this one spans is the WIDTH: the body one
//! character short of what this service mints, exactly that width, and one character past it,
//! plus the alphabet at that exact width (a lower-cased body, one carrying a separator) and
//! that width under another kind's letter, which is the one row the refusal belongs to. The
//! width is not written out here — it is read off an id `CreatePublicKey` MINTED in this test,
//! so the rows cannot drift from what the service issues.
//!
//! The wire harness and the create helpers are the crate's own.

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::PUBLIC_KEY_ID_PREFIX;
use smithy_server_runtime::MultiServiceRouter;

use crate::distribution_lifecycle::{call, emulator, Answer, API_VERSION};
use crate::public_key_delete::delete_key;
use crate::public_key_lifecycle::{create_key, get_key, pem, Key};

/// The sentence `NoSuchPublicKey` carries for an `Id` the account holds no key under, spelled
/// out as a literal rather than read off the crate under test, so the WORDING is what these
/// rows are held to.
const NO_SUCH_PUBLIC_KEY: &str = "The specified CloudFront public key does not exist.";

/// The sentence the same code carries for a value that could never be a public key identifier,
/// spelled out here for the same reason.
const UNREADABLE_IDENTIFIER: &str = "Id format is invalid";

/// `GET /2020-05-31/public-key/{Id}/config` — the second reader of the one stored record.
async fn get_key_config(router: &MultiServiceRouter, id: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/{API_VERSION}/public-key/{id}/config"),
        None,
        None,
    )
    .await
}

/// `PUT /2020-05-31/public-key/{Id}/config` with the `PublicKeyConfig` document `body`, under
/// the version token `if_match`.
async fn update_key(router: &MultiServiceRouter, id: &str, body: &str, if_match: &str) -> Answer {
    call(
        router,
        "PUT",
        &format!("/{API_VERSION}/public-key/{id}/config"),
        Some(body),
        Some(if_match),
    )
    .await
}

/// RULE: the width an identifier is issued at is the boundary — a value no wider than it, in the
/// alphabet and under this kind's letter, earns the absent-key sentence, and a wider one earns
/// the unreadable-identifier sentence — and the key the account really holds is left alone by
/// every row.
#[tokio::test]
async fn the_width_of_an_id_decides_which_sentence_a_refusal_carries() {
    let router = emulator();

    let present = Key {
        case: "the one key the account holds".to_string(),
        caller_reference: "public-key-id-width".to_string(),
        name: "public-key-id-width".to_string(),
        encoded_key: pem(4, 'W'),
        comment: Some("the key every refusal below must leave alone".to_string()),
    };
    let created = create_key(&router, &present.xml()).await;
    assert_eq!(
        created.status, 201,
        "expected HTTP 201 from CreatePublicKey, got {} with {}",
        created.status, created.raw
    );
    let minted = created.root().text_at("Id").unwrap_or_default().to_string();
    let etag = created.header("etag").to_string();

    // The width this service ISSUES an identifier body at, taken from the id it just minted
    // rather than written out here.
    let body_width = minted
        .strip_prefix(PUBLIC_KEY_ID_PREFIX)
        .unwrap_or_else(|| panic!("a minted public key id carries the namespace letter: {minted:?}"))
        .chars()
        .count();
    assert!(
        body_width > 1,
        "expected a minted id with a body to measure, got {minted:?}"
    );

    // (case, the `Id` the operations are addressed with, the sentence the one code carries)
    let rows: Vec<(String, String, &str)> = vec![
        (
            "a body one character short of the minted width".to_string(),
            format!("{PUBLIC_KEY_ID_PREFIX}{}", "A".repeat(body_width - 1)),
            NO_SUCH_PUBLIC_KEY,
        ),
        (
            "a body of exactly the minted width, never issued".to_string(),
            format!("{PUBLIC_KEY_ID_PREFIX}{}", "A".repeat(body_width)),
            NO_SUCH_PUBLIC_KEY,
        ),
        (
            "a body of the minted width, drawn from the digits of the alphabet".to_string(),
            format!("{PUBLIC_KEY_ID_PREFIX}{}", "9".repeat(body_width)),
            NO_SUCH_PUBLIC_KEY,
        ),
        (
            "a body one character past the minted width".to_string(),
            format!("{PUBLIC_KEY_ID_PREFIX}{}", "A".repeat(body_width + 1)),
            NO_SUCH_PUBLIC_KEY,
        ),
        (
            "a body of the minted width in lower case".to_string(),
            format!("{PUBLIC_KEY_ID_PREFIX}{}", "a".repeat(body_width)),
            NO_SUCH_PUBLIC_KEY,
        ),
        (
            "a body of the minted width carrying a separator".to_string(),
            format!(
                "{PUBLIC_KEY_ID_PREFIX}{}-{}",
                "A".repeat(body_width - 3),
                "AA"
            ),
            NO_SUCH_PUBLIC_KEY,
        ),
        (
            "the namespace letter with no body at all".to_string(),
            PUBLIC_KEY_ID_PREFIX.to_string(),
            NO_SUCH_PUBLIC_KEY,
        ),
        (
            "a body of the minted width under another namespace letter".to_string(),
            format!("E{}", "A".repeat(body_width)),
            UNREADABLE_IDENTIFIER,
        ),
    ];

    let body = Key {
        comment: Some("never stored".to_string()),
        ..present.clone()
    }
    .xml();

    for (case, id, expected) in &rows {
        for (operation, answer) in [
            ("GetPublicKey", get_key(&router, id).await),
            ("GetPublicKeyConfig", get_key_config(&router, id).await),
            ("UpdatePublicKey", update_key(&router, id, &body, &etag).await),
            ("DeletePublicKey", delete_key(&router, id, Some(&etag)).await),
        ] {
            assert_eq!(
                answer.status, 404,
                "case {case:?} ({operation}, Id {id:?}): expected HTTP 404, got {} with {}",
                answer.status, answer.raw
            );
            assert_eq!(
                answer.error_code(),
                "NoSuchPublicKey",
                "case {case:?} ({operation}, Id {id:?}): expected NoSuchPublicKey, got {}",
                answer.raw
            );
            let message = answer.root().find_text("Message").unwrap_or_default();
            assert_eq!(
                message, *expected,
                "case {case:?} ({operation}, Id {id:?}): expected {expected:?}, got \
                 {message:?}"
            );
            assert!(
                !message.contains(id.as_str()),
                "case {case:?} ({operation}): the refusal must not echo the Id, got {message:?}"
            );
        }
    }

    // The control: the id the service really minted still resolves, at the version its create
    // handed out, so not one row above touched the record — and the update of it that every
    // row's body asked for never happened.
    let still = get_key(&router, &minted).await;
    assert_eq!(
        still.status, 200,
        "expected the minted id to still resolve, got {} with {}",
        still.status, still.raw
    );
    assert_eq!(
        still.header("etag"),
        etag,
        "expected the stored key to keep its version token, got {:?}",
        still.header("etag")
    );
}
