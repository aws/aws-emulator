Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: b2bi.start-transformer-job-missing-input-and-deleted-transformer-not-found b2bi.start-transformer-job-client-token-reuse-conflict
Issue-Shape-Id: com.amazonaws.b2bi#B2BI
Issue-Key: b2bi.start-transformer-job-missing-input-and-deleted-transformer-not-found|_refusal#14|AssertionError|raise AssertionError(f"the call was expected to be refused but returned {answer!r}")
Issue-Key: b2bi.start-transformer-job-client-token-reuse-conflict|run#26|AssertionError|assert got == want, f"token reused with another output location: expected {want!r}, got {got!r}"
Issue-Commit: dd9cf420697e5d0dd5f9eb36b5f6cf849c89f05e

# b2bi StartTransformerJob: missing inputFile object is not refused (200 + job id instead of ResourceNotFoundException), and the clientToken-reuse conflict message is not AWS's wording

> _Note: this issue was identified against the code base at commit `dd9cf420697e5d0dd5f9eb36b5f6cf849c89f05e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartTransformerJob

## Description
Two net-new root causes in the b2bi StartTransformerJob handler. Both are grounded in failing behavioral checks; neither is covered by an open issue (the only open b2bi issues are 20261125T125531Z-b2bi-createtransformer-mints-transformerid-with-prefix-t-instead.md and 20261126T102412Z-b2bi-createstartermappingtemplate-injects-sample-values-into-body-segment.md, which concern CreateTransformer and CreateStarterMappingTemplate, and 20261109T123922Z-b2bi-cloudformation-... which concerns CFN resource types).

Cause 1 of 2 — StartTransformerJob accepts a job whose inputFile object does not exist.
Failing check: b2bi.start-transformer-job-missing-input-and-deleted-transformer-not-found
AWS: StartTransformerJob resolves the object named by `inputFile` (bucketName + key) BEFORE a job exists, and when that S3 object is absent it refuses the request with the modelled ResourceNotFoundException — HTTP 404, error code `ResourceNotFoundException` (Smithy: StartTransformerJob errors include ResourceNotFoundException @httpError 404, documented "Occurs when the requested resource does not exist, or cannot be found"; `inputFile` is a required S3Location member "Specifies the location of the input file for the transformation"). Recorded behavior shows the message `The S3 object s3://<bucket>/<key> does not exist`, whose fixed text is `The S3 object s3://` + `/` + ` does not exist` and whose variable parts are the request's `inputFile.bucketName` and `inputFile.key`. No job id is minted and nothing is written under the requested output prefix.
Emulator: the call succeeds. The replay got HTTP 200 with a body carrying `transformerJobId` (`{'transformerJobId': '58cfeceef6a82898fa16c8991', ...}`) — the handler mints the job id and only afterwards discovers the unreadable input, which it reports as the job's own terminal `failed` status (services/service-com-amazonaws-b2bi-b2bi/src/ops/start_transformer_job.rs: `run_job` turns a `read_edi_object` error into `Outcome::failed("The input file could not be read: …")`, and the pre-job validation in `handle` only checks that `inputFile` names a non-empty bucketName and key).
Why: a client that relies on the 404 to detect "the file has not been uploaded yet" sees a successful start instead. It gets back a job id for a job AWS never created, must poll GetTransformerJob to learn the input was missing, and cannot distinguish a not-yet-uploaded input from a transformation that genuinely failed. Error-path code that catches ResourceNotFoundException never runs, and job-id bookkeeping diverges from AWS (the emulator records a job AWS has no record of).
Fix: StartTransformerJob's pre-job validation in the operation handler — the existence of `inputFile`'s object must be checked in the same phase as the other caller preconditions (before the token claim mints a job id), and its absence raised as the modelled ResourceNotFoundException rather than deferred into the job's `failed` outcome.

Cause 2 of 2 — the ConflictException message for a clientToken reused with different parameters is not AWS's wording.
Failing check: b2bi.start-transformer-job-client-token-reuse-conflict
AWS: StartTransformerJob's `clientToken` is the operation's @idempotencyToken (Smithy: StartTransformerJobRequest$clientToken, @idempotencyToken). Replaying it with a different request — here the same token with a different `outputLocation` — is refused with the modelled ConflictException, HTTP 409, error code `ConflictException` (Smithy: StartTransformerJob errors include ConflictException @httpError 409). The recorded message is entirely fixed text with no variable part: `This clientToken has been used with a different request.`
Emulator: the status and code are right, but the message is `this clientToken already created transformer job <job-id> with different parameters` — different wording, lowercased, no trailing period, and it interpolates the id of the first job (a per-run value AWS's message does not contain at all). It comes from the shared helper `crate::state::token_already_used`, which start_transformer_job.rs wraps in `token_already_used(started)`.
Why: a client that branches on the conflict message — or an operator reading it — cannot match the text AWS produces, and the emulator leaks a job id into an error AWS keeps opaque. Message text is part of the contract for this error class.
Fix: the conflict path of StartTransformerJob's idempotency handling (and the shared token-reuse message helper it calls) must emit AWS's fixed text verbatim, with no interpolated job id.

Traces: b2bi.start-transformer-job-missing-input-and-deleted-transformer-not-found (Cause 1, asserted at check line 132: `got = _refusal(lambda: start("early/"))`), b2bi.start-transformer-job-client-token-reuse-conflict (Cause 2, asserted at check line 189).

## Evidence
Cause 1 — b2bi.start-transformer-job-missing-input-and-deleted-transformer-not-found:
  Expected: the call is REFUSED with ('ResourceNotFoundException', 'The S3 object s3://<bucket>/<key> does not exist') at HTTP 404, and no object exists in the bucket.
  Actual:   AssertionError: the call was expected to be refused but returned {'transformerJobId': '58cfeceef6a82898fa16c8991', 'ResponseMetadata': {'HTTPStatusCode': 200, 'HTTPHeaders': {'content-type': 'application/x-amz-json-1.0', 'content-length': '48', ...}}}
  (the job id value itself is per-run; the divergence is 200 + a minted job id where AWS answers 404 ResourceNotFoundException)

Cause 2 — b2bi.start-transformer-job-client-token-reuse-conflict:
  Expected: ('ConflictException', 'This clientToken has been used with a different request.')
  Actual:   ('ConflictException', 'this clientToken already created transformer job ab54de163a00e662fb052ef11 with different parameters')
  Fixed text of AWS's message: the whole string; it has no variable part. The emulator's string additionally carries a per-run job id ('ab54de163a00e662fb052ef11' in this run), which is not part of AWS's template.

Smithy (services/service-com-amazonaws-b2bi-b2bi/smithy-model/com-amazonaws-b2bi-b2bi.json): StartTransformerJob input {inputFile: S3Location required, outputLocation: S3Location required, transformerId: TransformerId required, clientToken: String @idempotencyToken}; errors AccessDeniedException 403, ConflictException 409, InternalServerException 500, ResourceNotFoundException 404, ThrottlingException 429, ValidationException 400.

## Steps to Reproduce
Cause 1:
1. Create an S3 bucket in the call's Region; do NOT upload the input object.
2. CreateTransformer (inputConversion X12/4010, mapping JSONATA '$'), then UpdateTransformer status=active.
3. StartTransformerJob with inputFile = {bucketName: <bucket>, key: 'inbound/<file>.x12'} (an object that does not exist) and outputLocation = {bucketName: <bucket>, key: 'early/'}.
4. AWS: HTTP 404 ResourceNotFoundException 'The S3 object s3://<bucket>/inbound/<file>.x12 does not exist', bucket still empty. Emulator: HTTP 200 with a transformerJobId.

Cause 2:
1. Same setup, with the input object uploaded.
2. StartTransformerJob with clientToken=<uuid> and outputLocation key 'first/' — accepted.
3. StartTransformerJob again with the SAME clientToken but outputLocation key 'conflict/'.
4. AWS: HTTP 409 ConflictException 'This clientToken has been used with a different request.'. Emulator: HTTP 409 ConflictException 'this clientToken already created transformer job <id> with different parameters'.

Replay both with: run_behavioral_suite (checks b2bi.start-transformer-job-missing-input-and-deleted-transformer-not-found, b2bi.start-transformer-job-client-token-reuse-conflict).
