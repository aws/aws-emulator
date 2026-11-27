// SPDX-License-Identifier: CC0-1.0

//! The two mapping sections a stored field-level encryption configuration carries, end to end
//! over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing, restXml
//! (de)serialization, the handler, the persisted state and serialization — the same path a
//! client takes.
//!
//! RULES (`smithy_query(query="shape",shape="FieldLevelEncryptionConfig,QueryArgProfileConfig,ContentTypeProfileConfig,ContentTypeProfiles,ContentTypeProfile,Format")`):
//!
//! * `ContentTypeProfileConfig` is the content type-profile mapping EVERY stored
//!   configuration carries. A configuration that states one is stored with it exactly as
//!   stated — down to whether it states a `ContentTypeProfiles` wrapper at all, and to its own
//!   `ForwardWhenContentTypeIsUnknown`. A configuration that states NONE is stored with the
//!   mapping CloudFront supplies for it: field-level encryption reads the fields it encrypts
//!   out of a request BODY, and the one body layout it can read them from is the one the
//!   model's `Format` enum admits a single value for — `URLEncoded`, the layout of an
//!   `application/x-www-form-urlencoded` body — so the supplied mapping covers that one
//!   content type under that one format, names no profile of its own (`ProfileId` carried as
//!   the empty text) and does not forward a body whose content type it does not cover.
//! * `QueryArgProfileConfig$ForwardWhenQueryArgProfileIsUnknown` — "Flag to set if you want a
//!   request to be forwarded to the origin even if the profile specified by the field-level
//!   encryption query argument, fle-profile, is unknown" — is a REQUIRED member the model
//!   gives no default, so a configuration that states a query argument section is stored with
//!   the flag that section submitted, whatever its mappings map.
//!
//! Neither rule is a property of one request document's form, so the table below spans what
//! they generalise over: whether the configuration states a content type section at all,
//! whether a section states its wrapper, how many mappings a section holds (none, one, two,
//! three), both settings of each forwarding flag, whether a content type mapping names a
//! profile, and whether a comment is carried. Every row is asserted on the create's own
//! answer, on `GetFieldLevelEncryption`, on `GetFieldLevelEncryptionConfig`, on the summary
//! `ListFieldLevelEncryptionConfigs` carries and on the answer of an
//! `UpdateFieldLevelEncryptionConfig` that submits the same document again — which also shows
//! the stored form is a FIXED POINT of a read-modify-write round trip.
//!
//! The harness (the router, the request helpers, the configuration writer and the profile
//! every mapping points at) is the configuration suites', reused rather than copied; the
//! expectations below are written out row by row rather than derived from the request, so a
//! change in what the service supplies fails this suite.

use smithy_server_runtime::MultiServiceRouter;

use crate::distribution_lifecycle::{emulator, Answer, Node};
use crate::field_level_encryption_config_lifecycle::{
    create_config, get_config, get_config_document, Config, ContentTypeMapping, QueryArgMapping,
    Section,
};
use crate::field_level_encryption_config_list::list_configs;
use crate::field_level_encryption_config_update::update_config;
use crate::field_level_encryption_profile_lifecycle::store_profile;

/// The only value the model's `Format` enum admits for a content type-profile mapping.
const URL_ENCODED: &str = "URLEncoded";

/// The content type whose body layout `URLEncoded` names, and so the one the mapping
/// CloudFront supplies covers.
const FORM_CONTENT_TYPE: &str = "application/x-www-form-urlencoded";

/// The sentence CloudFront refuses a configuration that leaves it no profile to encrypt
/// under with, spelled out here rather than taken from the crate.
const QUERY_ARG_PROFILE_EMPTY: &str =
    "The query arg profile in field level encryption config cannot be empty when content \
     type profile is empty or when no default profile id is given in any content type \
     profile.";

/// One `ContentTypeProfile` the stored configuration has to report: its `ContentType` and the
/// `ProfileId` it names, which is `None` when the mapping reports no such member at all.
#[derive(Debug, Clone)]
struct ExpectedMapping {
    content_type: String,
    profile_id: Option<String>,
}

impl ExpectedMapping {
    fn new(content_type: &str, profile_id: Option<&str>) -> Self {
        Self {
            content_type: content_type.to_string(),
            profile_id: profile_id.map(str::to_string),
        }
    }
}

/// The `ContentTypeProfileConfig` one stored configuration has to report.
#[derive(Debug, Clone)]
struct ExpectedContentType {
    forward_when_unknown: bool,
    /// The mappings its wrapper carries, or `None` when the stored section states no
    /// `ContentTypeProfiles` wrapper at all.
    mappings: Option<Vec<ExpectedMapping>>,
}

/// The content type-profile mapping a configuration that states none is stored with.
fn supplied() -> ExpectedContentType {
    ExpectedContentType {
        forward_when_unknown: false,
        mappings: Some(vec![ExpectedMapping::new(FORM_CONTENT_TYPE, Some(""))]),
    }
}

/// One row: the configuration a client submits beside the two sections the stored
/// configuration has to report for it.
struct Row {
    config: Config,
    /// What `ForwardWhenQueryArgProfileIsUnknown` has to read back as.
    forwards_unknown_profile: bool,
    content_type: ExpectedContentType,
}

/// The rows, each mapping pointing at the stored profile `profile`.
fn rows(profile: &str) -> Vec<Row> {
    let mapped = |query_arg: &str| QueryArgMapping::new(query_arg, profile);
    vec![
        // A configuration that states no content type section at all, under either setting of
        // the flag it does state: the supplied mapping, and the flag it submitted.
        Row {
            config: Config::new(
                "no content type section, one query argument mapping, submitted forwarding",
                "fle-sections-supplied-one-true",
                None,
                Some(Section::new(true, Some(vec![mapped("fle-profile")]))),
                None,
            ),
            forwards_unknown_profile: true,
            content_type: supplied(),
        },
        Row {
            config: Config::new(
                "no content type section, three query argument mappings, submitted NOT \
                 forwarding",
                "fle-sections-supplied-many-false",
                Some("three mappings and no content type section"),
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
            forwards_unknown_profile: false,
            content_type: supplied(),
        },
        // A configuration that states one: stored as stated, including its own flag.
        Row {
            config: Config::new(
                "a content type mapping naming the profile, and no query argument section",
                "fle-sections-content-only",
                None,
                None,
                Some(Section::new(
                    true,
                    Some(vec![ContentTypeMapping::new("text/html", Some(profile))]),
                )),
            ),
            forwards_unknown_profile: true,
            content_type: ExpectedContentType {
                forward_when_unknown: true,
                mappings: Some(vec![ExpectedMapping::new("text/html", Some(profile))]),
            },
        },
        Row {
            config: Config::new(
                "a content type mapping naming the profile, beside a query argument wrapper \
                 that maps nothing",
                "fle-sections-empty-query-arg",
                Some(""),
                Some(Section::new(false, Some(Vec::new()))),
                Some(Section::new(
                    false,
                    Some(vec![ContentTypeMapping::new(
                        "application/json",
                        Some(profile),
                    )]),
                )),
            ),
            forwards_unknown_profile: false,
            content_type: ExpectedContentType {
                forward_when_unknown: false,
                mappings: Some(vec![ExpectedMapping::new("application/json", Some(profile))]),
            },
        },
        Row {
            config: Config::new(
                "two content type mappings, one naming no profile, beside three query \
                 argument mappings",
                "fle-sections-both-many",
                Some("both sections"),
                Some(Section::new(
                    false,
                    Some(vec![mapped("fle-profile"), mapped("alt-profile")]),
                )),
                Some(Section::new(
                    true,
                    Some(vec![
                        ContentTypeMapping::new(FORM_CONTENT_TYPE, Some(profile)),
                        ContentTypeMapping::new("text/xml", None),
                    ]),
                )),
            ),
            forwards_unknown_profile: false,
            content_type: ExpectedContentType {
                forward_when_unknown: true,
                mappings: Some(vec![
                    ExpectedMapping::new(FORM_CONTENT_TYPE, Some(profile)),
                    ExpectedMapping::new("text/xml", None),
                ]),
            },
        },
        // A section the request states WITHOUT its wrapper is stored without one: nothing is
        // filled into a section the request made its own choice about.
        Row {
            config: Config::new(
                "a content type section with no wrapper at all, beside one query argument \
                 mapping",
                "fle-sections-content-no-wrapper",
                None,
                Some(Section::new(true, Some(vec![mapped("fle-profile")]))),
                Some(Section::new(false, None)),
            ),
            forwards_unknown_profile: true,
            content_type: ExpectedContentType {
                forward_when_unknown: false,
                mappings: None,
            },
        },
        Row {
            config: Config::new(
                "a query argument section with no wrapper at all, beside a content type \
                 mapping naming the profile",
                "fle-sections-query-arg-no-wrapper",
                None,
                Some(Section::new(false, None)),
                Some(Section::new(
                    true,
                    Some(vec![ContentTypeMapping::new("text/plain", Some(profile))]),
                )),
            ),
            forwards_unknown_profile: false,
            content_type: ExpectedContentType {
                forward_when_unknown: true,
                mappings: Some(vec![ExpectedMapping::new("text/plain", Some(profile))]),
            },
        },
    ]
}

/// Assert that one serialised `FieldLevelEncryptionConfig` — or a `FieldLevelEncryptionSummary`,
/// which repeats the same two sections — reports the sections `row` has to report.
fn assert_sections(row: &Row, document: &Node, context: &str, raw: &str) {
    let case = row.config.case.as_str();

    let flag = document.text_at("QueryArgProfileConfig/ForwardWhenQueryArgProfileIsUnknown");
    assert_eq!(
        flag,
        Some(row.forwards_unknown_profile.to_string().as_str()),
        "case {case:?} ({context}): expected ForwardWhenQueryArgProfileIsUnknown {}, got \
         {flag:?} in {raw}",
        row.forwards_unknown_profile
    );

    let section = document.at("ContentTypeProfileConfig").unwrap_or_else(|| {
        panic!("case {case:?} ({context}): expected a ContentTypeProfileConfig in {raw}")
    });
    let flag = section.text_at("ForwardWhenContentTypeIsUnknown");
    assert_eq!(
        flag,
        Some(row.content_type.forward_when_unknown.to_string().as_str()),
        "case {case:?} ({context}): expected ForwardWhenContentTypeIsUnknown {}, got {flag:?} \
         in {raw}",
        row.content_type.forward_when_unknown
    );

    let Some(expected) = &row.content_type.mappings else {
        assert!(
            section.at("ContentTypeProfiles").is_none(),
            "case {case:?} ({context}): expected no ContentTypeProfiles wrapper, got {raw}"
        );
        return;
    };
    let quantity = section.text_at("ContentTypeProfiles/Quantity");
    assert_eq!(
        quantity,
        Some(expected.len().to_string().as_str()),
        "case {case:?} ({context}): expected ContentTypeProfiles Quantity {}, got {quantity:?} \
         in {raw}",
        expected.len()
    );
    let elements = section
        .at("ContentTypeProfiles/Items")
        .map(|items| items.children_named("ContentTypeProfile"))
        .unwrap_or_default();
    assert_eq!(
        elements.len(),
        expected.len(),
        "case {case:?} ({context}): expected {} ContentTypeProfile entries, got {} in {raw}",
        expected.len(),
        elements.len()
    );
    for (mapping, element) in expected.iter().zip(elements) {
        for (member, want) in [
            ("Format", Some(URL_ENCODED.to_string())),
            ("ContentType", Some(mapping.content_type.clone())),
            ("ProfileId", mapping.profile_id.clone()),
        ] {
            let actual = element.text_at(member).map(str::to_string);
            assert_eq!(
                actual, want,
                "case {case:?} ({context}): expected ContentTypeProfile {member} {want:?}, got \
                 {actual:?} in {raw}"
            );
        }
    }
}

/// The `FieldLevelEncryptionSummary` the listing carries for the configuration `id`.
fn summary_of<'a>(page: &'a Answer, id: &str) -> &'a Node {
    page.root()
        .at("Items")
        .map(|items| items.children_named("FieldLevelEncryptionSummary"))
        .unwrap_or_default()
        .into_iter()
        .find(|summary| summary.text_at("Id") == Some(id))
        .unwrap_or_else(|| panic!("no summary for {id} in {}", page.raw))
}

/// Create `row` and answer its id, the version token the create minted and the create's own
/// answer.
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
    (id, etag, answer)
}

/// Every representation of a stored configuration reports the two sections the rules decide:
/// the create's answer, both reads and the listing's summary.
#[tokio::test]
async fn every_representation_reports_the_sections_the_configuration_is_stored_with() {
    let router = emulator();
    let profile = store_profile(&router, "fle-sections-suite-profile").await;

    for row in rows(&profile) {
        let (id, _etag, create) = created(&router, &row).await;
        let config = create
            .root()
            .at("FieldLevelEncryptionConfig")
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: expected a configuration in {}",
                    row.config.case, create.raw
                )
            });
        assert_sections(&row, config, "CreateFieldLevelEncryptionConfig", &create.raw);

        let read = get_config(&router, &id).await;
        assert_eq!(
            read.status, 200,
            "case {:?}: expected HTTP 200 from the read, got {} with {}",
            row.config.case, read.status, read.raw
        );
        let config = read
            .root()
            .at("FieldLevelEncryptionConfig")
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: expected a configuration in {}",
                    row.config.case, read.raw
                )
            });
        assert_sections(&row, config, "GetFieldLevelEncryption", &read.raw);

        let document = get_config_document(&router, &id).await;
        assert_eq!(
            document.status, 200,
            "case {:?}: expected HTTP 200 from the configuration read, got {} with {}",
            row.config.case, document.status, document.raw
        );
        assert_sections(
            &row,
            document.root(),
            "GetFieldLevelEncryptionConfig",
            &document.raw,
        );

        let page = list_configs(&router, "").await;
        assert_eq!(
            page.status, 200,
            "case {:?}: expected HTTP 200 from the listing, got {} with {}",
            row.config.case, page.status, page.raw
        );
        assert_sections(
            &row,
            summary_of(&page, &id),
            "ListFieldLevelEncryptionConfigs",
            &page.raw,
        );
    }
}

/// The rules hold of the UPDATE too, and the stored form is a FIXED POINT: submitting the
/// document the read reported stores the same two sections again, and so does submitting the
/// original document (for a configuration whose own document an update may carry — an update
/// that states no `QueryArgProfileConfig` is refused, which
/// `field_level_encryption_config_update.rs` asserts).
#[tokio::test]
async fn an_update_stores_the_same_sections_and_the_readback_is_a_fixed_point() {
    let router = emulator();
    let profile = store_profile(&router, "fle-sections-update-profile").await;

    for row in rows(&profile) {
        let (id, etag, _create) = created(&router, &row).await;
        let mut version = etag;

        // The document the READ reported, submitted straight back, and then once more: a
        // read-modify-write round trip converges on the same two sections. The ORIGINAL
        // document goes through too wherever an update may carry it.
        let back = Config {
            query_arg: Some(stored_query_arg(&row)),
            content_type: Some(stored_content_type(&row)),
            ..row.config.clone()
        };
        let mut documents: Vec<(&str, String)> = vec![
            ("the write-back's answer", back.xml()),
            ("the write-back submitted again", back.xml()),
        ];
        if row.config.query_arg.is_some() {
            documents.push(("the original document again", row.config.xml()));
        }

        for (context, document) in documents {
            let updated = update_config(&router, &id, &document, Some(&version)).await;
            assert_eq!(
                updated.status, 200,
                "case {:?} ({context}): expected HTTP 200 from the update, got {} with {}",
                row.config.case, updated.status, updated.raw
            );
            version = updated.header("etag").to_string();
            let config = updated
                .root()
                .at("FieldLevelEncryptionConfig")
                .unwrap_or_else(|| {
                    panic!(
                        "case {:?}: expected a configuration in {}",
                        row.config.case, updated.raw
                    )
                });
            assert_sections(&row, config, context, &updated.raw);

            let read = get_config(&router, &id).await;
            let config = read
                .root()
                .at("FieldLevelEncryptionConfig")
                .unwrap_or_else(|| {
                    panic!(
                        "case {:?}: expected a configuration in {}",
                        row.config.case, read.raw
                    )
                });
            assert_sections(&row, config, "the read after that update", &read.raw);
        }
    }
}

/// The `QueryArgProfileConfig` section a client reads back for `row`, as a document it can
/// submit again: the one the request stated, or the mapping that maps nothing for a request
/// that stated none.
fn stored_query_arg(row: &Row) -> Section<QueryArgMapping> {
    let mut section = row
        .config
        .query_arg
        .clone()
        .unwrap_or_else(|| Section::new(true, Some(Vec::new())));
    section.forward_when_unknown = row.forwards_unknown_profile;
    section
}

/// The `ContentTypeProfileConfig` section a client reads back for `row`, as a document it can
/// submit again.
fn stored_content_type(row: &Row) -> Section<ContentTypeMapping> {
    let mappings = row.content_type.mappings.as_ref().map(|mappings| {
        mappings
            .iter()
            .map(|mapping| {
                ContentTypeMapping::new(&mapping.content_type, mapping.profile_id.as_deref())
            })
            .collect()
    });
    Section::new(row.content_type.forward_when_unknown, mappings)
}

/// The mapping the service supplies names NO profile, so it does not rescue a configuration
/// that designates none: one that states neither a query argument mapping naming a profile
/// nor a content type mapping naming one is still refused, and stores nothing.
#[tokio::test]
async fn the_supplied_mapping_names_no_profile_so_it_rescues_no_configuration() {
    let router = emulator();
    let profile = store_profile(&router, "fle-sections-refusal-profile").await;

    let rows = vec![
        Config::new(
            "neither section at all",
            "fle-sections-refused-neither",
            None,
            None,
            None,
        ),
        Config::new(
            "a query argument wrapper that maps nothing and no content type section",
            "fle-sections-refused-empty",
            Some("nothing designated"),
            Some(Section::new(true, Some(Vec::new()))),
            None,
        ),
        Config::new(
            "a query argument section with no wrapper and no content type section",
            "fle-sections-refused-no-wrapper",
            None,
            Some(Section::new(false, None)),
            None,
        ),
    ];

    for row in &rows {
        let refused = create_config(&router, &row.xml()).await;
        assert_eq!(
            refused.status, 400,
            "case {:?}: expected HTTP 400 for a configuration designating no profile, got {} \
             with {}",
            row.case, refused.status, refused.raw
        );
        let code = refused.root().find_text("Code").map(str::to_string);
        assert_eq!(
            code.as_deref(),
            Some("QueryArgProfileEmpty"),
            "case {:?}: expected QueryArgProfileEmpty, got {code:?} in {}",
            row.case,
            refused.raw
        );
        let message = refused.root().find_text("Message").map(str::to_string);
        assert_eq!(
            message.as_deref(),
            Some(QUERY_ARG_PROFILE_EMPTY),
            "case {:?}: expected the refusal's own sentence, got {message:?} in {}",
            row.case,
            refused.raw
        );
    }

    // And the same configurations with a content type mapping that DOES name a profile are
    // stored, which is what shows the refusals above are about the profile and not about the
    // sections' shapes.
    for row in &rows {
        let allowed = Config {
            case: format!("{} , with a content type mapping naming the profile", row.case),
            caller_reference: format!("{}-named", row.caller_reference),
            content_type: Some(Section::new(
                false,
                Some(vec![ContentTypeMapping::new(
                    FORM_CONTENT_TYPE,
                    Some(&profile),
                )]),
            )),
            ..row.clone()
        };
        let answer = create_config(&router, &allowed.xml()).await;
        assert_eq!(
            answer.status, 201,
            "case {:?}: expected HTTP 201, got {} with {}",
            allowed.case, answer.status, answer.raw
        );
    }
}
