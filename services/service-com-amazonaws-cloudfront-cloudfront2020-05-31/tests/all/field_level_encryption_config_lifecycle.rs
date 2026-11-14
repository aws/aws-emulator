// SPDX-License-Identifier: CC0-1.0

//! The CloudFront field-level encryption CONFIGURATION control plane, end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing, restXml
//! (de)serialization, the handler, the persisted state and serialization — the same path a
//! client takes. The rules under test are the ones CloudFront's field-level encryption
//! configuration surface states:
//!
//! * a configuration is PERSISTED by the call that creates it, which MINTS its identity (an
//!   id and a version token) and records the instant it was written, so `Location` and a
//!   later `GetFieldLevelEncryption` answer from that record rather than from the request
//!   being served — down to the members the create left out;
//! * every CloudFront list travels in a wrapper stating its own length beside the list
//!   itself, and a request whose `Quantity` disagrees with its `Items` is refused
//!   (`InconsistentQuantities`, 400) — over the `QueryArgProfiles` wrapper and over the
//!   `ContentTypeProfiles` wrapper;
//! * a mapping names the PROFILE the matching fields are encrypted under, so an id no stored
//!   profile carries is refused (`NoSuchFieldLevelEncryptionProfile`, 404), and a query
//!   argument mapping that states no profile at all is refused (`QueryArgProfileEmpty`, 400);
//! * `FieldLevelEncryptionConfig$CallerReference` is "a unique number that ensures the
//!   request can't be replayed": the same request again reports the configuration the first
//!   create stored, and a different configuration under a reference already taken is refused
//!   (`FieldLevelEncryptionConfigAlreadyExists`, 409);
//! * `FieldLevelEncryptionConfig$Comment`: "The comment cannot be longer than 128
//!   characters", so a longer comment is refused before anything is stored;
//! * a mapping list is bounded, and a configuration past the bound is refused
//!   (`TooManyFieldLevelEncryptionQueryArgProfiles` /
//!   `TooManyFieldLevelEncryptionContentTypeProfiles`, 400);
//! * an id no configuration holds is `NoSuchFieldLevelEncryptionConfig` (404);
//! * the CONFIGURATION reader reports the stored `FieldLevelEncryptionConfig` document on its
//!   own, at the version the configuration itself is read at, and reading it changes nothing;
//! * a refusal stores nothing: the caller reference it named is still free afterwards.
//!
//! The table rows differ along everything these rules generalise over: which mapping
//! sections the configuration carries at all (neither, one, both), whether a section's
//! wrapper is present and how many mappings it holds (none, one, several), the forwarding
//! flags (both settings), whether a content type mapping names a profile at all, and whether
//! a comment is carried, empty, plain ASCII, XML metacharacters or exactly on the published
//! bound.
//!
//! The harness (the router, the request helpers and the element scanner) is the distribution
//! lifecycle suite's, the profile every mapping points at is stored through the profile
//! suite's own create path, and the wire timestamp check is the public key suite's — reused
//! rather than copied.

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::{
    MAX_FIELD_LEVEL_ENCRYPTION_CONFIG_COMMENT, MAX_FIELD_LEVEL_ENCRYPTION_CONTENT_TYPE_PROFILES,
    MAX_FIELD_LEVEL_ENCRYPTION_QUERY_ARG_PROFILES,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::distribution_lifecycle::{
    call, emulator, inconsistent_quantities_message, Answer, Node, API_VERSION, HOST,
};
use crate::field_level_encryption_profile_lifecycle::store_profile;
use crate::public_key_lifecycle::is_wire_timestamp;
// The XML escaper is the crate's: one escaper for every suite, so no two of them can
// disagree about how a submitted value is written.
use crate::response_headers_policy_lifecycle::escape;

/// The one `Format` the model's `Format` enum admits for a content type-profile mapping
/// (`smithy_query(query="shape",shape="Format")` lists `URLEncoded` and nothing else).
const URL_ENCODED: &str = "URLEncoded";

/// The sentences CloudFront sends on the wire for these refusals, spelled out here rather
/// than taken from the crate, so a reworded refusal fails this suite.
const NO_SUCH_CONFIG_MESSAGE: &str = "The specified field level encryption config does not exist.";
const NO_SUCH_PROFILE_MESSAGE: &str = "The specified field level encryption profile does not exist.";
const ID_FORMAT_INVALID_MESSAGE: &str = "Id format is invalid";
const QUERY_ARG_PROFILE_EMPTY: &str =
    "The query arg profile in field level encryption config cannot be empty when content \
     type profile is empty or when no default profile id is given in any content type \
     profile.";

// ---------------------------------------------------------------------------
// Calling the configuration surface
// ---------------------------------------------------------------------------

/// `POST /2020-05-31/field-level-encryption` with the `FieldLevelEncryptionConfig` document
/// `body` — the member the operation binds to the HTTP payload.
pub(crate) async fn create_config(router: &MultiServiceRouter, body: &str) -> Answer {
    call(
        router,
        "POST",
        &format!("/{API_VERSION}/field-level-encryption"),
        Some(body),
        None,
    )
    .await
}

/// `GET /2020-05-31/field-level-encryption/{Id}`.
pub(crate) async fn get_config(router: &MultiServiceRouter, id: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/{API_VERSION}/field-level-encryption/{id}"),
        None,
        None,
    )
    .await
}

/// `GET /2020-05-31/field-level-encryption/{Id}/config` — the CONFIGURATION half of one
/// stored field-level encryption configuration
/// (`smithy_query(query="operation",operation="GetFieldLevelEncryptionConfig")` states the
/// route).
pub(crate) async fn get_config_document(router: &MultiServiceRouter, id: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/{API_VERSION}/field-level-encryption/{id}/config"),
        None,
        None,
    )
    .await
}

// ---------------------------------------------------------------------------
// The configurations under test
// ---------------------------------------------------------------------------

/// One `QueryArgProfile`, as a client submits it.
#[derive(Debug, Clone)]
pub(crate) struct QueryArgMapping {
    query_arg: String,
    profile_id: String,
}

impl QueryArgMapping {
    pub(crate) fn new(query_arg: &str, profile_id: &str) -> Self {
        Self {
            query_arg: query_arg.to_string(),
            profile_id: profile_id.to_string(),
        }
    }

    /// The `QueryArgProfile` XML a client sends, in the model's member declaration order.
    fn xml(&self) -> String {
        format!(
            "<QueryArgProfile><QueryArg>{}</QueryArg><ProfileId>{}</ProfileId></QueryArgProfile>",
            escape(&self.query_arg),
            escape(&self.profile_id)
        )
    }

    /// Assert that `mapping` — a `QueryArgProfile` element the emulator serialised — reports
    /// exactly this mapping.
    fn assert_matches(&self, mapping: &Node, case: &str, context: &str, raw: &str) {
        for (member, expected) in [
            ("QueryArg", self.query_arg.clone()),
            ("ProfileId", self.profile_id.clone()),
        ] {
            let actual = mapping.text_at(member).map(str::to_string);
            assert_eq!(
                actual.as_deref(),
                Some(expected.as_str()),
                "case {case:?} ({context}): expected QueryArgProfile {member} {expected:?}, got \
                 {actual:?} in {raw}"
            );
        }
    }
}

/// One `ContentTypeProfile`, as a client submits it.
#[derive(Debug, Clone)]
pub(crate) struct ContentTypeMapping {
    pub(crate) content_type: String,
    /// `ProfileId`, or `None` to leave the optional member out entirely.
    pub(crate) profile_id: Option<String>,
}

impl ContentTypeMapping {
    pub(crate) fn new(content_type: &str, profile_id: Option<&str>) -> Self {
        Self {
            content_type: content_type.to_string(),
            profile_id: profile_id.map(str::to_string),
        }
    }

    /// The `ContentTypeProfile` XML a client sends, in the model's member declaration order
    /// (`Format`, `ProfileId`, `ContentType`).
    fn xml(&self) -> String {
        let mut out = format!("<ContentTypeProfile><Format>{URL_ENCODED}</Format>");
        if let Some(profile_id) = &self.profile_id {
            out.push_str(&format!("<ProfileId>{}</ProfileId>", escape(profile_id)));
        }
        out.push_str(&format!(
            "<ContentType>{}</ContentType></ContentTypeProfile>",
            escape(&self.content_type)
        ));
        out
    }

    /// Assert that `mapping` — a `ContentTypeProfile` element the emulator serialised —
    /// reports exactly this mapping.
    fn assert_matches(&self, mapping: &Node, case: &str, context: &str, raw: &str) {
        for (member, expected) in [
            ("Format", Some(URL_ENCODED.to_string())),
            ("ContentType", Some(self.content_type.clone())),
            // An optional member the create left out is reported as the stored record has
            // it: absent, rather than as an empty element invented by the read.
            ("ProfileId", self.profile_id.clone()),
        ] {
            let actual = mapping.text_at(member).map(str::to_string);
            assert_eq!(
                actual, expected,
                "case {case:?} ({context}): expected ContentTypeProfile {member} {expected:?}, \
                 got {actual:?} in {raw}"
            );
        }
    }
}

/// One mapping section of a configuration — a `QueryArgProfileConfig` or a
/// `ContentTypeProfileConfig`: a forwarding flag beside an OPTIONAL wrapper holding the
/// mappings.
#[derive(Debug, Clone)]
pub(crate) struct Section<M> {
    pub(crate) forward_when_unknown: bool,
    /// `None` leaves the wrapper out entirely — an absent wrapper states no quantity at all.
    pub(crate) mappings: Option<Vec<M>>,
    /// The `Quantity` the request STATES, when it is not the number of mappings carried.
    pub(crate) stated_quantity: Option<i32>,
}

impl<M> Section<M> {
    pub(crate) fn new(forward_when_unknown: bool, mappings: Option<Vec<M>>) -> Self {
        Self {
            forward_when_unknown,
            mappings,
            stated_quantity: None,
        }
    }

    /// The `Quantity` this section's request document states.
    fn quantity(&self) -> i32 {
        self.stated_quantity.unwrap_or_else(|| {
            i32::try_from(self.mappings.as_ref().map_or(0, Vec::len)).unwrap_or(i32::MAX)
        })
    }
}

/// One `FieldLevelEncryptionConfig`, as a client submits it.
#[derive(Debug, Clone)]
pub(crate) struct Config {
    /// What this row is a case of, named in every assertion it drives.
    pub(crate) case: String,
    pub(crate) caller_reference: String,
    /// `Comment`, or `None` to leave the optional member out entirely.
    pub(crate) comment: Option<String>,
    /// The `QueryArgProfileConfig`, or `None` to leave the whole section out.
    pub(crate) query_arg: Option<Section<QueryArgMapping>>,
    /// The `ContentTypeProfileConfig`, or `None` to leave the whole section out.
    pub(crate) content_type: Option<Section<ContentTypeMapping>>,
}

impl Config {
    pub(crate) fn new(
        case: &str,
        reference: &str,
        comment: Option<&str>,
        query_arg: Option<Section<QueryArgMapping>>,
        content_type: Option<Section<ContentTypeMapping>>,
    ) -> Self {
        Self {
            case: case.to_string(),
            caller_reference: reference.to_string(),
            comment: comment.map(str::to_string),
            query_arg,
            content_type,
        }
    }

    /// The `FieldLevelEncryptionConfig` XML a client sends for this configuration, in the
    /// model's member declaration order (`CallerReference`, `Comment`,
    /// `QueryArgProfileConfig`, `ContentTypeProfileConfig`).
    pub(crate) fn xml(&self) -> String {
        let mut out = format!(
            "<FieldLevelEncryptionConfig \
             xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">"
        );
        out.push_str(&format!(
            "<CallerReference>{}</CallerReference>",
            escape(&self.caller_reference)
        ));
        if let Some(comment) = &self.comment {
            out.push_str(&format!("<Comment>{}</Comment>", escape(comment)));
        }
        if let Some(section) = &self.query_arg {
            out.push_str(&format!(
                "<QueryArgProfileConfig><ForwardWhenQueryArgProfileIsUnknown>{}\
                 </ForwardWhenQueryArgProfileIsUnknown>",
                section.forward_when_unknown
            ));
            if let Some(mappings) = &section.mappings {
                out.push_str(&format!(
                    "<QueryArgProfiles><Quantity>{}</Quantity>",
                    section.quantity()
                ));
                if !mappings.is_empty() {
                    out.push_str("<Items>");
                    for mapping in mappings {
                        out.push_str(&mapping.xml());
                    }
                    out.push_str("</Items>");
                }
                out.push_str("</QueryArgProfiles>");
            }
            out.push_str("</QueryArgProfileConfig>");
        }
        if let Some(section) = &self.content_type {
            out.push_str(&format!(
                "<ContentTypeProfileConfig><ForwardWhenContentTypeIsUnknown>{}\
                 </ForwardWhenContentTypeIsUnknown>",
                section.forward_when_unknown
            ));
            if let Some(mappings) = &section.mappings {
                out.push_str(&format!(
                    "<ContentTypeProfiles><Quantity>{}</Quantity>",
                    section.quantity()
                ));
                if !mappings.is_empty() {
                    out.push_str("<Items>");
                    for mapping in mappings {
                        out.push_str(&mapping.xml());
                    }
                    out.push_str("</Items>");
                }
                out.push_str("</ContentTypeProfiles>");
            }
            out.push_str("</ContentTypeProfileConfig>");
        }
        out.push_str("</FieldLevelEncryptionConfig>");
        out
    }

    /// Assert that `config` — a `FieldLevelEncryptionConfig` element the emulator serialised
    /// — reports the configuration the stored record runs under: every member this request
    /// carried, and for a member it left out the value CloudFront supplies for it.
    pub(crate) fn assert_matches(&self, config: &Node, context: &str, raw: &str) {
        let case = self.case.as_str();
        for (member, expected) in [
            ("CallerReference", Some(self.caller_reference.clone())),
            // "An optional comment about the configuration": a configuration that states
            // none carries the empty comment the service supplies, so the member is
            // reported either way.
            ("Comment", Some(self.effective_comment())),
        ] {
            let actual = config.text_at(member).map(str::to_string);
            assert_eq!(
                actual, expected,
                "case {case:?} ({context}): expected {member} {expected:?}, got {actual:?} in \
                 {raw}"
            );
        }

        {
            // `QueryArgProfileConfig` is the mapping every stored configuration carries, so
            // a configuration that states none is reported with the mapping that maps
            // nothing.
            let section = self.effective_query_arg();
            let node = config.at("QueryArgProfileConfig").unwrap_or_else(|| {
                panic!("case {case:?} ({context}): expected a QueryArgProfileConfig in {raw}")
            });
            let flag = node.text_at("ForwardWhenQueryArgProfileIsUnknown");
            assert_eq!(
                flag,
                Some(section.forward_when_unknown.to_string().as_str()),
                "case {case:?} ({context}): expected \
                 ForwardWhenQueryArgProfileIsUnknown {}, got {flag:?} in {raw}",
                section.forward_when_unknown
            );
            assert_section(
                node,
                "QueryArgProfiles",
                "QueryArgProfile",
                &section,
                case,
                context,
                raw,
                |mapping, element| mapping.assert_matches(element, case, context, raw),
            );
        }

        match &self.content_type {
            None => assert!(
                config.at("ContentTypeProfileConfig").is_none(),
                "case {case:?} ({context}): expected no ContentTypeProfileConfig, got {raw}"
            ),
            Some(section) => {
                let node = config.at("ContentTypeProfileConfig").unwrap_or_else(|| {
                    panic!(
                        "case {case:?} ({context}): expected a ContentTypeProfileConfig in {raw}"
                    )
                });
                let flag = node.text_at("ForwardWhenContentTypeIsUnknown");
                assert_eq!(
                    flag,
                    Some(section.forward_when_unknown.to_string().as_str()),
                    "case {case:?} ({context}): expected ForwardWhenContentTypeIsUnknown {}, got \
                     {flag:?} in {raw}",
                    section.forward_when_unknown
                );
                assert_section(
                    node,
                    "ContentTypeProfiles",
                    "ContentTypeProfile",
                    section,
                    case,
                    context,
                    raw,
                    |mapping, element| mapping.assert_matches(element, case, context, raw),
                );
            }
        }
    }

    /// The `Comment` the stored configuration carries: the one this request submits, or the
    /// empty comment CloudFront supplies for a configuration that states none.
    pub(crate) fn effective_comment(&self) -> String {
        self.comment.clone().unwrap_or_default()
    }

    /// The `QueryArgProfileConfig` the stored configuration carries: the section this
    /// request submits exactly as submitted, or — for a request that states no section at
    /// all — the mapping that maps nothing, under the forwarding flag the service supplies
    /// for it.
    pub(crate) fn effective_query_arg(&self) -> Section<QueryArgMapping> {
        match &self.query_arg {
            Some(section) => section.clone(),
            None => Section::new(true, Some(Vec::new())),
        }
    }
}

/// Assert that the `wrapper` of one serialised section reports the mappings `section`
/// submitted, at the quantity it stated — the one walk both sections are checked by, since
/// both are a `Quantity` beside an `Items` list of the same shape.
#[allow(clippy::too_many_arguments)]
fn assert_section<M>(
    node: &Node,
    wrapper: &str,
    member: &str,
    section: &Section<M>,
    case: &str,
    context: &str,
    raw: &str,
    assert_mapping: impl Fn(&M, &Node),
) {
    let Some(mappings) = &section.mappings else {
        assert!(
            node.at(wrapper).is_none(),
            "case {case:?} ({context}): expected no {wrapper} wrapper, got {raw}"
        );
        return;
    };
    let quantity = node.text_at(&format!("{wrapper}/Quantity"));
    assert_eq!(
        quantity,
        Some(section.quantity().to_string().as_str()),
        "case {case:?} ({context}): expected {wrapper} Quantity {}, got {quantity:?} in {raw}",
        section.quantity()
    );
    let serialised: Vec<&Node> = node
        .at(&format!("{wrapper}/Items"))
        .map(|items| items.children_named(member))
        .unwrap_or_default();
    assert_eq!(
        serialised.len(),
        mappings.len(),
        "case {case:?} ({context}): expected {} {member} entries, got {} in {raw}",
        mappings.len(),
        serialised.len()
    );
    for (expected, actual) in mappings.iter().zip(serialised) {
        assert_mapping(expected, actual);
    }
}

/// Whether `value` has the shape of a field-level encryption configuration id CloudFront
/// mints: a `C` followed by 13 characters of the id alphabet, the shape its own answers show.
///
/// Spelled out here rather than taken from the crate, so a changed mint fails this suite.
pub(crate) fn is_config_id(value: &str) -> bool {
    match value.strip_prefix('C') {
        Some(body) => {
            body.len() == 13
                && body
                    .bytes()
                    .all(|b| b.is_ascii_digit() || b.is_ascii_uppercase())
        }
        None => false,
    }
}

/// The configurations every case below is run over, each mapping pointing at the stored
/// profile `profile`.
///
/// The rows span what the rules generalise over: one mapping section and both; a section
/// whose wrapper is left out, one stating an empty list, one mapping and several; both
/// settings of each forwarding flag; a content type mapping with and without a `ProfileId`;
/// and the comment forms (absent, empty, plain ASCII, XML metacharacters, and the published
/// 128-character bound). Every row names a profile in one of its two sections, since a
/// configuration naming none in either is refused (`QueryArgProfileEmpty`, 400) — asserted
/// in [`crate::request_validation_limits`].
pub(crate) fn table(profile: &str) -> Vec<Config> {
    // "The comment cannot be longer than 128 characters": the bound itself is admissible, so
    // one row sits exactly on it.
    let on_the_bound = "c".repeat(MAX_FIELD_LEVEL_ENCRYPTION_CONFIG_COMMENT);
    vec![
        Config::new(
            "a query argument section with no wrapper at all, beside a content type mapping \
             naming the profile",
            "fle-config-no-wrapper",
            Some(""),
            Some(Section::new(true, None)),
            Some(Section::new(
                false,
                Some(vec![ContentTypeMapping::new("text/html", Some(profile))]),
            )),
        ),
        Config::new(
            "an empty query argument list beside a content type mapping naming the profile",
            "fle-config-empty-lists",
            Some("the configuration for an emulator test"),
            Some(Section::new(false, Some(Vec::new()))),
            Some(Section::new(
                false,
                Some(vec![ContentTypeMapping::new(
                    "application/json",
                    Some(profile),
                )]),
            )),
        ),
        Config::new(
            "one query argument mapping",
            "fle-config-single-query-arg",
            Some("tags <field> & \"quotes\" and 'apostrophes'",),
            Some(Section::new(
                true,
                Some(vec![QueryArgMapping::new("fle-profile", profile)]),
            )),
            None,
        ),
        Config::new(
            "one content type mapping naming the profile",
            "fle-config-single-content-type",
            None,
            None,
            Some(Section::new(
                true,
                Some(vec![ContentTypeMapping::new(
                    "application/x-www-form-urlencoded",
                    Some(profile),
                )]),
            )),
        ),
        Config::new(
            "a content type mapping that names no profile, beside a query argument mapping",
            "fle-config-content-type-no-profile",
            Some("no profile on the mapping"),
            Some(Section::new(
                false,
                Some(vec![QueryArgMapping::new("fle-profile", profile)]),
            )),
            Some(Section::new(
                true,
                Some(vec![ContentTypeMapping::new("text/plain", None)]),
            )),
        ),
        Config::new(
            "several mappings in both sections, the comment exactly on the bound",
            "fle-config-both-sections",
            Some(on_the_bound.as_str()),
            Some(Section::new(
                false,
                Some(vec![
                    QueryArgMapping::new("fle-profile", profile),
                    QueryArgMapping::new("alt-profile", profile),
                    QueryArgMapping::new("profile&arg", profile),
                ]),
            )),
            Some(Section::new(
                true,
                Some(vec![
                    ContentTypeMapping::new("application/x-www-form-urlencoded", Some(profile)),
                    ContentTypeMapping::new("text/xml", None),
                ]),
            )),
        ),
    ]
}

/// A configuration whose only mapping is one query argument pointing at `profile`, under
/// `reference` — the minimal well-formed row the single-rule cases start from.
pub(crate) fn simple(case: &str, reference: &str, profile: &str) -> Config {
    Config::new(
        case,
        reference,
        None,
        Some(Section::new(
            true,
            Some(vec![QueryArgMapping::new("fle-profile", profile)]),
        )),
        None,
    )
}

// ---------------------------------------------------------------------------
// The cases
// ---------------------------------------------------------------------------

/// The create PERSISTS the configuration, and the read of it reports it from that record: the
/// minted id, the instant it was written, the version token and the configuration as
/// submitted — down to the members the create left out. `Location` names the configuration's
/// own resource path under this service's endpoint.
#[tokio::test]
async fn create_persists_the_configuration_the_read_then_reports() {
    let router = emulator();
    let profile = store_profile(&router, "fle-config-suite-profile").await;

    for row in table(&profile) {
        let created = create_config(&router, &row.xml()).await;
        assert_eq!(
            created.status, 201,
            "case {:?}: expected HTTP 201 from CreateFieldLevelEncryptionConfig, got {} with {}",
            row.case, created.status, created.raw
        );

        let id = created.root().text_at("Id").unwrap_or_default().to_string();
        assert!(
            is_config_id(&id),
            "case {:?}: expected a minted configuration id, got {id:?}",
            row.case
        );
        let last_modified = created
            .root()
            .text_at("LastModifiedTime")
            .unwrap_or_default()
            .to_string();
        assert!(
            is_wire_timestamp(&last_modified),
            "case {:?}: expected a wire timestamp for LastModifiedTime, got {last_modified:?}",
            row.case
        );
        let etag = created.header("etag").to_string();
        assert!(
            !etag.is_empty(),
            "case {:?}: expected an ETag header on the create, got {etag:?} with {}",
            row.case,
            created.raw
        );
        assert_eq!(
            created.header("location"),
            format!("https://{HOST}/{API_VERSION}/field-level-encryption/{id}"),
            "case {:?}: expected Location to name the created configuration, got {:?}",
            row.case,
            created.header("location")
        );
        row.assert_matches(
            created
                .root()
                .at("FieldLevelEncryptionConfig")
                .unwrap_or_else(|| {
                    panic!(
                        "the create answers with the configuration: {}",
                        created.raw
                    )
                }),
            "the create",
            &created.raw,
        );

        // The reader answers from the stored record: the same id, the same instant, the same
        // version and the same configuration.
        let read = get_config(&router, &id).await;
        assert_eq!(
            read.status, 200,
            "case {:?}: expected HTTP 200 from GetFieldLevelEncryption, got {} with {}",
            row.case, read.status, read.raw
        );
        assert_eq!(
            read.root().text_at("Id"),
            Some(id.as_str()),
            "case {:?}: expected the read to report the created id, got {:?}",
            row.case,
            read.root().text_at("Id")
        );
        assert_eq!(
            read.root().text_at("LastModifiedTime"),
            Some(last_modified.as_str()),
            "case {:?}: expected the read to report the stored LastModifiedTime \
             {last_modified:?}, got {:?}",
            row.case,
            read.root().text_at("LastModifiedTime")
        );
        assert_eq!(
            read.header("etag"),
            etag,
            "case {:?}: expected the read at the create's version {etag:?}, got {:?}",
            row.case,
            read.header("etag")
        );
        row.assert_matches(
            read.root()
                .at("FieldLevelEncryptionConfig")
                .unwrap_or_else(|| {
                    panic!("the read answers with the configuration: {}", read.raw)
                }),
            "the read",
            &read.raw,
        );
    }
}

/// A wrapper whose `Quantity` disagrees with its `Items` describes no list at all, and the
/// create is refused with the error the operation models for it (`InconsistentQuantities`,
/// 400), naming the wrapper, the quantity the request stated and the number it carried. The
/// refusal stores nothing: the caller reference it named is still free, so the corrected
/// request is accepted under it.
#[tokio::test]
async fn a_wrapper_whose_quantity_disagrees_with_its_items_is_refused() {
    let router = emulator();
    let profile = store_profile(&router, "fle-config-quantity-profile").await;

    // Each row breaks ONE wrapper of an otherwise well-formed configuration, and the rows
    // span the ways a quantity can disagree: too high, too low, and zero beside a carried
    // list, over each of the two wrappers a configuration reaches.
    let mut rows: Vec<(String, i32, usize, Config)> = Vec::new();

    let mut overstated = simple(
        "QueryArgProfiles Quantity one too high",
        "fle-config-quantity-high",
        &profile,
    );
    if let Some(section) = overstated.query_arg.as_mut() {
        section.stated_quantity = Some(2);
    }
    rows.push(("QueryArgProfiles".to_string(), 2, 1, overstated));

    let mut understated = Config::new(
        "QueryArgProfiles Quantity one too low",
        "fle-config-quantity-low",
        Some("two mappings, one declared"),
        Some(Section::new(
            true,
            Some(vec![
                QueryArgMapping::new("fle-profile", &profile),
                QueryArgMapping::new("alt-profile", &profile),
            ]),
        )),
        None,
    );
    if let Some(section) = understated.query_arg.as_mut() {
        section.stated_quantity = Some(1);
    }
    rows.push(("QueryArgProfiles".to_string(), 1, 2, understated));

    let mut zeroed = simple(
        "QueryArgProfiles Quantity zero beside a carried list",
        "fle-config-quantity-zero",
        &profile,
    );
    if let Some(section) = zeroed.query_arg.as_mut() {
        section.stated_quantity = Some(0);
    }
    rows.push(("QueryArgProfiles".to_string(), 0, 1, zeroed));

    let mut content_type_high = Config::new(
        "ContentTypeProfiles Quantity one too high",
        "fle-config-content-type-high",
        None,
        None,
        Some(Section::new(
            true,
            Some(vec![ContentTypeMapping::new(
                "application/x-www-form-urlencoded",
                Some(&profile),
            )]),
        )),
    );
    if let Some(section) = content_type_high.content_type.as_mut() {
        section.stated_quantity = Some(2);
    }
    rows.push(("ContentTypeProfiles".to_string(), 2, 1, content_type_high));

    let mut content_type_low = Config::new(
        "ContentTypeProfiles Quantity one too low beside the other section",
        "fle-config-content-type-low",
        Some(""),
        Some(Section::new(
            true,
            Some(vec![QueryArgMapping::new("fle-profile", &profile)]),
        )),
        Some(Section::new(
            false,
            Some(vec![
                ContentTypeMapping::new("text/plain", None),
                ContentTypeMapping::new("text/xml", Some(&profile)),
                ContentTypeMapping::new("application/json", Some(&profile)),
            ]),
        )),
    );
    if let Some(section) = content_type_low.content_type.as_mut() {
        section.stated_quantity = Some(2);
    }
    rows.push(("ContentTypeProfiles".to_string(), 2, 3, content_type_low));

    for (wrapper, stated, carried, row) in rows {
        let refused = create_config(&router, &row.xml()).await;
        assert_eq!(
            refused.status, 400,
            "case {:?}: expected HTTP 400 for a disagreeing quantity, got {} with {}",
            row.case, refused.status, refused.raw
        );
        assert_eq!(
            refused.error_code(),
            "InconsistentQuantities",
            "case {:?}: expected InconsistentQuantities, got {}",
            row.case,
            refused.raw
        );
        let expected = inconsistent_quantities_message(&wrapper, stated, carried);
        assert_eq!(
            refused.root().find_text("Message"),
            Some(expected.as_str()),
            "case {:?}: expected the refusal to read {expected:?}, got {:?}",
            row.case,
            refused.root().find_text("Message")
        );

        // The refusal stored nothing, so the reference it named is still free: the same
        // request with the wrapper corrected is accepted under it.
        let mut corrected = row.clone();
        if let Some(section) = corrected.query_arg.as_mut() {
            section.stated_quantity = None;
        }
        if let Some(section) = corrected.content_type.as_mut() {
            section.stated_quantity = None;
        }
        let accepted = create_config(&router, &corrected.xml()).await;
        assert_eq!(
            accepted.status, 201,
            "case {:?}: expected the corrected request to be accepted under the same caller \
             reference, got {} with {}",
            row.case, accepted.status, accepted.raw
        );
    }
}

/// A mapping names the profile the matching fields are encrypted under, so an id no stored
/// profile carries names no profile at all and the create is refused
/// (`NoSuchFieldLevelEncryptionProfile`, 404) — from either mapping kind, and whichever entry
/// names it. The refusal stores nothing, so the same request is refused the same way again.
#[tokio::test]
async fn a_mapping_naming_no_stored_profile_is_refused() {
    let router = emulator();
    let profile = store_profile(&router, "fle-config-unknown-profile-suite").await;

    // The rows span the ways an id can name no stored profile: a well-shaped id of the
    // profile namespace that was never minted, an id of ANOTHER CloudFront namespace, a value
    // of no CloudFront shape at all, a bad id in a LATER mapping rather than the first, and a
    // bad id on a content type mapping rather than a query argument one.
    let rows: Vec<Config> = vec![
        simple(
            "a never-minted profile id",
            "fle-config-unknown-profile",
            "P280MFCLSYOCVU",
        ),
        simple(
            "a distribution id where a profile id belongs",
            "fle-config-wrong-namespace",
            "EDFDVBD6EXAMPLE",
        ),
        simple(
            "a value of no CloudFront id shape",
            "fle-config-unshaped-profile",
            "not-a-profile-id",
        ),
        Config::new(
            "the second query argument mapping names no stored profile",
            "fle-config-second-unknown",
            None,
            Some(Section::new(
                true,
                Some(vec![
                    QueryArgMapping::new("fle-profile", &profile),
                    QueryArgMapping::new("alt-profile", "P3QPWXYZABCDE"),
                ]),
            )),
            None,
        ),
        Config::new(
            "a content type mapping names no stored profile",
            "fle-config-content-type-unknown",
            Some("an unknown profile on a content type mapping"),
            None,
            Some(Section::new(
                false,
                Some(vec![
                    ContentTypeMapping::new("text/plain", None),
                    ContentTypeMapping::new("text/xml", Some("P3QPWXYZABCDE")),
                ]),
            )),
        ),
    ];

    for row in rows {
        for attempt in 1..=2 {
            let refused = create_config(&router, &row.xml()).await;
            assert_eq!(
                refused.status, 404,
                "case {:?} (attempt {attempt}): expected HTTP 404 for an unknown profile, got {} \
                 with {}",
                row.case, refused.status, refused.raw
            );
            assert_eq!(
                refused.error_code(),
                "NoSuchFieldLevelEncryptionProfile",
                "case {:?} (attempt {attempt}): expected NoSuchFieldLevelEncryptionProfile, got {}",
                row.case,
                refused.raw
            );
            assert_eq!(
                refused.root().find_text("Message"),
                Some(NO_SUCH_PROFILE_MESSAGE),
                "case {:?} (attempt {attempt}): expected the modelled message, got {:?}",
                row.case,
                refused.root().find_text("Message")
            );
        }
    }
}

/// A query argument-profile mapping pairs a query argument with the profile the matching
/// fields are encrypted under, so one that states no profile maps its argument to nothing and
/// the create is refused (`QueryArgProfileEmpty`, 400) before anything is stored.
#[tokio::test]
async fn a_query_argument_mapping_specifying_no_profile_is_refused() {
    let router = emulator();
    let profile = store_profile(&router, "fle-config-empty-profile-suite").await;

    // The rows span the ways a mapping can state no profile: an empty element, one carrying
    // only whitespace, and an empty one in a LATER mapping rather than the first.
    let rows: Vec<Config> = vec![
        simple("an empty ProfileId", "fle-config-empty-profile", ""),
        simple(
            "a ProfileId of whitespace only",
            "fle-config-blank-profile",
            "   ",
        ),
        Config::new(
            "the second mapping states no profile",
            "fle-config-second-empty",
            Some("one mapping names a profile, the other none"),
            Some(Section::new(
                true,
                Some(vec![
                    QueryArgMapping::new("fle-profile", &profile),
                    QueryArgMapping::new("alt-profile", ""),
                ]),
            )),
            None,
        ),
    ];

    for row in rows {
        let refused = create_config(&router, &row.xml()).await;
        assert_eq!(
            refused.status, 400,
            "case {:?}: expected HTTP 400 for a mapping with no profile, got {} with {}",
            row.case, refused.status, refused.raw
        );
        assert_eq!(
            refused.error_code(),
            "QueryArgProfileEmpty",
            "case {:?}: expected QueryArgProfileEmpty, got {}",
            row.case,
            refused.raw
        );
        assert_eq!(
            refused.root().find_text("Message"),
            Some(QUERY_ARG_PROFILE_EMPTY),
            "case {:?}: expected the modelled message, got {:?}",
            row.case,
            refused.root().find_text("Message")
        );

        // Nothing was stored, so the reference the refusal named is still free: the same
        // request with the mapping completed is accepted under it.
        let completed = simple(&row.case, &row.caller_reference, &profile);
        let accepted = create_config(&router, &completed.xml()).await;
        assert_eq!(
            accepted.status, 201,
            "case {:?}: expected the completed request to be accepted under the same caller \
             reference, got {} with {}",
            row.case, accepted.status, accepted.raw
        );
    }
}

/// A caller reference identifies ONE configuration for its whole life: the same request again
/// reports the configuration the first create stored (minting nothing: the same id and the
/// same version), and a request that changes the configuration under a reference already taken
/// is refused (`FieldLevelEncryptionConfigAlreadyExists`, 409), leaving the stored
/// configuration exactly as it was.
#[tokio::test]
async fn a_caller_reference_names_at_most_one_configuration() {
    let router = emulator();
    let profile = store_profile(&router, "fle-config-reference-profile").await;

    for row in table(&profile) {
        let first = create_config(&router, &row.xml()).await;
        assert_eq!(
            first.status, 201,
            "case {:?}: expected HTTP 201 from the first create, got {} with {}",
            row.case, first.status, first.raw
        );
        let id = first.root().text_at("Id").unwrap_or_default().to_string();
        let etag = first.header("etag").to_string();

        // The same request again: the record the first create left behind, at its version.
        let repeat = create_config(&router, &row.xml()).await;
        assert_eq!(
            repeat.status, 201,
            "case {:?}: expected the replayed create to be answered, got {} with {}",
            row.case, repeat.status, repeat.raw
        );
        assert_eq!(
            repeat.root().text_at("Id"),
            Some(id.as_str()),
            "case {:?}: expected the replay to report the stored id {id:?}, got {:?}",
            row.case,
            repeat.root().text_at("Id")
        );
        assert_eq!(
            repeat.header("etag"),
            etag,
            "case {:?}: expected the replay at the stored version {etag:?}, got {:?}",
            row.case,
            repeat.header("etag")
        );

        // A DIFFERENT configuration under the same reference is a second configuration under
        // a reference already taken, which is refused.
        let mut changed = row.clone();
        changed.comment = Some("a different configuration under a taken reference".to_string());
        let refused = create_config(&router, &changed.xml()).await;
        assert_eq!(
            refused.status, 409,
            "case {:?}: expected HTTP 409 for a taken caller reference, got {} with {}",
            row.case, refused.status, refused.raw
        );
        assert_eq!(
            refused.error_code(),
            "FieldLevelEncryptionConfigAlreadyExists",
            "case {:?}: expected FieldLevelEncryptionConfigAlreadyExists, got {}",
            row.case,
            refused.raw
        );
        let message = refused.root().find_text("Message").unwrap_or_default();
        assert_eq!(
            message,
            "The caller reference that you are using to create a field level encryption config \
             is associated with another field level encryption config in your account.",
            "case {:?}: expected the service's fixed sentence for a taken caller reference, got \
             {message:?}",
            row.case
        );
        assert!(
            !message.contains(&id),
            "case {:?}: the sentence must not name the configuration holding the reference \
             ({id}), got {message:?}",
            row.case
        );

        // The refusal changed nothing: the stored configuration is still the one the first
        // create stored, at its version.
        let read = get_config(&router, &id).await;
        assert_eq!(
            read.status, 200,
            "case {:?}: expected the stored configuration to be readable after the refusal, got \
             {} with {}",
            row.case, read.status, read.raw
        );
        row.assert_matches(
            read.root()
                .at("FieldLevelEncryptionConfig")
                .unwrap_or_else(|| {
                    panic!("the read answers with the configuration: {}", read.raw)
                }),
            "after a refused second create",
            &read.raw,
        );
    }
}

/// "The comment cannot be longer than 128 characters": a comment within the bound is stored
/// and reported, and one past it is refused with the error the operation models for an
/// unusable argument (`InvalidArgument`, 400) before anything is minted — so the caller
/// reference the refused request named is still free.
#[tokio::test]
async fn a_comment_past_the_published_bound_is_refused() {
    let router = emulator();
    let profile = store_profile(&router, "fle-config-comment-profile").await;
    let bound = MAX_FIELD_LEVEL_ENCRYPTION_CONFIG_COMMENT;

    // The published bound, one inside it, and one past it — plus a multi-byte comment, so the
    // bound is counted in CHARACTERS rather than in bytes.
    let accepted: [(&str, String); 3] = [
        ("one character inside the bound", "a".repeat(bound - 1)),
        ("exactly on the bound", "b".repeat(bound)),
        ("multi-byte characters on the bound", "é".repeat(bound)),
    ];
    for (index, (case, comment)) in accepted.into_iter().enumerate() {
        let mut row = simple(case, &format!("fle-config-comment-ok-{index}"), &profile);
        row.comment = Some(comment);
        let answer = create_config(&router, &row.xml()).await;
        assert_eq!(
            answer.status, 201,
            "case {case:?}: expected HTTP 201 within the comment bound, got {} with {}",
            answer.status, answer.raw
        );
        let id = answer.root().text_at("Id").unwrap_or_default().to_string();
        let read = get_config(&router, &id).await;
        row.assert_matches(
            read.root()
                .at("FieldLevelEncryptionConfig")
                .unwrap_or_else(|| {
                    panic!("the read answers with the configuration: {}", read.raw)
                }),
            "the comment within the bound",
            &read.raw,
        );
    }

    let refused: [(&str, String); 3] = [
        ("one character past the bound", "a".repeat(bound + 1)),
        ("far past the bound", "b".repeat(bound * 4)),
        ("multi-byte characters past the bound", "é".repeat(bound + 1)),
    ];
    for (index, (case, comment)) in refused.into_iter().enumerate() {
        let reference = format!("fle-config-comment-long-{index}");
        let mut row = simple(case, &reference, &profile);
        row.comment = Some(comment);
        let answer = create_config(&router, &row.xml()).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400 past the comment bound, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "InvalidArgument",
            "case {case:?}: expected InvalidArgument, got {}",
            answer.raw
        );

        // Nothing was minted, so the reference the refusal named is still free: the same
        // request with an admissible comment is accepted under it.
        let mut shortened = simple(case, &reference, &profile);
        shortened.comment = Some("within the bound".to_string());
        let accepted = create_config(&router, &shortened.xml()).await;
        assert_eq!(
            accepted.status, 201,
            "case {case:?}: expected the shortened request to be accepted under the same caller \
             reference, got {} with {}",
            accepted.status, accepted.raw
        );
    }
}

/// Each mapping list is bounded: a configuration ON the bound is stored and reported, and one
/// past it is refused with the error the model names for that list
/// (`TooManyFieldLevelEncryptionQueryArgProfiles` /
/// `TooManyFieldLevelEncryptionContentTypeProfiles`, 400) before anything is minted.
#[tokio::test]
async fn a_mapping_list_past_its_bound_is_refused() {
    let router = emulator();
    let profile = store_profile(&router, "fle-config-bound-profile").await;

    // Each case is run on the bound itself and one past it, over both mapping lists.
    for (list, bound) in [
        (
            "QueryArgProfiles",
            MAX_FIELD_LEVEL_ENCRYPTION_QUERY_ARG_PROFILES,
        ),
        (
            "ContentTypeProfiles",
            MAX_FIELD_LEVEL_ENCRYPTION_CONTENT_TYPE_PROFILES,
        ),
    ] {
        for (case, count, expected_status) in [
            ("on the bound", bound, 201),
            ("one past the bound", bound + 1, 400),
        ] {
            let reference = format!("fle-config-{list}-{count}");
            let row = if list == "QueryArgProfiles" {
                Config::new(
                    case,
                    &reference,
                    None,
                    Some(Section::new(
                        true,
                        Some(
                            (0..count)
                                .map(|index| {
                                    QueryArgMapping::new(&format!("arg-{index}"), &profile)
                                })
                                .collect(),
                        ),
                    )),
                    None,
                )
            } else {
                Config::new(
                    case,
                    &reference,
                    None,
                    None,
                    Some(Section::new(
                        true,
                        Some(
                            (0..count)
                                .map(|index| {
                                    ContentTypeMapping::new(
                                        &format!("text/plain-{index}"),
                                        Some(&profile),
                                    )
                                })
                                .collect(),
                        ),
                    )),
                )
            };

            let answer = create_config(&router, &row.xml()).await;
            assert_eq!(
                answer.status, expected_status,
                "case {case:?} ({list}, {count} mappings): expected HTTP {expected_status}, got \
                 {} with {}",
                answer.status, answer.raw
            );
            if expected_status == 400 {
                let expected_code = format!("TooManyFieldLevelEncryption{list}");
                assert_eq!(
                    answer.error_code(),
                    expected_code,
                    "case {case:?} ({list}, {count} mappings): expected {expected_code}, got {}",
                    answer.raw
                );
            } else {
                let id = answer.root().text_at("Id").unwrap_or_default().to_string();
                let read = get_config(&router, &id).await;
                row.assert_matches(
                    read.root()
                        .at("FieldLevelEncryptionConfig")
                        .unwrap_or_else(|| {
                            panic!("the read answers with the configuration: {}", read.raw)
                        }),
                    "a list on its bound",
                    &read.raw,
                );
            }
        }
    }
}

/// An id no stored configuration carries names no configuration: the reader answers
/// `NoSuchFieldLevelEncryptionConfig` (404) whatever shape the id has, and the MESSAGE says
/// which condition it was — a value outside the shape a configuration id takes is the
/// identifier the service cannot read, a well-formed configuration id the store holds nothing
/// for is the absent configuration — and a stored configuration is still read at its own id.
#[tokio::test]
async fn an_id_no_configuration_holds_is_refused_by_the_reader() {
    let router = emulator();
    let profile = store_profile(&router, "fle-config-reader-profile").await;

    let stored = simple(
        "the one stored configuration",
        "fle-config-stored",
        &profile,
    );
    let created = create_config(&router, &stored.xml()).await;
    assert_eq!(
        created.status, 201,
        "case the one stored configuration: expected HTTP 201, got {} with {}",
        created.status, created.raw
    );
    let stored_id = created.root().text_at("Id").unwrap_or_default().to_string();

    // The rows span the ways an id can name no configuration: a well-shaped configuration id
    // that was never minted, an id of another CloudFront namespace (including the profile's,
    // which is a different resource under the same surface), and a value of no CloudFront id
    // shape at all — each with the sentence its shape earns.
    for (case, expected) in [
        ("C3V3PVOY5NSBVE", NO_SUCH_CONFIG_MESSAGE),
        ("EDFDVBD6EXAMPLE", ID_FORMAT_INVALID_MESSAGE),
        (profile.as_str(), ID_FORMAT_INVALID_MESSAGE),
        ("not-a-configuration-id", ID_FORMAT_INVALID_MESSAGE),
    ] {
        let answer = get_config(&router, case).await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected HTTP 404 for an unknown id, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "NoSuchFieldLevelEncryptionConfig",
            "case {case:?}: expected NoSuchFieldLevelEncryptionConfig, got {}",
            answer.raw
        );
        assert_eq!(
            answer.root().find_text("Message"),
            Some(expected),
            "case {case:?}: expected {expected:?}, got {:?}",
            answer.root().find_text("Message")
        );
    }

    let read = get_config(&router, &stored_id).await;
    assert_eq!(
        read.status, 200,
        "case the one stored configuration: expected it to remain readable at {stored_id}, got {} \
         with {}",
        read.status, read.raw
    );
}

/// The CONFIGURATION reader reports the stored `FieldLevelEncryptionConfig` document on its
/// own — the configuration as submitted, down to the members the create left out, at the
/// version token the create handed back and the same one `GetFieldLevelEncryption` is read at,
/// so the two readers of one record never disagree. The document carries only the submitted
/// configuration: the resource members (`Id`, `LastModifiedTime`) belong to the configuration
/// resource, not to its configuration document. Reading reports, so a second read answers
/// exactly the same and the resource read after it is untouched.
#[tokio::test]
async fn the_configuration_reader_reports_the_stored_configuration_document() {
    let router = emulator();
    let profile = store_profile(&router, "fle-config-document-profile").await;

    for row in table(&profile) {
        let created = create_config(&router, &row.xml()).await;
        assert_eq!(
            created.status, 201,
            "case {:?}: expected HTTP 201 from CreateFieldLevelEncryptionConfig, got {} with {}",
            row.case, created.status, created.raw
        );
        let id = created.root().text_at("Id").unwrap_or_default().to_string();
        let etag = created.header("etag").to_string();
        let last_modified = created
            .root()
            .text_at("LastModifiedTime")
            .unwrap_or_default()
            .to_string();

        let document = get_config_document(&router, &id).await;
        assert_eq!(
            document.status, 200,
            "case {:?}: expected HTTP 200 from GetFieldLevelEncryptionConfig, got {} with {}",
            row.case, document.status, document.raw
        );
        assert_eq!(
            document.header("etag"),
            etag,
            "case {:?}: expected the configuration read at the create's version {etag:?}, got {:?}",
            row.case,
            document.header("etag")
        );
        row.assert_matches(document.root(), "the configuration read", &document.raw);
        for member in ["Id", "LastModifiedTime"] {
            assert!(
                document.root().child(member).is_none(),
                "case {:?}: expected the configuration document to carry no {member}, got {}",
                row.case,
                document.raw
            );
        }

        // A report writes nothing: the second read answers exactly the same document at the
        // same version, and the resource read sees the record as the create left it.
        let again = get_config_document(&router, &id).await;
        assert_eq!(
            (again.status, again.raw.as_str(), again.header("etag")),
            (document.status, document.raw.as_str(), etag.as_str()),
            "case {:?}: expected the second configuration read to answer the same, got {} with {}",
            row.case,
            again.status,
            again.raw
        );
        let resource = get_config(&router, &id).await;
        assert_eq!(
            (
                resource.status,
                resource.root().text_at("Id"),
                resource.root().text_at("LastModifiedTime"),
                resource.header("etag")
            ),
            (
                200,
                Some(id.as_str()),
                Some(last_modified.as_str()),
                etag.as_str()
            ),
            "case {:?}: expected the configuration read to leave the record untouched, got {} \
             with {}",
            row.case,
            resource.status,
            resource.raw
        );
        row.assert_matches(
            resource
                .root()
                .at("FieldLevelEncryptionConfig")
                .unwrap_or_else(|| {
                    panic!(
                        "the resource read answers with the configuration: {}",
                        resource.raw
                    )
                }),
            "the resource read after the configuration read",
            &resource.raw,
        );
    }
}

/// An id no stored configuration carries names no configuration for the CONFIGURATION reader
/// either: it answers `NoSuchFieldLevelEncryptionConfig` (404) whatever shape the id has, with
/// the sentence that shape earns — the unreadable identifier for a value outside the shape a
/// configuration id takes, the absent configuration for a well-formed one — and the one stored
/// configuration's document is still read at its own id afterwards.
#[tokio::test]
async fn an_id_no_configuration_holds_is_refused_by_the_configuration_reader() {
    let router = emulator();
    let profile = store_profile(&router, "fle-config-document-reader-profile").await;

    let stored = simple(
        "the one stored configuration",
        "fle-config-document-stored",
        &profile,
    );
    let created = create_config(&router, &stored.xml()).await;
    assert_eq!(
        created.status, 201,
        "case the one stored configuration: expected HTTP 201, got {} with {}",
        created.status, created.raw
    );
    let stored_id = created.root().text_at("Id").unwrap_or_default().to_string();

    // The rows span the ways an id can name no configuration: a well-shaped configuration id
    // that was never minted, an id of another CloudFront namespace (including the profile's,
    // a different resource under the same surface), the configuration's own id with a
    // character changed, and a value of no CloudFront id shape at all.
    let mut mangled: Vec<char> = stored_id.chars().collect();
    if let Some(last) = mangled.last_mut() {
        *last = if *last == 'A' { 'B' } else { 'A' };
    }
    let mangled: String = mangled.into_iter().collect();
    for (case, expected) in [
        ("C3V3PVOY5NSBVE", NO_SUCH_CONFIG_MESSAGE),
        ("EDFDVBD6EXAMPLE", ID_FORMAT_INVALID_MESSAGE),
        (profile.as_str(), ID_FORMAT_INVALID_MESSAGE),
        (mangled.as_str(), NO_SUCH_CONFIG_MESSAGE),
        ("not-a-configuration-id", ID_FORMAT_INVALID_MESSAGE),
        ("0", ID_FORMAT_INVALID_MESSAGE),
    ] {
        let answer = get_config_document(&router, case).await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected HTTP 404 for an unknown id, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "NoSuchFieldLevelEncryptionConfig",
            "case {case:?}: expected NoSuchFieldLevelEncryptionConfig, got {}",
            answer.raw
        );
        assert_eq!(
            answer.root().find_text("Message"),
            Some(expected),
            "case {case:?}: expected {expected:?}, got {:?}",
            answer.root().find_text("Message")
        );
    }

    let document = get_config_document(&router, &stored_id).await;
    assert_eq!(
        document.status, 200,
        "case the one stored configuration: expected its document to remain readable at \
         {stored_id}, got {} with {}",
        document.status, document.raw
    );
    stored.assert_matches(
        document.root(),
        "the configuration read after the refusals",
        &document.raw,
    );
}
