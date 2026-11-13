Issue-Source: trace-sweep-divergence
Issue-Trace: observabilityadmin-s3-table-integration-error-paths
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: observabilityadmin-s3-table-integration-error-paths
Issue-Shape-Id: com.amazonaws.observabilityadmin#ObservabilityAdmin
Issue-Key: observabilityadmin|DeleteS3TableIntegration|4xx->4xx|$.Message
Issue-Key: observabilityadmin|DeleteS3TableIntegration|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: observabilityadmin|DeleteS3TableIntegration|4xx->4xx|status
Issue-Commit: 53eb816e99061cb37fa36e4196f468308f70e8b6

# DeleteS3TableIntegration answers an unknown integration ARN with 400 ValidationException instead of 404 ResourceNotFoundException

> _Note: this issue was identified against the code base at commit `53eb816e99061cb37fa36e4196f468308f70e8b6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteS3TableIntegration

## Description
This issue carries 1 cause; it must be closed before submitting.

Cause 1 of 1: DeleteS3TableIntegration's not-found path is served as an input-validation failure instead of a resource-not-found refusal (fixture observabilityadmin-s3-table-integration-error-paths, step task#3).

AWS: Smithy: DeleteS3TableIntegration input Arn -> ResourceArn (required, @length 1..1011, @pattern ^arn:aws([a-z0-9\-]+)?:([a-zA-Z0-9\-]+):([a-z0-9\-]+)?:([0-9]{12})?:(.+)$), http POST /DeleteS3TableIntegration, output Unit. The model's bound error set for this operation lists AccessDeniedException, InternalServerException, InvalidStateException, ServiceQuotaExceededException, TooManyRequestsException and ValidationException, and does NOT list ResourceNotFoundException -- so for the not-found path this is recorded behavior only. The recording shows that a syntactically VALID ResourceArn that names no existing S3 Table integration is refused with HTTP 404 and a not-found-class error (the service's ResourceNotFoundException, httpError 404, members Message/ResourceId/ResourceType), carrying the x-amzn-errortype response header for that error type. The Message template is 'Integration <integration-id> from requesterAccountId <account-id> not found': the fixed text is 'Integration ', ' from requesterAccountId ' and ' not found'; the two variable parts are the integration identifier taken from the resource portion of the request's Arn and the caller's 12-digit account id (the requester account), both read from the request/credentials rather than from any recording. ValidationException (400) is reserved for an Arn that fails the modeled length/pattern constraints, which is not the case here -- the ARN is well-formed, it merely addresses nothing.

Emulator: answers HTTP 400 with x-amzn-errortype 'ValidationException' and Message 'Arn names no S3 Table integration in us-west-2: <requested arn>' -- the emulator's own sentence, whose variable parts are the full requested ARN and the region, neither of which appears in AWS's template.

Why: the status CLASS is wrong, not just the prose. A client deleting an integration idempotently branches on 404/ResourceNotFoundException to treat the delete as already done; against the emulator it instead sees a 400 ValidationException, which every SDK and retry/error-classification layer reads as a malformed request -- a non-retryable client bug -- so idempotent teardown, cleanup scripts and 'delete if exists' flows fail hard instead of succeeding. It also makes a well-formed ARN indistinguishable from an ARN that actually violates the modeled pattern. The message itself is also unmatchable, and it omits the requesterAccountId that AWS reports.

Fix: the not-found path of the DeleteS3TableIntegration handler and its request-validation step (services/service-com-amazonaws-observabilityadmin-observabilityadmin/src/ops/ -- one file per operation): pattern/length failures on Arn must remain ValidationException 400, but a well-formed Arn that resolves to no stored integration must raise the not-found error (404) with AWS's message template, rather than being folded into the validation error.

Related open issues (neither covers this operation or this status-class divergence): '20261029T174915Z-observabilityadmin-refusal-contract-diverges-not-found-conflict-and.md' covers GetS3TableIntegration/CreateS3TableIntegration/telemetry rule and pipeline refusal MESSAGES whose status is already correct, and '20261029T184923Z-observabilityadmin-telemetry-rule-pipeline-refusal-contract-diverges-emulator.md' covers the telemetry rule/pipeline refusal wording. DeleteS3TableIntegration is absent from both, and uniquely here the HTTP status and the error type are wrong as well as the text.

## Evidence
Cause 1, fixture observabilityadmin-s3-table-integration-error-paths step task#3 (DeleteS3TableIntegration against an integration ARN that does not exist):
- status: expected 404, got 400.
- $headers.x-amzn-errortype: expected the not-found error type (recorded, value not asserted by the fixture); got 'ValidationException'.
- $.Message: expected 'Integration <integration-id> from requesterAccountId <account-id> not found' (recorded as 'Integration sim2realprobeobservabilit from requesterAccountId 000000000000 not found'); got 'Arn names no S3 Table integration in us-west-2: arn:aws:observabilityadmin:us-west-2:000000000000:s3tableintegration/sim2realprobeobservabilit'.
The integration id, account id, region and ARN above are this recording's per-run values and are not themselves requirements; the requirements are the 404 status, the not-found error type, and the fixed text 'Integration ' / ' from requesterAccountId ' / ' not found' with the integration id from the request's Arn and the requester account id substituted in.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Fixture observabilityadmin-s3-table-integration-error-paths, step task#3: POST /DeleteS3TableIntegration with an Arn that satisfies the modeled ResourceArn pattern/length but names no existing S3 Table integration in the account.
3. Compare the HTTP status, the x-amzn-errortype response header and $.Message against the recorded response: AWS answers 404 with the not-found error type and 'Integration <integration-id> from requesterAccountId <account-id> not found'; the emulator answers 400 with x-amzn-errortype 'ValidationException' and its own 'Arn names no S3 Table integration in <region>: <arn>'.
