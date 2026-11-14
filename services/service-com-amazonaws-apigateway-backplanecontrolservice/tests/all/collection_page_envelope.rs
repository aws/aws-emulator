// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the `position`/`limit` PAGE ENVELOPE every paginated collection
//! of this service answers in, and for the cursor the documentation-part collection is
//! walked by.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. [`Gateway`], `create_api`, `page_items` and
//! `string_member` are this crate's one wire-test harness, reused from
//! `rest_api_lifecycle`.
//!
//! The rules these cases assert, each over a SET of inputs rather than one call:
//!
//! * a page reports the cursor the collection CARRIES ON AT — the first element the page
//!   did not report — and the page that follows begins at that element, so a walk hands
//!   back every element exactly once;
//! * a page the collection holds nothing after reports no cursor, which is what ends a
//!   walk, and no echoed page size either; a page it does hold something after echoes the
//!   page size the request asked for, and nothing when the request asked for none;
//! * a collection that holds no element leaves the member that reports the page UNSET,
//!   while the page past the last element of a collection that does hold elements is the
//!   present-but-empty page;
//! * the documentation-part collection is ordered by the ADDRESS of the API entity each
//!   part documents, which is the address its cursor spells.
//!
//! The rows span four collections of three different cursor kinds — a minted resource id
//! (`GetRestApis`, `GetApiKeys`), a caller-chosen name (`GetModels`) and a composite
//! location (`GetDocumentationParts`) — crossed with the page sizes the `limit` member
//! publishes, each published bound and one past each.

use serde_json::{json, Value};
use service_com_amazonaws_apigateway_backplanecontrolservice::state::{
    DEFAULT_PAGE_LIMIT, MAX_PAGE_LIMIT,
};

use crate::rest_api_lifecycle::{create_api, page_items, string_member, Answer, Gateway};

/// The collections this suite drives, named by the operation that lists each.
const COLLECTIONS: &[&str] = &[
    "GetRestApis",
    "GetApiKeys",
    "GetModels",
    "GetDocumentationParts",
];

/// One collection stood up with `count` elements: the listing's URI, and the cursor of
/// each element in the order the collection reports them.
struct Listing {
    uri: String,
    cursors: Vec<String>,
}

impl Listing {
    /// The listing with `query` appended to its URI.
    fn query(&self, query: &str) -> String {
        if query.is_empty() {
            self.uri.clone()
        } else {
            format!("{}?{query}", self.uri)
        }
    }
}

/// Create `count` elements of `collection` on a fresh emulator and answer the listing
/// they are read back through.
///
/// Each element differs from its siblings in the member its own collection keys it by, so
/// the sequence the cursors walk is a sequence of distinct elements.
async fn stand_up(gateway: &Gateway, collection: &str, count: usize) -> Listing {
    match collection {
        "GetRestApis" => {
            let mut cursors = Vec::new();
            for index in 0..count {
                let (api, _root) = create_api(gateway, &format!("envelope-api-{index}")).await;
                cursors.push(api);
            }
            Listing {
                uri: "/restapis".to_string(),
                cursors,
            }
        }
        "GetApiKeys" => {
            let mut cursors = Vec::new();
            for index in 0..count {
                let case = format!("CreateApiKey(#{index})");
                let created = created(
                    gateway,
                    "/apikeys",
                    json!({ "name": format!("envelope-key-{index}") }),
                    &case,
                )
                .await;
                cursors.push(string_member(&created, "id", &case));
            }
            Listing {
                uri: "/apikeys".to_string(),
                cursors,
            }
        }
        "GetModels" => {
            let (api, _root) = create_api(gateway, "envelope-models").await;
            // A REST API is CREATED holding the models API Gateway seeds it with, so the
            // collection is emptied first: this suite is about the page envelope, and a
            // row that asks for `count` elements has to get a collection of exactly that
            // many — including the empty one.
            for name in crate::models::default_model_names() {
                let answer = gateway
                    .call("DELETE", &format!("/restapis/{api}/models/{name}"), None)
                    .await;
                assert_eq!(
                    answer.status, 202,
                    "case DeleteModel({name}): expected 202, got {} with {:?}",
                    answer.status, answer.body
                );
            }
            let mut cursors = Vec::new();
            for index in 0..count {
                let case = format!("CreateModel(#{index})");
                let created = created(
                    gateway,
                    &format!("/restapis/{api}/models"),
                    json!({
                        "name": format!("EnvelopeModel{index:03}"),
                        "contentType": "application/json",
                    }),
                    &case,
                )
                .await;
                cursors.push(string_member(&created, "name", &case));
            }
            Listing {
                uri: format!("/restapis/{api}/models"),
                cursors,
            }
        }
        "GetDocumentationParts" => {
            let (api, _root) = create_api(gateway, "envelope-parts").await;
            let mut cursors = Vec::new();
            for index in 0..count {
                let case = format!("CreateDocumentationPart(#{index})");
                created(
                    gateway,
                    &format!("/restapis/{api}/documentation/parts"),
                    json!({
                        "location": {
                            "type": "QUERY_PARAMETER",
                            "path": "/items",
                            "method": "GET",
                            "name": format!("q{index:03}"),
                        },
                        "properties": json!({ "description": case }).to_string(),
                    }),
                    &case,
                )
                .await;
                // A documentation part is cursored by the LOCATION of the entity it
                // documents, not by its minted id — and these locations differ only in
                // the parameter's name, so they are reported in that name's order.
                cursors.push(format!("QUERY_PARAMETER:q{index:03}:/items:GET:"));
            }
            Listing {
                uri: format!("/restapis/{api}/documentation/parts"),
                cursors,
            }
        }
        other => panic!("case {other}: this suite drives no such collection"),
    }
}

/// A create request, asserting the modelled 201 and answering the created record.
async fn created(gateway: &Gateway, uri: &str, request: Value, case: &str) -> Value {
    let answer = gateway.call("POST", uri, Some(request)).await;
    assert_eq!(
        answer.status, 201,
        "case {case}: expected 201, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body
}

/// One page of `listing`, asserting the 200 every page of a resolvable cursor answers.
async fn page(gateway: &Gateway, listing: &Listing, query: &str, case: &str) -> Answer {
    let answer = gateway.call("GET", &listing.query(query), None).await;
    assert_eq!(
        answer.status, 200,
        "case {case}: expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    answer
}

/// The page sizes every row is crossed with: one, a few, the documented default, the
/// published maximum, one past that maximum and one below the published minimum — the
/// last two are clamped into the range rather than refused.
///
/// Source: every paginated `limit` member of this service — "The maximum number of
/// returned results per page. The default value is 25 and the maximum value is 500".
fn page_sizes() -> Vec<i64> {
    vec![
        1,
        2,
        3,
        i64::from(DEFAULT_PAGE_LIMIT),
        i64::from(MAX_PAGE_LIMIT),
        i64::from(MAX_PAGE_LIMIT) + 1,
        0,
    ]
}

#[tokio::test]
async fn a_page_reports_the_cursor_the_collection_carries_on_at_and_echoes_the_size_beside_it() {
    // Five elements: more than the smallest page size by several, so a page is cut inside
    // the collection for the smaller sizes and holds the whole of it for the larger ones.
    const COUNT: usize = 5;

    for collection in COLLECTIONS {
        for size in page_sizes() {
            let gateway = Gateway::new();
            let listing = stand_up(&gateway, collection, COUNT).await;
            let effective =
                usize::try_from(size.clamp(1, i64::from(MAX_PAGE_LIMIT))).unwrap_or(1);
            let case = format!("{collection}(limit={size})");

            let mut walked: Vec<String> = Vec::new();
            let mut cursor: Option<String> = None;
            for round in 0..=COUNT {
                let query = match &cursor {
                    None => format!("limit={size}"),
                    Some(position) => format!("limit={size}&position={position}"),
                };
                let answer = page(&gateway, &listing, &query, &case).await;
                let items = page_items(&answer.body, &case);
                let reported = answer.body["position"].as_str().map(str::to_owned);
                assert!(
                    items.len() <= effective,
                    "case {case}: round {round} holds at most {effective} element(s), got {}",
                    items.len()
                );
                let taken = items.len();
                walked.extend(listing.cursors[walked.len()..walked.len() + taken].to_vec());
                // The cursor is where the collection CARRIES ON: the first element this
                // page did not report, and nothing once the walk reached the end.
                assert_eq!(
                    reported.as_deref(),
                    listing.cursors.get(walked.len()).map(String::as_str),
                    "case {case}: round {round} reports the cursor it carries on at, got \
                     {reported:?}"
                );
                // The echoed page size travels with that cursor: a page the collection
                // carries on after states what the caller asked of it, and the page that
                // ends the collection echoes nothing.
                assert_eq!(
                    answer.body["limit"].as_i64(),
                    reported.as_ref().map(|_| size),
                    "case {case}: round {round} echoes the page size beside the cursor, \
                     got {:?} for {reported:?}",
                    answer.body["limit"]
                );
                match reported {
                    None => break,
                    Some(position) => cursor = Some(position),
                }
            }
            assert_eq!(
                walked, listing.cursors,
                "case {case}: the walk reports every element exactly once, in order, got \
                 {walked:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_listing_that_asks_for_no_page_size_is_told_none() {
    // Rows: a collection whose whole content fits the documented default page size, and
    // the same collection with no element at all.
    for collection in COLLECTIONS {
        for count in [3, 0] {
            let gateway = Gateway::new();
            let listing = stand_up(&gateway, collection, count).await;
            let case = format!("{collection}(no limit, {count} element(s))");
            let answer = page(&gateway, &listing, "", &case).await;
            assert!(
                answer.body["limit"].is_null(),
                "case {case}: a request that asked for no page size is echoed none, got \
                 {:?}",
                answer.body["limit"]
            );
        }
    }
}

#[tokio::test]
async fn a_collection_that_holds_nothing_leaves_the_page_member_unset() {
    for collection in COLLECTIONS {
        let gateway = Gateway::new();

        // A collection that holds NO element has no page of elements to report, so the
        // member is left out of the answer rather than written as the empty list.
        let empty = stand_up(&gateway, collection, 0).await;
        let case = format!("{collection}(the empty collection)");
        let answer = page(&gateway, &empty, "", &case).await;
        assert_eq!(
            answer.body.get("item"),
            None,
            "case {case}: a collection with no element reports no page member, got {:?}",
            answer.body
        );
        assert!(
            answer.body["position"].is_null(),
            "case {case}: a collection with no element reports no cursor, got {:?}",
            answer.body["position"]
        );

        // A collection that HOLDS elements, walked to its last one, ends the walk there:
        // the last page reports the last element and no cursor at all.
        let gateway = Gateway::new();
        let listing = stand_up(&gateway, collection, 3).await;
        let last = listing
            .cursors
            .last()
            .expect("the row stood up three elements")
            .clone();
        let case = format!("{collection}(position={last})");
        let answer = page(&gateway, &listing, &format!("position={last}&limit=2"), &case).await;
        assert_eq!(
            page_items(&answer.body, &case).len(),
            1,
            "case {case}: the page beginning at the last element holds it alone, got {:?}",
            answer.body
        );
        assert!(
            answer.body["position"].is_null(),
            "case {case}: the page that ends the collection reports no cursor, got {:?}",
            answer.body["position"]
        );
        assert!(
            answer.body["limit"].is_null(),
            "case {case}: the page that ends the collection echoes no page size, got {:?}",
            answer.body["limit"]
        );
    }
}

#[tokio::test]
async fn the_page_past_the_last_documentation_part_is_the_present_but_empty_page() {
    // The documentation-part cursor is a POINT in the collection's order rather than a
    // record of it, so a caller can ask for the window past the last part — and a
    // collection that holds parts reports that window as the page it is: present, and
    // empty.
    let gateway = Gateway::new();
    let listing = stand_up(&gateway, "GetDocumentationParts", 3).await;
    for cursor in [
        // Past every part of this API by name, and past every entity type there is.
        "QUERY_PARAMETER:zzz:/items:GET:",
        "ZZZ::::",
    ] {
        let case = format!("GetDocumentationParts(position={cursor})");
        let answer = page(&gateway, &listing, &format!("position={cursor}"), &case).await;
        assert_eq!(
            answer.body.get("item"),
            Some(&Value::Array(Vec::new())),
            "case {case}: the page past the last part holds nothing, got {:?}",
            answer.body
        );
        assert!(
            answer.body["position"].is_null(),
            "case {case}: the page past the last part reports no cursor, got {:?}",
            answer.body["position"]
        );
    }
}

#[tokio::test]
async fn documentation_parts_are_ordered_by_the_entity_each_documents_and_cursored_by_it() {
    // One known collection, spanning the entity types, paths, methods, status codes and
    // names a location can name — created in an order that is NOT the order the
    // collection reports them in, so what is asserted is the order itself.
    //
    // The second column is the cursor the part's own address spells: the location's five
    // members — type, name, path, method, status code — joined by `:`, with an empty
    // segment for every member the entity type leaves unset. The first two rows are the
    // two forms API Gateway's own walks hand back for a resource and for a model.
    let collection: &[(Value, &str)] = &[
        (
            json!({ "type": "RESPONSE_HEADER", "path": "/widgets", "method": "GET",
                    "statusCode": "404", "name": "X-Trace" }),
            "RESPONSE_HEADER:X-Trace:/widgets:GET:404",
        ),
        (
            json!({ "type": "RESOURCE", "path": "/widgets" }),
            "RESOURCE::/widgets::",
        ),
        (
            json!({ "type": "MODEL", "name": "Widget" }),
            "MODEL:Widget:::",
        ),
        (
            json!({ "type": "METHOD", "path": "/widgets", "method": "POST" }),
            "METHOD::/widgets:POST:",
        ),
        (
            json!({ "type": "METHOD", "path": "/widgets", "method": "GET" }),
            "METHOD::/widgets:GET:",
        ),
        (json!({ "type": "API" }), "API::::"),
        (
            json!({ "type": "AUTHORIZER", "name": "guard" }),
            "AUTHORIZER:guard:::",
        ),
        (
            json!({ "type": "QUERY_PARAMETER", "path": "/widgets", "method": "GET",
                    "name": "page" }),
            "QUERY_PARAMETER:page:/widgets:GET:",
        ),
        (
            json!({ "type": "RESPONSE", "path": "/widgets", "method": "GET",
                    "statusCode": "200" }),
            "RESPONSE::/widgets:GET:200",
        ),
    ];

    // The order the collection reports them in: the ADDRESS of the entity each part
    // documents, segment by segment — the entity type, then the entity's name, path,
    // method and status code — which is the order of the cursors themselves.
    let expected: &[&str] = &[
        "API::::",
        "AUTHORIZER:guard:::",
        "METHOD::/widgets:GET:",
        "METHOD::/widgets:POST:",
        "MODEL:Widget:::",
        "QUERY_PARAMETER:page:/widgets:GET:",
        "RESOURCE::/widgets::",
        "RESPONSE::/widgets:GET:200",
        "RESPONSE_HEADER:X-Trace:/widgets:GET:404",
    ];

    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "documentation-part-order").await;
    let uri = format!("/restapis/{api}/documentation/parts");
    for (location, cursor) in collection {
        created(
            &gateway,
            &uri,
            json!({
                "location": location,
                "properties": json!({ "description": *cursor }).to_string(),
            }),
            &format!("CreateDocumentationPart({cursor})"),
        )
        .await;
    }

    // Walked one part at a time, every page reports the part the walk is at and the
    // cursor of the part it carries on at, and the walk reports the collection in the
    // order above exactly once.
    let mut walked: Vec<String> = Vec::new();
    let mut cursor: Option<String> = None;
    for round in 0..=collection.len() {
        let query = match &cursor {
            None => "limit=1".to_string(),
            Some(position) => format!("limit=1&position={position}"),
        };
        let case = format!("GetDocumentationParts(round {round})");
        let answer = gateway.call("GET", &format!("{uri}?{query}"), None).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected 200, got {} with {:?}",
            answer.status, answer.body
        );
        let items = page_items(&answer.body, &case);
        let reported = answer.body["position"].as_str().map(str::to_owned);
        assert_eq!(
            items.len(),
            1,
            "case {case}: a page of one holds the part the walk is at, got {:?}",
            answer.body
        );
        // Each part was created with its own location cursor as its content map's
        // description, so the part on the page names the address it sorts at.
        let properties = string_member(&items[0], "properties", &case);
        let content: Value = serde_json::from_str(&properties).unwrap_or_else(|error| {
            panic!("case {case}: expected a content map, got {error} for {properties}")
        });
        walked.push(
            content["description"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case}: expected a description, got {content:?}"))
                .to_string(),
        );
        assert_eq!(
            reported.as_deref(),
            expected.get(walked.len()).copied(),
            "case {case}: the cursor a page reports is the location of the part it carries \
             on at, got {reported:?}"
        );
        match reported {
            None => break,
            Some(position) => cursor = Some(position),
        }
    }
    assert_eq!(
        walked, expected,
        "case GetDocumentationParts: expected the parts in the order of the addresses of \
         the entities they document, got {walked:?}"
    );
}
