// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for WHICH half of an insight's address refuses a read of it.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees. The harness and the baseline `CreateDomain` body come from
//! [`super::domain_lifecycle`], the detail request, the entity builder, the id builder and
//! the guidance assertion from [`super::insight_details`], the submission from
//! [`super::insight_feedback`], the missing-resource pair from
//! [`super::missing_resource_status`] and the listing from [`super::insights`]; this suite
//! adds no second copy of any of them.
//!
//! The rule under test: an insight is addressed by an ENTITY and an insight id, and a read
//! refuses what that address resolves NOTHING for. The entity half is a requirement of every
//! operation that takes one — one this account holds nothing for is the modelled missing
//! resource. The id half is a requirement of an operation that ACTS on the insight it names
//! (`InsightFeedback` rates "an EXISTING insight", so an id that selects none leaves nothing
//! to rate) and of an address whose only resource it is: the ACCOUNT-wide entity names no
//! resource of this service, so an id selecting no insight of the account is that same
//! missing resource, while `DescribeInsightDetails` scoped to a DOMAIN this account holds has
//! resolved what it reports on and answers every id inside the modelled `GUID` form 200 with
//! its `@required` `Fields` — the insight's stored fields where one was raised under that id,
//! and the single informational field the service states about an insight's details
//! otherwise.
//!
//! So the assertions span a SET of inputs rather than one call, differing along what the
//! rule generalises over: the entity half (a domain holding an insight, a domain holding
//! none, and the account named both ways the optional `Value` allows), the id half (one
//! raised with detail fields, one raised with none, four ids no insight was raised under
//! at each extreme of the `GUID` pattern and in both hex cases, and a raised id with its
//! case changed, which names no insight because an id is the exact value the service
//! handed out), and the rendering the request asks for; plus the contrast rows, where the
//! entity does not resolve, and the same id halves sent to the submission that does
//! require them.
//!
//! The insights themselves are written into the store directly (through
//! [`super::domain_lifecycle::Emulator::store`]): the service RAISES an insight from what
//! it observes of a domain, so no wire call leaves one. Everything the rule is asserted ON
//! is read back through the wire.

use std::time::SystemTime;

use serde_json::{json, Value};
use service_com_amazonaws_opensearch_amazonopensearchservice::state::InsightDetailField;

use super::domain_lifecycle::{create_baseline, Answer, Emulator};
use super::insight_details::{
    assert_unselected_id, describe_insight, domain_entity, guid, guidance, raise_on_domain,
    record, ACCOUNT_ENTITY_TYPE,
};
use super::insight_feedback::{feedback, submission};
use super::insights::{insights, list};
use super::missing_resource_status::assert_missing_resource;

/// The domain holding the insights the rows read.
const HELD: &str = "insight-halves-held";

/// A domain of the same account that holds no insight at all, so a resolvable entity with
/// nothing raised on it is a row of its own.
const BARE: &str = "insight-halves-bare";

/// Domains this account holds nothing for: the contrast rows, with names at each end of
/// `DomainName`'s modelled length (3..28) and one never created.
const ABSENT: [&str; 3] = ["never-raised-here", "abc", "a-domain-name-at-its-upper-b"];

/// The id of the insight [`HELD`] holds WITH detail fields of its own.
fn detailed_id() -> String {
    guid("00000000c001")
}

/// The id of the insight [`HELD`] holds with NO detail field of its own.
fn bare_insight_id() -> String {
    guid("00000000c002")
}

/// The one detail field the detailed insight holds, so its view is told apart from the
/// guidance by what it reports.
fn stored_field() -> InsightDetailField {
    InsightDetailField {
        name: "Summary".to_string(),
        field_type: "text".to_string(),
        value: "Shard count on the hot data nodes is above the recommended ceiling."
            .to_string(),
        html_value: None,
    }
}

/// Ids no insight was raised under: every `GUID` group at its lowest digit, every group at
/// its highest, hex digits in both cases (the pattern admits either), one of the ordinary
/// form this crate builds, and the detailed id with its case changed.
fn unraised_ids() -> Vec<String> {
    vec![
        "00000000-0000-0000-0000-000000000000".to_string(),
        "ffffffff-ffff-ffff-ffff-ffffffffffff".to_string(),
        "A1b2C3d4-E5f6-7a8b-9C0d-1e2F3a4B5c6D".to_string(),
        guid("00000000c0ff"),
        detailed_id().to_uppercase(),
    ]
}

/// The three ways a request can answer `ShowHtmlContent`, which picks the rendering a
/// field's content is reported in and nothing about whether a read is answered.
fn renderings() -> Vec<Value> {
    vec![
        json!({}),
        json!({"ShowHtmlContent": false}),
        json!({"ShowHtmlContent": true}),
    ]
}

/// An emulator holding [`HELD`] with two insights raised on it — one carrying a detail
/// field, one carrying none — and [`BARE`] holding none.
async fn given_domains() -> Emulator {
    let emulator = Emulator::new();
    create_baseline(&emulator, HELD).await;
    create_baseline(&emulator, BARE).await;
    raise_on_domain(
        &emulator,
        HELD,
        record(&detailed_id(), SystemTime::now(), vec![stored_field()]),
    );
    raise_on_domain(
        &emulator,
        HELD,
        record(&bare_insight_id(), SystemTime::now(), Vec::new()),
    );
    emulator
}

/// Every entity this service resolves, as `(case, entity)`: both domains and the account
/// named both ways its optional `Value` allows.
fn resolvable_entities() -> Vec<(String, Value)> {
    vec![
        (format!("DomainName {HELD}"), domain_entity(HELD)),
        (format!("DomainName {BARE}"), domain_entity(BARE)),
        (
            "Account, unnamed".to_string(),
            json!({"Type": ACCOUNT_ENTITY_TYPE}),
        ),
        (
            "Account, named".to_string(),
            json!({"Type": ACCOUNT_ENTITY_TYPE, "Value": engine::emulator_account_id()}),
        ),
    ]
}

/// `InsightFeedback` for the address `(entity, insight_id)` — the operation that ACTS on
/// the insight the id names, so the id half is its requirement.
async fn rate(emulator: &Emulator, entity: Value, insight_id: &str) -> Answer {
    feedback(emulator, submission(entity, insight_id, "Up", None)).await
}

#[tokio::test]
async fn an_id_that_selects_no_insight_is_answered_by_what_its_address_resolves() {
    let emulator = given_domains().await;

    for (entity_case, entity) in resolvable_entities() {
        for unraised in unraised_ids() {
            for rendering in renderings() {
                let case = format!(
                    "DescribeInsightDetails({entity_case}, {unraised}, {rendering:?})"
                );
                let answer =
                    describe_insight(&emulator, entity.clone(), &unraised, rendering).await;
                // The informational view where the address resolved a domain — the ONE
                // field the service states about an insight's details, whose wording
                // `tests/all/insight_details.rs` pins — and the missing resource where the
                // insight its id selects was the only resource the address named.
                assert_unselected_id(&answer, &case, &entity);
            }
        }
    }
}

#[tokio::test]
async fn an_insight_that_was_raised_is_still_detailed_by_its_own_fields() {
    let emulator = given_domains().await;

    // The other side of the rule: the id half SELECTS, so an id an insight WAS raised
    // under reports that insight's own stored fields rather than the guidance — and one
    // raised with no field of its own reports the guidance, exactly as an id that selects
    // nothing does.
    for (entity_case, entity) in resolvable_entities() {
        let case = format!("DescribeInsightDetails({entity_case}, {})", detailed_id());
        let answer = describe_insight(&emulator, entity.clone(), &detailed_id(), json!({})).await;
        // Only the entities that HOLD the insight reach it: an insight is reported by the
        // entity that holds it, and the account holds every domain's.
        let reaches_held = entity["Type"] == "Account" || entity["Value"] == HELD;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected the detail read to report, got {} with {:?}",
            answer.status, answer.body
        );
        let reported: Vec<&str> = answer.body["Fields"]
            .as_array()
            .map(|fields| {
                fields
                    .iter()
                    .map(|field| field["Name"].as_str().unwrap_or_default())
                    .collect()
            })
            .unwrap_or_default();
        let held_field = stored_field().name;
        let expected = if reaches_held {
            vec![held_field.as_str()]
        } else {
            vec!["Message"]
        };
        assert_eq!(
            reported, expected,
            "case {case}: expected {expected:?}, got {reported:?}"
        );

        let bare_case = format!(
            "DescribeInsightDetails({entity_case}, {})",
            bare_insight_id()
        );
        let bare = describe_insight(&emulator, entity, &bare_insight_id(), json!({})).await;
        guidance(&bare, &bare_case);
    }
}

#[tokio::test]
async fn an_entity_this_account_holds_nothing_for_is_still_refused() {
    let emulator = given_domains().await;

    // The entity half IS a requirement: a `DomainName` this account holds no domain under
    // is the modelled missing resource, whichever id the read names — including one that
    // does select an insight elsewhere.
    for absent in ABSENT {
        for insight_id in [detailed_id(), unraised_ids()[0].clone()] {
            let case = format!("DescribeInsightDetails(DomainName {absent:?}, {insight_id})");
            let answer =
                describe_insight(&emulator, domain_entity(absent), &insight_id, json!({})).await;
            assert_missing_resource(&answer, &case);
            assert!(
                answer.body["Fields"].is_null(),
                "case {case}: a refused read carries no detailed view, got {:?}",
                answer.body
            );
        }
    }
}

#[tokio::test]
async fn the_submission_that_rates_an_insight_still_requires_the_id_half() {
    let emulator = given_domains().await;

    // The contrast that keeps the rule to the operation that REPORTS: the submission rates
    // an existing insight, so an id that selects none is refused there even though the
    // detail read answers it.
    for unraised in unraised_ids() {
        let case = format!("InsightFeedback(DomainName {HELD}, {unraised})");
        let rated = rate(&emulator, domain_entity(HELD), &unraised).await;
        assert_missing_resource(&rated, &case);
    }

    // And the id that does select an insight is rated.
    let case = format!("InsightFeedback(DomainName {HELD}, {})", detailed_id());
    let rated = rate(&emulator, domain_entity(HELD), &detailed_id()).await;
    assert_eq!(
        rated.status, 200,
        "case {case}: expected the rating of an insight the domain holds to be recorded, got \
         {} with {:?}",
        rated.status, rated.body
    );
}

#[tokio::test]
async fn the_reads_raised_no_insight_of_their_own() {
    let emulator = given_domains().await;

    // A detail read REPORTS: an id it answered the guidance for is not an insight it
    // raised, so the entity's listing is exactly the set that was raised on it.
    for (entity_case, entity) in resolvable_entities() {
        for unraised in unraised_ids() {
            describe_insight(&emulator, entity.clone(), &unraised, json!({})).await;
        }
        let case = format!("ListInsights({entity_case})");
        let answer = list(&emulator, json!({"Entity": entity})).await;
        let listed = insights(&answer, &case);
        let mut ids: Vec<&str> = listed
            .iter()
            .map(|insight| insight["InsightId"].as_str().unwrap_or_default())
            .collect();
        ids.sort_unstable();
        let mut expected = if entity_case == format!("DomainName {BARE}") {
            Vec::new()
        } else {
            vec![detailed_id(), bare_insight_id()]
        };
        expected.sort();
        assert_eq!(
            ids,
            expected.iter().map(String::as_str).collect::<Vec<&str>>(),
            "case {case}: expected the insights that were raised, got {ids:?}"
        );
    }
}
