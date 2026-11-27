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
Issue-Key: comprehendmedical|InferSNOMEDCT|2xx->2xx|$.Entities[*].Attributes[*].SNOMEDCTConcepts[*]
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
Issue-Commit: 7fb2ed4170396fe0a0c384ff71365204f905b96a

# ComprehendMedical rule-based inference engine diverges from recorded model output on all four synchronous inference operations (4 causes)

> _Note: this issue was identified against the code base at commit `7fb2ed4170396fe0a0c384ff71365204f905b96a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DetectPHI

## Description
All four synchronous ComprehendMedical text-inference operations now answer HTTP 200 (the 501 NotImplemented recorded by the open issue `20261022T000000Z-stub-text-inference-service-com-amazonaws-comprehendmedical-comprehendmedical-20181030.md` is gone), but the body they serve is produced by an in-process lexicon/rule engine whose confidence weights, entity segmentation and ontology concept tables are hand-written constants. The shared fault is that the response is NOT the output of the service's trained clinical-text model: the per-entity `Score`/`RelationshipScore` values are derived from fixed weights and token counts instead of from the analyzed text, the entity list is only as complete as the hand-written lexicon, the ranked ontology concept lists carry different codes and descriptions past the first one or two ranks, and `ModelVersion` is a hard-coded literal (`"1.1.0"`). That one engine backs all four operations, so one root cause shows up four times. These four causes are the four wire divergences the replay reported for this service; the open stub issue above describes the OPPOSITE behavior (a 501) and therefore does not cover any of them.

Cause 1 of 4: DetectPHI confidence scores are weight-table constants, not model output. Fixture `comprehendmedical-detect-phi-note`, step `task#1`.
  AWS: Smithy: DetectPHI -> DetectPHIResponse with required `Entities` (EntityList) and required `ModelVersion`; each `Entity.Score` is documented as "the level of confidence that Amazon Comprehend Medical has in the accuracy of the detection" — i.e. a probability the trained PHI model computes for that span of the submitted text. HTTP 200. Recorded behavior: for the submitted note the six detected PHI entities (spans, types and offsets) match, and every entity carries a model-computed score (the recording's six range from ~0.9676 to ~0.99999).
  Emulator: the same six entities come back with scores taken from the rule engine's fixed weights — repeated values such as 0.9756 appearing on two different entities, and 0.936/0.9814/0.9576/0.9869 on the others. Every one of `$.Entities[0..5].Score` diverges from the recorded value; status is 200 both ways.
  Why: a client that thresholds on `Score` (the documented use of the field — accept a redaction above a confidence floor, route the rest for review) partitions the same note differently against the emulator than against AWS, and a score that is a constant per entity-type cannot move with the evidence in the text, so no threshold behaves as it does in production.
  Fix: DetectPHI's response construction — the confidence computation feeding `Entity.Score` in the PHI detection handler, which reads the shared weight constants (`span_confidence` / `term_confidence` / `WEIGHT_*`) rather than scoring the span.
  Traces: comprehendmedical-detect-phi-note#task#1.

Cause 2 of 4: InferICD10CM drops an entity and ranks different ICD-10-CM concepts. Fixture `comprehendmedical-infer-icd10cm-conditions`, step `task#1`.
  AWS: Smithy: InferICD10CM -> InferICD10CMResponse with required `Entities` (ICD10CMEntityList) and `ModelVersion`; each entity carries `ICD10CMConcepts`, documented as the ICD-10-CM concepts the mention links to, each with a `Code`, a `Description` and a `Score`, ordered by descending score. HTTP 200. Recorded behavior: the note yields three entities, and each entity's concept list is a five-deep ranking over the real ICD-10-CM vocabulary (e.g. the hypertensive-disease entity ranks `I13.0` "Hypertensive heart and chronic kidney disease with heart failure and stage 1 through stage 4 chronic kidney disease, or unspecified chronic kidney disease" and `I12.0` at ranks 4 and 5).
  Emulator: `$.Entities[2]` is absent entirely (only two entities are reported), the concept codes and descriptions at the lower ranks are different entries (`I11.9` "Hypertensive heart disease without heart failure" and `I15.9` where the recording has `I13.0` and `I12.0`), every `Score`, `Traits[].Score` and `Attributes[].Score`/`RelationshipScore` diverges, and `$.ModelVersion` is the hard-coded `1.1.0`.
  Why: ICD-10-CM linking exists so a client can take the code: a missing entity means a diagnosis the client never sees, and a different code at the same rank means it bills/codes the encounter as a different condition. The hard-coded `ModelVersion` additionally defeats the documented purpose of that field (tracking which model analyzed a batch), since it never corresponds to the model that produced the answer.
  Fix: the InferICD10CM handler — its entity segmentation (why a third entity is never emitted), its ICD-10-CM concept table and ranking, and the `ModelVersion` constant in the response builder.
  Traces: comprehendmedical-infer-icd10cm-conditions#task#1.

Cause 3 of 4: InferRxNorm medication entities differ wholesale from the recorded ones. Fixture `comprehendmedical-infer-rxnorm-medications`, step `task#1`.
  AWS: Smithy: InferRxNorm -> InferRxNormResponse with required `Entities` (RxNormEntityList) and `ModelVersion`; each entity carries `Id`, `Text`, `Category`, `Type`, `Score`, offsets, `Attributes` (each with its own `Id`, `Score` and `RelationshipScore`) and `RxNormConcepts` (RxNorm `Code`/`Description`/`Score`, ranked). HTTP 200. Recorded behavior: the metformin mention comes back as one entity whose `Id` and whose attributes' `Id`s are drawn from a response-wide id space (13 for the entity, 14/15/16 for its DOSAGE/ROUTE_OR_MODE/FREQUENCY attributes) with model-computed scores (~0.8216 entity, ~0.8617 attributes) and a five-deep RxNorm ranking (`861007`, `860975`, `6809`, `1807917`, `861819`).
  Emulator: `$.Entities[0]` and `$.Entities[1]` both mismatch as whole objects — the same mention is reported with ids restarting at 1 (entity 1, attributes 2/3/4), with weight-table scores (0.84 entity; 0.966/0.90/0.936 attributes, `RelationshipScore` 0.95/0.79/0.71 straight off the distance-penalty formula) — and `$.ModelVersion` is again `1.1.0`.
  Why: a client that walks the attribute graph by `Id` (the documented way to tie a DOSAGE or FREQUENCY back to its medication) and that reads the top-ranked RxNorm code to normalize the prescription gets a different graph and different normalization than production; the scores again cannot be thresholded.
  Fix: the InferRxNorm handler — its medication lexicon and RxNorm concept table, its entity/attribute id assignment, its confidence computation, and the `ModelVersion` constant (see `services/service-com-amazonaws-comprehendmedical-comprehendmedical-20181030/src/ops/infer_rx_norm.rs`, `const MODEL_VERSION: &str = "1.1.0"` at line 129 and the score construction around lines 1019-1038).
  Traces: comprehendmedical-infer-rxnorm-medications#task#1.

Cause 4 of 4: InferSNOMEDCT loses entities and concepts and reports a different SNOMED CT version date. Fixture `comprehendmedical-infer-snomedct-concepts`, step `task#1`.
  AWS: Smithy: InferSNOMEDCT -> InferSNOMEDCTResponse with required `Entities` (SNOMEDCTEntityList), `ModelVersion`, `SNOMEDCTDetails` and `Characters`; each entity and each attribute carries `SNOMEDCTConcepts` (SNOMED CT `Code`/`Description`/`Score`, ranked), and `SNOMEDCTDetails` reports the edition/language/version date of the SNOMED CT release the linking used. HTTP 200. Recorded behavior: six entities, five-deep concept rankings on entities and on attributes (e.g. `258199000` "Type 3 (qualifier value)", `410657003` "Type (attribute)", `405751000` "Diabetes type (observable entity)" at the lower ranks, `422014003` "Disorder due to type 2 diabetes mellitus (disorder)"), and a `SNOMEDCTDetails.VersionDate` naming the release used.
  Emulator: `$.Entities[4]` and `$.Entities[5]` are absent, `$.Entities[3]` differs in `Category`, `Type`, `Text`, `EndOffset`, its traits, all three of its attributes and its whole concept list, several attributes' `SNOMEDCTConcepts` lists are empty or truncated (`$.Entities[0].Attributes[0].SNOMEDCTConcepts[4]` missing; `$.Entities[2].Attributes[*].SNOMEDCTConcepts` absent), the lower-ranked codes/descriptions are different entries (`237599002` "Insulin treated type 2 diabetes mellitus (disorder)" and `422034002` "Diabetic retinopathy associated with type 2 diabetes mellitus (disorder)" where the recording ranks `258199000`/`410657003`), every `Score` and `RelationshipScore` diverges, and both `$.ModelVersion` and `$.SNOMEDCTDetails.VersionDate` diverge from the recorded release metadata.
  Why: a client linking clinical text to SNOMED CT sees fewer findings than production, gets a different concept at the same rank for the ones it does see, and is told the linking came from a SNOMED CT release other than the one actually represented — so downstream terminology lookups and version pinning are both wrong.
  Fix: the InferSNOMEDCT handler — its entity segmentation, its SNOMED CT concept table and per-attribute linking, its confidence computation, and the `ModelVersion` / `SNOMEDCTDetails` constants in the response builder.
  Traces: comprehendmedical-infer-snomedct-concepts#task#1.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout: 4 divergence clusters, all service=comprehendmedical, all verdict=mismatch with expected_status 200 and actual_status 200 (no stub_path, no owned_by).

1) comprehendmedical-detect-phi-note#task#1, DetectPHI:
   $.Entities[0].Score expected 0.999757707118988, got 0.9359999895095825
   $.Entities[1].Score expected 0.999953031539917, got 0.9813600182533264
   $.Entities[2].Score expected 0.999990701675415, got 0.975600004196167
   $.Entities[3].Score expected 0.981614887714386, got 0.9575999975204468
   $.Entities[4].Score expected 0.9972621202468872, got 0.975600004196167
   $.Entities[5].Score expected 0.9676140546798706, got 0.9868896007537842

2) comprehendmedical-infer-icd10cm-conditions#task#1, InferICD10CM:
   $.Entities[2]: missing
   $.Entities[1].ICD10CMConcepts[3].Code expected 'I13.0', got 'I11.9'
   $.Entities[1].ICD10CMConcepts[3].Description expected 'Hypertensive heart and chronic kidney disease with heart failure and stage 1 through stage 4 chronic kidney disease, or unspecified chronic kidney disease', got 'Hypertensive heart disease without heart failure'
   $.Entities[1].ICD10CMConcepts[4].Code expected 'I12.0', got 'I15.9'
   plus every Score / Traits[].Score / Attributes[].Score / Attributes[].RelationshipScore on Entities[0..1], and $.ModelVersion

3) comprehendmedical-infer-rxnorm-medications#task#1, InferRxNorm:
   $.Entities[0] mismatched as a whole object — expected {Id: 13, Score: 0.821601390838623, Attributes: [{Id: 14, Type: DOSAGE, Score: 0.8617406487464905, RelationshipScore: 0.8813977241516113}, {Id: 15, ROUTE_OR_MODE, 0.8617406487464905, 0.8672217726707458}, {Id: 16, FREQUENCY, 0.8617406487464905, 0.8672217726707458}], RxNormConcepts: [861007, 860975, 6809, 1807917, 861819]}, got {Id: 1, Score: 0.8399999737739563, Attributes: [{Id: 2, DOSAGE, 0.9660000205039978, 0.949999988079071}, {Id: 3, ROUTE_OR_MODE, 0.8999999761581421, 0.7899999618530273}, {Id: 4, FREQUENCY, 0.9359999895095825, 0.7099999785423279}]}
   also $.Entities[1] and $.ModelVersion

4) comprehendmedical-infer-snomedct-concepts#task#1, InferSNOMEDCT:
   $.Entities[4], $.Entities[5] missing; $.Entities[3].{Category,Type,Text,EndOffset,Traits[0],Attributes[0..2],SNOMEDCTConcepts[0..4]} all mismatched
   $.Entities[0].Attributes[0].SNOMEDCTConcepts[2].Code expected '258199000' ('Type 3 (qualifier value)'), got '237599002' ('Insulin treated type 2 diabetes mellitus (disorder)')
   $.Entities[0].Attributes[0].SNOMEDCTConcepts[3].Code expected '410657003' ('Type (attribute)'), got '422034002'
   $.Entities[0].Attributes[0].SNOMEDCTConcepts[4] missing (expected 405751000 'Diabetes type (observable entity)')
   $.Entities[0].SNOMEDCTConcepts[2].Code expected '422014003' ('Disorder due to type 2 diabetes mellitus (disorder)'), got '237599002'
   $.Entities[2].Attributes[0..1].SNOMEDCTConcepts absent; plus $.ModelVersion and $.SNOMEDCTDetails.VersionDate

Source of the emulator values: services/service-com-amazonaws-comprehendmedical-comprehendmedical-20181030/src/ops/infer_rx_norm.rs line 129 (`const MODEL_VERSION: &str = "1.1.0"`), lines 1019-1038 (scores built from `span_confidence`/`term_confidence` and `WEIGHT_RELATIONSHIP - RELATIONSHIP_DISTANCE_PENALTY * distance`, floored at RELATIONSHIP_SCORE_FLOOR = 0.10), and the shared pass imported from src/ops/infer_icd10_cm.rs (WEIGHT_CUE / WEIGHT_MODIFIER / WEIGHT_PATTERN / WEIGHT_RELATIONSHIP, FIRST_ID).

## Steps to Reproduce
1. Build and start the emulator from this checkout; point an AWS SDK/CLI `comprehendmedical` client at it.
2. Replay the wire corpus (trace set simparity-wire-aws-v1); fixtures comprehendmedical-detect-phi-note, comprehendmedical-infer-icd10cm-conditions, comprehendmedical-infer-rxnorm-medications and comprehendmedical-infer-snomedct-concepts each have one step (task#1) that returns 200 with a diverging body.
3. Equivalently by hand: `aws comprehendmedical detect-phi --text '<the clinical note of the fixture>'` and compare `Entities[*].Score` against the recorded response — every score is one of the engine's weight constants rather than the recorded model probability.
4. `aws comprehendmedical infer-icd10cm --text '<the conditions note>'` -> only two entities instead of three, and ICD10CMConcepts ranks 4-5 carry different codes/descriptions.
5. `aws comprehendmedical infer-rx-norm --text '<the medications note>'` -> entity/attribute Ids restart at 1 and every score differs.
6. `aws comprehendmedical infer-snomedct --text '<the concepts note>'` -> four entities instead of six, attribute concept lists empty or short, and SNOMEDCTDetails.VersionDate different.
7. In every case `ModelVersion` is the literal `1.1.0`.
