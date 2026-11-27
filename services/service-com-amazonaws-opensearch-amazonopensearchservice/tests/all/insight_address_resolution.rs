// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the ADDRESS an insight is reached by, across both operations
//! that take one.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees. The harness and the baseline `CreateDomain` body come from
//! [`super::domain_lifecycle`], the detail read and the insight builder from
//! [`super::insight_details`], the submission from [`super::insight_feedback`], the
//! status-and-code pair every missing-resource refusal of this service is answered on
//! from [`super::missing_resource_status`], the refusal's sentence from
//! [`super::control_plane_refusal_wording`] and the listing from [`super::insights`];
//! this suite adds no second copy of any of them.
//!
//! The rule under test: an insight is addressed by the entity that holds it and the
//! insight's own id TOGETHER, and a read refuses what that address resolves NOTHING for. The
//! ENTITY
//! half has to resolve for either operation: an entity this account holds nothing for is
//! refused as ONE missing resource, on the status this service answers that shape on and
//! with a sentence that names neither half of the address, identically from both
//! operations — that part of the rule belongs to the address, not to the operation. The ID
//! half is resolved by what the OPERATION does with it and by what else the address names:
//! `InsightFeedback` rates "an
//! EXISTING insight", so an id the entity holds no insight under leaves nothing to rate
//! and is the same missing resource, and so is an id the ACCOUNT-wide entity selects no
//! insight under — the account is no resource of this service, so that insight is the only
//! resource such an address names — while `DescribeInsightDetails` scoped to a DOMAIN this
//! account holds has resolved what it REPORTS on and answers every id inside the modelled
//! `GUID` form with its
//! `@required` `Fields` — the stored fields where an insight was raised under that id, and
//! the service's own guidance about an insight's details otherwise. Neither operation
//! reports a success for an ENTITY that does not exist, and neither writes anything on a
//! read.
//!
//! So the assertions span a SET of inputs rather than one call: a table of addresses
//! crossing three kinds of entity half (a domain holding the insight, a sibling domain
//! holding another one, and three domains this account holds nothing for, their names at
//! each end of the modelled length) with three kinds of id half (the id the entity holds,
//! ids no entity holds, and the held id of the OTHER domain), each sent to both
//! operations; plus the `Account` entity `DescribeInsightDetails` also admits, named both
//! ways the model allows; plus the positive control, the one address that does resolve.

use std::time::SystemTime;

use serde_json::{json, Value};

use super::control_plane_refusal_wording::message;
use super::domain_lifecycle::{create_baseline, error_code, Answer, Emulator};
use super::insight_details::{
    describe_insight, domain_entity, guid, raise_on_domain, record, ACCOUNT_ENTITY_TYPE,
};
use super::insight_feedback::{feedback, submission};
use super::insights::{insights, list};
use super::missing_resource_status::assert_missing_resource;

/// The domain holding the insight every row is addressed against.
const HELD: &str = "insight-address-held";

/// A sibling domain of the same account, holding an insight of its OWN: an insight is
/// reported by the entity that holds it, so neither domain's id names an insight of the
/// other.
const SIBLING: &str = "insight-address-sibling";

/// Domains this account holds nothing for. The rows differ along the shape of the name:
/// one never created, and one at each end of `DomainName`'s modelled length (3..28).
const ABSENT: [&str; 3] = ["never-created", "abc", "a-domain-name-at-its-upper-b"];

/// The fixed sentence this service refuses a resource it cannot resolve with.
///
/// Restated here rather than imported from the service, so changing it in the service
/// fails this suite instead of being asserted against itself; the sibling suites that
/// assert the same refusal reach it as
/// `super::insight_address_resolution::RESOURCE_NOT_FOUND`, so the crate states the
/// sentence once.
pub(crate) const RESOURCE_NOT_FOUND: &str = "Resource not found";


/// The id of the insight [`HELD`] holds.
fn held_id() -> String {
    guid("00000000ab01")
}

/// The id of the insight [`SIBLING`] holds.
fn sibling_id() -> String {
    guid("00000000ab02")
}

/// Ids no entity holds an insight under. The rows differ along the shape of the id: every
/// group at its lowest digit, every group at its highest, hex digits in both cases (the
/// `GUID` pattern admits either), and the held id with its own digits changed.
fn unheld_ids() -> Vec<String> {
    vec![
        "00000000-0000-0000-0000-000000000000".to_string(),
        "ffffffff-ffff-ffff-ffff-ffffffffffff".to_string(),
        "A1b2C3d4-E5f6-7a8b-9C0d-1e2F3a4B5c6D".to_string(),
        guid("00000000ab99"),
    ]
}

/// An emulator holding the two domains above, each with one insight of its own raised on
/// it.
async fn given_two_domains() -> Emulator {
    let emulator = Emulator::new();
    for (name, insight_id) in [(HELD, held_id()), (SIBLING, sibling_id())] {
        create_baseline(&emulator, name).await;
        raise_on_domain(
            &emulator,
            name,
            record(&insight_id, SystemTime::now(), Vec::new()),
        );
    }
    emulator
}

/// `InsightFeedback` for the address `(entity, insight_id)`, rating it the same way
/// whichever row is sent: what the rating IS is [`super::insight_feedback`]'s subject,
/// and what this suite varies is the address it is sent to.
async fn rate(emulator: &Emulator, entity: Value, insight_id: &str) -> Answer {
    feedback(emulator, submission(entity, insight_id, "Up", None)).await
}

/// Assert `answer` is the ONE missing-resource refusal the rule states, naming neither
/// half of the address it was sent.
///
/// The status and the error code are asserted through the crate's one statement of them
/// ([`super::missing_resource_status::assert_missing_resource`]), so this suite pins
/// neither of its own; what it adds is the sentence and what the sentence does not name.
fn assert_unresolved_address(
    answer: &Answer,
    case: &str,
    entity_value: &str,
    insight_id: &str,
) {
    assert_missing_resource(answer, case);
    let said = message(answer);
    assert_eq!(
        said, RESOURCE_NOT_FOUND,
        "case {case}: expected the fixed sentence, got {said:?}"
    );
    assert!(
        !said.contains(entity_value) && !said.contains(insight_id),
        "case {case}: expected the refusal to name neither half of the address, got {said:?}"
    );
    assert!(
        answer.body["Fields"].is_null() && answer.body["Status"].is_null(),
        "case {case}: a refused call carries no answer of its own, got {:?}",
        answer.body
    );
}

/// Every address this suite is asserted over whose ENTITY half resolves to nothing, as
/// `(entity value, insight id)` pairs of a `DomainName` entity — the entity type both
/// operations take. The id half varies beside it (one held elsewhere, one held nowhere), to
/// show the refusal is the entity's and not the pair's.
fn unresolvable_entities() -> Vec<(String, String)> {
    let mut rows: Vec<(String, String)> = Vec::new();
    for absent in ABSENT {
        rows.push((absent.to_string(), held_id()));
        rows.push((
            absent.to_string(),
            unheld_ids()
                .first()
                .expect("the id table is not empty")
                .clone(),
        ));
    }
    rows
}

/// Every address this suite is asserted over whose entity RESOLVES and whose ID half
/// selects no insight: ids no entity holds, and each domain addressed with the other's
/// held id — an insight is reported by the entity that holds it, so one domain's id names
/// no insight of the other.
fn unselected_ids() -> Vec<(String, String)> {
    let mut rows: Vec<(String, String)> = Vec::new();
    for unheld in unheld_ids() {
        rows.push((HELD.to_string(), unheld.clone()));
        rows.push((SIBLING.to_string(), unheld));
    }
    rows.push((HELD.to_string(), sibling_id()));
    rows.push((SIBLING.to_string(), held_id()));
    rows
}

/// Every address this suite is asserted over that resolves to NO insight at all — both
/// tables above, which is what a call that must write nothing is driven over.
fn unresolvable_addresses() -> Vec<(String, String)> {
    let mut rows = unresolvable_entities();
    rows.extend(unselected_ids());
    rows
}

/// The `Fields` a detail read answers with, asserted non-empty: `Fields` is `@required`, so
/// a detailed view this service answers is never the empty set.
fn detailed_fields(answer: &Answer, case: &str) -> Vec<Value> {
    assert_eq!(
        answer.status, 200,
        "case {case}: expected the detail read to report, got {} with {:?}",
        answer.status, answer.body
    );
    let fields = answer.body["Fields"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("case {case}: expected the required Fields, got {:?}", answer.body)
        })
        .clone();
    assert!(
        !fields.is_empty(),
        "case {case}: expected a non-empty Fields list, got {:?}",
        answer.body
    );
    fields
}

#[tokio::test]
async fn an_entity_that_resolves_to_nothing_is_refused_by_both_operations_alike() {
    let emulator = given_two_domains().await;

    for (entity_value, insight_id) in unresolvable_entities() {
        let entity = domain_entity(&entity_value);

        let detail_case = format!("DescribeInsightDetails({entity_value:?}, {insight_id})");
        let detailed = describe_insight(&emulator, entity.clone(), &insight_id, json!({})).await;
        assert_unresolved_address(&detailed, &detail_case, &entity_value, &insight_id);

        let feedback_case = format!("InsightFeedback({entity_value:?}, {insight_id})");
        let rated = rate(&emulator, entity, &insight_id).await;
        assert_unresolved_address(&rated, &feedback_case, &entity_value, &insight_id);

        // The ENTITY half of the rule belongs to the ADDRESS, so the two operations refuse
        // it identically: same status, same shape, same sentence.
        assert_eq!(
            (detailed.status, error_code(&detailed), detailed.body["message"].clone()),
            (rated.status, error_code(&rated), rated.body["message"].clone()),
            "case {detail_case} against {feedback_case}: expected one refusal for the \
             entity, got {:?} and {:?}",
            detailed.body,
            rated.body
        );
    }
}

#[tokio::test]
async fn an_id_the_entity_holds_no_insight_under_is_detailed_and_refused_by_the_submission() {
    let emulator = given_two_domains().await;

    // The guidance a detail read states where it has no stored field to report is the same
    // sentence whichever id selected nothing, so it is read off the first row and every
    // later row is asserted to match it rather than against a literal of this suite's own
    // (`tests/all/insight_details.rs` pins the sentence itself).
    let mut guidance: Option<Vec<Value>> = None;
    for (entity_value, insight_id) in unselected_ids() {
        let entity = domain_entity(&entity_value);

        let detail_case = format!("DescribeInsightDetails({entity_value:?}, {insight_id})");
        let detailed = describe_insight(&emulator, entity.clone(), &insight_id, json!({})).await;
        let fields = detailed_fields(&detailed, &detail_case);
        match &guidance {
            None => guidance = Some(fields),
            Some(stated) => assert_eq!(
                &fields, stated,
                "case {detail_case}: expected the one guidance every id that selects no \
                 insight is detailed with, got {fields:?}"
            ),
        }

        // The SUBMISSION rates an existing insight, so the id half IS a requirement there:
        // the same address it leaves nothing to rate under is the missing resource.
        let feedback_case = format!("InsightFeedback({entity_value:?}, {insight_id})");
        let rated = rate(&emulator, entity, &insight_id).await;
        assert_unresolved_address(&rated, &feedback_case, &entity_value, &insight_id);
    }
}

#[tokio::test]
async fn an_account_entity_requires_the_id_half_it_is_the_only_resource_of() {
    let emulator = given_two_domains().await;

    // `DescribeInsightDetails` also takes an `Account` entity — "the account's own
    // insights and those of every domain it holds" — named both ways the model allows,
    // since `Value` is optional and the only account served is the one the request was made
    // for. The account itself is no resource of this service, so the insight the id selects
    // is the ONLY resource such an address names: the insights of both domains are reached
    // through it, and an id no insight was raised under resolves nothing at all and is the
    // same missing resource an unresolvable entity is.
    for entity in [
        json!({"Type": ACCOUNT_ENTITY_TYPE}),
        json!({"Type": ACCOUNT_ENTITY_TYPE, "Value": engine::emulator_account_id()}),
    ] {
        for insight_id in [held_id(), sibling_id()] {
            let case = format!("DescribeInsightDetails({entity:?}, {insight_id})");
            let answer = describe_insight(&emulator, entity.clone(), &insight_id, json!({})).await;
            assert_eq!(
                answer.status, 200,
                "case {case}: expected the account entity to reach the insight, got {} with \
                 {:?}",
                answer.status, answer.body
            );
        }

        for unheld in unheld_ids() {
            let case = format!("DescribeInsightDetails({entity:?}, {unheld})");
            let answer = describe_insight(&emulator, entity.clone(), &unheld, json!({})).await;
            assert_missing_resource(&answer, &case);
            assert_eq!(
                message(&answer),
                RESOURCE_NOT_FOUND,
                "case {case}: expected the fixed missing-resource sentence, got {:?}",
                answer.body
            );
            assert!(
                answer.body["Fields"].is_null(),
                "case {case}: a refused read carries no detailed view, got {:?}",
                answer.body
            );
        }
    }
}

#[tokio::test]
async fn the_address_that_does_resolve_is_still_served_and_the_refusals_wrote_nothing() {
    let emulator = given_two_domains().await;

    // The positive control: each domain's own insight is reached by its own address, so
    // the refusals above are the address resolving to nothing rather than the surface
    // being shut.
    for (name, insight_id) in [(HELD, held_id()), (SIBLING, sibling_id())] {
        let case = format!("DescribeInsightDetails({name:?}, {insight_id})");
        let answer = describe_insight(&emulator, domain_entity(name), &insight_id, json!({})).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected the insight the entity holds to be detailed, got {} with \
             {:?}",
            answer.status, answer.body
        );
        assert!(
            answer.body["Fields"]
                .as_array()
                .is_some_and(|fields| !fields.is_empty()),
            "case {case}: expected the detailed view to carry the required Fields, got {:?}",
            answer.body
        );

        let case = format!("InsightFeedback({name:?}, {insight_id})");
        let rated = rate(&emulator, domain_entity(name), &insight_id).await;
        assert_eq!(
            rated.status, 200,
            "case {case}: expected the rating of an insight the domain holds to be recorded, \
             got {} with {:?}",
            rated.status, rated.body
        );
    }

    // And the calls that resolved no insight left the insights each domain holds exactly as
    // they were: neither the refusals nor the informational detail reads raised an insight
    // or took one away.
    for (entity_value, insight_id) in unresolvable_addresses() {
        let entity = domain_entity(&entity_value);
        describe_insight(&emulator, entity.clone(), &insight_id, json!({})).await;
        rate(&emulator, entity, &insight_id).await;
    }
    for (name, insight_id) in [(HELD, held_id()), (SIBLING, sibling_id())] {
        let case = format!("ListInsights({name:?})");
        let answer = list(&emulator, json!({"Entity": domain_entity(name)})).await;
        let listed = insights(&answer, &case);
        let ids: Vec<&str> = listed
            .iter()
            .map(|insight| insight["InsightId"].as_str().unwrap_or_default())
            .collect();
        assert_eq!(
            ids,
            vec![insight_id.as_str()],
            "case {case}: expected the one insight the domain holds, got {ids:?}"
        );
    }
}
