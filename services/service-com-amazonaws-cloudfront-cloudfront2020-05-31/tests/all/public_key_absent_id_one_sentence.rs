// SPDX-License-Identifier: CC0-1.0

//! ONE code under TWO sentences, over every operation that addresses a public key by `Id`.
//!
//! RULE: `GetPublicKey`, `GetPublicKeyConfig`, `UpdatePublicKey` and `DeletePublicKey` model
//! one 404 error for an `Id` they cannot resolve — `NoSuchPublicKey`
//! (`smithy_query(query="operation",operation="GetPublicKey,GetPublicKeyConfig,UpdatePublicKey,DeletePublicKey")`
//! reports it and `AccessDenied` and no argument error) — so the MESSAGE is the whole of what
//! tells two conditions apart, and what decides it is whether the submitted value could be an
//! identifier CloudFront ISSUED FOR A PUBLIC KEY at all: a value written outside either form
//! CloudFront issues an identifier in, one written wider than an identifier of this mint is
//! issued, and one written exactly as an identifier is ISSUED — the identifier alphabet at the
//! issued width — under another kind's namespace letter are each the identifier the route reports
//! it cannot READ, while every other value is carried through to the STORE and the key it holds
//! none under is the key reported ABSENT. A value written in the UUID form CloudFront issues the
//! identifiers of its other kinds in is one of those: that form carries no namespace marker at
//! all and is shared by every kind identified by one, so it says which kind issued it no more
//! than a letter no kind issues under does. Both sentences are fixed text and echo nothing the
//! request sent.
//!
//! The rows span what the rule generalises over — the RELATION a value has to the identifier
//! namespaces this service mints in: a live distribution id, a live key group's UUID, both
//! field-level encryption namespaces, the public key namespace at widths this service mints
//! none at, a never-minted id of the right shape, the stored id recased and with a character
//! appended, the caller reference, a bare digit and a word of no id
//! shape at all. The key the account really holds is the control: it still answers 200 after
//! every row, so none of them touched the record.
//!
//! The wire harness and the create helpers are the crate's own.

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::{
    FIELD_LEVEL_ENCRYPTION_CONFIG_ID_PREFIX, FIELD_LEVEL_ENCRYPTION_PROFILE_ID_PREFIX,
    PUBLIC_KEY_ID_PREFIX,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::distribution_lifecycle::{call, config, emulator, Answer, API_VERSION};
use crate::key_group_lifecycle::{create_group, Group};
use crate::public_key_delete::delete_key;
use crate::public_key_lifecycle::{create_key, get_key, pem, Key};
use crate::web_acl_association::create as create_distribution_returning_id;

/// The sentence `NoSuchPublicKey` carries for an `Id` the account holds no key under, written
/// out here rather than read off the crate under test: the WORDING is what these rows are held
/// to.
const NO_SUCH_PUBLIC_KEY: &str = "The specified CloudFront public key does not exist.";

/// The sentence the same code carries for a value that could never be an identifier issued for a
/// public key, written out here for the same reason.
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

/// `PUT /2020-05-31/public-key/{Id}/config` with the `PublicKeyConfig` document `body`,
/// under the version token `if_match`.
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

#[tokio::test]
async fn one_code_carries_both_sentences_over_every_public_key_route() {
    let router = emulator();

    // The one key the account holds, plus a distribution and a key group whose ids are
    // MINTED here rather than written by hand, so those rows carry real identifiers of this
    // service's other kinds.
    let present = Key {
        case: "the one key the account holds".to_string(),
        caller_reference: "public-key-absent-id".to_string(),
        name: "public-key-absent-id".to_string(),
        encoded_key: pem(2, 'A'),
        comment: Some("the key every refusal below must leave alone".to_string()),
    };
    let created = create_key(&router, &present.xml()).await;
    assert_eq!(
        created.status, 201,
        "expected HTTP 201 from CreatePublicKey, got {} with {}",
        created.status, created.raw
    );
    let known = created.root().text_at("Id").unwrap_or_default().to_string();
    let etag = created.header("etag").to_string();

    let (distribution_id, _) = create_distribution_returning_id(
        &router,
        "a distribution whose id is of another namespace",
        &config("public-key-absent-id", "public-key-absent-id").xml(),
    )
    .await;
    let group = Group {
        case: "a key group whose id is a UUID".to_string(),
        name: "public-key-absent-id".to_string(),
        items: vec![known.clone()],
        comment: None,
    };
    let group_answer = create_group(&router, &group.xml()).await;
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

    // (case, the `Id` each operation is addressed with, the sentence the one code carries for it)
    // — a value that could be an identifier issued for a public key earns the absent-key
    // sentence, and one that could never be earns the unreadable-identifier sentence.
    let rows: Vec<(&str, String, &str)> = vec![
        (
            "a live distribution id",
            distribution_id,
            UNREADABLE_IDENTIFIER,
        ),
        (
            "a live key group id, drawn as a UUID",
            group_id,
            NO_SUCH_PUBLIC_KEY,
        ),
        (
            "a field-level encryption profile id",
            format!("{FIELD_LEVEL_ENCRYPTION_PROFILE_ID_PREFIX}6AAAAAAAAAAAA"),
            UNREADABLE_IDENTIFIER,
        ),
        (
            "a field-level encryption configuration id",
            format!("{FIELD_LEVEL_ENCRYPTION_CONFIG_ID_PREFIX}7AAAAAAAAAAAA"),
            UNREADABLE_IDENTIFIER,
        ),
        (
            "the public key prefix with too short a body",
            format!("{PUBLIC_KEY_ID_PREFIX}2AAA"),
            NO_SUCH_PUBLIC_KEY,
        ),
        (
            "the public key prefix with too long a body",
            format!("{PUBLIC_KEY_ID_PREFIX}2AAAAAAAAAAAAAAAAAAA"),
            UNREADABLE_IDENTIFIER,
        ),
        (
            "a public key id of this namespace that was never minted",
            "K2NEVERMINTED1".to_string(),
            NO_SUCH_PUBLIC_KEY,
        ),
        (
            "the stored id in lower case",
            known.to_lowercase(),
            UNREADABLE_IDENTIFIER,
        ),
        (
            "the stored id with one character appended",
            format!("{known}9"),
            UNREADABLE_IDENTIFIER,
        ),
        (
            "the caller reference rather than the id",
            present.caller_reference.clone(),
            UNREADABLE_IDENTIFIER,
        ),
        ("a bare digit", "0".to_string(), NO_SUCH_PUBLIC_KEY),
        (
            "a word of no CloudFront id shape at all",
            "..".to_string(),
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

    // The control: the key the account really holds still answers, under the version token
    // its create handed out, so not one of those refusals touched the record.
    let still = get_key(&router, &known).await;
    assert_eq!(
        still.status, 200,
        "expected the stored key to still answer, got {} with {}",
        still.status, still.raw
    );
    assert_eq!(
        still.header("etag"),
        etag,
        "expected the stored key to keep its version token, got {:?}",
        still.header("etag")
    );
}
