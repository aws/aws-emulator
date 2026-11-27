Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: wisdom.one-upload-backs-independent-contents wisdom.one-upload-backs-two-independent-contents wisdom.one-upload-id-backs-two-contents-with-same-document wisdom.create-content-upload-from-other-knowledge-base-rejected wisdom.create-content-upload-id-from-another-knowledge-base-rejected wisdom.create-content-upload-id-from-other-knowledge-base-rejected wisdom.update-content-upload-id-from-other-knowledge-base-rejected wisdom.content-upload-put-without-signed-headers-rejected wisdom.query-assistant-max-results-truncates-without-next-token wisdom.query-assistant-unissued-next-token-rejected wisdom.update-content-title-over-255-chars-rejected wisdom.update-content-metadata-eleven-entries-rejected wisdom.search-content-unissued-next-token-rejected
Issue-Shape-Id: com.amazonaws.wisdom#WisdomService
Issue-Key: wisdom|CreateContent|ValidationException
Issue-Key: wisdom.create-content-upload-from-other-knowledge-base-rejected|run#7|AssertionError|assert (code, message) == ("ValidationException", "Invalid Upload Id"), (
Issue-Key: wisdom.create-content-upload-id-from-another-knowledge-base-rejected|run#12|AssertionError|assert got == ("ValidationException", "Invalid Upload Id"), (
Issue-Key: wisdom.create-content-upload-id-from-other-knowledge-base-rejected|run#7|AssertionError|assert got == ("ValidationException", "Invalid Upload Id"), (
Issue-Key: wisdom.update-content-upload-id-from-other-knowledge-base-rejected|run#9|AssertionError|assert got == ("ValidationException", "Invalid Upload Id"), (
Issue-Key: wisdom.content-upload-put-without-signed-headers-rejected|_xml_error#11|ParseError|root = ET.fromstring(payload)
Issue-Key: wisdom.query-assistant-max-results-truncates-without-next-token|run#19|AssertionError|assert "nextToken" not in narrow, "maxResults=1 carried a nextToken although more documents matched"
Issue-Key: wisdom.query-assistant-unissued-next-token-rejected|run#13|AssertionError|raise AssertionError("the token-free query never served the document it was written with")
Issue-Key: wisdom.update-content-title-over-255-chars-rejected|run#16|AssertionError|assert got == (
Issue-Key: wisdom.update-content-metadata-eleven-entries-rejected|run#20|AssertionError|assert len(entries) == len(want) and set(entries) == want, (
Issue-Key: wisdom.search-content-unissued-next-token-rejected|run#15|AssertionError|assert got == ("ValidationException", "Invalid parameter"), (
Issue-Commit: d2c660dcba0958f9f444e52348f6b67098dedf4a

# wisdom: eight divergences in the content-upload plane, CreateContent/UpdateContent refusals and QueryAssistant/SearchContent paging

> _Note: this issue was identified against the code base at commit `d2c660dcba0958f9f444e52348f6b67098dedf4a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateContent

## Description
This issue bundles EIGHT distinct net-new root causes in the `wisdom` service (service heading `### wisdom` of the behavioral suite). All eight must be closed before submitting; each cause names its own failing checks.

=== Cause 1 of 8: CreateContent SPENDS the uploadId, so one upload cannot back more than one content ===
AWS: recorded behavior only. The model says only that `CreateContent$uploadId` (UploadId, @length 1..1200, required) is "A pointer to the uploaded asset. This value is returned by StartContentUpload" — nothing in the model makes that pointer single-use. The recordings these checks replay show the SAME uploadId accepted by two successive CreateContent calls in the issuing knowledge base, each producing an independent content (independent contentId/revisionId, each serving the uploaded bytes, deleting one leaving the other ACTIVE and readable).
Emulator: the first CreateContent succeeds and then deletes the receipt (`services/service-com-amazonaws-wisdom-wisdomservice/src/ops/create_content.rs`, `state.content_uploads.remove(&input.upload_id)`), so the second CreateContent with the same uploadId is refused 400 `ValidationException` with message `Invalid uploadId`. UpdateContent does the same with its replacement upload.
Why: a client that fans one uploaded asset out into several contents (the documented way to publish one document under several names) gets a hard ValidationException on every call after the first, and the asset it already PUT is unreachable.
Fix: the uploadId lifecycle in the CreateContent handler and its state (the receipt must stay resolvable after a create; the same applies to UpdateContent's replacement path).
Traces: wisdom.one-upload-backs-independent-contents, wisdom.one-upload-backs-two-independent-contents, wisdom.one-upload-id-backs-two-contents-with-same-document.

=== Cause 2 of 8: the invalid-uploadId ValidationException message is worded `Invalid uploadId` instead of `Invalid Upload Id` ===
AWS: recorded behavior only — the recorded refusal of a CreateContent/UpdateContent naming an uploadId that is not an upload of the addressed knowledge base is 400 `ValidationException` with the message `Invalid Upload Id`. The message is entirely FIXED text: it carries no value from the request or from state.
Emulator: same status and code, but the message is `Invalid uploadId` (`crate::state::MESSAGE_INVALID_UPLOAD_ID`), used by both create_content.rs and update_content.rs.
Why: a client that classifies refusals by message text (or an operator reading logs) sees a sentence AWS never sends for this class.
Fix: the fixed-message constant the CreateContent/UpdateContent handlers refuse this class with.
Traces: wisdom.create-content-upload-from-other-knowledge-base-rejected, wisdom.create-content-upload-id-from-another-knowledge-base-rejected, wisdom.create-content-upload-id-from-other-knowledge-base-rejected, wisdom.update-content-upload-id-from-other-knowledge-base-rejected.

=== Cause 3 of 8: the presigned content-upload URL refuses an unsigned PUT with a JSON body and the wrong error code, not an XML Error document ===
AWS: recorded behavior only. A PUT to the URL StartContentUpload published that omits the headers `headersToInclude` named is refused 403 with an XML error document whose `Code` is `SignatureDoesNotMatch` and whose `Message` is the fixed sentence `The request signature we calculated does not match the signature you provided. Check your key and signing method.` (fixed text, no variable part). The same URL then accepts the PUT that carries those headers.
Emulator: the status is 403, but the body is JSON — `{"message":"The request does not include the headers the upload was started with","code":"AccessDenied"}` with `content-type: application/json` (`src/service.rs`, `upload_error`) — so a client parsing the refusal gets `xml.etree.ElementTree.ParseError: not well-formed (invalid token): line 1, column 0`, and even when parsed the code is `AccessDenied` and the sentence is the emulator's own.
Why: every client of a presigned upload URL (the AWS SDKs, the CLI, any S3-style uploader) parses that refusal as XML; it cannot read this body at all, and retry/signature-refresh logic keyed on `SignatureDoesNotMatch` never fires.
Fix: the upload/asset data plane's error renderer (the refusal envelope of the content plane in the service's `service.rs`): render an XML `<Error>` document with the S3-style Code/Message for this class.
Traces: wisdom.content-upload-put-without-signed-headers-rejected.

=== Cause 4 of 8: QueryAssistant issues a nextToken when it truncates to maxResults ===
AWS: recorded behavior only — QueryAssistant with `maxResults=1` over three matching documents returns one result and NO `nextToken`, even though more documents matched; the model declares `nextToken` as output but the recording shows it absent for a truncated query (and absent for the full page too).
Emulator: returns the truncated page WITH a `nextToken` (query_assistant.rs computes `next_token` as `ranked.get(start + page_size)`), so the assertion `maxResults=1 carried a nextToken although more documents matched` fails.
Why: a client (or a botocore paginator) follows that token and walks pages AWS never offers, producing results — and request volume — a real account would not.
Fix: the QueryAssistant handler's paging decision (do not emit the output token when the ranked matches are truncated).
Traces: wisdom.query-assistant-max-results-truncates-without-next-token.

=== Cause 5 of 8: QueryAssistant serves NO result for an ACTIVE content whose text carries the query term ===
AWS: recorded behavior only — an assistant with a `KNOWLEDGE_BASE` association to a knowledge base holding one content whose uploaded text/plain asset contains the query term answers QueryAssistant (no nextToken, maxResults=10) with exactly that content's `results[].document.contentReference.contentId`.
Emulator: the token-free query returns an empty `results` list for the whole polling window (or answers `ResourceNotFoundException`, which the check's helper reads as empty), so the check fails with `the token-free query never served the document it was written with` and never reaches its forged-token assertion. This is not an ingestion-delay artifact: the content becomes ACTIVE 500 ms after the create (`state::CONTENT_ACTIVATION_DELAY`), and a sibling check with three documents in one knowledge base does get them served, so a single-document assistant is answered differently.
Why: the primary read path of the service answers nothing; a client that writes a document and queries for it concludes the knowledge base is empty.
Fix: the QueryAssistant handler's candidate selection and match path (the association walk, the ACTIVE filter and the searched-text selection) — and the assistant resolution that can turn this into a not-found.
Traces: wisdom.query-assistant-unissued-next-token-rejected.

=== Cause 6 of 8: an UpdateContent title past @length 255 is refused as BadRequestException `Invalid request body` instead of ValidationException ===
AWS: Smithy — `UpdateContent$title` targets `ContentTitle` with @length min 1 max 255, and UpdateContent declares `ValidationException` (HTTP 400) for "The input fails to satisfy the constraints specified by a service"; `BadRequestException` is declared by NO operation of this model. The recorded refusal of a 256-character title is 400 `ValidationException` with AWS's standard constraint sentence: `1 validation error detected: Value '<the title sent>' at 'title' failed to satisfy constraint: Member must have length less than or equal to 255` — fixed text apart from the value (taken from the request), the member path `title` and the limit `255` (from the model).
Emulator: answers `('BadRequestException', 'Invalid request body')`, i.e. the deserialization-boundary wording, for a plain @length violation. Hypothesis: this class is reaching the front-door body-read refusal (`service_common::front_door::INVALID_REQUEST_BODY` via the `override_refusal` seam) instead of the constraint renderer `service_common::aws_validation_refusal` the service comments say owns it; the accepted 255-character update immediately before it succeeds, so only the over-length path is affected.
Why: the error code is one no AWS model declares, so a client that catches `ValidationException` (botocore's modeled error class) does not catch it at all, and the message names neither the member nor the limit.
Fix: the refusal classification for UpdateContent's modeled @length violations (the service's `override_refusal` seam / the boundary validator it delegates to).
Traces: wisdom.update-content-title-over-255-chars-rejected.

=== Cause 7 of 8: the metadata @length refusal omits the rendered map value from the message ===
AWS: Smithy — `UpdateContent$metadata` targets `ContentMetadata` with @length min 0 max 10. The recorded refusal of an eleven-entry metadata map is 400 `ValidationException` whose message RENDERS the rejected map: `1 validation error detected: Value '{<k>=<v>, <k>=<v>, …}' at 'metadata' failed to satisfy constraint: Member must have length less than or equal to 10` — fixed text plus the entries taken from the request (entry order is not documented), the member path `metadata` and the limit `10` from the model.
Emulator: the refusal's message carries no `{…}` rendering at all — parsing the braces out of it yields the single empty entry `['']` against the eleven expected `attr00=value-0 … attr10=one-too-many` — so the value half of the clause is missing (the shared constraint renderer deliberately states a collection's @length clause "without a value half").
Why: a client or operator cannot see WHICH map was rejected, and any check or log parser keyed on AWS's sentence shape fails.
Fix: the constraint-refusal renderer for collection @length violations on this service (restore the rendered map as the value half).
Traces: wisdom.update-content-metadata-eleven-entries-rejected.

=== Cause 8 of 8: SearchContent's unissued-nextToken refusal is worded with the emulator's own sentence ===
AWS: recorded behavior only — SearchContent with a `nextToken` the service never issued (well-formed by the model's @length 1..2048) is refused 400 `ValidationException` with the fixed message `Invalid parameter`, and the same search without the token still lists the content.
Emulator: same status and code but the message is `nextToken: the specified token is not valid.` Note the SearchContent handler itself does not refuse an unknown token (it treats it as a resume point), so this sentence comes from the paging-token validator in front of the handler.
Why: the sentence is one AWS never sends for this class; a client keying on the message text mis-classifies the refusal.
Fix: the wording of the unissued-nextToken refusal for SearchContent (the paging-token validator / the service's refusal wording seam).
Traces: wisdom.search-content-unissued-next-token-rejected.

Note: the two `cloudformation__wisdom` failures of this run are marked OWNED by 20261109T223435Z-cloudformation-wisdom-cloudformation-does-not-register-aws-wisdom.md and are not re-filed here.

## Evidence
Suite: 54/69 passed; service `wisdom` 54/67 (81%), 13 failed. Verbatim failures:

1) [error] wisdom.one-upload-backs-independent-contents / one-upload-backs-two-independent-contents / one-upload-id-backs-two-contents-with-same-document:
   botocore.errorfactory.ValidationException: An error occurred (ValidationException) when calling the CreateContent operation: Invalid uploadId
   (raised on the SECOND CreateContent that names the same, same-knowledge-base uploadId; expected: a second independent content, each serving the uploaded bytes)

2) [mismatch] wisdom.create-content-upload-from-other-knowledge-base-rejected:
   AssertionError: CreateContent with another base's uploadId gave ('ValidationException', 'Invalid uploadId')   — expected ('ValidationException', 'Invalid Upload Id')
   [mismatch] wisdom.create-content-upload-id-from-another-knowledge-base-rejected: same, expected 'Invalid Upload Id'
   [mismatch] wisdom.create-content-upload-id-from-other-knowledge-base-rejected: AssertionError: expected ('ValidationException', 'Invalid Upload Id') for another knowledge base's upload id, got ('ValidationException', 'Invalid uploadId')
   [mismatch] wisdom.update-content-upload-id-from-other-knowledge-base-rejected: same expected/actual pair on UpdateContent

3) [error] wisdom.content-upload-put-without-signed-headers-rejected:
   xml.etree.ElementTree.ParseError: not well-formed (invalid token): line 1, column 0   (in _xml_error after the 403 status assertion passed)
   expected: 403 + XML Error with ('SignatureDoesNotMatch', 'The request signature we calculated does not match the signature you provided. Check your key and signing method.')
   emulator (src/service.rs upload_error): JSON {"message":"The request does not include the headers the upload was started with","code":"AccessDenied"}, content-type application/json

4) [mismatch] wisdom.query-assistant-max-results-truncates-without-next-token:
   AssertionError: maxResults=1 carried a nextToken although more documents matched

5) [mismatch] wisdom.query-assistant-unissued-next-token-rejected:
   AssertionError: the token-free query never served the document it was written with
   (QueryAssistant(queryText='orchard', maxResults=10) on an assistant associated with the knowledge base holding one text/plain content whose body is 'The orchard is pruned in late winter before the buds open.' returned no results for the whole polling window; CONTENT_ACTIVATION_DELAY is 500 ms)

6) [mismatch] wisdom.update-content-title-over-255-chars-rejected:
   AssertionError: a 256-char title gave ('BadRequestException', 'Invalid request body')
   expected ('ValidationException', "1 validation error detected: Value '<256-char title>' at 'title' failed to satisfy constraint: Member must have length less than or equal to 255")

7) [mismatch] wisdom.update-content-metadata-eleven-entries-rejected:
   AssertionError: the refusal echoed metadata entries [''], expected ['attr00=value-0', 'attr01=value-1', 'attr02=value-2', 'attr03=value-3', 'attr04=value-4', 'attr05=value-5', 'attr06=value-6', 'attr07=value-7', 'attr08=value-8', 'attr09=value-9', 'attr10=one-too-many']
   expected message shape: "1 validation error detected: Value '{<entries>}' at 'metadata' failed to satisfy constraint: Member must have length less than or equal to 10"

8) [mismatch] wisdom.search-content-unissued-next-token-rejected:
   AssertionError: expected ('ValidationException', 'Invalid parameter') for an unissued token, got ('ValidationException', 'nextToken: the specified token is not valid.')

## Steps to Reproduce
Boot the emulator and replay the 13 failing wisdom checks (trace_ids above). Per cause:

1) CreateKnowledgeBase(knowledgeBaseType=CUSTOM) -> StartContentUpload(contentType=text/plain) -> PUT the body to the published URL with headersToInclude -> CreateContent(uploadId=<id>, name=A) (succeeds) -> CreateContent(uploadId=<same id>, name=B): observe 400 ValidationException 'Invalid uploadId'; AWS creates a second independent content.
2) Two CUSTOM knowledge bases; StartContentUpload in KB1, then CreateContent (or UpdateContent) in KB2 naming that uploadId: observe the message 'Invalid uploadId' where AWS sends 'Invalid Upload Id'.
3) StartContentUpload, then PUT to slot['url'] with NO headers: observe 403 with a JSON body/code AccessDenied; AWS answers an XML <Error> with SignatureDoesNotMatch and S3's fixed sentence.
4) Create an assistant + KNOWLEDGE_BASE association, upload three documents sharing a term, QueryAssistant(queryText=<term>, maxResults=1): observe a nextToken in the response.
5) Same setup with ONE document whose text/plain body contains the term; QueryAssistant(queryText=<term>, maxResults=10) repeatedly: observe results == [] (the document is ACTIVE within 500 ms).
6) Create a content, UpdateContent(title=<255 chars>) (accepted), then UpdateContent(title=<256 chars>): observe ('BadRequestException','Invalid request body').
7) UpdateContent(metadata=<10 entries>) (accepted), then UpdateContent(metadata=<11 entries>): observe a ValidationException message with no rendered '{k=v, ...}' value.
8) SearchContent(searchExpression=NAME EQUALS <name>) (one match, no nextToken), then SearchContent(nextToken='sim2real-never-issued-page-token'): observe the message 'nextToken: the specified token is not valid.'
