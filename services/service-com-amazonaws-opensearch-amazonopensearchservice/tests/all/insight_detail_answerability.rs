// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for a detail read taken against a store that holds NO insight.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees. The harness and the baseline `CreateDomain` body come from
//! [`super::domain_lifecycle`], the detail request from [`super::insight_details`], the
//! listing from [`super::insights`] and the constraint report from
//! [`super::constraint_refusal_messages`]; this suite adds no second copy of any of them.
//!
//! The rule under test: a detail read reports on an insight of the address it names, and
//! refuses what that address resolves NOTHING for — against a store that holds no insight at
//! all, EVERY detail read scoped to a DOMAIN this account holds is answered 200 with the one
//! field the service states about an insight's details, whichever well-formed id it names,
//! while a read of the ACCOUNT-wide entity resolves nothing until an insight does (the
//! account itself is no resource of this service) and is the missing resource. Nothing else
//! is changed by that: an entity whose type is outside `InsightEntityType`, an `Account`
//! entity naming another account and a `DomainName` this account holds no domain under are
//! refused as the malformed requests and missing resources they are, and a read writes
//! nothing — it does not raise the insight it found no stored field for.
//!
//! No insight is seeded anywhere, which is what tells this suite apart from
//! [`super::insight_details`]: an insight is RAISED by the service observing a domain, so
//! a client meets a store that holds none, and that is the state every detail read of a
//! real client has to be answered in.
//!
//! So the assertions span a SET of inputs rather than one call: four entities — the account
//! named both ways the model admits, a domain polled to `Created && !Processing` and one
//! still provisioning — against four well-formed ids, in each of the three ways a request
//! can answer `ShowHtmlContent`; then five entity types outside the modelled set across
//! both operations that take an `InsightEntity`, and three entities this account holds
//! nothing for.

use serde_json::{json, Value};
use service_com_amazonaws_opensearch_amazonopensearchservice::ops::types::INSIGHT_ENTITY_TYPE_VALUES;

use super::constraint_refusal_messages::{clause, message, report, value_set};
use super::domain_lifecycle::{create_baseline, settled_domain, Answer, Emulator};
use super::insight_address_resolution::RESOURCE_NOT_FOUND;
use super::insight_details::{
    assert_unselected_id, describe_insight, domain_entity, guid, ACCOUNT_ENTITY_TYPE,
};
use super::insights::{list, refused};

/// The domain a case reads against once the clock owes it its terminal status — the
/// precondition a client polls for before it reads insight detail.
const READY_DOMAIN: &str = "insight-answerable-ready";

/// A domain still inside its provisioning span, so readiness is not what decides how a
/// detail read against it is answered.
const PROVISIONING_DOMAIN: &str = "insight-answerable-new";

/// The ids every case below reads.
///
/// All four are well-formed `GUID`s — `@length(36, 36)` with pattern
/// `^\p{XDigit}{8}-\p{XDigit}{4}-\p{XDigit}{4}-\p{XDigit}{4}-\p{XDigit}{12}$` — and no
/// insight was ever raised under any of them. The rows differ along the shape of the id:
/// every group at its lowest digit, every group at its highest, hex digits in both cases
/// (the pattern admits either), and one of the ordinary form this crate builds ids in.
fn unheld_ids() -> Vec<String> {
    vec![
        "00000000-0000-0000-0000-000000000000".to_string(),
        "ffffffff-ffff-ffff-ffff-ffffffffffff".to_string(),
        "A1b2C3d4-E5f6-7a8b-9C0d-1e2F3a4B5c6D".to_string(),
        guid("0000000000ff"),
    ]
}

/// Whether a `DescribeDomain` answer reports the `Created && !Processing` pair a readiness
/// poll waits for.
fn is_ready(answer: &Answer) -> bool {
    let status = &answer.body["DomainStatus"];
    status["Created"].as_bool() == Some(true) && status["Processing"].as_bool() == Some(false)
}

/// A refused detail read, asserted to be the missing-resource refusal the rule states:
/// the modelled shape on the status this service answers it on
/// ([`super::missing_resource_status::assert_missing_resource`], the crate's one statement
/// of that pair), carrying the fixed sentence and no detailed view.
fn assert_missing_resource(answer: &Answer, case: &str) {
    super::missing_resource_status::assert_missing_resource(answer, case);
    let said = answer.body["message"].as_str().unwrap_or_default();
    assert_eq!(
        said, RESOURCE_NOT_FOUND,
        "case {case}: expected the fixed missing-resource sentence, got {said:?}"
    );
    assert!(
        answer.body["Fields"].is_null(),
        "case {case}: a refused read carries no detailed view, got {:?}",
        answer.body
    );
}

/// Whether the store holds no insight at all: neither for the account nor on any domain.
///
/// Read the way a handler reaches the store, so a read that raised an insight to answer
/// with would be visible here even if the wire answer looked right.
fn holds_no_insight(emulator: &Emulator) -> bool {
    let store = emulator.store();
    let guard = store.lock();
    guard.account_insights.is_empty()
        && guard
            .domains
            .values()
            .all(|domain| domain.insights.is_empty())
}

/// The `Insights` of a listing for `entity`, asserted to be the empty set this emulator
/// holds for every entity.
async fn empty_listing(emulator: &Emulator, entity: &Value, case: &str) {
    let answer = list(emulator, json!({"Entity": entity})).await;
    assert_eq!(
        answer.status, 200,
        "case {case}: expected 200 from ListInsights, got {} with {:?}",
        answer.status, answer.body
    );
    let listed = answer.body["Insights"]
        .as_array()
        .map(Vec::len)
        .unwrap_or_default();
    assert_eq!(
        listed, 0,
        "case {case}: expected an empty listing, got {:?}",
        answer.body
    );
}

#[tokio::test]
async fn a_detail_read_against_a_store_holding_no_insight_is_answered_by_what_it_resolves() {
    let emulator = Emulator::new();
    create_baseline(&emulator, READY_DOMAIN).await;
    // The precondition a client polls for: the domain has reached the status the clock
    // owes it. Polled rather than waited out, so the case does not depend on how fast the
    // host runs.
    let settled = settled_domain(&emulator, READY_DOMAIN).await;
    assert_eq!(
        settled["Created"].as_bool(),
        Some(true),
        "case {READY_DOMAIN}: expected the settled domain to report itself created, got \
         {settled:?}"
    );
    // And one domain inside its provisioning span, so what decides the answer is whether
    // the address resolves an insight and not whether the domain has settled.
    create_baseline(&emulator, PROVISIONING_DOMAIN).await;
    let provisioning = emulator.describe(PROVISIONING_DOMAIN).await;
    assert!(
        !is_ready(&provisioning),
        "case {PROVISIONING_DOMAIN}: expected the transient state inside the span, got {:?}",
        provisioning.body["DomainStatus"]
    );

    // Every entity this service resolves: the account named both ways a caller can name it
    // — `Account`'s `Value` is optional, and the only account served is the one the request
    // was made for — and a `DomainName` entity for each of the two domains.
    let entities = [
        json!({"Type": ACCOUNT_ENTITY_TYPE}),
        json!({"Type": ACCOUNT_ENTITY_TYPE, "Value": engine::emulator_account_id()}),
        domain_entity(READY_DOMAIN),
        domain_entity(PROVISIONING_DOMAIN),
    ];

    for entity in &entities {
        assert!(
            INSIGHT_ENTITY_TYPE_VALUES.contains(&entity["Type"].as_str().unwrap_or_default()),
            "case {entity:?}: the row must name a modelled entity type"
        );
        // Nothing was ever raised for this entity, which is what every row below reads
        // against — and the listing says so on the wire, not just in the store.
        empty_listing(&emulator, entity, &format!("ListInsights({entity:?})")).await;

        for unheld in unheld_ids() {
            // The three ways a request can answer `ShowHtmlContent`: the guidance is
            // content the service states rather than content it marks up, so a read is
            // answered with the same field whichever rendering it asked for.
            for extra in [
                json!({}),
                json!({"ShowHtmlContent": false}),
                json!({"ShowHtmlContent": true}),
            ] {
                let case = format!("DescribeInsightDetails({entity:?}, {unheld:?}, {extra:?})");
                let answer =
                    describe_insight(&emulator, entity.clone(), &unheld, extra.clone()).await;
                assert_unselected_id(&answer, &case, entity);

                // A report: the same request is answered the same way again.
                let again = describe_insight(&emulator, entity.clone(), &unheld, extra).await;
                assert_eq!(
                    again.body, answer.body,
                    "case {case}(twice): expected the same view, got {:?}",
                    again.body
                );
            }
        }

        // The reads raised no insight for the entity, so its listing is what it was.
        empty_listing(
            &emulator,
            entity,
            &format!("ListInsights({entity:?}) after the reads"),
        )
        .await;
    }

    // And nothing was written: a detail read reports, it does not raise the insight it
    // found no stored field for.
    assert!(
        holds_no_insight(&emulator),
        "case the whole table: expected a detail read to store no insight"
    );
}

#[tokio::test]
async fn an_entity_type_outside_the_modelled_set_is_refused_with_its_value_set() {
    let emulator = Emulator::new();
    create_baseline(&emulator, READY_DOMAIN).await;
    let unheld = unheld_ids();
    let insight_id = unheld
        .first()
        .expect("the id table is not empty")
        .to_owned();

    // Rows differ along the shape of the type: a value of the right kind in the wrong
    // case, the singular of a modelled value, a value of the OTHER entity enum of this
    // service, a type of no entity at all, and no type at all.
    for not_a_type in ["account", "DOMAINNAME", "Domain", "Cluster", ""] {
        assert!(
            !INSIGHT_ENTITY_TYPE_VALUES.contains(&not_a_type),
            "case {not_a_type:?}: the row must name a type outside the modelled set"
        );
        let entity = json!({"Type": not_a_type, "Value": READY_DOMAIN});
        // The clause is the member's own: the value echoed at the path the member is
        // carried under, and the value set the model declares for it.
        let expected = report(&[clause(
            not_a_type,
            "entity.type",
            &value_set(INSIGHT_ENTITY_TYPE_VALUES),
        )]);

        // Both operations that take an `InsightEntity` resolve the entity the same way, so
        // both state the same clause for a type outside the set.
        for (case, answer) in [
            (
                format!("DescribeInsightDetails({entity:?})"),
                describe_insight(&emulator, entity.clone(), &insight_id, json!({})).await,
            ),
            (
                format!("ListInsights({entity:?})"),
                list(&emulator, json!({"Entity": entity.clone()})).await,
            ),
        ] {
            refused(&answer, &case, 400, "ValidationException");
            let stated = message(&answer, &case);
            assert_eq!(
                stated, expected,
                "case {case}: expected {expected:?}, got {stated:?}"
            );
        }
    }
}

#[tokio::test]
async fn an_entity_this_account_holds_nothing_for_is_refused_the_same_way() {
    let emulator = Emulator::new();
    create_baseline(&emulator, READY_DOMAIN).await;
    let unheld = unheld_ids();
    let insight_id = unheld
        .first()
        .expect("the id table is not empty")
        .to_owned();

    // A `DomainName` this account holds no domain under names no entity to report on, so
    // the read is refused as one missing resource — the ENTITY half of the address is the
    // one that refuses, where an id the entity holds no insight under is answered. Rows
    // differ along the shape of the name: one never created, and one at each end of the
    // modelled length.
    for name in ["never-created", "abc", "a-domain-name-at-its-upper-b"] {
        let answer =
            describe_insight(&emulator, domain_entity(name), &insight_id, json!({})).await;
        assert_missing_resource(
            &answer,
            &format!("DescribeInsightDetails(DomainName {name:?})"),
        );
    }

    // An `Account` entity's `Value` is "the Amazon Web Services account ID", and the only
    // account this emulator serves is the one the request was made for.
    for foreign in ["123456789012", "210987654321"] {
        assert_ne!(
            foreign,
            engine::emulator_account_id(),
            "case DescribeInsightDetails(Account {foreign:?}): the row must name another account"
        );
        let entity = json!({"Type": ACCOUNT_ENTITY_TYPE, "Value": foreign});
        let answer = describe_insight(&emulator, entity, &insight_id, json!({})).await;
        refused(
            &answer,
            &format!("DescribeInsightDetails(Account {foreign:?})"),
            400,
            "ValidationException",
        );
    }

    // A refused read raised nothing either.
    assert!(
        holds_no_insight(&emulator),
        "case the refused reads: expected a refused read to store no insight"
    );
}
