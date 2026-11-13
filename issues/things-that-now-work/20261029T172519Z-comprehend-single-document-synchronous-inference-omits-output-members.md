Issue-Source: trace-sweep-divergence
Issue-Trace: comprehend-detect-suite-single-document
Issue-Step: task#8
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: comprehend-detect-suite-single-document
Issue-Shape-Id: com.amazonaws.comprehend#Comprehend_20171127
Issue-Key: comprehend|DetectToxicContent|2xx->2xx|$.ResultList[*].Labels[*]
Issue-Key: comprehend|DetectToxicContent|2xx->2xx|$.ResultList[*].Toxicity
Issue-Key: comprehend|DetectDominantLanguage|2xx->2xx|$.Languages[*].Score
Issue-Key: comprehend|DetectEntities|2xx->2xx|$.Entities[*]
Issue-Key: comprehend|DetectEntities|2xx->2xx|$.Entities[*].BeginOffset
Issue-Key: comprehend|DetectEntities|2xx->2xx|$.Entities[*].EndOffset
Issue-Key: comprehend|DetectEntities|2xx->2xx|$.Entities[*].Score
Issue-Key: comprehend|DetectEntities|2xx->2xx|$.Entities[*].Text
Issue-Key: comprehend|DetectEntities|2xx->2xx|$.Entities[*].Type
Issue-Key: comprehend|DetectPiiEntities|2xx->2xx|$.Entities[*]
Issue-Key: comprehend|DetectPiiEntities|2xx->2xx|$.Entities[*].BeginOffset
Issue-Key: comprehend|DetectPiiEntities|2xx->2xx|$.Entities[*].EndOffset
Issue-Key: comprehend|DetectPiiEntities|2xx->2xx|$.Entities[*].Score
Issue-Key: comprehend|DetectPiiEntities|2xx->2xx|$.Entities[*].Type
Issue-Key: comprehend|DetectSentiment|2xx->2xx|$.Sentiment
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
Issue-Key: comprehend|ContainsPiiEntities|2xx->2xx|$.Labels[*]
Issue-Key: comprehend|ContainsPiiEntities|2xx->2xx|$.Labels[*].Score
Issue-Commit: 7b78f885090352093daa8ac758bd09ec0b97c628

# Comprehend single-document synchronous inference omits output members and misplaces every detected span

> _Note: this issue was identified against the code base at commit `7b78f885090352093daa8ac758bd09ec0b97c628`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DetectToxicContent

## Description
Eight net-new root causes in the single-document synchronous inference family. Two of them drop modeled output members entirely (DetectToxicContent serves no Labels at all; ContainsPiiEntities serves fewer labels than the document carries); the rest emit placeholder scores and spans that do not follow AWS's entity, span or class conventions. These are separate analysers from the BatchDetect ones (see the companion batch issue), so fixing the batch path does not close these steps.

Cause 1 of 8: DetectToxicContent omits the entire Labels list and reports Toxicity 0.0.
  AWS: Smithy: DetectToxicContent output ResultList of ToxicLabels{Labels: list of ToxicContent{Name: ToxicContentType, Score: Float}, Toxicity: Float ("Overall toxicity score for the string. Value range is zero to one")}; recorded behavior: for every text segment AWS returns one ToxicContent entry per toxic-content TYPE — the full label set (PROFANITY, HATE_SPEECH, INSULT, GRAPHIC, HARASSMENT_OR_ABUSE, SEXUAL, VIOLENCE_OR_THREAT) — each with its own score, plus an overall Toxicity.
  Emulator: Labels is absent and Toxicity is exactly 0.0, so the response carries no toxicity information whatsoever.
  Why: a moderation client reads Labels to decide per-category action; with the member missing it either NPEs or concludes every document is clean.
  Fix: the DetectToxicContent handler — it must enumerate every ToxicContentType into Labels and compute Toxicity.
  Traces: comprehend-detect-suite-single-document#task#8.

Cause 2 of 8: ContainsPiiEntities under-reports the label set and pins Score.
  AWS: Smithy: ContainsPiiEntities output Labels (list of EntityLabel{Name: PiiEntityType, Score}); recorded behavior: one EntityLabel per PII type the document contains, with calibrated scores.
  Emulator: one label only (the document's second PII type, NAME, is missing) and Score is the constant 0.8999999761581421 where the recording has a confident value.
  Why: a client gating on "does this text contain a name" is told it does not.
  Fix: the ContainsPiiEntities handler's label enumeration and scoring.
  Traces: comprehend-detect-suite-single-document#task#9.

Cause 3 of 8: DetectPiiEntities misses PII entities and mis-spans the one it finds.
  AWS: Smithy: DetectPiiEntities output Entities (list of PiiEntity{Score, Type: PiiEntityType, BeginOffset, EndOffset}); recorded behavior: one entity per PII occurrence, in document order, with offsets spanning the occurrence in Text.
  Emulator: it returns a single entity (the EMAIL) and drops the NAME and ADDRESS entities the recording reports, so entity order and count diverge too.
  Why: a redaction client driven by these offsets leaves the name and address in the document.
  Fix: the DetectPiiEntities handler's entity recognisers and offset computation.
  Traces: comprehend-detect-suite-single-document#task#4.

Cause 4 of 8: DetectEntities mis-segments entity spans and assigns the wrong Type, shifting the whole Entities list.
  AWS: Smithy: DetectEntities output Entities (list of Entity{Score, Type: EntityType, Text, BeginOffset, EndOffset}); recorded behavior: multi-word product names are split as AWS's recogniser splits them, each entity carries its own Type (COMMERCIAL_ITEM, PERSON, ORGANIZATION, LOCATION, OTHER), and the list covers every occurrence.
  Emulator: it merges two adjacent entities into one span, consequently shifts every later entity's index and Type, drops the final entity, and scores from a constant ladder (0.7125 / 0.675 / 0.95 / 0.99).
  Why: a client keyed on Entities[i].Type reads one entity's type against another's text, and loses the last entity entirely.
  Fix: the DetectEntities handler's recogniser (span segmentation and type assignment) and its scoring.
  Traces: comprehend-detect-suite-single-document#task#2.

Cause 5 of 8: DetectDominantLanguage reports a saturated Score.
  AWS: Smithy: DetectDominantLanguage output Languages (list of DominantLanguage{LanguageCode, Score}); recorded behavior: a calibrated Score strictly below 1.0.
  Emulator: Score exactly 1.0.
  Why: Score carries no information, so a confidence threshold always passes.
  Fix: the DetectDominantLanguage handler's score calibration.
  Traces: comprehend-detect-suite-single-document#task#1.

Cause 6 of 8: DetectSentiment picks the wrong Sentiment class and emits a degenerate score vector.
  AWS: Smithy: DetectSentiment output Sentiment (SentimentType) and SentimentScore{Positive, Negative, Neutral, Mixed}; recorded behavior: Sentiment is the argmax of a calibrated distribution; a factual document is NEUTRAL with a dominant Neutral score.
  Emulator: the document is classified POSITIVE off a near-even Positive/Neutral split, with Negative and Mixed pinned to exactly 0.0.
  Why: a client routing on Sentiment mislabels neutral text as positive.
  Fix: the DetectSentiment handler's classifier and score normalisation.
  Traces: comprehend-detect-suite-single-document#task#5.

Cause 7 of 8: DetectSyntax assigns wrong PartOfSpeech tags, bucketed scores, and a different token count.
  AWS: Smithy: DetectSyntax output SyntaxTokens (list of SyntaxToken{TokenId, Text, BeginOffset, EndOffset, PartOfSpeech{Tag, Score}}); recorded behavior: AWS's tokenisation and tag set — proper nouns PROPN, copulas VERB, pronouns PRON — with calibrated scores.
  Emulator: PROPN is tagged NOUN, copulas AUX, pronouns DET, nouns VERB; scores come from a fixed ladder; and its tokenisation produces a different number of tokens than the recording (trailing tokens absent, one token's Text/offsets shifted).
  Why: POS-based filtering and any offset-driven annotation of the source text both diverge from the real service.
  Fix: the DetectSyntax handler's tokeniser, tagger and tag-set mapping.
  Traces: comprehend-detect-suite-single-document#task#6.

Cause 8 of 8: DetectTargetedSentiment picks the wrong target entities, types and mention sentiment.
  AWS: Smithy: DetectTargetedSentiment output Entities (list of TargetedSentimentEntity{DescriptiveMentionIndex, Mentions: list of TargetedSentimentMention{Score, GroupScore, Text, Type, MentionSentiment{Sentiment, SentimentScore}, BeginOffset, EndOffset}}); recorded behavior: the targeted entities are the document's named entities (ORGANIZATION, PERSON, …) with calibrated mention scores and a per-mention sentiment distribution.
  Emulator: it targets common nouns instead (Type OTHER, Text 'language', 'machine'), so every mention's Text, offsets and Type diverge, mention sentiment is a fixed near-even Neutral/Positive split, GroupScore is pinned to 1.0, and the recording's last entity is missing.
  Why: the whole point of the operation — which named target carries which sentiment — is wrong, and a client indexing Entities[i].Mentions[0] reads a span that is not the entity it asked about.
  Fix: the DetectTargetedSentiment handler's target selection, mention typing and mention-sentiment scoring.
  Traces: comprehend-detect-suite-single-document#task#7.

## Evidence
Cause 1 (task#8): $.ResultList[0].Labels[0..6] missing — the recording carries PROFANITY, HATE_SPEECH, INSULT, GRAPHIC, HARASSMENT_OR_ABUSE, SEXUAL, VIOLENCE_OR_THREAT, each with a Score — got None; $.ResultList[0].Toxicity expected 0.02260000072419643, got 0.0.
Cause 2 (task#9): $.Labels[0].Score expected 1.0, got 0.8999999761581421; $.Labels[1] missing (recording has {'Name': 'NAME', 'Score': 1.0}).
Cause 3 (task#4): $.Entities[0].Type expected 'NAME', got 'EMAIL'; BeginOffset expected 130, got 196; $.Entities[1] missing ({'Type': 'ADDRESS', ...}); $.Entities[2] missing ({'Type': 'EMAIL', ...}).
Cause 4 (task#2): $.Entities[0].Text expected 'Amazon', got 'Amazon Comprehend'; EndOffset expected 6, got 17; $.Entities[1].Type expected 'COMMERCIAL_ITEM', got 'PERSON'; $.Entities[5] missing ({'Text': 'john.smith@example.com', 'Type': 'OTHER', ...}).
Cause 5 (task#1): $.Languages[0].Score expected 0.9877879619598388, got 1.0.
Cause 6 (task#5): $.Sentiment expected 'NEUTRAL', got 'POSITIVE'; SentimentScore.Neutral expected 0.9914856553077698, got 0.4426296055316925; SentimentScore.Negative expected 6.294498598435894e-05, got 0.0.
Cause 7 (task#6): $.SyntaxTokens[0].PartOfSpeech.Tag expected 'PROPN', got 'NOUN'; [2] expected 'VERB', got 'AUX'; [8] expected 'PRON', got 'DET'; [6]/[11] expected 'NOUN', got 'VERB'; Scores expected ~1.0, got 0.99/0.85/0.6; SyntaxTokens[35..40] missing and [33]/[34] Text and offsets shifted.
Cause 8 (task#7): $.Entities[0].Mentions[0].Type expected 'ORGANIZATION', got 'OTHER'; Entities[1].Mentions[0].Text expected 'John Smith', got 'language'; Entities[2].Mentions[0].Text expected 'john.smith@example.com', got 'machine'; Entities[2].Mentions[0].GroupScore expected 0.9999629855155944, got 1.0; MentionSentiment.SentimentScore.Neutral expected ~1.0, got 0.5060853958129883; $.Entities[5] missing.
(The recorded literals are this recording's values for its own document; the requirement is the convention — present output members, AWS's span/type/tag conventions, calibrated scores — not these numbers.)

## Steps to Reproduce
1. Boot the emulator from mainline and replay the wire corpus (replay_wire_corpus).
2. Fixture comprehend-detect-suite-single-document, all against the same input document: step task#8 (DetectToxicContent), task#9 (ContainsPiiEntities), task#4 (DetectPiiEntities), task#2 (DetectEntities), task#1 (DetectDominantLanguage), task#5 (DetectSentiment), task#6 (DetectSyntax), task#7 (DetectTargetedSentiment).
3. Compare each 200 response against the recording: DetectToxicContent carries no Labels, ContainsPiiEntities one label, DetectPiiEntities/DetectEntities drop and mis-span entities, DetectSentiment flips NEUTRAL to POSITIVE, DetectSyntax mis-tags and mis-tokenises, DetectTargetedSentiment targets common nouns.
