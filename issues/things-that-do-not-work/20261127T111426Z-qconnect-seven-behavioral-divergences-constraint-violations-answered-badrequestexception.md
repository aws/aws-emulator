Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: qconnect.create-content-eleven-metadata-entries-rejected qconnect.update-content-metadata-over-10-entries-rejected qconnect.update-content-title-over-255-chars-rejected qconnect.rename-quick-response-to-taken-name-rejected qconnect.update-quick-response-to-taken-name-rejected qconnect.search-content-next-token-it-never-issued-rejected qconnect.search-content-unissued-next-token-rejected qconnect.start-import-job-missing-required-headers-rejected qconnect.retrieve-unknown-association-refused-beside-answered-one qconnect.send-message-signed-twenty-minutes-ago-rejected
Issue-Shape-Id: com.amazonaws.qconnect#WisdomService
Issue-Key: qconnect.create-content-eleven-metadata-entries-rejected|run#14|AssertionError|assert (code, message) == ("ValidationException", expected), (
Issue-Key: qconnect.update-content-metadata-over-10-entries-rejected|run#17|AssertionError|assert (code, message) == (
Issue-Key: qconnect.update-content-title-over-255-chars-rejected|run#16|AssertionError|assert got == (
Issue-Key: qconnect.rename-quick-response-to-taken-name-rejected|run#10|AssertionError|assert (code, message) == ("ConflictException", "Name is already in use"), (
Issue-Key: qconnect.update-quick-response-to-taken-name-rejected|run#13|AssertionError|assert got == ("ConflictException", "Name is already in use"), f"renaming onto a held name answered {got!r}"
Issue-Key: qconnect.search-content-next-token-it-never-issued-rejected|_refusal#18|AssertionError|raise AssertionError(f"the call was expected to be refused but returned {answer!r}")
Issue-Key: qconnect.search-content-unissued-next-token-rejected|run#19|AssertionError|assert got == ("ValidationException", "Invalid parameter"), f"an unissued nextToken was answered with {got!r}"
Issue-Key: qconnect|StartContentUpload|ValidationException
Issue-Key: qconnect.retrieve-unknown-association-refused-beside-answered-one|run#18|AssertionError|assert ingestion == "SYNC_SUCCESS", f"ingestion settled in {ingestion!r}, expected 'SYNC_SUCCESS'"
Issue-Key: qconnect.send-message-signed-twenty-minutes-ago-rejected|_assert_expired#13|AssertionError|assert code == "InvalidSignatureException", f"{label}: error type {code!r}, body {body!r}"
Issue-Commit: c4056ad47c33d82108e7436e28d6f183e635f0d8

# qconnect: seven behavioral divergences — constraint violations answered BadRequestException instead of ValidationException, wrong ConflictException/nextToken wording, text/csv upload refused, ingestionStatus never served, expired SigV4 answered RequestTimeTooSkewed (7 causes)

> _Note: this issue was identified against the code base at commit `c4056ad47c33d82108e7436e28d6f183e635f0d8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateContent, CreateContent, UpdateQuickResponse, SearchContent, StartContentUpload, GetKnowledgeBase

## Description
Seven distinct root causes in the qconnect (Wisdom) emulator, all grounded in failing behavioral checks. The emulator is bootstrapped and answering (37/58 qconnect checks pass), so these are fidelity defects, not bootstrap gaps. Cause 1 of 7 covers three checks that share one cause; the rest are one check each.

=== Cause 1 of 7: modeled @length constraint violations are refused with an unmodeled BadRequestException / 'Invalid request body' instead of ValidationException with AWS's constraint-violation message ===
Failing checks: qconnect.create-content-eleven-metadata-entries-rejected, qconnect.update-content-metadata-over-10-entries-rejected, qconnect.update-content-title-over-255-chars-rejected.
AWS: Smithy model (services/service-com-amazonaws-qconnect-wisdomservice/smithy-model) — CreateContent.metadata and UpdateContent.metadata target ContentMetadata with @length {min:0, max:10}; UpdateContent.title targets ContentTitle with @length {min:1, max:255}. Both operations model ValidationException (errorKind client, httpError 400) and neither models any BadRequestException — 'BadRequestException' is not an error of these operations at all. AWS refuses a request that breaks a modeled constraint with HTTP 400, error code ValidationException, and the standard constraint-violation message whose fixed text is "<N> validation error detected: Value '<rendered value>' at '<member name>' failed to satisfy constraint: Member must have length less than or equal to <max>"; the variable parts are the violation count, the offending value as the service renders it (for a map: the entries joined as 'key=value, key=value' inside braces), the member name taken from the request ('metadata', 'title'), and the modeled maximum (10, 255). Nothing in the model or the recordings fixes the map's rendering order; the create-content check notes AWS renders the refused map in its own fixed key order rather than the request's, so the entry ORDER is not a requirement of the fix, but the message shape is.
Emulator: all three requests are refused with error code 'BadRequestException' and message 'Invalid request body'. The requests never reach the operation's own validation — the generic deserialize/validate stage of the shared request walk (documented in ops/update_content.rs: "deserialize → validate → handle → serialize") rejects the body wholesale.
Why: a client catching qconnect's modeled ValidationException (botocore exposes it as a typed exception on the client) does not catch BadRequestException, so a constraint violation surfaces as an unexpected error class and the per-member diagnostic — which member failed, which constraint, which limit — is lost. Code that validates input by round-tripping a rejection, or that parses the 'N validation error detected ... at <member>' message to tell the user which field is wrong, breaks.
Fix: the shared constraint-validation / error-mapping layer that the centralized request walk uses before `handle`, so a Smithy @length (and @range/@pattern) violation on any qconnect input member is mapped to the service's modeled ValidationException with AWS's constraint-violation message instead of the generic body-parse BadRequestException. Handlers: services/service-com-amazonaws-qconnect-wisdomservice/src/ops/create_content.rs and .../src/ops/update_content.rs. Note: both checks here also assert the positive path (10-entry metadata, 255-char title) is accepted and that the refused call leaves metadata/title and revisionId unchanged; those assertions passed, so only the refusal class and wording diverge. (Prior fixed issue 20261029T203651Z-qconnect-updatecontent-stub.md implemented UpdateContent itself; this is not a regression of that fix — the valid update succeeds — only the refusal's class and message diverge.)
Traces: qconnect.create-content-eleven-metadata-entries-rejected, qconnect.update-content-metadata-over-10-entries-rejected, qconnect.update-content-title-over-255-chars-rejected.

=== Cause 2 of 7: UpdateQuickResponse's duplicate-name ConflictException carries the wrong message text ===
Failing checks: qconnect.rename-quick-response-to-taken-name-rejected, qconnect.update-quick-response-to-taken-name-rejected.
AWS: Smithy model — UpdateQuickResponse models ConflictException (errorKind client, httpError 409). Recorded behavior: renaming a quick response onto a name another quick response in the same knowledge base holds is refused 409 ConflictException with the message 'Name is already in use' — that is the whole fixed text; it carries no variable part (it does not name the quick response or the colliding name).
Emulator: the refusal has the right code (ConflictException) but the message 'The value for name is already in use. Please update and try again.' — different fixed text.
Why: a client that matches on the message to distinguish a name collision from the other conflicts ConflictException covers (a concurrent create/mutate of the same resource) mis-classifies it, and any log/report that echoes the service message shows wording AWS never sends.
Fix: the duplicate-name conflict path of the UpdateQuickResponse handler, services/service-com-amazonaws-qconnect-wisdomservice/src/ops/update_quick_response.rs. Both checks' remaining assertions — the refused rename leaves both names as they were, and a rename to an unused name is applied — passed, so only the message text diverges.
Traces: qconnect.rename-quick-response-to-taken-name-rejected, qconnect.update-quick-response-to-taken-name-rejected.

=== Cause 3 of 7: SearchContent accepts a nextToken another operation issued and answers 200 with an empty page instead of refusing it ===
Failing check: qconnect.search-content-next-token-it-never-issued-rejected.
AWS: Smithy model — SearchContent.nextToken is an httpQuery member targeting NextToken (@length 1..2048) and SearchContent models ValidationException (400). Recorded behavior: a nextToken SearchContent did not itself issue — here the token a ListContents page handed back for the same knowledge base — is refused HTTP 400 ValidationException with message 'Invalid parameter' (fixed text, no variable part). A pagination token is scoped to the operation that minted it.
Emulator: the call succeeds — HTTP 200 with body {"contentSummaries": []} — so a search that matches one document silently reports no matches when it is resumed with a foreign token.
Why: this is a silent wrong answer, the worst failure mode for pagination. A client that mixes up tokens (a common bug, and exactly what AWS refuses) is told the knowledge base holds nothing matching instead of being told its token is invalid; a resume loop terminates early and the caller concludes content is missing.
Fix: nextToken validation in the SearchContent handler, services/service-com-amazonaws-qconnect-wisdomservice/src/ops/search_content.rs — a token must be rejected unless SearchContent itself issued it for this request's knowledge base and search expression, with ValidationException / 'Invalid parameter'. The check's later assertions (the token-less search still returns the one match, no nextToken on a one-match page) were never reached.
Traces: qconnect.search-content-next-token-it-never-issued-rejected.

=== Cause 4 of 7: SearchContent's refusal of an unissued nextToken carries the wrong message text ===
Failing check: qconnect.search-content-unissued-next-token-rejected.
AWS: as Cause 3 — an unissued/forged nextToken is refused 400 ValidationException with message 'Invalid parameter' (fixed text, no variable part; it does not name the member or echo the token).
Emulator: refused with code ValidationException but message 'nextToken: the specified token is not valid.' — different fixed text, and it names the member, which AWS's wording does not.
Why: a client or test that matches AWS's refusal message sees wording AWS never sends. Taken with Cause 3 the emulator is also inconsistent with itself: a forged token is refused while a token minted by a different operation is accepted.
Fix: the refusal message of the nextToken validation path in services/service-com-amazonaws-qconnect-wisdomservice/src/ops/search_content.rs. The check's surrounding assertions (the search by unique name finds exactly the document before and after the refusal) passed.
Traces: qconnect.search-content-unissued-next-token-rejected.

=== Cause 5 of 7: StartContentUpload refuses contentType text/csv on a QUICK_RESPONSES knowledge base, so no quick-response import can be staged ===
Failing check: qconnect.start-import-job-missing-required-headers-rejected.
AWS: Smithy model — StartContentUpload.contentType is required and constrained by @pattern ^(text/(plain|html|csv))|(application/(pdf|vnd\.openxmlformats-officedocument\.wordprocessingml\.document))|(application/x\.wisdom-json;source=(salesforce|servicenow|zendesk))$, so 'text/csv' is a modeled, valid content type; nothing in the model ties the permitted content types to the knowledge base's knowledgeBaseType. Staging a CSV against a QUICK_RESPONSES knowledge base is the documented first step of the quick-response import flow that StartImportJob(importJobType=QUICK_RESPONSES, uploadId=...) completes. Recorded behavior: StartContentUpload(knowledgeBaseId=<a QUICK_RESPONSES knowledge base>, contentType='text/csv') succeeds with 200 and returns uploadId/url/urlExpiry/headersToInclude.
Emulator: the call is refused HTTP 400 ValidationException with message 'Unsupported ContentType for KnowledgeBaseType: quick_replies' — a message AWS does not send, naming a knowledge-base type token ('quick_replies') that is not even a value of the modeled KnowledgeBaseType enum.
Why: the whole quick-response import surface is unreachable. A client cannot upload an import file at all, so StartImportJob can never be exercised — including its own refusal of a CSV missing required columns, which AWS answers 400 ValidationException with the fixed text 'One or more required headers [<required columns>] are missing from received headers [<the file's columns>]' (the bracketed lists are the required column set — Name, Routing Profile(s), Channel, Content — and the columns actually present in the uploaded file). That refusal could not be observed here because the upload never happened; once Cause 5 is fixed, re-replaying this check will show whether StartImportJob's header refusal also diverges.
Fix: the content-type gate in the StartContentUpload handler, services/service-com-amazonaws-qconnect-wisdomservice/src/ops/start_content_upload.rs — it must accept every content type the modeled @pattern permits for a QUICK_RESPONSES knowledge base (text/csv above all), and must not invent a knowledge-base-type-dependent restriction the model does not state.
Traces: qconnect.start-import-job-missing-required-headers-rejected.

=== Cause 6 of 7: GetKnowledgeBase never serves ingestionStatus, so a knowledge base never reports its content sync as complete ===
Failing check: qconnect.retrieve-unknown-association-refused-beside-answered-one.
AWS: Smithy model — KnowledgeBaseData.ingestionStatus targets SyncStatus, whose values are SYNC_FAILED, SYNCING_IN_PROGRESS, SYNC_SUCCESS, CREATE_IN_PROGRESS, documented as "Status of ingestion on data source" (KnowledgeBaseData also carries ingestionFailureReasons). Recorded behavior: after content is uploaded into a CUSTOM knowledge base and an assistant association is created, GetKnowledgeBase reports ingestionStatus and it settles on SYNC_SUCCESS; a client polls that member to know the uploaded content is retrievable.
Emulator: GetKnowledgeBase's knowledgeBase object omits ingestionStatus entirely — the member reads as absent (None) for the whole 900 s poll budget, so the check fails with "ingestion settled in None, expected 'SYNC_SUCCESS'".
Why: there is no way for a client to learn that ingestion finished. Any flow that waits for SYNC_SUCCESS before querying (the normal pattern before Retrieve/QueryAssistant) hangs until its own timeout and then reports a sync failure that never happened. It also blocks the rest of this check — the partial-batch behavior of Retrieve, where a live association answers and an unknown association id is refused in `errors` with code RESOURCE_NOT_FOUND and message 'Could not find the specified association.' — from ever being exercised.
Fix: the knowledge-base read/serialization path and the state that tracks content ingestion — services/service-com-amazonaws-qconnect-wisdomservice/src/ops/get_knowledge_base.rs and the service state it reads — must populate ingestionStatus with a SyncStatus value and advance it to SYNC_SUCCESS once the knowledge base's uploaded content is ingested. (Retrieve itself is an unimplemented stub covered by its own already-filed issue; this cause is about the missing ingestionStatus member, which fails before Retrieve is reached.)
Traces: qconnect.retrieve-unknown-association-refused-beside-answered-one.

=== Cause 7 of 7: a request signed outside the 5-minute window is refused RequestTimeTooSkewed instead of InvalidSignatureException with the expired-signature message ===
Failing check: qconnect.send-message-signed-twenty-minutes-ago-rejected.
AWS: recorded behavior only (SigV4 is not in the service model). A request whose X-Amz-Date is older than the server's 5-minute signing window is refused HTTP 403 with header x-amzn-errortype: InvalidSignatureException and message whose fixed text is "Signature expired: <request date> is now earlier than <window start> (<server clock> - 5 min.)" — the three variable parts are the request's own X-Amz-Date stamp, the start of the accepted window, and the server's clock, all in the %Y%m%dT%H%M%SZ form, with window start exactly 5 minutes before the server clock. The 403 status is matched by the emulator; the error type and message are not.
Emulator: 403 with x-amzn-errortype: RequestTimeTooSkewed and body {"__type": "RequestTimeTooSkewed", "message": "The difference between the request time and the current time is too large"} — the wrong error type, and a message that names neither the request's date, nor the window start, nor the server clock.
Why: SDKs and clients treat InvalidSignatureException and RequestTimeTooSkewed differently (retry and clock-skew-correction behavior keys on the error type), so a client whose clock is skewed does not take the path it takes against AWS, and a human debugging the refusal loses the three timestamps AWS puts in the message that identify the skew. This is the auth layer, so it affects every qconnect operation, not only the one the check signs.
Fix: the SigV4 verification middleware's clock-skew/expiry branch for this service — it must answer InvalidSignatureException with the expired-signature message above rather than RequestTimeTooSkewed. Note the check's later steps call SendMessage, which is an unimplemented stub covered by its own already-filed issue; this cause is the signature-expiry refusal, which fails before the accepted SendMessage is reached.
Traces: qconnect.send-message-signed-twenty-minutes-ago-rejected.

## Evidence
Suite: simparity-behavioral-aws-v1 — Total 37/63 passed; qconnect 37/58 (21 failed). Per-cause expected vs actual:

Cause 1 (CreateContent/UpdateContent constraint refusals):
- expected: ('ValidationException', "1 validation error detected: Value '{attr09=value9, ... attr10=value10}' at 'metadata' failed to satisfy constraint: Member must have length less than or equal to 10")
  actual (qconnect.create-content-eleven-metadata-entries-rejected): AssertionError: CreateContent with 11 metadata entries answered 'BadRequestException': 'Invalid request body'
- expected: ('ValidationException', "1 validation error detected: Value '{a=v-a, ... k=v-k}' at 'metadata' failed to satisfy constraint: Member must have length less than or equal to 10")
  actual (qconnect.update-content-metadata-over-10-entries-rejected): AssertionError: an 11-entry metadata map answered ('BadRequestException', 'Invalid request body')
- expected: ('ValidationException', "1 validation error detected: Value '<256 R's>' at 'title' failed to satisfy constraint: Member must have length less than or equal to 255")
  actual (qconnect.update-content-title-over-255-chars-rejected): AssertionError: a 256-character title answered ('BadRequestException', 'Invalid request body')

Cause 2 (UpdateQuickResponse duplicate name):
- expected: ('ConflictException', 'Name is already in use')
  actual (qconnect.rename-quick-response-to-taken-name-rejected): AssertionError: renaming onto a taken name answered 'ConflictException': 'The value for name is already in use. Please update and try again.'
  actual (qconnect.update-quick-response-to-taken-name-rejected): AssertionError: renaming onto a held name answered ('ConflictException', 'The value for name is already in use. Please update and try again.')

Cause 3 (SearchContent accepts a foreign token):
- expected: refusal ('ValidationException', 'Invalid parameter') for SearchContent(nextToken=<token ListContents issued>)
  actual (qconnect.search-content-next-token-it-never-issued-rejected): AssertionError: the call was expected to be refused but returned {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'contentSummaries': []}

Cause 4 (SearchContent unissued-token message):
- expected: ('ValidationException', 'Invalid parameter') for SearchContent(nextToken='c2VhcmNoLXBhZ2UtdGhhdC13YXMtbmV2ZXItc2VydmVk')
  actual (qconnect.search-content-unissued-next-token-rejected): AssertionError: an unissued nextToken was answered with ('ValidationException', 'nextToken: the specified token is not valid.')

Cause 5 (StartContentUpload text/csv):
- expected: StartContentUpload(knowledgeBaseId=<QUICK_RESPONSES kb>, contentType='text/csv') -> 200 with uploadId/url/headersToInclude
  actual (qconnect.start-import-job-missing-required-headers-rejected): botocore.errorfactory.ValidationException: An error occurred (ValidationException) when calling the StartContentUpload operation: Unsupported ContentType for KnowledgeBaseType: quick_replies

Cause 6 (GetKnowledgeBase.ingestionStatus):
- expected: GetKnowledgeBase(...)['knowledgeBase']['ingestionStatus'] polls to 'SYNC_SUCCESS'
  actual (qconnect.retrieve-unknown-association-refused-beside-answered-one): AssertionError: ingestion settled in None, expected 'SYNC_SUCCESS' (member absent for the whole poll budget)

Cause 7 (expired SigV4):
- expected: 403, header x-amzn-errortype 'InvalidSignatureException', message matching "Signature expired: <YYYYMMDDTHHMMSSZ> is now earlier than <YYYYMMDDTHHMMSSZ> (<YYYYMMDDTHHMMSSZ> - 5 min.)" with the first stamp equal to the request's X-Amz-Date and the window start exactly 5 min before the server clock
  actual (qconnect.send-message-signed-twenty-minutes-ago-rejected): AssertionError: signed 20 min ago: error type 'RequestTimeTooSkewed', body {'__type': 'RequestTimeTooSkewed', 'message': 'The difference between the request time and the current time is too large'}

## Steps to Reproduce
Replay the behavioral suite and the ten listed checks against the booted emulator (run_behavioral_suite, or each check individually). Minimal per-cause reproduction:

Cause 1: CreateKnowledgeBase(knowledgeBaseType='CUSTOM'); wait ACTIVE; StartContentUpload(contentType='text/plain') and PUT the bytes; then (a) CreateContent(..., metadata={11 entries}) and (b) CreateContent with 10 entries, then UpdateContent(contentId, metadata={11 entries}) and UpdateContent(contentId, title='R'*256). Each of (a) and the two UpdateContent calls is refused BadRequestException/'Invalid request body' instead of ValidationException with the '1 validation error detected: ... Member must have length less than or equal to <10|255>' message. The 10-entry / 255-char variants are accepted, as AWS does.

Cause 2: CreateKnowledgeBase(knowledgeBaseType='QUICK_RESPONSES'); wait ACTIVE; CreateQuickResponse twice with two distinct names; UpdateQuickResponse(quickResponseId=<second>, name=<first's name>) -> ConflictException with message 'The value for name is already in use. Please update and try again.' instead of 'Name is already in use'.

Cause 3: CreateKnowledgeBase('CUSTOM'); add two contents; ListContents(maxResults=1) and take its nextToken; SearchContent(searchExpression={filters:[{field:'NAME',operator:'EQUALS',value:<first name>}]}, maxResults=1, nextToken=<that ListContents token>) -> HTTP 200 {'contentSummaries': []} instead of 400 ValidationException 'Invalid parameter'.

Cause 4: same knowledge base; SearchContent(..., nextToken='c2VhcmNoLXBhZ2UtdGhhdC13YXMtbmV2ZXItc2VydmVk') -> ValidationException with message 'nextToken: the specified token is not valid.' instead of 'Invalid parameter'.

Cause 5: CreateKnowledgeBase(knowledgeBaseType='QUICK_RESPONSES'); wait ACTIVE; StartContentUpload(knowledgeBaseId=<it>, contentType='text/csv') -> ValidationException 'Unsupported ContentType for KnowledgeBaseType: quick_replies' instead of a 200 upload slot. (With that fixed, continue: PUT a CSV whose header row is 'Name,Content,Description' and call StartImportJob(importJobType='QUICK_RESPONSES', uploadId=...) to check the missing-required-headers refusal.)

Cause 6: CreateKnowledgeBase('CUSTOM', vectorIngestionConfiguration={'chunkingConfiguration':{'chunkingStrategy':'NONE'}}); wait ACTIVE; StartContentUpload + PUT a passage + CreateContent; CreateAssistant and CreateAssistantAssociation(associationType='KNOWLEDGE_BASE'); then poll GetKnowledgeBase(knowledgeBaseId) — 'ingestionStatus' is never present in the returned knowledgeBase.

Cause 7: sign any qconnect request (e.g. POST /assistants/{assistantId}/sessions/{sessionId}/message) with SigV4 whose X-Amz-Date is 20 minutes (or 10 minutes) in the past and send it raw -> 403 with x-amzn-errortype 'RequestTimeTooSkewed' instead of 'InvalidSignatureException' plus the 'Signature expired: ...' message.
