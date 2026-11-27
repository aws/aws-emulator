// SPDX-License-Identifier: CC0-1.0

//! The CloudFront origin access control control plane, end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing, restXml
//! (de)serialization, the handler, the persisted state and serialization — the same path
//! a client takes. The rules under test are the ones CloudFront's origin access control
//! surface states:
//!
//! * a control is PERSISTED by the call that creates it, which MINTS its identity (an id
//!   and a version token), so a later `GetOriginAccessControl` and every
//!   `ListOriginAccessControls` page answer from that record rather than from the request
//!   being served — down to the members the create left out;
//! * `ListOriginAccessControls` "gets the list of CloudFront origin access controls (OACs)
//!   in this Amazon Web Services account": one summary per stored control, reporting what
//!   the same record answers a get with;
//! * `MaxItems` cuts the page, `NextMarker` is present exactly when controls remain and
//!   names the id the page ended at, and a request resumes after the control its `Marker`
//!   names — so walking the pages at ANY admissible page size enumerates every control
//!   exactly once, in one order;
//! * "If you're not using origin access controls for your Amazon Web Services account, the
//!   ListOriginAccessControls operation doesn't return the Items element in the response";
//! * a page size that names no page and a `Marker` that is no resume point this service
//!   handed out are refused with the error the operation models (`InvalidArgument` 400),
//!   and change nothing;
//! * "An origin access control with the same name already exists" — a create under the
//!   `Name` of a stored control is refused (409) and leaves it untouched, whichever other
//!   member it differs in, while one under a free name describes a different control;
//! * an id no control holds is `NoSuchOriginAccessControl` (404);
//! * `OriginAccessControlConfig$Name`: "You can specify up to 64 characters", so a longer
//!   name is refused before anything is stored;
//! * the version token the create MINTS ("the version identifier for the current version
//!   of the origin access control") is the one every later read of that control reports,
//!   and the `Location` the create reports is the URI that control is read at — a read
//!   mints nothing and changes nothing a later caller sees;
//! * "The number of origin access controls in your Amazon Web Services account exceeds the
//!   maximum allowed" — a create that would take the account past its origin access
//!   control quota is refused with `TooManyOriginAccessControls` and mints nothing;
//! * a read of an `Id` this account holds no control under answers `NoSuchOriginAccessControl`
//!   and ONE sentence — the control that is absent — whatever shape the value takes, since the
//!   read models no argument error to report an identifier under.
//!
//! The table rows differ along everything these rules generalise over: every value of
//! every enum the configuration carries (`SigningProtocol`, `SigningBehavior` and
//! `OriginAccessControlOriginType`, from `smithy_query` by way of the generated
//! `ORIGIN_ACCESS_CONTROL_*_VALUES` catalogs), whether a description is carried at all and
//! whether it is empty, ASCII or XML metacharacters, and the published name bound, one
//! inside it and one past it.
//!
//! The harness (the router, the request helpers and the element scanner) is the
//! distribution lifecycle suite's, reused rather than copied.

use std::collections::BTreeSet;

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::ops::types::{
    ORIGIN_ACCESS_CONTROL_ORIGIN_TYPES_VALUES, ORIGIN_ACCESS_CONTROL_SIGNING_BEHAVIORS_VALUES,
    ORIGIN_ACCESS_CONTROL_SIGNING_PROTOCOLS_VALUES,
};
use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::{
    MAX_ITEMS, MAX_ITEMS_ORIGIN_ACCESS_CONTROLS, MAX_ORIGIN_ACCESS_CONTROLS,
    MAX_ORIGIN_ACCESS_CONTROL_NAME,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::distribution_lifecycle::{
    call, emulator, is_distribution_id, Answer, Node, API_VERSION, HOST,
};
// The XML escaper and the entity-id shape check are the crate's: one escaper and one id
// predicate for every suite, so no two of them can disagree about either.
use crate::response_headers_policy_lifecycle::escape;

/// The sentence this surface answers an id it holds no control under, spelled out here rather
/// than read from the crate, so a reworded refusal fails this suite.
const NO_SUCH_CONTROL_MESSAGE: &str = "The specified origin access control does not exist.";

/// The sentence it answers a value written in no alphabet CloudFront mints an identifier in,
/// under the same code and status — spelled out here for the same reason.
const UNREADABLE_IDENTIFIER_MESSAGE: &str = "Id format is invalid";

// ---------------------------------------------------------------------------
// Calling the origin access control surface
// ---------------------------------------------------------------------------

/// `POST /2020-05-31/origin-access-control` with the `OriginAccessControlConfig` document
/// `body` — the member the operation binds to the HTTP payload.
pub(crate) async fn create_control(router: &MultiServiceRouter, body: &str) -> Answer {
    call(
        router,
        "POST",
        &format!("/{API_VERSION}/origin-access-control"),
        Some(body),
        None,
    )
    .await
}

/// `GET /2020-05-31/origin-access-control/{Id}`.
pub(crate) async fn get_control(router: &MultiServiceRouter, id: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/{API_VERSION}/origin-access-control/{id}"),
        None,
        None,
    )
    .await
}

/// `GET /2020-05-31/origin-access-control`, with `query` appended verbatim
/// (`?MaxItems=2&Marker=E…`), from the operation's `http` binding.
pub(crate) async fn list_controls(router: &MultiServiceRouter, query: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/{API_VERSION}/origin-access-control{query}"),
        None,
        None,
    )
    .await
}

// ---------------------------------------------------------------------------
// The configurations under test
// ---------------------------------------------------------------------------

/// One `OriginAccessControlConfig`, as a client submits it.
#[derive(Debug, Clone)]
pub(crate) struct Control {
    /// What this row is a case of, named in every assertion it drives.
    pub(crate) case: String,
    name: String,
    /// `Description`, or `None` to leave the optional member out entirely.
    description: Option<String>,
    /// A value of `OriginAccessControlSigningProtocols`.
    signing_protocol: String,
    /// A value of `OriginAccessControlSigningBehaviors`.
    signing_behavior: String,
    /// A value of `OriginAccessControlOriginTypes`.
    origin_type: String,
}

impl Control {
    /// The `OriginAccessControlConfig` XML a client sends for this configuration, in the
    /// model's member declaration order.
    pub(crate) fn xml(&self) -> String {
        let mut out = format!(
            "<OriginAccessControlConfig \
             xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">"
        );
        out.push_str(&format!("<Name>{}</Name>", escape(&self.name)));
        if let Some(description) = &self.description {
            out.push_str(&format!(
                "<Description>{}</Description>",
                escape(description)
            ));
        }
        out.push_str(&format!(
            "<SigningProtocol>{}</SigningProtocol>\
             <SigningBehavior>{}</SigningBehavior>\
             <OriginAccessControlOriginType>{}</OriginAccessControlOriginType>\
             </OriginAccessControlConfig>",
            self.signing_protocol, self.signing_behavior, self.origin_type
        ));
        out
    }

    /// This configuration with `name` in place of its own, under the case name `case` —
    /// the one place the update suite varies a single member, so it states no second
    /// configuration builder of its own.
    pub(crate) fn with_name(&self, case: &str, name: &str) -> Control {
        Control {
            case: case.to_string(),
            name: name.to_string(),
            ..self.clone()
        }
    }

    /// The `OriginAccessControlOriginType` this configuration carries — the kind of origin
    /// the control it creates is for, which is what a suite attaching the control to a
    /// distribution's origin pairs that origin with.
    pub(crate) fn origin_type(&self) -> &str {
        &self.origin_type
    }

    /// This configuration with `description` in place of its own (`None` leaving the
    /// optional member out entirely), under the case name `case`.
    pub(crate) fn with_description(&self, case: &str, description: Option<&str>) -> Control {
        Control {
            case: case.to_string(),
            description: description.map(str::to_string),
            ..self.clone()
        }
    }

    /// Assert that `config` — an `OriginAccessControlConfig` element the emulator
    /// serialised — reports exactly this configuration.
    pub(crate) fn assert_matches(&self, config: &Node, context: &str, raw: &str) {
        for (member, expected) in [
            ("Name", self.name.clone()),
            ("SigningProtocol", self.signing_protocol.clone()),
            ("SigningBehavior", self.signing_behavior.clone()),
            (
                "OriginAccessControlOriginType",
                self.origin_type.clone(),
            ),
        ] {
            let actual = config.text_at(member).map(str::to_string);
            assert_eq!(
                actual.as_deref(),
                Some(expected.as_str()),
                "case {} ({context}): expected {member} {expected:?}, got {actual:?} in {raw}",
                self.case
            );
        }
        // An optional member the create left out is reported as the stored record has it:
        // absent, rather than as an empty element invented by the read.
        let actual = config.text_at("Description").map(str::to_string);
        assert_eq!(
            actual,
            self.description.clone(),
            "case {} ({context}): expected Description {:?}, got {actual:?} in {raw}",
            self.case,
            self.description
        );
    }

    /// Assert that `summary` — an `OriginAccessControlSummary` element of a listing —
    /// reports this configuration under `id`.
    fn assert_summary(&self, summary: &Node, id: &str, raw: &str) {
        for (member, expected) in [
            ("Id", id.to_string()),
            ("Name", self.name.clone()),
            // `OriginAccessControlSummary$Description` is REQUIRED while the
            // configuration's is optional, so a control created without one is summarised
            // with an empty description.
            (
                "Description",
                self.description.clone().unwrap_or_default(),
            ),
            ("SigningProtocol", self.signing_protocol.clone()),
            ("SigningBehavior", self.signing_behavior.clone()),
            (
                "OriginAccessControlOriginType",
                self.origin_type.clone(),
            ),
        ] {
            let actual = summary.text_at(member).map(str::to_string);
            assert_eq!(
                actual.as_deref(),
                Some(expected.as_str()),
                "case {} (summary of {id}): expected {member} {expected:?}, got {actual:?} \
                 in {raw}",
                self.case
            );
        }
    }
}

/// The configurations every case below is run over.
///
/// The cross product of `OriginAccessControlSigningBehaviors` and
/// `OriginAccessControlOriginTypes` — every value of each, from the model's own catalogs —
/// with `OriginAccessControlSigningProtocols` and the description forms cycled across it,
/// so every value of every enum the configuration carries appears at least once and the
/// descriptions (absent, empty, ASCII, XML metacharacters) span the forms a text member
/// takes.
pub(crate) fn table() -> Vec<Control> {
    // The description forms, cycled across the rows: absent, empty, plain ASCII and text
    // carrying XML metacharacters, which the wire form has to survive.
    let descriptions: [Option<&str>; 4] = [
        None,
        Some(""),
        Some("the origin access control for an emulator test"),
        Some("tags <oac> & \"quotes\" and 'apostrophes'"),
    ];

    let mut rows: Vec<Control> = Vec::new();
    for (index, behavior) in ORIGIN_ACCESS_CONTROL_SIGNING_BEHAVIORS_VALUES
        .iter()
        .enumerate()
    {
        for (offset, origin_type) in ORIGIN_ACCESS_CONTROL_ORIGIN_TYPES_VALUES
            .iter()
            .enumerate()
        {
            let step = index * ORIGIN_ACCESS_CONTROL_ORIGIN_TYPES_VALUES.len() + offset;
            let protocol = ORIGIN_ACCESS_CONTROL_SIGNING_PROTOCOLS_VALUES
                [step % ORIGIN_ACCESS_CONTROL_SIGNING_PROTOCOLS_VALUES.len()];
            rows.push(Control {
                case: format!("{behavior} over {origin_type} signed with {protocol}"),
                name: format!("oac-{step}-{behavior}-{origin_type}"),
                description: descriptions[step % descriptions.len()].map(str::to_string),
                signing_protocol: protocol.to_string(),
                signing_behavior: (*behavior).to_string(),
                origin_type: (*origin_type).to_string(),
            });
        }
    }

    // The published name bound ("You can specify up to 64 characters") and the shortest
    // name a caller can submit: the ends of what `Name` admits.
    rows.push(Control {
        case: "a name at the published bound".to_string(),
        name: "n".repeat(MAX_ORIGIN_ACCESS_CONTROL_NAME),
        description: Some("a name of exactly the documented length".to_string()),
        signing_protocol: "sigv4".to_string(),
        signing_behavior: "always".to_string(),
        origin_type: "s3".to_string(),
    });
    rows.push(Control {
        case: "a one-character name".to_string(),
        name: "x".to_string(),
        description: None,
        signing_protocol: "sigv4a".to_string(),
        signing_behavior: "never".to_string(),
        origin_type: "mediastore".to_string(),
    });
    rows
}

// ---------------------------------------------------------------------------
// Reading an answer
// ---------------------------------------------------------------------------

/// The ids a listing's page reports, in the order the page carries them.
pub(crate) fn page_ids(answer: &Answer) -> Vec<String> {
    answer
        .root()
        .at("Items")
        .map(|items| {
            items
                .children_named("OriginAccessControlSummary")
                .into_iter()
                .map(|summary| summary.text_at("Id").unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// Every `OriginAccessControlSummary` the account holds, walking `NextMarker` at `page`
/// items a page (`None` asks for no page size at all), so a caller of this helper never
/// depends on the whole collection fitting one page.
///
/// The walk is BOUNDED: a listing hands out at most one resume point per stored control,
/// so a repeated resume point or more pages than the account can hold is a pagination
/// defect, and this fails on it instead of looping forever.
async fn all_summaries(router: &MultiServiceRouter, page: Option<usize>) -> Vec<Node> {
    let mut summaries: Vec<Node> = Vec::new();
    let mut marker: Option<String> = None;
    let mut resume_points: BTreeSet<String> = BTreeSet::new();
    for _ in 0..=MAX_ORIGIN_ACCESS_CONTROLS {
        let query = match (page, &marker) {
            (Some(size), Some(value)) => format!("?MaxItems={size}&Marker={value}"),
            (Some(size), None) => format!("?MaxItems={size}"),
            (None, Some(value)) => format!("?Marker={value}"),
            (None, None) => String::new(),
        };
        let answer = list_controls(router, &query).await;
        assert_eq!(
            answer.status, 200,
            "case the whole account: expected HTTP 200 for a listing of {query:?}, got {} \
             with {}",
            answer.status, answer.raw
        );
        if let Some(items) = answer.root().at("Items") {
            summaries.extend(
                items
                    .children_named("OriginAccessControlSummary")
                    .into_iter()
                    .cloned(),
            );
        }
        match answer.root().text_at("NextMarker") {
            Some(next) => {
                assert!(
                    resume_points.insert(next.to_string()),
                    "case the whole account: the listing handed out the resume point {next} \
                     twice, so the walk is not advancing: {}",
                    answer.raw
                );
                marker = Some(next.to_string());
            }
            None => return summaries,
        }
    }
    panic!(
        "case the whole account: the walk took more than {MAX_ORIGIN_ACCESS_CONTROLS} pages, \
         so the listing is not advancing"
    );
}

/// One member's text out of every summary the account holds, in listing order, walked at
/// `page` items a page.
async fn all_summary_texts(
    router: &MultiServiceRouter,
    page: Option<usize>,
    member: &str,
) -> Vec<String> {
    all_summaries(router, page)
        .await
        .iter()
        .map(|summary| summary.text_at(member).unwrap_or_default().to_string())
        .collect()
}

/// Whether `value` has the shape of an id this service mints: `E` followed by 13
/// uppercase alphanumerics, the shape every CloudFront entity id takes — the harness's
/// own [`is_distribution_id`], because a control's id and a distribution's id are drawn
/// from that one shape.
fn is_entity_id(value: &str) -> bool {
    is_distribution_id(value)
}

/// Create every row of `rows` through the API and report `(row, id)` in creation order.
async fn create_all(router: &MultiServiceRouter, rows: &[Control]) -> Vec<(Control, String)> {
    let mut created: Vec<(Control, String)> = Vec::new();
    let mut ids: BTreeSet<String> = BTreeSet::new();
    let mut etags: BTreeSet<String> = BTreeSet::new();
    for row in rows {
        let answer = create_control(router, &row.xml()).await;
        assert_eq!(
            answer.status, 201,
            "case {}: expected HTTP 201 from CreateOriginAccessControl, got {} with {}",
            row.case, answer.status, answer.raw
        );
        let id = answer
            .root()
            .text_at("Id")
            .unwrap_or_default()
            .to_string();
        assert!(
            is_entity_id(&id),
            "case {}: expected a minted origin access control id, got {id:?}",
            row.case
        );
        assert!(
            ids.insert(id.clone()),
            "case {}: the id {id} repeated across creates",
            row.case
        );
        let etag = answer.header("etag").to_string();
        assert!(
            is_entity_id(&etag),
            "case {}: expected a minted version token in ETag, got {etag:?}",
            row.case
        );
        assert!(
            etags.insert(etag.clone()),
            "case {}: the version token {etag} repeated across creates",
            row.case
        );
        let location = answer.header("location").to_string();
        assert_eq!(
            location,
            format!("https://{HOST}/{API_VERSION}/origin-access-control/{id}"),
            "case {}: expected the new control's URI in Location, got {location:?}",
            row.case
        );
        created.push((row.clone(), id));
    }
    created
}

// ---------------------------------------------------------------------------
// The cases
// ---------------------------------------------------------------------------

#[tokio::test]
async fn create_persists_the_configuration_every_read_then_reports() {
    let router = emulator();
    let created = create_all(&router, &table()).await;

    for (row, id) in &created {
        // The create's own answer already reports the record it stored.
        let read = get_control(&router, id).await;
        assert_eq!(
            read.status, 200,
            "case {}: expected HTTP 200 from GetOriginAccessControl, got {} with {}",
            row.case, read.status, read.raw
        );
        assert_eq!(
            read.root().text_at("Id"),
            Some(id.as_str()),
            "case {}: expected the stored id {id} in the read, got {}",
            row.case,
            read.raw
        );
        let config = read
            .root()
            .at("OriginAccessControlConfig")
            .unwrap_or_else(|| panic!("case {}: no configuration in {}", row.case, read.raw));
        row.assert_matches(config, "GetOriginAccessControl", &read.raw);
        assert!(
            is_entity_id(read.header("etag")),
            "case {}: expected a version token in the read's ETag, got {:?}",
            row.case,
            read.header("etag")
        );
    }

    // An id no control holds is answered under ONE code and its 404, and WHICH sentence it
    // carries follows the ALPHABET the value is written in and nothing else: `Id` is a plain
    // `string` `@httpLabel` with no `@pattern` and no `@length`, so a value of the alphabet an
    // entity identifier is minted in — under the control's own namespace letter, an unminted
    // one, or a single digit — is resolved against the store and reported as the control that
    // is ABSENT, while a value written outside that alphabet (a lower-cased id, a bare word, a
    // UUID) could never have been an identifier of this mint and is the identifier the route
    // reports it cannot READ.
    let distribution_id = created
        .first()
        .map(|(_, id)| id.clone())
        .unwrap_or_else(|| panic!("the table must create at least one control"));
    let lower_cased = distribution_id.to_lowercase();
    for (id, expected) in [
        ("E1AAAAAAAAAAAA", NO_SUCH_CONTROL_MESSAGE),
        ("not-an-id", UNREADABLE_IDENTIFIER_MESSAGE),
        ("0", NO_SUCH_CONTROL_MESSAGE),
        (lower_cased.as_str(), UNREADABLE_IDENTIFIER_MESSAGE),
        ("W1AAAAAAAAAAAA", NO_SUCH_CONTROL_MESSAGE),
        (
            "00000000-0000-0000-0000-000000000000",
            UNREADABLE_IDENTIFIER_MESSAGE,
        ),
    ] {
        let missing = get_control(&router, id).await;
        assert_eq!(
            missing.status, 404,
            "case {id}: expected HTTP 404 for an id no control holds, got {} with {}",
            missing.status, missing.raw
        );
        assert_eq!(
            missing.error_code(),
            "NoSuchOriginAccessControl",
            "case {id}: expected NoSuchOriginAccessControl, got {}",
            missing.raw
        );
        assert_eq!(
            missing.root().find_text("Message"),
            Some(expected),
            "case {id}: expected the sentence {expected:?}, got {}",
            missing.raw
        );
    }
}

#[tokio::test]
async fn an_account_using_no_origin_access_controls_lists_no_items() {
    let router = emulator();

    let listed = list_controls(&router, "").await;
    assert_eq!(
        listed.status, 200,
        "case empty account: expected HTTP 200 from ListOriginAccessControls, got {} with {}",
        listed.status, listed.raw
    );
    assert_eq!(
        listed.root().text_at("Quantity"),
        Some("0"),
        "case empty account: expected Quantity 0, got {}",
        listed.raw
    );
    assert_eq!(
        listed.root().text_at("IsTruncated"),
        Some("false"),
        "case empty account: expected IsTruncated false, got {}",
        listed.raw
    );
    assert_eq!(
        listed.root().text_at("MaxItems"),
        Some(MAX_ITEMS_ORIGIN_ACCESS_CONTROLS.to_string().as_str()),
        "case empty account: expected the default MaxItems {MAX_ITEMS_ORIGIN_ACCESS_CONTROLS}, \
         got {}",
        listed.raw
    );
    assert_eq!(
        listed.root().text_at("Marker"),
        Some(""),
        "case empty account: expected an empty Marker echo, got {}",
        listed.raw
    );
    assert!(
        listed.root().at("Items").is_none(),
        "case empty account: an account using no origin access controls carries no Items \
         element, got {}",
        listed.raw
    );
    assert!(
        listed.root().text_at("NextMarker").is_none(),
        "case empty account: expected no NextMarker on an untruncated page, got {}",
        listed.raw
    );
}

#[tokio::test]
async fn a_listing_reports_every_stored_control_and_pages_through_them() {
    let router = emulator();
    let created = create_all(&router, &table()).await;

    // One stable total order over the stored controls, which every page indexes into.
    let mut expected: Vec<(Control, String)> = created.clone();
    expected.sort_by(|left, right| left.1.cmp(&right.1));
    let order: Vec<String> = expected.iter().map(|(_, id)| id.clone()).collect();

    // A listing that asks for no page size reports the whole (sub-ceiling) collection,
    // each summary reporting what the stored record answers a get with.
    let listed = list_controls(&router, "").await;
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
    assert_eq!(
        listed.root().text_at("IsTruncated"),
        Some("false"),
        "case whole collection: expected IsTruncated false, got {}",
        listed.raw
    );
    let items = listed
        .root()
        .at("Items")
        .unwrap_or_else(|| panic!("case whole collection: no Items in {}", listed.raw));
    let summaries = items.children_named("OriginAccessControlSummary");
    for ((row, id), summary) in expected.iter().zip(summaries) {
        row.assert_summary(summary, id, &listed.raw);
    }

    // Walking the pages at ANY admissible page size enumerates every control exactly
    // once, in that same order: sizes below, at and past the collection size, and one
    // past the server's own ceiling.
    let sizes = [
        1usize,
        2,
        3,
        order.len() - 1,
        order.len(),
        order.len() + 1,
        usize::try_from(MAX_ITEMS).expect("the ceiling fits a usize") + 1,
    ];
    for size in sizes {
        let mut walked: Vec<String> = Vec::new();
        let mut marker: Option<String> = None;
        loop {
            let query = match &marker {
                Some(value) => format!("?MaxItems={size}&Marker={value}"),
                None => format!("?MaxItems={size}"),
            };
            let page = list_controls(&router, &query).await;
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
            assert_eq!(
                page.root().text_at("Marker"),
                Some(marker.clone().unwrap_or_default().as_str()),
                "case page size {size}: expected the requested Marker echoed, got {}",
                page.raw
            );
            let ids = page_ids(&page);
            // The page served is the requested size bounded above by the server's own
            // ceiling, and never longer than what remains.
            let ceiling = usize::try_from(MAX_ITEMS).expect("the ceiling fits a usize");
            assert!(
                ids.len() <= size.min(ceiling),
                "case page size {size}: a page served {} items, got {}",
                ids.len(),
                page.raw
            );
            walked.extend(ids.iter().cloned());
            let truncated = page.root().text_at("IsTruncated") == Some("true");
            let next = page.root().text_at("NextMarker").map(str::to_string);
            assert_eq!(
                truncated,
                walked.len() < order.len(),
                "case page size {size}: expected IsTruncated {} after {} of {} items, got {}",
                walked.len() < order.len(),
                walked.len(),
                order.len(),
                page.raw
            );
            assert_eq!(
                next.is_some(),
                truncated,
                "case page size {size}: NextMarker is present exactly when the page is \
                 truncated, got {}",
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
            "case page size {size}: walking the pages must enumerate every control exactly \
             once, in order"
        );
    }

    // A listing is a READ: the same request answers the same records again.
    let again = list_controls(&router, "").await;
    assert_eq!(
        page_ids(&again),
        order,
        "case repeated listing: a read must not change what a later caller sees, got {}",
        again.raw
    );
}

#[tokio::test]
async fn a_page_size_or_marker_that_names_no_page_is_refused() {
    let router = emulator();
    let created = create_all(&router, &table()[..3]).await;
    let before: Vec<String> = {
        let listed = list_controls(&router, "").await;
        page_ids(&listed)
    };

    let rows = [
        ("a negative page size", "?MaxItems=-1"),
        ("a page size that is no integer", "?MaxItems=not-a-number"),
        ("a page size past what the member can hold", "?MaxItems=99999999999"),
        ("a marker that is no id at all", "?Marker=nope"),
        ("a marker too short to be an id", "?Marker=E123"),
        ("a marker in the wrong case", "?Marker=e1aaaaaaaaaaaa"),
        ("a marker too long to be an id", "?Marker=E1AAAAAAAAAAAAAAAA"),
        (
            "an unusable page size beside a usable marker",
            "?MaxItems=-1&Marker=E1AAAAAAAAAAAA",
        ),
    ];
    for (case, query) in rows {
        let answer = list_controls(&router, query).await;
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
    }

    // A well-shaped marker naming no stored control is a resume POSITION, not a refusal:
    // the page it names holds the controls ordered after it.
    let last = before
        .last()
        .cloned()
        .expect("the fixture created some controls");
    let after_last = list_controls(&router, &format!("?Marker={last}")).await;
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
    let listed = list_controls(&router, "").await;
    assert_eq!(
        page_ids(&listed),
        before,
        "case refusals: a refused listing must store nothing, got {}",
        listed.raw
    );
    assert_eq!(
        created.len(),
        before.len(),
        "case refusals: expected the created controls to be the listed ones"
    );
}

#[tokio::test]
async fn a_create_under_a_taken_name_names_the_control_that_holds_it() {
    let router = emulator();
    let row = Control {
        case: "the control the account already holds".to_string(),
        name: "oac-already-exists".to_string(),
        description: Some("the first create's description".to_string()),
        signing_protocol: "sigv4".to_string(),
        signing_behavior: "always".to_string(),
        origin_type: "s3".to_string(),
    };
    let created = create_control(&router, &row.xml()).await;
    assert_eq!(
        created.status, 201,
        "case {}: expected HTTP 201, got {} with {}",
        row.case, created.status, created.raw
    );
    let id = created
        .root()
        .text_at("Id")
        .unwrap_or_default()
        .to_string();

    let repeated = create_control(&router, &row.xml()).await;
    assert_eq!(
        repeated.status, 409,
        "case {}: expected HTTP 409 for a repeat of every parameter, got {} with {}",
        row.case, repeated.status, repeated.raw
    );
    assert_eq!(
        repeated.error_code(),
        "OriginAccessControlAlreadyExists",
        "case {}: expected OriginAccessControlAlreadyExists, got {}",
        row.case,
        repeated.raw
    );

    // A request under the taken NAME names the control that holds it, whichever other
    // member differs — including the optional one.
    let under_the_taken_name = [
        Control {
            case: "a different description".to_string(),
            description: Some("another description".to_string()),
            ..row.clone()
        },
        Control {
            case: "no description at all".to_string(),
            description: None,
            ..row.clone()
        },
        Control {
            case: "a different signing protocol".to_string(),
            signing_protocol: "sigv4a".to_string(),
            ..row.clone()
        },
        Control {
            case: "a different signing behavior".to_string(),
            signing_behavior: "no-override".to_string(),
            ..row.clone()
        },
        Control {
            case: "a different origin type".to_string(),
            origin_type: "mediapackagev2".to_string(),
            ..row.clone()
        },
    ];
    for variant in &under_the_taken_name {
        let answer = create_control(&router, &variant.xml()).await;
        assert_eq!(
            answer.status, 409,
            "case {}: expected HTTP 409 under the taken name, got {} with {}",
            variant.case, answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "OriginAccessControlAlreadyExists",
            "case {}: expected OriginAccessControlAlreadyExists, got {}",
            variant.case,
            answer.raw
        );
    }

    // A request under a FREE name describes a different control and creates one, even when
    // every other parameter repeats a stored control's.
    let mut ids: BTreeSet<String> = BTreeSet::new();
    ids.insert(id.clone());
    for variant in &[
        Control {
            case: "the same parameters under a free name".to_string(),
            name: "oac-already-exists-renamed".to_string(),
            ..row.clone()
        },
        Control {
            case: "other parameters under another free name".to_string(),
            name: "oac-already-exists-other".to_string(),
            description: None,
            signing_protocol: "sigv4a".to_string(),
            ..row.clone()
        },
    ] {
        let answer = create_control(&router, &variant.xml()).await;
        assert_eq!(
            answer.status, 201,
            "case {}: a configuration under a free name is a new control, got {} \
             with {}",
            variant.case, answer.status, answer.raw
        );
        let other = answer
            .root()
            .text_at("Id")
            .unwrap_or_default()
            .to_string();
        assert!(
            ids.insert(other.clone()),
            "case {}: expected a fresh id, got {other} again",
            variant.case
        );
    }

    // The control the conflict named is untouched, and the listing holds one record per
    // distinct name.
    let read = get_control(&router, &id).await;
    assert_eq!(
        read.status, 200,
        "case {}: the refused repeat must leave the stored control readable, got {} with {}",
        row.case, read.status, read.raw
    );
    let config = read
        .root()
        .at("OriginAccessControlConfig")
        .unwrap_or_else(|| panic!("case {}: no configuration in {}", row.case, read.raw));
    row.assert_matches(config, "after a refused repeat", &read.raw);

    let listed = list_controls(&router, "").await;
    let listed_ids: BTreeSet<String> = page_ids(&listed).into_iter().collect();
    assert_eq!(
        listed_ids, ids,
        "case the whole account: expected exactly the created controls, got {}",
        listed.raw
    );
}

#[tokio::test]
async fn a_name_past_the_documented_bound_is_refused() {
    let router = emulator();

    // "You can specify up to 64 characters": the bound itself is accepted, one past it is
    // not, and neither is a name far past it.
    let accepted = Control {
        case: "a name at the published bound".to_string(),
        name: "a".repeat(MAX_ORIGIN_ACCESS_CONTROL_NAME),
        description: None,
        signing_protocol: "sigv4".to_string(),
        signing_behavior: "always".to_string(),
        origin_type: "s3".to_string(),
    };
    let answer = create_control(&router, &accepted.xml()).await;
    assert_eq!(
        answer.status, 201,
        "case {}: expected HTTP 201 at the bound, got {} with {}",
        accepted.case, answer.status, answer.raw
    );

    for (case, length) in [
        ("one past the published bound", MAX_ORIGIN_ACCESS_CONTROL_NAME + 1),
        ("far past the published bound", MAX_ORIGIN_ACCESS_CONTROL_NAME * 4),
    ] {
        let refused = Control {
            case: case.to_string(),
            name: "b".repeat(length),
            description: None,
            signing_protocol: "sigv4".to_string(),
            signing_behavior: "always".to_string(),
            origin_type: "s3".to_string(),
        };
        let answer = create_control(&router, &refused.xml()).await;
        assert_eq!(
            answer.status, 400,
            "case {case}: expected HTTP 400 for a {length}-character name, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "InvalidArgument",
            "case {case}: expected InvalidArgument, got {}",
            answer.raw
        );
    }

    // The refusals stored nothing: only the accepted control is listed.
    let listed = list_controls(&router, "").await;
    assert_eq!(
        listed.root().text_at("Quantity"),
        Some("1"),
        "case refused names: expected only the accepted control to be stored, got {}",
        listed.raw
    );
}

#[tokio::test]
async fn the_minted_version_token_and_location_are_what_every_later_read_answers_with() {
    let router = emulator();
    let rows = table();

    // What each create minted, so the reads below are checked against the create's own
    // answer rather than against a token the test made up.
    let mut minted: Vec<(Control, String, String)> = Vec::new();
    for row in &rows {
        let created = create_control(&router, &row.xml()).await;
        assert_eq!(
            created.status, 201,
            "case {}: expected HTTP 201 from CreateOriginAccessControl, got {} with {}",
            row.case, created.status, created.raw
        );
        let id = created
            .root()
            .text_at("Id")
            .unwrap_or_default()
            .to_string();
        let etag = created.header("etag").to_string();

        // "The URL of the origin access control": the URI the control is READ at, so a
        // caller holding only the create's Location reaches the record that was stored.
        let location = created.header("location").to_string();
        let path = location
            .strip_prefix(&format!("https://{HOST}"))
            .unwrap_or_else(|| {
                panic!(
                    "case {}: expected a Location under https://{HOST}, got {location:?}",
                    row.case
                )
            })
            .to_string();
        let at_location = call(&router, "GET", &path, None, None).await;
        assert_eq!(
            at_location.status, 200,
            "case {}: expected HTTP 200 reading the control at its own Location {path}, \
             got {} with {}",
            row.case, at_location.status, at_location.raw
        );
        assert_eq!(
            at_location.root().text_at("Id"),
            Some(id.as_str()),
            "case {}: expected the Location to name the created control {id}, got {}",
            row.case,
            at_location.raw
        );

        minted.push((row.clone(), id, etag));
    }

    // Reading a control mints nothing: every read answers with the token the create handed
    // back, however many times it is read, and with the configuration that was stored.
    for attempt in 1..=3 {
        for (row, id, etag) in &minted {
            let read = get_control(&router, id).await;
            assert_eq!(
                read.status, 200,
                "case {} (read {attempt}): expected HTTP 200, got {} with {}",
                row.case, read.status, read.raw
            );
            assert_eq!(
                read.header("etag"),
                etag.as_str(),
                "case {} (read {attempt}): expected the minted version token {etag}, got {:?}",
                row.case,
                read.header("etag")
            );
            let config = read.root().at("OriginAccessControlConfig").unwrap_or_else(|| {
                panic!(
                    "case {} (read {attempt}): no configuration in {}",
                    row.case, read.raw
                )
            });
            row.assert_matches(config, "a repeated read", &read.raw);
        }
    }

    // The tokens are per CONTROL, so two controls never share one.
    let tokens: BTreeSet<String> = minted
        .iter()
        .map(|(_, _, etag)| etag.clone())
        .collect();
    assert_eq!(
        tokens.len(),
        minted.len(),
        "case the whole account: expected one distinct version token per control, got \
         {} for {} controls",
        tokens.len(),
        minted.len()
    );
}

#[tokio::test]
async fn a_create_past_the_account_quota_is_refused_and_mints_nothing() {
    let router = emulator();

    // Fill the account to its origin access control quota, each create describing a
    // DIFFERENT control (the enum catalogs cycled across the rows, so the configurations
    // differ in more than their names).
    let at_quota: Vec<Control> = (0..MAX_ORIGIN_ACCESS_CONTROLS)
        .map(|index| Control {
            case: format!("control {index} of the quota"),
            name: format!("oac-quota-{index}"),
            description: None,
            signing_protocol: ORIGIN_ACCESS_CONTROL_SIGNING_PROTOCOLS_VALUES
                [index % ORIGIN_ACCESS_CONTROL_SIGNING_PROTOCOLS_VALUES.len()]
            .to_string(),
            signing_behavior: ORIGIN_ACCESS_CONTROL_SIGNING_BEHAVIORS_VALUES
                [index % ORIGIN_ACCESS_CONTROL_SIGNING_BEHAVIORS_VALUES.len()]
            .to_string(),
            origin_type: ORIGIN_ACCESS_CONTROL_ORIGIN_TYPES_VALUES
                [index % ORIGIN_ACCESS_CONTROL_ORIGIN_TYPES_VALUES.len()]
            .to_string(),
        })
        .collect();
    let created = create_all(&router, &at_quota).await;
    assert_eq!(
        created.len(),
        MAX_ORIGIN_ACCESS_CONTROLS,
        "case filling the quota: expected {MAX_ORIGIN_ACCESS_CONTROLS} controls created"
    );

    // Walked across pages at a page size well under the quota, so the case is about the
    // QUOTA rather than about the quota happening to fit one page.
    let before = all_summary_texts(&router, Some(7), "Id").await;
    assert_eq!(
        before.len(),
        MAX_ORIGIN_ACCESS_CONTROLS,
        "case filling the quota: expected the account to hold its quota of \
         {MAX_ORIGIN_ACCESS_CONTROLS} controls, got {}",
        before.len()
    );

    // A create that would take the account past the quota is refused — and the refusal is
    // about the COUNT, so it holds for every configuration, not just one.
    let past_quota = [
        Control {
            case: "one past the quota".to_string(),
            name: "oac-past-the-quota".to_string(),
            description: None,
            signing_protocol: "sigv4".to_string(),
            signing_behavior: "always".to_string(),
            origin_type: "s3".to_string(),
        },
        Control {
            case: "one past the quota, described".to_string(),
            name: "oac-past-the-quota-described".to_string(),
            description: Some("a control the account has no room for".to_string()),
            signing_protocol: "sigv4a".to_string(),
            signing_behavior: "no-override".to_string(),
            origin_type: "mediastore".to_string(),
        },
        Control {
            case: "a repeat of the first refused create".to_string(),
            name: "oac-past-the-quota".to_string(),
            description: None,
            signing_protocol: "sigv4".to_string(),
            signing_behavior: "always".to_string(),
            origin_type: "s3".to_string(),
        },
    ];
    for row in &past_quota {
        let answer = create_control(&router, &row.xml()).await;
        assert_eq!(
            answer.status, 400,
            "case {}: expected HTTP 400 past the quota, got {} with {}",
            row.case, answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "TooManyOriginAccessControls",
            "case {}: expected TooManyOriginAccessControls, got {}",
            row.case,
            answer.raw
        );
        assert!(
            answer.root().text_at("Id").is_none(),
            "case {}: a refused create mints no id, got {}",
            row.case,
            answer.raw
        );
    }

    // Every refusal minted nothing: the account still holds exactly the controls it held,
    // and none of them is one of the refused configurations.
    let after = all_summary_texts(&router, Some(7), "Id").await;
    assert_eq!(
        after, before,
        "case past the quota: a refused create must store nothing"
    );
    let names: BTreeSet<String> = all_summary_texts(&router, None, "Name")
        .await
        .into_iter()
        .collect();
    for row in &past_quota {
        assert!(
            !names.contains(&row.name),
            "case {}: the refused control {} must not be stored, got {names:?}",
            row.case,
            row.name
        );
    }
}
