Issue-Source: trace-sweep-divergence
Issue-Trace: textract-analyze-expense-document-without-source-rejected
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: textract-analyze-expense-document-without-source-rejected textract-detect-document-text-document-without-source textract-start-text-detection-parameter-refusals
Issue-Shape-Id: com.amazonaws.textract#Textract
Issue-Key: textract|AnalyzeExpense|4xx->4xx|$.Message
Issue-Key: textract|DetectDocumentText|4xx->4xx|$.Message
Issue-Key: textract|StartDocumentTextDetection|4xx->4xx|$.Message
Issue-Commit: 599f5e0fd2976b77e672c15894a37066c05508bc

# Textract refusal messages collapse to the generic 'Request has invalid parameters' sentence instead of AWS's per-parameter wording

> _Note: this issue was identified against the code base at commit `599f5e0fd2976b77e672c15894a37066c05508bc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AnalyzeExpense

## Description
This issue reports 3 distinct root causes in the textract service, all of the same class: a request that violates an input rule is refused with the right HTTP status (400) and the right error code (InvalidParameterException), but the Message is the emulator's one-size-fits-all sentence instead of the sentence Amazon Textract states that particular refusal with. A fixer must close ALL THREE before submitting.

Already-filed work that does NOT cover anything below: the open 20261127T131328Z-textract-document-limit-refusals-and-pdf-reading-diverge.md is a BEHAVIORAL issue about DocumentTooLargeException wording, the inline-bytes ceiling and PDF reading; it carries no replay key for any wire fixture listed here and says nothing about the document-source or DocumentLocation refusal sentences. The wire divergences for DetectDocumentText/AnalyzeDocument Blocks geometry in this same replay were marked OWNED (20261125T053817Z, 20261126T113734Z) and are not part of this issue.

Cause 1 of 3 — AnalyzeExpense with a Document that carries neither Bytes nor S3Object is refused with the generic constraint sentence instead of the document-source sentence.
AWS: Smithy (services/service-com-amazonaws-textract-textract/smithy-model/com-amazonaws-textract-textract.json): AnalyzeExpense takes a required Document member whose shape has exactly two members, Bytes and S3Object, and models InvalidParameterException (client error, HTTP 400, members Message/Code) documented "An input parameter violated a constraint. For example, in synchronous operations, an InvalidParameterException exception occurs when neither of the S3Object or Bytes values are supplied in the Document request parameter." The recorded response for that request is HTTP 400, __type InvalidParameterException, Message "Requested document should either contain bytes or s3 object." — that sentence is WHOLLY FIXED text for this refusal class (including the trailing period); it has no variable part, echoes nothing from the request, and is distinct from the generic constraint sentence AWS uses for ordinary length/pattern/enum violations.
Emulator: HTTP 400 InvalidParameterException with Message "Request has invalid parameters". The source is crate state's single constant INVALID_PARAMETERS_MESSAGE (services/service-com-amazonaws-textract-textract/src/state.rs line 267), returned by reference_for_document for the (None, None) and (Some, Some) cases (state.rs lines 1065-1067) and lifted into the modeled error by the handler (src/ops/analyze_expense.rs lines 206-217).
Why: a client that classifies or surfaces the refusal by its message — the only part of this 400 that says WHICH rule was broken, since AWS's generic sentence names neither member nor value — cannot tell "you supplied no document source" from any other constraint violation. Error-matching, user-facing messaging and support triage built on AWS's wording all break while status and code look correct.
Fix: the document-source check in the textract crate's shared state module (state.rs reference_for_document) must carry its OWN sentence for a Document naming neither (or both) source(s), rather than reusing the generic constraint constant; the per-operation handlers already map that Err into the modeled InvalidParameterException and need no change.
Traces: textract-analyze-expense-document-without-source-rejected#task#1.

Cause 2 of 3 — DetectDocumentText with a Document that carries neither Bytes nor S3Object has the same wrong sentence.
AWS: Smithy: DetectDocumentText takes the same required Document and models the same InvalidParameterException at HTTP 400. Recorded response: HTTP 400 InvalidParameterException, Message "Requested document should either contain bytes or s3 object." — the identical fixed sentence, so the requirement is shared by every synchronous read path (AnalyzeExpense, DetectDocumentText, AnalyzeDocument), not special to one operation.
Emulator: HTTP 400 InvalidParameterException "Request has invalid parameters", from the same reference_for_document branch (state.rs lines 1065-1067) via src/ops/detect_document_text.rs.
Why: same as Cause 1, on the most used synchronous operation; a caller validating its own wiring against the error text sees a message AWS never sends for a missing document source.
Fix: once reference_for_document states the document-source refusal with AWS's sentence, this path is fixed with it — the fix belongs in that shared validation helper, not duplicated per handler.
Traces: textract-detect-document-text-document-without-source#task#2.

Cause 3 of 3 — StartDocumentTextDetection with a DocumentLocation whose S3Object bucket/name are not usable is refused with the generic sentence instead of the DocumentLocation sentence.
AWS: Smithy: StartDocumentTextDetection takes a required DocumentLocation whose only member is S3Object (Bucket, @length 3..255 @pattern ^[0-9A-Za-z\.\-_]*$; Name, @length 1..1024 @pattern \S), and models InvalidParameterException at HTTP 400. Recorded response for a submit whose DocumentLocation does not carry usable bucket/name values: HTTP 400 InvalidParameterException with Message "Document Location parameter must contain valid bucket/name values" — fixed text for this refusal class, naming the input member (Document Location) and the two values it must contain; no part of it is taken from the request, so neither the bucket nor the name is echoed.
Emulator: HTTP 400 InvalidParameterException "Request has invalid parameters", produced by reference_for_location (state.rs lines 1084-1088, absent S3Object) and by s3_ref's (None, _) | (_, None) arm (state.rs line 1112, absent/empty Bucket or Name), both of which return INVALID_PARAMETERS_MESSAGE; the handler lifts it at src/ops/start_document_text_detection.rs lines 243-256.
Why: the asynchronous submit is where a caller's S3 wiring is validated. AWS's message tells it the DOCUMENT LOCATION is at fault; the emulator's says only that something in the request was invalid, so a caller cannot distinguish a bad bucket/name from any other constraint violation on the same call, and retry logic keyed on the sentence misroutes.
Fix: the asynchronous location validation in the textract crate's state module (state.rs reference_for_location and the s3_ref bucket/name check it calls) must state this refusal with AWS's DocumentLocation sentence; the Start* handlers already answer it as InvalidParameterException. The same helper serves StartDocumentAnalysis, so fixing it there keeps the two submits consistent.
Traces: textract-start-text-detection-parameter-refusals#task#1.

## Evidence
Wire corpus simparity-wire-aws-v1 replayed against the emulator booted from this checkout: 16 fixtures replayed, 73 steps, 47 passed, 26 mismatched, 0 errored, 0 blocked, score 64.4%, bootstrap_signal false (textract answered every call), 7 divergence clusters of which 2 were marked OWNED and skipped.

Cause 1 — fixture textract-analyze-expense-document-without-source-rejected, step task#1, operation AnalyzeExpense, verdict mismatch, expected_status 400, actual_status 400:
  $.Message: value_mismatch (expected 'Requested document should either contain bytes or s3 object.', got 'Request has invalid parameters')
  Status and error code agree; only the sentence diverges.

Cause 2 — fixture textract-detect-document-text-document-without-source, step task#2, operation DetectDocumentText, verdict mismatch, expected_status 400, actual_status 400:
  $.Message: value_mismatch (expected 'Requested document should either contain bytes or s3 object.', got 'Request has invalid parameters')

Cause 3 — fixture textract-start-text-detection-parameter-refusals, step task#1, operation StartDocumentTextDetection, verdict mismatch, expected_status 400, actual_status 400:
  $.Message: value_mismatch (expected 'Document Location parameter must contain valid bucket/name values', got 'Request has invalid parameters')

In all three the expected string is the fixed text of the refusal class — none of it is a per-run value (no bucket name, object key, request id or timestamp appears in it), so the comparison is a requirement and not an artifact of one recording.

## Steps to Reproduce
1. Build and boot the emulator from this checkout and replay the simparity-wire-aws-v1 wire corpus (textract fixtures).
2. Cause 1 — call AnalyzeExpense with Document={} (neither Bytes nor S3Object). Observe HTTP 400 InvalidParameterException with Message 'Request has invalid parameters'; AWS answers 'Requested document should either contain bytes or s3 object.'. Repeat with BOTH Bytes and S3Object supplied to confirm the same branch is taken.
3. Cause 2 — call DetectDocumentText with Document={} and observe the same wrong sentence; also check AnalyzeDocument, which shares reference_for_document.
4. Cause 3 — call StartDocumentTextDetection with DocumentLocation={'S3Object': {'Bucket': '', 'Name': ''}} (and with S3Object omitted entirely). Observe HTTP 400 InvalidParameterException 'Request has invalid parameters'; AWS answers 'Document Location parameter must contain valid bucket/name values'. Check StartDocumentAnalysis for the same helper.
5. After the fix, re-replay textract-analyze-expense-document-without-source-rejected#task#1, textract-detect-document-text-document-without-source#task#2 and textract-start-text-detection-parameter-refusals#task#1 and require every step of each fixture to pass, including the steps that already pass today.
