// SPDX-License-Identifier: CC0-1.0

//! What a stored field-level encryption configuration reports for the flag that decides a
//! request naming a query argument profile it cannot resolve, end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing, restXml
//! (de)serialization, the handler, the persisted state and serialization — the same path a
//! client takes.
//!
//! RULE: `QueryArgProfileConfig$ForwardWhenQueryArgProfileIsUnknown` is "Flag to set if you
//! want a request to be forwarded to the origin even if the profile specified by the
//! field-level encryption query argument, fle-profile, is unknown"
//! (`smithy_query(query="shape",shape="QueryArgProfileConfig")`), a REQUIRED boolean the model
//! gives no default and no server-side override: the configuration is stored with the setting
//! its request submitted for it, whatever its `QueryArgProfiles` map, so a request submitting
//! the off position reads back off.
//! `ContentTypeProfileConfig$ForwardWhenContentTypeIsUnknown` is the other section's own
//! member and is stored exactly as submitted too, and a configuration that states no content
//! type section at all carries the mapping the service supplies for it.
//!
//! The rule holds of the STORED configuration, so every reader reports it: the create's own
//! answer, `GetFieldLevelEncryption`, `GetFieldLevelEncryptionConfig`, the listing's summary,
//! and the answer of an `UpdateFieldLevelEncryptionConfig` that submits the same flag again —
//! and a client that reads the configuration back and puts it straight back sees no drift.
//!
//! The rows differ along what the rule generalises over: the flag the request submits (both
//! settings), how many query arguments the mapping maps (none, one, several), whether the
//! wrapper is stated at all, and the content type section beside it (absent, mapping nothing,
//! mapping something, under both settings of its own flag).
//!
//! The harness (the router, the request helpers, the configuration writer and the profile
//! every mapping points at) is the configuration suites', reused rather than copied.

use smithy_server_runtime::MultiServiceRouter;

use crate::distribution_lifecycle::{emulator, Answer, Node};
use crate::field_level_encryption_config_lifecycle::{
    create_config, get_config, get_config_document, Config, ContentTypeMapping, QueryArgMapping,
    Section,
};
use crate::field_level_encryption_config_update::update_config;
use crate::field_level_encryption_profile_lifecycle::store_profile;

/// One row: the configuration a client submits, and the flag the stored configuration has to
/// report for it.
struct Row {
    config: Config,
    /// What `ForwardWhenQueryArgProfileIsUnknown` must read back as.
    forwards: bool,
}

/// The configurations the rule is asserted over, each mapping pointing at the stored profile
/// `profile`.
///
/// Every row names a profile in one of its two sections, since a configuration naming none in
/// either is refused (`QueryArgProfileEmpty`, 400).
fn rows(profile: &str) -> Vec<Row> {
    let mapped = |query_arg: &str| QueryArgMapping::new(query_arg, profile);
    vec![
        Row {
            config: Config::new(
                "one query argument mapping, submitted forwarding",
                "fle-forwarding-one-true",
                None,
                Some(Section::new(true, Some(vec![mapped("fle-profile")]))),
                None,
            ),
            forwards: true,
        },
        Row {
            config: Config::new(
                "one query argument mapping, submitted NOT forwarding",
                "fle-forwarding-one-false",
                None,
                Some(Section::new(false, Some(vec![mapped("fle-profile")]))),
                None,
            ),
            forwards: false,
        },
        Row {
            config: Config::new(
                "three query argument mappings, submitted NOT forwarding",
                "fle-forwarding-many-false",
                Some("several mappings"),
                Some(Section::new(
                    false,
                    Some(vec![
                        mapped("fle-profile"),
                        mapped("alt-profile"),
                        mapped("profile&arg"),
                    ]),
                )),
                None,
            ),
            forwards: false,
        },
        Row {
            config: Config::new(
                "one query argument mapping beside a content type section that maps nothing \
                 and does not forward",
                "fle-forwarding-with-content-type",
                Some(""),
                Some(Section::new(false, Some(vec![mapped("fle-profile")]))),
                Some(Section::new(false, Some(Vec::new()))),
            ),
            forwards: false,
        },
        Row {
            config: Config::new(
                "an empty query argument mapping, submitted NOT forwarding, beside a content \
                 type mapping naming the profile",
                "fle-forwarding-empty-false",
                None,
                Some(Section::new(false, Some(Vec::new()))),
                Some(Section::new(
                    true,
                    Some(vec![ContentTypeMapping::new("text/html", Some(profile))]),
                )),
            ),
            forwards: false,
        },
        Row {
            config: Config::new(
                "an empty query argument mapping, submitted forwarding, beside a content type \
                 mapping naming the profile",
                "fle-forwarding-empty-true",
                None,
                Some(Section::new(true, Some(Vec::new()))),
                Some(Section::new(
                    false,
                    Some(vec![ContentTypeMapping::new(
                        "application/json",
                        Some(profile),
                    )]),
                )),
            ),
            forwards: true,
        },
        Row {
            config: Config::new(
                "no query argument wrapper at all, submitted NOT forwarding, beside a content \
                 type mapping naming the profile",
                "fle-forwarding-no-wrapper-false",
                None,
                Some(Section::new(false, None)),
                Some(Section::new(
                    true,
                    Some(vec![ContentTypeMapping::new("text/plain", Some(profile))]),
                )),
            ),
            forwards: false,
        },
    ]
}

/// The flag one serialised `FieldLevelEncryptionConfig` document reports, and the flag of the
/// content type section beside it.
fn flags(config: &Node) -> (Option<String>, Option<String>) {
    (
        config
            .text_at("QueryArgProfileConfig/ForwardWhenQueryArgProfileIsUnknown")
            .map(str::to_string),
        config
            .text_at("ContentTypeProfileConfig/ForwardWhenContentTypeIsUnknown")
            .map(str::to_string),
    )
}

/// Assert that the `FieldLevelEncryptionConfig` inside `answer` reports `forwards` for the
/// query argument section, and the submitted value for the content type section beside it.
fn assert_flags(answer: &Answer, row: &Row, context: &str) {
    let case = row.config.case.as_str();
    let config = answer
        .root()
        .descendant("FieldLevelEncryptionConfig")
        .unwrap_or_else(|| {
            panic!("case {case:?} ({context}): expected a configuration in {}", answer.raw)
        });
    let (query_arg, content_type) = flags(config);
    assert_eq!(
        query_arg.as_deref(),
        Some(row.forwards.to_string().as_str()),
        "case {case:?} ({context}): expected ForwardWhenQueryArgProfileIsUnknown {}, got \
         {query_arg:?} in {}",
        row.forwards,
        answer.raw
    );
    let submitted = Some(
        row.config
            .effective_content_type()
            .forward_when_unknown
            .to_string(),
    );
    assert_eq!(
        content_type, submitted,
        "case {case:?} ({context}): expected ForwardWhenContentTypeIsUnknown {submitted:?}, got \
         {content_type:?} in {}",
        answer.raw
    );
}

/// Create one row and answer the id, the version token the create handed back, and the
/// create's own answer.
async fn created(router: &MultiServiceRouter, row: &Row) -> (String, String, Answer) {
    let answer = create_config(router, &row.config.xml()).await;
    assert_eq!(
        answer.status, 201,
        "case {:?}: expected HTTP 201 from the create, got {} with {}",
        row.config.case, answer.status, answer.raw
    );
    let id = answer.root().text_at("Id").unwrap_or_default().to_string();
    assert!(
        !id.is_empty(),
        "case {:?}: the create must mint an id, got {}",
        row.config.case,
        answer.raw
    );
    let etag = answer.header("etag").to_string();
    assert!(
        !etag.is_empty(),
        "case {:?}: the create must hand back a version token, got {}",
        row.config.case,
        answer.raw
    );
    (id, etag, answer)
}

/// A configuration reports the forwarding flag its request submitted, and every reader of
/// the stored configuration reports that value.
#[tokio::test]
async fn a_configuration_is_stored_with_the_forwarding_flag_its_request_submitted() {
    let router = emulator();
    let profile = store_profile(&router, "fle-forwarding-suite-profile").await;

    for row in rows(&profile) {
        let (id, _, create) = created(&router, &row).await;
        assert_flags(&create, &row, "the create's answer");

        let read = get_config(&router, &id).await;
        assert_eq!(
            read.status, 200,
            "case {:?}: expected HTTP 200 from the read, got {} with {}",
            row.config.case, read.status, read.raw
        );
        assert_flags(&read, &row, "GetFieldLevelEncryption");

        let document = get_config_document(&router, &id).await;
        assert_eq!(
            document.status, 200,
            "case {:?}: expected HTTP 200 from the configuration read, got {} with {}",
            row.config.case, document.status, document.raw
        );
        let (query_arg, _) = flags(document.root());
        assert_eq!(
            query_arg.as_deref(),
            Some(row.forwards.to_string().as_str()),
            "case {:?} (GetFieldLevelEncryptionConfig): expected \
             ForwardWhenQueryArgProfileIsUnknown {}, got {query_arg:?} in {}",
            row.config.case,
            row.forwards,
            document.raw
        );
    }
}

/// The rule holds of the UPDATE too — the write that replaces a stored configuration stores
/// the same flag the create would have — and the readback is a FIXED POINT: a client that
/// submits the document it read back is stored with exactly the same flag again.
#[tokio::test]
async fn an_update_stores_the_same_flag_and_the_readback_is_a_fixed_point() {
    let router = emulator();
    let profile = store_profile(&router, "fle-forwarding-update-profile").await;

    for row in rows(&profile) {
        let (id, etag, _) = created(&router, &row).await;

        // The same document again, through the update: the flag the update stores is the one
        // the request submitted, as on the create.
        let updated = update_config(&router, &id, &row.config.xml(), Some(&etag)).await;
        assert_eq!(
            updated.status, 200,
            "case {:?}: expected HTTP 200 from the update, got {} with {}",
            row.config.case, updated.status, updated.raw
        );
        assert_flags(&updated, &row, "the update's answer");
        let updated_etag = updated.header("etag").to_string();

        // And putting the flag the READ reported straight back stores the same value, so a
        // read-modify-write round trip converges.
        let back = Config {
            query_arg: row.config.query_arg.clone().map(|mut section| {
                section.forward_when_unknown = row.forwards;
                section
            }),
            ..row.config.clone()
        };
        let again = update_config(&router, &id, &back.xml(), Some(&updated_etag)).await;
        assert_eq!(
            again.status, 200,
            "case {:?}: expected HTTP 200 writing back what the read reported, got {} with {}",
            row.config.case, again.status, again.raw
        );
        assert_flags(&again, &row, "the write-back's answer");

        let read = get_config(&router, &id).await;
        assert_flags(&read, &row, "the read after the write-back");
    }
}
