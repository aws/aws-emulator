// SPDX-License-Identifier: CC0-1.0

//! Both forwarding flags of a field-level encryption configuration ROUND TRIP: the value the
//! request submitted is the value every reader reports, and flipping it is a change the store
//! keeps. End to end over HTTP, so each case exercises routing, restXml (de)serialization,
//! the handler, the persisted state and serialization — the same path a client takes.
//!
//! RULE: `QueryArgProfileConfig$ForwardWhenQueryArgProfileIsUnknown` ("Flag to set if you
//! want a request to be forwarded to the origin even if the profile specified by the
//! field-level encryption query argument, fle-profile, is unknown") and
//! `ContentTypeProfileConfig$ForwardWhenContentTypeIsUnknown` ("what to do when an unknown
//! content type is provided for the profile")
//! (`smithy_query(query="shape",shape="QueryArgProfileConfig,ContentTypeProfileConfig")`) are
//! REQUIRED members of the section that carries them, so a request that states a section
//! always states its flag and the service decides nothing about it: the stored configuration
//! carries exactly what was submitted, whatever the mapping beside the flag maps. The service
//! supplies a value only where the request states no section at all
//! (`server_supplied_config_members.rs` states that rule).
//!
//! The rows are PAIRS that differ in nothing but the flag: the same mapping is submitted
//! twice, once under each setting, and the two stored configurations must report different
//! flags. A service that derived either flag from its own mapping — or from anything else in
//! the configuration — would report one value for both rows of a pair and fail here, however
//! the single-row suites happened to be written.
//!
//! The rows differ along what the rule generalises over: how many query arguments the mapping
//! maps (none, one, three), how many content types the mapping beside it maps (one, two),
//! whether a content type mapping names a profile, and both settings of both flags. Each pair
//! is asserted on the create's own answer, on `GetFieldLevelEncryption`, on
//! `GetFieldLevelEncryptionConfig`, on the summary `ListFieldLevelEncryptionConfigs` carries,
//! and on the answer of an `UpdateFieldLevelEncryptionConfig` that submits the OTHER setting
//! of the flag — which is what shows the flag is stored rather than computed.
//!
//! The harness (the router, the request helpers, the configuration writer and the profile
//! every mapping points at) is the configuration suites', reused rather than copied.

use smithy_server_runtime::MultiServiceRouter;

use crate::distribution_lifecycle::{emulator, Answer, Node};
use crate::field_level_encryption_config_lifecycle::{
    create_config, get_config, get_config_document, Config, ContentTypeMapping, QueryArgMapping,
    Section,
};
use crate::field_level_encryption_config_list::list_configs;
use crate::field_level_encryption_config_update::update_config;
use crate::field_level_encryption_profile_lifecycle::store_profile;

/// One case: the mappings both rows of the pair carry, named for what makes it a case of its
/// own.
struct Mappings {
    case: &'static str,
    /// The query arguments the query arg-profile mapping maps, each to the stored profile.
    query_args: &'static [&'static str],
    /// The content types the content type-profile mapping maps, each with whether it names
    /// the stored profile.
    content_types: &'static [(&'static str, bool)],
}

/// The mappings the rule is asserted over.
///
/// Every case maps at least one content type to the profile, since a configuration that names
/// a profile in neither section is refused (`QueryArgProfileEmpty`, 400) and so has no stored
/// flag to report.
const CASES: &[Mappings] = &[
    Mappings {
        case: "a mapping that maps no query argument at all",
        query_args: &[],
        content_types: &[("application/x-www-form-urlencoded", true)],
    },
    Mappings {
        case: "a mapping that maps one query argument",
        query_args: &["fle-profile"],
        content_types: &[("application/x-www-form-urlencoded", true)],
    },
    Mappings {
        case: "a mapping that maps three query arguments",
        query_args: &["fle-profile", "alt-profile", "profile&arg"],
        content_types: &[("text/html", true), ("application/json", false)],
    },
];

/// Both settings of a required boolean, which every pair below is asserted over.
const SETTINGS: [bool; 2] = [true, false];

/// The configuration one row submits: `mappings`, with the two flags set to `query_arg_flag`
/// and `content_type_flag`.
fn config(
    mappings: &Mappings,
    query_arg_flag: bool,
    content_type_flag: bool,
    reference: &str,
    profile: &str,
) -> Config {
    Config::new(
        &format!(
            "{} under ForwardWhenQueryArgProfileIsUnknown {query_arg_flag} and \
             ForwardWhenContentTypeIsUnknown {content_type_flag}",
            mappings.case
        ),
        reference,
        None,
        Some(Section::new(
            query_arg_flag,
            Some(
                mappings
                    .query_args
                    .iter()
                    .map(|query_arg| QueryArgMapping::new(query_arg, profile))
                    .collect(),
            ),
        )),
        Some(Section::new(
            content_type_flag,
            Some(
                mappings
                    .content_types
                    .iter()
                    .map(|(content_type, names_profile)| {
                        ContentTypeMapping::new(content_type, names_profile.then_some(profile))
                    })
                    .collect(),
            ),
        )),
    )
}

/// The two flags one serialised `FieldLevelEncryptionConfig` — or the
/// `FieldLevelEncryptionSummary` that repeats both sections — reports.
fn flags(node: &Node) -> (Option<String>, Option<String>) {
    (
        node.text_at("QueryArgProfileConfig/ForwardWhenQueryArgProfileIsUnknown")
            .map(str::to_string),
        node.text_at("ContentTypeProfileConfig/ForwardWhenContentTypeIsUnknown")
            .map(str::to_string),
    )
}

/// Assert that the configuration inside `answer` reports exactly the two submitted flags.
fn assert_flags(answer: &Answer, case: &str, context: &str, submitted: (bool, bool)) {
    let node = answer
        .root()
        .descendant("FieldLevelEncryptionConfig")
        .unwrap_or_else(|| {
            panic!("case {case:?} ({context}): expected a configuration in {}", answer.raw)
        });
    assert_flags_on(node, case, context, submitted, &answer.raw);
}

/// Assert that `node` reports exactly the two submitted flags.
fn assert_flags_on(
    node: &Node,
    case: &str,
    context: &str,
    submitted: (bool, bool),
    raw: &str,
) {
    let expected = (
        Some(submitted.0.to_string()),
        Some(submitted.1.to_string()),
    );
    let actual = flags(node);
    assert_eq!(
        actual, expected,
        "case {case:?} ({context}): expected (ForwardWhenQueryArgProfileIsUnknown, \
         ForwardWhenContentTypeIsUnknown) {expected:?}, got {actual:?} in {raw}"
    );
}

/// The `FieldLevelEncryptionSummary` the listing carries for the configuration `id`.
fn summary_of<'a>(page: &'a Answer, id: &str, case: &str) -> &'a Node {
    page.root()
        .at("Items")
        .map(|items| items.children_named("FieldLevelEncryptionSummary"))
        .unwrap_or_default()
        .into_iter()
        .find(|summary| summary.text_at("Id") == Some(id))
        .unwrap_or_else(|| panic!("case {case:?}: no summary for {id} in {}", page.raw))
}

/// Create `config` and answer its id, the version token the create minted and its answer.
async fn created(router: &MultiServiceRouter, config: &Config) -> (String, String, Answer) {
    let answer = create_config(router, &config.xml()).await;
    assert_eq!(
        answer.status, 201,
        "case {:?}: expected HTTP 201 from the create, got {} with {}",
        config.case, answer.status, answer.raw
    );
    let id = answer.root().text_at("Id").unwrap_or_default().to_string();
    assert!(
        !id.is_empty(),
        "case {:?}: the create must mint an id, got {}",
        config.case,
        answer.raw
    );
    (id, answer.header("etag").to_string(), answer)
}

/// Every reader reports the flags the request submitted, under either setting of each and
/// whatever the mapping beside it maps.
#[tokio::test]
async fn every_reader_reports_the_submitted_forwarding_flags() {
    let router = emulator();
    let profile = store_profile(&router, "fle-flag-round-trip-profile").await;

    for (index, mappings) in CASES.iter().enumerate() {
        for query_arg_flag in SETTINGS {
            for content_type_flag in SETTINGS {
                let submitted = (query_arg_flag, content_type_flag);
                let row = config(
                    mappings,
                    query_arg_flag,
                    content_type_flag,
                    &format!("fle-flags-{index}-{query_arg_flag}-{content_type_flag}"),
                    &profile,
                );
                let case = row.case.clone();
                let (id, _etag, create) = created(&router, &row).await;
                assert_flags(&create, &case, "the create's answer", submitted);

                let read = get_config(&router, &id).await;
                assert_eq!(
                    read.status, 200,
                    "case {case:?}: expected HTTP 200 from the read, got {} with {}",
                    read.status, read.raw
                );
                assert_flags(&read, &case, "GetFieldLevelEncryption", submitted);

                let document = get_config_document(&router, &id).await;
                assert_eq!(
                    document.status, 200,
                    "case {case:?}: expected HTTP 200 from the configuration read, got {} \
                     with {}",
                    document.status, document.raw
                );
                assert_flags_on(
                    document.root(),
                    &case,
                    "GetFieldLevelEncryptionConfig",
                    submitted,
                    &document.raw,
                );

                let page = list_configs(&router, "").await;
                assert_flags_on(
                    summary_of(&page, &id, &case),
                    &case,
                    "ListFieldLevelEncryptionConfigs",
                    submitted,
                    &page.raw,
                );
            }
        }
    }
}

/// The two rows of a pair differ: an update that submits the OTHER setting of each flag,
/// leaving the mapping exactly as it was, stores that other setting — so the flag follows the
/// request and not the mapping.
#[tokio::test]
async fn flipping_a_flag_over_the_same_mapping_changes_what_is_stored() {
    let router = emulator();
    let profile = store_profile(&router, "fle-flag-flip-profile").await;

    for (index, mappings) in CASES.iter().enumerate() {
        for submitted in SETTINGS {
            let row = config(
                mappings,
                submitted,
                submitted,
                &format!("fle-flags-flip-{index}-{submitted}"),
                &profile,
            );
            let case = row.case.clone();
            let (id, etag, _create) = created(&router, &row).await;

            // The same mappings again, with both flags flipped: nothing else about the
            // configuration changes, so what the readers report afterwards can only have
            // come from the request.
            let flipped = config(
                mappings,
                !submitted,
                !submitted,
                &row.caller_reference,
                &profile,
            );
            let updated = update_config(&router, &id, &flipped.xml(), Some(&etag)).await;
            assert_eq!(
                updated.status, 200,
                "case {case:?}: expected HTTP 200 from the update, got {} with {}",
                updated.status, updated.raw
            );
            let expected = (!submitted, !submitted);
            assert_flags(&updated, &case, "the update's answer", expected);

            let read = get_config(&router, &id).await;
            assert_flags(&read, &case, "the read after the update", expected);

            let document = get_config_document(&router, &id).await;
            assert_flags_on(
                document.root(),
                &case,
                "the configuration read after the update",
                expected,
                &document.raw,
            );
        }
    }
}
