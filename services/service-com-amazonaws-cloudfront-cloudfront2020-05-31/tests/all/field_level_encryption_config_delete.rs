// SPDX-License-Identifier: CC0-1.0

//! `DeleteFieldLevelEncryptionConfig`, end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing, restXml
//! (de)serialization, the handler, the persisted state and serialization — the same path a
//! client takes. The rules under test are the ones CloudFront's delete states:
//!
//! * the delete REMOVES the stored record every read answered from, so the id afterwards
//!   names no configuration (`NoSuchFieldLevelEncryptionConfig`, HTTP 404) — through the
//!   configuration reader as well as the configuration-and-metadata one — and the caller
//!   reference it held is free for a later create to claim under a new id;
//! * the delete is gated on the configuration's own version token: an absent `If-Match` and
//!   one that is no version token at all are `InvalidIfMatchVersion` (HTTP 400), a
//!   well-formed token that is not this configuration's current one is `PreconditionFailed`
//!   (HTTP 412), and in either case the configuration stays exactly where it was;
//! * a cache behaviour names the configuration whose fields it encrypts through its
//!   `FieldLevelEncryptionId`, so a configuration any stored behaviour — the default one or
//!   any ordered one — still names is `FieldLevelEncryptionConfigInUse` (HTTP 409) and stays
//!   readable, and once every behaviour has let go the same delete lands.
//!
//! The rows span what those rules generalise over: every configuration of
//! [`crate::field_level_encryption_config_lifecycle::table`] (which mapping sections the
//! configuration carries at all, whether a section's wrapper is present and how many mappings
//! it holds, both settings of each forwarding flag, whether a content type mapping names a
//! profile, and comments that are absent, empty, plain ASCII, XML metacharacters or exactly on
//! the published bound), every form an `If-Match` takes (absent, empty, bare, quoted, weak,
//! lower case, a token of another resource, a value that is no token), which cache behaviour
//! names the configuration, and ids of several shapes the store holds nothing for.
//!
//! The wire harness is the one [`crate::distribution_lifecycle`] builds, the configuration
//! create and the two readers are
//! [`crate::field_level_encryption_config_lifecycle`]'s, and the profile every mapping points
//! at is stored through the profile suite's own create path — reused rather than copied.

use smithy_server_runtime::MultiServiceRouter;

use crate::distribution_lifecycle::{
    call, create_distribution, emulator, get_distribution, is_distribution_id, is_version_token,
    update_distribution, Answer, API_VERSION,
};
use crate::field_level_encryption_config_lifecycle::{
    create_config, get_config, get_config_document, is_config_id, simple, table, Config,
};
use crate::field_level_encryption_profile_lifecycle::store_profile;

/// The sentences CloudFront sends on the wire for these refusals, spelled out here rather
/// than taken from the crate, so a reworded refusal fails this suite.
const NO_SUCH_CONFIG_MESSAGE: &str = "The specified field level encryption config does not exist.";
const ID_FORMAT_INVALID_MESSAGE: &str = "Id format is invalid";
const CONFIG_IN_USE_MESSAGE: &str =
    "The field level encryption config is currently associated with a cache behavior. Please \
     disassociate the config before deleting.";

/// How one row below spells a version token in the `If-Match` header: bare, as CloudFront's
/// own `ETag` header reports it, or quoted or weak, as HTTP spells an entity tag.
type Spelling = fn(&str) -> String;

/// `DELETE /2020-05-31/field-level-encryption/{Id}` under the version token `if_match` — the
/// route and the header the model binds this operation to (`IfMatch` travels in `If-Match`).
pub(crate) async fn delete_config(
    router: &MultiServiceRouter,
    config_id: &str,
    if_match: Option<&str>,
) -> Answer {
    call(
        router,
        "DELETE",
        &format!("/{API_VERSION}/field-level-encryption/{config_id}"),
        None,
        if_match,
    )
    .await
}

/// Create `config`, returning its minted id and its current version token.
async fn create(router: &MultiServiceRouter, config: &Config) -> (String, String) {
    let answer = create_config(router, &config.xml()).await;
    assert_eq!(
        answer.status, 201,
        "case {:?}: expected HTTP 201 from CreateFieldLevelEncryptionConfig, got {} with {}",
        config.case, answer.status, answer.raw
    );
    let id = answer.root().text_at("Id").unwrap_or_default().to_string();
    let etag = answer.header("etag").to_string();
    assert!(
        is_config_id(&id),
        "case {:?}: expected a minted configuration id, got {id:?}",
        config.case
    );
    assert!(
        is_version_token(&etag),
        "case {:?}: expected a minted version token, got {etag:?}",
        config.case
    );
    (id, etag)
}

/// Assert that `id` names no field-level encryption configuration any more: both readers
/// answer `NoSuchFieldLevelEncryptionConfig` with the modelled sentence.
async fn assert_gone(router: &MultiServiceRouter, id: &str, case: &str) {
    for (reader, answer) in [
        ("GetFieldLevelEncryption", get_config(router, id).await),
        (
            "GetFieldLevelEncryptionConfig",
            get_config_document(router, id).await,
        ),
    ] {
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected HTTP 404 from {reader} after the delete, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "NoSuchFieldLevelEncryptionConfig",
            "case {case:?}: expected NoSuchFieldLevelEncryptionConfig from {reader} after the \
             delete, got {}",
            answer.raw
        );
        let message = answer.root().find_text("Message").unwrap_or_default();
        assert_eq!(
            message, NO_SUCH_CONFIG_MESSAGE,
            "case {case:?}: expected the modelled missing-configuration sentence from {reader}, \
             got {message:?}"
        );
    }
}

/// The `DistributionConfig` a case below submits: one origin, a default cache behaviour and
/// the ordered cache behaviours the case asks for, each naming `config_id` when the case says
/// that behaviour uses field-level encryption.
fn distribution_config_xml(
    caller_reference: &str,
    default_uses: bool,
    ordered: &[bool],
    config_id: &str,
) -> String {
    let attachment = |uses: bool| {
        if uses {
            format!("<FieldLevelEncryptionId>{config_id}</FieldLevelEncryptionId>")
        } else {
            String::new()
        }
    };
    let mut behaviors = format!("<Quantity>{}</Quantity>", ordered.len());
    if !ordered.is_empty() {
        behaviors.push_str("<Items>");
        for (position, uses) in ordered.iter().enumerate() {
            behaviors.push_str(&format!(
                "<CacheBehavior><PathPattern>/path-{position}/*</PathPattern>\
                 <TargetOriginId>primary</TargetOriginId>\
                 <ViewerProtocolPolicy>allow-all</ViewerProtocolPolicy>{}\
                 </CacheBehavior>",
                attachment(*uses)
            ));
        }
        behaviors.push_str("</Items>");
    }
    format!(
        "<DistributionConfig xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
         <CallerReference>{caller_reference}</CallerReference>\
         <Origins><Quantity>1</Quantity><Items>\
         <Origin><Id>primary</Id><DomainName>a.example.com</DomainName></Origin>\
         </Items></Origins>\
         <DefaultCacheBehavior><TargetOriginId>primary</TargetOriginId>\
         <ViewerProtocolPolicy>allow-all</ViewerProtocolPolicy>{}\
         </DefaultCacheBehavior>\
         <CacheBehaviors>{behaviors}</CacheBehaviors>\
         <Comment>{caller_reference}</Comment>\
         <PriceClass>PriceClass_All</PriceClass>\
         <Enabled>true</Enabled>\
         </DistributionConfig>",
        attachment(default_uses)
    )
}

/// Every configuration the create accepts is removed by its delete under its version token:
/// the delete answers HTTP 204 with no body, both readers then report the id as naming
/// nothing, deleting it again is `NoSuchFieldLevelEncryptionConfig`, and the caller reference
/// it held is free for a NEW configuration under a new id.
#[tokio::test]
async fn every_stored_configuration_is_removed_by_its_delete() {
    let router = emulator();
    let profile = store_profile(&router, "fle-config-delete-profile").await;

    for row in table(&profile) {
        let (id, etag) = create(&router, &row).await;

        // Round trip: the configuration is readable before the delete, so the 404 after it is
        // the delete's doing and not a configuration that was never stored.
        let fetched = get_config(&router, &id).await;
        assert_eq!(
            fetched.status, 200,
            "case {:?}: expected HTTP 200 from GetFieldLevelEncryption before the delete, got {} \
             with {}",
            row.case, fetched.status, fetched.raw
        );

        let deleted = delete_config(&router, &id, Some(&etag)).await;
        assert_eq!(
            deleted.status, 204,
            "case {:?}: expected HTTP 204 from DeleteFieldLevelEncryptionConfig, got {} with {}",
            row.case, deleted.status, deleted.raw
        );
        assert!(
            deleted.raw.trim().is_empty(),
            "case {:?}: expected no body from a 204, got {}",
            row.case,
            deleted.raw
        );

        assert_gone(&router, &id, &row.case).await;
        let again = delete_config(&router, &id, Some(&etag)).await;
        assert_eq!(
            again.status, 404,
            "case {:?}: expected HTTP 404 from a second delete, got {} with {}",
            row.case, again.status, again.raw
        );
        assert_eq!(
            again.error_code(),
            "NoSuchFieldLevelEncryptionConfig",
            "case {:?}: expected NoSuchFieldLevelEncryptionConfig from a second delete, got {}",
            row.case,
            again.raw
        );

        // The caller reference the deleted configuration held identifies nothing any more, so
        // a create under it mints a SECOND configuration rather than reporting the one that is
        // gone.
        let (reused_id, reused_etag) = create(&router, &row).await;
        assert_ne!(
            reused_id, id,
            "case {:?}: expected a freshly minted id for the configuration recreated under the \
             freed caller reference, got the deleted one {id:?}",
            row.case
        );

        // Leave the store as this row found it, so the next row's reference is the only one in
        // play.
        let cleanup = delete_config(&router, &reused_id, Some(&reused_etag)).await;
        assert_eq!(
            cleanup.status, 204,
            "case {:?}: expected HTTP 204 deleting the recreated configuration, got {} with {}",
            row.case, cleanup.status, cleanup.raw
        );
    }
}

/// The delete is gated on the configuration's OWN version token.
///
/// The rows span every form an `If-Match` takes: absent, empty, a value that is no version
/// token at all, a token spelled in lower case, a well-formed token belonging to another
/// configuration, and the current token bare, quoted and weak — the three ways HTTP and
/// CloudFront's own `ETag` header spell one.
#[tokio::test]
async fn the_version_token_gates_the_delete() {
    let router = emulator();
    let profile = store_profile(&router, "fle-config-version-gate-profile").await;

    let other = simple(
        "another configuration, whose token is not the subject's",
        "fle-config-version-gate-other",
        &profile,
    );
    let (_, other_etag) = create(&router, &other).await;

    let subject = simple(
        "the configuration under the version gate",
        "fle-config-version-gate",
        &profile,
    );

    // Each refusal is asserted on its own freshly created configuration, and the configuration
    // is read back after it: a refused delete removes nothing.
    let refusals: &[(&str, Option<&str>, u16, &str)] = &[
        ("no If-Match at all", None, 400, "InvalidIfMatchVersion"),
        (
            "an If-Match that is no version token",
            Some("not-a-version"),
            400,
            "InvalidIfMatchVersion",
        ),
        ("an empty If-Match", Some(""), 400, "InvalidIfMatchVersion"),
        (
            "an If-Match carrying a lowercase token",
            Some("e1stale1version"),
            400,
            "InvalidIfMatchVersion",
        ),
        (
            "a version token that is another configuration's",
            Some(other_etag.as_str()),
            412,
            "PreconditionFailed",
        ),
    ];

    for (case, if_match, status, code) in refusals {
        let (id, etag) = create(&router, &subject).await;
        let answer = delete_config(&router, &id, *if_match).await;
        assert_eq!(
            answer.status, *status,
            "case {case:?}: expected HTTP {status}, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            *code,
            "case {case:?}: expected {code}, got {}",
            answer.raw
        );
        let fetched = get_config(&router, &id).await;
        assert_eq!(
            fetched.status, 200,
            "case {case:?}: expected the refused delete to leave the configuration readable, got \
             {} with {}",
            fetched.status, fetched.raw
        );
        assert_eq!(
            fetched.header("etag"),
            etag,
            "case {case:?}: expected the refused delete to leave the version token alone, got {:?}",
            fetched.header("etag")
        );

        // Clear the way for the next row, which creates a configuration under the same caller
        // reference.
        let cleanup = delete_config(&router, &id, Some(&etag)).await;
        assert_eq!(
            cleanup.status, 204,
            "case {case:?}: expected the current token to delete the configuration, got {} with \
             {}",
            cleanup.status, cleanup.raw
        );
    }

    // The current token, spelled each of the three admissible ways, deletes the configuration.
    let spellings: &[(&str, Spelling)] = &[
        ("the current token, bare", |etag| etag.to_string()),
        ("the current token, quoted", |etag| format!("\"{etag}\"")),
        ("the current token, weak", |etag| format!("W/\"{etag}\"")),
    ];
    for (case, spell) in spellings {
        let (id, etag) = create(&router, &subject).await;
        let answer = delete_config(&router, &id, Some(&spell(&etag))).await;
        assert_eq!(
            answer.status, 204,
            "case {case:?}: expected HTTP 204, got {} with {}",
            answer.status, answer.raw
        );
        assert_gone(&router, &id, case).await;
    }
}

/// A configuration a stored cache behaviour names cannot be deleted, and once every behaviour
/// has let go the same delete lands — the stored `FieldLevelEncryptionId` is the gate.
///
/// The rows cover which behaviour names the configuration: the default cache behaviour, one
/// ordered behaviour among several, and both at once. A second configuration no behaviour
/// names is deleted in each case, so the gate is shown to select on the attachment rather
/// than on the store holding a distribution at all.
#[tokio::test]
async fn a_configuration_a_cache_behaviour_names_cannot_be_deleted() {
    let attachments: &[(&str, bool, &[bool], &[bool])] = &[
        (
            "the default cache behaviour names the configuration",
            true,
            &[],
            &[],
        ),
        (
            "a later ordered cache behaviour names the configuration",
            false,
            &[false, true],
            &[false, false],
        ),
        (
            "the default behaviour and an ordered one both name the configuration",
            true,
            &[true],
            &[false],
        ),
    ];

    for (case, default_uses, ordered, detached) in attachments {
        let router = emulator();
        let profile = store_profile(&router, "fle-config-in-use-profile").await;

        let subject = simple(
            "the configuration under the attachment gate",
            "fle-config-in-use",
            &profile,
        );
        let (config_id, config_etag) = create(&router, &subject).await;

        // A second configuration nothing names: the distribution below leaves it alone, so it
        // is deletable throughout.
        let spare = simple(
            "a configuration no cache behaviour names",
            "fle-config-not-in-use",
            &profile,
        );
        let (spare_id, spare_etag) = create(&router, &spare).await;

        let created = create_distribution(
            &router,
            &distribution_config_xml("ref-fle-delete-gate", *default_uses, ordered, &config_id),
        )
        .await;
        assert_eq!(
            created.status, 201,
            "case {case:?}: expected HTTP 201 from CreateDistribution, got {} with {}",
            created.status, created.raw
        );
        let distribution_id = created.root().text_at("Id").unwrap_or_default().to_string();
        assert!(
            is_distribution_id(&distribution_id),
            "case {case:?}: expected a minted distribution id, got {distribution_id:?}"
        );
        let distribution_etag = created.header("etag").to_string();

        let refused = delete_config(&router, &config_id, Some(&config_etag)).await;
        assert_eq!(
            refused.status, 409,
            "case {case:?}: expected HTTP 409 while a behaviour names the configuration, got {} \
             with {}",
            refused.status, refused.raw
        );
        assert_eq!(
            refused.error_code(),
            "FieldLevelEncryptionConfigInUse",
            "case {case:?}: expected FieldLevelEncryptionConfigInUse while in use, got {}",
            refused.raw
        );
        assert_eq!(
            refused.root().find_text("Message").unwrap_or_default(),
            CONFIG_IN_USE_MESSAGE,
            "case {case:?}: expected the modelled in-use sentence, got {}",
            refused.raw
        );
        assert_eq!(
            get_config(&router, &config_id).await.status,
            200,
            "case {case:?}: expected the refused delete to leave the configuration readable"
        );

        // The configuration nothing names is removed while the distribution stands, so the
        // refusal above is about the attachment and not about the store.
        let spare_deleted = delete_config(&router, &spare_id, Some(&spare_etag)).await;
        assert_eq!(
            spare_deleted.status, 204,
            "case {case:?}: expected HTTP 204 deleting the configuration nothing names, got {} \
             with {}",
            spare_deleted.status, spare_deleted.raw
        );
        assert_gone(&router, &spare_id, "a configuration no cache behaviour names").await;

        // A version that is not the configuration's is refused on its own terms, before
        // anything about the attachment.
        for (description, if_match, status, code) in [
            ("no If-Match at all", None, 400, "InvalidIfMatchVersion"),
            (
                "a version token that is the distribution's",
                Some(distribution_etag.as_str()),
                412,
                "PreconditionFailed",
            ),
        ] {
            let answer = delete_config(&router, &config_id, if_match).await;
            assert_eq!(
                answer.status, status,
                "case {case:?} with {description}: expected HTTP {status}, got {} with {}",
                answer.status, answer.raw
            );
            assert_eq!(
                answer.error_code(),
                code,
                "case {case:?} with {description}: expected {code}, got {}",
                answer.raw
            );
        }

        // Take the configuration out of every cache behaviour, and the same delete lands.
        let updated = update_distribution(
            &router,
            &distribution_id,
            &distribution_config_xml("ref-fle-delete-gate", false, detached, &config_id),
            Some(&distribution_etag),
        )
        .await;
        assert_eq!(
            updated.status, 200,
            "case {case:?}: expected HTTP 200 from UpdateDistribution, got {} with {}",
            updated.status, updated.raw
        );

        let deleted = delete_config(&router, &config_id, Some(&config_etag)).await;
        assert_eq!(
            deleted.status, 204,
            "case {case:?}: expected HTTP 204 once no behaviour names the configuration, got {} \
             with {}",
            deleted.status, deleted.raw
        );
        assert_gone(&router, &config_id, case).await;

        // The distribution itself outlives the configuration's delete.
        assert_eq!(
            get_distribution(&router, &distribution_id).await.status,
            200,
            "case {case:?}: expected the distribution to outlive the configuration"
        );
    }
}

/// An id the store holds no field-level encryption configuration for is
/// `NoSuchFieldLevelEncryptionConfig`, whatever shape the id takes — never a 204, which would
/// claim something was removed. The MESSAGE says which condition the delete found: the
/// identifier's shape is judged before the store is consulted, so a value outside the shape a
/// configuration id takes is the identifier CloudFront cannot read, while a well-formed
/// configuration id the store holds nothing for is the absent configuration.
#[tokio::test]
async fn an_id_no_configuration_holds_is_refused() {
    let router = emulator();
    let profile = store_profile(&router, "fle-config-absent-id-profile").await;
    let row = simple(
        "the only configuration the account holds",
        "fle-config-the-only-one",
        &profile,
    );
    let (known, etag) = create(&router, &row).await;

    let mut flipped: Vec<char> = known.chars().collect();
    let last = flipped.len() - 1;
    flipped[last] = if flipped[last] == 'A' { 'B' } else { 'A' };
    let absent: [(&str, String, &str); 5] = [
        (
            "a configuration id never minted",
            "C0000000000000".to_string(),
            NO_SUCH_CONFIG_MESSAGE,
        ),
        (
            "a known id with one character changed",
            flipped.into_iter().collect(),
            NO_SUCH_CONFIG_MESSAGE,
        ),
        (
            "a known id in lower case",
            known.to_lowercase(),
            ID_FORMAT_INVALID_MESSAGE,
        ),
        (
            "a token that is no configuration id",
            "not-a-config-id".to_string(),
            ID_FORMAT_INVALID_MESSAGE,
        ),
        (
            "the caller reference rather than the id",
            "fle-config-the-only-one".to_string(),
            ID_FORMAT_INVALID_MESSAGE,
        ),
    ];

    for (case, candidate, expected) in absent {
        let answer = delete_config(&router, &candidate, Some(&etag)).await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected HTTP 404 for {candidate:?}, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "NoSuchFieldLevelEncryptionConfig",
            "case {case:?}: expected NoSuchFieldLevelEncryptionConfig for {candidate:?}, got {}",
            answer.raw
        );
        let message = answer.root().find_text("Message").unwrap_or_default();
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
    }

    // None of those refusals touched the configuration the account really holds.
    let fetched = get_config(&router, &known).await;
    assert_eq!(
        fetched.status, 200,
        "case the only configuration the account holds: expected it to survive every refusal, got \
         {} with {}",
        fetched.status, fetched.raw
    );
}
