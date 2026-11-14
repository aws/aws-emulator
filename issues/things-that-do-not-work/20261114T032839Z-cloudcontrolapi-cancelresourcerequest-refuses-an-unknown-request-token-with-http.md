Issue-Source: trace-sweep-divergence
Issue-Trace: cloudcontrol-invalid-request-token-error-paths
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cloudcontrol-invalid-request-token-error-paths cloudcontrolapi-unknown-request-token-handling
Issue-Shape-Id: com.amazonaws.cloudcontrol#CloudApiService
Issue-Key: cloudcontrolapi|CancelResourceRequest|4xx->4xx|status
Issue-Commit: d96d9e0689f67235cf5db5594b0e3f429b7ae97b

# CancelResourceRequest refuses an unknown request token with HTTP 404 instead of 400

> _Note: this issue was identified against the code base at commit `d96d9e0689f67235cf5db5594b0e3f429b7ae97b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CancelResourceRequest

## Description
ONE net-new root cause in the cloudcontrolapi (Cloud Control API, sdkId CloudControl, awsJson1_0) service: the HTTP status Cloud Control puts on a RequestTokenNotFoundException refusal from CancelResourceRequest. Model citations are from services/service-com-amazonaws-cloudcontrol-cloudapiservice/smithy-model/com-amazonaws-cloudcontrol-cloudapiservice.json.

Cause 1 of 1: CancelResourceRequest answers an unknown/never-minted RequestToken with HTTP 404 where Cloud Control answers HTTP 400.
AWS: Smithy models CancelResourceRequest with errors RequestTokenNotFoundException (client, @httpError 404) and ConcurrentModificationException (server, 500); its required input member RequestToken targets RequestToken (@length 1..128, @pattern ^[-A-Za-z0-9+/=]+$), so a well-formed token that names no request record is a lookup miss rather than a validation failure and must surface as RequestTokenNotFoundException. The recorded wire response for this awsJson1_0 service carries HTTP 400 for that shape: the model's @httpError 404 is NOT the status this service puts on the wire, and the recording is the wire truth here. The error payload itself is already correct in the emulator — only the status line diverges, so the body (__type com.amazon.cloudapiservice#RequestTokenNotFoundException and its Message, whose fixed text is "Request with token " ... " was not found" around the RequestToken taken from the request) is not in dispute for this cause. Hypothesis: the 404-on-the-wire mapping comes from the generic @httpError-to-status path rather than from this service's recorded protocol behavior, which pins every not-found shape of this service at 400.
Emulator: answers HTTP 404 (detail "status: expected 400, got 404"); every other compared key on the response matches, so the divergence is exactly and only the status code.
Why: a client relying on AWS's behavior classifies the refusal by status. A 400 is a non-retryable client fault that botocore and the AWS SDKs surface as the modeled RequestTokenNotFoundException; a 404 off a JSON-RPC POST is routinely treated by SDK retry/endpoint logic as a transport- or endpoint-level fault, so cancel-if-still-running cleanup either retries a refusal that can never succeed or reports an endpoint problem instead of "that token names no request". Status-code-keyed error handling therefore behaves differently against the emulator than against AWS even though the body is identical.
Fix: the unknown-token branch of the CancelResourceRequest handler (services/service-com-amazonaws-cloudcontrol-cloudapiservice/src/ops/cancel_resource_request.rs) and whatever maps RequestTokenNotFoundException onto a response status for this service must emit HTTP 400, not the modeled 404. GetResourceRequestStatus has the same 404-vs-400 defect on its own path and is already filed (see below), so the status mapping is likely shared; this operation is a separate cause because the replay reports no owning issue for it and its own handler path produces the refusal.
Why it is not already covered: open issue 20261103T024056Z-cloudcontrolapi-refusals-answer-404-or-200-where-aws.md bundles six cloudcontrolapi refusal causes and does carry the replay keys for the GetResourceRequestStatus, GetResource and UpdateResource not-found steps, but it has NO Issue-Key or cause for CancelResourceRequest and the replay reports owned_by empty for this cluster, so this operation's refusal is unexercised by that issue's pointer and would be closed unrun.
Traces: cloudcontrol-invalid-request-token-error-paths#task#2 (cluster also covers cloudcontrolapi-unknown-request-token-handling#task#1).

## Evidence
From the replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up=true, bootstrap_signal=false, score_pct=83.7).

Cluster key service=cloudcontrolapi|CancelResourceRequest|status=400->404|keys=status, verdict mismatch, 2 affected steps.
Representative: fixture cloudcontrol-invalid-request-token-error-paths, step task#2, operation CancelResourceRequest.
  Expected status: 400
  Actual status:   404
  Detail:          "status: expected 400, got 404"
Other member of the cluster: cloudcontrolapi-unknown-request-token-handling#task#1.

No body key is reported as diverging (the cluster's compared keys are "status" only), so the emulator's __type and Message already match the recording; the requirement this step pins is the status line alone. The particular token string in each recording is a per-run value that fills the message template and is not part of the requirement.

Model: CancelResourceRequest errors = [ConcurrentModificationException (server, httpError 500), RequestTokenNotFoundException (client, httpError 404)]; input RequestToken required, @length 1..128, @pattern ^[-A-Za-z0-9+/=]+$. RequestTokenNotFoundException doc: "A resource operation with the specified request token can't be found." The recorded wire status for this shape on this service is 400, which overrides the modeled 404.

## Steps to Reproduce
Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1 (fixture cloudcontrol-invalid-request-token-error-paths, step task#2; also cloudcontrolapi-unknown-request-token-handling, step task#1).

Or drive it directly with botocore:
  cc = boto3.client("cloudcontrol", endpoint_url=<emulator>)
  cc.cancel_resource_request(RequestToken="<a token matching ^[-A-Za-z0-9+/=]{1,128}$ that no CreateResource/UpdateResource/DeleteResource call on this emulator ever minted>")

Expected: HTTP 400 with __type "com.amazon.cloudapiservice#RequestTokenNotFoundException" and Message "Request with token <that token> was not found".
Actual: the same error body, but the response status line is HTTP 404.

Inspect the raw status (e.g. e.response["ResponseMetadata"]["HTTPStatusCode"] on the raised ClientError, or curl -i -X POST with X-Amz-Target: CloudApiService.CancelResourceRequest and body {"RequestToken":"<token>"}) to see 404 where AWS returns 400.
