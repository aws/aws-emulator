Issue-Source: trace-sweep-divergence
Issue-Trace: comprehend-detect-suite-single-document
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: comprehend-detect-suite-single-document
Issue-Shape-Id: com.amazonaws.comprehend#Comprehend_20171127
Issue-Key: comprehend|DetectDominantLanguage|2xx->2xx|$.Languages[*].Score
Issue-Key: comprehend|DetectEntities|2xx->2xx|$.Entities[*].Score
Issue-Key: comprehend|DetectPiiEntities|2xx->2xx|$.Entities[*].Score
Issue-Key: comprehend|DetectSentiment|2xx->2xx|$.SentimentScore.Mixed
Issue-Key: comprehend|DetectSentiment|2xx->2xx|$.SentimentScore.Negative
Issue-Key: comprehend|DetectSentiment|2xx->2xx|$.SentimentScore.Neutral
Issue-Key: comprehend|DetectSentiment|2xx->2xx|$.SentimentScore.Positive
Issue-Key: comprehend|DetectToxicContent|2xx->2xx|$.ResultList[*].Labels[*].Score
Issue-Key: comprehend|DetectToxicContent|2xx->2xx|$.ResultList[*].Toxicity
Issue-Commit: 7a03c6c633814b42753ce8c8f03d4e7bdff82ea0

# Comprehend single-document Detect* confidence scores are drawn from a shape-level constant ladder instead of the document (5 causes)

> _Note: this issue was identified against the code base at commit `7a03c6c633814b42753ce8c8f03d4e7bdff82ea0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DetectDominantLanguage

## Description
FIVE net-new root causes in the comprehend crate's SINGLE-DOCUMENT synchronous inference layer (the Detect* handlers exercised by fixture comprehend-detect-suite-single-document). Every one of the five steps answered HTTP 200 exactly as recorded and the response SHAPE is correct (the modelled output members are present, entity/label counts and most spans line up), so the divergence is entirely in the analysed confidence VALUES. One fault recurs through all five: the confidences are taken from a small fixed ladder of literals attached to the response shape rather than computed from the document under analysis, so (a) the same literal comes back for unrelated detections and even across different operations, (b) saturation is applied where AWS reports a middling confidence and vice versa, and (c) the SentimentScore/Toxicity distributions no longer describe the text. A fixer must close all FIVE causes.

PRIOR ART (named, not covering): 20261113T194011Z-comprehend-single-document-detect-inference-synthesizes-ladder-constant.md described this same fixture but is RESOLVED, so it covers nothing; 20261120T213134Z-comprehend-batchdetect-inference-emits-per-shape-constant-confidences.md and 20261120T175104Z-comprehend-batchdetectentities-emits-uncalibrated-per-entity-confidence-scores--r2.md are OPEN but own the BATCH fan-out fixture (comprehend-batch-detect-suite) only. Replay reports stub_path empty and owned_by EMPTY for all five steps below.

Cause 1 of 5: DetectDominantLanguage saturates the language confidence for a document AWS is only moderately confident about. Fixture comprehend-detect-suite-single-document, step task#1.
  AWS: Smithy: DetectDominantLanguage(Text required, CustomerInputString @length min 1, sensitive) -> Languages: ListOfDominantLanguages of DominantLanguage{LanguageCode (RFC 5646), Score 'The level of confidence that Amazon Comprehend has in the accuracy of the detection'}. Recorded behavior: the Score reflects how much evidence the submitted text gives for the detected language, so a document mixing languages, names and technical tokens comes back visibly below saturation.
  Emulator: $.Languages[0].Score is 0.9999989867210388 where 0.9877879619598388 is recorded — the value is pinned to the top of the ladder regardless of the text.
  Why: clients gate on Languages[0].Score ('route to translation only above 0.99', 'queue for human review below 0.99'). A constant at ~0.999999 makes the review branch unreachable, so the emulator and AWS take different paths on the same document.
  Fix: the DetectDominantLanguage handler's language-detection scorer in the comprehend crate — derive Score from the submitted text's own detection evidence instead of a shape-level constant.
  Traces: comprehend-detect-suite-single-document#task#1.

Cause 2 of 5: DetectEntities scores every entity off the same ladder, inverting AWS's confidence ordering and sharing literals with other operations. Fixture comprehend-detect-suite-single-document, step task#2.
  AWS: Smithy: DetectEntities(Text, LanguageCode) -> Entities: ListOfEntities of Entity{Score 'The level of confidence that Amazon Comprehend has in the accuracy of the detection', Type (EntityType), Text, BeginOffset, EndOffset}. Recorded behavior: each detection carries its own confidence computed from that span in that document; unambiguous entities reach 1.0 while weaker ones sit distinctly lower, and the values are not shared between detections.
  Emulator: all six entity Scores diverge and the ordering is not preserved: $.Entities[0].Score 0.9956942796707153 (recorded 0.9613922834396362), [1] 0.9977195262908936 (recorded 1.0), [2] 0.8286985754966736 (recorded 0.998832941055298), [3] 0.8121159076690674 (recorded 0.990321397781372), [4] 0.9967453479766846 (recorded 0.9989029169082642), [5] 0.9985878467559814 (recorded 0.992952287197113). The literals are not even private to this operation: 0.8286985754966736 is also returned by DetectPiiEntities (cause 3) and by DetectTargetedSentiment on this same fixture, and 0.9967453479766846 reappears as a ContainsPiiEntities label score.
  Why: clients filter entity output by Score before writing to an index or triggering an action. Demoting an entity AWS reports at 0.9988 to 0.83 drops it from any >0.9 filter, while promoting one AWS reports at 0.961 keeps a detection AWS's own ranking placed lower — the extracted entity set differs from AWS's for the same text.
  Fix: the DetectEntities handler's entity scorer — one confidence per detected span, computed from that span, never a value shared with another detection or another operation.
  Traces: comprehend-detect-suite-single-document#task#2.

Cause 3 of 5: DetectPiiEntities returns PII confidences from the same cross-operation constant pool. Fixture comprehend-detect-suite-single-document, step task#4.
  AWS: Smithy: DetectPiiEntities(Text required, LanguageCode required) -> Entities: ListOfPiiEntities of PiiEntity{Score 'The level of confidence that Amazon Comprehend has in the accuracy of the detection', Type (PiiEntityType), BeginOffset, EndOffset}. Recorded behavior: PII detections of structured shapes (an address, an e-mail, a name) are reported at or very near saturation, each with its own value.
  Emulator: $.Entities[0].Score 0.8286985754966736 (recorded 0.9992809891700744), [1] 0.9967453479766846 (recorded 0.9519889950752258), [2] 0.9975816011428833 (recorded 0.9999270439147948). The first value is the SAME literal DetectEntities returns for its third entity and the second/third are the same literals ContainsPiiEntities returns for its labels, i.e. the pool is shared across handlers.
  Why: PII redaction pipelines threshold on Score to decide what to mask. A detection AWS reports at 0.9993 arriving as 0.83 falls below a typical 0.9 redaction threshold, so text the emulator leaves in the clear is redacted against AWS.
  Fix: the DetectPiiEntities handler's PII scorer — per-detection confidence derived from the matched span, with no literal shared with DetectEntities/ContainsPiiEntities.
  Traces: comprehend-detect-suite-single-document#task#4.

Cause 4 of 5: DetectSentiment returns a distribution that is not the document's — the dominant member is diluted and the residual members collapse onto a floor constant and an exact zero. Fixture comprehend-detect-suite-single-document, step task#5.
  AWS: Smithy: DetectSentiment(Text required, LanguageCode required) -> {Sentiment (SentimentType), SentimentScore{Positive, Negative, Neutral, Mixed}}, each member documented 'The level of confidence that Amazon Comprehend has in the accuracy of its detection of the <SENTIMENT> sentiment'. Recorded behavior: the four members form one normalised distribution strongly concentrated on the reported Sentiment, with the residual members at small but MUTUALLY DISTINCT magnitudes spanning several orders (here 7.4e-06, 6.3e-05, 8.4e-03).
  Emulator: $.SentimentScore.Neutral 0.9335982203483582 (recorded 0.9914856553077698), Positive 0.06640077382326126 (recorded 0.008444002829492092) — nearly eight times the recorded mass leaking into the second sentiment — while Negative comes back as the floor literal 9.999999974752427e-07 (recorded 6.294498598435894e-05) and Mixed as exactly 0.0 (recorded 7.401431048492668e-06). The same floor literal and the same hard 0.0 are what the batch sentiment path emits, confirming a fixed table rather than a computed distribution.
  Why: clients branch on the distribution, not just on Sentiment ('ambiguous if a second sentiment exceeds 0.05', 'confident only above 0.99'). Pulling Neutral from 0.991 to 0.934 and pushing Positive from 0.008 to 0.066 flips a decisive verdict to ambiguous, and zeroing residual members destroys any log-scale comparison between documents.
  Fix: the DetectSentiment handler's sentiment scorer — emit one normalised distribution computed from the document and concentrated on the reported Sentiment, with distinct non-zero residual members, instead of filling members from a floor/ladder table.
  Traces: comprehend-detect-suite-single-document#task#5.

Cause 5 of 5: DetectToxicContent returns ONE identical literal for every toxicity label and for the overall Toxicity, so no content type can be distinguished. Fixture comprehend-detect-suite-single-document, step task#8.
  AWS: Smithy: DetectToxicContent(TextSegments required @length min 1, LanguageCode required) -> ResultList: ListOfToxicLabels of ToxicLabels{Labels: ListOfToxicContent of ToxicContent{Name (ToxicContentType), Score 'Model confidence in the detected content type. Value range is zero to one'}, Toxicity 'Overall toxicity score for the string. Value range is zero to one'}. Recorded behavior: each toxicity type gets its own score and the overall Toxicity is its own value, all differing per label (recorded here across 0.0108..0.0607 with Toxicity 0.0226).
  Emulator: all seven label Scores AND the overall Toxicity come back as the single literal 0.00147058826405555 — $.ResultList[0].Labels[0..6].Score and $.ResultList[0].Toxicity — against recorded 0.018200000748038292, 0.023900000378489494, 0.024800000712275505, 0.01860000006854534, 0.06069999933242798, 0.0198999997228384, 0.01080000028014183 and Toxicity 0.02260000072419643. The literal looks like a constant (1/680) independent of the segment.
  Why: a moderation client ranks the label array to report WHICH toxicity type fired and compares Toxicity against a policy threshold. With one shared constant, every label ties, the top-label choice is arbitrary, and the segment's overall toxicity carries no information — content AWS would surface at 0.0607 on one label is indistinguishable from the rest.
  Fix: the DetectToxicContent handler's toxicity scorer — one confidence per ToxicContentType computed from the segment, plus an overall Toxicity derived from them, instead of a single constant for the whole result.
  Traces: comprehend-detect-suite-single-document#task#8.

In all five causes the recorded numbers are this recording's values for ITS OWN input text and are NOT themselves the requirement. The requirement is the convention: one confidence per detection, computed from that detection in the submitted document; a normalised SentimentScore concentrated on the reported Sentiment with distinct residual members; per-label toxicity scores plus an overall Toxicity derived from them; and never a literal that recurs across unrelated detections or across operations.

## Evidence
All five steps: expected HTTP 200, got HTTP 200. Status and response shape match; the divergence is entirely in the body values. Fixture comprehend-detect-suite-single-document throughout. Replay verdict 'mismatch', stub_path empty, owned_by empty for each step.

Cause 1 - task#1, DetectDominantLanguage:
  $.Languages[0].Score: expected 0.9877879619598388, got 0.9999989867210388

Cause 2 - task#2, DetectEntities:
  $.Entities[0].Score: expected 0.9613922834396362, got 0.9956942796707153
  $.Entities[1].Score: expected 1.0, got 0.9977195262908936
  $.Entities[2].Score: expected 0.998832941055298, got 0.8286985754966736
  $.Entities[3].Score: expected 0.990321397781372, got 0.8121159076690674
  $.Entities[4].Score: expected 0.9989029169082642, got 0.9967453479766846
  $.Entities[5].Score: expected 0.992952287197113, got 0.9985878467559814

Cause 3 - task#4, DetectPiiEntities:
  $.Entities[0].Score: expected 0.9992809891700744, got 0.8286985754966736
  $.Entities[1].Score: expected 0.9519889950752258, got 0.9967453479766846
  $.Entities[2].Score: expected 0.9999270439147948, got 0.9975816011428833

Cause 4 - task#5, DetectSentiment:
  $.SentimentScore.Neutral: expected 0.9914856553077698, got 0.9335982203483582
  $.SentimentScore.Positive: expected 0.008444002829492092, got 0.06640077382326126
  $.SentimentScore.Negative: expected 6.294498598435894e-05, got 9.999999974752427e-07 (floor literal)
  $.SentimentScore.Mixed: expected 7.401431048492668e-06, got 0.0

Cause 5 - task#8, DetectToxicContent:
  $.ResultList[0].Labels[0].Score: expected 0.018200000748038292, got 0.00147058826405555
  $.ResultList[0].Labels[1].Score: expected 0.023900000378489494, got 0.00147058826405555
  $.ResultList[0].Labels[2].Score: expected 0.024800000712275505, got 0.00147058826405555
  $.ResultList[0].Labels[3].Score: expected 0.01860000006854534, got 0.00147058826405555
  $.ResultList[0].Labels[4].Score: expected 0.06069999933242798, got 0.00147058826405555
  $.ResultList[0].Labels[5].Score: expected 0.0198999997228384, got 0.00147058826405555
  $.ResultList[0].Labels[6].Score: expected 0.01080000028014183, got 0.00147058826405555
  $.ResultList[0].Toxicity: expected 0.02260000072419643, got the SAME 0.00147058826405555

Cross-cause evidence of one shared constant pool: 0.8286985754966736 is returned both for $.Entities[2].Score of DetectEntities (cause 2) and for $.Entities[0].Score of DetectPiiEntities (cause 3), and again for a DetectTargetedSentiment mention on this same fixture; 0.9967453479766846 is returned by DetectEntities[4], DetectPiiEntities[1] and as a ContainsPiiEntities label score; 0.9975816011428833 is returned by DetectPiiEntities[2] and ContainsPiiEntities[0]; 9.999999974752427e-07 is the single residual value used by both DetectSentiment and the batch sentiment path; 0.00147058826405555 covers eight distinct recorded toxicity values.

## Steps to Reproduce
1. Clone the repo, boot the emulator from the checkout and replay the wire corpus (replay_wire_corpus). Fixture comprehend-detect-suite-single-document replays with no blocked or errored step; replay reports stub_path and owned_by empty for the five steps below.
2. Cause 1: step task#1 - DetectDominantLanguage with the fixture's Text. Read $.Languages[0].Score: it comes back at ~0.999999 where the recording has 0.98779, i.e. pinned to the top of the ladder.
3. Cause 2: step task#2 - DetectEntities with the same Text. Read $.Entities[*].Score: all six differ and the relative ordering is inverted (the entity AWS reports at 0.9988 is returned at 0.8287; the one AWS reports at 0.9614 is returned at 0.9957).
4. Cause 3: step task#4 - DetectPiiEntities (Text + LanguageCode 'en'). Read $.Entities[*].Score and compare with step task#2 and step task#9: 0.8286985754966736, 0.9967453479766846 and 0.9975816011428833 are literals shared with the other handlers.
5. Cause 4: step task#5 - DetectSentiment (Text + LanguageCode 'en'). Read $.SentimentScore: Neutral is diluted to 0.9336 from 0.9915 with 0.0664 leaking into Positive, Negative is the floor literal 9.999999974752427e-07 and Mixed is exactly 0.0.
6. Cause 5: step task#8 - DetectToxicContent (TextSegments + LanguageCode 'en'). Read $.ResultList[0]: every Labels[*].Score and the overall Toxicity are the identical literal 0.00147058826405555.
7. Re-replay fixture comprehend-detect-suite-single-document after the fix: ALL FIVE steps (task#1, task#2, task#4, task#5, task#8) must match, not just the representative task#1.
