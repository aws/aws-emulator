Issue-Source: trace-sweep-divergence
Issue-Trace: comprehend-batch-detect-suite
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: comprehend-batch-detect-suite comprehend-detect-entities-bytes-without-endpoint comprehend-detect-suite-single-document
Issue-Shape-Id: com.amazonaws.comprehend#Comprehend_20171127
Issue-Key: comprehend|BatchDetectEntities|2xx->2xx|$.ResultList[*].Entities[*].Score
Issue-Key: comprehend|DetectEntities|2xx->2xx|$.Entities[*].Score
Issue-Key: comprehend|DetectDominantLanguage|2xx->2xx|$.Languages[*].Score
Issue-Key: comprehend|DetectPiiEntities|2xx->2xx|$.Entities[*].Score
Issue-Key: comprehend|DetectSentiment|2xx->2xx|$.SentimentScore.Mixed
Issue-Key: comprehend|DetectSentiment|2xx->2xx|$.SentimentScore.Negative
Issue-Key: comprehend|DetectSentiment|2xx->2xx|$.SentimentScore.Neutral
Issue-Key: comprehend|DetectSentiment|2xx->2xx|$.SentimentScore.Positive
Issue-Key: comprehend|DetectToxicContent|2xx->2xx|$.ResultList[*].Labels[*].Score
Issue-Key: comprehend|DetectToxicContent|2xx->2xx|$.ResultList[*].Toxicity
Issue-Commit: 1a8d6da9d066bb6b81849373d117fc388454286a

# Comprehend inference confidence scores are drawn from a shared constant pool instead of computed per document

> _Note: this issue was identified against the code base at commit `1a8d6da9d066bb6b81849373d117fc388454286a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchDetectEntities

## Description
SEVEN net-new root causes in the comprehend crate's synchronous inference layer. Every one of these steps answered HTTP 200 exactly as recorded and every modelled output member is present, so the divergence is entirely in the CONFIDENCE VALUES: the emulator fills Score / SentimentScore / Toxicity from a small pool of literals that is shared across operations and documents, instead of deriving a confidence from the analysis of the submitted text. The clearest proof of a shared pool is that the SAME literal shows up under unrelated operations on unrelated inputs: 0.9962049126625061 is returned both for BatchDetectEntities $.ResultList[2].Entities[1].Score and for DetectEntities $.Entities[1].Score on a different fixture, and 0.8286985754966736 / 0.9967453479766846 are returned by DetectEntities AND by DetectPiiEntities on the same document. A fixer must close ALL SEVEN causes (7 distinct fixture steps, listed per cause and in Issue-Trace-Ids) and re-replay each of them, not only the representative.

PRIOR ART (named, NOT covering): 20261127T063924Z-comprehend-batchdetect-confidence-scores-are-synthesized-per-shape.md is OPEN and owns the BatchDetect replay keys for BatchDetectDominantLanguage/BatchDetectKeyPhrases/BatchDetectSentiment/BatchDetectSyntax on fixture comprehend-batch-detect-suite, and 20261126T023639Z-comprehend-single-document-detectsyntax-and-detecttargetedsentiment-mislabel-tokens.md is OPEN on DetectSyntax task#6 of comprehend-detect-suite-single-document. Replay reports stub_path EMPTY and owned_by EMPTY for all seven steps below, i.e. no open issue carries their replay key.

General requirement for every cause: AWS returns, for each detected unit, ONE confidence that belongs to THAT unit in THAT document, and the components of a SentimentScore form one normalised distribution over that document. The recorded floats are this recording's values for ITS OWN input and are NOT themselves the requirement; the requirement is that the value is computed from the submitted text and is not a literal reused across units, documents or operations.

Cause 1 of 7: BatchDetectEntities emits per-entity confidences from the shared pool rather than from the batch member being analysed. Fixture comprehend-batch-detect-suite, step task#2.
  AWS: Smithy: BatchDetectEntities(TextList required CustomerInputStringList @length min 1, LanguageCode required LanguageCode) -> ResultList: ListOfDetectEntitiesResult of BatchDetectEntitiesItemResult{Index, Entities: ListOfEntities of Entity{Score 'The level of confidence that Amazon Comprehend has in the accuracy of the detection', Type EntityType [PERSON, LOCATION, ORGANIZATION, COMMERCIAL_ITEM, EVENT, DATE, QUANTITY, TITLE, OTHER], Text, BeginOffset, EndOffset}}, ErrorList required. One Score per detected entity, derived from that entity mention in that batch member.
  Emulator: $.ResultList[2].Entities[0].Score comes back 0.9938162565231323 where the recording has 0.9917884469032288, and $.ResultList[2].Entities[1].Score comes back 0.9962049126625061 where the recording has 0.9983819723129272 — and 0.9962049126625061 is the identical literal the single-document DetectEntities path returns for an unrelated entity on fixture comprehend-detect-entities-bytes-without-endpoint (Cause 2), which no per-document computation would produce.
  Why: a batch client thresholds entity acceptance on Score ('keep entities above 0.995'). A pooled confidence accepts and rejects a different set of entities than AWS for the same TextList, and because the same literal recurs across batch members the client can no longer rank members by detection quality.
  Fix: the BatchDetectEntities handler's per-member inference path in the comprehend crate — compute Entity.Score for each mention from that mention in that batch member, not from a shared literal table.
  Traces: comprehend-batch-detect-suite#task#2.

Cause 2 of 7: single-document DetectEntities emits per-entity confidences from the same pool. Fixture comprehend-detect-entities-bytes-without-endpoint, step task#1.
  AWS: Smithy: DetectEntities(Text CustomerInputString @length min 1 sensitive, LanguageCode, EndpointArn, Bytes, DocumentReaderConfig) -> Entities: ListOfEntities of Entity{Score, Type, Text, BeginOffset, EndOffset}. Each Score is the confidence for that entity mention in the submitted Text.
  Emulator: three of three entity Scores diverge — 0.9952885508537292 against recorded 0.999344766139984, 0.9962049126625061 against recorded 0.998554527759552 (the literal also used by Cause 1 on a different fixture), and 0.7154213190078735 against recorded 0.9968968629837036. The last is the worst: a date mention AWS reports at ~0.997 is reported at ~0.715, i.e. an order-of-magnitude different error rate.
  Why: a client that drops entities below a fixed confidence (commonly 0.9 or 0.95) silently loses the DATE entity the emulator scores 0.715 but AWS scores 0.997, so the extracted entity set for the same text differs from production.
  Fix: the DetectEntities handler's scoring path in the comprehend crate — derive Entity.Score from the matched span in the submitted Text.
  Traces: comprehend-detect-entities-bytes-without-endpoint#task#1.

Cause 3 of 7: DetectDominantLanguage emits a pooled language confidence. Fixture comprehend-detect-suite-single-document, step task#1.
  AWS: Smithy: DetectDominantLanguage(Text required) -> Languages: ListOfDominantLanguages of DominantLanguage{LanguageCode, Score 'The level of confidence that Amazon Comprehend has in the accuracy of the detection'}. Score reflects how strongly the submitted text identifies that language.
  Emulator: $.Languages[0].Score comes back 0.994535505771637 where the recording has 0.9877879619598388 — the detected LanguageCode matches, only the confidence is substituted.
  Why: language routers gate on this Score ('if below 0.98, treat the document as mixed-language and skip translation'). A substituted confidence moves documents across that gate in the opposite direction from AWS.
  Fix: the DetectDominantLanguage handler's scoring path in the comprehend crate.
  Traces: comprehend-detect-suite-single-document#task#1.

Cause 4 of 7: single-document DetectEntities on the multi-entity document returns six pooled scores, two of which are shared verbatim with DetectPiiEntities on the SAME document. Fixture comprehend-detect-suite-single-document, step task#2.
  AWS: Smithy: DetectEntities -> Entities[*].Score, one confidence per mention; AWS records 1.0 only for a mention it is fully certain of and otherwise distinct per-mention values.
  Emulator: all six Scores diverge: 0.9956942796707153 vs recorded 0.9613922834396362; 0.9977195262908936 vs recorded 1.0 (the emulator never reproduces the certain mention); 0.8286985754966736 vs recorded 0.998832941055298; 0.8121159076690674 vs recorded 0.990321397781372; 0.9967453479766846 vs recorded 0.9989029169082642; 0.9985878467559814 vs recorded 0.992952287197113. Two of those literals (0.8286985754966736, 0.9967453479766846) are returned again by DetectPiiEntities on this same document (Cause 5) for DIFFERENT spans.
  Why: two mentions AWS reports above 0.99 are reported at 0.83 and 0.81, so any client filter in the 0.9-0.99 band drops entities AWS keeps; and because the pool is shared with the PII path, the two operations' confidences are correlated in a way production never exhibits.
  Fix: the DetectEntities handler's scoring path in the comprehend crate — one score per mention from that mention, and no pool shared with the PII handler.
  Traces: comprehend-detect-suite-single-document#task#2.

Cause 5 of 7: DetectPiiEntities emits confidences from the entity pool. Fixture comprehend-detect-suite-single-document, step task#4.
  AWS: Smithy: DetectPiiEntities(Text required, LanguageCode required) -> Entities: ListOfPiiEntities of PiiEntity{Score 'The level of confidence that Amazon Comprehend has in the accuracy of the detection', Type PiiEntityType, BeginOffset, EndOffset}. Score is the confidence for that PII span.
  Emulator: all three Scores diverge — 0.8286985754966736 vs recorded 0.9992809891700744, 0.9967453479766846 vs recorded 0.9519889950752258, 0.9975816011428833 vs recorded 0.9999270439147948. The first two are the literals DetectEntities returns for unrelated spans on the same document (Cause 4), confirming one shared pool across the two operations.
  Why: PII redaction pipelines redact above a configured confidence. A span AWS reports at 0.9993 arriving at 0.8287 is NOT redacted by a client whose threshold sits at 0.9, so the emulator causes a redaction policy to under-redact relative to AWS — the highest-impact consequence of the pooled scores.
  Fix: the DetectPiiEntities handler's scoring path in the comprehend crate.
  Traces: comprehend-detect-suite-single-document#task#4.

Cause 6 of 7: DetectSentiment returns a SentimentScore distribution that is not computed from the document. Fixture comprehend-detect-suite-single-document, step task#5.
  AWS: Smithy: DetectSentiment(Text required, LanguageCode required) -> Sentiment (SentimentType), SentimentScore{Positive, Negative, Neutral, Mixed} 'describes the level of confidence that Amazon Comprehend has in the accuracy of its detection of sentiments' — four floats forming one normalised distribution over the submitted Text, with the member matching Sentiment dominant.
  Emulator: all four members diverge: Mixed 1.4154457161907885e-08 vs recorded 7.401431048492668e-06; Negative 6.342651613522321e-05 vs recorded 6.294498598435894e-05; Neutral 0.9335358142852783 vs recorded 0.9914856553077698; Positive 0.06640077382326126 vs recorded 0.008444002829492092. The emulator's Positive is ~8x the recorded value and its Neutral ~6 points lower, i.e. the distribution's shape — not just its precision — differs.
  Why: clients rarely branch on Sentiment alone; they branch on the margin ('NEUTRAL but Positive>0.05 -> route to review'). AWS's 0.0084 keeps this document out of review, the emulator's 0.0664 routes it in, so the same text takes a different path.
  Fix: the DetectSentiment handler's scoring path in the comprehend crate — compute a normalised four-way distribution from the submitted Text rather than selecting a stored distribution.
  Traces: comprehend-detect-suite-single-document#task#5.

Cause 7 of 7: DetectToxicContent returns a constant per-label score and an uncorrelated overall Toxicity. Fixture comprehend-detect-suite-single-document, step task#8.
  AWS: Smithy: DetectToxicContent(TextSegments required ListOfTextSegments @length min 1 sensitive, LanguageCode required) -> ResultList: ListOfToxicLabels of ToxicLabels{Labels: ListOfToxicContent of ToxicContent{Name ToxicContentType [GRAPHIC, HARASSMENT_OR_ABUSE, HATE_SPEECH, INSULT, PROFANITY, SEXUAL, VIOLENCE_OR_THREAT], Score}, Toxicity 'Overall toxicity score for the content'}. Each of the seven labels carries its OWN score for that segment and Toxicity is the overall score for the same segment.
  Emulator: four of the seven label Scores come back as the SINGLE literal 0.0005714285653084517 (for labels recorded at 0.018200000748038292, 0.023900000378489494, 0.024800000712275505 and 0.0198999997228384), the remaining three come back 4.889975389232859e-05 / 5.037783193984069e-05 / 3.696857675095089e-05 against recorded 0.01860000006854534 / 0.06069999933242798 / 0.01080000028014183, and $.ResultList[0].Toxicity comes back 0.002419769763946533 against recorded 0.02260000072419643. Every value is one to three orders of magnitude below the recording and four of them are literally identical, so the labels are not being scored independently.
  Why: moderation clients compare each label against its own threshold and the overall Toxicity against a policy threshold (e.g. 0.01 or 0.02). AWS's recorded scores sit at or above such thresholds while the emulator's sit far below, so content AWS flags for review passes clean through the emulator — and because four labels share one value, a client cannot tell which category triggered.
  Fix: the DetectToxicContent handler's scoring path in the comprehend crate — score each ToxicContentType label independently for the submitted segment and derive Toxicity from those label scores.
  Traces: comprehend-detect-suite-single-document#task#8.

## Evidence
All seven steps: expected HTTP 200, got HTTP 200; response shape and all modelled members present; replay verdict 'mismatch', stub_path EMPTY, owned_by EMPTY for each. The divergence is confined to confidence members.

Cause 1 - comprehend-batch-detect-suite#task#2, BatchDetectEntities:
  $.ResultList[2].Entities[0].Score: expected 0.9917884469032288, got 0.9938162565231323
  $.ResultList[2].Entities[1].Score: expected 0.9983819723129272, got 0.9962049126625061

Cause 2 - comprehend-detect-entities-bytes-without-endpoint#task#1, DetectEntities:
  $.Entities[0].Score: expected 0.999344766139984, got 0.9952885508537292
  $.Entities[1].Score: expected 0.998554527759552, got 0.9962049126625061  <-- same literal as Cause 1
  $.Entities[2].Score: expected 0.9968968629837036, got 0.7154213190078735

Cause 3 - comprehend-detect-suite-single-document#task#1, DetectDominantLanguage:
  $.Languages[0].Score: expected 0.9877879619598388, got 0.994535505771637

Cause 4 - comprehend-detect-suite-single-document#task#2, DetectEntities:
  $.Entities[0].Score: expected 0.9613922834396362, got 0.9956942796707153
  $.Entities[1].Score: expected 1.0, got 0.9977195262908936
  $.Entities[2].Score: expected 0.998832941055298, got 0.8286985754966736
  $.Entities[3].Score: expected 0.990321397781372, got 0.8121159076690674
  $.Entities[4].Score: expected 0.9989029169082642, got 0.9967453479766846
  $.Entities[5].Score: expected 0.992952287197113, got 0.9985878467559814

Cause 5 - comprehend-detect-suite-single-document#task#4, DetectPiiEntities:
  $.Entities[0].Score: expected 0.9992809891700744, got 0.8286985754966736  <-- same literal as Cause 4 $.Entities[2]
  $.Entities[1].Score: expected 0.9519889950752258, got 0.9967453479766846  <-- same literal as Cause 4 $.Entities[4]
  $.Entities[2].Score: expected 0.9999270439147948, got 0.9975816011428833

Cause 6 - comprehend-detect-suite-single-document#task#5, DetectSentiment:
  $.SentimentScore.Mixed: expected 7.401431048492668e-06, got 1.4154457161907885e-08
  $.SentimentScore.Negative: expected 6.294498598435894e-05, got 6.342651613522321e-05
  $.SentimentScore.Neutral: expected 0.9914856553077698, got 0.9335358142852783
  $.SentimentScore.Positive: expected 0.008444002829492092, got 0.06640077382326126

Cause 7 - comprehend-detect-suite-single-document#task#8, DetectToxicContent:
  $.ResultList[0].Labels[0].Score: expected 0.018200000748038292, got 0.0005714285653084517
  $.ResultList[0].Labels[1].Score: expected 0.023900000378489494, got 0.0005714285653084517
  $.ResultList[0].Labels[2].Score: expected 0.024800000712275505, got 0.0005714285653084517
  $.ResultList[0].Labels[3].Score: expected 0.01860000006854534, got 4.889975389232859e-05
  $.ResultList[0].Labels[4].Score: expected 0.06069999933242798, got 5.037783193984069e-05
  $.ResultList[0].Labels[5].Score: expected 0.0198999997228384, got 0.0005714285653084517
  $.ResultList[0].Labels[6].Score: expected 0.01080000028014183, got 3.696857675095089e-05
  $.ResultList[0].Toxicity: expected 0.02260000072419643, got 0.002419769763946533
  (0.0005714285653084517 returned for four distinct labels.)

## Steps to Reproduce
1. Clone the repo, boot the emulator from the checkout and replay the wire corpus (replay_wire_corpus). Fixtures comprehend-batch-detect-suite, comprehend-detect-entities-bytes-without-endpoint and comprehend-detect-suite-single-document all replay with no blocked or errored step; replay reports stub_path EMPTY and owned_by EMPTY for the seven steps named below.
2. Cause 1: comprehend-batch-detect-suite step task#2 - BatchDetectEntities with the fixture's TextList/LanguageCode. Read $.ResultList[2].Entities[*].Score: 0.9938162565231323 / 0.9962049126625061 against recorded 0.9917884469032288 / 0.9983819723129272.
3. Cause 2: comprehend-detect-entities-bytes-without-endpoint step task#1 - DetectEntities with the fixture's Text. $.Entities[2].Score is 0.7154213190078735 against recorded 0.9968968629837036, and $.Entities[1].Score is 0.9962049126625061 - byte-identical to the value seen in step 2 on a different fixture. Two unrelated documents producing the same float is the shared-pool signature.
4. Cause 3: comprehend-detect-suite-single-document step task#1 - DetectDominantLanguage. LanguageCode matches; $.Languages[0].Score is 0.994535505771637 against recorded 0.9877879619598388.
5. Causes 4 and 5: comprehend-detect-suite-single-document steps task#2 (DetectEntities) and task#4 (DetectPiiEntities) on the same document. Collect both Score lists and intersect them: 0.8286985754966736 and 0.9967453479766846 appear in BOTH, attached to different spans - one pool serving two operations.
6. Cause 6: comprehend-detect-suite-single-document step task#5 - DetectSentiment. Sentiment matches, but $.SentimentScore.Positive is 0.06640077382326126 against recorded 0.008444002829492092 and Neutral 0.9335358142852783 against 0.9914856553077698.
7. Cause 7: comprehend-detect-suite-single-document step task#8 - DetectToxicContent. Read $.ResultList[0].Labels[*].Score: four of the seven are the identical literal 0.0005714285653084517, the rest are ~1e-05, and $.ResultList[0].Toxicity is 0.002419769763946533 against recorded 0.02260000072419643.
8. After the fix, re-replay ALL SEVEN steps - comprehend-batch-detect-suite#task#2, comprehend-detect-entities-bytes-without-endpoint#task#1, and comprehend-detect-suite-single-document#task#1, #task#2, #task#4, #task#5, #task#8 - not just the representative.
