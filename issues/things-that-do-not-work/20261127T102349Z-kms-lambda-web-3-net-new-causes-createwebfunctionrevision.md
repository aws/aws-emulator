Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kms__lambda-web.web-function-revision-key-change-describes-the-key-with-a-real-call kms__lambda-web.web-function-revision-under-a-customer-managed-key-serves-without-kms-grants kms__lambda-web.web-function-revision-with-an-asymmetric-or-absent-kms-key-rejected
Issue-Key: kms__lambda-web.web-function-revision-key-change-describes-the-key-with-a-real-call|run#29|AssertionError|assert describes, (
Issue-Key: kms__lambda-web.web-function-revision-under-a-customer-managed-key-serves-without-kms-grants|run#33|AssertionError|assert decrypts, (
Issue-Key: kms__lambda-web.web-function-revision-with-an-asymmetric-or-absent-kms-key-rejected|_refusal#20|AssertionError|raise AssertionError("CreateWebFunctionRevision accepted a key it must refuse")
Issue-Commit: fff8505842aad03b204d6100e6a2dc09c6204c11

# kms__lambda-web: 3 net-new causes — CreateWebFunctionRevision accepts an asymmetric CMK, and a revision's KMS use (DescribeKey on the caller's behalf, the dry-run Decrypt probe, the serving Decrypt) reaches no CloudTrail event history

> _Note: this issue was identified against the code base at commit `fff8505842aad03b204d6100e6a2dc09c6204c11`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateWebFunctionRevision

## Description
Three net-new root causes in lambda-web's customer-managed-key path, all grounded in the three failing cross-service checks listed under `### kms__lambda-web`. Note on attribution: the suite lists these checks under the cross-service heading `kms__lambda-web`; every diverging behaviour below is lambda-web's (the key pre-flight it runs and the KMS calls it is supposed to make), and KMS itself answered every direct call the checks made.

Cause 1 of 3: CreateWebFunctionRevision accepts a key it must refuse — an asymmetric (RSA_2048, ENCRYPT_DECRYPT) customer managed key.
AWS: `kmsKeyArn` is "the Amazon Resource Name (ARN) of the AWS KMS key used to encrypt the revision's code and environment variables" (Smithy: CreateWebFunctionRevisionRequest.kmsKeyArn, KmsKeyArn @length 20..2048, @pattern ^arn:(aws[a-z-]*):kms:[a-z0-9-]+:[0-9]{12}:key/[a-z0-9-]+$). Encrypting a revision's code is a symmetric-encryption KMS operation, so only a symmetric encryption key is usable; a revision naming an asymmetric key is refused on the create with ValidationException (Smithy: CreateWebFunctionRevision errors include ValidationException, HTTP 400 — "The request failed validation because a parameter was missing, malformed, or outside the allowed range"). The refusal's message names the offending key as asymmetric: its fixed text is the class sentence stating the key is asymmetric and therefore unusable for this member, and its only variable part is the key ARN taken from the request's `kmsKeyArn`. Recorded behaviour: a 400 ValidationException whose message contains "asymmetric" (the check asserts exactly `code == "ValidationException" and "asymmetric" in message.lower()`), and no revision is added.
Emulator: the create SUCCEEDS. The pre-flight (services/service-com-amazonaws-lambdaweb-lambdaweb/src/state/revision_key.rs -> services/common/src/kms_key.rs::key_use_refusal) asks KMS DescribeKey and judges only the key's `KeyState` (STATES_THAT_ADMIT_NO_USE) plus relayed NotFoundException/InvalidArnException sentences. An Enabled RSA_2048 key passes that reading, so the revision is stored and returned 202 and the check fails with "CreateWebFunctionRevision accepted a key it must refuse".
Why: a client that names an asymmetric key is told the revision was created; against AWS it is told at once, with a correctable ValidationException, that the key cannot encrypt the revision. Code written against the emulator silently ships a function whose revision AWS would never have built, and the account's revision list diverges from AWS's (the check also asserts ListWebFunctionRevisions still holds only the initial revision after the refusals).
Fix: the key pre-flight — `state::revision_key::refusal` and the shared reading it delegates to (`services/common/src/kms_key.rs`): the reading must also weigh the described key's `KeySpec`/`KeyUsage`, not its `KeyState` alone, and CreateWebFunctionRevision/CreateWebFunction must raise ValidationException with the asymmetric sentence before anything is stored. Related but NOT the same cause: the absent-key branch of the same check (a key id the account never created, expected ValidationException with message `Key '<requested key>' does not exist` — fixed prefix "Key '" and suffix "' does not exist" around the requested key identifier) was never reached, because the call aborted on the asymmetric key first; it is unverified rather than diverging, and a fix should confirm that branch too.
Traces: kms__lambda-web.web-function-revision-with-an-asymmetric-or-absent-kms-key-rejected.

Cause 2 of 3: moving a revision onto a customer managed key leaves no CloudTrail event-history record of Lambda's key use — neither the caller-attributed DescribeKey nor the service principal's dry-run Decrypt probe.
AWS: creating a revision under a customer managed key makes Lambda describe that key on the caller's behalf: event history holds a kms.amazonaws.com `DescribeKey` whose `resources[].ARN` is the key, whose `userIdentity.arn` is the CALLER's ARN and whose `userIdentity.invokedBy` is `lambda.amazonaws.com`, recorded with no `requestParameters.dryRun` and no `errorCode`; beside it a `Decrypt` by the service principal (`userIdentity.type` == `AWSService`) carrying `requestParameters.dryRun` true and `requestParameters.encryptionContext["aws:lambda:WebFunctionRevisionArn"]` equal to the new revision's ARN, which KMS answers with `errorCode` `DryRunOperationException`. Recorded behaviour only for the shape of those records (reference the check cites: https://docs.aws.amazon.com/lambda/latest/dg/web-functions-encryption.html); the variable parts of each record are the key ARN, the caller's ARN and the revision ARN from the request, the rest is fixed for the class.
Emulator: after the rekeyed revision reaches Active with `kmsKeyArn` set, CloudTrail LookupEvents(EventName=DescribeKey|Decrypt) returns no kms.amazonaws.com record naming the key at all — the check polls to its audit budget and reports "no DescribeKey by the caller via Lambda reached the key's event history; events seen []". The DescribeKey the pre-flight does make is driven over the in-process callback with the emulator's own credentials and is never recorded through the shared recorder for caller-attributed cross-service calls (services/common/src/on_behalf_of_calls.rs), and no dry-run Decrypt probe is made at all.
Why: an audit/compliance client cannot see that the function's key was used, who used it, or under which encryption context; alarms, Athena queries and EventBridge rules keyed on kms.amazonaws.com DescribeKey/Decrypt with `invokedBy` lambda.amazonaws.com or on the revision's encryption context never fire, and a key policy written against those conditions cannot be validated against the emulator.
Fix: the create's key pre-flight and the revision-build path in the lambda-web crate (`src/ops/create_web_function_revision.rs` + `src/state/revision_key.rs`): make the DescribeKey a caller-attributed call recorded through `services/common/src/on_behalf_of_calls.rs` into the shared management-event log CloudTrail's LookupEvents reads, and add the service-principal dry-run Decrypt probe under the revision's encryption context so KMS records it with DryRunOperationException.
Traces: kms__lambda-web.web-function-revision-key-change-describes-the-key-with-a-real-call.

Cause 3 of 3: a revision serving traffic under a customer managed key never decrypts with that key — no service-principal Decrypt under the revision's encryption context is recorded.
AWS: a web function revision encrypted with a customer managed key is served with access authorised by the key policy alone: event history holds a kms.amazonaws.com `Decrypt` whose `resources[].ARN` is the key, `userIdentity.type` == `AWSService` (the Lambda service principal), `requestParameters.encryptionContext["aws:lambda:WebFunctionRevisionArn"]` equal to the revision's ARN, with no `dryRun` and no `errorCode`, and ListGrants on the key stays empty throughout. Recorded behaviour only (same reference as above); variable parts are the key ARN and the revision ARN.
Emulator: the endpoint reaches Active and a signed GET is served 200 with the revision's own reply, and ListGrants is correctly empty, but LookupEvents(EventName=Decrypt) holds no such record — "no Decrypt by the Lambda service principal under the revision's encryption context reached the key's event history, so the key was never shown to be used". The serving path runs the package without ever asking KMS to decrypt the revision under its key.
Why: the emulator serves encrypted revisions without exercising the key at all, so a key policy that would actually deny the Lambda service principal appears to work, and nothing in the audit record shows the revision's key in use. A client validating least-privilege key policies or auditing CMK usage per revision gets a false pass.
Fix: the endpoint data plane in the lambda-web crate (`src/state/endpoint_plane.rs` / `src/state/web_runtime.rs`, where a revision's package is run): decrypt the revision's material under its `kmsKeyArn` as the Lambda service principal with the `aws:lambda:WebFunctionRevisionArn` encryption context, so KMS authorises it against the key policy and the call lands in the account's management-event log.
Traces: kms__lambda-web.web-function-revision-under-a-customer-managed-key-serves-without-kms-grants.

No open issue covers these: the lambda-web issues on file concern endpoint/revision wiring (e.g. 20261125T183716Z-lambda-web-createwebfunctionrevision-error-response.md on the absent `errors` member) and the kms issues on file concern other services' key handling; none names lambda-web's key-spec pre-flight or its KMS audit records.

## Evidence
Suite: 18/21 passed; kms__lambda-web 0/3 (3 failed), all three mismatches.

Cause 1 — check web-function-revision-with-an-asymmetric-or-absent-kms-key-rejected, line 244 of the check:
  got = _refusal(lambda: lw.create_web_function_revision(functionName=..., kmsKeyArn=<RSA_2048 ENCRYPT_DECRYPT key ARN>, buildConfig=..., serviceConfig=...))
  -> AssertionError: CreateWebFunctionRevision accepted a key it must refuse
  Expected: ClientError, HTTP 400, Error.Code == "ValidationException", Error.Message naming the requested key as asymmetric (assertion: 'asymmetric' in message.lower()), and no new revision in ListWebFunctionRevisions.
  Actual: HTTP 202, a revision created under the asymmetric key.
  Emulator source: services/service-com-amazonaws-lambdaweb-lambdaweb/src/ops/create_web_function_revision.rs:266-287 calls state::revision_key::refusal, which delegates to services/common/src/kms_key.rs::key_use_refusal (lines 148-173) — that function reads only KeyMetadata.key_state against STATES_THAT_ADMIT_NO_USE (lines 122-130) and never reads KeySpec or KeyUsage, so an Enabled RSA_2048 key yields None (no refusal).

Cause 2 — check web-function-revision-key-change-describes-the-key-with-a-real-call, line 284:
  after CreateWebFunctionRevision(kmsKeyArn=<CMK>) settled Active with kmsKeyArn == the CMK, the check polled cloudtrail.lookup_events(LookupAttributes=[{EventName: DescribeKey}], then Decrypt, StartTime=before the rekey) to its audit budget.
  -> AssertionError: no DescribeKey by the caller via Lambda reached the key's event history; events seen []
  Expected: a kms.amazonaws.com DescribeKey with resources[].ARN == key ARN, userIdentity.arn == the caller's ARN, userIdentity.invokedBy == "lambda.amazonaws.com", no requestParameters.dryRun and no errorCode; plus a Decrypt with userIdentity.type == "AWSService", requestParameters.dryRun true, encryptionContext["aws:lambda:WebFunctionRevisionArn"] == the new revisionArn and errorCode == "DryRunOperationException".
  Actual: zero matching events (the summary of everything seen is the empty list).

Cause 3 — check web-function-revision-under-a-customer-managed-key-serves-without-kms-grants, line 317:
  the CMK-encrypted revision's endpoint reached Active, a SigV4-signed GET returned 200 with the revision's reply, ListGrants was empty (correct), then the check polled lookup_events(EventName=Decrypt) to its audit budget.
  -> AssertionError: no Decrypt by the Lambda service principal under the revision's encryption context reached the key's event history, so the key was never shown to be used
  Expected: a kms.amazonaws.com Decrypt with resources[].ARN == key ARN, userIdentity.type == "AWSService", encryptionContext["aws:lambda:WebFunctionRevisionArn"] == revisionArn, no dryRun, no errorCode.
  Actual: none.

## Steps to Reproduce
Cause 1:
1. CreateRole (lambda.amazonaws.com trust), create a versioned bucket and put a .zip deployment package in it.
2. kms:CreateKey twice: one default symmetric encryption key, one with KeySpec=RSA_2048, KeyUsage=ENCRYPT_DECRYPT.
3. lambda-web CreateWebFunction with that role and package (no kmsKeyArn); wait for the initial revision to be Active.
4. Call CreateWebFunctionRevision(functionName=<fn>, kmsKeyArn=<the RSA_2048 key ARN>, buildConfig=..., serviceConfig=...).
   Emulator: HTTP 202 and a stored revision. AWS: HTTP 400 ValidationException naming the key as asymmetric, and ListWebFunctionRevisions unchanged.
5. (Also confirm, once step 4 refuses:) the same call with kmsKeyArn=arn:aws:kms:<region>:<account>:key/<random uuid> must answer ValidationException with message `Key '<that ARN>' does not exist`.

Cause 2:
1. Same setup, plus a symmetric CMK whose policy allows kms:* to the account root and kms:Decrypt to lambda.amazonaws.com for aws:SourceArn like the function's revision ARNs.
2. CreateWebFunction without a key; wait for Active.
3. Note the time, then CreateWebFunctionRevision(kmsKeyArn=<CMK ARN>) and wait for the new revision to be Active with that kmsKeyArn.
4. cloudtrail LookupEvents(LookupAttributes=[{AttributeKey: EventName, AttributeValue: DescribeKey}], StartTime=the noted time), then the same for Decrypt, paging NextToken.
   Emulator: no kms.amazonaws.com event naming the key. AWS: the caller-attributed DescribeKey (invokedBy lambda.amazonaws.com) and the service principal's dry-run Decrypt (errorCode DryRunOperationException) under encryptionContext aws:lambda:WebFunctionRevisionArn = the new revision ARN.

Cause 3:
1. Same setup; CreateWebFunction with revisionConfig.kmsKeyArn = <CMK ARN> and an IamAuth HomeRegion endpoint.
2. Wait for the endpoint and the revision to be Active; issue a SigV4-signed GET to the endpoint's domainName and confirm 200.
3. kms ListGrants(KeyId=<CMK>) — empty, as expected.
4. cloudtrail LookupEvents(EventName=Decrypt, StartTime=before the create), paging NextToken.
   Emulator: no kms.amazonaws.com Decrypt by an AWSService identity under encryptionContext aws:lambda:WebFunctionRevisionArn = the revision ARN. AWS: exactly that record, with no dryRun and no errorCode.
