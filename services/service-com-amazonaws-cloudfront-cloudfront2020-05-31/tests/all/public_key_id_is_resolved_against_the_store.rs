// SPDX-License-Identifier: CC0-1.0

//! What the public key surface READS its `Id` as, end to end over HTTP.
//!
//! RULE under test: the operations that address a public key by `Id` — `GetPublicKey`,
//! `GetPublicKeyConfig`, `UpdatePublicKey` and `DeletePublicKey`
//! (`smithy_query(query="operation",operation="GetPublicKey,GetPublicKeyConfig,UpdatePublicKey,DeletePublicKey")`)
//! — model one 404 error, `NoSuchPublicKey`, so one code and one status cover both ways an
//! `Id` can fail to name a key and the MESSAGE is what tells them apart: a value outside the
//! shape CloudFront mints public key ids in is the identifier the service cannot READ, while
//! a value of that shape the store holds no key for is the key that is ABSENT. Neither
//! sentence echoes the value submitted.
//!
//! The rows span what the rule generalises over — a value's RELATION to this service's
//! identifier namespaces, on both sides of the split. The point this suite adds to the
//! sibling table in [`crate::refusal_message_fidelity`] is that the values it drives are ids
//! that really do resolve: the key group id below names a key group `GetKeyGroup` answers 200
//! for, and the public key id below names the key `GetPublicKey` answers 200 for, so each read
//! is shown to resolve its OWN kind's id and to refuse every other.
//!
//! The wire harness and the create helpers are the crate's own.

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::{
    FIELD_LEVEL_ENCRYPTION_CONFIG_ID_PREFIX, FIELD_LEVEL_ENCRYPTION_PROFILE_ID_PREFIX,
    PUBLIC_KEY_ID_PREFIX,
};

use crate::distribution_lifecycle::{emulator, is_minted_uuid};
use crate::key_group_lifecycle::{create_group, get_group, Group};
use crate::public_key_lifecycle::{create_key, get_key, pem, Key};

/// The sentence `NoSuchPublicKey` carries for an `Id` of a public key's own shape the store
/// holds no key for, spelled out as a literal rather than read off the crate's producer, so
/// the WORDING is what these rows are held to.
const NO_SUCH_PUBLIC_KEY: &str = "The specified CloudFront public key does not exist.";

/// The sentence the same code carries for an `Id` outside that shape: the identifier the
/// service cannot read, named by the member that carried it.
const ID_FORMAT_INVALID: &str = "Id format is invalid";

#[tokio::test]
async fn a_public_key_id_outside_its_shape_is_unreadable_and_one_inside_it_is_absent() {
    let router = emulator();

    // The one public key the account holds, and the one key group: both ids are MINTED here
    // rather than written by hand, so the values below are real identifiers of this service.
    let key = Key {
        case: "the one key the account holds".to_string(),
        caller_reference: "public-key-id-resolution".to_string(),
        name: "public-key-id-resolution".to_string(),
        encoded_key: pem(3, 'K'),
        comment: None,
    };
    let created = create_key(&router, &key.xml()).await;
    assert_eq!(
        created.status, 201,
        "expected HTTP 201 from CreatePublicKey, got {} with {}",
        created.status, created.raw
    );
    let key_id = created.root().text_at("Id").unwrap_or_default().to_string();

    let group = Group {
        case: "the one key group the account holds".to_string(),
        name: "public-key-id-resolution".to_string(),
        items: vec![key_id.clone()],
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
    assert!(
        is_minted_uuid(&group_id),
        "expected a key group id drawn as a UUID, got {group_id:?}"
    );

    // Each kind's own read resolves its own id: the ids below are not values this service
    // could never have issued, they are ids of ANOTHER of its kinds.
    let group_read = get_group(&router, &group_id).await;
    assert_eq!(
        group_read.status, 200,
        "expected the key group id {group_id:?} to resolve through GetKeyGroup, got {} with {}",
        group_read.status, group_read.raw
    );
    let key_read = get_key(&router, &key_id).await;
    assert_eq!(
        key_read.status, 200,
        "expected the public key id {key_id:?} to resolve through GetPublicKey, got {} with {}",
        key_read.status, key_read.raw
    );

    // (case, the `Id` the read is addressed with, the sentence it is refused with) — a live
    // id of another kind, each other namespace this service mints ids in, a value in the
    // public key namespace of no minted id's width, and ids that could have been public keys
    // CloudFront issued.
    let rows: Vec<(&str, String, &str)> = vec![
        (
            "a key group id that really names a key group",
            group_id,
            ID_FORMAT_INVALID,
        ),
        (
            "the public key prefix with a body of no minted id's width",
            format!("{PUBLIC_KEY_ID_PREFIX}2AA"),
            ID_FORMAT_INVALID,
        ),
        (
            "a field-level encryption profile id",
            format!("{FIELD_LEVEL_ENCRYPTION_PROFILE_ID_PREFIX}4AAAAAAAAAAAA"),
            ID_FORMAT_INVALID,
        ),
        (
            "a field-level encryption configuration id",
            format!("{FIELD_LEVEL_ENCRYPTION_CONFIG_ID_PREFIX}5AAAAAAAAAAAA"),
            ID_FORMAT_INVALID,
        ),
        (
            "a public key id of this namespace that was never minted",
            "K2NEVERMINTED1".to_string(),
            NO_SUCH_PUBLIC_KEY,
        ),
        (
            "another public key id of this namespace that was never minted",
            format!("{PUBLIC_KEY_ID_PREFIX}3ZZZZZZZZZZZZ"),
            NO_SUCH_PUBLIC_KEY,
        ),
        (
            "the stored id in lower case",
            key_id.to_lowercase(),
            ID_FORMAT_INVALID,
        ),
        (
            "the caller reference rather than the id",
            key.caller_reference.clone(),
            ID_FORMAT_INVALID,
        ),
        (
            "a word of no CloudFront id shape at all",
            "..".to_string(),
            ID_FORMAT_INVALID,
        ),
    ];

    for (case, id, sentence) in &rows {
        let answer = get_key(&router, id).await;
        assert_eq!(
            answer.status, 404,
            "case {case:?} (Id {id:?}): expected HTTP 404, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "NoSuchPublicKey",
            "case {case:?} (Id {id:?}): expected NoSuchPublicKey, got {}",
            answer.raw
        );
        let message = answer.root().find_text("Message").unwrap_or_default();
        assert_eq!(
            message, *sentence,
            "case {case:?} (Id {id:?}): expected {sentence:?}, got {message:?}"
        );
        assert!(
            !message.contains(id.as_str()),
            "case {case:?}: the refusal must not echo the Id, got {message:?}"
        );
    }

    // The key the account really holds still answers, and so does the key group: not one of
    // those refusals touched either record.
    let still = get_key(&router, &key_id).await;
    assert_eq!(
        still.status, 200,
        "expected the stored key to still answer, got {} with {}",
        still.status, still.raw
    );
}
