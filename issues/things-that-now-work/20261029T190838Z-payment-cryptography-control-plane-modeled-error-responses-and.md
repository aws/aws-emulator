Issue-Source: trace-sweep-divergence
Issue-Trace: payment-cryptography-validation-and-notfound-errors-v2
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: payment-cryptography-validation-and-notfound-errors-v2 payment-cryptography-alias-management-v2 payment-cryptography-mpa-team-association-errors-v2 payment-cryptography-tagging-workflow-v2
Issue-Shape-Id: com.amazonaws.paymentcryptography#PaymentCryptographyControlPlane
Issue-Key: payment-cryptography|GetKey|4xx->5xx|$.ResourceId
Issue-Key: payment-cryptography|GetKey|4xx->5xx|$.__type
Issue-Key: payment-cryptography|GetKey|4xx->5xx|$headers.content-type
Issue-Key: payment-cryptography|GetKey|4xx->5xx|status
Issue-Key: payment-cryptography|GetAlias|4xx->5xx|$.ResourceId
Issue-Key: payment-cryptography|GetAlias|4xx->5xx|$.__type
Issue-Key: payment-cryptography|GetAlias|4xx->5xx|$headers.content-type
Issue-Key: payment-cryptography|GetAlias|4xx->5xx|status
Issue-Key: payment-cryptography|GetMpaTeamAssociation|4xx->5xx|$.ResourceId
Issue-Key: payment-cryptography|GetMpaTeamAssociation|4xx->5xx|$.__type
Issue-Key: payment-cryptography|GetMpaTeamAssociation|4xx->5xx|$headers.content-type
Issue-Key: payment-cryptography|GetMpaTeamAssociation|4xx->5xx|status
Issue-Key: payment-cryptography|AssociateMpaTeam|4xx->5xx|$.ResourceId
Issue-Key: payment-cryptography|AssociateMpaTeam|4xx->5xx|$.__type
Issue-Key: payment-cryptography|AssociateMpaTeam|4xx->5xx|$headers.content-type
Issue-Key: payment-cryptography|AssociateMpaTeam|4xx->5xx|status
Issue-Key: payment-cryptography|DisassociateMpaTeam|4xx->5xx|$.ResourceId
Issue-Key: payment-cryptography|DisassociateMpaTeam|4xx->5xx|$.__type
Issue-Key: payment-cryptography|DisassociateMpaTeam|4xx->5xx|$headers.content-type
Issue-Key: payment-cryptography|DisassociateMpaTeam|4xx->5xx|status
Issue-Key: payment-cryptography|GetAlias|4xx->5xx|$.Message
Issue-Key: payment-cryptography|ListKeys|4xx->5xx|$.Message
Issue-Key: payment-cryptography|ListKeys|4xx->5xx|$.__type
Issue-Key: payment-cryptography|ListKeys|4xx->5xx|$headers.content-type
Issue-Key: payment-cryptography|ListKeys|4xx->5xx|status
Issue-Key: payment-cryptography|CreateAlias|4xx->5xx|$.Message
Issue-Key: payment-cryptography|CreateAlias|4xx->5xx|$.__type
Issue-Key: payment-cryptography|CreateAlias|4xx->5xx|$headers.content-type
Issue-Key: payment-cryptography|CreateAlias|4xx->5xx|status
Issue-Key: payment-cryptography|DeleteKey|4xx->5xx|$.ResourceId
Issue-Key: payment-cryptography|DeleteKey|4xx->5xx|$.__type
Issue-Key: payment-cryptography|DeleteKey|4xx->5xx|$headers.content-type
Issue-Key: payment-cryptography|DeleteKey|4xx->5xx|status
Issue-Related: 20261029T190838Z-payment-cryptography-control-plane-every-success-path-control.md
Issue-Commit: 69bb2c4e0c47afe7ac06c67651974f0cf752730a

# Payment Cryptography control plane: modeled error responses and CreateKey replaced by AmbiguousService 500

> _Note: this issue was identified against the code base at commit `69bb2c4e0c47afe7ac06c67651974f0cf752730a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetKey

## Description
This bundle has 10 causes, numbered below. Causes 1-9 are negative-path control-plane calls whose modeled client error (ResourceNotFoundException or ValidationException, HTTP 400) is replaced by HTTP 500 with the emulator-authored code 'AmbiguousService'; cause 10 is the remaining CreateKey success-path cluster. All ten are refused by the router before any handler runs, and all ten are framed as application/x-amz-json-1.1 instead of the awsJson1_0 content type. A single routing fix unblocks dispatch, but each cause below additionally requires the right status, error code, error member and message once dispatch works - the refusal currently masks whether any of that validation exists at all.

Shared protocol defects in every cause:
(a) Content type - the control plane is awsJson1_0 (Smithy: service PaymentCryptographyControlPlane, protocols [awsJson1_0]), so responses must carry Content-Type: application/x-amz-json-1.0; the emulator serves application/x-amz-json-1.1.
(b) Error framing - a modeled awsJson1_0 error must serialize __type with the recorded exception shape id and the error's own members. The recordings show __type 'com.amazonaws.paymentcryptography.exception#ResourceNotFoundException' / 'com.amazonaws.paymentcryptography.exception#ValidationException' at HTTP 400. Note the on-disk model gives ResourceNotFoundException httpError 404; the recorded wire status for these calls is 400, and the recording is what a client observes, so 400 is the requirement here.

Related open issue: 20261029T174910Z-payment-cryptography-control-plane-is-entirely-unreachable-the.md files the same routing root cause from the BEHAVIORAL corpus and carries no wire replay key, so it does not cover these wire fixture steps.

Cause 1 of 10: GetKey on an identifier that does not exist.
AWS: ResourceNotFoundException (service-common error, member ResourceId: "The identifier of the resource that was not found" - a key ARN or alias name). Recorded: HTTP 400, __type 'com.amazonaws.paymentcryptography.exception#ResourceNotFoundException', $.ResourceId set to the KeyIdentifier the request asked for (an alias name in this recording).
Emulator: HTTP 500, __type 'AmbiguousService', $.ResourceId missing, content-type 1.1.
Why: a client that catches ResourceNotFoundException to distinguish "absent" from "broken" never matches; it sees an unmodeled code at a retryable status and burns its retry budget.
Fix: the control-plane crate's request-claim seam (can_handle) plus GetKey's lookup path and the awsJson1_0 error serializer (ResourceId member, __type, 400).
Traces: payment-cryptography-validation-and-notfound-errors-v2#task#1.

Cause 2 of 10: GetAlias on an alias that was just deleted.
AWS: ResourceNotFoundException. Recorded: HTTP 400, __type '...exception#ResourceNotFoundException', ResourceId = the alias name from the request.
Emulator: HTTP 500, 'AmbiguousService', ResourceId missing, content-type 1.1.
Why: the delete-then-read lifecycle cannot be verified; a caller cannot tell a deleted alias from a service failure.
Fix: routing claim seam, then GetAlias's not-found path and the error serializer.
Traces: payment-cryptography-alias-management-v2#task#9.

Cause 3 of 10: GetMpaTeamAssociation for a protected operation with no association.
AWS: ResourceNotFoundException. Recorded: HTTP 400, __type '...exception#ResourceNotFoundException', ResourceId = the protected-operation name from the request (e.g. the IMPORT_ROOT_PUBLIC_KEY_CERTIFICATE value the caller passed).
Emulator: HTTP 500, 'AmbiguousService', ResourceId missing, content-type 1.1.
Why: MPA association state is unreadable, so a client cannot discover whether an approval team is attached.
Fix: routing claim seam, then GetMpaTeamAssociation's not-found path and the error serializer.
Traces: payment-cryptography-mpa-team-association-errors-v2#task#1.

Cause 4 of 10: AssociateMpaTeam naming an approval team that does not exist.
AWS: ResourceNotFoundException. Recorded: HTTP 400, __type '...exception#ResourceNotFoundException', ResourceId = the MPA approval-team ARN taken from the request.
Emulator: HTTP 500, 'AmbiguousService', ResourceId missing, content-type 1.1.
Why: a client cannot distinguish a bad team ARN from an outage, and gets no echo of which resource was rejected.
Fix: routing claim seam, then AssociateMpaTeam's team-resolution path and the error serializer.
Traces: payment-cryptography-mpa-team-association-errors-v2#task#2.

Cause 5 of 10: DisassociateMpaTeam for a protected operation with no association.
AWS: ResourceNotFoundException. Recorded: HTTP 400, __type '...exception#ResourceNotFoundException', ResourceId = the protected-operation name from the request.
Emulator: HTTP 500, 'AmbiguousService', ResourceId missing, content-type 1.1.
Why: idempotent teardown of an MPA association is impossible to write against.
Fix: routing claim seam, then DisassociateMpaTeam's not-found path and the error serializer.
Traces: payment-cryptography-mpa-team-association-errors-v2#task#3.

Cause 6 of 10: GetAlias with a syntactically invalid alias name.
AWS: ValidationException (httpError 400, member Message). Smithy GetAliasInput.AliasName is @required with @length min 7 max 256 and @pattern ^alias/[a-zA-Z0-9/_-]+$. Recorded: HTTP 400, __type '...exception#ValidationException', Message "1 validation error detected: Value '<the rejected value>' at 'aliasName' failed to satisfy constraint: Member must satisfy regular expression pattern: alias/[a-zA-Z0-9/_-]+". Fixed text: "1 validation error detected: Value '" ... "' at '" ... "' failed to satisfy constraint: Member must satisfy regular expression pattern: "; variable parts: the offending value from the request, the lowerCamel member name ('aliasName'), and the pattern from the model.
Emulator: HTTP 500, 'AmbiguousService', $.Message missing, content-type 1.1.
Why: client-side input validation tests against AWS semantics break - a malformed alias looks like a server fault rather than a 400 the caller must fix.
Fix: routing claim seam, then GetAlias's input constraint validation and the ValidationException serializer.
Traces: payment-cryptography-validation-and-notfound-errors-v2#task#2.

Cause 7 of 10: ListKeys with MaxResults above the modeled maximum.
AWS: ValidationException 400. Smithy ListKeysInput.MaxResults has @range min 1 max 100. Recorded: HTTP 400, __type '...exception#ValidationException', Message "1 validation error detected: Value '<value>' at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 100". Fixed text: "1 validation error detected: Value '" ... "' at '" ... "' failed to satisfy constraint: Member must have value less than or equal to "; variable parts: the submitted value, the member name 'maxResults', and the model's maximum (100).
Emulator: HTTP 500, 'AmbiguousService', $.Message missing, content-type 1.1.
Why: range constraints are unenforced and unreported, so a paginating client gets no signal that its page size is illegal.
Fix: routing claim seam, then ListKeys' range validation and the ValidationException serializer.
Traces: payment-cryptography-validation-and-notfound-errors-v2#task#3.

Cause 8 of 10: CreateAlias with a syntactically invalid alias name.
AWS: ValidationException 400 against the same ^alias/[a-zA-Z0-9/_-]+$ alias-name pattern. Recorded: HTTP 400, __type '...exception#ValidationException', Message "1 validation error detected: Value '<the rejected value>' at 'aliasName' failed to satisfy constraint: Member must satisfy regular expression pattern: alias/[a-zA-Z0-9/_-]+" (same fixed text and variable parts as cause 6).
Emulator: HTTP 500, 'AmbiguousService', $.Message missing, content-type 1.1.
Why: an invalid alias is not rejected at creation, so a caller cannot rely on the alias namespace being well-formed.
Fix: routing claim seam, then CreateAlias's input constraint validation and the ValidationException serializer.
Traces: payment-cryptography-validation-and-notfound-errors-v2#task#4.

Cause 9 of 10: DeleteKey on an identifier that does not exist.
AWS: ResourceNotFoundException. Recorded: HTTP 400, __type '...exception#ResourceNotFoundException', ResourceId = the KeyIdentifier from the request.
Emulator: HTTP 500, 'AmbiguousService', ResourceId missing, content-type 1.1.
Why: a deletion routine cannot treat "already gone" as benign, because the absent-key case is indistinguishable from a server error.
Fix: routing claim seam, then DeleteKey's lookup path and the error serializer.
Traces: payment-cryptography-validation-and-notfound-errors-v2#task#5.

Cause 10 of 10: CreateKey under the tagging workflow returns no key.
AWS: Smithy CreateKey -> CreateKeyOutput with required member Key. Recorded: HTTP 200.
Emulator: HTTP 500 ('AmbiguousService'), so the tagging fixture's setup never produces a key and its TagResource/UntagResource/ListTagsForResource steps are blocked.
Why: the whole tag/untag surface is unreachable for a client; nothing can be tagged because nothing can be created.
Fix: routing claim seam, then CreateKey's handler/serializer.
Traces: payment-cryptography-tagging-workflow-v2#setup#1.

## Evidence
Replay of simparity-wire-aws-v1: 11 fixtures replayed, 0/73 steps passed, 31 mismatched, 42 blocked, score 0%.

Expected (recorded AWS, awsJson1_0, Content-Type application/x-amz-json-1.0):
  GetKey(absent identifier)            -> 400, {"__type": "com.amazonaws.paymentcryptography.exception#ResourceNotFoundException", "ResourceId": "<identifier from the request>"}
  GetAlias(deleted alias)              -> 400, same __type, ResourceId = requested alias name
  GetMpaTeamAssociation(no assoc.)     -> 400, same __type, ResourceId = requested protected-operation name
  AssociateMpaTeam(absent team)        -> 400, same __type, ResourceId = requested approval-team ARN
  DisassociateMpaTeam(no assoc.)       -> 400, same __type, ResourceId = requested protected-operation name
  DeleteKey(absent identifier)         -> 400, same __type, ResourceId = requested identifier
  GetAlias(bad alias name)             -> 400, {"__type": "com.amazonaws.paymentcryptography.exception#ValidationException", "Message": "1 validation error detected: Value 'not-a-valid-alias-name' at 'aliasName' failed to satisfy constraint: Member must satisfy regular expression pattern: alias/[a-zA-Z0-9/_-]+"}
  CreateAlias(bad alias name)          -> 400, same __type and message template at 'aliasName'
  ListKeys(MaxResults=1000)            -> 400, {"__type": "...#ValidationException", "Message": "1 validation error detected: Value '1000' at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 100"}
  CreateKey (tagging setup)            -> 200 with required Key member
(The quoted values 'not-a-valid-alias-name', '1000' and the alias/ARN identifiers are the fixtures' request inputs filling the message template; the template and the model-derived limit/pattern are the requirement, not those specific values.)

Actual (emulator), identical for all ten causes:
  HTTP 500, Content-Type: application/x-amz-json-1.1
  body {"__type": "AmbiguousService", "message": "2 emulated services all claim requests for 'payment-cryptography': payment-cryptography, payment-cryptography; the request is rejected rather than served by an arbitrary one"}
  Replay reports per step: "$.__type: value_mismatch (expected 'com.amazonaws.paymentcryptography.exception#ResourceNotFoundException', got 'AmbiguousService')" / "(expected '...#ValidationException', got 'AmbiguousService')", plus "$.ResourceId: missing" or "$.Message: missing", plus "$headers.content-type: value_mismatch (expected 'application/x-amz-json-1.0', got 'application/x-amz-json-1.1')".

Message anatomy: the 'AmbiguousService' text is entirely emulator-authored framework wording with no AWS counterpart; its variable parts (claimant count and claimant names) come from the emulator's service registry, not from the request, so none of it is a requirement. 'AmbiguousService' appears in no error union of com.amazonaws.paymentcryptography; the status is also wrong (500 vs 400) and 500 is classified retryable by AWS SDKs, so the SDK retries a deterministic client error.

Model references used: smithy_query digest (protocols [awsJson1_0], common_errors include ResourceNotFoundException and ValidationException); shape ResourceNotFoundException (errorKind client, member ResourceId, model httpError 404 vs recorded 400); shape ValidationException (errorKind client, httpError 400, member Message); GetAliasInput.AliasName @required @length 7..256 @pattern ^alias/[a-zA-Z0-9/_-]+$; ListKeysInput.MaxResults @range 1..100.

## Steps to Reproduce
1. Boot the emulator from this checkout (the paymentcryptography control-plane and paymentcryptographydata data-plane crates are both workspace members and both declare sigv4Name 'payment-cryptography').
2. Replay fixture payment-cryptography-validation-and-notfound-errors-v2, step task#1 (GetKey on an identifier that does not exist). Observe HTTP 500 with __type 'AmbiguousService' and Content-Type application/x-amz-json-1.1 instead of HTTP 400 with __type 'com.amazonaws.paymentcryptography.exception#ResourceNotFoundException' and a ResourceId echoing the requested identifier.
3. Replay the remaining nine causes and check each one's status, __type, error member and message independently: payment-cryptography-validation-and-notfound-errors-v2#task#2 (GetAlias ValidationException on the alias-name pattern), #task#3 (ListKeys ValidationException on maxResults <= 100), #task#4 (CreateAlias ValidationException on the alias-name pattern), #task#5 (DeleteKey ResourceNotFoundException); payment-cryptography-alias-management-v2#task#9 (GetAlias ResourceNotFoundException after delete); payment-cryptography-mpa-team-association-errors-v2#task#1 (GetMpaTeamAssociation), #task#2 (AssociateMpaTeam), #task#3 (DisassociateMpaTeam); payment-cryptography-tagging-workflow-v2#setup#1 (CreateKey must return 200 with a Key).
4. Verify every response carries Content-Type: application/x-amz-json-1.0 and that the ValidationException messages follow the AWS template with the request's own value, the lowerCamel member name and the model's pattern/limit substituted in.
5. Verify no response body anywhere carries the code 'AmbiguousService'.
