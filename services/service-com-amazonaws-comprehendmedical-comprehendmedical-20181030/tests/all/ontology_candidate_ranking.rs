// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for how the synchronous inference answers LINK a mention to an
//! ontology, which is what `InferICD10CM`, `InferRxNorm` and `InferSNOMEDCT` exist for.
//!
//! The rules under test:
//!
//! * a linked mention reports the candidates of ITS OWN concept family, most likely
//!   first, as many as the ontology offers up to five, with strictly descending scores
//!   none of which is more than the mention's own confidence — so a client can page the
//!   candidates for review and tell a confident match from a long-tail one (what each
//!   score itself answers is pinned by
//!   `a_candidate_reports_how_well_its_own_description_names_the_span` below);
//! * every span the pass LINKS — an entity and one of its attributes alike — reports as
//!   many candidates as the ontology this service carries can rank for its own written
//!   form, the concepts the data linked leading the list, and the completion reaches only
//!   a concept that NAMES that form rather than one sharing a word every clinical
//!   description carries;
//! * a qualifier written as part of a condition's name carries the qualifier-value
//!   concept it names in its own right, and not the concepts of the mention it
//!   qualifies;
//! * an age counts how old the person is, so a medical-condition analysis reports it
//!   neither as a time expression of its own nor as the time of a condition, while a
//!   time expression beside a condition is still reported;
//! * a span the pass placed but the operation reports no entity for still takes its place
//!   in the response-wide `Id` sequence, and the ids rise with the offsets of the spans
//!   they number;
//! * every ontology candidate's score answers how well ITS OWN description names the
//!   written form it is reported on, never the rank the ontology gives it;
//! * every operation reports the version of its own model, in the documented n.n.n form,
//!   and `InferSNOMEDCT` reports the revision of the ontology it linked against.
//!
//! What the model fixes about a candidate's score is what it ANSWERS — "the level of
//! confidence that Amazon Comprehend Medical has that the entity is accurately linked to"
//! that concept — not a number, so the properties asserted here are the invariants that
//! follow from it: the score is above zero and never above the span's own confidence, the
//! list descends, it varies with the candidate's OWN description rather than with its
//! rank, and a span the ontology holds neighbours for reports more than one candidate. The
//! exact product the rows compute is this pass's own rule for reading a description's
//! overlap, stated here so a scorer that answers by rank alone cannot pass; a later
//! refinement of that rule is free to change the product, and only these invariants are
//! what the service promises.
//!
//! Each table spans a CLASS of input — several concept families, all three ontologies,
//! both polarities of each rule — rather than one note. Every request goes over the wire
//! the service actually speaks (awsJson1_1, `POST /` with
//! `X-Amz-Target: ComprehendMedical_20181030.<Operation>`).

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The `X-Amz-Target` prefix from the service's wire protocol.
const TARGET_PREFIX: &str = "ComprehendMedical_20181030";

/// The most candidates one mention reports, which is what the ontology-linking answers
/// page for review.
const CANDIDATE_LIMIT: usize = 5;

/// The least share of a span's confidence a candidate the service's ontology data LINKS
/// the span's written form to is reported with, whatever its own description says.
const LINKED_CANDIDATE_FLOOR: f64 = 0.2;

/// How far two floats this suite computes the same value in different ways may differ.
const TOLERANCE: f64 = 1.0e-4;

/// A router carrying only this service, over its own engine.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_comprehendmedical_comprehendmedical_20181030::EmulatedService::new(
            engine,
        ),
    );
    router
}

/// Call `operation` with `text` and return the parsed JSON answer, asserting it succeeded.
async fn infer(router: &MultiServiceRouter, operation: &str, text: &str) -> Value {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
        .header("content-type", "application/x-amz-json-1.1")
        .body(bytes::Bytes::from(
            serde_json::to_vec(&json!({"Text": text})).expect("serialize the request body"),
        ))
        .expect("build the request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let answer: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    assert_eq!(
        status, 200,
        "case {operation} {text:?}: expected 200, got {status} {answer}"
    );
    answer
}

/// The entities of an answer.
fn entities(case: &str, answer: &Value) -> Vec<Value> {
    answer["Entities"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: an answer carries an Entities list, got {answer}"))
}

/// The attributes of an entity.
fn attributes(entity: &Value) -> Vec<Value> {
    entity["Attributes"].as_array().cloned().unwrap_or_default()
}

/// The entity of `answer` written as `mention`.
fn mention_entity(case: &str, answer: &Value, mention: &str) -> Value {
    entities(case, answer)
        .into_iter()
        .find(|entity| entity["Text"].as_str() == Some(mention))
        .unwrap_or_else(|| panic!("case {case:?}: expected the mention {mention:?}, got {answer}"))
}

/// The candidate list `span` reports, under the member name the ontology's own concept
/// list carries.
fn candidates(case: &str, span: &Value, member: &str) -> Vec<Value> {
    span[member]
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: expected a {member} list, got {span}"))
}

/// A score the answer reports.
fn score(span: &Value, member: &str) -> f64 {
    span[member].as_f64().unwrap_or_default()
}

/// Assert `list` is a ranked candidate list of the mention `span`: at least `least`
/// candidates and never more than the limit, strictly descending scores in (0, 1), and no
/// candidate reported as a likelier match than the span's own detection is confident.
fn assert_ranked_candidates(case: &str, span: &Value, list: &[Value], least: usize) {
    assert!(
        list.len() >= least && list.len() <= CANDIDATE_LIMIT,
        "case {case:?}: expected {least}..={CANDIDATE_LIMIT} ranked candidates, got {} in \
         {span}",
        list.len()
    );
    let scores: Vec<f64> = list.iter().map(|concept| score(concept, "Score")).collect();
    assert!(
        scores.iter().all(|value| *value > 0.0 && *value < 1.0),
        "case {case:?}: every candidate score is a confidence in (0, 1), got {scores:?}"
    );
    assert!(
        scores.windows(2).all(|pair| pair[0] > pair[1]),
        "case {case:?}: the candidates are ranked most likely first, got {scores:?}"
    );
    // A candidate's score reports the likelihood of THAT match, so it is bounded by how
    // confident the detection of the span itself is: the ontology cannot be surer that the
    // span refers to a concept than the pass is that the span is there at all.
    let confidence = score(span, "Score");
    assert!(
        scores.iter().all(|value| *value <= confidence + TOLERANCE),
        "case {case:?}: no candidate is likelier than the span's own confidence \
         {confidence}, got {scores:?}"
    );
}

/// One linked-mention row: the operation, the text, the mention the entity reports, the
/// member its candidate list is carried under, how many candidates the ontology offers
/// for that family at least, and the string every candidate of the family carries —
/// the ICD-10-CM category code the family lives under, or (where the ontology's codes are
/// opaque numbers) a word of the concept's own name.
type CandidateRow = (
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    usize,
    Option<Family>,
);

/// What makes a candidate part of the mention's own concept family.
#[derive(Debug, Clone, Copy)]
enum Family {
    /// Every candidate's `Code` starts with this, which for ICD-10-CM is the category
    /// the family is classified under.
    Code(&'static str),
    /// Every candidate's `Description` carries this word, which is how a family shows in
    /// an ontology whose codes are opaque identifiers.
    Description(&'static str),
}

/// A linked mention reports the ranked candidates of its own concept family.
///
/// The rows span all three ontologies and several families each, so a handler that
/// answered with one canned list, truncated the list, or attached one family's codes to
/// another family's text fails here.
#[tokio::test]
async fn a_linked_mention_reports_the_ranked_candidates_of_its_own_family() {
    let router = router();
    // Sources: the ICD-10-CM tabular list (Centers for Disease Control) for the category
    // codes, the SNOMED CT US edition for the concept names, and the RxNorm release for
    // the ingredient and brand concepts — the three ontologies the operations link to.
    let rows: &[CandidateRow] = &[
        (
            "InferICD10CM",
            "Patient has type 2 diabetes mellitus.",
            "diabetes mellitus",
            "ICD10CMConcepts",
            5,
            Some(Family::Code("E11")),
        ),
        (
            "InferICD10CM",
            "Patient has type 1 diabetes mellitus.",
            "diabetes mellitus",
            "ICD10CMConcepts",
            5,
            Some(Family::Code("E10")),
        ),
        (
            "InferICD10CM",
            "Patient has hypertension.",
            "hypertension",
            "ICD10CMConcepts",
            5,
            Some(Family::Code("I1")),
        ),
        (
            "InferICD10CM",
            "Patient has congestive heart failure.",
            "congestive heart failure",
            "ICD10CMConcepts",
            5,
            Some(Family::Code("I50")),
        ),
        (
            "InferICD10CM",
            "Patient has asthma.",
            "asthma",
            "ICD10CMConcepts",
            5,
            Some(Family::Code("J45")),
        ),
        (
            "InferICD10CM",
            "Patient has pneumonia.",
            "pneumonia",
            "ICD10CMConcepts",
            5,
            Some(Family::Code("J1")),
        ),
        (
            "InferICD10CM",
            "Patient reports a cough.",
            "cough",
            "ICD10CMConcepts",
            3,
            Some(Family::Code("R05")),
        ),
        (
            "InferSNOMEDCT",
            "Patient has type 2 diabetes mellitus.",
            "diabetes mellitus",
            "SNOMEDCTConcepts",
            5,
            Some(Family::Description("diabet")),
        ),
        (
            "InferSNOMEDCT",
            "Patient has hypertension.",
            "hypertension",
            "SNOMEDCTConcepts",
            5,
            Some(Family::Description("hyperten")),
        ),
        (
            "InferSNOMEDCT",
            "Patient had a myocardial infarction.",
            "myocardial infarction",
            "SNOMEDCTConcepts",
            5,
            Some(Family::Description("myocardial")),
        ),
        (
            "InferSNOMEDCT",
            "Patient has asthma.",
            "asthma",
            "SNOMEDCTConcepts",
            3,
            Some(Family::Description("asthma")),
        ),
        (
            "InferSNOMEDCT",
            "Patient reports a cough.",
            "cough",
            "SNOMEDCTConcepts",
            3,
            Some(Family::Description("cough")),
        ),
        (
            "InferRxNorm",
            "He takes metformin 500 mg orally twice daily.",
            "metformin",
            "RxNormConcepts",
            1,
            Some(Family::Description("metformin")),
        ),
        (
            "InferRxNorm",
            "He takes insulin glargine 20 units nightly.",
            "insulin glargine",
            "RxNormConcepts",
            2,
            Some(Family::Description("insulin")),
        ),
        (
            // A brand name is linked to its own brand concept first and then to the
            // ingredient it delivers, so the family is the brand, not one word.
            "InferRxNorm",
            "He takes Tylenol 500 mg as needed.",
            "Tylenol",
            "RxNormConcepts",
            2,
            None,
        ),
    ];

    for (operation, text, mention, member, least, family) in rows {
        let case = format!("{operation} {text:?}");
        let answer = infer(&router, operation, text).await;
        let entity = mention_entity(&case, &answer, mention);
        let list = candidates(&case, &entity, member);
        assert_ranked_candidates(&case, &entity, &list, *least);
        for concept in &list {
            let code = concept["Code"].as_str().unwrap_or_default();
            let description = concept["Description"].as_str().unwrap_or_default();
            assert!(
                !code.is_empty() && !description.is_empty(),
                "case {case:?}: every candidate names a code and its description, got \
                 {concept}"
            );
            match family {
                Some(Family::Code(prefix)) => assert!(
                    code.starts_with(prefix),
                    "case {case:?}: the candidates of {mention:?} belong to the family \
                     {prefix}, got {code:?} ({description:?})"
                ),
                Some(Family::Description(word)) => assert!(
                    description.to_lowercase().contains(word),
                    "case {case:?}: the candidates of {mention:?} name the concept family \
                     {word:?}, got {description:?} ({code:?})"
                ),
                None => {}
            }
        }
    }
}

/// The candidates an ATTRIBUTE reports are ranked for its OWN written form: the modelled
/// `SNOMEDCTAttribute` carries "the SNOMED-CT concepts specific to an attribute", while
/// the entity's list answers what the ENTITY could refer to — so a qualifier split off a
/// condition's name LEADS with the qualifier value it names, not with the disorder its
/// entity is ranked to.
///
/// The rows span the subtype qualifiers (the digit and the roman-numeral spelling) and
/// the acuity, quality and direction modifiers, so a handler that ranked an attribute's
/// list for its entity's written form — which turns a qualifier into the disorder it
/// qualifies — fails here, as does one that left a modifier's own concepts out.
#[tokio::test]
async fn an_attribute_reports_the_concepts_its_own_text_names() {
    let router = router();
    // (text, the mention, the attribute as written, the concept its own form names).
    // Source: the qualifier-value hierarchy of the SNOMED CT US edition.
    let rows: &[(&str, &str, &str, &str, &str)] = &[
        (
            "Patient has type 2 diabetes mellitus.",
            "diabetes mellitus",
            "type 2",
            "258195006",
            "Type 2 (qualifier value)",
        ),
        (
            "Patient has type 1 diabetes mellitus.",
            "diabetes mellitus",
            "type 1",
            "258194007",
            "Type 1 (qualifier value)",
        ),
        (
            "Patient has type II diabetes mellitus.",
            "diabetes mellitus",
            "type II",
            "258195006",
            "Type 2 (qualifier value)",
        ),
        (
            "Chronic cough with wheezing.",
            "cough",
            "Chronic",
            "90734009",
            "Chronic (qualifier value)",
        ),
        (
            "Acute bronchitis on examination.",
            "bronchitis",
            "Acute",
            "263654008",
            "Acute (qualifier value)",
        ),
        (
            "Severe asthma reported.",
            "asthma",
            "Severe",
            "24484000",
            "Severe (severity modifier) (qualifier value)",
        ),
        (
            "Mild anemia noted.",
            "anemia",
            "Mild",
            "255604002",
            "Mild (qualifier value)",
        ),
        (
            "Left pneumonia on the radiograph.",
            "pneumonia",
            "Left",
            "7771000",
            "Left (qualifier value)",
        ),
        (
            "Bilateral pneumonia on the radiograph.",
            "pneumonia",
            "Bilateral",
            "51440002",
            "Right and left (qualifier value)",
        ),
    ];

    for (text, mention, written, code, description) in rows {
        let case = format!("InferSNOMEDCT {text:?} {written:?}");
        let answer = infer(&router, "InferSNOMEDCT", text).await;
        let entity = mention_entity(&case, &answer, mention);
        let entity_codes: Vec<String> = candidates(&case, &entity, "SNOMEDCTConcepts")
            .iter()
            .map(|concept| concept["Code"].as_str().unwrap_or_default().to_string())
            .collect();
        let attribute = attributes(&entity)
            .into_iter()
            .find(|attribute| attribute["Text"].as_str() == Some(*written))
            .unwrap_or_else(|| {
                panic!("case {case:?}: expected the attribute {written:?}, got {answer}")
            });
        let list = candidates(&case, &attribute, "SNOMEDCTConcepts");
        assert_ranked_candidates(&case, &attribute, &list, 1);

        let leading = (
            list[0]["Code"].as_str().unwrap_or_default(),
            list[0]["Description"].as_str().unwrap_or_default(),
        );
        assert_eq!(
            leading,
            (*code, *description),
            "case {case:?}: the attribute's own concept is its leading candidate, got \
             {leading:?}"
        );
        let reported: Vec<String> = list
            .iter()
            .map(|concept| concept["Code"].as_str().unwrap_or_default().to_string())
            .collect();
        assert!(
            entity_codes.first().is_some_and(|first| first != code),
            "case {case:?}: the two lists answer different questions, so the qualifier the \
             attribute names is not what the entity is ranked to, got {entity_codes:?} and \
             {reported:?}"
        );
        // The concept the attribute's own form NAMES leads its list, so a concept its
        // entity is ranked to as well can only have been reached by searching the
        // ontology: a later candidate, never the leading one.
        assert!(
            reported
                .iter()
                .enumerate()
                .filter(|(_, reported_code)| entity_codes.contains(reported_code))
                .all(|(rank, _)| rank > 0),
            "case {case:?}: a concept the entity is also ranked to is a later candidate of \
             the attribute, got {reported:?} against {entity_codes:?}"
        );
    }
}

/// Every span `operation` reports for `text`, entities and attributes together, as
/// `(BeginOffset, Id, Type, Text)`.
async fn reported_spans(
    router: &MultiServiceRouter,
    operation: &str,
    text: &str,
) -> Vec<(i64, i64, String, String)> {
    let case = format!("{operation} {text:?}");
    let answer = infer(router, operation, text).await;
    let mut spans = Vec::new();
    for entity in entities(&case, &answer) {
        for span in std::iter::once(entity.clone()).chain(attributes(&entity)) {
            spans.push((
                span["BeginOffset"].as_i64().unwrap_or_default(),
                span["Id"].as_i64().unwrap_or_default(),
                span["Type"].as_str().unwrap_or_default().to_string(),
                span["Text"].as_str().unwrap_or_default().to_string(),
            ));
        }
    }
    spans.sort_unstable();
    spans
}

/// An age is how old the person is, not the time course of a condition: a
/// medical-condition analysis reports it as neither entity nor attribute, while a time
/// expression written beside a condition is still reported. The age span is still placed,
/// so it takes its place in the response-wide `Id` sequence.
///
/// The rows are the written forms an age takes ("65-year-old", "Age: 72", "45 yo") in
/// both operations that place time expressions, plus the opposite polarity: a duration
/// that really is the time course of the condition.
#[tokio::test]
async fn an_age_is_not_the_time_course_of_a_condition() {
    let router = router();
    let rows: &[(&str, &str, &str, Option<&str>)] = &[
        (
            "InferICD10CM",
            "Patient is a 65-year-old male with type 2 diabetes mellitus.",
            "65",
            None,
        ),
        (
            "InferICD10CM",
            "Age: 72. Patient has asthma.",
            "72",
            None,
        ),
        (
            "InferICD10CM",
            "45 yo male with pneumonia.",
            "45",
            None,
        ),
        (
            "InferSNOMEDCT",
            "Patient is a 65-year-old male with type 2 diabetes mellitus.",
            "65",
            None,
        ),
        (
            // The same analysis still reports the duration a condition has lasted.
            "InferICD10CM",
            "Patient reports fever for three weeks.",
            "99",
            Some("three weeks"),
        ),
    ];

    for (operation, text, age, timed) in rows {
        let case = format!("{operation} {text:?}");
        let spans = reported_spans(&router, operation, text).await;
        assert!(
            !spans.is_empty(),
            "case {case:?}: the text names a condition, got no span"
        );
        assert!(
            spans.iter().all(|(_, _, _, written)| written != age),
            "case {case:?}: an age is not a span of a medical-condition analysis, got \
             {spans:?}"
        );
        match timed {
            Some(duration) => assert!(
                spans
                    .iter()
                    .any(|(_, _, kind, written)| written == duration
                        && (kind == "TIME_EXPRESSION" || kind == "TIME_TO_DX_NAME")),
                "case {case:?}: the time course of the condition is still reported, got \
                 {spans:?}"
            ),
            None => assert!(
                spans
                    .iter()
                    .all(|(_, _, kind, _)| kind != "TIME_EXPRESSION"
                        && kind != "TIME_TO_DX_NAME"),
                "case {case:?}: an age is the age of the person, not a time expression of \
                 a condition, got {spans:?}"
            ),
        }

        // The ids rise with the offsets of the spans they number, and the age the pass
        // placed without reporting has taken the first of them.
        let ids: Vec<i64> = spans.iter().map(|(_, id, _, _)| *id).collect();
        assert!(
            ids.windows(2).all(|pair| pair[0] < pair[1]),
            "case {case:?}: the Ids rise with the offsets of the spans they number, got \
             {spans:?}"
        );
        if timed.is_none() {
            assert!(
                ids.first().is_some_and(|first| *first > 1),
                "case {case:?}: the age span the pass placed takes the first Id, so the \
                 first reported span carries a later one, got {spans:?}"
            );
        }
    }
}

/// A value written under a field label of its own identifier class is established by two
/// independent pieces of evidence — the characters take the class's written form AND the
/// record says which class the value is — so `DetectPHI` reports it with more confidence
/// than the same written form standing on its own.
///
/// The rows are three identifier classes, each in both polarities, so a handler whose
/// `Score` is one constant per detection class fails here.
#[tokio::test]
async fn a_labelled_value_is_reported_with_more_confidence_than_a_bare_one() {
    let router = router();
    // (the class's mention, the note that labels it, the note that does not)
    let rows: &[(&str, &str, &str)] = &[
        (
            "617-555-0199",
            "Phone: 617-555-0199.",
            "Reach him on 617-555-0199.",
        ),
        (
            "03/14/2024",
            "Date of visit: 03/14/2024.",
            "He felt unwell around 03/14/2024.",
        ),
        (
            "j.smith@example.com",
            "Email: j.smith@example.com.",
            "Write to j.smith@example.com.",
        ),
    ];

    for (mention, labelled, bare) in rows {
        let case = format!("DetectPHI {mention:?}");
        let under_label = infer(&router, "DetectPHI", labelled).await;
        let on_its_own = infer(&router, "DetectPHI", bare).await;
        let with_label = score(&mention_entity(&case, &under_label, mention), "Score");
        let without = score(&mention_entity(&case, &on_its_own, mention), "Score");
        assert!(
            with_label > without,
            "case {case:?}: a value written under a field label of its class carries more \
             evidence than the same form alone, got {with_label} under {labelled:?} and \
             {without} in {bare:?}"
        );
        assert!(
            with_label < 1.0 && without > 0.0,
            "case {case:?}: both detections report a confidence in (0, 1), got \
             {with_label} and {without}"
        );
    }
}

/// Every one of the four synchronous operations reports the version of ITS OWN model, in
/// the documented n.n.n form and stable from call to call, and `InferSNOMEDCT` reports
/// the revision of the ontology it linked against.
#[tokio::test]
async fn every_operation_reports_the_version_of_its_own_model() {
    let router = router();
    let text = "Patient has type 2 diabetes mellitus and takes metformin 500 mg daily. \
                Reach him on 617-555-0199.";
    for operation in ["DetectPHI", "InferICD10CM", "InferRxNorm", "InferSNOMEDCT"] {
        let first = infer(&router, operation, text).await;
        let second = infer(&router, operation, text).await;
        let version = first["ModelVersion"].as_str().unwrap_or_default();
        let parts: Vec<&str> = version.split('.').collect();
        assert!(
            parts.len() == 3
                && parts
                    .iter()
                    .all(|part| !part.is_empty() && part.chars().all(|digit| digit.is_numeric())),
            "case {operation}: ModelVersion is the model's version in n.n.n form, got \
             {version:?}"
        );
        assert_eq!(
            second["ModelVersion"].as_str().unwrap_or_default(),
            version,
            "case {operation}: the model behind an analysis does not change between two \
             calls, got {second}"
        );
    }

    let answer = infer(&router, "InferSNOMEDCT", text).await;
    let details = &answer["SNOMEDCTDetails"];
    // The edition and the language are what the modelled members document: the US
    // edition, and US English for every operation of the service.
    assert_eq!(
        details["Edition"].as_str(),
        Some("US"),
        "case InferSNOMEDCT: the revision reports the edition it linked against, got \
         {details}"
    );
    assert_eq!(
        details["Language"].as_str(),
        Some("en"),
        "case InferSNOMEDCT: the revision reports the language of its descriptions, got \
         {details}"
    );
    let version_date = details["VersionDate"].as_str().unwrap_or_default();
    let fields: Vec<&str> = version_date.split('-').collect();
    assert!(
        fields.len() == 3
            && fields
                .iter()
                .all(|field| !field.is_empty() && field.chars().all(|digit| digit.is_numeric())),
        "case InferSNOMEDCT: the revision reports the release date of the ontology, got \
         {version_date:?}"
    );
}

/// The words a written form and an ontology description are compared by: the alphanumeric
/// runs each is written in, lowercased, counted once.
///
/// The suite states the comparison itself rather than calling the service's own: an
/// integration test reaches the crate over the wire, and a check that asked the
/// implementation what it computes could not tell a per-candidate likelihood from a rank
/// ladder. Stated here it is the independent expectation the rows are measured against.
fn concept_words(text: &str) -> BTreeSet<String> {
    text.split(|character: char| !character.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_lowercase)
        .collect()
}

/// How well the ontology description `description` names the written form `written`:
/// twice the words the two share over the number of words they carry together, which is 1
/// for a description naming the written form and nothing else and 0 for one repeating
/// none of its words.
fn concept_match(written: &str, description: &str) -> f64 {
    let written = concept_words(written);
    let description = concept_words(description);
    if written.is_empty() || description.is_empty() {
        return 0.0;
    }
    let shared = written.intersection(&description).count();
    2.0 * shared as f64 / (written.len() + description.len()) as f64
}

/// Which level of the answer a row reads its span off.
#[derive(Debug, Clone, Copy)]
enum Level {
    Entity,
    Attribute,
}

/// The entity or the attribute of `answer` written exactly as `written`, whichever `level`
/// names.
fn level_span(case: &str, answer: &Value, level: Level, written: &str) -> Value {
    let found = entities(case, answer);
    let spans: Vec<Value> = match level {
        Level::Entity => found,
        Level::Attribute => found.iter().flat_map(attributes).collect(),
    };
    spans
        .into_iter()
        .find(|span| span["Text"].as_str() == Some(written))
        .unwrap_or_else(|| panic!("case {case:?}: expected the span {written:?}, got {answer}"))
}

/// One likelihood row: the operation, the text, the level the span is reported at, the
/// span as written, the member its candidate list is carried under, and how many
/// candidates the ontology offers for that form at least.
type LikelihoodRow = (
    &'static str,
    &'static str,
    Level,
    &'static str,
    &'static str,
    usize,
);

/// A candidate's score is how well its OWN description names the written form it is
/// reported on, carried by the confidence of that span.
///
/// Every modelled ontology concept's `Score` reports "the likelihood of the match", which
/// is an answer about THAT candidate — so a handler whose candidate scores are a function
/// of rank alone (the span's confidence split between its candidates, each successive one
/// keeping a fixed share of the one in front of it) reports the same ladder for every span
/// with the same number of candidates, and fails on every row here whose leading
/// description names its span to anything but that one share.
///
/// The rows span the three ontologies, the entity and the attribute level, and leading
/// descriptions that name their span exactly ("metformin"), nearly ("Diabetes mellitus
/// (disorder)") and only in part ("Essential (primary) hypertension"). The attribute rows
/// are also where the whole product of the operation shows: an attribute reports the same
/// ranked list an entity does, so a list truncated to the one concept its written form
/// names fails them.
#[tokio::test]
async fn a_candidate_reports_how_well_its_own_description_names_the_span() {
    let router = router();
    let rows: &[LikelihoodRow] = &[
        (
            "InferICD10CM",
            "Patient has diabetes mellitus.",
            Level::Entity,
            "diabetes mellitus",
            "ICD10CMConcepts",
            5,
        ),
        (
            "InferICD10CM",
            "Patient has hypertension.",
            Level::Entity,
            "hypertension",
            "ICD10CMConcepts",
            5,
        ),
        (
            "InferSNOMEDCT",
            "Patient has diabetes mellitus.",
            Level::Entity,
            "diabetes mellitus",
            "SNOMEDCTConcepts",
            5,
        ),
        (
            "InferSNOMEDCT",
            "Patient has asthma.",
            Level::Entity,
            "asthma",
            "SNOMEDCTConcepts",
            3,
        ),
        (
            "InferRxNorm",
            "He takes metformin 500 mg orally twice daily.",
            Level::Entity,
            "metformin",
            "RxNormConcepts",
            1,
        ),
        (
            "InferRxNorm",
            "He takes insulin glargine 20 units nightly.",
            Level::Entity,
            "insulin glargine",
            "RxNormConcepts",
            2,
        ),
        (
            "InferSNOMEDCT",
            "Patient has type 2 diabetes mellitus.",
            Level::Attribute,
            "type 2",
            "SNOMEDCTConcepts",
            2,
        ),
        (
            "InferSNOMEDCT",
            "Chronic cough with wheezing.",
            Level::Attribute,
            "Chronic",
            "SNOMEDCTConcepts",
            2,
        ),
        (
            "InferSNOMEDCT",
            "Left lung pneumonia.",
            Level::Attribute,
            "lung",
            "SNOMEDCTConcepts",
            2,
        ),
    ];

    for (operation, text, level, written, member, least) in rows {
        let case = format!("{operation} {text:?} {written:?}");
        let answer = infer(&router, operation, text).await;
        let span = level_span(&case, &answer, *level, written);
        let list = candidates(&case, &span, member);
        assert!(
            list.len() >= *least && list.len() <= CANDIDATE_LIMIT,
            "case {case:?}: expected {least}..={CANDIDATE_LIMIT} candidates, got {} in {span}",
            list.len()
        );
        let confidence = score(&span, "Score");
        assert!(
            confidence > 0.0 && confidence < 1.0,
            "case {case:?}: a span reports a confidence in (0, 1), got {confidence}"
        );

        // Nothing ranks ahead of the leading candidate, so its score is exactly what its
        // own description answers about this written form.
        let description = list[0]["Description"].as_str().unwrap_or_default();
        let named = concept_match(written, description);
        let expected = confidence * named;
        let reported = score(&list[0], "Score");
        assert!(
            (reported - expected).abs() < TOLERANCE,
            "case {case:?}: the leading candidate {description:?} names {written:?} to \
             {named}, so it is reported at {expected}, got {reported}"
        );

        let mut previous = f64::INFINITY;
        for candidate in &list {
            let description = candidate["Description"].as_str().unwrap_or_default();
            let reported = score(candidate, "Score");
            assert!(
                reported > 0.0 && reported < previous,
                "case {case:?}: the candidates are ranked most likely first, got {reported} \
                 after {previous} in {span}"
            );
            let bound =
                confidence * concept_match(written, description).max(LINKED_CANDIDATE_FLOOR);
            assert!(
                reported <= bound + TOLERANCE,
                "case {case:?}: {description:?} is reported at {reported}, above what its \
                 own description answers about {written:?} ({bound})"
            );
            previous = reported;
        }
    }
}

/// A candidate the ontology data LINKS the written form to is reported whatever its own
/// description says: a mention written as an abbreviation, and a brand name linked to the
/// ingredient it delivers, share no word with the concept's own name, and the match is
/// still not reported as impossible.
///
/// The rows are the written forms whose description overlap is zero — abbreviations of a
/// generic name, a brand name and the ingredient behind it — in both ontologies that carry
/// them, so a handler scoring a candidate by description overlap ALONE would report zero
/// for them and fails here.
#[tokio::test]
async fn a_linked_candidate_is_reported_whatever_its_description_says() {
    let router = router();
    // (operation, text, the mention, the member, the code whose description names the
    // mention in other words). Sources: the RxNorm ingredient and brand concepts, and the
    // SNOMED CT disorder hierarchy.
    let rows: &[(&str, &str, &str, &str, &str)] = &[
        (
            "InferRxNorm",
            "He takes ASA 81 mg daily.",
            "ASA",
            "RxNormConcepts",
            "1191",
        ),
        (
            "InferRxNorm",
            "He takes HCTZ 25 mg daily.",
            "HCTZ",
            "RxNormConcepts",
            "5487",
        ),
        (
            "InferRxNorm",
            "He takes Tylenol 500 mg as needed.",
            "Tylenol",
            "RxNormConcepts",
            "161",
        ),
    ];

    for (operation, text, written, member, code) in rows {
        let case = format!("{operation} {text:?} {written:?}");
        let answer = infer(&router, operation, text).await;
        let span = level_span(&case, &answer, Level::Entity, written);
        let list = candidates(&case, &span, member);
        let confidence = score(&span, "Score");
        let linked = list
            .iter()
            .find(|candidate| candidate["Code"].as_str() == Some(*code))
            .unwrap_or_else(|| panic!("case {case:?}: expected the concept {code}, got {span}"));
        let description = linked["Description"].as_str().unwrap_or_default();
        assert_eq!(
            concept_match(written, description),
            0.0,
            "case {case:?}: this row exists for a description sharing no word with the \
             mention, got {description:?}"
        );
        let reported = score(linked, "Score");
        assert!(
            reported > 0.0 && reported <= confidence * LINKED_CANDIDATE_FLOOR + TOLERANCE,
            "case {case:?}: a linked candidate whose description names the mention in other \
             words is still reported, at most at the linked share of the confidence \
             {confidence}, got {reported} in {span}"
        );
    }
}

/// One completion row: the operation, the text, the level the span is reported at, the
/// span as written, the member its candidate list is carried under, the code the ontology
/// data LINKS that form to, the codes the completion has to reach for it, and how many
/// candidates the list carries at least.
type CompletionRow = (
    &'static str,
    &'static str,
    Level,
    &'static str,
    &'static str,
    &'static str,
    &'static [&'static str],
    usize,
);

/// The share of the words `written` is written in that `description` repeats: how much of
/// the written FORM a candidate's own description names, whatever else it spells out
/// beside it.
fn words_covered(written: &str, description: &str) -> f64 {
    let written = concept_words(written);
    let description = concept_words(description);
    if written.is_empty() {
        return 0.0;
    }
    written.intersection(&description).count() as f64 / written.len() as f64
}

/// Every span an ontology-linking operation LINKS reports as many ranked candidates as the
/// ontology this service carries can rank for its own written form — one rule for an
/// entity and for one of its attributes alike — and the concepts the data linked still
/// lead the list.
///
/// A candidate list is what these operations exist to produce ("the concepts that the
/// entity could refer to, along with a score indicating the likelihood of the match", "the
/// concepts specific to an attribute"), so a client pages it to disambiguate a mention: a
/// list cut off at the one concept a lexicon row happens to name leaves it with nothing to
/// choose between. The rows span all three ontologies, both levels, and families whose
/// authored list is one, two and three concepts deep, so a handler that reported only the
/// codes its own row names — or that let the completion displace them from the head of the
/// list — fails here.
#[tokio::test]
async fn a_linked_span_reports_every_candidate_the_ontology_can_rank_for_it() {
    let router = router();
    // Sources: the ICD-10-CM tabular list (Centers for Disease Control) and the SNOMED CT
    // US edition, as this crate's ontology data carries them, plus the RxNorm ingredient
    // concepts.
    let rows: &[CompletionRow] = &[
        (
            // The authored family names the three congestive forms; the ontology also
            // holds the acute one, which names two of the three words of the mention.
            "InferSNOMEDCT",
            "Assessment: congestive heart failure.",
            Level::Entity,
            "congestive heart failure",
            "SNOMEDCTConcepts",
            "42343007",
            &["56675007"],
            4,
        ),
        (
            // A family of ONE concept: everything after it is the ontology's own.
            "InferSNOMEDCT",
            "History of coronary artery disease.",
            Level::Entity,
            "coronary artery disease",
            "SNOMEDCTConcepts",
            "53741008",
            &["232717009"],
            2,
        ),
        (
            // An attribute is completed by the same rule: the qualifier value its own form
            // names leads, and the disorders that name that form follow.
            "InferSNOMEDCT",
            "Patient has type 2 diabetes mellitus.",
            Level::Attribute,
            "type 2",
            "SNOMEDCTConcepts",
            "258195006",
            &["44054006"],
            2,
        ),
        (
            "InferICD10CM",
            "Assessment: chronic kidney disease.",
            Level::Entity,
            "chronic kidney disease",
            "ICD10CMConcepts",
            "N18.9",
            &["E11.22"],
            3,
        ),
        (
            "InferICD10CM",
            "Patient had a stroke.",
            Level::Entity,
            "stroke",
            "ICD10CMConcepts",
            "I63.9",
            &["I64"],
            2,
        ),
        (
            // A family the data already ranks several codes for keeps exactly its own
            // ranking: the completion fills what the links leave unanswered, nothing else.
            "InferICD10CM",
            "Known atrial fibrillation.",
            Level::Entity,
            "atrial fibrillation",
            "ICD10CMConcepts",
            "I48.91",
            &[],
            3,
        ),
        (
            "InferRxNorm",
            "He takes insulin glargine 20 units nightly.",
            Level::Entity,
            "insulin glargine",
            "RxNormConcepts",
            "274783",
            &[],
            2,
        ),
    ];

    for (operation, text, level, written, member, leading, completed, least) in rows {
        let case = format!("{operation} {text:?} {written:?}");
        let answer = infer(&router, operation, text).await;
        let span = level_span(&case, &answer, *level, written);
        let list = candidates(&case, &span, member);
        assert_ranked_candidates(&case, &span, &list, *least);
        let codes: Vec<&str> = list
            .iter()
            .map(|concept| concept["Code"].as_str().unwrap_or_default())
            .collect();
        assert_eq!(
            codes.first().copied(),
            Some(*leading),
            "case {case:?}: the concept the ontology data links the form to leads the list, \
             got {codes:?}"
        );
        let unique: BTreeSet<&str> = codes.iter().copied().collect();
        assert_eq!(
            unique.len(),
            codes.len(),
            "case {case:?}: every candidate is reported once, got {codes:?}"
        );
        for reached in *completed {
            let rank = codes.iter().position(|code| code == reached);
            assert!(
                rank.is_some_and(|rank| rank > 0),
                "case {case:?}: the ontology holds {reached} for this form, so the completion \
                 reports it BELOW the linked candidates, got {codes:?}"
            );
            let description = list[rank.unwrap_or_default()]["Description"]
                .as_str()
                .unwrap_or_default();
            let covered = words_covered(written, description);
            assert!(
                covered > 0.5,
                "case {case:?}: a completed candidate names the form it is reported on, got \
                 {description:?} repeating {covered} of the words of {written:?}"
            );
        }
    }
}

/// One naming-bar row: the operation, the text, the span as written, the member its
/// candidate list is carried under, how many leading candidates the ontology data LINKS
/// that form to, and the codes a word-for-word search would have reached — each shares a
/// word with the form and names none of it.
type NamingRow = (
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    usize,
    &'static [&'static str],
);

/// The completion reaches only the concepts that NAME the span: a concept of the ontology
/// sharing a word every clinical description carries ("disease", "heart", "diabetic") is
/// no candidate for the mention, and a form the ontology holds no such neighbour of is
/// reported with the candidates the data links it to and nothing else.
///
/// The other side of the rule above: without it a word-for-word search fills every list to
/// five with whatever row shares a common word, which is a candidate list a client cannot
/// use. The rows name, per ontology, the concept such a search would have reached and the
/// list it would have been reached for, plus a brand name whose linked concepts repeat no
/// word of it at all — the naming bar governs the completion, never a link. A form the
/// ontology holds no naming neighbour of (SNOMED CT "hyperlipidemia", say) therefore
/// reports the concepts the data links it to and nothing else.
#[tokio::test]
async fn the_completion_reaches_only_a_concept_that_names_the_span() {
    let router = router();
    let rows: &[NamingRow] = &[
        (
            // "Chronic kidney disease, unspecified" and "Hypertensive heart disease with
            // heart failure" share only "disease" with this mention.
            "InferICD10CM",
            "History of coronary artery disease.",
            "coronary artery disease",
            "ICD10CMConcepts",
            1,
            &["N18.9", "I11.0"],
        ),
        (
            // "Heart structure (body structure)" shares only "heart".
            "InferSNOMEDCT",
            "Assessment: congestive heart failure.",
            "congestive heart failure",
            "SNOMEDCTConcepts",
            3,
            &["80891009"],
        ),
        (
            // "Diabetic retinopathy associated with type 2 diabetes mellitus (disorder)"
            // shares only "diabetic".
            "InferSNOMEDCT",
            "Admitted with diabetic ketoacidosis.",
            "diabetic ketoacidosis",
            "SNOMEDCTConcepts",
            1,
            &["422034002"],
        ),
        (
            // A brand name is LINKED to its brand concept and to the ingredient it
            // delivers, neither of which repeats a word of it: a linked candidate is
            // reported whatever its own description says, so the naming bar applies to
            // the completion and to nothing else.
            "InferRxNorm",
            "He takes Tylenol 500 mg as needed.",
            "Tylenol",
            "RxNormConcepts",
            2,
            &[],
        ),
    ];

    for (operation, text, written, member, linked, unreachable) in rows {
        let case = format!("{operation} {text:?} {written:?}");
        let answer = infer(&router, operation, text).await;
        let span = level_span(&case, &answer, Level::Entity, written);
        let list = candidates(&case, &span, member);
        assert_ranked_candidates(&case, &span, &list, 1);
        let codes: Vec<&str> = list
            .iter()
            .map(|concept| concept["Code"].as_str().unwrap_or_default())
            .collect();
        // The leading candidates are the ones the ontology data LINKS the form to, which
        // it reports whatever their own description says; every candidate after them is
        // there because it NAMES the form.
        for concept in list.iter().skip(*linked) {
            let description = concept["Description"].as_str().unwrap_or_default();
            let covered = words_covered(written, description);
            assert!(
                covered > 0.5,
                "case {case:?}: {description:?} names {covered} of the words of {written:?}, \
                 so it is no candidate of it, got {codes:?}"
            );
        }
        for reached in *unreachable {
            assert!(
                !codes.contains(reached),
                "case {case:?}: {reached} shares a word with the form and names none of it, \
                 so the completion does not reach it, got {codes:?}"
            );
        }
    }
}

/// Of two candidates the ontology was SEARCHED for, the one whose description names the
/// span more fully is ranked ahead — an ordering, asserted without restating what any
/// score is, so a handler that ranked the searched candidates by anything but how well
/// they name the span (the order its tables happen to declare them in, for instance) fails
/// here.
///
/// The rows are the attribute forms the ontology data holds several neighbours for: a
/// subtype qualifier, where one neighbour repeats both of the form's words and the other
/// only one, and an acuity modifier, where both repeat its one word but one spells out
/// less beside it. Sources: the qualifier-value, disorder and body-structure hierarchies of
/// the SNOMED CT US edition, as this crate's ontology data carries them.
#[tokio::test]
async fn a_searched_candidate_that_names_the_span_more_fully_is_ranked_ahead() {
    let router = router();
    // (text, the attribute as written, the nearer neighbour, the further one)
    let rows: &[(&str, &str, &str, &str)] = &[
        (
            // "Diabetes mellitus type 2 (disorder)" repeats both "type" and "2";
            // "Type 1 (qualifier value)" repeats only "type".
            "Patient has type 2 diabetes mellitus.",
            "type 2",
            "44054006",
            "258194007",
        ),
        (
            // Both name "chronic"; "Chronic kidney disease (disorder)" spells out less
            // beside it than "Chronic obstructive lung disease (disorder)".
            "Chronic cough with wheezing.",
            "Chronic",
            "709044004",
            "13645005",
        ),
    ];

    for (text, written, nearer, further) in rows {
        let case = format!("InferSNOMEDCT {text:?} {written:?}");
        let answer = infer(&router, "InferSNOMEDCT", text).await;
        let span = level_span(&case, &answer, Level::Attribute, written);
        let list = candidates(&case, &span, "SNOMEDCTConcepts");
        let rank = |code: &str| {
            list.iter()
                .position(|candidate| candidate["Code"].as_str() == Some(code))
        };
        let (near, far) = (rank(nearer), rank(further));
        assert!(
            near.is_some_and(|near| far.is_none_or(|far| near < far)),
            "case {case:?}: {nearer} names {written:?} more fully than {further}, so it is \
             ranked ahead of it, got {nearer} at {near:?} and {further} at {far:?} in {span}"
        );
    }
}
