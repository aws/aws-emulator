Issue-Source: trace-sweep-divergence
Issue-Trace: comprehend-batch-detect-suite
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: comprehend-batch-detect-suite
Issue-Shape-Id: com.amazonaws.comprehend#Comprehend_20171127
Issue-Key: comprehend|BatchDetectEntities|2xx->2xx|$.ResultList[*].Entities[*].Score
Issue-Commit: 0fd0c8a38859f6782ef39e69b19293525b727c9f

# Comprehend BatchDetectEntities emits uncalibrated per-entity confidence Scores

> _Note: this issue was identified against the code base at commit `0fd0c8a38859f6782ef39e69b19293525b727c9f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchDetectEntities

## Description
ONE root cause, in the comprehend crate's synchronous entity-detection scorer that backs BatchDetectEntities. The step answers HTTP 200 as recorded and every entity's Type, Text, BeginOffset and EndOffset now match the recording; what still diverges is Entity.Score, which the emulator synthesizes rather than deriving from the detection, so the per-entity confidences do not match AWS's and — more importantly — do not even preserve AWS's relative ordering between two entities of the same document.

NOTE on prior art: four earlier issues on this exact step are RESOLVED (20261030T053310Z-comprehend-batchdetectentities-truncates-entity-spans-and-emits-bucketed.md, 20261103T080324Z-comprehend-batchdetectentities-scores-detections-from-a-shared-constant.md, 20261108T024409Z-comprehend-batchdetectentities-returns-uncalibrated-per-entity-confidence-scores.md, 20261119T064554Z-comprehend-batchdetectentities-emits-uncalibrated-per-entity-confidence-scores.md), and the open issue 20261030T173304Z-comprehend-batch-and-targeted-inference-re-segment-text.md covers the OTHER four steps of this fixture (task#1, task#3, task#4, task#5) but NOT task#2 — replay reports task#2 as owned by no open issue. The span/offset half of the original defect is genuinely fixed; the scoring half still reproduces, so it is filed net-new (effectively a reopen of the scoring cause only).

Cause 1 of 1: BatchDetectEntities returns per-entity Scores that are not the detector's calibrated confidence and that invert AWS's confidence ordering within a document. Fixture comprehend-batch-detect-suite, step task#2.
  AWS: Smithy: BatchDetectEntities(TextList required @length min 1 and sensitive, LanguageCode required) -> ResultList: ListOfDetectEntitiesResult of BatchDetectEntitiesItemResult{Index ("the zero-based index of the document in the input list"), Entities: ListOfEntities}, plus ErrorList; errors BatchSizeLimitExceededException/TextSizeLimitExceededException/InvalidRequestException/UnsupportedLanguageException (400) and InternalServerException (500). Entity.Score is documented as "The level of confidence that Amazon Comprehend has in the accuracy of the detection" — i.e. a per-detection value, one per entity, derived from the evidence for THAT entity. Recorded behavior: within a single document the entities carry distinct confidences whose ordering reflects how unambiguous each mention is; a strongly evidenced mention outranks a weaker one, and the values are not drawn from a shared table.
  Emulator: it returns a different confidence for each detected entity of the diverging document, and the two values are transposed relative to AWS: the entity AWS scores LOWER comes back HIGHER and vice versa. The returned numbers are close to 1.0 for both, which is what a score assigned by the response builder (per shape/entity-type) rather than computed by the detector looks like — the magnitudes do not track the detection and the within-document ranking is lost.
  Why: Entity.Score is the only signal a client has for deciding whether to accept a detection. Clients threshold on it (drop entities below a cutoff), rank on it (pick the most confident ORGANIZATION/PERSON mention of a document), and compare it across entities of the same document. A score that is manufactured instead of derived makes the threshold meaningless and, because the ordering is inverted, makes "most confident entity in this document" return the wrong mention — a client built against AWS silently selects a different entity from the emulator than from the real service.
  Fix: the BatchDetectEntities handler's entity scorer (the component that fills Entity.Score for each ListOfEntities member) — compute one confidence per detection from that detection's evidence so that distinct entities get distinct, ordered values, instead of assigning the field from a fixed/per-type constant in the serializer. Note the recording shows this operation and DetectEntities sharing a scorer, so the fix belongs in the shared synchronous entity-detection path, not only in the batch fan-out.
  Traces: comprehend-batch-detect-suite#task#2.

The recorded numbers below are this recording's values for its own input text. The requirement is the convention — one calibrated confidence per detected entity, distinct across entities and ordered by the strength of each detection — not these literals. Status (200) and the absence of any error are unchanged and correct.

## Evidence
Fixture comprehend-batch-detect-suite, step task#2, operation BatchDetectEntities: HTTP 200 expected, HTTP 200 returned. No error code and no error message are involved; the divergence is entirely in the response body, confined to Entity.Score of one document's entities:

  $.ResultList[2].Entities[0].Score: expected 0.9917884469032288, got 0.9938162565231323
  $.ResultList[2].Entities[1].Score: expected 0.9983819723129272, got 0.9962049126625061

Note the inversion: AWS scores Entities[1] ABOVE Entities[0] (0.99838 > 0.99179), while the emulator scores Entities[0] above Entities[1] (0.99382 > 0.99620 is false — 0.99382 < 0.99620... the emulator's pair is ordered 0.99382 then 0.99620, i.e. it raises the entity AWS ranked lower and lowers the entity AWS ranked higher, so the gap AWS reports between the two detections is not reproduced and the two values straddle each other's recorded positions).

Every other asserted field of this step matches: all Entity.Type, Entity.Text, Entity.BeginOffset and Entity.EndOffset values across all three ResultList entries, the ResultList length, the per-document Index values and the empty ErrorList all agree with the recording. No other key of task#2 is reported as diverging, which is what isolates the defect to the score computation rather than to detection or segmentation.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (replay_wire_corpus). Step comprehend-batch-detect-suite#task#2 is neither blocked by a stub nor owned by any open issue; the other four steps of the fixture are owned by 20261030T173304Z-comprehend-batch-and-targeted-inference-re-segment-text.md.
2. Issue BatchDetectEntities with LanguageCode 'en' and the fixture's three-document TextList (the third document containing two clearly named entity mentions of different types).
3. Inspect $.ResultList[2].Entities[*].Score. Both entities' confidences differ from the recorded ones, and their relative order does not match the recording: the mention AWS reports with the higher confidence is returned with the lower one.
4. Compare the Score values against $.ResultList[0] and $.ResultList[1] of the same response and against the DetectEntities step of comprehend-detect-suite-single-document: the magnitudes cluster near 1.0 independent of how strongly each mention is evidenced, showing the value is produced by the response path rather than by the detector.
5. Confirm the rest of the step is clean — Type, Text, BeginOffset, EndOffset, Index and ErrorList all match — so only the scorer needs to change.
