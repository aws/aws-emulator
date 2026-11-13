// SPDX-License-Identifier: CC0-1.0

//! The CloudFront connection group surface, end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing, restXml
//! (de)serialization, the handler, the persisted state and serialization — the same path a
//! client takes. The rules under test are the ones CloudFront's connection group surface
//! states:
//!
//! * a connection group is PERSISTED by the call that creates it, which MINTS its identity
//!   (the `Id`, the ARN built from it, the routing endpoint "assigned to the connection
//!   group" — the address of the listener the group is served on, which a viewer can dial —
//!   and a version token), so a later `GetConnectionGroup` and a later
//!   `ListConnectionGroups` both answer from that one record — down to the members the
//!   create left out, which take their documented defaults (`Ipv6Enabled`'s "default is
//!   `true`");
//! * `GetConnectionGroup`'s `Identifier` is "The ID, name, or Amazon Resource Name (ARN) of
//!   the connection group", so all three reach the one record;
//! * `ListConnectionGroups` reports every persisted group in ONE stable total order, cut
//!   to at most `MaxItems` entries beginning after the `Marker` id, and carries a
//!   `NextMarker` exactly when groups remain — so walking the pages at ANY admissible page
//!   size enumerates the collection exactly once;
//! * a `Marker` this service never handed out is no resume point, and a well-formed one
//!   that names no stored group is an entity that does not exist;
//! * an `AnycastIpListId` — on a create, or on the listing's `AssociationFilter` — names
//!   an Anycast static IP list, so a value this emulator cannot resolve to one is an
//!   invalid argument;
//! * a name "unique within your Amazon Web Services account" names at most one group;
//! * the group CloudFront creates BY ITSELF for a distribution tenant that named none is
//!   one of these records, carrying `IsDefault`;
//! * a group CloudFront has just created is `InProgress`, and the CLOCK — not a later call
//!   — owes it `Deployed`.
//!
//! The table rows differ along everything these rules generalise over: whether a create
//! names `Ipv6Enabled`, `Enabled` or `Tags` at all, the two values each boolean can take,
//! how many tags the group carries, and every page size from one up to and past the
//! collection's size — plus both ends of the admissible `MaxItems` range and one value
//! past each.
//!
//! The wire harness (the router, the request helpers and the element scanner) is the one
//! [`crate::distribution_lifecycle`] already builds.

use std::collections::BTreeSet;
use std::time::Duration;

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::{
    CONNECTION_GROUP_ID_PREFIX, MAX_ITEMS, PROPAGATION_DELAY, STATUS_DEPLOYED, STATUS_IN_PROGRESS,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::distribution_lifecycle::{
    call, emulator, is_reachable_authority, is_version_token, Answer, Node, ACCOUNT_ID,
    API_VERSION,
};
use crate::distribution_tenant_lifecycle::{
    create_distribution_tenant, encode_label, managed_certificate_request,
    multi_tenant_distribution,
};
use crate::response_headers_policy_lifecycle::{assert_absent, text};

// ---------------------------------------------------------------------------
// Calling the connection group surface
// ---------------------------------------------------------------------------

/// `POST /2020-05-31/connection-group` with the request document `body`.
///
/// `pub(crate)` so a sibling suite that needs a connection group of its own — one whose
/// ROUTING ENDPOINT it compares something against — creates one over the same wire path
/// rather than growing a copy of this.
pub(crate) async fn create_connection_group(router: &MultiServiceRouter, body: &str) -> Answer {
    call(
        router,
        "POST",
        &format!("/{API_VERSION}/connection-group"),
        Some(body),
        None,
    )
    .await
}

/// `GET /2020-05-31/connection-group/{Identifier}`.
///
/// `pub(crate)` for the same reason the create above is: a sibling suite that reads a
/// group it created back goes through this one wire path rather than growing a copy of it.
pub(crate) async fn get_connection_group(router: &MultiServiceRouter, identifier: &str) -> Answer {
    call(
        router,
        "GET",
        &format!(
            "/{API_VERSION}/connection-group/{}",
            encode_label(identifier)
        ),
        None,
        None,
    )
    .await
}

/// `POST /2020-05-31/connection-groups` with the request document `body`.
///
/// `pub(crate)` so a sibling suite that has to see what the collection holds after a
/// mutation — a delete, say — reads it over this one wire path rather than growing a copy.
pub(crate) async fn list_connection_groups(router: &MultiServiceRouter, body: &str) -> Answer {
    call(
        router,
        "POST",
        &format!("/{API_VERSION}/connection-groups"),
        Some(body),
        None,
    )
    .await
}

/// A `ListConnectionGroupsRequest` document naming whichever of the three input members
/// the case under test names.
pub(crate) fn list_body(
    max_items: Option<i32>,
    marker: Option<&str>,
    anycast_filter: Option<&str>,
) -> String {
    let mut out = format!(
        "<ListConnectionGroupsRequest xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">"
    );
    if let Some(filter) = anycast_filter {
        out.push_str("<AssociationFilter>");
        if !filter.is_empty() {
            out.push_str(&format!("<AnycastIpListId>{filter}</AnycastIpListId>"));
        }
        out.push_str("</AssociationFilter>");
    }
    if let Some(marker) = marker {
        out.push_str(&format!("<Marker>{marker}</Marker>"));
    }
    if let Some(max_items) = max_items {
        out.push_str(&format!("<MaxItems>{max_items}</MaxItems>"));
    }
    out.push_str("</ListConnectionGroupsRequest>");
    out
}

/// The `ConnectionGroupSummary` elements one listing answered with.
///
/// `ConnectionGroupSummaryList`'s member carries `xmlName` `ConnectionGroupSummary`
/// (`smithy_query(query="shape",shape="ConnectionGroupSummaryList")`), so each entry
/// travels in an element of that name inside the `ConnectionGroups` wrapper.
fn summaries(answer: &Answer) -> Vec<&Node> {
    match answer.root().at("ConnectionGroups") {
        Some(wrapper) => wrapper.children_named("ConnectionGroupSummary"),
        None => Vec::new(),
    }
}

/// The ids one listing reported, in the order it reported them.
pub(crate) fn summary_ids(answer: &Answer) -> Vec<String> {
    summaries(answer)
        .iter()
        .map(|summary| {
            summary
                .text_at("Id")
                .expect("every summary carries an Id")
                .to_string()
        })
        .collect()
}

/// Whether `value` has the shape of a connection group id this service mints: the `cg_`
/// prefix followed by 13 uppercase alphanumerics, the body every CloudFront token this
/// service mints carries.
///
/// The version-token shape is the harness's own
/// [`crate::distribution_lifecycle::is_version_token`], so this file carries no second
/// copy of it.
fn is_connection_group_id(value: &str) -> bool {
    match value.strip_prefix(CONNECTION_GROUP_ID_PREFIX) {
        Some(body) => {
            body.len() == 13
                && body
                    .bytes()
                    .all(|b| b.is_ascii_digit() || b.is_ascii_uppercase())
        }
        None => false,
    }
}

// ---------------------------------------------------------------------------
// The creates under test
// ---------------------------------------------------------------------------

/// One `CreateConnectionGroupRequest` a client submits.
#[derive(Debug, Clone)]
struct Create {
    case: &'static str,
    name: &'static str,
    /// `Ipv6Enabled`, or `None` to leave the optional member out entirely.
    ipv6_enabled: Option<bool>,
    /// `Enabled`, or `None` to leave the optional member out entirely.
    enabled: Option<bool>,
    /// `AnycastIpListId`, or `None` to leave the optional member out entirely.
    anycast_ip_list_id: Option<&'static str>,
    tags: Vec<(&'static str, &'static str)>,
}

impl Create {
    /// The `CreateConnectionGroupRequest` XML a client sends for this create.
    fn xml(&self) -> String {
        let mut out = format!(
            "<CreateConnectionGroupRequest \
             xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
             <Name>{}</Name>",
            self.name
        );
        if let Some(ipv6_enabled) = self.ipv6_enabled {
            out.push_str(&format!("<Ipv6Enabled>{ipv6_enabled}</Ipv6Enabled>"));
        }
        if let Some(enabled) = self.enabled {
            out.push_str(&format!("<Enabled>{enabled}</Enabled>"));
        }
        if let Some(anycast_ip_list_id) = self.anycast_ip_list_id {
            out.push_str(&format!(
                "<AnycastIpListId>{anycast_ip_list_id}</AnycastIpListId>"
            ));
        }
        if !self.tags.is_empty() {
            out.push_str("<Tags><Items>");
            for (key, value) in &self.tags {
                out.push_str(&format!(
                    "<Tag><Key>{key}</Key><Value>{value}</Value></Tag>"
                ));
            }
            out.push_str("</Items></Tags>");
        }
        out.push_str("</CreateConnectionGroupRequest>");
        out
    }

    /// `Enabled` as the record must report it: the value submitted, or — for a create that
    /// named none — the serving group such a create asks for, written out as the LITERAL
    /// the rule states rather than read back off the crate's own constant.
    fn expected_enabled(&self) -> bool {
        self.enabled.unwrap_or(true)
    }

    /// `Ipv6Enabled` as the record must report it. The model pins the default literally —
    /// "Enable IPv6 for the connection group. The default is `true`" — so the literal is
    /// what a create that named none is asserted against.
    fn expected_ipv6_enabled(&self) -> bool {
        self.ipv6_enabled.unwrap_or(true)
    }
}

/// Two Anycast static IP list ids in the shape CloudFront writes one in. Nothing in these
/// cases creates an Anycast static IP list, so this emulator holds none that either id
/// names - which is what the cases below assert.
const ANYCAST_LIST_ABSENT: &str = "ipl-08d1a1b2c3d4e5f67";
const ANYCAST_LIST_ALSO_ABSENT: &str = "ipl-0999888777666555";

/// The name this service gives the account's automatically created connection group,
/// written out as the LITERAL rather than read back off the crate's own constant.
const RECORDED_DEFAULT_GROUP_NAME: &str = "default";

/// The sentence CloudFront answers an `AssociationFilter` it cannot use with, as the
/// RECORDED `ListConnectionGroups` contract pins it — written out here rather than read
/// back off the crate's own constant, so a change to that constant fails this case.
const RECORDED_INVALID_FILTER_MESSAGE: &str = "The association filter id is invalid.";

/// The creates every case below runs, differing along everything the rules generalise
/// over: which optional members the request names at all, the two values each boolean can
/// take, and how many tags the group carries.
fn creates() -> Vec<Create> {
    vec![
        Create {
            case: "every optional member left out",
            name: "plain-group",
            ipv6_enabled: None,
            enabled: None,
            anycast_ip_list_id: None,
            tags: Vec::new(),
        },
        Create {
            case: "ipv6 turned off",
            name: "ipv4-only-group",
            ipv6_enabled: Some(false),
            enabled: None,
            anycast_ip_list_id: None,
            tags: Vec::new(),
        },
        Create {
            case: "a disabled group",
            name: "disabled-group",
            ipv6_enabled: None,
            enabled: Some(false),
            anycast_ip_list_id: None,
            tags: Vec::new(),
        },
        Create {
            case: "both booleans named, one tag",
            name: "tagged-group",
            ipv6_enabled: Some(true),
            enabled: Some(true),
            anycast_ip_list_id: None,
            tags: vec![("stage", "prod")],
        },
        Create {
            case: "several tags",
            name: "many-tags-group",
            ipv6_enabled: Some(false),
            enabled: Some(true),
            anycast_ip_list_id: None,
            tags: vec![("stage", "dev"), ("team", "edge"), ("owner", "platform")],
        },
        Create {
            case: "a name at the other end of the alphabet",
            name: "zzz-last-group",
            ipv6_enabled: None,
            enabled: Some(false),
            anycast_ip_list_id: None,
            tags: Vec::new(),
        },
    ]
}

/// Run every create of [`creates`] against `router` and answer each row's minted id, in
/// the order the rows were created.
async fn create_every_group(router: &MultiServiceRouter) -> Vec<String> {
    let mut ids = Vec::new();
    for create in creates() {
        let answer = create_connection_group(router, &create.xml()).await;
        assert_eq!(
            answer.status, 201,
            "case {}: expected HTTP 201, got {} with {}",
            create.case, answer.status, answer.raw
        );
        ids.push(text(
            answer.root(),
            "Id",
            create.case,
            "the created group",
            &answer.raw,
        ));
    }
    ids
}

// ---------------------------------------------------------------------------
// The create, and the two reads of what it stored
// ---------------------------------------------------------------------------

/// A create MINTS the group's identity, stores the whole record, and both reads of that
/// record — `GetConnectionGroup` by each of its three identifiers, and
/// `ListConnectionGroups` — report exactly what was stored.
#[tokio::test]
async fn each_create_persists_the_group_every_read_then_reports() {
    let router = emulator();
    let mut minted: BTreeSet<String> = BTreeSet::new();

    for create in creates() {
        let case = create.case;
        let answer = create_connection_group(&router, &create.xml()).await;
        assert_eq!(
            answer.status, 201,
            "case {case}: expected HTTP 201, got {} with {}",
            answer.status, answer.raw
        );

        let group = answer.root();
        let id = text(group, "Id", case, "the created group", &answer.raw);
        assert!(
            is_connection_group_id(&id),
            "case {case}: expected a minted connection group id, got {id:?}"
        );
        assert!(
            minted.insert(id.clone()),
            "case {case}: the mint handed out {id:?} twice"
        );

        let etag = answer.header("etag").to_string();
        assert!(
            is_version_token(&etag),
            "case {case}: expected a minted version token in the ETag header, got {etag:?}"
        );

        let expected_arn = format!("arn:aws:cloudfront::{ACCOUNT_ID}:connection-group/{id}");
        assert_eq!(
            text(group, "Arn", case, "the created group", &answer.raw),
            expected_arn,
            "case {case}: the ARN is built from the minted id"
        );
        assert_eq!(
            text(group, "Name", case, "the created group", &answer.raw),
            create.name,
            "case {case}: the stored name is the one submitted"
        );
        let routing_endpoint = text(
            group,
            "RoutingEndpoint",
            case,
            "the created group",
            &answer.raw,
        );
        assert!(
            is_reachable_authority(&routing_endpoint),
            "case {case}: expected a routing endpoint a viewer can dial - the address of \
             the listener this group is served on - got {routing_endpoint:?}"
        );
        assert_eq!(
            text(group, "Enabled", case, "the created group", &answer.raw),
            create.expected_enabled().to_string(),
            "case {case}: Enabled is the value submitted, or its default"
        );
        assert_eq!(
            text(group, "Ipv6Enabled", case, "the created group", &answer.raw),
            create.expected_ipv6_enabled().to_string(),
            "case {case}: Ipv6Enabled is the value submitted, or its documented default"
        );
        assert_eq!(
            text(group, "IsDefault", case, "the created group", &answer.raw),
            "false",
            "case {case}: a group a caller created is not the account's default one"
        );
        assert_eq!(
            text(group, "Status", case, "the created group", &answer.raw),
            STATUS_IN_PROGRESS,
            "case {case}: a group that has just been created has not propagated yet"
        );
        match create.anycast_ip_list_id {
            Some(expected) => assert_eq!(
                text(
                    group,
                    "AnycastIpListId",
                    case,
                    "the created group",
                    &answer.raw
                ),
                expected,
                "case {case}: the Anycast IP list id is held as submitted"
            ),
            None => assert_absent(group, "AnycastIpListId", case, "the created group"),
        }
        for (key, value) in &create.tags {
            let tags = group
                .at("Tags/Items")
                .unwrap_or_else(|| panic!("case {case}: the answer carries no Tags: {}", answer.raw));
            let found = tags.children_named("Tag").into_iter().any(|tag| {
                tag.text_at("Key") == Some(*key) && tag.text_at("Value") == Some(*value)
            });
            assert!(
                found,
                "case {case}: expected the tag {key}={value} on the created group, got {tags:?}"
            );
        }
        let created_time = text(group, "CreatedTime", case, "the created group", &answer.raw);
        assert_eq!(
            text(
                group,
                "LastModifiedTime",
                case,
                "the created group",
                &answer.raw
            ),
            created_time,
            "case {case}: a group nothing has modified was last modified when it was created"
        );
        assert!(
            !created_time.is_empty(),
            "case {case}: expected a CreatedTime, got {created_time:?}"
        );

        // "The ID, name, or Amazon Resource Name (ARN) of the connection group": all three
        // reach the one record, and each reports the document the create answered with.
        for (identifier_case, identifier) in [
            ("by id", id.clone()),
            ("by name", create.name.to_string()),
            ("by ARN", expected_arn.clone()),
        ] {
            let read = get_connection_group(&router, &identifier).await;
            assert_eq!(
                read.status, 200,
                "case {case} ({identifier_case}): expected HTTP 200, got {} with {}",
                read.status, read.raw
            );
            assert_eq!(
                read.header("etag"),
                etag,
                "case {case} ({identifier_case}): the read reports the stored version token"
            );
            for member in ["Id", "Name", "Arn", "RoutingEndpoint", "Enabled", "Ipv6Enabled"] {
                assert_eq!(
                    read.root().text_at(member),
                    group.text_at(member),
                    "case {case} ({identifier_case}): {member} must come from the stored \
                     record, got {}",
                    read.raw
                );
            }
        }
    }

    // Every group the creates stored is listed exactly once, with the same values the
    // single-group read answers with.
    let listed = list_connection_groups(&router, &list_body(Some(MAX_ITEMS), None, None)).await;
    assert_eq!(
        listed.status, 200,
        "case the whole listing: expected HTTP 200, got {} with {}",
        listed.status, listed.raw
    );
    let ids: Vec<String> = summary_ids(&listed);
    assert_eq!(
        ids.iter().cloned().collect::<BTreeSet<String>>(),
        minted,
        "case the whole listing: expected every stored group exactly once, got {ids:?}"
    );
    for summary in summaries(&listed) {
        let id = summary.text_at("Id").expect("every summary carries an Id");
        let read = get_connection_group(&router, id).await;
        for member in [
            "Id",
            "Name",
            "Arn",
            "RoutingEndpoint",
            "Enabled",
            "Status",
            "IsDefault",
            "AnycastIpListId",
            "CreatedTime",
            "LastModifiedTime",
        ] {
            assert_eq!(
                summary.text_at(member),
                read.root().text_at(member),
                "case the whole listing ({id}): the summary's {member} must be the stored \
                 one, got {}",
                listed.raw
            );
        }
        assert_eq!(
            summary.text_at("ETag"),
            Some(read.header("etag")),
            "case the whole listing ({id}): the summary reports the stored version token"
        );
    }
}

// ---------------------------------------------------------------------------
// Pagination
// ---------------------------------------------------------------------------

/// Walk every page of the listing at `page_size` and answer the ids in the order they
/// were reported.
async fn walk_pages(router: &MultiServiceRouter, page_size: i32) -> Vec<String> {
    let case = format!("page size {page_size}");
    let mut ids: Vec<String> = Vec::new();
    let mut marker: Option<String> = None;
    for _ in 0..64 {
        let body = list_body(Some(page_size), marker.as_deref(), None);
        let answer = list_connection_groups(router, &body).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected HTTP 200, got {} with {}",
            answer.status, answer.raw
        );
        let page = summary_ids(&answer);
        assert!(
            page.len() <= page_size as usize,
            "case {case}: a page holds at most {page_size} groups, got {page:?}"
        );
        ids.extend(page.iter().cloned());
        match answer.root().text_at("NextMarker") {
            Some(next) => {
                assert_eq!(
                    Some(next.to_string()),
                    page.last().cloned(),
                    "case {case}: the NextMarker is the id the page ended at, got {}",
                    answer.raw
                );
                marker = Some(next.to_string());
            }
            // No `NextMarker`: this was the last page.
            None => return ids,
        }
    }
    panic!("case {case}: the page walk did not terminate");
}

/// The listing cuts a page at `MaxItems`, resumes after the `Marker`, and hands back a
/// `NextMarker` exactly when groups remain — so walking the pages at ANY admissible page
/// size enumerates the whole collection exactly once, in one stable order.
#[tokio::test]
async fn the_listing_pages_at_max_items_and_resumes_from_the_marker() {
    let router = emulator();
    let created = create_every_group(&router).await;
    let all: Vec<String> = {
        let mut sorted = created.clone();
        sorted.sort();
        sorted
    };

    // One row per page size from the smallest admissible one to one past the collection's
    // size, plus both the largest size this service serves and one past it.
    for page_size in [1, 2, 3, 5, 6, 7, MAX_ITEMS, MAX_ITEMS + 1] {
        let walked = walk_pages(&router, page_size).await;
        assert_eq!(
            walked, all,
            "case page size {page_size}: expected every group exactly once in id order, got \
             {walked:?}"
        );
    }

    // A listing that names no `MaxItems` at all serves the whole collection: it is smaller
    // than this service's own page ceiling.
    let answer = list_connection_groups(&router, &list_body(None, None, None)).await;
    assert_eq!(
        answer.status, 200,
        "case no MaxItems: expected HTTP 200, got {} with {}",
        answer.status, answer.raw
    );
    assert_eq!(
        summary_ids(&answer),
        all,
        "case no MaxItems: expected the whole collection, got {}",
        answer.raw
    );
    assert_absent(answer.root(), "NextMarker", "no MaxItems", "the last page");

    // The `Marker` the last page ended at resumes AFTER it: nothing is left to report.
    let last = all.last().expect("the table creates groups").clone();
    let answer = list_connection_groups(&router, &list_body(None, Some(&last), None)).await;
    assert_eq!(
        answer.status, 200,
        "case the marker of the last group: expected HTTP 200, got {} with {}",
        answer.status, answer.raw
    );
    assert!(
        summaries(&answer).is_empty(),
        "case the marker of the last group: expected no groups after the last one, got {}",
        answer.raw
    );
}

// ---------------------------------------------------------------------------
// The association filter, and the entity it names
// ---------------------------------------------------------------------------

/// An `AnycastIpListId` names an Anycast static IP list: on the listing's
/// `AssociationFilter` and on a create alike, a value this emulator cannot resolve to one
/// is an invalid argument, and the refusal leaves the store exactly as it was. A filter
/// element naming no list narrows nothing.
#[tokio::test]
async fn an_anycast_ip_list_this_service_cannot_resolve_is_an_invalid_argument() {
    let router = emulator();
    let created = create_every_group(&router).await;
    let all: BTreeSet<String> = created.iter().cloned().collect();

    // The filter names an Anycast static IP list; this emulator holds none, so each of
    // these is the modelled `InvalidArgument` rather than an empty page. The sentence is
    // the filter's own, so every row answers the same one whatever it submitted.
    for (case, filter) in [
        ("a list id in CloudFront's shape", ANYCAST_LIST_ABSENT),
        ("another list id", ANYCAST_LIST_ALSO_ABSENT),
        ("a value of no list shape at all", "not a valid id !!"),
    ] {
        let answer = list_connection_groups(&router, &list_body(None, None, Some(filter))).await;
        assert_eq!(
            answer.status, 400,
            "case filter {case}: expected HTTP 400, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "InvalidArgument",
            "case filter {case}: expected InvalidArgument, got {}",
            answer.raw
        );
        assert_eq!(
            answer.root().find_text("Message"),
            Some(RECORDED_INVALID_FILTER_MESSAGE),
            "case filter {case}: expected the filter's own recorded sentence, got {}",
            answer.raw
        );
    }

    // A filter element that names no list narrows nothing: the whole collection is served.
    for (case, filter) in [
        ("an empty filter element", Some("")),
        ("no filter at all", None),
    ] {
        let answer = list_connection_groups(&router, &list_body(None, None, filter)).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected HTTP 200, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            summary_ids(&answer).into_iter().collect::<BTreeSet<String>>(),
            all,
            "case {case}: expected the whole collection, got {}",
            answer.raw
        );
    }

    // The create applies the same rule to the same member, and stores nothing when it
    // refuses.
    for (case, anycast_ip_list_id) in [
        ("a list id in CloudFront's shape", ANYCAST_LIST_ABSENT),
        ("a value of no list shape at all", "not a valid id !!"),
    ] {
        let create = Create {
            case,
            name: "anycast-group",
            ipv6_enabled: None,
            enabled: None,
            anycast_ip_list_id: Some(anycast_ip_list_id),
            tags: Vec::new(),
        };
        let answer = create_connection_group(&router, &create.xml()).await;
        assert_eq!(
            answer.status, 400,
            "case create with {case}: expected HTTP 400, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "InvalidArgument",
            "case create with {case}: expected InvalidArgument, got {}",
            answer.raw
        );
    }
    let answer = list_connection_groups(&router, &list_body(None, None, None)).await;
    assert_eq!(
        summary_ids(&answer).into_iter().collect::<BTreeSet<String>>(),
        all,
        "case after the refused creates: the store holds exactly what the creates stored, \
         got {}",
        answer.raw
    );
    let refused_name = get_connection_group(&router, "anycast-group").await;
    assert_eq!(
        refused_name.status, 404,
        "case after the refused creates: the refused name names no group, got {} with {}",
        refused_name.status, refused_name.raw
    );
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// A `MaxItems` that names no page is refused, a `Marker` this service never handed out is
/// no resume point, and a well-formed marker naming no stored group is an entity that does
/// not exist.
#[tokio::test]
async fn a_request_that_names_no_page_is_refused() {
    let router = emulator();
    let created = create_every_group(&router).await;

    // Below the admissible `MaxItems` range: the member bounds the page, so a negative
    // count bounds nothing and names no page at all.
    for (case, max_items) in [("negative", -1), ("far negative", i32::MIN)] {
        let answer = list_connection_groups(&router, &list_body(Some(max_items), None, None)).await;
        assert_eq!(
            answer.status, 400,
            "case MaxItems {case}: expected HTTP 400, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "InvalidArgument",
            "case MaxItems {case}: expected InvalidArgument, got {}",
            answer.raw
        );
    }

    // The smallest admissible page size is served.
    let answer = list_connection_groups(&router, &list_body(Some(1), None, None)).await;
    assert_eq!(
        answer.status, 200,
        "case MaxItems one: expected HTTP 200, got {} with {}",
        answer.status, answer.raw
    );
    assert_eq!(
        summaries(&answer).len(),
        1,
        "case MaxItems one: expected one group on the page, got {}",
        answer.raw
    );

    // A marker that is no connection group id at all was never a resume point.
    for (case, marker) in [
        ("a word", "next-please"),
        ("a distribution id", "E1234567890ABC"),
        ("the prefix alone", CONNECTION_GROUP_ID_PREFIX),
        ("a short body", "cg_ABC"),
        ("a lowercase body", "cg_abcdefghijklm"),
    ] {
        let answer = list_connection_groups(&router, &list_body(None, Some(marker), None)).await;
        assert_eq!(
            answer.status, 400,
            "case marker {case}: expected HTTP 400, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "InvalidArgument",
            "case marker {case}: expected InvalidArgument, got {}",
            answer.raw
        );
    }

    // A marker in the shape this service mints, naming no stored group: the entity is not
    // there.
    let absent = format!("{CONNECTION_GROUP_ID_PREFIX}ZZZZZZZZZZZZZ");
    assert!(
        !created.contains(&absent),
        "case an absent marker: the fixture must not hold {absent}"
    );
    let answer = list_connection_groups(&router, &list_body(None, Some(&absent), None)).await;
    assert_eq!(
        answer.status, 404,
        "case an absent marker: expected HTTP 404, got {} with {}",
        answer.status, answer.raw
    );
    assert_eq!(
        answer.error_code(),
        "EntityNotFound",
        "case an absent marker: expected EntityNotFound, got {}",
        answer.raw
    );

    // And the same rule for a single-group read: an identifier no group answers to.
    for (case, identifier) in [
        ("an absent id", absent.clone()),
        ("an absent name", "no-such-group".to_string()),
        (
            "an absent ARN",
            format!("arn:aws:cloudfront::{ACCOUNT_ID}:connection-group/{absent}"),
        ),
    ] {
        let answer = get_connection_group(&router, &identifier).await;
        assert_eq!(
            answer.status, 404,
            "case {case}: expected HTTP 404, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "EntityNotFound",
            "case {case}: expected EntityNotFound, got {}",
            answer.raw
        );
    }
}

/// A name is "unique within your Amazon Web Services account", and a name at all is
/// required: neither a repeat nor a blank one creates a second group, and the listing is
/// unchanged by the refusal.
#[tokio::test]
async fn a_name_identifies_one_group_and_a_blank_name_identifies_none() {
    let router = emulator();
    let created = create_every_group(&router).await;

    for (case, name) in [
        ("the first row's name", creates()[0].name.to_string()),
        ("the last row's name", creates()[5].name.to_string()),
    ] {
        let body = format!(
            "<CreateConnectionGroupRequest \
             xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
             <Name>{name}</Name></CreateConnectionGroupRequest>"
        );
        let answer = create_connection_group(&router, &body).await;
        assert_eq!(
            answer.status, 409,
            "case {case}: expected HTTP 409, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "EntityAlreadyExists",
            "case {case}: expected EntityAlreadyExists, got {}",
            answer.raw
        );
    }

    for (case, name) in [("empty", ""), ("blank", "   ")] {
        let body = format!(
            "<CreateConnectionGroupRequest \
             xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
             <Name>{name}</Name></CreateConnectionGroupRequest>"
        );
        let answer = create_connection_group(&router, &body).await;
        assert_eq!(
            answer.status, 400,
            "case a {case} name: expected HTTP 400, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "InvalidArgument",
            "case a {case} name: expected InvalidArgument, got {}",
            answer.raw
        );
    }

    // A refused create stored nothing.
    let answer = list_connection_groups(&router, &list_body(None, None, None)).await;
    assert_eq!(
        summary_ids(&answer).into_iter().collect::<BTreeSet<String>>(),
        created.into_iter().collect::<BTreeSet<String>>(),
        "case after the refusals: the store holds exactly the groups the creates stored, \
         got {}",
        answer.raw
    );
}

// ---------------------------------------------------------------------------
// The account's default connection group, and the clock
// ---------------------------------------------------------------------------

/// "When you first create a distribution tenant and you don't specify a connection group,
/// CloudFront will automatically create a default connection group for you": that group is
/// a record like any other — the tenant is routed through it, it is listed, it is readable
/// by each of its identifiers, and it is the only one carrying `IsDefault`. And the CLOCK,
/// not a later call, owes every group `Deployed`.
#[tokio::test]
async fn the_automatic_default_group_is_listed_and_the_clock_deploys_it() {
    let router = emulator();

    // Nothing has needed a connection group yet, so the account holds none.
    let empty = list_connection_groups(&router, &list_body(None, None, None)).await;
    assert_eq!(
        empty.status, 200,
        "case an account with no connection group: expected HTTP 200, got {} with {}",
        empty.status, empty.raw
    );
    assert!(
        summaries(&empty).is_empty(),
        "case an account with no connection group: expected no groups, got {}",
        empty.raw
    );

    // A caller-created group is not the default one, so it does not satisfy a tenant that
    // names none.
    let created = create_connection_group(
        &router,
        &Create {
            case: "a group created before any tenant",
            name: "explicit-group",
            ipv6_enabled: None,
            enabled: None,
            anycast_ip_list_id: None,
            tags: Vec::new(),
        }
        .xml(),
    )
    .await;
    assert_eq!(
        created.status, 201,
        "case a group created before any tenant: expected HTTP 201, got {} with {}",
        created.status, created.raw
    );
    let explicit_id = created
        .root()
        .text_at("Id")
        .expect("the created group carries an Id")
        .to_string();

    // Two tenants that name no connection group: the first one creates the account's
    // default group, the second is routed through that same one.
    let distribution_id = multi_tenant_distribution(&router, "ref-connection-groups").await;
    let mut tenant_groups: BTreeSet<String> = BTreeSet::new();
    for index in 0..2 {
        let body = format!(
            "<CreateDistributionTenantRequest \
             xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
             <DistributionId>{distribution_id}</DistributionId>\
             <Name>cg-tenant-{index}</Name>\
             <Domains><member><Domain>cg-tenant-{index}.example.com</Domain></member></Domains>\
             <Parameters><member><Name>tenantName</Name><Value>acme</Value></member></Parameters>{}\
             </CreateDistributionTenantRequest>",
            managed_certificate_request()
        );
        let answer = create_distribution_tenant(&router, &body).await;
        assert_eq!(
            answer.status, 201,
            "case tenant {index}: expected HTTP 201, got {} with {}",
            answer.status, answer.raw
        );
        tenant_groups.insert(
            answer
                .root()
                .text_at("ConnectionGroupId")
                .expect("a created tenant names its connection group")
                .to_string(),
        );
    }
    assert_eq!(
        tenant_groups.len(),
        1,
        "case two tenants naming no connection group: both are routed through the one default \
         group, got {tenant_groups:?}"
    );
    let default_id = tenant_groups
        .iter()
        .next()
        .expect("the tenants named one group")
        .clone();
    assert_ne!(
        default_id, explicit_id,
        "case a group created before any tenant: a caller-created group is not the default one"
    );

    // The listing now holds both groups, and exactly one of them is the default.
    let listed = list_connection_groups(&router, &list_body(None, None, None)).await;
    assert_eq!(
        summary_ids(&listed).into_iter().collect::<BTreeSet<String>>(),
        BTreeSet::from([default_id.clone(), explicit_id.clone()]),
        "case after the tenants: expected both groups, got {}",
        listed.raw
    );
    let defaults: Vec<&str> = summaries(&listed)
        .iter()
        .filter(|summary| summary.text_at("IsDefault") == Some("true"))
        .map(|summary| summary.text_at("Id").unwrap_or_default())
        .collect();
    assert_eq!(
        defaults,
        vec![default_id.as_str()],
        "case after the tenants: exactly the automatic group is the default one, got {}",
        listed.raw
    );

    // The automatic group is a full record: readable by each identifier, carrying the name
    // this service gives it and a routing endpoint of its own.
    let read = get_connection_group(&router, &default_id).await;
    assert_eq!(
        read.status, 200,
        "case the default group: expected HTTP 200, got {} with {}",
        read.status, read.raw
    );
    assert_eq!(
        read.root().text_at("Name"),
        Some(RECORDED_DEFAULT_GROUP_NAME),
        "case the default group: expected the name this service gives the automatic group, \
         got {}",
        read.raw
    );
    assert_eq!(
        read.root().text_at("IsDefault"),
        Some("true"),
        "case the default group: the automatic group is the default one, got {}",
        read.raw
    );
    let by_name = get_connection_group(&router, RECORDED_DEFAULT_GROUP_NAME).await;
    assert_eq!(
        by_name.root().text_at("Id"),
        Some(default_id.as_str()),
        "case the default group by name: expected the same record, got {}",
        by_name.raw
    );
    assert_eq!(
        read.root().text_at("Status"),
        Some(STATUS_IN_PROGRESS),
        "case the default group: a group that has just been created has not propagated yet, \
         got {}",
        read.raw
    );

    // The CLOCK owes every group the terminal status: nothing else is called in between.
    tokio::time::sleep(PROPAGATION_DELAY + Duration::from_millis(100)).await;
    let settled = list_connection_groups(&router, &list_body(None, None, None)).await;
    for summary in summaries(&settled) {
        assert_eq!(
            summary.text_at("Status"),
            Some(STATUS_DEPLOYED),
            "case after the propagation delay: every group is deployed, got {}",
            settled.raw
        );
    }
    let read = get_connection_group(&router, &explicit_id).await;
    assert_eq!(
        read.root().text_at("Status"),
        Some(STATUS_DEPLOYED),
        "case after the propagation delay: the single-group read reports the same status, got {}",
        read.raw
    );
}
