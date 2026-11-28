// SPDX-License-Identifier: CC0-1.0

//! What a public key read does with the `Id` it is addressed with, over an account holding
//! SEVERAL keys, end to end over HTTP.
//!
//! RULE under test: `GetPublicKey` (`GET /2020-05-31/public-key/{Id}`) and
//! `GetPublicKeyConfig` (`GET /2020-05-31/public-key/{Id}/config`) answer one 404,
//! `NoSuchPublicKey`, beside `AccessDenied` and no argument error at all
//! (`smithy_query(query="operation",operation="GetPublicKey,GetPublicKeyConfig")`), so the
//! MESSAGE is what tells two conditions apart. EVERY id the account really holds is served from
//! that key's own
//! record; every id the account holds no key under is the key reported ABSENT, whatever alphabet
//! and width it is written in, because `Id` is a plain `string` the model constrains no further
//! and WHICH identifier was ever issued is the store's to say; and a value that IS an identifier
//! CloudFront issued for ANOTHER kind of this service — the identifier alphabet at the issued
//! width under another kind's namespace letter — is the identifier the route reports it cannot
//! READ, which tells a caller its own identifier handling is broken rather than that a
//! resource is not there.
//!
//! What this suite adds to its siblings, which drive ONE stored key
//! ([`crate::public_key_absent_id_one_sentence`],
//! [`crate::public_key_id_form_boundary`]), is the other half of the same rule: an account
//! holding SEVERAL keys resolves each minted id to that key's own record, so the reads are
//! shown to resolve against state rather than to answer whatever single record is there. The
//! rows then vary what the rule generalises over by deriving, from each minted id, the ways a
//! value can MISS the store — the id one character short of the issued width, with its body
//! digits-only, recased, one character too long and broken by a separator, beside values of no
//! CloudFront identifier shape at all, each the absent key — beside the one way it names another
//! kind's record instead: the issued width under another namespace letter.
//! None of them is an id the account holds, and each one earns the sentence its own form
//! decides from both readers.
//!
//! The wire harness, the configuration writer and the create and read helpers are the crate's
//! own, reused rather than copied.

use std::collections::BTreeSet;

use smithy_server_runtime::MultiServiceRouter;

use crate::distribution_lifecycle::{call, emulator, Answer, API_VERSION};
use crate::public_key_lifecycle::{create_key, get_key, pem, Key};
use crate::response_headers_policy_lifecycle::text;

/// The sentence `NoSuchPublicKey` carries for an `Id` the account holds no key under, spelled
/// out here as a literal rather than read off the crate under test, so the WORDING is what
/// these rows are held to.
const NO_SUCH_PUBLIC_KEY: &str = "The specified CloudFront public key does not exist.";

/// The sentence that says a caller's own identifier handling is broken, which these routes
/// answer a value that could never be an identifier issued for a public key.
const UNREADABLE_IDENTIFIER: &str = "Id format is invalid";

/// The one 404 code both readers model for an `Id` they cannot resolve.
const NO_SUCH_PUBLIC_KEY_CODE: &str = "NoSuchPublicKey";

/// `GET /2020-05-31/public-key/{Id}/config` — the second reader of the one stored record.
async fn get_key_document(router: &MultiServiceRouter, id: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/{API_VERSION}/public-key/{id}/config"),
        None,
        None,
    )
    .await
}

/// One key this account really holds: the configuration its create submitted beside the
/// identity, version token and upload instant that create minted.
#[derive(Debug, Clone)]
struct Stored {
    key: Key,
    id: String,
    etag: String,
    created_time: String,
}

/// The keys the account holds, varied along the members a create may differ in — the shape of
/// the encoded key, and whether a comment is carried at all, is empty, or carries text — so no
/// assertion below rests on one configuration's shape.
fn table() -> Vec<Key> {
    vec![
        Key {
            case: "a key uploaded with a multi-line key and a comment".to_string(),
            caller_reference: "each-minted-id-1".to_string(),
            name: "each-minted-id-1".to_string(),
            encoded_key: pem(4, 'A'),
            comment: Some("the first key the account holds".to_string()),
        },
        Key {
            case: "a key uploaded without a comment".to_string(),
            caller_reference: "each-minted-id-2".to_string(),
            name: "each-minted-id-2".to_string(),
            encoded_key: pem(2, 'B'),
            comment: None,
        },
        Key {
            case: "a key uploaded with an empty comment".to_string(),
            caller_reference: "each-minted-id-3".to_string(),
            name: "each-minted-id-3".to_string(),
            encoded_key: pem(3, 'C'),
            comment: Some(String::new()),
        },
        Key {
            case: "a key uploaded with a short key and a comment".to_string(),
            caller_reference: "each-minted-id-4".to_string(),
            name: "each-minted-id-4".to_string(),
            encoded_key: pem(1, 'D'),
            comment: Some("the last key the account holds".to_string()),
        },
    ]
}

/// Upload every row of [`table`] and report what each create minted.
async fn seed(router: &MultiServiceRouter) -> Vec<Stored> {
    let mut stored = Vec::new();
    for key in table() {
        let created = create_key(router, &key.xml()).await;
        assert_eq!(
            created.status, 201,
            "case {}: expected HTTP 201 from CreatePublicKey, got {} with {}",
            key.case, created.status, created.raw
        );
        let document = created.root();
        let id = text(document, "Id", &key.case, "the create", &created.raw);
        let created_time = text(document, "CreatedTime", &key.case, "the create", &created.raw);
        stored.push(Stored {
            key,
            id,
            etag: created.header("etag").to_string(),
            created_time,
        });
    }

    // Every create minted its own identity: the rows below are distinct keys rather than one
    // record read four times.
    let ids: BTreeSet<String> = stored.iter().map(|held| held.id.clone()).collect();
    assert_eq!(
        ids.len(),
        stored.len(),
        "case the seeded account: expected one minted id per key, got {ids:?}"
    );
    stored
}

/// Assert that both readers serve `held` at its own minted id, from that key's own record.
async fn assert_served(router: &MultiServiceRouter, held: &Stored) {
    let case = &held.key.case;
    let read = get_key(router, &held.id).await;
    assert_eq!(
        read.status, 200,
        "case {case:?} (GetPublicKey, Id {:?}): expected HTTP 200, got {} with {}",
        held.id, read.status, read.raw
    );
    assert_eq!(
        read.root().text_at("Id"),
        Some(held.id.as_str()),
        "case {case:?} (GetPublicKey): expected the read to report the id it was addressed with \
         ({:?}), got {}",
        held.id,
        read.raw
    );
    assert_eq!(
        read.root().text_at("CreatedTime"),
        Some(held.created_time.as_str()),
        "case {case:?} (GetPublicKey): expected the instant the create recorded ({:?}), got {}",
        held.created_time,
        read.raw
    );
    let config = read.root().at("PublicKeyConfig").unwrap_or_else(|| {
        panic!(
            "case {case:?} (GetPublicKey): the read carries no PublicKeyConfig: {}",
            read.raw
        )
    });
    held.key.assert_matches(config, "GetPublicKey", &read.raw);
    assert_eq!(
        read.header("etag"),
        held.etag,
        "case {case:?} (GetPublicKey): expected the version token the create handed back ({:?}), \
         got {:?}",
        held.etag,
        read.header("etag")
    );

    // `PublicKeyConfig` is the other read's `@httpPayload`, so the configuration IS that
    // document's root element — and both readers answer from the one record.
    let document = get_key_document(router, &held.id).await;
    assert_eq!(
        document.status, 200,
        "case {case:?} (GetPublicKeyConfig, Id {:?}): expected HTTP 200, got {} with {}",
        held.id, document.status, document.raw
    );
    held.key
        .assert_matches(document.root(), "GetPublicKeyConfig", &document.raw);
    assert_eq!(
        document.header("etag"),
        held.etag,
        "case {case:?} (GetPublicKeyConfig): expected one version token across both reads, got \
         {:?} and {:?}",
        document.header("etag"),
        held.etag
    );
}

/// The ways a value can MISS the store: `(case, value)`.
///
/// Every row is derived from `minted` rather than written out here, so none of them can drift
/// from the form this service issues — the width (one character short of the issued one, and one
/// past it), the body's character classes (digits only), the case it is written in and a
/// separator through it. None is an identifier issued for another kind, so each is the store's
/// to answer for.
fn near_misses(minted: &str) -> Vec<(String, String)> {
    let (namespace, body) = minted.split_at(1);
    let shortened: String = body.chars().take(body.chars().count() - 1).collect();

    vec![
        (
            "the minted id one character short".to_string(),
            format!("{namespace}{shortened}"),
        ),
        (
            "the minted width over a digits-only body".to_string(),
            format!("{namespace}{}", "9".repeat(body.chars().count())),
        ),
        ("the minted id in lower case".to_string(), minted.to_lowercase()),
        (
            "the minted id one character longer".to_string(),
            format!("{minted}9"),
        ),
        (
            "the minted id's body broken by a separator".to_string(),
            format!("{namespace}{shortened}-9"),
        ),
    ]
}

/// The one way a value stops being an identifier a public key could answer to, derived from a
/// minted id: it is written as an identifier is ISSUED under the namespace letter of ANOTHER
/// kind of this service, which makes it that kind's identifier: `(case, value)`.
fn unreadable_forms(minted: &str) -> Vec<(String, String)> {
    let (namespace, body) = minted.split_at(1);
    // The namespace letter this service does NOT mint a public key under, so a value carrying it
    // at the issued width is an identifier of that other kind however well-formed its body is.
    let other_namespace = if namespace == "E" { "K" } else { "E" };

    vec![(
        "the minted id under another namespace letter".to_string(),
        format!("{other_namespace}{body}"),
    )]
}

/// Values of no CloudFront identifier shape at all: `(case, value)`.
///
/// None of them is an identifier issued for another kind of this service, so each is resolved
/// against the STORE — `Id` is a plain `string` the model constrains no further, so what a
/// value is written in says nothing about whether a key answers to it.
fn values_of_no_identifier_shape() -> Vec<(String, String)> {
    vec![
        ("a bare word".to_string(), "not-an-identifier".to_string()),
        (
            "a dotted name, as a host is written".to_string(),
            "key.example.com".to_string(),
        ),
        (
            "a namespace-labelled opaque id".to_string(),
            "dt_3JO1OQa51Jb3BZh7XQh6pbvF3Wa".to_string(),
        ),
        (
            "a value far wider than any id this service mints".to_string(),
            "K".repeat(256),
        ),
    ]
}

/// Values the store is left to answer for, because the value alone says nothing that rules a
/// public key out: one of the identifier alphabet narrower than the width an identifier is issued
/// at, which could be one CloudFront wrote at another width, and one written in the UUID form
/// CloudFront issues the identifiers of its other kinds in, which carries no namespace marker at
/// all: `(case, value)`.
fn values_narrower_than_the_issued_width() -> Vec<(String, String)> {
    vec![
        ("a bare digit".to_string(), "7".to_string()),
        // The OTHER form CloudFront issues an identifier in: a UUID carries no namespace
        // marker at all and is shared by every kind identified by one, so it says which kind
        // issued it no more than a letter no kind issues under does, and the store answers for
        // it.
        (
            "a UUID, the shape the policy kinds' ids take".to_string(),
            "00000000-0000-0000-0000-000000000000".to_string(),
        ),
    ]
}

/// RULE: every id the account really holds is served from its OWN record, by both readers.
#[tokio::test]
async fn every_minted_id_is_served_from_its_own_record() {
    let router = emulator();
    let stored = seed(&router).await;

    for held in &stored {
        assert_served(&router, held).await;
    }

    // Reading one key never answers from another: each configuration came back under its own
    // id above, so the names the account holds are as many as its keys.
    let names: BTreeSet<String> = stored.iter().map(|held| held.key.name.clone()).collect();
    assert_eq!(
        names.len(),
        stored.len(),
        "case the seeded account: expected each read to answer from its own record, got {names:?}"
    );
}

/// RULE: an id the account holds no key under is the key reported ABSENT by both readers, while
/// a value that IS an identifier issued for another kind of this service earns the
/// unreadable-identifier sentence — the two sentences the one code carries.
#[tokio::test]
async fn every_id_the_account_holds_no_key_under_earns_the_sentence_its_form_decides() {
    let router = emulator();
    let stored = seed(&router).await;
    let held: BTreeSet<String> = stored.iter().map(|key| key.id.clone()).collect();

    let mut rows: Vec<(String, String, &str)> = values_narrower_than_the_issued_width()
        .into_iter()
        .map(|(case, id)| (case, id, NO_SUCH_PUBLIC_KEY))
        .chain(
            values_of_no_identifier_shape()
                .into_iter()
                .map(|(case, id)| (case, id, NO_SUCH_PUBLIC_KEY)),
        )
        .collect();
    for key in &stored {
        rows.extend(
            near_misses(&key.id)
                .into_iter()
                .map(|(case, id)| (case, id, NO_SUCH_PUBLIC_KEY)),
        );
        rows.extend(
            unreadable_forms(&key.id)
                .into_iter()
                .map(|(case, id)| (case, id, UNREADABLE_IDENTIFIER)),
        );
    }

    for (case, id, expected) in &rows {
        assert!(
            !held.contains(id),
            "case {case:?}: the value submitted must be one the account holds no key under, got \
             the stored id {id:?}"
        );
        for (reader, answer) in [
            ("GetPublicKey", get_key(&router, id).await),
            ("GetPublicKeyConfig", get_key_document(&router, id).await),
        ] {
            assert_eq!(
                answer.status, 404,
                "case {case:?} ({reader}, Id {id:?}): expected HTTP 404, got {} with {}",
                answer.status, answer.raw
            );
            assert_eq!(
                answer.error_code(),
                NO_SUCH_PUBLIC_KEY_CODE,
                "case {case:?} ({reader}, Id {id:?}): expected {NO_SUCH_PUBLIC_KEY_CODE}, got {}",
                answer.raw
            );
            let message = answer.root().find_text("Message").unwrap_or_default();
            assert_eq!(
                message, *expected,
                "case {case:?} ({reader}, Id {id:?}): expected {expected:?}, got \
                 {message:?}"
            );
            assert!(
                !message.contains(id.as_str()),
                "case {case:?} ({reader}): the refusal must not echo the Id, got {message:?}"
            );
        }
    }

    // The control: every key the account really holds is still served from its own record, at
    // the version its create handed back, so not one refusal above touched the store.
    for key in &stored {
        assert_served(&router, key).await;
    }
}
