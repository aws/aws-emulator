Issue-Source: trace-sweep-divergence
Issue-Trace: comprehendmedical-detect-phi-note
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: comprehendmedical-detect-phi-note comprehendmedical-infer-icd10cm-conditions comprehendmedical-infer-rxnorm-medications comprehendmedical-infer-snomedct-concepts
Issue-Shape-Id: com.amazonaws.comprehendmedical#ComprehendMedical_20181030
Issue-Key: comprehendmedical|DetectPHI|2xx->2xx|$.Entities[*].Score
Issue-Key: comprehendmedical|InferICD10CM|2xx->2xx|$.Entities[*]
Issue-Key: comprehendmedical|InferICD10CM|2xx->2xx|$.Entities[*].Attributes[*].RelationshipScore
Issue-Key: comprehendmedical|InferICD10CM|2xx->2xx|$.Entities[*].Attributes[*].Score
Issue-Key: comprehendmedical|InferICD10CM|2xx->2xx|$.Entities[*].ICD10CMConcepts[*].Code
Issue-Key: comprehendmedical|InferICD10CM|2xx->2xx|$.Entities[*].ICD10CMConcepts[*].Description
Issue-Key: comprehendmedical|InferICD10CM|2xx->2xx|$.Entities[*].ICD10CMConcepts[*].Score
Issue-Key: comprehendmedical|InferICD10CM|2xx->2xx|$.Entities[*].Score
Issue-Key: comprehendmedical|InferICD10CM|2xx->2xx|$.Entities[*].Traits[*].Score
Issue-Key: comprehendmedical|InferICD10CM|2xx->2xx|$.ModelVersion
Issue-Key: comprehendmedical|InferRxNorm|2xx->2xx|$.Entities[*]
Issue-Key: comprehendmedical|InferRxNorm|2xx->2xx|$.ModelVersion
Issue-Key: comprehendmedical|InferSNOMEDCT|2xx->2xx|$.Entities[*]
Issue-Key: comprehendmedical|InferSNOMEDCT|2xx->2xx|$.Entities[*].Attributes[*]
Issue-Key: comprehendmedical|InferSNOMEDCT|2xx->2xx|$.Entities[*].Attributes[*].RelationshipScore
Issue-Key: comprehendmedical|InferSNOMEDCT|2xx->2xx|$.Entities[*].Attributes[*].SNOMEDCTConcepts
Issue-Key: comprehendmedical|InferSNOMEDCT|2xx->2xx|$.Entities[*].Attributes[*].SNOMEDCTConcepts[*].Code
Issue-Key: comprehendmedical|InferSNOMEDCT|2xx->2xx|$.Entities[*].Attributes[*].SNOMEDCTConcepts[*].Description
Issue-Key: comprehendmedical|InferSNOMEDCT|2xx->2xx|$.Entities[*].Attributes[*].SNOMEDCTConcepts[*].Score
Issue-Key: comprehendmedical|InferSNOMEDCT|2xx->2xx|$.Entities[*].Attributes[*].Score
Issue-Key: comprehendmedical|InferSNOMEDCT|2xx->2xx|$.Entities[*].Category
Issue-Key: comprehendmedical|InferSNOMEDCT|2xx->2xx|$.Entities[*].EndOffset
Issue-Key: comprehendmedical|InferSNOMEDCT|2xx->2xx|$.Entities[*].SNOMEDCTConcepts[*]
Issue-Key: comprehendmedical|InferSNOMEDCT|2xx->2xx|$.Entities[*].SNOMEDCTConcepts[*].Code
Issue-Key: comprehendmedical|InferSNOMEDCT|2xx->2xx|$.Entities[*].SNOMEDCTConcepts[*].Description
Issue-Key: comprehendmedical|InferSNOMEDCT|2xx->2xx|$.Entities[*].SNOMEDCTConcepts[*].Score
Issue-Key: comprehendmedical|InferSNOMEDCT|2xx->2xx|$.Entities[*].Score
Issue-Key: comprehendmedical|InferSNOMEDCT|2xx->2xx|$.Entities[*].Text
Issue-Key: comprehendmedical|InferSNOMEDCT|2xx->2xx|$.Entities[*].Traits[*]
Issue-Key: comprehendmedical|InferSNOMEDCT|2xx->2xx|$.Entities[*].Traits[*].Score
Issue-Key: comprehendmedical|InferSNOMEDCT|2xx->2xx|$.Entities[*].Type
Issue-Key: comprehendmedical|InferSNOMEDCT|2xx->2xx|$.ModelVersion
Issue-Key: comprehendmedical|InferSNOMEDCT|2xx->2xx|$.SNOMEDCTDetails.VersionDate
Issue-Commit: 1ea1d7aa4460bb0d617c8232f75fea135d2cee77

# ComprehendMedical synchronous inference returns rule-engine scores and wrong ontology links instead of modeled inference output (4 causes)

> _Note: this issue was identified against the code base at commit `1ea1d7aa4460bb0d617c8232f75fea135d2cee77`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DetectPHI

## Description
All four of ComprehendMedical's synchronous text-inference operations now answer HTTP 200, but the body they serve is produced by an in-process rule/lexicon engine with hand-weighted confidences and a hand-written ontology table (see the handler module docs, e.g. `//! IMPLEMENTED. ... the engine here is rule- and lexicon-based` in src/ops/detect_phi.rs, which states it reuses `crate::ops::infer_icd10_cm`'s tokenizer, spans, lexicon matching and confidence weights). For the same input text the recorded AWS responses carry different per-entity confidences, a different ranked list of ontology concepts (different Code/Description at the same list position), a different set of entities, and a different model/ontology version. The shared fault is that the response content is synthesized by that local engine rather than reproducing the recorded inference output for the recorded inputs, so every field downstream of the engine (Score, RelationshipScore, concept Code/Description/Score, entity membership and ordering, ModelVersion, SNOMEDCTDetails.VersionDate) diverges. Four causes, one per operation.

Cause 1 of 4: DetectPHI per-entity confidence scores.
AWS: Smithy: DetectPHI -> DetectPHIResponse{Entities: EntityList (required), ModelVersion: String (required, @length min 1)}; Entity.Score is documented as "The level of confidence that Amazon Comprehend Medical has in the accuracy of the detection." The model does not fix numeric values, so: recorded behavior only — for the recorded clinical note AWS returns six PHI entities whose Score values are the deployed model's confidences (all above 0.96, e.g. the first six entities' scores in the fixture), at the entity ordering and offsets the recording shows. Status 200, no error.
Emulator: status 200, same entity count and offsets, but every Score is a locally computed weight: $.Entities[0].Score expected 0.999757707118988, got 0.9359999895095825; [1] expected 0.999953031539917, got 0.9813600182533264; [2] expected 0.999990701675415, got 0.975600004196167; [3] expected 0.981614887714386, got 0.9575999975204468; [4] expected 0.9972621202468872, got 0.975600004196167; [5] expected 0.9676140546798706, got 0.9868896007537842. The repeated 0.975600004196167 for two different entities shows the values come from a small fixed weight table, not from inference.
Why: a client that thresholds on Score (the documented use of the field — redact above a confidence, route for human review below it) takes different decisions against the emulator than against AWS; a de-identification pipeline tuned at e.g. 0.99 drops entities the real service would have accepted, and no two entities in AWS output share a weight-table constant.
Fix: the DetectPHI handler's response construction and the confidence weighting of the shared clinical-text engine it borrows (services/service-com-amazonaws-comprehendmedical-comprehendmedical-20181030/src/ops/detect_phi.rs and the scoring it reuses from src/ops/infer_icd10_cm.rs).
Traces: comprehendmedical-detect-phi-note#task#1.

Cause 2 of 4: InferICD10CM entity set, ICD-10-CM concept ranking and ModelVersion.
AWS: Smithy: InferICD10CM -> InferICD10CMResponse{Entities: ICD10CMEntityList (required), ModelVersion: String}; ICD10CMConcept.Code is "The ICD-10-CM code that identifies the concept found in the knowledge base from the Centers for Disease Control", Description is "The long description of the ICD-10-CM code in the ontology", Score is "The level of confidence that Amazon Comprehend Medical has that the entity is accurately linked to an ICD-10-CM concept". Recorded behavior only for the values: for the recorded note AWS returns three entities, each with up to five ICD-10-CM concepts in DESCENDING Score order drawn from the CDC ontology, plus the deployed ModelVersion. Status 200.
Emulator: status 200 but a different answer throughout — $.Entities[2] is absent from the emulator body entirely; the concept lists are ranked differently and name different codes at the same position ($.Entities[1].ICD10CMConcepts[3].Code expected 'I13.0' ("Hypertensive heart and chronic kidney disease with heart failure and stage 1 through stage 4 chronic kidney disease, or unspecified chronic kidney disease"), got 'I11.9' ("Hypertensive heart disease without heart failure"); [4].Code expected 'I12.0', got 'I15.9'); and every Score/RelationshipScore differs (e.g. $.Entities[0].Score expected 0.877546489238739, got 0.972000002861023; $.Entities[0].Attributes[0].RelationshipScore expected 0.6795811653137207, got 0.949999988079071; $.Entities[1].ICD10CMConcepts[0].Score expected 0.6778631210327148, got 0.41999998688697815). $.ModelVersion also diverges.
Why: the response is a clinical coding result. A client that takes the top-ranked Code to bill or chart a diagnosis records a different ICD-10-CM code against the emulator than against AWS, and an entity the real service reports is simply missing, so code-completeness checks pass locally and fail in production.
Fix: the InferICD10CM handler's entity extraction, its ontology-linking/ranking table and the ModelVersion it reports (services/service-com-amazonaws-comprehendmedical-comprehendmedical-20181030/src/ops/infer_icd10_cm.rs).
Traces: comprehendmedical-infer-icd10cm-conditions#task#1.

Cause 3 of 4: InferRxNorm returns different medication entities and no RxNorm concept links.
AWS: Smithy: InferRxNorm -> InferRxNormResponse{Entities: RxNormEntityList (required), ModelVersion: String}; RxNormConcept.Code is the "RxNorm concept ID, also known as the RxCUI", with Description and a linkage Score. Recorded behavior only for values: for the recorded medication text AWS returns each MEDICATION entity with its attributes (DOSAGE/ROUTE_OR_MODE/FREQUENCY) AND a non-empty RxNormConcepts list ranked by Score (e.g. RxCUI '861007' for the generic name at the top, then lower-scored alternatives). Status 200.
Emulator: status 200, but $.Entities[0] and $.Entities[1] do not match the recorded entities at all — the emulator's entity object carries NO RxNormConcepts member (the whole ontology linkage, the point of the operation, is absent), uses a different Id numbering and different Score/RelationshipScore values (entity Score 0.8399999737739563 vs the recorded 0.821601390838623; attribute RelationshipScores 0.949999988079071/0.7899999618530273/0.7099999785423279 vs the recorded 0.8813977241516113/0.8672217726707458/0.8672217726707458). $.ModelVersion also diverges.
Why: InferRxNorm exists to map free-text medications onto RxCUIs. A client that reads RxNormConcepts to normalize a medication list gets an empty/missing list from the emulator and cannot distinguish that from "no concept matched", so medication reconciliation code is exercised on a path AWS never returns.
Fix: the InferRxNorm handler's entity construction and its RxNorm concept linking (services/service-com-amazonaws-comprehendmedical-comprehendmedical-20181030/src/ops/infer_rx_norm.rs).
Traces: comprehendmedical-infer-rxnorm-medications#task#1.

Cause 4 of 4: InferSNOMEDCT entity set, SNOMED-CT concept ranking, ModelVersion and SNOMEDCTDetails.VersionDate.
AWS: Smithy: InferSNOMEDCT -> InferSNOMEDCTResponse{Entities: SNOMEDCTEntityList (required), ModelVersion: String, SNOMEDCTDetails, Characters}; SNOMEDCTConcept.Code is "The numeric ID for the SNOMED-CT concept" with its Description and linkage Score, and SNOMEDCTDetails reports "the SNOMED-CT edition, language, and version date" of the ontology actually used. Recorded behavior only for values: AWS returns six entities for the recorded note, each (and each attribute) carrying a five-deep SNOMED-CT concept list in descending Score order, and a VersionDate matching the US-edition release in use. Status 200.
Emulator: status 200 with a different body: $.Entities[4] and $.Entities[5] are missing; $.Entities[3] differs in Text, Type, Category, EndOffset, Traits and Attributes; attribute-level concept lists are absent where AWS has them ($.Entities[2].Attributes[0].SNOMEDCTConcepts, $.Entities[2].Attributes[1].SNOMEDCTConcepts) and truncated elsewhere ($.Entities[2].SNOMEDCTConcepts[1..4], $.Entities[3].SNOMEDCTConcepts[1..4] missing); the concepts that are present name different codes at the same position (e.g. $.Entities[0].Attributes[0].SNOMEDCTConcepts[2].Code expected '258199000' ("Type 3 (qualifier value)"), got '237599002' ("Insulin treated type 2 diabetes mellitus (disorder)"); [3].Code expected '410657003' ("Type (attribute)"), got '422034002'; [4].Code expected '405751000' ("Diabetes type (observable entity)"), got '258194007'); and every Score/RelationshipScore differs. $.ModelVersion and $.SNOMEDCTDetails.VersionDate also diverge.
Why: a client linking clinical findings to SNOMED-CT gets different concept IDs, fewer entities and no attribute-level linkage than AWS returns, and the reported ontology VersionDate does not describe the concepts actually served, so a consumer cannot tell which SNOMED-CT release the codes came from.
Fix: the InferSNOMEDCT handler's entity extraction, its SNOMED-CT concept linking (including attribute-level concepts) and the ModelVersion / SNOMEDCTDetails it reports (services/service-com-amazonaws-comprehendmedical-comprehendmedical-20181030/src/ops/infer_snomedct.rs).
Traces: comprehendmedical-infer-snomedct-concepts#task#1.

Related open issue, which does NOT cover these: `20261022T000000Z-stub-text-inference-service-com-amazonaws-comprehendmedical-comprehendmedical-20181030.md` states these six synchronous operations answer HTTP 501 NotImplemented and explicitly rejects a hand-written term table with digest-derived scores. The code has since drifted: the replay shows all four of these operations answering 200 from exactly such a rule/lexicon engine. That issue describes a 501; these are 200-with-wrong-content divergences and are not owned by it (the replay reports owned_by=[] and stub_path="" for all four).

## Evidence
Replay of simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up=true, bootstrap_signal=false): 4 divergence clusters, all service=comprehendmedical, all verdict=mismatch, expected_status=200 / actual_status=200, stub_path empty, owned_by empty.

1) comprehendmedical-detect-phi-note#task#1 (DetectPHI): $.Entities[0..5].Score all mismatch — expected 0.999757707118988 / 0.999953031539917 / 0.999990701675415 / 0.981614887714386 / 0.9972621202468872 / 0.9676140546798706; got 0.9359999895095825 / 0.9813600182533264 / 0.975600004196167 / 0.9575999975204468 / 0.975600004196167 / 0.9868896007537842.

2) comprehendmedical-infer-icd10cm-conditions#task#1 (InferICD10CM): divergent keys $.Entities[0].Score, $.Entities[0].Traits[0].Score, $.Entities[0].Attributes[0].{Score,RelationshipScore}, $.Entities[0].ICD10CMConcepts[0..4].Score, $.Entities[1].Score, $.Entities[1].Traits[0].Score, $.Entities[1].ICD10CMConcepts[0..4].{Score,Code,Description}, $.Entities[2] (missing from emulator), $.ModelVersion. Examples: Entities[0].Score expected 0.877546489238739, got 0.972000002861023; Entities[1].ICD10CMConcepts[3].Code expected 'I13.0', got 'I11.9'; [3].Description expected 'Hypertensive heart and chronic kidney disease with heart failure and stage 1 through stage 4 chronic kidney disease, or unspecified chronic kidney disease', got 'Hypertensive heart disease without heart failure'; [4].Code expected 'I12.0', got 'I15.9'.

3) comprehendmedical-infer-rxnorm-medications#task#1 (InferRxNorm): divergent keys $.Entities[0], $.Entities[1], $.ModelVersion. Expected Entities[0] = MEDICATION/GENERIC_NAME 'metformin' (Id 13, offsets 188-197, Score 0.821601390838623) with three attributes (Ids 14-16, RelationshipScores 0.8813977241516113 / 0.8672217726707458 / 0.8672217726707458) AND RxNormConcepts [861007 'metformin hydrochloride 500 mg oral tablet' 0.3430459499359131, 860975, 6809, 1807917, 861819]; got Id 1, Score 0.8399999737739563, attributes Ids 2-4 with RelationshipScores 0.949999988079071 / 0.7899999618530273 / 0.7099999785423279, and NO RxNormConcepts member at all.

4) comprehendmedical-infer-snomedct-concepts#task#1 (InferSNOMEDCT): 70+ divergent keys, including $.Entities[4] and $.Entities[5] missing; $.Entities[3].{Text,Type,Category,EndOffset,Score,Traits[0],Attributes[0..2]}; $.Entities[2].Attributes[0].SNOMEDCTConcepts and $.Entities[2].Attributes[1].SNOMEDCTConcepts missing; $.Entities[2].SNOMEDCTConcepts[1..4] and $.Entities[3].SNOMEDCTConcepts[1..4] missing; concept code/description swaps such as Entities[0].Attributes[0].SNOMEDCTConcepts[2].Code expected '258199000' ('Type 3 (qualifier value)') got '237599002' ('Insulin treated type 2 diabetes mellitus (disorder)'), [3].Code expected '410657003' ('Type (attribute)') got '422034002', [4].Code expected '405751000' ('Diabetes type (observable entity)') got '258194007'; plus every Score/RelationshipScore, $.ModelVersion and $.SNOMEDCTDetails.VersionDate.

Model evidence (smithy_query on services/service-com-amazonaws-comprehendmedical-comprehendmedical-20181030/smithy-model/com-amazonaws-comprehendmedical-comprehendmedical-20181030.json): DetectPHIResponse.ModelVersion required @length min 1; InferICD10CMResponse/InferRxNormResponse/InferSNOMEDCTResponse carry required Entities plus ModelVersion (and SNOMEDCTDetails for SNOMED-CT); ICD10CMConcept/RxNormConcept/SNOMEDCTConcept each document Code, Description and a linkage Score; SNOMEDCTDetails documents the edition, language and VersionDate of the ontology used.

Source evidence: services/service-com-amazonaws-comprehendmedical-comprehendmedical-20181030/src/ops/detect_phi.rs lines 5-21 declare the operation IMPLEMENTED over a "rule- and lexicon-based" engine reusing infer_icd10_cm's "tokenizer, spans, lexicon matching, confidence weights and character offsets" — the mechanism that produces the diverging scores and concept rankings.

## Steps to Reproduce
1. Clone this branch and boot the emulator built from it (the data listener answers; /__admin/health returns 200).
2. Replay the simparity-wire-aws-v1 wire corpus, or issue the four calls directly with an AWS SDK/CLI comprehendmedical client pointed at the emulator:
   a. `aws comprehendmedical detect-phi --text <the clinical note from fixture comprehendmedical-detect-phi-note>` and compare $.Entities[*].Score with the recorded response (step task#1).
   b. `aws comprehendmedical infer-icd10cm --text <the note from comprehendmedical-infer-icd10cm-conditions>` and compare the entity count, each ICD10CMConcepts list (Code/Description/Score order) and $.ModelVersion (step task#1).
   c. `aws comprehendmedical infer-rx-norm --text <the note from comprehendmedical-infer-rxnorm-medications>` and observe that the returned entities carry no RxNormConcepts member and different Ids/Scores (step task#1).
   d. `aws comprehendmedical infer-snomedct --text <the note from comprehendmedical-infer-snomedct-concepts>` and observe the missing entities 4-5, the missing attribute-level SNOMEDCTConcepts, the truncated concept lists, the swapped concept codes and the diverging $.ModelVersion / $.SNOMEDCTDetails.VersionDate (step task#1).
3. Each call returns HTTP 200, so the divergence is only visible by diffing the body against the recorded AWS response.

## Resolution

What the rule behind this report asks for is served; what it asks for beyond a rule is
recorded here as declined, so a later reader does not read this file as "the four fixtures
match".

Fixed. EVERY span an ontology-linking operation links now reports as many ranked candidates
as the ontology this service carries can rank for its own written form — one rule for an
entity, for one of its attributes and for a medication mention alike — instead of the
candidates its own lexicon row happened to name. The concepts the ontology data links the
form to still lead the list, in the order the data ranks them; the completion only ranks
below them, and reaches only a concept whose own description NAMES that form (more than
half of the words it is written in), so a row sharing a word every clinical description
carries ("disease", "chronic", "structure") is no candidate. The completion lives once, in
the shared clinical-text pass, and all three linking passes run it: `ICD10CMConcepts`,
`RxNormConcepts`, `SNOMEDCTEntity$SNOMEDCTConcepts` and
`SNOMEDCTAttribute$SNOMEDCTConcepts`. Pinned by the two new rows-driven tests in
`tests/all/ontology_candidate_ranking.rs`
(`a_linked_span_reports_every_candidate_the_ontology_can_rank_for_it`,
`the_completion_reaches_only_a_concept_that_names_the_span`).

Declined, with the reason each cannot be derived:

* Every `Score` / `RelationshipScore` of the four operations (causes 1-4), `DetectPHI`
  `$.Entities[*].Score` included — the per-span floats of a trained model. The spans, types,
  categories, offsets, ids, candidate ordering and list bounds agree; each confidence is read
  off the evidence that placed the span and how fully its form is written, and each candidate's
  off how well that candidate's own description names the form. No modelled or documented rule
  fixes any of these floats, and the replay withholds the recorded ones, so the only way to
  report them is to fit one recording.
* The MEMBERSHIP of a candidate list and the entities a recording carries that the emulator
  does not (`$.Entities[*]`, `ICD10CMConcepts[*].Code`/`.Description`,
  `SNOMEDCTConcepts[*].Code`/`.Description`, the RxCUIs of a medication) — the content of the
  CDC ICD-10-CM, NLM RxNorm and SNOMED CT US Edition releases and of AWS's trained entity
  model. This pass ranks the ontology data it carries; adding a recorded code to a row would
  be a table entry fitted to one recorded input.
* An attribute whose written form the ontology data holds no concept for still reports no
  concept list: the completion answers with the neighbours of a form the ontology NAMES, and
  inventing a code for a form it does not hold is the one thing a concept list must never do.
* `$.ModelVersion` and `$.SNOMEDCTDetails.VersionDate` — AWS's internal model release and
  ontology revision strings, which the replay withholds and no model doc determines.
  `Edition` "US" and `Language` "en" are the modelled values and agree.
* The `Id` numbering of `InferRxNorm` — `Entity$Id`/`Attribute$Id` is modelled as "a
  monotonically increasing id unique within this response rather than a global unique
  identifier", which 1..n over the spans the response reports satisfies.
