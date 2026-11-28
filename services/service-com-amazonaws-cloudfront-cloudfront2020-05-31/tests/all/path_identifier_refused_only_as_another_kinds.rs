// SPDX-License-Identifier: CC0-1.0

//! WHICH submitted value a read-by-identifier refuses on its FORM, over the two kinds whose
//! recorded answers settle it, end to end over HTTP.
//!
//! RULE under test: an operation addressed by a path identifier models ONE not-found code for an
//! `Id` it cannot resolve (`smithy_query(query="operation",operation="GetPublicKey,\
//! GetPublicKeyConfig,UpdatePublicKey,DeletePublicKey,GetFieldLevelEncryptionConfig,\
//! GetFieldLevelEncryptionProfile")` reports each kind's not-found error and `AccessDenied` and
//! no argument error, over an `Id` that is a plain `string` `@httpLabel` with no `@pattern` and
//! no `@length`), so the MESSAGE is what tells two conditions apart — and the only thing a
//! REQUEST says that rules the addressed kind out is that the value IS an identifier CloudFront
//! issued for ANOTHER kind of this service: the identifier alphabet (decimal digits and
//! uppercase letters) at the width an identifier is issued at, under another kind's namespace
//! letter. Such a value names that other kind's record, so the route reports the identifier it
//! cannot READ. EVERY other value is carried through to the STORE, because WHICH identifier of a
//! kind was ever issued is a property of the store and the width this service mints at is one
//! width CloudFront writes an identifier in rather than the only one — so the record the store
//! holds none under is the record reported ABSENT, whatever width the value is written at.
//!
//! Beyond that one claim, how much of a value a route reads is the addressed kind's own recorded
//! contract: the field-level encryption records' routes also report the identifier they cannot
//! read for a value written OUTSIDE that alphabet, while the public key's routes read nothing
//! else at all and resolve such a value against the store.
//!
//! The rows span what the rule generalises over — the RELATION the submitted value has to the
//! namespaces this service issues identifiers in, at the widths it writes them at:
//!
//! * the alphabet at the issued width under the addressed kind's OWN letter, never minted;
//! * the alphabet WIDER than the issued width, under the kind's own letter and under a letter no
//!   kind of this service issues an identifier under (the shape of a word a caller writes when it
//!   makes an identifier up);
//! * the alphabet NARROWER than the issued width;
//! * the alphabet at the issued width under a letter no kind issues an identifier under;
//! * the UUID form CloudFront issues the identifiers of its other kinds in, which carries no
//!   namespace marker at all;
//! * a value written outside the alphabet, in lower case around separators — the form a caller
//!   names a resource of its own with;
//! * and the identifiers this account REALLY HOLDS for two other kinds, minted by their own
//!   creates: a public key where a field-level encryption identifier belongs, and a field-level
//!   encryption profile and configuration where a public key identifier belongs.
//!
//! Each row is driven through every operation of the addressed kind, and the records this
//! account really holds are read back at their own identifiers afterwards: these are reporting
//! calls and refused mutations, so none of them may change what a later caller sees.
//!
//! The wire harness and the create helpers are the crate's own, reused rather than copied.

use smithy_server_runtime::MultiServiceRouter;

use crate::distribution_lifecycle::{call, emulator, Answer, API_VERSION};
use crate::field_level_encryption_config_lifecycle::{
    create_config, get_config_document, simple,
};
use crate::field_level_encryption_profile_lifecycle::{
    create_profile, get_profile, upload_key, Entity, Profile,
};
use crate::public_key_delete::delete_key;
use crate::public_key_lifecycle::{create_key, get_key, pem, Key};

/// The sentence each kind's not-found code carries for an identifier the store holds no record
/// under, written out here rather than read off the crate under test: the WORDING is what these
/// rows are held to.
const NO_SUCH_PUBLIC_KEY: &str = "The specified CloudFront public key does not exist.";
const NO_SUCH_CONFIG: &str = "The specified field level encryption config does not exist.";
const NO_SUCH_PROFILE: &str = "The specified field level encryption profile does not exist.";

/// The sentence the same codes carry for a value that IS an identifier issued for another kind.
const UNREADABLE_IDENTIFIER: &str = "Id format is invalid";

/// The caller reference every resource this suite stands up is created under, and the shape of a
/// name a caller gives a resource of its own: lower case around separators.
const REFERENCE: &str = "path-identifier-refusal-probe";

/// The identifiers this account really holds, each minted by its own create.
struct Held {
    public_key: String,
    profile: String,
    config: String,
    /// The document a refused update submits, which no row may store.
    key_document: String,
    /// The version token the public key's create handed out, so no refused mutation below is
    /// answered by a precondition instead.
    key_etag: String,
}

/// `GET /2020-05-31/public-key/{Id}/config` — the public key's second reader.
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

/// `PUT /2020-05-31/public-key/{Id}/config` under the version token `if_match`.
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

/// Create one public key, one field-level encryption profile and one configuration naming that
/// profile, each through its own API, and report the identifier each create handed back.
async fn hold(router: &MultiServiceRouter) -> Held {
    let key = Key {
        case: "the public key this account holds".to_string(),
        caller_reference: format!("{REFERENCE}-key"),
        name: format!("{REFERENCE}-key"),
        encoded_key: pem(4, 'R'),
        comment: Some("the key every refusal below must leave alone".to_string()),
    };
    let created_key = create_key(router, &key.xml()).await;
    assert_eq!(
        created_key.status, 201,
        "expected HTTP 201 from CreatePublicKey, got {} with {}",
        created_key.status, created_key.raw
    );
    let public_key = created_key
        .root()
        .text_at("Id")
        .unwrap_or_default()
        .to_string();
    let key_etag = created_key.header("etag").to_string();

    let encrypting_key = upload_key(router, 0).await;
    let created_profile = create_profile(
        router,
        &Profile::new(
            "the profile this account holds",
            &format!("{REFERENCE}-profile"),
            None,
            vec![Entity::new(&encrypting_key, "provider-one", &["SSN"])],
        )
        .xml(),
    )
    .await;
    assert_eq!(
        created_profile.status, 201,
        "expected HTTP 201 from CreateFieldLevelEncryptionProfile, got {} with {}",
        created_profile.status, created_profile.raw
    );
    let profile = created_profile
        .root()
        .text_at("Id")
        .unwrap_or_default()
        .to_string();

    let created_config = create_config(
        router,
        &simple(
            "the configuration this account holds",
            &format!("{REFERENCE}-config"),
            &profile,
        )
        .xml(),
    )
    .await;
    assert_eq!(
        created_config.status, 201,
        "expected HTTP 201 from CreateFieldLevelEncryptionConfig, got {} with {}",
        created_config.status, created_config.raw
    );
    let config = created_config
        .root()
        .text_at("Id")
        .unwrap_or_default()
        .to_string();

    // Every create answered with an identifier of one width under one letter, so the widths the
    // rows are derived from are read off the service rather than written out here.
    let issued = public_key.len();
    for (case, id) in [
        ("the public key", &public_key),
        ("the profile", &profile),
        ("the configuration", &config),
    ] {
        assert_eq!(
            id.len(),
            issued,
            "case {case}: expected an identifier of {issued} characters, got {id:?}"
        );
    }

    Held {
        public_key,
        profile,
        config,
        key_document: Key {
            comment: Some("never stored".to_string()),
            ..key
        }
        .xml(),
        key_etag,
    }
}

/// The values every kind's rows share, derived from `letter` (the addressed kind's own namespace
/// letter) and `issued` (the width this service issues an identifier at): `(case, id)`, each one
/// the record reported ABSENT.
fn carried_to_the_store(letter: char, issued: usize) -> Vec<(String, String)> {
    let body = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ"
        .chars()
        .cycle()
        .take(issued - 1)
        .collect::<String>();
    vec![
        (
            "the kind's own letter at the issued width, never minted".to_string(),
            format!("{letter}{body}"),
        ),
        (
            "the kind's own letter over a body one character too long".to_string(),
            format!("{letter}{body}7"),
        ),
        (
            "an uppercase word wider than any identifier is issued at".to_string(),
            format!("NONEXISTENTRECORD{body}"),
        ),
        (
            "the kind's own letter over a body one character short".to_string(),
            format!("{letter}{}", &body[..body.len() - 1]),
        ),
        (
            "a letter no kind of this service issues an identifier under".to_string(),
            format!("W{body}"),
        ),
        (
            "a UUID, the form CloudFront issues its other kinds' identifiers in".to_string(),
            "3f2504e0-4f89-41d3-9a0c-0305e82c3301".to_string(),
        ),
    ]
}

/// Assert `answer` is the 404 `code` carrying `expected`, and that it echoes nothing of `id`.
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
    let message = answer.root().find_text("Message").unwrap_or_default();
    assert_eq!(
        message, expected,
        "case {case}: expected {expected:?}, got {message:?}"
    );
    assert!(
        !message.contains(id),
        "case {case}: both sentences are fixed text, so the refusal must not echo {id:?}, got \
         {message:?}"
    );
}

/// RULE, on the four operations addressed by a PUBLIC KEY `Id`: only an identifier issued for
/// another kind of this service is refused on its form; every other value, in any characters and
/// at any width, is resolved against the store.
#[tokio::test]
async fn a_public_key_route_refuses_only_another_kinds_identifier() {
    let router = emulator();
    let held = hold(&router).await;
    let letter = held
        .public_key
        .chars()
        .next()
        .expect("a minted public key id");
    let issued = held.public_key.len();

    let mut rows: Vec<(String, String, &str)> = carried_to_the_store(letter, issued)
        .into_iter()
        .map(|(case, id)| (case, id, NO_SUCH_PUBLIC_KEY))
        .collect();
    // The recorded probe class: a name a caller writes for a resource of its own, in lower case
    // around separators, which claims no namespace of this service at all.
    rows.push((
        "a lower-case hyphenated name of no identifier form".to_string(),
        format!("{REFERENCE}-nonexistent-pubkey"),
        NO_SUCH_PUBLIC_KEY,
    ));
    rows.push((
        "the stored identifier in lower case".to_string(),
        held.public_key.to_lowercase(),
        NO_SUCH_PUBLIC_KEY,
    ));
    // And the one class the form rules out: an identifier this account really holds for another
    // kind of this service.
    rows.push((
        "the field-level encryption profile this account holds".to_string(),
        held.profile.clone(),
        UNREADABLE_IDENTIFIER,
    ));
    rows.push((
        "the field-level encryption configuration this account holds".to_string(),
        held.config.clone(),
        UNREADABLE_IDENTIFIER,
    ));

    for (case, id, expected) in &rows {
        assert_ne!(
            *id, held.public_key,
            "case {case}: the value submitted must not be the identifier the store holds"
        );
        for (operation, answer) in [
            ("GetPublicKey", get_key(&router, id).await),
            ("GetPublicKeyConfig", get_key_config(&router, id).await),
            (
                "UpdatePublicKey",
                update_key(&router, id, &held.key_document, &held.key_etag).await,
            ),
            (
                "DeletePublicKey",
                delete_key(&router, id, Some(&held.key_etag)).await,
            ),
        ] {
            assert_refusal(
                &answer,
                &format!("{operation} with {case} ({id:?})"),
                "NoSuchPublicKey",
                expected,
                id,
            );
        }
    }

    assert_held_records_survived(&router, &held).await;
}

/// RULE, on the field-level encryption records' reads: a value of the alphabet at any width
/// other than the issued one is resolved against the store — which is what the recorded answers
/// to an uppercase word wider than an identifier is issued report — while an identifier issued
/// for another kind, and a value written outside that alphabet, are the identifiers these routes
/// report they cannot read.
#[tokio::test]
async fn a_field_level_encryption_route_refuses_another_kinds_identifier_and_a_foreign_form() {
    let router = emulator();
    let held = hold(&router).await;
    let issued = held.config.len();

    for (kind, minted, code, absent, another_kinds) in [
        (
            "GetFieldLevelEncryptionConfig",
            held.config.clone(),
            "NoSuchFieldLevelEncryptionConfig",
            NO_SUCH_CONFIG,
            held.public_key.clone(),
        ),
        (
            "GetFieldLevelEncryptionProfile",
            held.profile.clone(),
            "NoSuchFieldLevelEncryptionProfile",
            NO_SUCH_PROFILE,
            held.public_key.clone(),
        ),
    ] {
        let letter = minted.chars().next().expect("a minted identifier");
        let mut rows: Vec<(String, String, &str)> = carried_to_the_store(letter, issued)
            .into_iter()
            .map(|(case, id)| (case, id, absent))
            .collect();
        rows.push((
            "the public key this account holds".to_string(),
            another_kinds,
            UNREADABLE_IDENTIFIER,
        ));
        rows.push((
            "a lower-case hyphenated name of no identifier form".to_string(),
            format!("{REFERENCE}-nonexistent"),
            UNREADABLE_IDENTIFIER,
        ));

        for (case, id, expected) in &rows {
            assert_ne!(
                *id, minted,
                "case {kind} with {case}: the value submitted must not be the identifier the \
                 store holds"
            );
            let answer = if kind == "GetFieldLevelEncryptionConfig" {
                get_config_document(&router, id).await
            } else {
                get_profile(&router, id).await
            };
            assert_refusal(
                &answer,
                &format!("{kind} with {case} ({id:?})"),
                code,
                expected,
                id,
            );
        }
    }

    assert_held_records_survived(&router, &held).await;
}

/// Read every record this account really holds back at its own identifier: every refusal above
/// answered a value rather than a store these rows broke.
async fn assert_held_records_survived(router: &MultiServiceRouter, held: &Held) {
    let key = get_key(router, &held.public_key).await;
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
    let profile = get_profile(router, &held.profile).await;
    assert_eq!(
        profile.status, 200,
        "case the held profile: expected it to survive every refusal, got {} with {}",
        profile.status, profile.raw
    );
    let config = get_config_document(router, &held.config).await;
    assert_eq!(
        config.status, 200,
        "case the held configuration: expected it to survive every refusal, got {} with {}",
        config.status, config.raw
    );
}
