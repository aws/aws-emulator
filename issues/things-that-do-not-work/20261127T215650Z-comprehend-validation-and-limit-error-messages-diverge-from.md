Issue-Source: trace-sweep-divergence
Issue-Trace: comprehend-batch-key-phrases-whole-call-refusals
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: comprehend-batch-key-phrases-whole-call-refusals comprehend-batch-syntax-whole-call-refusals comprehend-detect-sentiment-language-code-refusals comprehend-detect-toxic-content-segment-count-limit comprehend-stop-training-entity-recognizer-arn-refusals
Issue-Shape-Id: com.amazonaws.comprehend#Comprehend_20171127
Issue-Key: comprehend|BatchDetectKeyPhrases|4xx->4xx|$.message
Issue-Key: comprehend|BatchDetectSyntax|4xx->4xx|$.message
Issue-Key: comprehend|DetectSentiment|4xx->4xx|$.message
Issue-Key: comprehend|DetectToxicContent|4xx->4xx|$.Message
Issue-Key: comprehend|StopTrainingEntityRecognizer|4xx->4xx|$.message
Issue-Commit: 1a8d6da9d066bb6b81849373d117fc388454286a

# Comprehend validation and limit error messages diverge from AWS wording

> _Note: this issue was identified against the code base at commit `1a8d6da9d066bb6b81849373d117fc388454286a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchDetectKeyPhrases

## Description
FIVE net-new root causes in the comprehend crate's input-validation message formatting. In every case the emulator REFUSES the request with the right HTTP status (400) and the right error class, so only the message text diverges — but the message is part of the contract: AWS's pre-handler validator has a fixed template per constraint kind, and the emulator uses templates of its own invention. Four of the five also LEAK the rejected value into the message, which AWS does not do for these members. A fixer must close ALL FIVE causes and re-replay all five steps.

AWS's validator templates, as the recordings show them:
  - collection-member constraint:  "<n> validation error detected: Value at '<memberName>' failed to satisfy constraint: Member must satisfy constraint: [Member must have length greater than or equal to <min>]"  — the member name is the COLLECTION ('textList'), never an indexed path, the nested constraint is wrapped in square brackets, and the rejected value does NOT appear.
  - enum constraint:               "<n> validation error detected: Value at '<memberName>' failed to satisfy constraint: Member must satisfy enum value set: [<values>]"  — no "Value '<v>' " prefix.
  - pattern constraint:            "<n> validation error detected: Value at '<memberName>' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern without the ^ and $ anchors>"
  - DetectToxicContent segment cap: "Please use a smaller number of segments in a single request. Maximum segments is <max>."
In all of these the fixed text is literal and the variable parts are the member name, the constraint bound and the enum/pattern body taken from the model and the request — never a value from one recording.

Cause 1 of 5: BatchDetectKeyPhrases reports an empty TextList member with an indexed path and drops AWS's bracketed nested-constraint wrapper. Fixture comprehend-batch-key-phrases-whole-call-refusals, step task#4.
  AWS: Smithy: BatchDetectKeyPhrases.TextList -> CustomerInputStringList, @length min 1, member CustomerInputString @length min 1 (and sensitive). An empty string in the list is refused with HTTP 400 and the message "1 validation error detected: Value at 'textList' failed to satisfy constraint: Member must satisfy constraint: [Member must have length greater than or equal to 1]". Fixed text: everything except the error count, the member name 'textList' and the bound 1. Note AWS names only the collection member and omits both the offending index and the offending value (the member is @sensitive).
  Emulator: HTTP 400 with "1 validation error detected: Value at 'textList.2.member' failed to satisfy constraint: Member must have length greater than or equal to 1" — it substitutes the indexed path 'textList.2.member' for 'textList' and emits the inner constraint directly instead of the "Member must satisfy constraint: [...]" wrapper.
  Why: clients and SDK test suites match on this message to classify a refusal, and operators grep for the member name. The indexed path makes the message vary with WHICH element was empty, so a client cannot match a stable string, and the missing wrapper means a parser written against AWS's two-level format fails to extract the nested constraint.
  Fix: the comprehend crate's request-validation message formatter for collection-member length constraints — report the collection member name, wrap the nested constraint in "Member must satisfy constraint: [...]", and do not include the index or the value.
  Traces: comprehend-batch-key-phrases-whole-call-refusals#task#4.

Cause 2 of 5: BatchDetectSyntax produces the same malformed collection-member message. Fixture comprehend-batch-syntax-whole-call-refusals, step task#3.
  AWS: Smithy: BatchDetectSyntax.TextList -> CustomerInputStringList, same @length min 1 on the list and its member. Recorded: HTTP 400, "1 validation error detected: Value at 'textList' failed to satisfy constraint: Member must satisfy constraint: [Member must have length greater than or equal to 1]".
  Emulator: HTTP 400 with "1 validation error detected: Value at 'textList.2.member' failed to satisfy constraint: Member must have length greater than or equal to 1" — identical deviation to Cause 1, confirming it is the shared formatter and not one handler, but it is a separate replay step and must be re-verified on its own.
  Why: as Cause 1. A fixer who repairs only the BatchDetectKeyPhrases path leaves BatchDetectSyntax answering the wrong text.
  Fix: same shared validation formatter; verify it is applied on the BatchDetectSyntax path too.
  Traces: comprehend-batch-syntax-whole-call-refusals#task#3.

Cause 3 of 5: DetectSentiment's enum-constraint message injects the rejected value and re-orders the value set. Fixture comprehend-detect-sentiment-language-code-refusals, step task#2.
  AWS: Smithy: DetectSentiment.LanguageCode is required and targets LanguageCode, an enum whose members are [en, es, fr, de, it, pt, ar, hi, ja, ko, zh, zh-TW]. An unmodelled code is refused with HTTP 400 and "1 validation error detected: Value at 'languageCode' failed to satisfy constraint: Member must satisfy enum value set: [hi, de, zh-TW, ko, pt, en, it, fr, zh, es, ar, ja]" — no "Value '<code>' " prefix, and the set is enumerated in AWS's own (non-model) order, which is stable for this member.
  Emulator: HTTP 400 with "1 validation error detected: Value 'nl' at 'languageCode' failed to satisfy constraint: Member must satisfy enum value set: [en, es, fr, de, it, pt, ar, hi, ja, ko, zh, zh-TW]" — it adds the "Value '<code>' " prefix AWS omits for this member and lists the set in model declaration order. The 12 accepted codes themselves are correct; only the rendering differs. ('nl' is this recording's input filling the template, not part of it — the requirement is that NO rejected value is interpolated.)
  Why: this message is compared verbatim by SDK protocol tests and by clients that assert on refusal text; the leading "Value 'nl' " also echoes caller input back into an error string, which AWS deliberately avoids here. The set order is part of the literal message a verbatim comparison sees.
  Fix: the comprehend crate's enum-constraint validation formatter — drop the value prefix for this member and emit the enum value set in the order AWS reports for LanguageCode.
  Traces: comprehend-detect-sentiment-language-code-refusals#task#2.

Cause 4 of 5: DetectToxicContent's segment-cap refusal uses an invented message instead of AWS's guidance sentence. Fixture comprehend-detect-toxic-content-segment-count-limit, step task#2.
  AWS: Smithy: DetectToxicContent.TextSegments is required, ListOfTextSegments @length min 1, sensitive; exceeding the per-request segment cap is refused with HTTP 400 carrying $.Message "Please use a smaller number of segments in a single request. Maximum segments is 10." Fixed text: the whole sentence pair; the only variable is the maximum (10 for DetectToxicContent).
  Emulator: HTTP 400 with $.Message "TextSegments carries 11 text strings, which exceeds the limit of 10 for DetectToxicContent." — a message of its own composition that also echoes the submitted segment count (11, this recording's input) and the operation name, neither of which appears in AWS's text.
  Why: the message is the only machine-visible explanation of the cap for a client that did not pre-count; batching clients key their back-off/split logic on it. AWS's wording also instructs the caller what to do ('use a smaller number of segments'), which the emulator's text does not.
  Fix: the DetectToxicContent handler's segment-count validation in the comprehend crate — emit AWS's fixed sentence with the operation's maximum substituted, and do not interpolate the submitted count.
  Traces: comprehend-detect-toxic-content-segment-count-limit#task#2.

Cause 5 of 5: StopTrainingEntityRecognizer's pattern-constraint message injects the rejected ARN and prints the regex with its ^/$ anchors. Fixture comprehend-stop-training-entity-recognizer-arn-refusals, step task#4.
  AWS: Smithy: StopTrainingEntityRecognizer.EntityRecognizerArn is required, EntityRecognizerArn @length 0..256 with @pattern "^arn:aws(-[^:]+)?:comprehend:[a-zA-Z0-9-]*:[0-9]{12}:entity-recognizer/[a-zA-Z0-9](-*[a-zA-Z0-9])*(/version/[a-zA-Z0-9](-*[a-zA-Z0-9])*)?$". A non-matching ARN is refused with HTTP 400 and "1 validation error detected: Value at 'entityRecognizerArn' failed to satisfy constraint: Member must satisfy regular expression pattern: arn:aws(-[^:]+)?:comprehend:[a-zA-Z0-9-]*:[0-9]{12}:entity-recognizer/[a-zA-Z0-9](-*[a-zA-Z0-9])*(/version/[a-zA-Z0-9](-*[a-zA-Z0-9])*)?" — the pattern is rendered WITHOUT the leading ^ and trailing $, and the rejected ARN is not echoed.
  Emulator: HTTP 400 with "1 validation error detected: Value 'arn:aws:comprehend:us-west-2:000000000000:document-classifier/sim2real-probe-cmprec-44e407a0' at 'entityRecognizerArn' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:aws(-[^:]+)?:comprehend:[a-zA-Z0-9-]*:[0-9]{12}:entity-recognizer/[a-zA-Z0-9](-*[a-zA-Z0-9])*(/version/[a-zA-Z0-9](-*[a-zA-Z0-9])*)?$" — it interpolates the submitted ARN and renders the Smithy pattern verbatim including the anchors. (That ARN string is this recording's input filling the template; the requirement is that no value is interpolated and the anchors are stripped.)
  Why: clients and tests assert on this message; the anchors and the echoed ARN both make it differ from AWS for every caller, and echoing a caller-supplied ARN into an error string is behavior AWS does not exhibit on this member.
  Fix: the comprehend crate's pattern-constraint validation formatter — strip the ^ and $ anchors when rendering the pattern and omit the rejected value.
  Traces: comprehend-stop-training-entity-recognizer-arn-refusals#task#4.

## Evidence
All five steps: expected HTTP 400, got HTTP 400 — status and error class match, the message body is the sole divergence. Replay verdict 'mismatch', stub_path EMPTY, owned_by EMPTY for each.

Cause 1 - comprehend-batch-key-phrases-whole-call-refusals#task#4, BatchDetectKeyPhrases, $.message:
  expected: "1 validation error detected: Value at 'textList' failed to satisfy constraint: Member must satisfy constraint: [Member must have length greater than or equal to 1]"
  got:      "1 validation error detected: Value at 'textList.2.member' failed to satisfy constraint: Member must have length greater than or equal to 1"

Cause 2 - comprehend-batch-syntax-whole-call-refusals#task#3, BatchDetectSyntax, $.message:
  expected: "1 validation error detected: Value at 'textList' failed to satisfy constraint: Member must satisfy constraint: [Member must have length greater than or equal to 1]"
  got:      "1 validation error detected: Value at 'textList.2.member' failed to satisfy constraint: Member must have length greater than or equal to 1"

Cause 3 - comprehend-detect-sentiment-language-code-refusals#task#2, DetectSentiment, $.message:
  expected: "1 validation error detected: Value at 'languageCode' failed to satisfy constraint: Member must satisfy enum value set: [hi, de, zh-TW, ko, pt, en, it, fr, zh, es, ar, ja]"
  got:      "1 validation error detected: Value 'nl' at 'languageCode' failed to satisfy constraint: Member must satisfy enum value set: [en, es, fr, de, it, pt, ar, hi, ja, ko, zh, zh-TW]"
  (two deviations: the "Value 'nl' " prefix AWS omits, and the set rendered in model order instead of AWS's order)

Cause 4 - comprehend-detect-toxic-content-segment-count-limit#task#2, DetectToxicContent, $.Message:
  expected: "Please use a smaller number of segments in a single request. Maximum segments is 10."
  got:      "TextSegments carries 11 text strings, which exceeds the limit of 10 for DetectToxicContent."

Cause 5 - comprehend-stop-training-entity-recognizer-arn-refusals#task#4, StopTrainingEntityRecognizer, $.message:
  expected: "1 validation error detected: Value at 'entityRecognizerArn' failed to satisfy constraint: Member must satisfy regular expression pattern: arn:aws(-[^:]+)?:comprehend:[a-zA-Z0-9-]*:[0-9]{12}:entity-recognizer/[a-zA-Z0-9](-*[a-zA-Z0-9])*(/version/[a-zA-Z0-9](-*[a-zA-Z0-9])*)?"
  got:      "1 validation error detected: Value 'arn:aws:comprehend:us-west-2:000000000000:document-classifier/sim2real-probe-cmprec-44e407a0' at 'entityRecognizerArn' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:aws(-[^:]+)?:comprehend:[a-zA-Z0-9-]*:[0-9]{12}:entity-recognizer/[a-zA-Z0-9](-*[a-zA-Z0-9])*(/version/[a-zA-Z0-9](-*[a-zA-Z0-9])*)?$"
  (two deviations: the interpolated ARN, and the ^/$ anchors retained from the Smithy @pattern)

## Steps to Reproduce
1. Clone the repo, boot the emulator from the checkout and replay the wire corpus (replay_wire_corpus). All five fixtures replay with no blocked or errored step; replay reports stub_path EMPTY and owned_by EMPTY for the five steps below. Every one of them returns the expected HTTP 400, so compare the message bodies only.
2. Cause 1: comprehend-batch-key-phrases-whole-call-refusals step task#4 - BatchDetectKeyPhrases with an empty string among the TextList entries. Read $.message: the member is reported as 'textList.2.member' instead of 'textList' and the "Member must satisfy constraint: [...]" wrapper is missing.
3. Cause 2: comprehend-batch-syntax-whole-call-refusals step task#3 - BatchDetectSyntax with an empty string among the TextList entries. $.message shows the same two deviations; verify it independently of Cause 1 (shared formatter, separate replay key).
4. Cause 3: comprehend-detect-sentiment-language-code-refusals step task#2 - DetectSentiment with a LanguageCode outside the modelled enum. $.message begins "1 validation error detected: Value 'nl' at 'languageCode' ..." where AWS has no value prefix, and the enum value set is printed in model order rather than AWS's order.
5. Cause 4: comprehend-detect-toxic-content-segment-count-limit step task#2 - DetectToxicContent with more TextSegments than the cap. $.Message is the emulator's own sentence naming the submitted count and the operation, not AWS's "Please use a smaller number of segments in a single request. Maximum segments is 10."
6. Cause 5: comprehend-stop-training-entity-recognizer-arn-refusals step task#4 - StopTrainingEntityRecognizer with an ARN that does not match the entity-recognizer pattern. $.message interpolates the submitted ARN and prints the @pattern with its leading ^ and trailing $, both of which AWS's rendering omits.
7. After the fix, re-replay ALL FIVE steps: comprehend-batch-key-phrases-whole-call-refusals#task#4, comprehend-batch-syntax-whole-call-refusals#task#3, comprehend-detect-sentiment-language-code-refusals#task#2, comprehend-detect-toxic-content-segment-count-limit#task#2 and comprehend-stop-training-entity-recognizer-arn-refusals#task#4.
