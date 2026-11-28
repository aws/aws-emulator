Issue-Source: trace-sweep-divergence
Issue-Trace: comprehend-detect-entities-bytes-need-custom-endpoint
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: comprehend-detect-entities-bytes-need-custom-endpoint comprehend-detect-entities-bytes-without-endpoint comprehend-detect-entities-document-bytes-refusals comprehend-detect-sentiment-language-code-refusals
Issue-Shape-Id: com.amazonaws.comprehend#Comprehend_20171127
Issue-Key: comprehend|DetectEntities|4xx->2xx|$.__type
Issue-Key: comprehend|DetectEntities|4xx->2xx|$.message
Issue-Key: comprehend|DetectEntities|4xx->2xx|status
Issue-Key: comprehend|DetectEntities|2xx->4xx|$.Entities
Issue-Key: comprehend|DetectEntities|2xx->4xx|status
Issue-Key: comprehend|DetectSentiment|2xx->4xx|$.Sentiment
Issue-Key: comprehend|DetectSentiment|2xx->4xx|$.SentimentScore
Issue-Key: comprehend|DetectSentiment|2xx->4xx|status
Issue-Commit: 1a8d6da9d066bb6b81849373d117fc388454286a

# Comprehend DetectEntities bytes gating and DetectSentiment input acceptance answer the wrong status

> _Note: this issue was identified against the code base at commit `1a8d6da9d066bb6b81849373d117fc388454286a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DetectEntities

## Description
THREE net-new root causes where the emulator returns the WRONG HTTP STATUS for a synchronous inference request — the most severe divergence class in this corpus, because the client sees success where AWS refuses, or a refusal where AWS succeeds. The emulator's input gating for DetectEntities is wrong in BOTH directions (it accepts a document-bytes request AWS rejects, and it rejects a request AWS accepts), and DetectSentiment refuses an input AWS accepts. A fixer must close ALL THREE causes and re-replay all three steps.

Cause 1 of 3: DetectEntities accepts a Bytes-only request with no custom-model endpoint and answers 200 with a synthesized entity list, where AWS refuses it with 400 ValidationException 'Request text cannot be null'. Fixture comprehend-detect-entities-bytes-need-custom-endpoint, step task#2 (cluster of 4 affected steps: this one plus comprehend-detect-entities-bytes-need-custom-endpoint#task#3, comprehend-detect-entities-bytes-without-endpoint#task#2 and comprehend-detect-entities-document-bytes-refusals#task#2 — all four are the same refusal and all four are fixed together).
  AWS: Smithy: DetectEntities(Text CustomerInputString @length min 1 sensitive, LanguageCode, EndpointArn EntityRecognizerEndpointArn @pattern entity-recognizer-endpoint/..., Bytes SemiStructuredDocumentBlob @length min 1, DocumentReaderConfig); errors InvalidRequestException(400), ResourceUnavailableException(404), TextSizeLimitExceededException(400), UnsupportedLanguageException(400), InternalServerException(500). The Bytes member's own documentation states: 'This field applies only when you use a custom entity recognition model that was trained with PDF annotations. For other cases, enter your text input in the Text field' and 'If you use the Bytes parameter, do not use the Text parameter.' So on the default (non-custom, no-EndpointArn) path the document-bytes input is not a text source and the request has no text at all. Recorded behavior for that request: HTTP 400 with $.__type 'ValidationException' and $.message 'Request text cannot be null' — fixed text, no variable parts.
  Emulator: HTTP 200 with an Entities list and NO error members — $.__type and $.message are absent. It treats the Bytes blob as if it were the Text input, on a request that names no endpoint, and manufactures an analysis.
  Why: a client that forgets EndpointArn (or points Bytes at a PDF expecting custom-model analysis) is told by AWS, synchronously and unambiguously, that it supplied no text; against the emulator the same bug is invisible and the client consumes fabricated entities as if they came from its document. Worse, it is a false NEGATIVE on a precondition: code written and tested against the emulator ships a request real AWS rejects outright.
  Fix: the DetectEntities handler's input gating in the comprehend crate — when Bytes is supplied without a custom-model EndpointArn, refuse with HTTP 400, __type 'ValidationException' and message 'Request text cannot be null' instead of analysing the blob.
  Traces: comprehend-detect-entities-bytes-need-custom-endpoint#task#2 (representative), plus comprehend-detect-entities-bytes-need-custom-endpoint#task#3, comprehend-detect-entities-bytes-without-endpoint#task#2, comprehend-detect-entities-document-bytes-refusals#task#2 in the same cluster.

Cause 2 of 3: the same gate is over-strict in the other direction — DetectEntities refuses with 400 a request AWS answers 200 on, returning its full entity list. Fixture comprehend-detect-entities-bytes-without-endpoint, step task#3.
  AWS: Smithy: DetectEntities as above. Recorded behavior: HTTP 200 with $.Entities = the three mentions of the fixture's text (a PERSON, a LOCATION and a DATE span, each with its own Score, Text, Type, BeginOffset and EndOffset) — i.e. AWS accepts this combination of DetectEntities input members and analyses the text. The Bytes documentation above permits a plain-text document in Bytes ('Using a plain-text file in the Bytes parameter is equivalent to using the Text parameter (the Entities field in the response is identical)'), so an accepted bytes/text combination must still produce the text's entities.
  Emulator: HTTP 400 — $.Entities is absent entirely. The request is rejected before analysis.
  Hypothesis: the gating added for the Bytes path classifies this member combination as invalid (e.g. Bytes together with Text, or a plain-text Bytes payload) and refuses the whole call, rather than analysing the text AWS analyses. The requirement from the recording is the OUTCOME: this request must answer 200 with the entity list for its text, not 400.
  Why: the opposite of Cause 1 and equally breaking — a request that works in production fails against the emulator, so an application cannot be exercised at all on this path. Combined with Cause 1, the DetectEntities accept/refuse boundary is inverted on both sides, which no client can work around.
  Fix: the DetectEntities handler's input gating in the comprehend crate — the same gate as Cause 1; make it refuse exactly the no-text/no-endpoint case and accept everything AWS accepts, then run the normal text analysis for the accepted request.
  Traces: comprehend-detect-entities-bytes-without-endpoint#task#3.

Cause 3 of 3: DetectSentiment refuses with 400 a request AWS answers 200 on, so the baseline call of the language-code suite never runs. Fixture comprehend-detect-sentiment-language-code-refusals, step task#1.
  AWS: Smithy: DetectSentiment(Text required CustomerInputString @length min 1 sensitive, LanguageCode required LanguageCode enum [en, es, fr, de, it, pt, ar, hi, ja, ko, zh, zh-TW]) -> Sentiment (SentimentType), SentimentScore{Positive, Negative, Neutral, Mixed}; errors InvalidRequestException(400), TextSizeLimitExceededException(400), UnsupportedLanguageException(400), InternalServerException(500). Recorded behavior for this request: HTTP 200 with $.Sentiment 'POSITIVE' and a full four-member SentimentScore — AWS accepts this Text/LanguageCode pair.
  Emulator: HTTP 400 — both $.Sentiment and $.SentimentScore are absent. The call is refused before inference.
  Hypothesis: the DetectSentiment precondition check rejects a modelled input this fixture uses — the enum has 12 members including the hyphenated 'zh-TW', and the emulator's accepted set is reported elsewhere in this corpus as [en, es, fr, de, it, pt, ar, hi, ja, ko, zh, zh-TW], so the refusal is more likely a Text precondition (length/encoding) or a case/format normalisation of LanguageCode than an unknown code. The requirement from the recording is the OUTCOME: this request must answer 200 with Sentiment and a four-member SentimentScore.
  Why: DetectSentiment is the single most used Comprehend operation; a precondition that refuses valid input makes the happy path unavailable, and the step that AWS uses as the suite's positive control is exactly the one that fails. A client sees a 400 for input production accepts.
  Fix: the DetectSentiment handler's input validation in the comprehend crate — refuse only what the model and AWS refuse (missing/empty Text, unmodelled LanguageCode, oversize text) and let this request through to the inference path.
  Traces: comprehend-detect-sentiment-language-code-refusals#task#1.

Note on scope: the status, the error __type and the error message are the requirements here. The entity values in Cause 2's recorded 200 and the SentimentScore floats in Cause 3's recorded 200 are this recording's values for ITS OWN input and are tracked separately (the confidence-pool issue on this service); what this issue demands is that the two steps answer 200 with those members PRESENT, and that the Cause 1 cluster answers 400 ValidationException 'Request text cannot be null'.

## Evidence
All three steps replayed with no blocked or errored step; replay verdict 'mismatch', stub_path EMPTY, owned_by EMPTY for each.

Cause 1 - comprehend-detect-entities-bytes-need-custom-endpoint#task#2, DetectEntities (cluster of 4 affected steps):
  status: expected 400, got 200
  $.__type: expected 'ValidationException', got None (member absent)
  $.message: expected 'Request text cannot be null', got None (member absent)
  affected: comprehend-detect-entities-bytes-need-custom-endpoint#task#2, comprehend-detect-entities-bytes-need-custom-endpoint#task#3, comprehend-detect-entities-bytes-without-endpoint#task#2, comprehend-detect-entities-document-bytes-refusals#task#2

Cause 2 - comprehend-detect-entities-bytes-without-endpoint#task#3, DetectEntities:
  status: expected 200, got 400
  $.Entities: expected a three-element list (a PERSON span, a LOCATION span and a DATE span, each with Score/Text/Type/BeginOffset/EndOffset), got None (member absent)

Cause 3 - comprehend-detect-sentiment-language-code-refusals#task#1, DetectSentiment:
  status: expected 200, got 400
  $.Sentiment: expected 'POSITIVE', got None (member absent)
  $.SentimentScore: expected a four-member distribution {Positive, Negative, Neutral, Mixed}, got None (member absent)

Taken together the DetectEntities gate is inverted on both sides: the no-text/no-endpoint Bytes request that AWS refuses is accepted (Cause 1), and a request AWS accepts is refused (Cause 2).

## Steps to Reproduce
1. Clone the repo, boot the emulator from the checkout and replay the wire corpus (replay_wire_corpus). Fixtures comprehend-detect-entities-bytes-need-custom-endpoint, comprehend-detect-entities-bytes-without-endpoint, comprehend-detect-entities-document-bytes-refusals and comprehend-detect-sentiment-language-code-refusals all replay with no blocked or errored step; replay reports stub_path EMPTY and owned_by EMPTY for the three steps below.
2. Cause 1: comprehend-detect-entities-bytes-need-custom-endpoint step task#2 - DetectEntities with the fixture's Bytes document and no custom-model EndpointArn. The emulator answers HTTP 200 with an Entities list; AWS recorded HTTP 400 with __type 'ValidationException' and message 'Request text cannot be null'. Confirm the same three other steps in the cluster behave identically: comprehend-detect-entities-bytes-need-custom-endpoint#task#3, comprehend-detect-entities-bytes-without-endpoint#task#2 and comprehend-detect-entities-document-bytes-refusals#task#2.
3. Cause 2: comprehend-detect-entities-bytes-without-endpoint step task#3 - replay the step exactly as recorded. The emulator answers HTTP 400 with no $.Entities; AWS recorded HTTP 200 with the three entity mentions of the fixture's text. Contrast with task#1 of the same fixture, which the emulator does answer 200 - the difference isolates which member combination the gate wrongly refuses.
4. Cause 3: comprehend-detect-sentiment-language-code-refusals step task#1 - replay the suite's positive-control DetectSentiment call. The emulator answers HTTP 400 with neither $.Sentiment nor $.SentimentScore; AWS recorded HTTP 200 with Sentiment 'POSITIVE' and a four-member SentimentScore. Contrast with task#2 of the same fixture (an unmodelled LanguageCode), which is correctly refused 400 - so the precondition check is rejecting valid input, not an unknown enum member.
5. After the fix, re-replay ALL THREE steps: comprehend-detect-entities-bytes-need-custom-endpoint#task#2 (and its three cluster members), comprehend-detect-entities-bytes-without-endpoint#task#3 and comprehend-detect-sentiment-language-code-refusals#task#1. Statuses must be 400/200/200 respectively, with the error members present in the first case and the output members present in the other two.
