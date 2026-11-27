Issue-Source: trace-sweep-divergence
Issue-Trace: mwaa-create-cli-token-name-constraints-reach-lookup
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: mwaa-create-cli-token-name-constraints-reach-lookup mwaa-create-web-login-token-name-constraints-reach-lookup
Issue-Shape-Id: com.amazonaws.mwaa#AmazonMWAA
Issue-Key: mwaa|CreateCliToken|4xx->4xx|$.message
Issue-Key: mwaa|CreateCliToken|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: mwaa|CreateCliToken|4xx->4xx|status
Issue-Key: mwaa|CreateWebLoginToken|4xx->4xx|$.message
Issue-Key: mwaa|CreateWebLoginToken|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: mwaa|CreateWebLoginToken|4xx->4xx|status
Issue-Commit: 558d2bea4a2bc55ff66bbf3c15524a570b71a39f

# mwaa: Name length pre-validation short-circuits the environment lookup on token operations

> _Note: this issue was identified against the code base at commit `558d2bea4a2bc55ff66bbf3c15524a570b71a39f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCliToken

## Description
Shared fault (2 causes): the MWAA token-minting operations apply the modeled @length constraint on the `Name` URI label as a hard pre-check and reject the request with 400 ValidationException before the environment is ever looked up. On AWS these operations reach the environment lookup and answer 404 ResourceNotFoundException for a name that does not name an existing environment, regardless of how long the name is. The emulator therefore reports a validation failure where AWS reports a missing resource, and a client can never distinguish "no such environment" from "malformed name".

Cause 1 of 2: CreateCliToken (POST /clitoken/{Name}) — fixture mwaa-create-cli-token-name-constraints-reach-lookup, step task#2.
AWS: Smithy (com.amazonaws.mwaa) CreateCliToken.Name -> EnvironmentName, @httpLabel, @required, @length(min 1, max 80), @pattern ^[a-zA-Z][0-9a-zA-Z-_]*$; the only modeled error is ResourceNotFoundException (httpError 404, single `message` member, doc "ResourceNotFoundException: The resource is not available."). ValidationException is NOT bound to CreateCliToken at all. The recorded response for a name longer than 80 characters that names no environment is HTTP 404 with body message "Environment not found" — the whole message is fixed text for this error class, with no variable part. Recorded behavior only for the fact that the over-long name is not rejected up front: the recording shows the request reaching the lookup.
Emulator: HTTP 400, x-amzn-errortype: ValidationException, message "1 validation error detected: Value '<name>' at 'name' failed to satisfy constraint: Member must have length less than or equal to 80" (the fixed text is "1 validation error detected: Value '...' at '...' failed to satisfy constraint: Member must have length less than or equal to ..."; the name, the member name and the limit are the variable parts). Expected status 404, got 400; expected $.message 'Environment not found', got the validation text; the error type header is ValidationException where AWS emits ResourceNotFoundException.
Why: a client that branches on ResourceNotFoundException (to create the environment, to poll, or to surface "environment does not exist") instead sees a non-retryable 400 ValidationException that is not even a modeled error for this operation, so its error handling for the missing-environment case is dead code against the emulator.
Fix: the CreateCliToken handler's input validation — the `Name` label length check must not be enforced ahead of the environment lookup for this operation; the lookup miss must produce ResourceNotFoundException (404) with message "Environment not found".
Traces: mwaa-create-cli-token-name-constraints-reach-lookup#task#2 (also task#3).

Cause 2 of 2: CreateWebLoginToken (POST /webtoken/{Name}) — fixture mwaa-create-web-login-token-name-constraints-reach-lookup, step task#2.
AWS: Smithy CreateWebLoginToken.Name -> EnvironmentName, @httpLabel, @required, @length(min 1, max 80), @pattern ^[a-zA-Z][0-9a-zA-Z-_]*$; modeled errors AccessDeniedException(403), InternalServerException(500), ResourceNotFoundException(404), ValidationException(400). ValidationException IS modeled here, but the recorded response for an over-long name that names no environment is HTTP 404 with message "Environment not found" — i.e. AWS resolves the environment first and the length bound is not applied as a request-rejecting pre-check on this path. Message text is fixed for the error class, with no variable part.
Emulator: HTTP 400, x-amzn-errortype: ValidationException, message "1 validation error detected: Value '<name>' at 'name' failed to satisfy constraint: Member must have length less than or equal to 80" (same template as cause 1; name, member name and limit are the request-derived parts).
Why: same client impact — the missing-environment branch is unreachable, and an over-long but otherwise unknown environment name is misreported as a client input defect instead of a lookup miss.
Fix: the CreateWebLoginToken handler's input validation — same ordering defect as cause 1; both operations share the `Name` label pre-check, so the fix belongs where that check is applied for the token operations.
Traces: mwaa-create-web-login-token-name-constraints-reach-lookup#task#2 (also task#3).

No existing open mwaa issue covers this; the open mwaa-related issues concern MWAA Serverless CreateWorkflow validation and CloudFormation resource-type registration, not the token operations.

## Evidence
CreateCliToken — expected (recorded AWS): HTTP 404, ResourceNotFoundException, $.message = 'Environment not found'. Actual (emulator): HTTP 400, x-amzn-errortype: ValidationException, $.message = "1 validation error detected: Value 'sim2real-probe-mwaa-cli-...' at 'name' failed to satisfy constraint: Member must have length less than or equal to 80". Divergence key service=mwaa|CreateCliToken|status=404->400|keys=$.message,$headers.x-amzn-errortype,status; 2 affected steps (task#2, task#3).

CreateWebLoginToken — expected (recorded AWS): HTTP 404, ResourceNotFoundException, $.message = 'Environment not found'. Actual (emulator): HTTP 400, x-amzn-errortype: ValidationException, $.message = "1 validation error detected: Value 'sim2real-probe-mwaa-web-...' at 'name' failed to satisfy constraint: Member must have length less than or equal to 80". Divergence key service=mwaa|CreateWebLoginToken|status=404->400|keys=$.message,$headers.x-amzn-errortype,status; 2 affected steps (task#2, task#3).

Model evidence: CreateCliToken binds ONLY ResourceNotFoundException(404); ValidationException is not a modeled error for it. ResourceNotFoundException: httpError 404, member `message`.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Replay fixture mwaa-create-cli-token-name-constraints-reach-lookup, step task#2: POST /clitoken/{Name} with a Name longer than 80 characters that matches ^[a-zA-Z][0-9a-zA-Z-_]*$ and names no existing environment. Observe 400 ValidationException instead of 404 ResourceNotFoundException with message 'Environment not found'.
3. Replay fixture mwaa-create-web-login-token-name-constraints-reach-lookup, step task#2: POST /webtoken/{Name} with the same kind of Name. Observe the same 400 ValidationException instead of 404 'Environment not found'.
4. Both fixtures' task#3 steps reproduce the same divergence.
