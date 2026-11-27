Issue-Source: trace-sweep-divergence
Issue-Trace: comprehend-batch-detect-syntax-batch-size-limit-rejected
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: comprehend-batch-detect-syntax-batch-size-limit-rejected comprehend-batch-syntax-whole-call-refusals comprehend-batch-key-phrases-whole-call-refusals comprehend-detect-entities-document-bytes-refusals comprehend-detect-suite-single-document
Issue-Shape-Id: com.amazonaws.comprehend#Comprehend_20171127
Issue-Key: comprehend|BatchDetectSyntax|2xx->2xx|$.ResultList[*].SyntaxTokens[*].PartOfSpeech.Tag
Issue-Key: comprehend|BatchDetectKeyPhrases|2xx->2xx|$.ResultList[*].KeyPhrases[*]
Issue-Key: comprehend|BatchDetectKeyPhrases|2xx->2xx|$.ResultList[*].KeyPhrases[*].EndOffset
Issue-Key: comprehend|BatchDetectKeyPhrases|2xx->2xx|$.ResultList[*].KeyPhrases[*].Text
Issue-Key: comprehend|DetectEntities|2xx->2xx|$.Entities[*].Type
Issue-Key: comprehend|DetectTargetedSentiment|2xx->2xx|$.Entities[*].Mentions[*].MentionSentiment.SentimentScore.Mixed
Issue-Related: 20261126T023639Z-comprehend-single-document-detectsyntax-and-detecttargetedsentiment-mislabel-tokens.md
Issue-Related: 20261127T063924Z-comprehend-batchdetect-confidence-scores-are-synthesized-per-shape.md
Issue-Commit: 1a8d6da9d066bb6b81849373d117fc388454286a

# Comprehend token tags, entity types and key-phrase spans ignore the submitted text

> _Note: this issue was identified against the code base at commit `1a8d6da9d066bb6b81849373d117fc388454286a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchDetectSyntax

## Description
FIVE net-new root causes in the comprehend crate's inference layer, all distinct from pure confidence-value noise: here the emulator emits the WRONG LABEL or the WRONG SPAN. Part-of-speech tags are assigned per-position instead of per-token, an entity is given the wrong EntityType, key phrases are returned with offsets and text that do not correspond to a phrase in the input, and targeted-sentiment mentions are reported at another entity's span. All five steps answered HTTP 200 as recorded, so the defect is in the analysis, not in the protocol. A fixer must close ALL FIVE causes and re-replay all five steps.

PRIOR ART (named, NOT covering): 20261126T023639Z-comprehend-single-document-detectsyntax-and-detecttargetedsentiment-mislabel-tokens.md is OPEN and describes DetectSyntax/DetectTargetedSentiment on fixture comprehend-detect-suite-single-document, but replay reports owned_by EMPTY for comprehend-detect-suite-single-document#task#7 (Cause 5 below) — the open issue's replay key does not cover this cluster (the cluster includes $.Entities[*].Mentions[*].MentionSentiment.SentimentScore.Mixed, which that issue's key list omits), so the step would be closed unrun. 20261127T063924Z-comprehend-batchdetect-confidence-scores-are-synthesized-per-shape.md is OPEN only on fixture comprehend-batch-detect-suite. Replay reports stub_path EMPTY and owned_by EMPTY for all five steps below.

Cause 1 of 5: BatchDetectSyntax assigns PartOfSpeech.Tag by token POSITION rather than by the word, so the same index gets the same tag in every batch member. Fixture comprehend-batch-detect-syntax-batch-size-limit-rejected, step task#1.
  AWS: Smithy: BatchDetectSyntax(TextList required CustomerInputStringList @length min 1 sensitive, LanguageCode required SyntaxLanguageCode [en, es, fr, de, it, pt]) -> ResultList of BatchDetectSyntaxItemResult{Index, SyntaxTokens: ListOfSyntaxTokens of SyntaxToken{TokenId, Text, BeginOffset, EndOffset, PartOfSpeech: PartOfSpeechTag{Tag PartOfSpeechTagType, Score}}}. Each token's Tag is the part of speech of THAT word in THAT sentence; VERB and NOUN are distinct members of PartOfSpeechTagType and are selected from the word, never from its index.
  Emulator: $.ResultList[i].SyntaxTokens[1].PartOfSpeech.Tag comes back 'NOUN' for EVERY batch member i in 0..24, where the recording has 'VERB' for all of them — the second token of each document is a verb and is tagged as a noun in all 25 results. 25 diverging members, one per batch entry, all the same substitution.
  Why: a client that extracts predicates by collecting VERB tokens, or candidate terms by collecting NOUN tokens, gets an empty predicate list and a polluted term list for every document in the batch. Because the mistake is uniform across the batch, the error does not average out — every document is wrong in the same way.
  Fix: the BatchDetectSyntax handler's tagger in the comprehend crate — select PartOfSpeechTagType from the token's own surface form and context, not from its position in the token list.
  Traces: comprehend-batch-detect-syntax-batch-size-limit-rejected#task#1.

Cause 2 of 5: BatchDetectSyntax saturates PartOfSpeech.Score to 1.0 and mislabels PROPN/NOUN across a long document. Fixture comprehend-batch-syntax-whole-call-refusals, step task#1.
  AWS: Smithy: as Cause 1; PartOfSpeechTag.Score is 'The confidence that Amazon Comprehend has that the part of speech was correctly identified'. Recorded behavior: on an ambiguous document the confidences spread widely (0.5447735190391540, 0.5669102072715759, 0.6122575402259827, 0.7270651459693909 ... 0.999964475631714), and PROPN is used for proper nouns while NOUN is used for common nouns.
  Emulator: essentially every PartOfSpeech.Score in the result comes back as the constant 1.0 (over 100 diverging members, e.g. token 0 recorded 0.7270651459693909, token 6 recorded 0.5669102072715759, token 10 recorded 0.544773519039154 — all returned as 1.0), and the tags are swapped in both directions: $.ResultList[1].SyntaxTokens[0].PartOfSpeech.Tag comes back 'NOUN' where the recording has 'PROPN', while $.ResultList[1].SyntaxTokens[6] and [10] come back 'PROPN' where the recording has 'NOUN'.
  Why: saturating to 1.0 destroys the only signal a client has for spotting ambiguous tokens — a 'review tokens below 0.8' rule never fires, even on the tokens AWS itself reports at 0.54. Swapping PROPN and NOUN moves tokens between the named-entity candidate set and the common-noun set, so term extraction and anonymisation heuristics built on the tag change behavior.
  Fix: the BatchDetectSyntax handler's tagger/scorer in the comprehend crate — derive Score from the tagging decision instead of returning a constant, and distinguish proper from common nouns.
  Traces: comprehend-batch-syntax-whole-call-refusals#task#1.

Cause 3 of 5: BatchDetectKeyPhrases returns a key phrase whose Text and EndOffset are not a noun phrase from the input — on a long repetitive document it returns the whole run of text as one phrase, clipped to a round offset, and drops a second phrase AWS reports. Fixture comprehend-batch-key-phrases-whole-call-refusals, step task#1.
  AWS: Smithy: BatchDetectKeyPhrases(TextList required, LanguageCode required) -> ResultList of BatchDetectKeyPhrasesItemResult{Index, KeyPhrases: ListOfKeyPhrases of KeyPhrase{Score, Text 'The text of a key noun phrase', BeginOffset 'The zero-based offset from the beginning of the source text to the first character in the key phrase', EndOffset 'the last character in the key phrase'}}. So Text MUST be a key NOUN PHRASE and MUST equal the source substring between its own BeginOffset and EndOffset; the offsets are determined by where the phrase ends in the text, never by a buffer boundary.
  Emulator: for the long batch member, $.ResultList[1].KeyPhrases[0].EndOffset comes back 5000 where the recording has 4949 — the suspiciously round 5000 is a truncation limit, not a phrase boundary — and $.ResultList[1].KeyPhrases[0].Text comes back as the entire repeated run of text rather than the single phrase AWS extracted, with Score 0.9999997615814209 against a recorded 0.8039007782936096 (AWS is appropriately UNconfident about a phrase in degenerate repeated text, the emulator is maximally confident). $.ResultList[1].KeyPhrases[1] is missing entirely (expected present), and the short member's two Scores also diverge (0.9999285936355591 / 0.9997500777244568 against 0.9999857544898988 / 0.9999053478240968).
  Why: a client slices the source text with BeginOffset/EndOffset to highlight or extract each phrase. An EndOffset pinned to a 5000-character boundary slices mid-phrase, and a Text that is the whole run rather than a noun phrase makes the result unusable for keyword extraction; dropping the second phrase changes the cardinality of the result list, so clients that index KeyPhrases[1] read out of range.
  Fix: the BatchDetectKeyPhrases handler's phrase extraction in the comprehend crate — end each phrase at its own boundary in the source text (no fixed character cap), keep Text == source[BeginOffset..EndOffset], and emit every phrase found rather than collapsing the member into one.
  Traces: comprehend-batch-key-phrases-whole-call-refusals#task#1.

Cause 4 of 5: DetectEntities types an organization mention as OTHER. Fixture comprehend-detect-entities-document-bytes-refusals, step task#1.
  AWS: Smithy: DetectEntities -> Entities[*].Type is EntityType, enum [PERSON, LOCATION, ORGANIZATION, COMMERCIAL_ITEM, EVENT, DATE, QUANTITY, TITLE, OTHER]. ORGANIZATION is the member for a company/institution mention; OTHER is the catch-all used only when no other member applies, so a recognised organization is never typed OTHER.
  Emulator: $.Entities[1].Type comes back 'OTHER' where the recording has 'ORGANIZATION'. The span, offsets and the rest of the entity list match — only the classification is wrong.
  Why: entity consumers branch on Type: ORGANIZATION mentions feed company resolution, KYC and redaction rules, while OTHER is normally discarded. Typing an organization OTHER silently removes it from every one of those paths.
  Fix: the DetectEntities handler's entity classifier in the comprehend crate — map a recognised organization span to EntityType ORGANIZATION and reserve OTHER for spans with no applicable member.
  Traces: comprehend-detect-entities-document-bytes-refusals#task#1.

Cause 5 of 5: DetectTargetedSentiment reports mentions at the wrong span (Text/Type/offsets belong to another entity) and fills MentionSentiment.SentimentScore from a shared table, including the Mixed member. Fixture comprehend-detect-suite-single-document, step task#7.
  AWS: Smithy: DetectTargetedSentiment(Text required sensitive, LanguageCode required) -> Entities: ListOfTargetedSentimentEntities of TargetedSentimentEntity{DescriptiveMentionIndex, Mentions of TargetedSentimentMention{Score, GroupScore 'the confidence that all the entities mentioned in the group relate to the same entity', Text 'The text in the document that identifies the entity', Type, MentionSentiment{Sentiment, SentimentScore{Positive, Negative, Neutral, Mixed}}, BeginOffset, EndOffset}}. Each mention is anchored to a real span: Text MUST be the document substring between its own BeginOffset and EndOffset, Type MUST describe that span, and SentimentScore is one normalised distribution for that mention (AWS records exact 0.0 for members it rules out).
  Emulator: the mention layout is shifted by one entity. $.Entities[2].Mentions[0].BeginOffset/EndOffset come back 150/162 where the recording has 196/218, and Text/Type diverge with them; the shift continues into $.Entities[3].Mentions[0] and $.Entities[4].Mentions[0]. GroupScore is saturated to 1.0 where the recording has 0.9999629855155944, and the per-mention SentimentScore is drawn from a shared table: Mixed 4.868698333382326e-09 and 4.601400593884364e-09 where the recording has exactly 0.0, Negative 4.0612103475723416e-05 / 6.118862074799836e-05 where the recording has 9.999999974752427e-07 / 0.0, Neutral 0.6901879906654358 where the recording has 0.9999979734420776 and Positive 0.3097713887691498 where the recording has 9.999999974752427e-07 — and the triple (Neutral 0.9978035688400269, Positive 0.002135267248377204, Negative 6.118862074799836e-05) is repeated verbatim for unrelated mentions. Mention Scores reuse the entity pool (0.9956942796707153, 0.8286985754966736 — the same literals DetectEntities/DetectPiiEntities return on this document).
  Why: a targeted-sentiment client slices the document with BeginOffset/EndOffset to attribute a verdict to each aspect. Mentions reported at another entity's span attribute sentiment to the wrong part of the text; and a mention AWS reports as decisively NEUTRAL (~1.0) arriving as 0.69 Neutral / 0.31 Positive flips it into an ambiguous one, changing the per-aspect verdict.
  Fix: the DetectTargetedSentiment handler in the comprehend crate — anchor each mention to the span it matched (Text == document[BeginOffset..EndOffset], Type from that span), and compute Score, GroupScore and a normalised MentionSentiment.SentimentScore per mention instead of reading the shared table used by the entity and sentiment handlers.
  Traces: comprehend-detect-suite-single-document#task#7.

In all five causes the recorded tags, types, offsets and floats are this recording's values for ITS OWN input and are not themselves the requirement. The requirements are the conventions: one part-of-speech tag per token chosen from the word (not its index), PROPN vs NOUN distinguished, a key phrase that is a noun phrase equal to source[BeginOffset..EndOffset] with no fixed character cap, ORGANIZATION rather than OTHER for a recognised organization, and one mention per real span with a distribution computed for that mention.

## Evidence
All five steps: expected HTTP 200, got HTTP 200; replay verdict 'mismatch', stub_path EMPTY, owned_by EMPTY.

Cause 1 - comprehend-batch-detect-syntax-batch-size-limit-rejected#task#1, BatchDetectSyntax (25 diverging members):
  $.ResultList[0].SyntaxTokens[1].PartOfSpeech.Tag: expected 'VERB', got 'NOUN'
  $.ResultList[1].SyntaxTokens[1].PartOfSpeech.Tag: expected 'VERB', got 'NOUN'
  ... identical substitution for every index through $.ResultList[24].SyntaxTokens[1].PartOfSpeech.Tag

Cause 2 - comprehend-batch-syntax-whole-call-refusals#task#1, BatchDetectSyntax (100+ diverging members):
  $.ResultList[1].SyntaxTokens[0].PartOfSpeech.Tag: expected 'PROPN', got 'NOUN'
  $.ResultList[1].SyntaxTokens[6].PartOfSpeech.Tag: expected 'NOUN', got 'PROPN'
  $.ResultList[1].SyntaxTokens[10].PartOfSpeech.Tag: expected 'NOUN', got 'PROPN'
  $.ResultList[0].SyntaxTokens[0].PartOfSpeech.Score: expected 0.9574857354164124, got 1.0
  $.ResultList[0].SyntaxTokens[1].PartOfSpeech.Score: expected 0.999964475631714, got 1.0
  $.ResultList[1].SyntaxTokens[0].PartOfSpeech.Score: expected 0.7270651459693909, got 1.0
  $.ResultList[1].SyntaxTokens[6].PartOfSpeech.Score: expected 0.5669102072715759, got 1.0
  $.ResultList[1].SyntaxTokens[10].PartOfSpeech.Score: expected 0.544773519039154, got 1.0
  ... 1.0 returned for every diverging score through $.ResultList[2].SyntaxTokens[1].PartOfSpeech.Score

Cause 3 - comprehend-batch-key-phrases-whole-call-refusals#task#1, BatchDetectKeyPhrases:
  $.ResultList[1].KeyPhrases[0].EndOffset: expected 4949, got 5000   (round truncation boundary, not a phrase boundary)
  $.ResultList[1].KeyPhrases[0].Text: expected the single extracted phrase, got the entire repeated run of source text
  $.ResultList[1].KeyPhrases[0].Score: expected 0.8039007782936096, got 0.9999997615814209
  $.ResultList[1].KeyPhrases[1]: expected present, missing from the response
  $.ResultList[0].KeyPhrases[0].Score: expected 0.9999857544898988, got 0.9999285936355591
  $.ResultList[0].KeyPhrases[1].Score: expected 0.9999053478240968, got 0.9997500777244568

Cause 4 - comprehend-detect-entities-document-bytes-refusals#task#1, DetectEntities:
  $.Entities[1].Type: expected 'ORGANIZATION', got 'OTHER'

Cause 5 - comprehend-detect-suite-single-document#task#7, DetectTargetedSentiment (38 diverging members):
  $.Entities[2].Mentions[0].BeginOffset: expected 196, got 150
  $.Entities[2].Mentions[0].EndOffset: expected 218, got 162
  $.Entities[2].Mentions[0].GroupScore: expected 0.9999629855155944, got 1.0
  $.Entities[2].Mentions[0].Text / .Type: diverge with the offsets (cluster key list)
  $.Entities[3].Mentions[0].BeginOffset / .EndOffset / .Text / .Type / .Score: diverge (cluster key list)
  $.Entities[4].Mentions[0].BeginOffset / .EndOffset / .Text / .Type / .Score: diverge (cluster key list)
  $.Entities[0].Mentions[0].MentionSentiment.SentimentScore.Mixed: expected 0.0, got 4.868698333382326e-09
  $.Entities[0].Mentions[0].MentionSentiment.SentimentScore.Negative: expected 9.999999974752427e-07, got 4.0612103475723416e-05
  $.Entities[0].Mentions[0].MentionSentiment.SentimentScore.Neutral: expected 0.9999979734420776, got 0.6901879906654358
  $.Entities[0].Mentions[0].MentionSentiment.SentimentScore.Positive: expected 9.999999974752427e-07, got 0.3097713887691498
  $.Entities[0].Mentions[0].Score: expected 0.9986129999160768, got 0.9956942796707153
  $.Entities[1].Mentions[0].MentionSentiment.SentimentScore.Mixed: expected 0.0, got 4.601400593884364e-09
  $.Entities[1].Mentions[0].MentionSentiment.SentimentScore.Negative: expected 0.0, got 6.118862074799836e-05
  $.Entities[1].Mentions[0].MentionSentiment.SentimentScore.Neutral: expected 0.9999989867210388, got 0.9978035688400269
  $.Entities[1].Mentions[0].MentionSentiment.SentimentScore.Positive: expected 9.999999974752427e-07, got 0.002135267248377204
  $.Entities[1].Mentions[0].Score: expected 0.9999549984931946, got 0.8286985754966736
  $.Entities[2].Mentions[0].MentionSentiment.SentimentScore.Neutral: expected 1.0, got 0.9978035688400269 (triple repeated verbatim across unrelated mentions)

## Steps to Reproduce
1. Clone the repo, boot the emulator from the checkout and replay the wire corpus (replay_wire_corpus). All five fixtures replay with no blocked or errored step; replay reports stub_path EMPTY and owned_by EMPTY for the five steps below.
2. Cause 1: comprehend-batch-detect-syntax-batch-size-limit-rejected step task#1 - BatchDetectSyntax with the fixture's 25-entry TextList. Read $.ResultList[*].SyntaxTokens[1].PartOfSpeech.Tag: 'NOUN' for all 25 results where AWS recorded 'VERB' for all 25, i.e. the tag follows the token INDEX, not the word.
3. Cause 2: comprehend-batch-syntax-whole-call-refusals step task#1 - BatchDetectSyntax. Collect every $.ResultList[*].SyntaxTokens[*].PartOfSpeech.Score: the constant 1.0 is returned even for tokens AWS recorded at 0.5447/0.5669/0.7270. Then compare tags at $.ResultList[1].SyntaxTokens[0], [6] and [10]: PROPN and NOUN are swapped in both directions.
4. Cause 3: comprehend-batch-key-phrases-whole-call-refusals step task#1 - BatchDetectKeyPhrases with the long repetitive member. Read $.ResultList[1].KeyPhrases[0]: EndOffset is 5000 (AWS recorded 4949) and Text is the whole repeated run instead of one noun phrase; slice the request's text with the returned offsets and confirm it does not delimit a phrase. Note $.ResultList[1].KeyPhrases[1] is absent although AWS returned it.
5. Cause 4: comprehend-detect-entities-document-bytes-refusals step task#1 - DetectEntities. $.Entities[1].Type is 'OTHER' where AWS recorded 'ORGANIZATION'; the span and offsets match, so only the classifier is wrong.
6. Cause 5: comprehend-detect-suite-single-document step task#7 - DetectTargetedSentiment. For each $.Entities[i].Mentions[0], slice the request Text with the returned BeginOffset/EndOffset and compare with the returned Text: Entities[2] is reported at 150/162 where AWS recorded 196/218, and the shift continues through Entities[3] and Entities[4]. Then read $.Entities[*].Mentions[0].MentionSentiment.SentimentScore: Mixed is ~4.6e-09 where AWS recorded exactly 0.0 and the triple (Neutral 0.9978035688400269, Positive 0.002135267248377204, Negative 6.118862074799836e-05) recurs verbatim.
7. After the fix, re-replay ALL FIVE steps: comprehend-batch-detect-syntax-batch-size-limit-rejected#task#1, comprehend-batch-syntax-whole-call-refusals#task#1, comprehend-batch-key-phrases-whole-call-refusals#task#1, comprehend-detect-entities-document-bytes-refusals#task#1 and comprehend-detect-suite-single-document#task#7.
