Issue-Source: trace-sweep-divergence
Issue-Trace: appsync-delete-data-source-name-grammar
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: appsync-delete-data-source-name-grammar appsync-delete-resolver-field-name-grammar appsync-delete-resolver-type-name-grammar appsync-get-introspection-max-results-range untag-resource-arn-length-floor
Issue-Shape-Id: com.amazonaws.appsync#AWSDeepdishControlPlaneService
Issue-Key: appsync|DeleteDataSource|4xx->4xx|$.message
Issue-Key: appsync|DeleteDataSource|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: appsync|DeleteDataSource|4xx->4xx|status
Issue-Key: appsync|DeleteResolver|4xx->4xx|$.message
Issue-Key: appsync|DeleteResolver|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: appsync|DeleteResolver|4xx->4xx|status
Issue-Key: appsync|GetDataSourceIntrospection|4xx->4xx|$.message
Issue-Key: appsync|GetDataSourceIntrospection|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: appsync|GetDataSourceIntrospection|4xx->4xx|status
Issue-Key: appsync|UntagResource|4xx->4xx|$.message
Issue-Key: appsync|UntagResource|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: appsync|UntagResource|4xx->4xx|status
Issue-Commit: 5d024fed191bbfdb41e9accab576b1e1cc16d383

# AppSync identifier-grammar and query-parse refusals return the wrong status, class and message on lookup paths

> _Note: this issue was identified against the code base at commit `5d024fed191bbfdb41e9accab576b1e1cc16d383`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteDataSource

## Description
On AppSync's lookup-style operations the emulator validates the request member first and refuses 400, where AWS performs the lookup and answers 404 NotFoundException; where it does answer 404 it uses the wrong not-found message, and a malformed query-string integer leaks the emulator's own parser text. The status code — not just the wording — differs, so client branching inverts. 6 causes in this issue; all six must be closed before submitting. The open issue 20261029T174359Z-appsync-control-plane-error-semantics-not-found-bad.md names a related not-found/bad-request inversion, but it carries none of the replay keys below, so none of these causes is covered by it.

=== Cause 1 of 6: DeleteDataSource refuses a name that violates the member grammar instead of reporting it not found ===
AWS: Smithy (appsync) DeleteDataSource — DELETE /v1/apis/{apiId}/datasources/{name}, name: ResourceName @httpLabel @length 1-65536 @pattern ^[_A-Za-z][_0-9A-Za-z]*$; modeled BadRequestException|400, ConcurrentModificationException|409, InternalFailureException|500, NotFoundException|404, UnauthorizedException|401. Recorded behavior for a name that does not match that pattern (e.g. a leading digit) is 404 NotFoundException with the message 'Data source <name> not found' — fixed text 'Data source ' + the name taken from the request URI + ' not found'. AWS does not enforce the member pattern server-side on this path; it resolves the name and reports it missing.
Emulator: 400 with "1 validation error detected: Value '<name>' at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[_A-Za-z][_0-9A-Za-z]*$" and x-amzn-errortype BadRequestException (whose header value also fails the recorded header's shape).
Why: both HTTP status (400 vs 404) and class differ. A client deleting idempotently treats NotFoundException as success and a 400 as a hard input error, so a tear-down that is a no-op against AWS aborts against the emulator.
Fix: the DeleteDataSource handler's request validation — the name label must not be pattern-validated before the data-source lookup; the lookup's not-found path owns this response.
Traces: appsync-delete-data-source-name-grammar#task#3.

=== Cause 2 of 6: DeleteResolver refuses a fieldName/typeName that violates the member grammar instead of reporting no resolver ===
AWS: Smithy (appsync) DeleteResolver — DELETE /v1/apis/{apiId}/types/{typeName}/resolvers/{fieldName}, both labels ResourceName @pattern ^[_A-Za-z][_0-9A-Za-z]*$; modeled NotFoundException|404 and BadRequestException|400. Recorded behavior for a fieldName or typeName that does not match that pattern is 404 NotFoundException with the fixed message 'No resolver found.' — no request values in it.
Emulator: 400 with "1 validation error detected: Value '<value>' at 'fieldName' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[_A-Za-z][_0-9A-Za-z]*$" and x-amzn-errortype BadRequestException.
Why: same inversion as Cause 1 on the resolver path — idempotent delete logic that swallows NotFoundException now sees a 400.
Fix: the DeleteResolver handler's request validation — drop the label pattern pre-check and let the resolver lookup produce 404 'No resolver found.'
Traces: appsync-delete-resolver-field-name-grammar#task#6.

=== Cause 3 of 6: DeleteResolver reports the wrong not-found subject when the TYPE is missing ===
AWS: on the same operation, recorded behavior when the typeName names a type that does not exist on the api is 404 NotFoundException with the fixed message 'Type not found.' — AWS distinguishes the missing type from the missing resolver, which is reported as 'No resolver found.'
Emulator: 404 'No resolver found.' for the missing-type case too — the status and class are right, the subject is wrong.
Why: the message is the only signal of which level of the lookup failed; a client that creates the type before retrying cannot tell that the type, not the resolver, is absent.
Fix: the type/resolver lookup order in the DeleteResolver handler — the type lookup must fail first with its own message.
Traces: appsync-delete-resolver-type-name-grammar#task#8.

=== Cause 4 of 6: GetDataSourceIntrospection validates maxResults before resolving introspectionId ===
AWS: Smithy (appsync) GetDataSourceIntrospection — GET /v1/datasources/introspections/{introspectionId}, maxResults: MaxResults @httpQuery @range 0-25 (default 0); modeled BadRequestException|400, InternalFailureException|500, NotFoundException|404. Recorded behavior for an introspectionId that does not exist, even when maxResults is outside 0-25, is 404 NotFoundException with the fixed message 'No introspection was found that matches the supplied ID.' — the resource lookup precedes the range check.
Emulator: 400 with 'maxResults must be greater than 0 and less than or equal to 25.' and x-amzn-errortype BadRequestException; the introspectionId is never resolved.
Why: wrong status and wrong diagnosis — a client polling an introspection that has expired is told its pagination argument is bad instead of that the introspection is gone, and 400-vs-404 flips its retry decision. The emulator's text also states a lower bound of >0 where the model's range is 0-25.
Fix: validation ordering in the GetDataSourceIntrospection handler — resolve introspectionId first, then range-check maxResults.
Traces: appsync-get-introspection-max-results-range#task#1.

=== Cause 5 of 6: a non-numeric maxResults leaks the emulator's integer parser message ===
AWS: for the same operation, recorded behavior when the maxResults query-string value is not an integer is 400 with the fixed message "Request can't be parsed" — entirely fixed text, no request values, and the error type is the operation's modeled BadRequestException.
Emulator: 400 with 'invalid i32 `abc`: invalid digit found in string' and x-amzn-errortype ValidationException. The appsync Smithy model binds NO ValidationException shape at all (the service's 400 client error is BadRequestException), so this is an unmodeled error class.
Why: the message is an internal Rust parse diagnostic echoing the submitted value, and the class is not in the operation's error set, so no generated SDK models it — `except BadRequestException` never matches and the failure surfaces as an unknown error.
Fix: the query-string deserializer for this handler must map a parse failure to the modeled BadRequestException with AWS's fixed text, not surface the integer-parser error.
Traces: appsync-get-introspection-max-results-range#task#4.

=== Cause 6 of 6: UntagResource refuses a well-formed ARN at the length floor instead of reporting the resource missing ===
AWS: Smithy (appsync) UntagResource — DELETE /v1/tags/{resourceArn}, resourceArn @httpLabel @length 70-75 @pattern ^arn:aws:appsync:[A-Za-z0-9_/.-]{0,63}:\d{12}:apis/[0-9A-Za-z_-]{26}$; modeled AccessDeniedException|403, BadRequestException|400, InternalFailureException|500, LimitExceededException|429, NotFoundException|404, UnauthorizedException|401. Recorded behavior for an ARN that satisfies the pattern at the 70-character floor but names no existing resource is 404 NotFoundException with the fixed message 'Resource was not found'.
Emulator: 400 'Invalid input resource arn' with x-amzn-errortype BadRequestException — it rejects the ARN as malformed rather than resolving it.
Why: generic tagging tooling removes tags best-effort and treats NotFoundException as 'already gone'; a 400 'Invalid input resource arn' for a valid ARN makes the ARN itself look wrong and aborts the cleanup.
Fix: the resource-ARN parsing/validation shared by the Tag/Untag/ListTagsForResource handlers — accept the full modeled ARN range and fall through to the not-found path.
Traces: untag-resource-arn-length-floor#task#2.

## Evidence
Cause 1 — appsync-delete-data-source-name-grammar#task#3 DeleteDataSource: status expected 404, got 400; $.message expected 'Data source 9sim2real_probe_ds_58f48c34 not found', got "1 validation error detected: Value '9sim2real_probe_ds_58f48c34' at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[_A-Za-z][_0-9A-Za-z]*$"; $headers.x-amzn-errortype shape_mismatch, got 'BadRequestException' (the recorded header's value does not have that shape even where the class name is right).
Cause 2 — appsync-delete-resolver-field-name-grammar#task#6 DeleteResolver: status expected 404, got 400; $.message expected 'No resolver found.', got "1 validation error detected: Value '1greeting' at 'fieldName' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[_A-Za-z][_0-9A-Za-z]*$" (same cluster covers appsync-delete-resolver-type-name-grammar#task#6 and #task#7).
Cause 3 — appsync-delete-resolver-type-name-grammar#task#8 DeleteResolver: 404 both sides; $.message expected 'Type not found.', got 'No resolver found.'
Cause 4 — appsync-get-introspection-max-results-range#task#1 GetDataSourceIntrospection: status expected 404, got 400; $.message expected 'No introspection was found that matches the supplied ID.', got 'maxResults must be greater than 0 and less than or equal to 25.' (also #task#2).
Cause 5 — appsync-get-introspection-max-results-range#task#4 GetDataSourceIntrospection: $.message expected "Request can't be parsed", got 'invalid i32 `abc`: invalid digit found in string'; $headers.x-amzn-errortype got 'ValidationException', a shape the appsync model does not define.
Cause 6 — untag-resource-arn-length-floor#task#2 UntagResource: status expected 404, got 400; $.message expected 'Resource was not found', got 'Invalid input resource arn'.

## Steps to Reproduce
Cause 1: CreateGraphqlApi, then DeleteDataSource(apiId, name='9sim2real_probe_ds') -> must be 404 NotFoundException 'Data source 9sim2real_probe_ds not found'.
Cause 2: DeleteResolver(apiId, typeName='Query', fieldName='1greeting') -> must be 404 NotFoundException 'No resolver found.'
Cause 3: DeleteResolver(apiId, typeName=<a type absent from the schema>, fieldName=<valid grammar>) -> must be 404 'Type not found.'
Cause 4: GetDataSourceIntrospection(introspectionId=<never created>, maxResults=26) -> must be 404 NotFoundException 'No introspection was found that matches the supplied ID.'
Cause 5: GET /v1/datasources/introspections/<id>?maxResults=abc -> must be 400 BadRequestException "Request can't be parsed".
Cause 6: UntagResource(resourceArn=<well-formed 70-char appsync apis/ ARN for no existing api>, tagKeys=['k']) -> must be 404 NotFoundException 'Resource was not found'.
