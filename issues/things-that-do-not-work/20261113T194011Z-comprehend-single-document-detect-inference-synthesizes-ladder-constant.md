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
Issue-Key: comprehend|DetectPiiEntities|2xx->2xx|$.Entities[*]
Issue-Key: comprehend|DetectPiiEntities|2xx->2xx|$.Entities[*].BeginOffset
Issue-Key: comprehend|DetectPiiEntities|2xx->2xx|$.Entities[*].EndOffset
Issue-Key: comprehend|DetectPiiEntities|2xx->2xx|$.Entities[*].Score
Issue-Key: comprehend|DetectPiiEntities|2xx->2xx|$.Entities[*].Type
Issue-Key: comprehend|DetectSentiment|2xx->2xx|$.SentimentScore.Mixed
Issue-Key: comprehend|DetectSentiment|2xx->2xx|$.SentimentScore.Negative
Issue-Key: comprehend|DetectSentiment|2xx->2xx|$.SentimentScore.Neutral
Issue-Key: comprehend|DetectSentiment|2xx->2xx|$.SentimentScore.Positive
Issue-Key: comprehend|DetectSyntax|2xx->2xx|$.SyntaxTokens[*]
Issue-Key: comprehend|DetectSyntax|2xx->2xx|$.SyntaxTokens[*].BeginOffset
Issue-Key: comprehend|DetectSyntax|2xx->2xx|$.SyntaxTokens[*].EndOffset
Issue-Key: comprehend|DetectSyntax|2xx->2xx|$.SyntaxTokens[*].PartOfSpeech.Score
Issue-Key: comprehend|DetectSyntax|2xx->2xx|$.SyntaxTokens[*].PartOfSpeech.Tag
Issue-Key: comprehend|DetectSyntax|2xx->2xx|$.SyntaxTokens[*].Text
Issue-Key: comprehend|DetectTargetedSentiment|2xx->2xx|$.Entities[*]
Issue-Key: comprehend|DetectTargetedSentiment|2xx->2xx|$.Entities[*].Mentions[*].BeginOffset
Issue-Key: comprehend|DetectTargetedSentiment|2xx->2xx|$.Entities[*].Mentions[*].EndOffset
Issue-Key: comprehend|DetectTargetedSentiment|2xx->2xx|$.Entities[*].Mentions[*].GroupScore
Issue-Key: comprehend|DetectTargetedSentiment|2xx->2xx|$.Entities[*].Mentions[*].MentionSentiment.SentimentScore.Negative
Issue-Key: comprehend|DetectTargetedSentiment|2xx->2xx|$.Entities[*].Mentions[*].MentionSentiment.SentimentScore.Neutral
Issue-Key: comprehend|DetectTargetedSentiment|2xx->2xx|$.Entities[*].Mentions[*].MentionSentiment.SentimentScore.Positive
Issue-Key: comprehend|DetectTargetedSentiment|2xx->2xx|$.Entities[*].Mentions[*].Score
Issue-Key: comprehend|DetectTargetedSentiment|2xx->2xx|$.Entities[*].Mentions[*].Text
Issue-Key: comprehend|DetectTargetedSentiment|2xx->2xx|$.Entities[*].Mentions[*].Type
Issue-Key: comprehend|DetectToxicContent|2xx->2xx|$.ResultList[*].Labels[*].Score
Issue-Key: comprehend|DetectToxicContent|2xx->2xx|$.ResultList[*].Toxicity
Issue-Key: comprehend|ContainsPiiEntities|2xx->2xx|$.Labels[*].Score
Issue-Related: 20261030T173304Z-comprehend-batch-and-targeted-inference-re-segment-text.md
Issue-Commit: f9b57e7c2082dec33445c051631ac259919daf74

# Comprehend single-document Detect* inference synthesizes ladder-constant confidences and mis-segments text (8 causes)

> _Note: this issue was identified against the code base at commit `f9b57e7c2082dec33445c051631ac259919daf74`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DetectDominantLanguage

## Description
EIGHT net-new root causes in the comprehend crate's single-document synchronous inference layer (the Detect*/ContainsPiiEntities handlers of fixture comprehend-detect-suite-single-document). Every one of the eight steps answered HTTP 200 exactly as recorded, so the status and the response SHAPE are right; the divergence is entirely in the analysed values. Two faults recur across the eight: (a) every confidence value is drawn from a small fixed ladder of constants belonging to the shape rather than computed per detection, so the same literal recurs across unrelated detections and across operations, and (b) the segmenters (entity spans, PII spans, tokenizer, mention/co-reference grouping) cut the input at the wrong boundaries, which shifts, mistypes or drops whole detections. A fixer must close all EIGHT causes.

PRIOR ART (named, not covering): 20261030T173304Z-comprehend-batch-and-targeted-inference-re-segment-text.md covers the BatchDetect* fan-out of fixture comprehend-batch-detect-suite and also claims DetectTargetedSentiment, and 20261108T024409Z-comprehend-batchdetectentities-returns-uncalibrated-per-entity-confidence-scores.md covers BatchDetectEntities. Replay reports NONE of the eight steps below as owned by an open issue (owned_by is empty for all eight, including task#7, whose current divergence is a different set of spans and types than that issue describes), so all eight are filed here net-new.

Cause 1 of 8: DetectDominantLanguage reports a constant confidence instead of one derived from the document's language evidence. Fixture comprehend-detect-suite-single-document, step task#1.
  AWS: Smithy: DetectDominantLanguage(Text required, CustomerInputString @length min 1) -> Languages: ListOfDominantLanguages of DominantLanguage{LanguageCode, Score}; DominantLanguage.Score is documented 'The level of confidence that Amazon Comprehend has in the accuracy of the detection.' Recorded behavior: the single detected language carries a calibrated confidence strictly below 1.0 that reflects how much evidence the document gives for that language.
  Emulator: it answers a round constant for the shape regardless of the text (0.9990000128746033 against a recorded 0.9877879619598388) — the same 0.999 family the batch path returns, i.e. a per-shape literal rather than a per-detection score.
  Why: clients gate on this Score to decide whether to trust the detected language before routing for translation or picking a downstream LanguageCode. A flat near-1.0 confidence makes the low-confidence branch unreachable, so a document whose language is genuinely uncertain is treated as certain.
  Fix: the DetectDominantLanguage handler's language detector — compute the confidence from the detection evidence for the document instead of returning a constant.
  Traces: comprehend-detect-suite-single-document#task#1.

Cause 2 of 8: DetectEntities assigns every entity a confidence from a fixed ladder, unrelated to the detection. Fixture comprehend-detect-suite-single-document, step task#2.
  AWS: Smithy: DetectEntities(Text, LanguageCode) -> Entities: ListOfEntities of Entity{Score, Type (EntityType: PERSON|LOCATION|ORGANIZATION|COMMERCIAL_ITEM|EVENT|DATE|QUANTITY|TITLE|OTHER), Text, BeginOffset, EndOffset}; Entity.Score is 'The level of confidence that Amazon Comprehend has in the accuracy of the detection.' Recorded behavior: each of the document's entities carries its own calibrated confidence, tightly clustered high (~0.96 to 1.0) for clearly named entities and distinct per entity.
  Emulator: the six entities come back with ladder values that do not track the detection (0.963302731513977, 0.9776536226272583, 0.8128078579902649, 0.7777777910232544, 0.9683794379234314, 0.9900744557380676) against recorded 0.9614 / 1.0 / 0.9988 / 0.9903 / 0.9989 / 0.9930 — including 0.7777777910232544 (a 7/9 ratio artefact) and 0.8128 for entities AWS detects with ~0.99 confidence. 0.963302731513977 is the same literal the BatchDetectEntities path emits, confirming a shared constant source rather than a scorer.
  Why: a client filters or ranks entities by Score ('only accept above 0.95', 'rank mentions by confidence'). Ladder constants drop high-confidence entities below the threshold and destroy any ordering, so the same document yields different downstream entity sets against the emulator and AWS.
  Fix: the DetectEntities handler's entity scorer — one confidence per detection, derived from the detection, not a table of per-shape constants.
  Traces: comprehend-detect-suite-single-document#task#2.

Cause 3 of 8: DetectPiiEntities misses one PII span entirely, mistypes the span it keeps, and returns one entity fewer than recorded. Fixture comprehend-detect-suite-single-document, step task#4.
  AWS: Smithy: DetectPiiEntities(Text required, LanguageCode required) -> Entities: ListOfPiiEntities of PiiEntity{Score, Type (PiiEntityType, which includes ADDRESS, EMAIL, NAME, PHONE, SSN, ...), BeginOffset ('zero-based offset from the beginning of the source text to the first character in the entity'), EndOffset}; Score is 'The level of confidence that Amazon Comprehend has in the accuracy of the detection.' Recorded behavior: AWS returns one PiiEntity per PII span in the document, each bounded exactly by that span, each typed with the PiiEntityType matching what the span actually is (a postal address span typed ADDRESS, an e-mail address span typed EMAIL), and each with its own high confidence.
  Emulator: it returns one entity FEWER than recorded. The address span is never detected: the slot that should hold the ADDRESS entity instead holds the EMAIL entity's span (Type expected 'ADDRESS' got 'EMAIL', BeginOffset expected 166 got 196, EndOffset expected 173 got 218), the trailing recorded element is absent ($.Entities[2]: missing), and the first entity's Score is the flat ladder value 0.75 where AWS records 0.9992809891700744.
  Why: PII detection is used to redact or to refuse: a span the emulator never reports is PII a client leaves unredacted, and a mistyped span routes to the wrong redaction policy. The 0.75 ladder score also sits below the common 0.9 acceptance threshold, so a correctly detected entity is discarded.
  Fix: the DetectPiiEntities handler's PII span detector and type classifier (detect every PII span and type it from PiiEntityType by what the span is) plus its per-entity confidence computation.
  Traces: comprehend-detect-suite-single-document#task#4.

Cause 4 of 8: DetectSentiment's SentimentScore vector is diluted and its non-dominant members are one repeated constant. Fixture comprehend-detect-suite-single-document, step task#5.
  AWS: Smithy: DetectSentiment(Text required, LanguageCode required) -> {Sentiment (SentimentType), SentimentScore{Positive, Negative, Neutral, Mixed}}; each SentimentScore member is 'The level of confidence that Amazon Comprehend has in the accuracy of its detection of the <SENTIMENT> sentiment.' Recorded behavior: the four values form one distribution that concentrates sharply on the reported Sentiment, with the remaining members pushed to small and MUTUALLY DISTINCT magnitudes (order 1e-06 to 1e-02 for a confident document).
  Emulator: the dominant member is diluted and mass leaks into a second sentiment (Neutral expected 0.9914856553077698, got 0.6069637537002563, with Positive expected 0.008444002829492092 returned as 0.3910442292690277), and the two non-dominant members are the SAME constant 0.0009960159659385681 for both Mixed and Negative — the identical literal this emulator emits on the BatchDetectSentiment and DetectTargetedSentiment paths.
  Why: a client thresholding sentiment ('act only above 0.95', 'treat a second sentiment above 0.1 as ambiguous') gets the opposite verdict: a clear-cut NEUTRAL document reads as borderline NEUTRAL/POSITIVE, and because the residual members are a shared constant no relative ordering between documents survives.
  Fix: the DetectSentiment handler's sentiment scorer — emit one normalised distribution per document concentrated on the returned Sentiment, rather than filling members from a fixed table.
  Traces: comprehend-detect-suite-single-document#task#5.

Cause 5 of 8: DetectSyntax tokenizes the document short, mis-tags the tail tokens, and scores every token off a fixed ladder. Fixture comprehend-detect-suite-single-document, step task#6.
  AWS: Smithy: DetectSyntax(Text required, LanguageCode: SyntaxLanguageCode required) -> SyntaxTokens: ListOfSyntaxTokens of SyntaxToken{TokenId, Text ('the word that was recognized in the source text'), BeginOffset, EndOffset, PartOfSpeech}; SyntaxToken is documented as 'There is one syntax token record for each word in the source text', and PartOfSpeechTag{Tag (PartOfSpeechTagType), Score ('the confidence that Amazon Comprehend has that the part of speech was correctly identified')}. Recorded behavior: one token per word of the whole document, each token's span bounded by that word, per-token Score at or near 1.0 and differing token by token.
  Emulator: the token list ends early — the recorded tail tokens are absent ($.SyntaxTokens[35] through $.SyntaxTokens[40] all 'missing') and the last tokens it does emit are shifted onto the wrong words ($.SyntaxTokens[33].Text and EndOffset, $.SyntaxTokens[34].BeginOffset/EndOffset diverge) — and the tags of the tokens around that boundary diverge too ($.SyntaxTokens[29|30|31].PartOfSpeech.Tag). Every PartOfSpeech.Score is drawn from the same recurring ladder (0.9999899864196777, 0.9999499917030334, 0.7499943971633911, 0.9090826511383057, 0.8333263993263245, 0.663931131362915, 0.7999935746192932) where AWS records 1.0 or ~0.99999 per token.
  Why: a client walks SyntaxTokens to extract phrases, strip function words or align annotations back onto the source by offset. Missing trailing tokens silently truncate the analysis of the document's last clause, shifted offsets mis-align every annotation after the boundary, and the recurring Score constants (several far below AWS's 1.0) make confidence filtering drop correctly tagged tokens.
  Fix: the DetectSyntax handler's tokenizer (one token per word across the whole input, with that word's offsets) and its per-token part-of-speech tagger/confidence computation.
  Traces: comprehend-detect-suite-single-document#task#6.

Cause 6 of 8: DetectTargetedSentiment splits named mentions, mistypes them, drops entity groups, and splits the mention sentiment distribution. Fixture comprehend-detect-suite-single-document, step task#7.
  AWS: Smithy: DetectTargetedSentiment(Text required, LanguageCode required) -> Entities: list of TargetedSentimentEntity{DescriptiveMentionIndex, Mentions}; TargetedSentimentMention carries Score, GroupScore, Text ('the text in the document that identifies the entity'), Type (TargetedSentimentEntityType, e.g. ORGANIZATION/PERSON/COMMERCIAL_ITEM), MentionSentiment{Sentiment, SentimentScore} and BeginOffset/EndOffset. Recorded behavior: each group's mention spans the FULL named mention at its true offsets, Type names the real entity class, and a factual mention is NEUTRAL with SentimentScore.Neutral ~0.9999 and the other members at or near 0.
  Emulator: multi-word named mentions are split at the word boundary and the halves are grouped as separate entities (Entities[0].Mentions[0].Text expected 'Amazon Comprehend' got 'Amazon' with EndOffset expected 17 got 6; Entities[1].Mentions[0].Text expected 'John Smith' got 'Comprehend' at 7-17 where AWS has 130-140), the split half is typed COMMERCIAL_ITEM where AWS returns PERSON, two recorded entity groups are absent entirely ($.Entities[5] and $.Entities[6] missing), and the mention sentiment is split between Neutral 0.5322045087814331 and Positive 0.46580344438552856 — the SAME pair for every mention — where AWS records Neutral ~0.999998 with the others ~1e-06/0.0, with Score and GroupScore again ladder constants (0.949999988079071, 0.0009960159659385681).
  Why: targeted sentiment answers 'which entity, where, and how is it spoken of'. Splitting a named mention points the client at a fragment, the wrong Type routes it to the wrong entity handling, dropped groups lose entities the document discusses outright, and a 0.53/0.47 Neutral/Positive split turns a factual mention into a coin flip, which inverts any per-entity aggregate.
  Fix: the DetectTargetedSentiment handler — mention detection and co-reference grouping (one group per entity, mention spans covering the whole named mention), the entity typer, and the mention sentiment scorer (one distribution per mention concentrated on the reported Sentiment).
  Traces: comprehend-detect-suite-single-document#task#7.

Cause 7 of 8: DetectToxicContent returns 0.0 for every toxic label score and for overall Toxicity, so the analysis carries no signal at all. Fixture comprehend-detect-suite-single-document, step task#8.
  AWS: Smithy: DetectToxicContent(TextSegments required @length min 1, LanguageCode required) -> ResultList: ListOfToxicLabels of ToxicLabels{Labels: list of ToxicContent{Name (ToxicContentType: GRAPHIC|HARASSMENT_OR_ABUSE|HATE_SPEECH|INSULT|PROFANITY|SEXUAL|VIOLENCE_OR_THREAT), Score ('Model confidence in the detected content type. Value range is zero to one, where one is highest confidence')}, Toxicity ('Overall toxicity score for the string. Value range is zero to one, where one is the highest confidence')}. Recorded behavior: every one of the seven ToxicContentType labels is returned with its own small non-zero confidence for benign text, and Toxicity is a non-zero aggregate of them.
  Emulator: all seven Labels[*].Score AND the segment's Toxicity are returned as exactly 0.0, against seven distinct recorded values (0.0182, 0.0239, 0.0248, 0.0186, 0.0607, 0.0199, 0.0108) and a recorded Toxicity of 0.0226 — the label names and list shape are right, but the model output is hard-zero.
  Why: this is a safety gate: clients compare Labels[*].Score and Toxicity against a block/flag threshold and some also require the scores to be a proper distribution. A hard 0.0 means the emulator reports every input as perfectly non-toxic, so a moderation path built on it never fires and cannot be exercised at all — and a client that normalises by Toxicity divides by zero.
  Fix: the DetectToxicContent handler's toxicity scorer — emit a per-label confidence in (0,1] for each ToxicContentType and a non-zero aggregate Toxicity, instead of zero-filling the response.
  Traces: comprehend-detect-suite-single-document#task#8.

Cause 8 of 8: ContainsPiiEntities reports ladder constants instead of per-label confidences. Fixture comprehend-detect-suite-single-document, step task#9.
  AWS: Smithy: ContainsPiiEntities(Text required, LanguageCode required) -> Labels: ListOfEntityLabels of EntityLabel{Name (PiiEntityType), Score ('The level of confidence that Amazon Comprehend has in the accuracy of the detection')}. Recorded behavior: for a document that unambiguously contains each reported PII category, AWS returns a saturated confidence (1.0) per label.
  Emulator: the two labels come back with the ladder constants 0.8999999761581421 and 0.75 where AWS records 1.0 for both — the same 0.9/0.75 literals the DetectPiiEntities path uses, i.e. a per-shape constant rather than a per-label confidence.
  Why: ContainsPiiEntities is the cheap pre-filter in front of redaction: a client routes a document for PII handling when a label's Score clears a threshold. A label AWS reports at full confidence arriving as 0.75 falls below a 0.9 gate, so the document skips PII handling entirely.
  Fix: the ContainsPiiEntities handler's label scorer — derive each EntityLabel.Score from the detections supporting that PiiEntityType instead of a fixed constant.
  Traces: comprehend-detect-suite-single-document#task#9.

In all eight causes the recorded offsets, texts, tags and numeric scores are this recording's values for ITS OWN input text and are not themselves the requirement. The requirement is the convention: one detection per span/word/entity with the span bounded by what it describes, types drawn from the modelled enums by what the span actually is, one normalised distribution per detection concentrated on the reported label, a non-zero in-range confidence for every toxicity label, and a per-detection confidence rather than a literal that recurs across detections and across operations.

## Evidence
All eight steps: expected HTTP 200, got HTTP 200. The status and response shape match; the divergence is entirely in the body values. Fixture comprehend-detect-suite-single-document throughout.

Cause 1 — task#1, DetectDominantLanguage:
  $.Languages[0].Score: expected 0.9877879619598388, got 0.9990000128746033

Cause 2 — task#2, DetectEntities (six entities):
  $.Entities[0].Score: expected 0.9613922834396362, got 0.963302731513977
  $.Entities[1].Score: expected 1.0, got 0.9776536226272583
  $.Entities[2].Score: expected 0.998832941055298, got 0.8128078579902649
  $.Entities[3].Score: expected 0.990321397781372, got 0.7777777910232544
  $.Entities[4].Score: expected 0.9989029169082642, got 0.9683794379234314
  $.Entities[5].Score: expected 0.992952287197113, got 0.9900744557380676

Cause 3 — task#4, DetectPiiEntities:
  $.Entities[0].Score: expected 0.9992809891700744, got 0.75
  $.Entities[1].Type: expected 'ADDRESS', got 'EMAIL'
  $.Entities[1].BeginOffset: expected 166, got 196; EndOffset: expected 173, got 218
  $.Entities[1].Score: expected 0.9519889950752258, got 0.8999999761581421
  $.Entities[2]: missing (expected {'BeginOffset': 196, 'EndOffset': 218, 'Score': 0.9999270439147948, 'Type': 'EMAIL'}, got None)

Cause 4 — task#5, DetectSentiment:
  $.SentimentScore.Neutral: expected 0.9914856553077698, got 0.6069637537002563
  $.SentimentScore.Positive: expected 0.008444002829492092, got 0.3910442292690277
  $.SentimentScore.Mixed: expected 7.401431048492668e-06, got 0.0009960159659385681
  $.SentimentScore.Negative: expected 6.294498598435894e-05, got the SAME 0.0009960159659385681

Cause 5 — task#6, DetectSyntax:
  $.SyntaxTokens[35], [36], [37], [38], [39], [40]: missing (recorded tail tokens absent)
  $.SyntaxTokens[33].Text and $.SyntaxTokens[33].EndOffset diverge; $.SyntaxTokens[34].BeginOffset and EndOffset diverge
  $.SyntaxTokens[29|30|31].PartOfSpeech.Tag diverge
  $.SyntaxTokens[3].PartOfSpeech.Score: expected 1.0, got 0.9999899864196777; [7] expected 1.0 got 0.9999499917030334; [8] expected 1.0 got 0.663931131362915; [0] expected 0.9999994039535522 got 0.8333263993263245; [11] expected 0.9517213702201844 got 0.7999935746192932 — the same handful of constants recurring across ~40 tokens

Cause 6 — task#7, DetectTargetedSentiment:
  $.Entities[0].Mentions[0].Text: expected 'Amazon Comprehend', got 'Amazon'; EndOffset expected 17, got 6
  $.Entities[1].Mentions[0].Text: expected 'John Smith', got 'Comprehend'; BeginOffset expected 130 got 7; EndOffset expected 140 got 17; Type expected 'PERSON', got 'COMMERCIAL_ITEM'
  $.Entities[0].Mentions[0].MentionSentiment.SentimentScore.Neutral: expected 0.9999979734420776, got 0.5322045087814331; Positive expected 9.999999974752427e-07, got 0.46580344438552856 (identical pair repeated for $.Entities[1])
  $.Entities[0].Mentions[0].Score: expected 0.9986129999160768, got 0.949999988079071; Mixed/Negative returned as the constant 0.0009960159659385681
  $.Entities[2].Mentions[0].BeginOffset: expected 196, got 51; EndOffset expected 218, got 58
  $.Entities[5], $.Entities[6]: missing (two recorded entity groups absent)

Cause 7 — task#8, DetectToxicContent:
  $.ResultList[0].Labels[0..6].Score: expected 0.018200000748038292 / 0.023900000378489494 / 0.024800000712275505 / 0.01860000006854534 / 0.06069999933242798 / 0.0198999997228384 / 0.01080000028014183, got 0.0 for ALL SEVEN
  $.ResultList[0].Toxicity: expected 0.02260000072419643, got 0.0

Cause 8 — task#9, ContainsPiiEntities:
  $.Labels[0].Score: expected 1.0, got 0.8999999761581421
  $.Labels[1].Score: expected 1.0, got 0.75

Cross-cause evidence of a shared constant source: 0.0009960159659385681 appears as a SentimentScore member in causes 4 and 6; 0.963302731513977 appears in cause 2 and on the BatchDetectEntities path; 0.8999999761581421 / 0.75 appear in both cause 3 and cause 8.

## Steps to Reproduce
1. Clone the repo, boot the emulator from the checkout and replay the wire corpus (replay_wire_corpus). Fixture comprehend-detect-suite-single-document replays with no blocked step; replay reports owned_by empty for all eight steps below.
2. Cause 1: step task#1 — DetectDominantLanguage with the fixture's single Text. Read $.Languages[0].Score: it is the round constant 0.999 regardless of the input, not the recorded 0.98779.
3. Cause 2: step task#2 — DetectEntities over the same Text. Read $.Entities[*].Score: the six values come from a fixed ladder (including 0.7777777910232544 and 0.8128) for entities recorded at ~0.99-1.0.
4. Cause 3: step task#4 — DetectPiiEntities (Text + LanguageCode 'en') over a document containing a postal address followed by an e-mail address. Count $.Entities: only two come back; the address span is never detected, the e-mail span occupies the address's slot typed 'EMAIL', the trailing element is missing, and $.Entities[0].Score is 0.75.
5. Cause 4: step task#5 — DetectSentiment over the same Text. Read $.SentimentScore: Neutral is diluted to 0.607 with 0.391 leaking into Positive, and Mixed and Negative are both the single constant 0.0009960159659385681.
6. Cause 5: step task#6 — DetectSyntax (LanguageCode 'en'). Count $.SyntaxTokens: the list stops six tokens short of the recorded 41, the last emitted tokens sit on the wrong words/offsets, the tags near that boundary diverge, and PartOfSpeech.Score values repeat across tokens.
7. Cause 6: step task#7 — DetectTargetedSentiment over the same Text. Read $.Entities[*].Mentions[0]: multi-word named mentions are split into separate groups ('Amazon' / 'Comprehend' instead of 'Amazon Comprehend'), Type is COMMERCIAL_ITEM where PERSON is recorded, two recorded entity groups are missing, and every MentionSentiment is the same Neutral 0.5322 / Positive 0.4658 split.
8. Cause 7: step task#8 — DetectToxicContent with the fixture's TextSegments and LanguageCode 'en'. Read $.ResultList[0]: all seven Labels[*].Score and Toxicity are exactly 0.0.
9. Cause 8: step task#9 — ContainsPiiEntities over the PII-bearing Text. Read $.Labels[*].Score: 0.8999999761581421 and 0.75 where both labels are recorded at 1.0.
