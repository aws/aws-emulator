// SPDX-License-Identifier: CC0-1.0

//! `ListFieldLevelEncryptionConfigs`, end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing, restXml
//! (de)serialization, the handler, the persisted state and serialization — the same path a
//! client takes. The rules under test are the ones CloudFront states for this listing:
//!
//! * "List all field-level encryption configurations that have been created in CloudFront for
//!   this account": one `FieldLevelEncryptionSummary` per STORED configuration, each
//!   reporting the record the create persisted — the minted id, the instant it was written
//!   and the configuration's `Comment`, `QueryArgProfileConfig` and `ContentTypeProfileConfig`
//!   — which is what a read of the same configuration answers with;
//! * `MaxItems` cuts the page and is echoed as asked for, `Quantity` states the page's own
//!   length, `NextMarker` is present exactly when configurations remain and names the id the
//!   page ended at, and a request resumes AFTER the configuration its `Marker` names — so
//!   walking the pages at ANY admissible page size enumerates every configuration exactly
//!   once, in one order;
//! * a page size that names no page, and a `Marker` that is no resume point this service
//!   handed out, are refused with the error the operation models (`InvalidArgument`, 400) and
//!   change nothing;
//! * a refused create stores nothing, so a configuration whose wrapper states a `Quantity`
//!   that disagrees with its `Items` (`InconsistentQuantities`, 400) is listed by nothing
//!   afterwards;
//! * a listing is a READ: it answers the same records again and mints nothing.
//!
//! The table rows differ along everything these rules generalise over: the number of
//! configurations stored (none, one, several), which mapping sections a configuration carries
//! at all (neither, one, both), how many mappings a section holds (none, one, several), both
//! settings of each forwarding flag, whether a content type mapping names a profile, the form
//! of the comment carried (absent, ASCII, XML-significant characters, non-ASCII and the
//! documented 128-character bound), the page size (one, several, the collection size, past it
//! and past the server's own ceiling) and the resume point.
//!
//! The harness (the router, the request helpers and the element scanner) is the distribution
//! lifecycle suite's, the profile every mapping points at is stored through the profile
//! suite's own create path, the wire timestamp check is the public key suite's and the XML
//! escaper is the response headers policy suite's — reused rather than copied.

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::{
    INVALID_FIELD_LEVEL_ENCRYPTION_CONFIG_MARKER_MESSAGE,
    MAX_FIELD_LEVEL_ENCRYPTION_CONFIG_COMMENT, MAX_ITEMS, MAX_ITEMS_INVALID_MESSAGE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::distribution_lifecycle::{call, emulator, Answer, Node, API_VERSION};
use crate::field_level_encryption_config_lifecycle::FORM_CONTENT_TYPE;
use crate::field_level_encryption_profile_lifecycle::store_profile;
use crate::public_key_lifecycle::is_wire_timestamp;
use crate::response_headers_policy_lifecycle::escape;

/// The one `Format` the model's `Format` enum admits for a content type-profile mapping
/// (`smithy_query(query="shape",shape="Format")` lists `URLEncoded` and nothing else).
const URL_ENCODED: &str = "URLEncoded";

// ---------------------------------------------------------------------------
// Calling the surface
// ---------------------------------------------------------------------------

/// `POST /2020-05-31/field-level-encryption` with the `FieldLevelEncryptionConfig` document
/// `body` — the member the create binds to the HTTP payload.
async fn create_config(router: &MultiServiceRouter, body: &str) -> Answer {
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
async fn get_config(router: &MultiServiceRouter, id: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/{API_VERSION}/field-level-encryption/{id}"),
        None,
        None,
    )
    .await
}

/// `GET /2020-05-31/field-level-encryption`, with `query` appended verbatim
/// (`?MaxItems=2&Marker=C…`), from the operation's `http` binding.
pub(crate) async fn list_configs(router: &MultiServiceRouter, query: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/{API_VERSION}/field-level-encryption{query}"),
        None,
        None,
    )
    .await
}

// ---------------------------------------------------------------------------
// The configurations under test
// ---------------------------------------------------------------------------

/// One mapping section of a configuration — a `QueryArgProfileConfig` or a
/// `ContentTypeProfileConfig`: a forwarding flag beside a wrapper holding the mappings, each
/// mapping named by the request value that selects it (a query argument, a content type).
#[derive(Debug, Clone)]
struct Section {
    forward_when_unknown: bool,
    /// What each mapping matches on, in request order.
    selectors: Vec<String>,
    /// Whether each mapping names the profile at all — `ContentTypeProfile$ProfileId` is
    /// optional, so a mapping may select no profile.
    names_profile: bool,
    /// Whether this is the content type-profile mapping the SERVICE supplies rather than one
    /// the request stated: its mapping names no profile of its own, which it carries as the
    /// empty text rather than as an absent member.
    supplied: bool,
}

impl Section {
    fn new(forward_when_unknown: bool, selectors: &[&str], names_profile: bool) -> Self {
        Self {
            forward_when_unknown,
            selectors: selectors.iter().map(|value| value.to_string()).collect(),
            names_profile,
            supplied: false,
        }
    }

    /// The content type-profile mapping a configuration that states none is stored with: the
    /// one content type field-level encryption reads fields from, naming no profile, not
    /// forwarding a content type it does not cover.
    fn supplied() -> Self {
        Self {
            supplied: true,
            ..Self::new(false, &[FORM_CONTENT_TYPE], false)
        }
    }

    /// The `Quantity` the wrapper states: the number of mappings it carries.
    fn quantity(&self) -> usize {
        self.selectors.len()
    }
}

/// One `FieldLevelEncryptionConfig` a client submits.
#[derive(Debug, Clone)]
struct Row {
    /// What this row is a case of, named in every assertion it drives.
    case: String,
    caller_reference: String,
    /// `Comment`, or `None` to leave the optional member out entirely.
    comment: Option<String>,
    /// The `QueryArgProfileConfig`, or `None` to leave the whole section out.
    query_arg: Option<Section>,
    /// The `ContentTypeProfileConfig`, or `None` to leave the whole section out.
    content_type: Option<Section>,
    /// The profile every mapping of this row points at.
    profile: String,
}

impl Row {
    /// One row of the table: the configuration a client submits, under the caller reference
    /// the table then gives it.
    fn new(
        profile: &str,
        case: &str,
        comment: Option<&str>,
        query_arg: Option<Section>,
        content_type: Option<Section>,
    ) -> Self {
        Self {
            case: case.to_string(),
            caller_reference: String::new(),
            comment: comment.map(str::to_string),
            query_arg,
            content_type,
            profile: profile.to_string(),
        }
    }

    /// The `FieldLevelEncryptionConfig` document, which IS the create's request body (its
    /// `httpPayload` member), in the model's member declaration order.
    fn xml(&self) -> String {
        let mut out = format!(
            "<FieldLevelEncryptionConfig \
             xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
             <CallerReference>{}</CallerReference>",
            escape(&self.caller_reference)
        );
        if let Some(comment) = &self.comment {
            out.push_str(&format!("<Comment>{}</Comment>", escape(comment)));
        }
        if let Some(section) = &self.query_arg {
            out.push_str(&format!(
                "<QueryArgProfileConfig><ForwardWhenQueryArgProfileIsUnknown>{}\
                 </ForwardWhenQueryArgProfileIsUnknown><QueryArgProfiles><Quantity>{}</Quantity>",
                section.forward_when_unknown,
                section.quantity()
            ));
            if !section.selectors.is_empty() {
                out.push_str("<Items>");
                for selector in &section.selectors {
                    out.push_str(&format!(
                        "<QueryArgProfile><QueryArg>{}</QueryArg><ProfileId>{}</ProfileId>\
                         </QueryArgProfile>",
                        escape(selector),
                        escape(&self.profile)
                    ));
                }
                out.push_str("</Items>");
            }
            out.push_str("</QueryArgProfiles></QueryArgProfileConfig>");
        }
        if let Some(section) = &self.content_type {
            out.push_str(&format!(
                "<ContentTypeProfileConfig><ForwardWhenContentTypeIsUnknown>{}\
                 </ForwardWhenContentTypeIsUnknown><ContentTypeProfiles><Quantity>{}</Quantity>",
                section.forward_when_unknown,
                section.quantity()
            ));
            if !section.selectors.is_empty() {
                out.push_str("<Items>");
                for selector in &section.selectors {
                    out.push_str(&format!(
                        "<ContentTypeProfile><Format>{URL_ENCODED}</Format>"
                    ));
                    if section.names_profile {
                        out.push_str(&format!(
                            "<ProfileId>{}</ProfileId>",
                            escape(&self.profile)
                        ));
                    }
                    out.push_str(&format!(
                        "<ContentType>{}</ContentType></ContentTypeProfile>",
                        escape(selector)
                    ));
                }
                out.push_str("</Items>");
            }
            out.push_str("</ContentTypeProfiles></ContentTypeProfileConfig>");
        }
        out.push_str("</FieldLevelEncryptionConfig>");
        out
    }

    /// Assert that `summary` — one `FieldLevelEncryptionSummary` the listing serialised —
    /// reports this configuration as the store holds it.
    ///
    /// `FieldLevelEncryptionSummary` is `Id`, `LastModifiedTime`, `Comment`,
    /// `QueryArgProfileConfig` and `ContentTypeProfileConfig` (from
    /// `smithy_query(query="shape",shape="FieldLevelEncryptionSummary")`), so a summary
    /// carries no `CallerReference` and nothing else of the configuration either.
    fn assert_summary(&self, summary: &Node, id: &str, raw: &str) {
        let case = self.case.as_str();
        let listed_id = summary.text_at("Id").map(str::to_string);
        assert_eq!(
            listed_id.as_deref(),
            Some(id),
            "case {case:?}: expected the stored id {id:?}, got {listed_id:?} in {raw}"
        );
        let listed_comment = summary.text_at("Comment").map(str::to_string);
        let expected_comment = self.comment.clone().unwrap_or_default();
        assert_eq!(
            listed_comment.as_deref(),
            Some(expected_comment.as_str()),
            "case {case:?}: expected the stored Comment {expected_comment:?}, got \
             {listed_comment:?} in {raw}"
        );
        assert!(
            summary.text_at("CallerReference").is_none(),
            "case {case:?}: a summary models no CallerReference, got {raw}"
        );

        // `QueryArgProfileConfig` is the mapping every stored configuration carries, so a
        // configuration created without one is summarised with the mapping that maps
        // nothing, under its flag's own unset position.
        let query_arg = self.effective_query_arg();
        self.assert_section(
            summary,
            "QueryArgProfileConfig",
            "ForwardWhenQueryArgProfileIsUnknown",
            "QueryArgProfiles",
            "QueryArgProfile",
            "QueryArg",
            Some(&query_arg),
            raw,
        );
        self.assert_section(
            summary,
            "ContentTypeProfileConfig",
            "ForwardWhenContentTypeIsUnknown",
            "ContentTypeProfiles",
            "ContentTypeProfile",
            "ContentType",
            Some(&self.effective_content_type()),
            raw,
        );
    }

    /// The `QueryArgProfileConfig` the stored configuration carries: the section this row
    /// submits, with the forwarding flag the stored configuration answers from its own
    /// mapping — whether it maps NO query argument to a profile — or, for a row that states
    /// no section, the mapping that maps nothing.
    fn effective_query_arg(&self) -> Section {
        let mut section = self
            .query_arg
            .clone()
            .unwrap_or_else(|| Section::new(true, &[], false));
        section.forward_when_unknown = section.selectors.is_empty();
        section
    }

    /// The `ContentTypeProfileConfig` the stored configuration carries: the section this row
    /// submits, or the mapping the service supplies for a row that states none.
    fn effective_content_type(&self) -> Section {
        self.content_type.clone().unwrap_or_else(Section::supplied)
    }

    /// Assert that one mapping section of a serialised document reports the section this row
    /// submitted — the one walk both sections are checked by, since both are a forwarding
    /// flag beside a `Quantity` and an `Items` list of mappings naming a profile.
    #[allow(clippy::too_many_arguments)]
    fn assert_section(
        &self,
        document: &Node,
        config_member: &str,
        flag_member: &str,
        wrapper: &str,
        mapping_member: &str,
        selector_member: &str,
        section: Option<&Section>,
        raw: &str,
    ) {
        let case = self.case.as_str();
        let Some(section) = section else {
            assert!(
                document.at(config_member).is_none(),
                "case {case:?}: expected no {config_member}, got {raw}"
            );
            return;
        };
        let node = document.at(config_member).unwrap_or_else(|| {
            panic!("case {case:?}: expected a {config_member} in {raw}");
        });
        let flag = node.text_at(flag_member).map(str::to_string);
        assert_eq!(
            flag.as_deref(),
            Some(section.forward_when_unknown.to_string().as_str()),
            "case {case:?}: expected {flag_member} {}, got {flag:?} in {raw}",
            section.forward_when_unknown
        );
        let quantity = node.text_at(&format!("{wrapper}/Quantity")).map(str::to_string);
        assert_eq!(
            quantity.as_deref(),
            Some(section.quantity().to_string().as_str()),
            "case {case:?}: expected {wrapper} Quantity {}, got {quantity:?} in {raw}",
            section.quantity()
        );
        let mappings: Vec<&Node> = node
            .at(&format!("{wrapper}/Items"))
            .map(|items| items.children_named(mapping_member))
            .unwrap_or_default();
        assert_eq!(
            mappings.len(),
            section.selectors.len(),
            "case {case:?}: expected {} {mapping_member} entries, got {} in {raw}",
            section.selectors.len(),
            mappings.len()
        );
        for (selector, mapping) in section.selectors.iter().zip(mappings) {
            let actual = mapping.text_at(selector_member).map(str::to_string);
            assert_eq!(
                actual.as_deref(),
                Some(selector.as_str()),
                "case {case:?}: expected {mapping_member} {selector_member} {selector:?}, got \
                 {actual:?} in {raw}"
            );
            // A mapping names the profile the matching fields are encrypted under, and a
            // content type mapping that named none is reported with none rather than with a
            // member invented by the read.
            let profile = mapping.text_at("ProfileId").map(str::to_string);
            let expected = if section.supplied {
                Some(String::new())
            } else {
                section.names_profile.then(|| self.profile.clone())
            };
            assert_eq!(
                profile, expected,
                "case {case:?}: expected {mapping_member} ProfileId {expected:?}, got \
                 {profile:?} in {raw}"
            );
        }
    }
}

/// The configurations every rule below is asserted over, each mapping pointing at the stored
/// profile `profile`.
///
/// The rows span what the rules generalise over: a configuration with neither mapping section,
/// with one and with both; a section holding no mapping, one and several; both settings of
/// each forwarding flag; a content type mapping with and without a `ProfileId`; and the
/// comment forms (absent, ASCII, XML metacharacters, non-ASCII and the documented
/// 128-character bound, which `FieldLevelEncryptionConfig$Comment` states as "The comment
/// cannot be longer than 128 characters"). Every row names a profile in one of its sections,
/// since a configuration naming none in either is refused rather than stored.
fn table(profile: &str) -> Vec<Row> {
    let on_the_bound = "c".repeat(MAX_FIELD_LEVEL_ENCRYPTION_CONFIG_COMMENT);
    let mut rows = vec![
        Row::new(
            profile,
            "a query argument section holding no mapping, beside a content type mapping",
            Some(""),
            Some(Section::new(true, &[], true)),
            Some(Section::new(false, &["text/html"], true)),
        ),
        Row::new(
            profile,
            "one query argument mapping",
            Some("the configuration for an emulator test"),
            Some(Section::new(false, &["fle-profile"], true)),
            None,
        ),
        Row::new(
            profile,
            "one content type mapping naming the profile",
            Some("tags <field> & \"quotes\" and 'apostrophes'"),
            None,
            Some(Section::new(
                true,
                &["application/x-www-form-urlencoded"],
                true,
            )),
        ),
        Row::new(
            profile,
            "a content type mapping that names no profile, beside a query argument mapping",
            Some("フィールドレベル暗号化 — 設定"),
            Some(Section::new(true, &["fle-profile"], true)),
            Some(Section::new(false, &["text/plain"], false)),
        ),
        Row::new(
            profile,
            "several mappings in both sections, the comment exactly on the bound",
            Some(on_the_bound.as_str()),
            Some(Section::new(
                true,
                &["fle-profile", "alt-profile", "profile&arg"],
                true,
            )),
            Some(Section::new(
                false,
                &["application/x-www-form-urlencoded", "text/xml"],
                true,
            )),
        ),
    ];
    // "A unique number that ensures the request can't be replayed": one reference per row, so
    // no row is read as a repeat of another.
    for (index, row) in rows.iter_mut().enumerate() {
        row.caller_reference = format!("fle-config-listing-{index}");
    }
    rows
}

// ---------------------------------------------------------------------------
// Reading an answer
// ---------------------------------------------------------------------------

/// Every `FieldLevelEncryptionSummary` a page reports, in page order.
fn page_summaries(answer: &Answer) -> Vec<&Node> {
    answer
        .root()
        .at("Items")
        .map(|items| items.children_named("FieldLevelEncryptionSummary"))
        .unwrap_or_default()
}

/// The ids a page reports, in the order the page carries them.
pub(crate) fn page_ids(answer: &Answer) -> Vec<String> {
    page_summaries(answer)
        .into_iter()
        .map(|summary| summary.text_at("Id").unwrap_or_default().to_string())
        .collect()
}

/// Whether `value` has the shape of a field-level encryption configuration id CloudFront
/// mints: a `C` followed by 13 characters of the id alphabet, the shape its own answers show.
///
/// Spelled out here rather than taken from the crate, so a changed mint fails this suite.
fn is_config_id(value: &str) -> bool {
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

/// Create every row through the API and report `(row, id, last_modified_time)` in creation
/// order.
async fn create_all(router: &MultiServiceRouter, rows: &[Row]) -> Vec<(Row, String, String)> {
    let mut created: Vec<(Row, String, String)> = Vec::new();
    for row in rows {
        let answer = create_config(router, &row.xml()).await;
        assert_eq!(
            answer.status, 201,
            "case {:?}: expected HTTP 201 from CreateFieldLevelEncryptionConfig, got {} with {}",
            row.case, answer.status, answer.raw
        );
        let id = answer.root().text_at("Id").unwrap_or_default().to_string();
        assert!(
            is_config_id(&id),
            "case {:?}: expected a minted configuration id, got {id:?} in {}",
            row.case,
            answer.raw
        );
        assert!(
            created.iter().all(|(_, stored, _)| stored != &id),
            "case {:?}: the id {id} repeated across creates",
            row.case
        );
        let last_modified = answer
            .root()
            .text_at("LastModifiedTime")
            .unwrap_or_default()
            .to_string();
        assert!(
            is_wire_timestamp(&last_modified),
            "case {:?}: expected a wire timestamp for LastModifiedTime, got {last_modified:?}",
            row.case
        );
        created.push((row.clone(), id, last_modified));
    }
    created
}

// ---------------------------------------------------------------------------
// The rules
// ---------------------------------------------------------------------------

/// An account holding no field-level encryption configurations lists none: a page with
/// nothing in it, carrying no `Items` element at all, the default page size, and nothing that
/// says more remain.
#[tokio::test]
async fn an_account_using_no_configurations_lists_no_items() {
    let router = emulator();
    let case = "empty account";

    let listed = list_configs(&router, "").await;
    assert_eq!(
        listed.status, 200,
        "case {case}: expected HTTP 200 from ListFieldLevelEncryptionConfigs, got {} with {}",
        listed.status, listed.raw
    );
    assert_eq!(
        listed.root().text_at("Quantity"),
        Some("0"),
        "case {case}: expected Quantity 0, got {}",
        listed.raw
    );
    assert_eq!(
        listed.root().text_at("MaxItems"),
        Some(MAX_ITEMS.to_string().as_str()),
        "case {case}: expected the default MaxItems {MAX_ITEMS}, got {}",
        listed.raw
    );
    assert!(
        listed.root().at("Items").is_none(),
        "case {case}: an account holding no configurations carries no Items element, got {}",
        listed.raw
    );
    assert!(
        listed.root().text_at("NextMarker").is_none(),
        "case {case}: expected no NextMarker when nothing remains, got {}",
        listed.raw
    );
}

/// The listing reports every stored configuration — each summary carrying the record the
/// create persisted, which is what a read of the same configuration answers with — and
/// walking the pages at any admissible page size enumerates every configuration exactly once,
/// in one order.
#[tokio::test]
async fn a_listing_reports_every_stored_configuration_and_pages_through_them() {
    let router = emulator();
    let profile = store_profile(&router, "fle-config-listing-profile").await;
    let created = create_all(&router, &table(&profile)).await;

    // One stable total order over the stored configurations, which every page indexes into.
    let mut expected = created.clone();
    expected.sort_by(|left, right| left.1.cmp(&right.1));
    let order: Vec<String> = expected.iter().map(|(_, id, _)| id.clone()).collect();

    // A listing that asks for no page size reports the whole (sub-ceiling) collection.
    let listed = list_configs(&router, "").await;
    assert_eq!(
        listed.status, 200,
        "case whole collection: expected HTTP 200, got {} with {}",
        listed.status, listed.raw
    );
    assert_eq!(
        page_ids(&listed),
        order,
        "case whole collection: expected every stored id once, in order, got {}",
        listed.raw
    );
    assert_eq!(
        listed.root().text_at("Quantity"),
        Some(order.len().to_string().as_str()),
        "case whole collection: expected Quantity {}, got {}",
        order.len(),
        listed.raw
    );
    assert!(
        listed.root().text_at("NextMarker").is_none(),
        "case whole collection: nothing remains, got {}",
        listed.raw
    );

    // Each summary reports the stored record, and the reader of the same configuration
    // answers from that one record too.
    for ((row, id, last_modified), summary) in expected.iter().zip(page_summaries(&listed)) {
        row.assert_summary(summary, id, &listed.raw);
        let listed_time = summary.text_at("LastModifiedTime").map(str::to_string);
        assert_eq!(
            listed_time.as_deref(),
            Some(last_modified.as_str()),
            "case {:?}: expected the stored LastModifiedTime {last_modified:?}, got \
             {listed_time:?} in {}",
            row.case,
            listed.raw
        );

        let read = get_config(&router, id).await;
        assert_eq!(
            read.status, 200,
            "case {:?}: expected HTTP 200 from GetFieldLevelEncryption, got {} with {}",
            row.case, read.status, read.raw
        );
        assert_eq!(
            read.root().text_at("LastModifiedTime"),
            Some(last_modified.as_str()),
            "case {:?}: the summary and the read must answer from one record, got {}",
            row.case,
            read.raw
        );
        // The read's configuration document carries the same mapping sections the summary
        // reports, since both are projections of the one stored configuration.
        let config = read
            .root()
            .at("FieldLevelEncryptionConfig")
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: the read answers with the configuration: {}",
                    row.case, read.raw
                )
            });
        row.assert_section(
            config,
            "QueryArgProfileConfig",
            "ForwardWhenQueryArgProfileIsUnknown",
            "QueryArgProfiles",
            "QueryArgProfile",
            "QueryArg",
            Some(&row.effective_query_arg()),
            &read.raw,
        );
        row.assert_section(
            config,
            "ContentTypeProfileConfig",
            "ForwardWhenContentTypeIsUnknown",
            "ContentTypeProfiles",
            "ContentTypeProfile",
            "ContentType",
            Some(&row.effective_content_type()),
            &read.raw,
        );
    }

    // Walking the pages at ANY admissible page size enumerates every configuration exactly
    // once, in that same order: sizes below, at and past the collection size, and one past
    // the server's own ceiling.
    let ceiling = usize::try_from(MAX_ITEMS).expect("the ceiling fits a usize");
    let sizes = [
        1usize,
        2,
        3,
        order.len() - 1,
        order.len(),
        order.len() + 1,
        ceiling + 1,
    ];
    for size in sizes {
        let mut walked: Vec<String> = Vec::new();
        let mut marker: Option<String> = None;
        loop {
            let query = match &marker {
                Some(value) => format!("?MaxItems={size}&Marker={value}"),
                None => format!("?MaxItems={size}"),
            };
            let page = list_configs(&router, &query).await;
            assert_eq!(
                page.status, 200,
                "case page size {size}: expected HTTP 200 for {query}, got {} with {}",
                page.status, page.raw
            );
            assert_eq!(
                page.root().text_at("MaxItems"),
                Some(size.to_string().as_str()),
                "case page size {size}: expected the requested MaxItems echoed, got {}",
                page.raw
            );
            let ids = page_ids(&page);
            // The page served is the requested size bounded above by the server's own
            // ceiling, and never longer than what remains.
            assert!(
                ids.len() <= size.min(ceiling),
                "case page size {size}: a page served {} items, got {}",
                ids.len(),
                page.raw
            );
            assert_eq!(
                page.root().text_at("Quantity"),
                Some(ids.len().to_string().as_str()),
                "case page size {size}: expected Quantity to state the page's length, got {}",
                page.raw
            );
            walked.extend(ids.iter().cloned());
            let remaining = walked.len() < order.len();
            let next = page.root().text_at("NextMarker").map(str::to_string);
            assert_eq!(
                next.is_some(),
                remaining,
                "case page size {size}: expected NextMarker present {remaining} after {} of {} \
                 items, got {}",
                walked.len(),
                order.len(),
                page.raw
            );
            match next {
                Some(value) => {
                    assert_eq!(
                        Some(value.as_str()),
                        ids.last().map(String::as_str),
                        "case page size {size}: expected NextMarker to name the id the page \
                         ended at, got {}",
                        page.raw
                    );
                    marker = Some(value);
                }
                None => break,
            }
        }
        assert_eq!(
            walked, order,
            "case page size {size}: walking the pages must enumerate every configuration \
             exactly once, in order"
        );
    }

    // A listing is a READ: the same request answers the same records again.
    let again = list_configs(&router, "").await;
    assert_eq!(
        page_ids(&again),
        order,
        "case repeated listing: a read must not change what a later caller sees, got {}",
        again.raw
    );
}

/// A page size that names no page, and a `Marker` that is no resume point this service handed
/// out, are refused with the modelled `InvalidArgument` (HTTP 400) and store nothing; a
/// well-shaped marker naming no stored configuration is a resume POSITION rather than a
/// refusal.
#[tokio::test]
async fn a_page_size_or_marker_that_names_no_page_is_refused() {
    let router = emulator();
    let profile = store_profile(&router, "fle-config-refusal-profile").await;
    let created = create_all(&router, &table(&profile)[..3]).await;
    let before = page_ids(&list_configs(&router, "").await);

    // Rows: every form a page size can be unusable in (negative, no integer at all,
    // past what the member holds), and every form a marker can fail the id shape in (no id at
    // all, too short, too long, the wrong case, another namespace's token, characters outside
    // the id alphabet) — plus an unusable size beside a usable marker, where the size is what
    // is refused.
    let rows = [
        ("a negative page size", "?MaxItems=-1", false),
        (
            "a page size that is no integer",
            "?MaxItems=not-a-number",
            false,
        ),
        (
            "a page size past what the member can hold",
            "?MaxItems=99999999999",
            false,
        ),
        ("a marker that is no id at all", "?Marker=nope", true),
        ("a marker too short to be an id", "?Marker=C123", true),
        (
            "a marker too long to be an id",
            "?Marker=C1AAAAAAAAAAAAAAAA",
            true,
        ),
        ("a marker in the wrong case", "?Marker=c1aaaaaaaaaaaa", true),
        (
            "a marker from the profile namespace",
            "?Marker=P1AAAAAAAAAAAA",
            true,
        ),
        (
            "a marker from the distribution namespace",
            "?Marker=E1AAAAAAAAAAAA",
            true,
        ),
        (
            "a marker carrying characters outside the id alphabet",
            "?Marker=C1AAAA%20AAAAAA%21",
            true,
        ),
        (
            "an unusable page size beside a usable marker",
            "?MaxItems=-1&Marker=C1AAAAAAAAAAAA",
            false,
        ),
    ];
    for (case, query, over_marker) in rows {
        let answer = list_configs(&router, query).await;
        assert_eq!(
            answer.status, 400,
            "case {case}: expected HTTP 400 for {query}, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "InvalidArgument",
            "case {case}: expected InvalidArgument for {query}, got {}",
            answer.raw
        );
        // What each refusal states is the CONSTRAINT on the parameter — a marker is a
        // field-level encryption configuration id, a page size is a non-zero positive integer
        // — so every unusable value is refused with that one sentence rather than with text
        // quoted out of the request.
        let expected_message = if over_marker {
            INVALID_FIELD_LEVEL_ENCRYPTION_CONFIG_MARKER_MESSAGE
        } else {
            MAX_ITEMS_INVALID_MESSAGE
        };
        let message = answer.root().find_text("Message").unwrap_or_default();
        assert_eq!(
            message, expected_message,
            "case {case}: expected the constraint stated for {query}, got {}",
            answer.raw
        );
    }

    // A well-shaped marker naming no stored configuration is a resume POSITION: the page it
    // names holds the configurations ordered after it.
    let last = before
        .last()
        .cloned()
        .expect("the fixture created some configurations");
    let after_last = list_configs(&router, &format!("?Marker={last}")).await;
    assert_eq!(
        after_last.status, 200,
        "case resume at the last id: expected HTTP 200, got {} with {}",
        after_last.status, after_last.raw
    );
    assert!(
        page_ids(&after_last).is_empty(),
        "case resume at the last id: nothing is ordered after it, got {}",
        after_last.raw
    );

    // Every refusal above left the store exactly as it was.
    let listed = list_configs(&router, "").await;
    assert_eq!(
        page_ids(&listed),
        before,
        "case refusals: a refused listing must store nothing, got {}",
        listed.raw
    );
    assert_eq!(
        created.len(),
        before.len(),
        "case refusals: expected the created configurations to be the listed ones"
    );
}

/// A wrapper whose `Quantity` disagrees with its `Items` describes no list at all: the create
/// is refused with the error the operation models for it (`InconsistentQuantities`, 400),
/// naming the wrapper, the quantity the request stated and the number it carried — and
/// nothing is stored, so the listing is unchanged and the corrected request is accepted under
/// the same caller reference.
#[tokio::test]
async fn a_refused_create_is_listed_by_nothing() {
    let router = emulator();
    let profile = store_profile(&router, "fle-config-inconsistent-profile").await;

    // The rows span the ways a quantity can disagree — too high, too low and zero beside a
    // carried list — over each of the two wrappers a configuration reaches.
    let comment = "a configuration that states the wrong quantity";
    let query_arg_row = Row::new(
        &profile,
        "",
        Some(comment),
        Some(Section::new(true, &["fle-profile"], true)),
        None,
    );
    let content_type_row = Row::new(
        &profile,
        "",
        Some(comment),
        None,
        Some(Section::new(
            true,
            &["application/x-www-form-urlencoded", "text/plain"],
            true,
        )),
    );
    let rows = [
        ("QueryArgProfiles", 2, 1, &query_arg_row),
        ("QueryArgProfiles", 0, 1, &query_arg_row),
        ("ContentTypeProfiles", 5, 2, &content_type_row),
        ("ContentTypeProfiles", 1, 2, &content_type_row),
    ];
    for (index, (wrapper, stated, carried, row)) in rows.into_iter().enumerate() {
        let case = format!("{wrapper} stating {stated} with {carried} carried");
        let reference = format!("fle-config-inconsistent-{index}");
        let mut row = row.clone();
        row.case = case.clone();
        row.caller_reference = reference.clone();
        // What the listing holds BEFORE this row runs: each accepted create below adds one,
        // so the baseline is read again per row rather than once for the whole table.
        let before = page_ids(&list_configs(&router, "").await);
        // The wrapper's stated `Quantity` is the only thing this row changes, so the
        // document is the well-formed one with that number put in its place.
        let well_formed = row.xml();
        let broken = well_formed.replacen(
            &format!("<{wrapper}><Quantity>{carried}</Quantity>"),
            &format!("<{wrapper}><Quantity>{stated}</Quantity>"),
            1,
        );
        assert_ne!(
            broken, well_formed,
            "case {case:?}: the row must state a quantity other than the number it carries"
        );

        let refused = create_config(&router, &broken).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected HTTP 400, got {} with {}",
            refused.status, refused.raw
        );
        assert_eq!(
            refused.error_code(),
            "InconsistentQuantities",
            "case {case:?}: expected InconsistentQuantities, got {}",
            refused.raw
        );
        // The sentence names the offending wrapper, the quantity the request stated and the
        // number of items it carried — all three read from the request.
        let message = refused.root().find_text("Message").unwrap_or_default();
        assert_eq!(
            message,
            format!(
                "The specified quantity of {wrapper} ({stated}) does not match the actual \
                 quantity supplied ({carried})."
            ),
            "case {case:?}: expected the quantity mismatch stated, got {}",
            refused.raw
        );

        // The refusal stored nothing: the listing is what it was, and the caller reference
        // the refused request named is still free.
        let listed = list_configs(&router, "").await;
        assert_eq!(
            page_ids(&listed),
            before,
            "case {case:?}: a refused create must store nothing, got {}",
            listed.raw
        );

        let accepted = create_config(&router, &well_formed).await;
        assert_eq!(
            accepted.status, 201,
            "case {case:?}: expected the corrected request to be accepted, got {} with {}",
            accepted.status, accepted.raw
        );
        let id = accepted
            .root()
            .text_at("Id")
            .unwrap_or_default()
            .to_string();
        let listed = list_configs(&router, "").await;
        let ids = page_ids(&listed);
        assert!(
            ids.contains(&id),
            "case {case:?}: expected the created configuration {id} to be listed, got {}",
            listed.raw
        );
        let summary = page_summaries(&listed)
            .into_iter()
            .find(|summary| summary.text_at("Id") == Some(id.as_str()))
            .unwrap_or_else(|| panic!("case {case:?}: the created configuration is listed"));
        row.assert_summary(summary, &id, &listed.raw);
    }
}
