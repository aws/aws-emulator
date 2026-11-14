// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the BOUNDARIES of a `position`/`limit` collection of this
//! service: the answer a collection that holds nothing makes, and the page a limited read
//! of the documentation-part collection is cut at.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees. [`Gateway`], `create_api`, `page_items` and `created_domain` are this crate's own
//! wire-test harness, reused from `rest_api_lifecycle` and `domain_name_collection`.
//!
//! The rules asserted here, each over a SET of inputs rather than one call:
//!
//! * a collection that holds no element reports no page of elements at all — the member
//!   is left unset, and with no cursor and no echoed page size beside it the whole answer
//!   is `{}` — across every listing of this service, whatever the collection is of;
//! * a limited page of an API's documentation parts holds the parts at the first
//!   ADDRESSES the request's filters select, and reports the address of the part it
//!   carries on at as its cursor, so the part a page reports and the cursor beside it
//!   belong to different entities.

use serde_json::{json, Value};

use crate::domain_name_collection::created_domain;
use crate::rest_api_lifecycle::{create_api, page_items, Answer, Gateway};

/// A GET of `uri`, asserting the 200 every listing of a readable collection answers.
async fn listed(gateway: &Gateway, uri: &str, case: &str) -> Answer {
    let answer = gateway.call("GET", uri, None).await;
    assert_eq!(
        answer.status, 200,
        "case {case}: expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    answer
}

/// `CreateDocumentationPart` for `location`, asserting the modelled 201.
async fn created_part(gateway: &Gateway, api: &str, location: Value, description: &str) {
    let answer = gateway
        .call(
            "POST",
            &format!("/restapis/{api}/documentation/parts"),
            Some(json!({
                "location": location,
                "properties": json!({ "description": description }).to_string(),
            })),
        )
        .await;
    assert_eq!(
        answer.status, 201,
        "case CreateDocumentationPart({location:?}): expected 201, got {} with {:?}",
        answer.status, answer.body
    );
}

/// The five location members of each part on a page, in page order, joined the way a
/// `position` cursor spells one address: type, name, path, method, status code.
fn reported_addresses(body: &Value, case: &str) -> Vec<String> {
    page_items(body, case)
        .iter()
        .map(|item| {
            let location = &item["location"];
            ["type", "name", "path", "method", "statusCode"]
                .iter()
                .map(|member| location[*member].as_str().unwrap_or_default().to_string())
                .collect::<Vec<String>>()
                .join(":")
        })
        .collect()
}

#[tokio::test]
async fn a_collection_that_holds_no_element_reports_no_page_of_elements() {
    let gateway = Gateway::new();
    // A record each of the API-scoped and domain-name-scoped collections hangs off; both
    // are created with none of the collections below.
    let (api, _root) = create_api(&gateway, "empty-collections").await;
    let domain_name = "empty-collections.example.com";
    created_domain(&gateway, domain_name, json!({})).await;

    // Rows span the collections of this service whose elements a caller creates — the
    // account's own, an API's children, and a domain name's mappings, which is the one the
    // recorded body `{}` was observed on — so what is asserted is the envelope rather than
    // one listing's handler. Left out: the collections an account or an API is CREATED
    // holding (a REST API's models and gateway responses, the SDK-type catalog), which
    // never hold nothing.
    let rows: Vec<(&str, String)> = vec![
        ("GetApiKeys", "/apikeys".to_string()),
        ("GetUsagePlans", "/usageplans".to_string()),
        ("GetVpcLinks", "/vpclinks".to_string()),
        ("GetClientCertificates", "/clientcertificates".to_string()),
        (
            "GetDomainNameAccessAssociations",
            "/domainnameaccessassociations".to_string(),
        ),
        (
            "GetBasePathMappings",
            format!("/domainnames/{domain_name}/basepathmappings"),
        ),
        ("GetAuthorizers", format!("/restapis/{api}/authorizers")),
        (
            "GetRequestValidators",
            format!("/restapis/{api}/requestvalidators"),
        ),
        ("GetDeployments", format!("/restapis/{api}/deployments")),
        (
            "GetDocumentationParts",
            format!("/restapis/{api}/documentation/parts"),
        ),
        (
            "GetDocumentationVersions",
            format!("/restapis/{api}/documentation/versions"),
        ),
    ];

    for (case, uri) in &rows {
        let answer = listed(&gateway, uri, case).await;
        assert_eq!(
            answer.body,
            json!({}),
            "case {case}: a collection that holds no element reports nothing at all, got \
             {:?}",
            answer.body
        );
        assert_eq!(
            answer.body.get("item"),
            None,
            "case {case}: expected no page member, got {:?}",
            answer.body
        );
        assert!(
            answer.body["position"].is_null(),
            "case {case}: expected no cursor, got {:?}",
            answer.body["position"]
        );
        assert!(
            answer.body["limit"].is_null(),
            "case {case}: expected no echoed page size, got {:?}",
            answer.body["limit"]
        );
    }

    // The two ACCOUNT-level collections this suite's own records joined, read on an
    // emulator that holds none of either: the same rule, on the listings the rows above
    // cannot stand up empty beside the API and the domain name they need.
    let empty = Gateway::new();
    for (case, uri) in [
        ("GetRestApis", "/restapis?limit=25"),
        ("GetDomainNames", "/domainnames"),
    ] {
        let answer = listed(&empty, uri, case).await;
        assert_eq!(
            answer.body,
            json!({}),
            "case {case}: a collection that holds no element reports nothing at all, got \
             {:?}",
            answer.body
        );
    }
}

#[tokio::test]
async fn a_limited_page_of_documentation_parts_holds_the_parts_its_filters_select() {
    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "documentation-part-window").await;

    // One API documented at four entity types, created in an order that is not the order
    // the collection reports them in. Their addresses — `type:name:path:method:statusCode`
    // — sort API::::, METHOD::/pets:GET:, MODEL:Pet:::, RESOURCE::/pets::.
    for (location, description) in [
        (json!({ "type": "MODEL", "name": "Pet" }), "pet model"),
        (json!({ "type": "RESOURCE", "path": "/pets" }), "the pets"),
        (
            json!({ "type": "METHOD", "path": "/pets", "method": "GET" }),
            "list pets",
        ),
        (json!({ "type": "API" }), "the api"),
    ] {
        created_part(&gateway, &api, location, description).await;
    }

    // Rows are `(query, the addresses the page reports, the cursor it carries on at)`.
    // They span the page sizes a limited read cuts the collection at, each filter the
    // request may narrow it by, and a cursor the collection is resumed at.
    let rows: Vec<(&str, Vec<&str>, Option<&str>)> = vec![
        (
            "limit=1",
            vec!["API::::"],
            // The part the page did NOT report, which is where the next page begins.
            Some("METHOD::/pets:GET:"),
        ),
        (
            "limit=2",
            vec!["API::::", "METHOD::/pets:GET:"],
            Some("MODEL:Pet:::"),
        ),
        (
            "limit=3",
            vec!["API::::", "METHOD::/pets:GET:", "MODEL:Pet:::"],
            Some("RESOURCE::/pets::"),
        ),
        (
            "limit=4",
            vec![
                "API::::",
                "METHOD::/pets:GET:",
                "MODEL:Pet:::",
                "RESOURCE::/pets::",
            ],
            None,
        ),
        // A page cut inside the collection at a cursor the collection itself reported.
        (
            "limit=1&position=MODEL:Pet:::",
            vec!["MODEL:Pet:::"],
            Some("RESOURCE::/pets::"),
        ),
        (
            "limit=2&position=METHOD::/pets:GET:",
            vec!["METHOD::/pets:GET:", "MODEL:Pet:::"],
            Some("RESOURCE::/pets::"),
        ),
        // Narrowed first, then cut: a filtered listing reports the parts of the entities
        // it names and nothing else, even where another entity's part sorts among them.
        ("type=METHOD", vec!["METHOD::/pets:GET:"], None),
        ("type=MODEL&limit=1", vec!["MODEL:Pet:::"], None),
        (
            "path=/pets&limit=1",
            vec!["METHOD::/pets:GET:"],
            Some("RESOURCE::/pets::"),
        ),
        (
            "path=/pets",
            vec!["METHOD::/pets:GET:", "RESOURCE::/pets::"],
            None,
        ),
        ("name=Pet", vec!["MODEL:Pet:::"], None),
        (
            "locationStatus=DOCUMENTED&limit=2",
            vec!["API::::", "METHOD::/pets:GET:"],
            Some("MODEL:Pet:::"),
        ),
        ("locationStatus=UNDOCUMENTED", vec![], None),
    ];

    for (query, addresses, cursor) in &rows {
        let case = format!("GetDocumentationParts?{query}");
        let answer = listed(
            &gateway,
            &format!("/restapis/{api}/documentation/parts?{query}"),
            &case,
        )
        .await;
        assert_eq!(
            reported_addresses(&answer.body, &case),
            addresses
                .iter()
                .map(|address| (*address).to_string())
                .collect::<Vec<String>>(),
            "case {case}: expected the parts at {addresses:?}, got {:?}",
            answer.body
        );
        assert_eq!(
            answer.body["position"].as_str(),
            *cursor,
            "case {case}: expected the cursor {cursor:?}, got {:?}",
            answer.body["position"]
        );
        // The echoed page size travels with the cursor: a request whose page the
        // collection carries on after is told the size it asked for, and the page that
        // ends the collection is told none.
        let echoed = cursor.and_then(|_| {
            query
                .split('&')
                .find_map(|parameter| parameter.strip_prefix("limit="))
                .and_then(|size| size.parse::<i64>().ok())
        });
        assert_eq!(
            answer.body["limit"].as_i64(),
            echoed,
            "case {case}: expected the echoed page size {echoed:?}, got {:?}",
            answer.body["limit"]
        );
    }
}
