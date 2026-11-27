// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DescribeInsightDetails`.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a
//! client sees. The harness and the baseline `CreateDomain` body come from
//! [`super::domain_lifecycle`], and the refusal assertion from [`super::insights`];
//! this suite adds no second copy of either.
//!
//! The rule under test: a detail read reports the detail fields persisted on the ONE
//! insight the entity the request names holds under the requested id — each field as
//! its stored name, type and value, in the order the insight holds them — rendering
//! each value with the markup the field carries when the request asks for HTML content
//! and as plain content otherwise; the insight is addressed by the ENTITY and the id
//! together, and the read refuses what that address resolves NOTHING for — an entity this
//! account does not hold is the ONE missing resource naming neither half of the address,
//! and so is an id the ACCOUNT-wide entity selects no insight under, that account being no
//! resource of this service; while an id a resolvable DOMAIN holds no insight under is this
//! read's own 200, carrying the one field the service states about an insight's details,
//! exactly as an insight the service has raised no detail field for is detailed; and
//! nothing is written.
//!
//! So the assertions span a SET of inputs rather than one call: a table of fields
//! covering both values of `InsightFieldType`, a field with a marked-up rendering and
//! fields without one, an empty value and a long one; then the three ways a request can
//! answer `ShowHtmlContent`, every entity type the model admits with the account named
//! both ways, five ids no entity holds an insight under against four entities that do
//! resolve, five entities this account holds nothing for, and an insight carrying no
//! detail field at all.
//!
//! The insights themselves are written into the store directly (through
//! [`super::domain_lifecycle::Emulator::store`]): the service RAISES an insight and its
//! fields from what it observes of a domain, so no wire call leaves one. Everything the
//! rule is asserted ON is read back through the wire.

use std::time::SystemTime;

use serde_json::{json, Value};
use service_com_amazonaws_opensearch_amazonopensearchservice::ops::types::{
    INSIGHT_ENTITY_TYPE_VALUES, INSIGHT_FIELD_TYPE_VALUES,
};
use service_com_amazonaws_opensearch_amazonopensearchservice::state::{
    InsightDetailField, InsightRecord,
};

use super::domain_lifecycle::{
    create_baseline, Answer, Emulator, PREFIX, RECORDED_NOT_FOUND_STATUS,
};
use super::insight_address_resolution::RESOURCE_NOT_FOUND;
use super::insights::refused;

/// One row of the detail-field table: the field's name, its type, its plain value and
/// the marked-up rendering of that same content where the field has one.
struct Row {
    name: &'static str,
    field_type: &'static str,
    value: &'static str,
    html_value: Option<&'static str>,
}

/// A long text value, built rather than written out, so a row exercises content well
/// past the length of a one-line description.
fn long_text() -> String {
    "Nodes in this domain have been running hot for several hours. ".repeat(40)
}

/// The detail fields every case below is asserted over.
///
/// The rows differ along what the rule generalises over: what the field carries (a text
/// description or metric data — between them every value of `InsightFieldType`, the
/// catalog the model declares, as `ops::types`' `INSIGHT_FIELD_TYPE_VALUES`, asserted
/// below), whether the field has a marked-up rendering of its content as well as a
/// plain one, and how long that content is — nothing at all, one line, a numeric
/// datapoint set, and many lines.
fn rows() -> Vec<Row> {
    vec![
        Row {
            name: "Summary",
            field_type: "text",
            value: "Shard count on the hot data nodes is above the recommended ceiling.",
            html_value: Some(
                "<p>Shard count on the hot data nodes is above the \
                 <b>recommended ceiling</b>.</p>",
            ),
        },
        Row {
            name: "Recommendation",
            field_type: "text",
            value: "Reduce the number of shards per data node, or add data nodes.",
            html_value: None,
        },
        Row {
            name: "JVMMemoryPressure",
            field_type: "metric",
            value: "{\"Unit\":\"Percent\",\"Datapoints\":[[0,72.5],[300,81.25]]}",
            html_value: None,
        },
        Row {
            name: "SearchableDocuments",
            field_type: "metric",
            value: "0",
            html_value: None,
        },
        Row {
            name: "Notes",
            field_type: "text",
            value: "",
            html_value: None,
        },
    ]
}

/// [`rows`] as the fields the store holds on an insight, in the order a detail read
/// must report them, with the long row appended so one field's content is far longer
/// than a line.
fn stored_fields() -> Vec<InsightDetailField> {
    let mut fields: Vec<InsightDetailField> = rows()
        .iter()
        .map(|row| InsightDetailField {
            name: row.name.to_string(),
            field_type: row.field_type.to_string(),
            value: row.value.to_string(),
            html_value: row.html_value.map(str::to_string),
        })
        .collect();
    fields.push(InsightDetailField {
        name: "Observation".to_string(),
        field_type: "text".to_string(),
        value: long_text(),
        html_value: Some(format!("<p>{}</p>", long_text())),
    });
    fields
}

/// The names of [`stored_fields`], in the order the insight holds them.
fn field_names() -> Vec<String> {
    stored_fields()
        .iter()
        .map(|field| field.name.clone())
        .collect()
}

/// The `GUID` an insight of this suite is addressed by: `InsightId` is
/// `@length(36, 36)` with pattern
/// `^\p{XDigit}{8}-\p{XDigit}{4}-\p{XDigit}{4}-\p{XDigit}{4}-\p{XDigit}{12}$`, so an
/// id is a hyphenated UUID whose last field is what tells it apart.
///
/// Reached by a sibling suite as `super::insight_details::guid`, so the crate holds ONE
/// builder of a well-formed insight id.
pub(crate) fn guid(suffix: &str) -> String {
    format!("4d2e7a91-6b3c-4f5d-9e8a-{suffix}")
}

/// The id of the insight the detailed view is read back from.
fn detailed_id() -> String {
    guid("000000000001")
}

/// An insight raised at `raised`, carrying `details`. Shared with
/// [`super::insight_feedback`], which rates the insights this builder raises.
pub(crate) fn record(
    insight_id: &str,
    raised: SystemTime,
    details: Vec<InsightDetailField>,
) -> InsightRecord {
    InsightRecord {
        insight_id: insight_id.to_string(),
        display_name: "Shard count is above the recommended ceiling".to_string(),
        insight_type: "RECOMMENDATION".to_string(),
        priority: "HIGH".to_string(),
        status: "ACTIVE".to_string(),
        created_at: raised,
        updated_at: raised,
        is_experimental: false,
        details,
        feedback: None,
    }
}

/// Raise `record` on the domain `name`. Shared with [`super::insight_feedback`].
pub(crate) fn raise_on_domain(emulator: &Emulator, name: &str, record: InsightRecord) {
    let store = emulator.store();
    let mut guard = store.lock();
    let domain = guard
        .domains
        .get_mut(name)
        .unwrap_or_else(|| panic!("case raise_on_domain({name:?}): the domain must exist"));
    domain.insights.insert(record.insight_id.clone(), record);
}

/// A domain holding the whole [`stored_fields`] table on one insight.
async fn seeded(emulator: &Emulator, name: &str) -> String {
    create_baseline(emulator, name).await;
    let id = detailed_id();
    raise_on_domain(
        emulator,
        name,
        record(&id, SystemTime::now(), stored_fields()),
    );
    id
}

/// An `InsightEntity` naming the domain `name`. Reached by a sibling suite as
/// `super::insight_details::domain_entity`, so the crate holds ONE builder of it.
pub(crate) fn domain_entity(name: &str) -> Value {
    json!({"Type": "DomainName", "Value": name})
}

/// The `InsightEntityType` value that names the ACCOUNT-wide set of insights rather than
/// one domain's, written out as the wire value a request carries. One of
/// `ops::types`' `INSIGHT_ENTITY_TYPE_VALUES`, which the rows that use it assert.
pub(crate) const ACCOUNT_ENTITY_TYPE: &str = "Account";

/// Whether `entity` names that account-wide set.
pub(crate) fn account_wide(entity: &Value) -> bool {
    entity["Type"].as_str() == Some(ACCOUNT_ENTITY_TYPE)
}

/// What a detail read whose id selects NO insight of `entity` must answer — the crate's ONE
/// statement of that rule, so no suite pins its own:
///
/// * an entity naming a DOMAIN this account holds has resolved the resource the read reports
///   on, so the read is answered 200 with the one field the service states about an
///   insight's details ([`guidance`]);
/// * the ACCOUNT-wide entity names no resource of this service at all — every request is
///   made for that account — so the insight its id selects is the only resource such an
///   address names, and an id selecting none is the missing resource, answered the one way
///   this service answers those ([`super::missing_resource_status::assert_missing_resource`])
///   and carrying no detailed view.
pub(crate) fn assert_unselected_id(answer: &Answer, case: &str, entity: &Value) {
    if account_wide(entity) {
        super::missing_resource_status::assert_missing_resource(answer, case);
        assert_eq!(
            message_of(answer),
            RESOURCE_NOT_FOUND,
            "case {case}: expected the fixed missing-resource sentence, got {:?}",
            answer.body
        );
        assert!(
            answer.body["Fields"].is_null(),
            "case {case}: a refused read carries no detailed view, got {:?}",
            answer.body
        );
        return;
    }
    guidance(answer, case);
}

/// The `message` an answer states, or the empty string where it states none.
fn message_of(answer: &Answer) -> &str {
    answer.body["message"].as_str().unwrap_or_default()
}

/// `POST /2021-01-01/opensearch/insight-details` with `body`.
async fn describe(emulator: &Emulator, body: Value) -> Answer {
    emulator
        .call(
            "POST",
            &format!("{PREFIX}/opensearch/insight-details"),
            Some(body),
        )
        .await
}

/// The detailed view of `insight_id` as the entity `entity` holds it, with whatever
/// extra members `extra` carries merged in. Shared with the suite that asserts how an
/// address this service cannot resolve is refused.
pub(crate) async fn describe_insight(
    emulator: &Emulator,
    entity: Value,
    insight_id: &str,
    extra: Value,
) -> Answer {
    let mut body = json!({"Entity": entity, "InsightId": insight_id});
    if let Some(members) = extra.as_object() {
        for (member, value) in members {
            body[member] = value.clone();
        }
    }
    describe(emulator, body).await
}

/// The `Fields` of an accepted detail read.
fn fields(answer: &Answer, case: &str) -> Vec<Value> {
    assert_eq!(
        answer.status, 200,
        "case {case}: expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body["Fields"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: expected a Fields list, got {:?}", answer.body))
        .clone()
}

/// The `Name` of every field of a detailed view, in the order it reports them.
fn names(reported: &[Value]) -> Vec<String> {
    reported
        .iter()
        .map(|field| field["Name"].as_str().unwrap_or_default().to_owned())
        .collect()
}

/// The `Value` the detailed view reports for the field named `name`.
fn reported_value(reported: &[Value], name: &str, case: &str) -> String {
    reported
        .iter()
        .find(|field| field["Name"].as_str() == Some(name))
        .map(|field| {
            field["Value"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case}: expected a string Value, got {field:?}"))
                .to_owned()
        })
        .unwrap_or_else(|| panic!("case {case}: expected the field {name:?} to be reported"))
}

/// The guidance view, asserted as the rule states it: the ONE field the service states
/// about an insight's details, under the name and the modelled type it states it with
/// and carrying the service's own sentence — whichever insight a read details, and in
/// either rendering, because the guidance is content the service states rather than
/// content it marks up.
pub(crate) fn guidance(answer: &Answer, case: &str) -> Vec<Value> {
    /// The sentence the service states about an insight's details, as the wire answer
    /// recorded for a detail read words it.
    const GUIDANCE: &str = "Please use the Amazon OpenSearch Console to view the Insight \
                            Details for a better user experience.";

    let reported = fields(answer, case);
    assert_eq!(
        reported.len(),
        1,
        "case {case}: expected the one guidance field, got {:?}",
        answer.body
    );
    let field = &reported[0];
    assert_eq!(
        field["Name"].as_str(),
        Some("Message"),
        "case {case}: expected the guidance field's name, got {field:?}"
    );
    let field_type = field["Type"].as_str().unwrap_or_default();
    // The type catalog is the model's own enum, as generated into `ops::types`.
    assert!(
        INSIGHT_FIELD_TYPE_VALUES.contains(&field_type),
        "case {case}: expected a modelled field type, got {field_type:?}"
    );
    assert_eq!(
        field_type, "text",
        "case {case}: expected the guidance to be text, got {field:?}"
    );
    assert_eq!(
        field["Value"].as_str(),
        Some(GUIDANCE),
        "case {case}: expected {GUIDANCE:?}, got {field:?}"
    );
    // It is the guidance, not some other insight's view: no stored field is reported.
    for held in field_names() {
        assert!(
            !names(&reported).contains(&held),
            "case {case}: expected no stored field of another insight, got {:?}",
            names(&reported)
        );
    }
    reported
}

/// `POST /2021-01-01/opensearch/insights` for `entity` — the listing the detail read
/// must leave exactly as it was.
async fn list(emulator: &Emulator, entity: Value) -> Answer {
    emulator
        .call(
            "POST",
            &format!("{PREFIX}/opensearch/insights"),
            Some(json!({"Entity": entity})),
        )
        .await
}

#[tokio::test]
async fn a_detail_read_reports_every_stored_field_of_the_insight_in_order() {
    let emulator = Emulator::new();
    let id = seeded(&emulator, "insight-detail-read").await;
    let entity = domain_entity("insight-detail-read");

    let answer = describe_insight(&emulator, entity.clone(), &id, json!({})).await;
    let reported = fields(&answer, "whole detailed view");
    assert_eq!(
        names(&reported),
        field_names(),
        "case whole detailed view: expected the fields in the order the insight holds them, got {:?}",
        names(&reported)
    );

    for stored in stored_fields() {
        let case = format!("InsightField({:?})", stored.name);
        // The type catalog these rows draw on is the model's own enum, as generated
        // into `ops::types`: a row naming a value outside it would assert nothing about
        // this service's wire surface.
        assert!(
            INSIGHT_FIELD_TYPE_VALUES.contains(&stored.field_type.as_str()),
            "case {case}: the row must use a modelled field type, got {:?}",
            stored.field_type
        );

        let field = reported
            .iter()
            .find(|field| field["Name"].as_str() == Some(stored.name.as_str()))
            .unwrap_or_else(|| panic!("case {case}: expected the field to be reported"));
        assert_eq!(
            field["Type"].as_str(),
            Some(stored.field_type.as_str()),
            "case {case}: expected the stored type, got {:?}",
            field["Type"]
        );
        // No `ShowHtmlContent` on the request, so every value is the plain rendering —
        // including the fields whose content also has a marked-up one.
        assert_eq!(
            field["Value"].as_str(),
            Some(stored.value.as_str()),
            "case {case}: expected the stored plain value, got {:?}",
            field["Value"]
        );
    }

    // A detail read is a report: a second one answers exactly the same, and the listing
    // the insight belongs to is untouched, so reading the detail moved nothing.
    let before = list(&emulator, entity.clone()).await;
    let again = describe_insight(&emulator, entity.clone(), &id, json!({})).await;
    assert_eq!(
        again.body, answer.body,
        "case whole detailed view(twice): a read must leave the insight as it was, got {:?}",
        again.body
    );
    let after = list(&emulator, entity).await;
    assert_eq!(
        after.body, before.body,
        "case whole detailed view(listing): a detail read must leave the listing as it was, got {:?}",
        after.body
    );
}

#[tokio::test]
async fn show_html_content_picks_the_rendering_each_field_is_reported_in() {
    let emulator = Emulator::new();
    let id = seeded(&emulator, "insight-detail-html").await;
    let entity = domain_entity("insight-detail-html");

    // The three answers a request can give `ShowHtmlContent`: the marked-up rendering
    // only where it asks for HTML content.
    for (show_html, extra) in [
        (false, json!({})),
        (false, json!({"ShowHtmlContent": false})),
        (true, json!({"ShowHtmlContent": true})),
    ] {
        let case = format!("ShowHtmlContent {extra:?}");
        let answer = describe_insight(&emulator, entity.clone(), &id, extra.clone()).await;
        let reported = fields(&answer, &case);
        assert_eq!(
            names(&reported),
            field_names(),
            "case {case}: expected the same fields whichever rendering was asked for, got {:?}",
            names(&reported)
        );

        for stored in stored_fields() {
            let case = format!("{case}/{:?}", stored.name);
            let expected = match (show_html, stored.html_value.as_deref()) {
                // A field with a marked-up rendering reports it only for a request that
                // asked for HTML content; one without reports its plain content either
                // way.
                (true, Some(html)) => html.to_string(),
                _ => stored.value.clone(),
            };
            assert_eq!(
                reported_value(&reported, &stored.name, &case),
                expected,
                "case {case}: expected the {} rendering, got {:?}",
                if show_html { "marked-up" } else { "plain" },
                reported_value(&reported, &stored.name, &case)
            );
        }
    }
}

#[tokio::test]
async fn an_insight_is_addressed_through_the_entity_that_holds_it() {
    let emulator = Emulator::new();
    let id = seeded(&emulator, "insight-detail-entity").await;
    create_baseline(&emulator, "insight-detail-other").await;

    // One insight held for the ACCOUNT itself, so the entity table covers both homes an
    // insight can have.
    let account_wide_id = guid("000000000002");
    let account_wide = record(
        &account_wide_id,
        SystemTime::now(),
        vec![InsightDetailField {
            name: "Summary".to_string(),
            field_type: "text".to_string(),
            value: "Reserved Instance coverage is low for this account.".to_string(),
            html_value: None,
        }],
    );
    {
        let store = emulator.store();
        let mut guard = store.lock();
        guard
            .account_insights
            .insert(account_wide.insight_id.clone(), account_wide);
    }

    // Every entity type the model admits, with the account named both ways a caller can
    // name it: a `DomainName` entity holds the insights of that one domain, and an
    // `Account` entity those of the account and of every domain it holds.
    for (entity, insight_id, expected_first_field) in [
        (
            domain_entity("insight-detail-entity"),
            id.clone(),
            "Summary",
        ),
        (json!({"Type": "Account"}), id.clone(), "Summary"),
        (
            json!({"Type": "Account", "Value": engine::emulator_account_id()}),
            id.clone(),
            "Summary",
        ),
        (
            json!({"Type": "Account"}),
            account_wide_id.clone(),
            "Summary",
        ),
    ] {
        let case = format!("DescribeInsightDetails({entity:?}, {insight_id:?})");
        assert!(
            INSIGHT_ENTITY_TYPE_VALUES.contains(&entity["Type"].as_str().unwrap_or_default()),
            "case {case}: the row must name a modelled entity type"
        );
        let answer = describe_insight(&emulator, entity, &insight_id, json!({})).await;
        let reported = fields(&answer, &case);
        assert_eq!(
            names(&reported).first().map(String::as_str),
            Some(expected_first_field),
            "case {case}: expected the insight's own fields, got {:?}",
            answer.body
        );
    }

    // A sibling domain of the same account does not hold this insight, and no
    // `DomainName` entity holds the account's own one: an insight is reported by the
    // entity that holds it, so another entity of the same account selects no insight
    // under that id — the id itself never widens the entity a read is scoped to, and the
    // read reports the service's guidance rather than that entity's stored fields.
    for (entity, insight_id) in [
        (domain_entity("insight-detail-other"), id.clone()),
        (
            domain_entity("insight-detail-entity"),
            account_wide_id.clone(),
        ),
    ] {
        let case = format!("DescribeInsightDetails({entity:?}, {insight_id:?})");
        let answer = describe_insight(&emulator, entity.clone(), &insight_id, json!({})).await;
        guidance(&answer, &case);
    }
}

#[tokio::test]
async fn an_id_the_entity_selects_no_insight_under_is_answered_by_what_its_address_resolves() {
    let emulator = Emulator::new();
    let id = seeded(&emulator, "insight-detail-missing").await;
    create_baseline(&emulator, "insight-detail-nothing").await;

    // Id rows differ along the shape of the id: one never raised, one all zeroes, one
    // all `f`s, one with its hex digits in both cases, and the stored id uppercased —
    // `GUID`'s pattern admits either case, and an id is the exact value the service
    // handed out, so one differing in case names no insight.
    let unheld_ids = [
        guid("000000000099"),
        "00000000-0000-0000-0000-000000000000".to_string(),
        "ffffffff-ffff-ffff-ffff-ffffffffffff".to_string(),
        "A1b2C3d4-E5f6-7a8b-9C0d-1e2F3a4B5c6D".to_string(),
        id.to_uppercase(),
    ];
    // Entity rows: the domain holding an insight, a sibling domain holding none, and
    // the account named both ways — every entity type the model admits, each one this
    // account does hold, so the entity resolves and only the id does not.
    let entities = [
        domain_entity("insight-detail-missing"),
        domain_entity("insight-detail-nothing"),
        json!({"Type": ACCOUNT_ENTITY_TYPE}),
        json!({"Type": ACCOUNT_ENTITY_TYPE, "Value": engine::emulator_account_id()}),
    ];

    for entity in &entities {
        let before = list(&emulator, entity.clone()).await;
        for unheld in &unheld_ids {
            assert_ne!(
                unheld, &id,
                "case DescribeInsightDetails({unheld:?}): the row must name another id"
            );
            // The three ways a request can answer `ShowHtmlContent`: the guidance is
            // content the service states rather than content it marks up, so every one of
            // them is detailed with the same field.
            for extra in [
                json!({}),
                json!({"ShowHtmlContent": false}),
                json!({"ShowHtmlContent": true}),
            ] {
                let case = format!("DescribeInsightDetails({entity:?}, {unheld:?}, {extra:?})");
                let answer =
                    describe_insight(&emulator, entity.clone(), unheld, extra.clone()).await;
                // What the address RESOLVED decides it: a read scoped to a domain this
                // account holds is detailed with the guidance, while the account-wide
                // entity names no resource but the insight its id selects, so an id
                // selecting none is the missing resource.
                assert_unselected_id(&answer, &case, entity);
            }
        }
        // The reads raised no insight: the entity's listing is exactly what it was.
        let after = list(&emulator, entity.clone()).await;
        assert_eq!(
            after.body, before.body,
            "case DescribeInsightDetails({entity:?})(listing): a detail read must leave the \
             listing as it was, got {:?}",
            after.body
        );
    }

    // The insight the entity DOES hold is still reported in full, so an id it holds
    // none under left the set it was looked up in as it was.
    let answer = describe_insight(
        &emulator,
        domain_entity("insight-detail-missing"),
        &id,
        json!({}),
    )
    .await;
    assert_eq!(
        names(&fields(&answer, "the insight that is held")),
        field_names(),
        "case the insight that is held: expected its stored fields, got {:?}",
        answer.body
    );
}

#[tokio::test]
async fn an_entity_this_account_does_not_hold_is_refused() {
    let emulator = Emulator::new();
    let id = seeded(&emulator, "insight-detail-entities").await;

    // Rows differ along the shape of the name: one never created, one at the shortest
    // the model admits, and one at its longest.
    for name in [
        "never-created",
        "abc",
        "a-domain-name-at-its-upper-b",
    ] {
        let answer = describe_insight(&emulator, domain_entity(name), &id, json!({})).await;
        refused(
            &answer,
            &format!("DescribeInsightDetails(DomainName {name:?})"),
            RECORDED_NOT_FOUND_STATUS,
            "ResourceNotFoundException",
        );
    }

    // A `DomainName` entity names its domain in `Value`, so one that names none
    // resolves to no entity rather than to every domain; and an `Account` entity's
    // `Value` is "the Amazon Web Services account ID", of which this emulator serves
    // exactly the one the request was made for.
    for entity in [
        json!({"Type": "DomainName"}),
        json!({"Type": "DomainName", "Value": null}),
        json!({"Type": "Account", "Value": "123456789012"}),
        json!({"Type": "Account", "Value": "210987654321"}),
    ] {
        assert_ne!(
            entity["Value"].as_str().unwrap_or_default(),
            engine::emulator_account_id(),
            "case DescribeInsightDetails({entity:?}): the row must not name the served account"
        );
        let answer = describe_insight(&emulator, entity.clone(), &id, json!({})).await;
        refused(
            &answer,
            &format!("DescribeInsightDetails({entity:?})"),
            400,
            "ValidationException",
        );
    }
}

#[tokio::test]
async fn an_insight_the_service_raised_no_field_for_details_with_the_services_guidance() {
    let emulator = Emulator::new();
    create_baseline(&emulator, "insight-detail-empty").await;
    let bare_id = guid("000000000003");
    raise_on_domain(
        &emulator,
        "insight-detail-empty",
        record(&bare_id, SystemTime::now(), Vec::new()),
    );

    // `Fields` is required and the detailed view of an insight the service holds is never
    // empty, so an insight it has raised no detail field for details with the one field it
    // states about an insight's details — in either rendering, and the same on a second
    // read.
    for extra in [
        json!({}),
        json!({"ShowHtmlContent": false}),
        json!({"ShowHtmlContent": true}),
    ] {
        let case = format!("insight with no detail field {extra:?}");
        let answer = describe_insight(
            &emulator,
            domain_entity("insight-detail-empty"),
            &bare_id,
            extra.clone(),
        )
        .await;
        let reported = guidance(&answer, &case);
        let again = describe_insight(
            &emulator,
            domain_entity("insight-detail-empty"),
            &bare_id,
            extra,
        )
        .await;
        assert_eq!(
            fields(&again, &case),
            reported,
            "case {case}(twice): expected the same view, got {:?}",
            again.body
        );
    }
}
