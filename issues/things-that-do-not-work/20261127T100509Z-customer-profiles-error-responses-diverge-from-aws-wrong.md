Issue-Source: trace-sweep-divergence
Issue-Trace: customer-profiles-batch-put-profile-object-fails-an-unregistered-object-type-per-item
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: customer-profiles-batch-put-profile-object-fails-an-unregistered-object-type-per-item customer-profiles-delete-profile-object-refusals customer-profiles-list-profile-objects-not-found customer-profiles-put-profile-object-refuses-a-non-json-object
Issue-Shape-Id: com.amazonaws.customerprofiles#CustomerProfiles_20200815
Issue-Key: customer-profiles|BatchPutProfileObject|2xx->4xx|$.Failed
Issue-Key: customer-profiles|BatchPutProfileObject|2xx->4xx|$.Successful
Issue-Key: customer-profiles|BatchPutProfileObject|2xx->4xx|status
Issue-Key: customer-profiles|DeleteProfileObject|4xx->4xx|$.Message
Issue-Key: customer-profiles|ListProfileObjects|4xx->4xx|$.Message
Issue-Key: customer-profiles|PutProfileObject|4xx->4xx|$.Message
Issue-Commit: 1d46c777556b138301c0ca0bdf752665685bd84a

# customer-profiles error responses diverge from AWS: wrong exception messages and a whole-request 404 instead of per-item batch failure

> _Note: this issue was identified against the code base at commit `1d46c777556b138301c0ca0bdf752665685bd84a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchPutProfileObject

## Description
Four distinct refusal-path defects in the customer-profiles emulator. All four are about the SHAPE and TEXT of a refusal: the emulator picks a different transport outcome than AWS for a per-item batch failure, and words three modeled exception messages differently from AWS. THIS ISSUE HAS 4 CAUSES; all four must be fixed before it is closed.

Cause 1 of 4: BatchPutProfileObject answers an unregistered ObjectTypeName with a whole-request 404 instead of a 200 carrying a per-item failure.
AWS: Smithy BatchPutProfileObject -> http {method: PUT, uri: /domains/{DomainName}/profiles/objects/batch-put-profile-object, code: 200}; output BatchPutProfileObjectResponse has members Successful (BatchPutProfileObjectResponseList) and Failed (BatchPutProfileObjectErrorList). BatchPutProfileObjectErrorItem has required Id (the batch item's correlating Id from the request), required Code (responseCode, integer range 200-599, documented as "The HTTP status code for the error") and Message (text, "A message describing the error"). The batch API therefore reports an item-level failure IN a successful response, not as a request-level exception. The recording confirms it: HTTP 200 with Successful: [] and Failed: [{Code: 404, Id: <the request item's Id>, Message: "The ObjectTypeName <ObjectTypeName> is not found in the domain"}]. In that message the fixed text is "The ObjectTypeName " ... " is not found in the domain"; the only variable part is the ObjectTypeName value taken verbatim from the request (the probe type name in the recording is just one such value). Id is likewise the Id of the failing request item, and Code is 404 because the missing resource is the object type.
Emulator: returns HTTP 404 for the whole request and no body members at all - $.Failed missing (expected a one-item list, got None) and $.Successful missing (expected [], got None).
Why: a client batching up to 10 items (Items @length min 1 max 10) cannot tell which items failed and which were written. It sees a request-level ResourceNotFoundException, so the SDK raises instead of returning a response; partial-success handling, retry-the-failed-items loops and per-Id error reporting never run, and a caller that batches a mix of valid and invalid object types wrongly concludes nothing was written.
Fix: the BatchPutProfileObject handler - object-type resolution must be performed per item and collected into Failed/Successful rather than aborting the request, and the serializer must always emit both lists (empty list, not omitted).
Traces: customer-profiles-batch-put-profile-object-fails-an-unregistered-object-type-per-item#task#3.

Cause 2 of 4: DeleteProfileObject's not-found message is emulator-worded instead of AWS-worded.
AWS: Smithy DeleteProfileObject -> errors include ResourceNotFoundException (client, httpError 404, member Message). Recorded behavior: status 404 with Message exactly "Profile object is not found in the system". This message is entirely fixed text - it interpolates NOTHING from the request, in particular not the ProfileObjectUniqueKey.
Emulator: status 404 (correct) but Message "ProfileObject <unique-key> not found" - different fixed text and it leaks the request's ProfileObjectUniqueKey into the message (the long hex key in the recording is just that run's value, not part of any template).
Why: clients and test suites that assert on the exception message, and operators reading logs, see a message AWS never produces; a message built around the key also implies the key is echoed back, which AWS does not do.
Fix: DeleteProfileObject's handler/error construction - emit the fixed AWS text with no interpolation.
Traces: customer-profiles-delete-profile-object-refusals#task#10.

Cause 3 of 4: ListProfileObjects' unknown-object-type message drops the word "is".
AWS: Smithy ListProfileObjects -> errors include ResourceNotFoundException (client, httpError 404, member Message). Recorded behavior: status 404 with Message "ObjectType <ObjectTypeName> is not found". Fixed text: "ObjectType " ... " is not found"; the single variable part is the ObjectTypeName taken from the request body (the name in the recording is only an example value).
Emulator: status 404 (correct) but Message "ObjectType <ObjectTypeName> not found" - the fixed text omits "is".
Why: exact-match assertions on the error message fail, and message-matching client logic that distinguishes AWS error texts mis-classifies the failure.
Fix: ListProfileObjects' handler/error construction - restore AWS's fixed wording around the interpolated object-type name.
Traces: customer-profiles-list-profile-objects-not-found#task#5.

Cause 4 of 4: PutProfileObject's invalid-Object message is emulator-worded instead of AWS-worded.
AWS: Smithy PutProfileObject -> input member Object targets stringifiedJson (@length min 1 max 256000, sensitive, documented as "A string that is serialized from a JSON object"); errors include BadRequestException (client, httpError 400, doc "The input you provided is invalid", member Message). Recorded behavior for an Object that is not a serialized JSON object: status 400 with Message "Failed to put profile object due to the Object is invalid." - entirely fixed text, including the trailing period, with no part taken from the request (notably the offending Object value is NOT echoed, consistent with Object being @sensitive).
Emulator: status 400 (correct) but Message "Object must be a string serialized from a JSON object" - its own wording, derived from the member's documentation rather than AWS's error text.
Why: a client that branches on, or a fixture that asserts, AWS's validation message does not recognise the refusal; the divergence is invisible in the status code, so it only surfaces as a wrong-message failure deep in client code.
Fix: PutProfileObject's input validation / error construction - emit AWS's fixed BadRequestException text for an Object that is not a serialized JSON object.
Traces: customer-profiles-put-profile-object-refuses-a-non-json-object#task#4.

No open issue covers any of these four. The only related open customer-profiles issue is "Implement customer-profiles StopUploadJob: unimplemented scaffold stub", which is about an unimplemented operation and does not touch these refusal paths.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up, /__admin/health 200):

Cause 1 - BatchPutProfileObject, fixture customer-profiles-batch-put-profile-object-fails-an-unregistered-object-type-per-item step task#3: verdict mismatch, status expected 200, got 404. $.Failed: missing (expected [{'Code': 404, 'Id': 'row-a', 'Message': 'The ObjectTypeName <ObjectTypeName> is not found in the domain'}], got None). $.Successful: missing (expected [], got None). (The recorded Id 'row-a' and the probe object-type name are that run's request values, not requirements.)

Cause 2 - DeleteProfileObject, fixture customer-profiles-delete-profile-object-refusals step task#10: status 404 both sides; $.Message value_mismatch - expected 'Profile object is not found in the system', got 'ProfileObject <unique-key> not found'.

Cause 3 - ListProfileObjects, fixture customer-profiles-list-profile-objects-not-found step task#5: status 404 both sides; $.Message value_mismatch - expected 'ObjectType UndefinedVoucher is not found', got 'ObjectType UndefinedVoucher not found' (UndefinedVoucher is the request's ObjectTypeName; the fixed text differs by the missing 'is').

Cause 4 - PutProfileObject, fixture customer-profiles-put-profile-object-refuses-a-non-json-object step task#4: status 400 both sides; $.Message value_mismatch - expected 'Failed to put profile object due to the Object is invalid.', got 'Object must be a string serialized from a JSON object'.

Model evidence: smithy_query operation=BatchPutProfileObject/DeleteProfileObject/ListProfileObjects/PutProfileObject and shape=BatchPutProfileObjectErrorItem/BatchPutProfileObjectRequestItem/ResourceNotFoundException/BadRequestException on services/service-com-amazonaws-customerprofiles-customerprofiles-20200815/smithy-model/com-amazonaws-customerprofiles-customerprofiles-20200815.json.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay the wire corpus (trace set simparity-wire-aws-v1).
3. Cause 1: replay fixture customer-profiles-batch-put-profile-object-fails-an-unregistered-object-type-per-item, step task#3 - PUT /domains/{DomainName}/profiles/objects/batch-put-profile-object with an ObjectTypeName that was never registered in the domain and one Items entry. Expect HTTP 200 with Successful: [] and Failed: [{Id: <that item's Id>, Code: 404, Message: 'The ObjectTypeName <ObjectTypeName> is not found in the domain'}]; the emulator answers HTTP 404 with neither list.
4. Cause 2: replay fixture customer-profiles-delete-profile-object-refusals, step task#10 - POST /domains/{DomainName}/profiles/objects/delete for a ProfileObjectUniqueKey that does not exist. Expect 404 ResourceNotFoundException with Message 'Profile object is not found in the system'.
5. Cause 3: replay fixture customer-profiles-list-profile-objects-not-found, step task#5 - POST /domains/{DomainName}/profiles/objects with an unregistered ObjectTypeName. Expect 404 with Message 'ObjectType <ObjectTypeName> is not found'.
6. Cause 4: replay fixture customer-profiles-put-profile-object-refuses-a-non-json-object, step task#4 - PUT /domains/{DomainName}/profiles/objects with Object set to a string that is not a serialized JSON object. Expect 400 BadRequestException with Message 'Failed to put profile object due to the Object is invalid.'
7. All four steps must pass before closing.
