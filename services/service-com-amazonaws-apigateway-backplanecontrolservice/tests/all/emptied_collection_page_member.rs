// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the member a `position`/`limit` collection of this service
//! reports its PAGE under when the collection holds no element.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. [`Gateway`], `create_api`, `make_deployable` and
//! `string_member` are this crate's one wire-test harness, reused from
//! `rest_api_lifecycle`; a domain name comes through `domain_name_collection`, a deployed
//! stage through `base_path_mapping`, a snapshot through `documentation_versions` and a
//! VPC link through `vpc_links`, so every element a row creates is created through its own
//! API.
//!
//! The rule, asserted over a TABLE of collections rather than one listing: a paginated
//! collection of this service writes its page member — `item` on the wire — whenever it
//! answers, as the EMPTY list when the collection holds no element, and the base path
//! mappings of a domain name are the one collection whose empty answer leaves the member
//! out, so the whole body that one answers is `{}`.
//!
//! Each row reaches the empty collection by DELETING the one element it created, not by
//! never creating one, so what is asserted is the answer a collection makes once it is
//! emptied — the state a client reaches by cleaning up after itself — and the rows span
//! the account's own collections, an API's children and a domain name's mappings.

use serde_json::{json, Value};

use crate::base_path_mapping::deployed_api;
use crate::documentation_versions::create_version;
use crate::domain_name_collection::created_domain;
use crate::rest_api_lifecycle::{create_api, make_deployable, string_member, Gateway};
use crate::vpc_links::{create_vpc_link, target_arn};

/// The one collection of this service whose empty answer leaves the page member out.
const ITEMLESS_COLLECTION: &str = "GetBasePathMappings";

/// Every collection this suite empties, named by the operation that lists it. Three of
/// them — `GetDeployments`, `GetDocumentationVersions` and `GetVpcLinks` — are
/// API-scoped, API-scoped and account-level respectively; `GetBasePathMappings` is the
/// exception row.
const COLLECTIONS: &[&str] = &[
    "GetRestApis",
    "GetApiKeys",
    "GetUsagePlans",
    "GetClientCertificates",
    "GetVpcLinks",
    "GetDeployments",
    "GetDocumentationVersions",
    ITEMLESS_COLLECTION,
];

/// A POST of `request` to `uri`, asserting `expected`, and answering the created record.
async fn created(gateway: &Gateway, uri: &str, request: Value, expected: u16, case: &str) -> Value {
    let answer = gateway.call("POST", uri, Some(request)).await;
    assert_eq!(
        answer.status, expected,
        "case {case}: expected {expected} from the create, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body
}

/// A DELETE of `uri`, asserting the 202 every delete of this service answers.
async fn deleted(gateway: &Gateway, uri: &str, case: &str) {
    let answer = gateway.call("DELETE", uri, None).await;
    assert_eq!(
        answer.status, 202,
        "case {case}: expected 202 from the delete, got {} with {:?}",
        answer.status, answer.body
    );
}

/// Create exactly one element of `collection` on a fresh emulator, delete it again, and
/// answer the URI the now-empty collection is listed through.
async fn emptied(gateway: &Gateway, collection: &str) -> String {
    match collection {
        "GetRestApis" => {
            let (api, _root) = create_api(gateway, "emptied-api").await;
            deleted(gateway, &format!("/restapis/{api}"), collection).await;
            "/restapis".to_string()
        }
        "GetApiKeys" => {
            let key = created(
                gateway,
                "/apikeys",
                json!({ "name": "emptied-key" }),
                201,
                collection,
            )
            .await;
            let id = string_member(&key, "id", collection);
            deleted(gateway, &format!("/apikeys/{id}"), collection).await;
            "/apikeys".to_string()
        }
        "GetUsagePlans" => {
            let plan = created(
                gateway,
                "/usageplans",
                json!({ "name": "emptied-plan" }),
                201,
                collection,
            )
            .await;
            let id = string_member(&plan, "id", collection);
            deleted(gateway, &format!("/usageplans/{id}"), collection).await;
            "/usageplans".to_string()
        }
        "GetClientCertificates" => {
            let certificate = created(gateway, "/clientcertificates", json!({}), 201, collection)
                .await;
            let id = string_member(&certificate, "clientCertificateId", collection);
            deleted(gateway, &format!("/clientcertificates/{id}"), collection).await;
            "/clientcertificates".to_string()
        }
        "GetVpcLinks" => {
            let answer = create_vpc_link(
                gateway,
                json!({ "name": "emptied-link", "targetArns": [target_arn(0)] }),
            )
            .await;
            assert_eq!(
                answer.status, 202,
                "case {collection}: expected 202 from CreateVpcLink, got {} with {:?}",
                answer.status, answer.body
            );
            let id = string_member(&answer.body, "id", collection);
            deleted(gateway, &format!("/vpclinks/{id}"), collection).await;
            "/vpclinks".to_string()
        }
        "GetDeployments" => {
            let (api, _root) = create_api(gateway, "emptied-deployments").await;
            make_deployable(gateway, &api).await;
            let deployment = created(
                gateway,
                &format!("/restapis/{api}/deployments"),
                json!({}),
                201,
                collection,
            )
            .await;
            let id = string_member(&deployment, "id", collection);
            deleted(
                gateway,
                &format!("/restapis/{api}/deployments/{id}"),
                collection,
            )
            .await;
            format!("/restapis/{api}/deployments")
        }
        "GetDocumentationVersions" => {
            let (api, _root) = create_api(gateway, "emptied-versions").await;
            let answer = create_version(gateway, &api, json!({ "documentationVersion": "v1" })).await;
            assert_eq!(
                answer.status, 201,
                "case {collection}: expected 201 from CreateDocumentationVersion, got {} \
                 with {:?}",
                answer.status, answer.body
            );
            deleted(
                gateway,
                &format!("/restapis/{api}/documentation/versions/v1"),
                collection,
            )
            .await;
            format!("/restapis/{api}/documentation/versions")
        }
        ITEMLESS_COLLECTION => {
            let (api, stage) = deployed_api(gateway, "emptied-mappings", "prod").await;
            let domain_name = "emptied-mappings.example.com";
            created_domain(gateway, domain_name, json!({})).await;
            created(
                gateway,
                &format!("/domainnames/{domain_name}/basepathmappings"),
                json!({ "basePath": "v1", "restApiId": api, "stage": stage }),
                201,
                collection,
            )
            .await;
            deleted(
                gateway,
                &format!("/domainnames/{domain_name}/basepathmappings/v1"),
                collection,
            )
            .await;
            format!("/domainnames/{domain_name}/basepathmappings")
        }
        other => panic!("case {other}: this suite empties no such collection"),
    }
}

#[tokio::test]
async fn an_emptied_collection_reports_the_empty_page_and_nothing_beside_it() {
    for collection in COLLECTIONS {
        let gateway = Gateway::new();
        let uri = emptied(&gateway, collection).await;
        let case = format!("{collection}(emptied)");

        let answer = gateway.call("GET", &uri, None).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected 200, got {} with {:?}",
            answer.status, answer.body
        );
        // The page member, and nothing beside it: an emptied collection has no element
        // after the page it reports, so it carries on at no cursor and echoes no page
        // size.
        let expected = if *collection == ITEMLESS_COLLECTION {
            json!({})
        } else {
            json!({ "item": [] })
        };
        assert_eq!(
            answer.body, expected,
            "case {case}: expected {expected:?}, got {:?}",
            answer.body
        );
    }
}

#[tokio::test]
async fn an_emptied_collection_asked_for_a_page_size_reports_the_same_empty_page() {
    // The page sizes a request may ask of a collection: the smallest, the documented
    // default and the published maximum — "The maximum number of returned results per
    // page. The default value is 25 and the maximum value is 500"
    // (`smithy_query(query="doc", shape="GetVpcLinksRequest", member="limit")`, the same
    // sentence every paginated `limit` member of this service carries). The page a request
    // asks for cannot be filled by a collection that holds nothing, so the answer is the
    // same empty one whatever size it asked for.
    for collection in COLLECTIONS {
        for size in [1, 25, 500] {
            let gateway = Gateway::new();
            let uri = emptied(&gateway, collection).await;
            let case = format!("{collection}(emptied, limit={size})");

            let answer = gateway.call("GET", &format!("{uri}?limit={size}"), None).await;
            assert_eq!(
                answer.status, 200,
                "case {case}: expected 200, got {} with {:?}",
                answer.status, answer.body
            );
            let expected = if *collection == ITEMLESS_COLLECTION {
                json!({})
            } else {
                json!({ "item": [] })
            };
            assert_eq!(
                answer.body, expected,
                "case {case}: expected {expected:?}, got {:?}",
                answer.body
            );
        }
    }
}
