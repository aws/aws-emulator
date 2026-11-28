// SPDX-License-Identifier: CC0-1.0

//! A value written in the UUID form CloudFront issues identifiers in, submitted where an
//! identifier of a kind minted under its OWN namespace letter belongs, end to end over HTTP.
//!
//! RULE under test: the operations addressed by a public key `Id` and by a field-level
//! encryption profile `Id` model ONE not-found error each for an `Id` they cannot resolve
//! (`smithy_query(query="operation",operation="GetPublicKey,GetPublicKeyConfig,UpdatePublicKey,\
//! DeletePublicKey,GetFieldLevelEncryptionProfile")` reports each kind's not-found error and
//! `AccessDenied` and no argument error, over an `Id` that is a plain `string` `@httpLabel` with
//! no `@pattern` and no `@length`), so the MESSAGE is the whole of what tells two conditions
//! apart, and what decides it is whether the submitted value could be an identifier CloudFront
//! ISSUED FOR THE ADDRESSED KIND. CloudFront issues an identifier in two forms: the prefixed
//! TOKEN, whose leading character names the kind that issued it, and the UUID its key groups,
//! its policy kinds and its key value stores are minted with. A UUID carries no such marker at
//! all — every kind identified by one is minted in the same form — so it says which kind issued
//! it no more than a token under a letter no kind issues under does: it is carried through to the
//! STORE, and the record the store holds none under is the record reported ABSENT, never the
//! identifier the route reports it cannot READ.
//!
//! The rows span what the rule generalises over:
//!
//! * the PROVENANCE of the UUID: one a `CreateKeyGroup` of this account really minted (so the
//!   value is an identifier of another kind that really resolves), and three nobody minted;
//! * the BODY of the UUID: both extremes of its hex alphabet (all zeros and all `f`s) and a
//!   mixed one;
//! * the ADDRESSED KIND: a public key, through every operation addressed by one of its
//!   identifiers — both readers, the update and the delete — and a field-level encryption
//!   profile, whose identifiers come from a namespace of its own as well;
//! * and the BOUNDARY of the form, in the second test: values that look like a UUID but are
//!   none — one in upper case, one with its separators removed, one with a group of the wrong
//!   width and one carrying a sixth group — which the profile's route, reading the identifier
//!   ALPHABET, reports as the identifier it cannot read, while the public key's routes, reading
//!   only whether a value IS an identifier issued for another kind, resolve them against the
//!   store: how much of a value a route reads before it resolves it is that kind's own recorded
//!   contract.
//!
//! Both sentences are fixed text, so no row may see its value echoed back, and the records this
//! account really holds are read at their own identifiers afterwards: these are reporting calls
//! and one refused update and delete per row, so nothing above may change what a later caller
//! sees.
//!
//! The wire harness and the create helpers are the crate's own, reused rather than copied.

use smithy_server_runtime::MultiServiceRouter;

use crate::distribution_lifecycle::{call, emulator, is_minted_uuid, Answer, API_VERSION};
use crate::field_level_encryption_profile_lifecycle::{get_profile, store_profile};
use crate::key_group_lifecycle::{create_group, Group};
use crate::public_key_delete::delete_key;
use crate::public_key_lifecycle::{create_key, get_key, pem, Key};

/// The sentence `NoSuchPublicKey` carries for an `Id` the account holds no key under, written
/// out here rather than read off the crate under test: the WORDING is what these rows are held
/// to.
const NO_SUCH_PUBLIC_KEY: &str = "The specified CloudFront public key does not exist.";

/// The sentence `NoSuchFieldLevelEncryptionProfile` carries for an `Id` the account holds no
/// profile under.
const NO_SUCH_PROFILE: &str = "The specified field level encryption profile does not exist.";

/// The sentence those codes carry for a value that could be an identifier of neither form
/// CloudFront issues one in.
const UNREADABLE_IDENTIFIER: &str = "Id format is invalid";

/// The caller reference the records this suite stands up are created under.
const REFERENCE: &str = "uuid-form-identifier";

/// `GET /2020-05-31/public-key/{Id}/config` — the second reader of the one stored key.
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

/// The records this account really holds while the rows below are driven: one public key (with
/// the version token its create handed out), one key group listing it — whose own id is a UUID —
/// and one field-level encryption profile.
struct Held {
    key_id: String,
    key_etag: String,
    group_id: String,
    profile_id: String,
    /// The `PublicKeyConfig` document every refused update submits, which no refusal may store.
    key_document: String,
}

/// Stand up the records the rows are driven against, asserting each create answered.
async fn hold(router: &MultiServiceRouter) -> Held {
    let key = Key {
        case: "the one key the account holds".to_string(),
        caller_reference: REFERENCE.to_string(),
        name: REFERENCE.to_string(),
        encoded_key: pem(3, 'U'),
        comment: Some("the key every refusal below must leave alone".to_string()),
    };
    let created = create_key(router, &key.xml()).await;
    assert_eq!(
        created.status, 201,
        "expected HTTP 201 from CreatePublicKey, got {} with {}",
        created.status, created.raw
    );
    let key_id = created.root().text_at("Id").unwrap_or_default().to_string();
    let key_etag = created.header("etag").to_string();

    let group = Group {
        case: "the one key group the account holds".to_string(),
        name: REFERENCE.to_string(),
        items: vec![key_id.clone()],
        comment: None,
    };
    let group_answer = create_group(router, &group.xml()).await;
    assert_eq!(
        group_answer.status, 201,
        "expected HTTP 201 from CreateKeyGroup, got {} with {}",
        group_answer.status, group_answer.raw
    );
    let group_id = group_answer
        .root()
        .text_at("Id")
        .unwrap_or_default()
        .to_string();
    // The key group id is MINTED here rather than written by hand, so the row that submits it
    // carries a real identifier of another kind — and it really is of the UUID form.
    assert!(
        is_minted_uuid(&group_id),
        "expected a key group id drawn as a UUID, got {group_id:?}"
    );

    let profile_id = store_profile(router, &format!("{REFERENCE}-profile")).await;

    let key_document = Key {
        comment: Some("never stored".to_string()),
        ..key
    }
    .xml();

    Held {
        key_id,
        key_etag,
        group_id,
        profile_id,
        key_document,
    }
}

/// Address `id` through every operation of this surface that is addressed by one of these
/// identifiers, answering `(operation, the kind's not-found code, the answer)`.
async fn address(
    router: &MultiServiceRouter,
    id: &str,
    held: &Held,
) -> Vec<(&'static str, &'static str, Answer)> {
    vec![
        ("GetPublicKey", "NoSuchPublicKey", get_key(router, id).await),
        (
            "GetPublicKeyConfig",
            "NoSuchPublicKey",
            get_key_config(router, id).await,
        ),
        (
            "UpdatePublicKey",
            "NoSuchPublicKey",
            update_key(router, id, &held.key_document, &held.key_etag).await,
        ),
        (
            "DeletePublicKey",
            "NoSuchPublicKey",
            delete_key(router, id, Some(&held.key_etag)).await,
        ),
        (
            "GetFieldLevelEncryptionProfile",
            "NoSuchFieldLevelEncryptionProfile",
            get_profile(router, id).await,
        ),
    ]
}

/// The sentence the kind behind `code` answers for an identifier it holds no record under.
fn absent(code: &str) -> &'static str {
    match code {
        "NoSuchPublicKey" => NO_SUCH_PUBLIC_KEY,
        _ => NO_SUCH_PROFILE,
    }
}

/// Assert one refusal: HTTP 404, the kind's own not-found code, the sentence `expected`, and
/// that the value submitted is nowhere in the answer.
fn assert_refusal(answer: &Answer, case: &str, code: &str, expected: &str, id: &str) {
    assert_eq!(
        answer.status, 404,
        "case {case}: expected HTTP 404, got {} with {}",
        answer.status, answer.raw
    );
    assert_eq!(
        answer.error_code(),
        code,
        "case {case}: expected {code}, got {}",
        answer.raw
    );
    let actual = answer.root().find_text("Message").unwrap_or_default();
    assert_eq!(
        actual, expected,
        "case {case}: expected {expected:?}, got {actual:?}"
    );
    assert!(
        !answer.raw.contains(id),
        "case {case}: both sentences are fixed text, but the answer echoed {id:?} in {}",
        answer.raw
    );
}

/// Read the records the account really holds back at their own identifiers: every refusal above
/// answered an identifier rather than a store this suite broke.
async fn assert_held_records_survived(router: &MultiServiceRouter, held: &Held) {
    let key = get_key(router, &held.key_id).await;
    assert_eq!(
        key.status, 200,
        "case the held public key: expected it to survive every refusal, got {} with {}",
        key.status, key.raw
    );
    assert_eq!(
        key.header("etag"),
        held.key_etag,
        "case the held public key: expected it to keep the version its create handed out, got {:?}",
        key.header("etag")
    );
    let profile = get_profile(router, &held.profile_id).await;
    assert_eq!(
        profile.status, 200,
        "case the held profile: expected it to survive every refusal, got {} with {}",
        profile.status, profile.raw
    );
}

/// The sentence the kind behind `code` answers for a value of NEITHER form CloudFront issues an
/// identifier in.
///
/// How much of such a value a route reads before it resolves it is that resource kind's own
/// recorded contract: the field-level encryption profile's routes read the identifier ALPHABET,
/// so a value carrying lower case or a separator is the identifier they report they cannot READ,
/// while the public key's routes read only whether the value IS an identifier issued for another
/// kind of this service, so every other value — this one included — is resolved against the
/// store and reported as the key that is ABSENT.
fn unreadable_or_absent(code: &str) -> &'static str {
    match code {
        "NoSuchPublicKey" => NO_SUCH_PUBLIC_KEY,
        _ => UNREADABLE_IDENTIFIER,
    }
}

/// RULE: a value written in the UUID form CloudFront issues identifiers in is carried through to
/// the store wherever an identifier of a kind minted under its own namespace letter belongs, so
/// the record the store holds none under is the record reported ABSENT — whichever of that
/// kind's operations is addressed by it, and whether or not the UUID really names a resource of
/// another kind.
#[tokio::test]
async fn a_value_of_the_uuid_form_is_resolved_against_the_store() {
    let router = emulator();
    let held = hold(&router).await;

    // (case, the `Id` submitted) — the key group's really-minted UUID, then UUIDs nobody minted:
    // both extremes of the hex alphabet and a mixed one.
    let rows: Vec<(String, String)> = vec![
        (
            "a key group id this account really holds".to_string(),
            held.group_id.clone(),
        ),
        (
            "a UUID of zeros, never minted".to_string(),
            "00000000-0000-0000-0000-000000000000".to_string(),
        ),
        (
            "a UUID of the alphabet's other extreme, never minted".to_string(),
            "ffffffff-ffff-ffff-ffff-ffffffffffff".to_string(),
        ),
        (
            "a mixed UUID, never minted".to_string(),
            "3f2504e0-4f89-41d3-9a0c-0305e82c3301".to_string(),
        ),
    ];

    for (case, id) in &rows {
        assert!(
            is_minted_uuid(id),
            "case {case:?}: the value submitted must be of the UUID form, got {id:?}"
        );
        for (operation, code, answer) in address(&router, id, &held).await {
            assert_refusal(
                &answer,
                &format!("{operation} with {case} ({id:?})"),
                code,
                absent(code),
                id,
            );
        }
    }

    assert_held_records_survived(&router, &held).await;
}

/// The RULE's boundary: a value that only LOOKS like a UUID is of neither form CloudFront issues
/// an identifier in, so the profile's route — which reads the identifier alphabet — reports the
/// identifier it cannot read, while the public key's routes, which read only whether a value is
/// an identifier issued for another kind, resolve it against the store and report the key as
/// absent.
#[tokio::test]
async fn a_value_that_is_no_uuid_is_reported_unreadable() {
    let router = emulator();
    let held = hold(&router).await;

    // One real UUID, broken one way per row: recased, with its separators removed, with a group
    // of the wrong width, and with a sixth group no UUID carries.
    let uuid = "3f2504e0-4f89-41d3-9a0c-0305e82c3301";
    let rows: Vec<(String, String)> = vec![
        ("a UUID in upper case".to_string(), uuid.to_uppercase()),
        (
            "a UUID with its separators removed".to_string(),
            uuid.replace('-', ""),
        ),
        (
            "a UUID with a group one character short".to_string(),
            "3f2504e0-4f89-41d3-9a0c-0305e82c330".to_string(),
        ),
        (
            "a UUID carrying a sixth group".to_string(),
            format!("{uuid}-0305e82c3301"),
        ),
    ];

    for (case, id) in &rows {
        assert!(
            !is_minted_uuid(id),
            "case {case:?}: the value submitted must NOT be of the UUID form, got {id:?}"
        );
        for (operation, code, answer) in address(&router, id, &held).await {
            assert_refusal(
                &answer,
                &format!("{operation} with {case} ({id:?})"),
                code,
                unreadable_or_absent(code),
                id,
            );
        }
    }

    assert_held_records_survived(&router, &held).await;
}
