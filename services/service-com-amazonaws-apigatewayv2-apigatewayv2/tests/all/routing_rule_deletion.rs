// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `DeleteRoutingRule`: the removal of one routing rule from
//! the custom domain name that owns it.
//!
//! Every test drives the real `restJson1` request path — `MultiServiceRouter::dispatch`
//! over a signed `http::Request` — so what is asserted is the wire behaviour a client
//! sees, not a handler called directly. The rules a test deletes are seeded through
//! `CreateRoutingRule` and read back through `GetRoutingRule`/`ListRoutingRules`, whose
//! own suites are `routing_rule_lifecycle`, `routing_rule_read_back` and
//! `routing_rule_update`.
//!
//! The rules asserted here, each over a TABLE rather than over one request:
//!
//! * `DeleteRoutingRule` removes exactly the rule its path addresses and answers the
//!   modelled 204 with no body: a later `GetRoutingRule` of that id is refused as missing
//!   and a later `ListRoutingRules` reports every OTHER rule of the domain name
//!   unchanged.
//! * A request whose path addresses a routing rule the named domain name does not hold —
//!   an id nothing minted, a rule another domain name holds, or a rule this call already
//!   removed — is refused with the modelled `NotFoundException` (404), and so is one
//!   naming a custom domain name the account does not hold or naming it under a
//!   `DomainNameId` that is not that domain name's own.
//! * A refused delete removes nothing.
//!
//! The rows vary what the deletion generalises over: how many rules the domain name
//! holds, which of them the path addresses (the first, one in the middle, the last), the
//! form of an unusable identifier, and whether the domain name is addressed by name alone
//! or by name and its own `DomainNameId`.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_apigatewayv2_apigatewayv2::state::{
    INVALID_DOMAIN_NAME_IDENTIFIER, ROUTING_RULE_NOT_FOUND,
};

use crate::api_lifecycle::{as_str, call_in_region, error_code};

/// The region every request in this suite is signed for. Deliberately NOT the emulator's
/// `us-east-1` default, so nothing region-derived can be hardcoded.
const REGION: &str = "ap-southeast-2";

/// The custom domain name whose rules a test deletes.
const DOMAIN: &str = "deleted.example.com";

/// A second seeded domain name, so a row can address a rule that exists — under the other
/// domain name — and so a delete can be shown to leave its neighbour alone.
const OTHER_DOMAIN: &str = "neighbour.example.com";

/// The stage every seeded api carries.
const STAGE: &str = "dev";

/// Send one request signed for [`REGION`] and answer `(status, body)`. An empty body
/// parses to `Value::Null`, which is how the modelled 204 is asserted.
async fn call(
    emulator: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (u16, Value) {
    call_in_region(emulator, REGION, method, uri, body).await
}

async fn create_routing_rule(
    emulator: &MultiServiceRouter,
    domain: &str,
    request: &Value,
) -> (u16, Value) {
    call(
        emulator,
        "POST",
        &format!("/v2/domainnames/{domain}/routingrules"),
        Some(request.clone()),
    )
    .await
}

async fn get_routing_rule(
    emulator: &MultiServiceRouter,
    domain: &str,
    routing_rule_id: &str,
    query: &str,
) -> (u16, Value) {
    call(
        emulator,
        "GET",
        &format!("/v2/domainnames/{domain}/routingrules/{routing_rule_id}{query}"),
        None,
    )
    .await
}

async fn delete_routing_rule(
    emulator: &MultiServiceRouter,
    domain: &str,
    routing_rule_id: &str,
    query: &str,
) -> (u16, Value) {
    call(
        emulator,
        "DELETE",
        &format!("/v2/domainnames/{domain}/routingrules/{routing_rule_id}{query}"),
        None,
    )
    .await
}

async fn list_routing_rules(
    emulator: &MultiServiceRouter,
    domain: &str,
    query: &str,
) -> (u16, Value) {
    call(
        emulator,
        "GET",
        &format!("/v2/domainnames/{domain}/routingrules{query}"),
        None,
    )
    .await
}

/// Every `RoutingRuleId` a listing of `domain` reports, asserting the 200 first.
async fn listed_ids(emulator: &MultiServiceRouter, case: &str, domain: &str) -> BTreeSet<String> {
    let (status, body) = list_routing_rules(emulator, domain, "?maxResults=100").await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from ListRoutingRules of {domain:?}, got {status} {body}"
    );
    body["routingRules"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|rule| as_str(rule, "routingRuleId"))
        .collect()
}

/// Create one custom domain name, asserting the 201 the model binds to
/// `CreateDomainName`.
async fn seed_domain_name(emulator: &MultiServiceRouter, domain: &str) {
    let (status, body) = call(
        emulator,
        "POST",
        "/v2/domainnames",
        Some(json!({
            "domainName": domain,
            // AWS Certificate Manager is the only source `CertificateArn` documents.
            "domainNameConfigurations": [{
                "certificateArn": engine::emulator_arn(
                    "acm",
                    REGION,
                    &format!("certificate/{domain}"),
                ),
            }],
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "seeding domain name {domain:?}: expected 201 from CreateDomainName, got {status} {body}"
    );
}

/// Create one HTTP api carrying [`STAGE`], and answer its `ApiId`.
async fn seed_api(emulator: &MultiServiceRouter, name: &str) -> String {
    let (status, body) = call(
        emulator,
        "POST",
        "/v2/apis",
        Some(json!({ "name": name, "protocolType": "HTTP" })),
    )
    .await;
    assert_eq!(
        status, 201,
        "seeding api {name:?}: expected 201 from CreateApi, got {status} {body}"
    );
    let api_id = as_str(&body, "apiId");
    let (status, stage) = call(
        emulator,
        "POST",
        &format!("/v2/apis/{api_id}/stages"),
        Some(json!({ "stageName": STAGE })),
    )
    .await;
    assert_eq!(
        status, 201,
        "seeding stage {STAGE:?} of api {name:?}: expected 201, got {status} {stage}"
    );
    api_id
}

/// A fresh emulator holding [`DOMAIN`], [`OTHER_DOMAIN`] and one api with [`STAGE`].
async fn seeded() -> (MultiServiceRouter, String) {
    let (emulator, api_id, _engine) = seeded_with_engine().await;
    (emulator, api_id)
}

/// [`seeded`], and the engine it was built from — for the rows that address a domain name by
/// its own `DomainNameId`, a value this model reports through no operation at all
/// ([`minted_domain_name_id`]).
async fn seeded_with_engine() -> (MultiServiceRouter, String, ::std::sync::Arc<engine::InMemoryEngine>)
{
    let (emulator, engine) = crate::api_lifecycle::emulator_with_engine();
    seed_domain_name(&emulator, DOMAIN).await;
    seed_domain_name(&emulator, OTHER_DOMAIN).await;
    let api_id = seed_api(&emulator, "deleted-rules-api").await;
    (emulator, api_id, engine)
}

/// The simplest rule body: one `invokeApi` action on [`STAGE`], one base-path condition
/// naming `base_path`, and `priority`.
fn simple_rule(api_id: &str, base_path: &str, priority: i64) -> Value {
    json!({
        "priority": priority,
        "actions": [{ "invokeApi": { "apiId": api_id, "stage": STAGE } }],
        "conditions": [{ "matchBasePaths": { "anyOf": [base_path] } }],
    })
}

/// Seed `count` rules under `domain`, each with a priority and a base path of its own,
/// and answer their `RoutingRuleId`s in creation order.
async fn seed_rules(
    emulator: &MultiServiceRouter,
    domain: &str,
    api_id: &str,
    count: usize,
) -> Vec<String> {
    let mut ids = Vec::with_capacity(count);
    for index in 0..count {
        let priority = i64::try_from(index).unwrap_or_default() + 1;
        let request = simple_rule(api_id, &format!("path{index}"), priority);
        let (status, created) = create_routing_rule(emulator, domain, &request).await;
        assert_eq!(
            status, 201,
            "seeding rule {index} under {domain:?}: expected 201 from CreateRoutingRule, \
             got {status} {created}"
        );
        ids.push(as_str(&created, "routingRuleId"));
    }
    ids
}

/// The `DomainNameId` this store minted for `domain`, read out of the store itself: no
/// operation of this model reports the id, and the address a domain name publishes is the one
/// its own socket answers on rather than a name that spells the id
/// (`crate::service::domain_name_socket`).
fn minted_domain_name_id(
    engine: &::std::sync::Arc<engine::InMemoryEngine>,
    domain: &str,
) -> String {
    let state = engine.state::<
        service_com_amazonaws_apigatewayv2_apigatewayv2::ComAmazonawsApigatewayv2Apigatewayv2State,
    >();
    let store = state.lock();
    store
        .domain_names
        .get(domain)
        .map(|record| record.domain_name_id.clone())
        .unwrap_or_default()
}

/// Assert one response is the modelled refusal of a request naming a resource the account
/// does not hold: 404 `NotFoundException`, carrying `message`.
fn assert_not_found(case: &str, status: u16, body: &Value, message: &str) {
    assert_eq!(
        status, 404,
        "case {case:?}: expected 404 for a resource the account does not hold, \
         got {status} {body}"
    );
    assert_eq!(
        error_code(body),
        "NotFoundException",
        "case {case:?}: expected NotFoundException, got {body}"
    );
    assert_eq!(
        as_str(body, "message"),
        message,
        "case {case:?}: expected the message {message:?}, got {body}"
    );
}

/// One row of the deletion table: how many rules the domain name holds and which of them
/// the path addresses.
struct DeletionCase {
    label: &'static str,
    total: usize,
    /// The index, in creation order, of the rule to delete.
    index: usize,
}

/// The rows vary the size of the collection the rule is removed from and the position of
/// the removed rule in it: the only rule, the first of several, one in the middle, and
/// the last.
fn deletion_cases() -> Vec<DeletionCase> {
    vec![
        DeletionCase {
            label: "the only rule the domain name holds",
            total: 1,
            index: 0,
        },
        DeletionCase {
            label: "the first of two",
            total: 2,
            index: 0,
        },
        DeletionCase {
            label: "the last of two",
            total: 2,
            index: 1,
        },
        DeletionCase {
            label: "one in the middle of five",
            total: 5,
            index: 2,
        },
        DeletionCase {
            label: "the first of five",
            total: 5,
            index: 0,
        },
        DeletionCase {
            label: "the last of five",
            total: 5,
            index: 4,
        },
    ]
}

#[tokio::test]
async fn delete_routing_rule_removes_only_the_rule_its_path_addresses() {
    for case in deletion_cases() {
        let label = case.label;
        let (emulator, api_id) = seeded().await;
        let ids = seed_rules(&emulator, DOMAIN, &api_id, case.total).await;
        // A rule of the neighbour domain name, so the delete can be shown not to reach
        // another domain name's collection.
        let neighbour = seed_rules(&emulator, OTHER_DOMAIN, &api_id, 1).await;
        let target = ids[case.index].clone();

        let (status, body) = delete_routing_rule(&emulator, DOMAIN, &target, "").await;
        assert_eq!(
            status, 204,
            "case {label:?}: expected 204 from DeleteRoutingRule, got {status} {body}"
        );
        assert_eq!(
            body,
            Value::Null,
            "case {label:?}: expected DeleteRoutingRule to answer no body, got {body}"
        );

        // The rule the path addressed is gone: its reader refuses it as missing.
        let (status, read) = get_routing_rule(&emulator, DOMAIN, &target, "").await;
        assert_not_found(label, status, &read, ROUTING_RULE_NOT_FOUND);

        // Every other rule of the domain name is still there, reported by its own reader
        // and by the listing.
        let survivors: BTreeSet<String> = ids
            .iter()
            .filter(|id| **id != target)
            .cloned()
            .collect::<BTreeSet<String>>();
        for survivor in &survivors {
            let (status, read) = get_routing_rule(&emulator, DOMAIN, survivor, "").await;
            assert_eq!(
                status, 200,
                "case {label:?}: expected 200 from GetRoutingRule of {survivor:?}, \
                 got {status} {read}"
            );
        }
        assert_eq!(
            listed_ids(&emulator, label, DOMAIN).await,
            survivors,
            "case {label:?}: expected ListRoutingRules to report exactly {survivors:?}"
        );

        // And the neighbour domain name's own collection is untouched.
        assert_eq!(
            listed_ids(&emulator, label, OTHER_DOMAIN).await,
            neighbour.iter().cloned().collect::<BTreeSet<String>>(),
            "case {label:?}: expected {OTHER_DOMAIN:?} to keep {neighbour:?}"
        );
    }
}

#[tokio::test]
async fn delete_routing_rule_is_reported_as_missing_once_the_rule_is_gone() {
    let (emulator, api_id) = seeded().await;
    // Deleting every rule of the domain name in turn, so the repeat is asserted for a
    // collection that empties out as well as for one that still holds rules.
    let ids = seed_rules(&emulator, DOMAIN, &api_id, 3).await;

    for (index, target) in ids.iter().enumerate() {
        let label = format!("rule {index} of {}", ids.len());
        let (status, body) = delete_routing_rule(&emulator, DOMAIN, target, "").await;
        assert_eq!(
            status, 204,
            "case {label:?}: expected 204 from the first DeleteRoutingRule, got {status} {body}"
        );

        // The second delete of the same path has nothing to remove, so it is the modelled
        // refusal of a rule the domain name does not hold.
        let (status, again) = delete_routing_rule(&emulator, DOMAIN, target, "").await;
        assert_not_found(&label, status, &again, ROUTING_RULE_NOT_FOUND);
    }

    let remaining = listed_ids(&emulator, "every rule deleted", DOMAIN).await;
    assert!(
        remaining.is_empty(),
        "expected the domain name to hold no rule once every one was deleted, got {remaining:?}"
    );
}

/// One row of the missing-resource table.
struct MissingCase {
    label: &'static str,
    /// The domain name the request addresses; `None` is [`DOMAIN`], which is seeded.
    domain: Option<&'static str>,
    /// The `RoutingRuleId` the request addresses; `None` is the id of a rule only the
    /// OTHER domain name holds, which no request to [`DOMAIN`] may reach. An id is minted
    /// within the collection that holds the rule, so that is the LAST of the neighbour's
    /// rules, one past however many [`DOMAIN`] holds.
    routing_rule_id: Option<&'static str>,
    /// The query string the request carries, for a row about `DomainNameId`. `None` is
    /// the `DomainNameId` of the OTHER domain name, which this one does not answer to.
    query: Option<&'static str>,
    /// The sentence the refusal must carry.
    message: &'static str,
}

/// The rows vary which resource of the request resolves to nothing — the rule, the domain
/// name, or the domain name under the id the request named — and the form the unusable
/// identifier takes.
fn missing_cases() -> Vec<MissingCase> {
    vec![
        MissingCase {
            label: "an id nothing minted",
            domain: None,
            routing_rule_id: Some("abcdefghij"),
            query: Some(""),
            message: ROUTING_RULE_NOT_FOUND,
        },
        MissingCase {
            label: "an id of a shape this service never mints",
            domain: None,
            routing_rule_id: Some("NOT-A-RULE-ID"),
            query: Some(""),
            message: ROUTING_RULE_NOT_FOUND,
        },
        MissingCase {
            label: "a rule only another domain name holds",
            domain: None,
            routing_rule_id: None,
            query: Some(""),
            message: ROUTING_RULE_NOT_FOUND,
        },
        MissingCase {
            label: "a domain name the account does not hold",
            domain: Some("unseeded.example.com"),
            routing_rule_id: Some("abcdefghij"),
            query: Some(""),
            message: INVALID_DOMAIN_NAME_IDENTIFIER,
        },
        MissingCase {
            label: "a domainNameId no domain name carries",
            domain: None,
            routing_rule_id: None,
            query: Some("?domainNameId=0000000000"),
            message: INVALID_DOMAIN_NAME_IDENTIFIER,
        },
        MissingCase {
            label: "the domainNameId of ANOTHER domain name",
            domain: None,
            routing_rule_id: None,
            query: None,
            message: INVALID_DOMAIN_NAME_IDENTIFIER,
        },
    ]
}

#[tokio::test]
async fn delete_routing_rule_on_a_resource_the_account_does_not_hold_removes_nothing() {
    let (emulator, api_id, engine) = seeded_with_engine().await;
    let own = seed_rules(&emulator, DOMAIN, &api_id, 2).await;
    // The rules that exist under the OTHER domain name. Each collection mints its ids of
    // its own, so only the neighbour's LAST id — one past the count `DOMAIN` holds — is an
    // id no rule of `DOMAIN` carries.
    let elsewhere = seed_rules(&emulator, OTHER_DOMAIN, &api_id, own.len() + 1).await;
    let elsewhere_id = elsewhere
        .last()
        .expect("the neighbour was seeded with rules")
        .clone();
    assert!(
        !own.contains(&elsewhere_id),
        "expected {elsewhere_id:?} to be an id only {OTHER_DOMAIN:?} holds, got {own:?}"
    );
    let foreign_query = format!(
        "?domainNameId={}",
        minted_domain_name_id(&engine, OTHER_DOMAIN)
    );

    for case in missing_cases() {
        let label = case.label;
        let domain = case.domain.unwrap_or(DOMAIN);
        let routing_rule_id = case.routing_rule_id.unwrap_or(&elsewhere_id);
        let query = case.query.unwrap_or(&foreign_query);

        let (status, body) = delete_routing_rule(&emulator, domain, routing_rule_id, query).await;
        assert_not_found(label, status, &body, case.message);

        // The refused delete removed nothing: both collections still hold every rule they
        // were seeded with.
        assert_eq!(
            listed_ids(&emulator, label, DOMAIN).await,
            own.iter().cloned().collect::<BTreeSet<String>>(),
            "case {label:?}: expected {DOMAIN:?} to keep {own:?}"
        );
        assert_eq!(
            listed_ids(&emulator, label, OTHER_DOMAIN).await,
            elsewhere.iter().cloned().collect::<BTreeSet<String>>(),
            "case {label:?}: expected {OTHER_DOMAIN:?} to keep {elsewhere:?}"
        );
    }
}

#[tokio::test]
async fn delete_routing_rule_under_the_domain_names_own_id_removes_the_rule() {
    let (emulator, api_id, engine) = seeded_with_engine().await;
    let ids = seed_rules(&emulator, DOMAIN, &api_id, 2).await;
    let own_id = minted_domain_name_id(&engine, DOMAIN);
    assert!(
        !own_id.is_empty(),
        "expected a minted DomainNameId for {DOMAIN:?}, got {own_id:?}"
    );

    // The positive control for the `DomainNameId` rows above: the domain name's OWN id
    // addresses it, so the delete reaches the rule.
    let label = "the domain name's own DomainNameId";
    let (status, body) = delete_routing_rule(
        &emulator,
        DOMAIN,
        &ids[0],
        &format!("?domainNameId={own_id}"),
    )
    .await;
    assert_eq!(
        status, 204,
        "case {label:?}: expected 204 from DeleteRoutingRule, got {status} {body}"
    );

    let (status, read) = get_routing_rule(&emulator, DOMAIN, &ids[0], "").await;
    assert_not_found(label, status, &read, ROUTING_RULE_NOT_FOUND);
    assert_eq!(
        listed_ids(&emulator, label, DOMAIN).await,
        [ids[1].clone()].into_iter().collect::<BTreeSet<String>>(),
        "case {label:?}: expected only {:?} to remain",
        ids[1]
    );
}
