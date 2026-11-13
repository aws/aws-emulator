Issue-Source: trace-sweep-divergence
Issue-Trace: emr-serverless-tag-resource-list-tags-roundtrip
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: emr-serverless-tag-resource-list-tags-roundtrip emr-serverless-untag-resource-removes-tag
Issue-Shape-Id: com.amazonaws.emrserverless#AwsToledoWebService
Issue-Key: emr-serverless|TagResource|2xx->2xx|$headers.content-length
Issue-Key: emr-serverless|UntagResource|2xx->2xx|$headers.content-length
Issue-Commit: c25092682df62c5e37a98aae2aa377f0a9463dd0

# EMR Serverless tagging responses omit the Content-Length header on their empty 204 bodies

> _Note: this issue was identified against the code base at commit `c25092682df62c5e37a98aae2aa377f0a9463dd0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource

## Description
The emulator's EMR Serverless tagging mutations return the right status (204) and the right empty body, but they emit no Content-Length response header at all, so the empty body is unframed. Both bodiless tagging operations share the defect, which points at one place: the response writer used for EMR Serverless operations whose output shape has no members never sets a content-length for a zero-byte body.

Cause 1 of 2: TagResource's 204 response carries no Content-Length header.
  AWS: Smithy: TagResource is `POST /tags/{resourceArn}` with output shape TagResourceResponse, which has NO members (`"output": {}`); its modeled errors are InternalServerException (500), ResourceNotFoundException (404) and ValidationException (400). The model's http code is defaulted (200, `codeDefaulted: true`), so the exact success status is recorded behavior only: the recording shows AWS answering 204 with an empty body. AWS frames that empty body explicitly — the recorded response carries a Content-Length header (its value is a framing value, and the fixture asserts only that the header is present, not what it holds). Expected: 204 with a Content-Length header present.
  Emulator: answers 204 with the correct empty body, but `$headers.content-length` is reported `missing (expected '<shaped-not-asserted>', got None)` — the header is absent from the response entirely.
  Why: a client relying on AWS's behavior cannot tell a deliberately empty body from a truncated or still-streaming one. An HTTP/1.1 response with neither Content-Length nor a transfer encoding is unframed, so strict clients, SDK response parsers and intermediate proxies may block waiting for more bytes, treat the connection as unreusable and tear it down, or surface a framing/parse error instead of the success the call actually got. A tag write that in fact succeeded then looks like a transport failure and gets retried.
  Fix: the TagResource handler's serializer / the shared EMR Serverless empty-output response writer — it must set Content-Length on bodiless success responses rather than leaving the header off.
  Traces: emr-serverless-tag-resource-list-tags-roundtrip#task#3.

Cause 2 of 2: UntagResource's 204 response carries no Content-Length header, the same way.
  AWS: Smithy: UntagResource is `DELETE /tags/{resourceArn}` (idempotent) with output shape UntagResourceResponse, likewise with NO members (`"output": {}`); modeled errors InternalServerException (500), ResourceNotFoundException (404), ValidationException (400). Success status again recorded behavior only: the recording shows 204 with an empty body and a Content-Length header present (presence asserted, value not). Expected: 204 with a Content-Length header present.
  Emulator: answers 204 with the correct empty body, but `$headers.content-length` is reported `missing (expected '<shaped-not-asserted>', got None)`.
  Why: identical client consequence — an unframed empty 204 leaves the tag-removal's success indistinguishable from a truncated response, so clients and proxies may stall, drop the connection or report a framing error for a call that succeeded.
  Fix: the UntagResource handler's serializer / the same shared EMR Serverless empty-output response writer.
  Traces: emr-serverless-untag-resource-removes-tag#task#3.

No existing open EMR Serverless issue covers response framing: the open issues on this service concern job-run execution and lifecycle state (20261022T154846Z, 20261024T013257Z, 20261026T072046Z, 20261030T093552Z), application-lifecycle refusal wording (20261102T083744Z), application idle auto-stop (20261103T124913Z), CloudFormation stub provisioning (20261109T141917Z), and read-response shape for GetJobRun/GetSessionEndpoint (20261109T183045Z). None of them touches TagResource or UntagResource or the Content-Length header.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator_up=true, bootstrap_signal=false, score_pct=88.3):

Cause 1 — fixture emr-serverless-tag-resource-list-tags-roundtrip, step task#3, operation TagResource, verdict mismatch, expected_status 204, actual_status 204:
  detail: $headers.content-length: missing (expected '<shaped-not-asserted>', got None)
  divergence_key: service=emr-serverless|TagResource|status=204->204|keys=$headers.content-length

Cause 2 — fixture emr-serverless-untag-resource-removes-tag, step task#3, operation UntagResource, verdict mismatch, expected_status 204, actual_status 204:
  detail: $headers.content-length: missing (expected '<shaped-not-asserted>', got None)
  divergence_key: service=emr-serverless|UntagResource|status=204->204|keys=$headers.content-length

Expected vs actual, both causes:
  Expected: HTTP 204, empty body, Content-Length response header PRESENT (the fixture asserts presence only — '<shaped-not-asserted>' — because the value is a framing value and not a fixed requirement).
  Actual:   HTTP 204, empty body, NO Content-Length response header (got None).

Status and body are correct in both; the sole divergence is the absent response header. Neither cluster carries a stub_path and neither carries an owned_by, so neither is already filed. Smithy (com-amazonaws-emrserverless-awstoledowebservice): TagResource output TagResourceResponse has no members; UntagResource output UntagResourceResponse has no members — both are bodiless-success operations, which is why one response-writer defect shows up on both.

## Steps to Reproduce
1. git_clone() the repo and boot the emulator built from it.
2. Replay the wire corpus (replay_wire_corpus) for trace set simparity-wire-aws-v1.
3. Cause 1: replay fixture `emr-serverless-tag-resource-list-tags-roundtrip` and stop at step `task#3` — the TagResource call that tags the created EMR Serverless application ARN. Inspect the raw HTTP response headers (not just the decoded SDK output): the status is 204 and the body is empty, but no Content-Length header is returned, whereas the recording has one.
4. Cause 2: replay fixture `emr-serverless-untag-resource-removes-tag` and stop at step `task#3` — the UntagResource call (DELETE /tags/{resourceArn}?tagKeys=...) that removes the tag. Same observation: 204, empty body, Content-Length header absent.
5. Confirm the fix by re-replaying both fixtures/steps and asserting a Content-Length header is present on each 204.
