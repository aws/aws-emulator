Issue-Source: trace-sweep-divergence
Issue-Trace: resiliencehubv2-policy-tagging
Issue-Step: task#3
Issue-Severity: LOW
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: resiliencehubv2-policy-tagging
Issue-Shape-Id: com.amazonaws.resiliencehubv2#NGRHServiceCore
Issue-Key: resiliencehubv2|TagResource|2xx->2xx|$headers.content-length
Issue-Commit: 8be6c2401be6803b94081d6cd0d76b281ab9e586

# resiliencehubv2 TagResource empty response omits the Content-Length header

> _Note: this issue was identified against the code base at commit `8be6c2401be6803b94081d6cd0d76b281ab9e586`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource

## Description
Wire replay of the simparity-wire-aws-v1 corpus against the emulator built from this checkout found ONE net-new cause on the resiliencehubv2 tagging surface. The cluster reported stub_path="" and owned_by=[] - it is not blocked by an unimplemented operation and no open issue carries its replay key. (The open issue "resiliencehubv2 error bodies diverge from AWS: validation messages reworded and clientToken conflict never raised", 20261111T074347Z-resiliencehubv2-error-bodies-diverge-from-aws-validation-messages.md, owns the five CreateAssertion/CreateInputSource/CreatePolicy refusal clusters in this same replay; it says nothing about tagging responses and does not cover this step.)

=== Cause 1 of 1: TagResource's successful, body-less response carries no Content-Length header ===
AWS: Smithy TagResource (POST /v2/tags/{resourceArn}; input resourceArn -> Arn @httpLabel required, tags -> TagMap required @length 1..50 sensitive; output shape TagResourceResponse has NO members; errors AccessDeniedException 403, InternalServerException 500, ResourceNotFoundException 404, ThrottlingException 429, ValidationException 400). Because the output structure is empty under restJson1 there is no payload to serialize. The HTTP framing of that empty success is recorded behavior only: AWS answers the tagging call with a body-less success response that still carries a Content-Length header (value 0 for the empty body), as every step of the recorded tagging fixture shows. The status itself is NOT the divergence - the emulator returns the same status the recording does.
Emulator: the status matches, but the response is emitted with NO Content-Length header at all - the header is absent rather than present-with-value-0. The replay reports `$headers.content-length: missing (expected '<shaped-not-asserted>', got None)`, i.e. the recording asserts only that the header exists, and the emulator does not send it.
Why: a client that frames a body-less response by Content-Length gets a response whose body length is undeclared. HTTP clients, proxies and connection-reuse layers that require an explicit length on a response with no transfer encoding either treat the connection as read-until-close (defeating keep-alive for every tagging call) or report a malformed response, so a TagResource that AWS makes cheap and reusable becomes a connection-terminating call against the emulator. Anything asserting on the response's headers (SDK response-metadata checks, HTTP-level contract tests) sees a header AWS always sends go missing.
Fix: the resiliencehubv2 response serializer's empty-output path (the restJson1 serializer branch that builds the body-less success for operations whose output structure has no members, shared by TagResource and the other empty-output operations such as UntagResource) - emit Content-Length: 0 for the empty body instead of omitting the header. This is response framing, not handler logic: the TagResource handler's state mutation is already correct, since the step's status and the tagging effect both match the recording.
Traces: resiliencehubv2-policy-tagging#task#3.

## Evidence
Wire corpus simparity-wire-aws-v1 replayed against the emulator built from this checkout (emulator_up=true, bootstrap_signal=false, 13 fixtures replayed, 124 steps, 50 passed, 38 mismatched, 36 blocked, score 40.3%).

Cause 1 - fixture resiliencehubv2-policy-tagging, step task#3, operation TagResource, verdict mismatch, divergence_key `service=resiliencehubv2|TagResource|status=204->204|keys=$headers.content-length`:
  expected status 204 / actual status 204 (the status AGREES - it is not part of this finding)
  $headers.content-length: missing
    expected: '<shaped-not-asserted>'  (the recording asserts the header is PRESENT; its value is not asserted, and for an empty body it is 0)
    actual:   None                     (the emulator sends no Content-Length header)
stub_path="", owned_by=[], affected=["resiliencehubv2-policy-tagging#task#3"], affected_count=1.

Model evidence (services/service-com-amazonaws-resiliencehubv2-ngrhservicecore/smithy-model/com-amazonaws-resiliencehubv2-ngrhservicecore.json, service NGRHServiceCore, sdkId resiliencehubv2, protocol restJson1):
- TagResource: http POST /v2/tags/{resourceArn}; input TagResourceRequest { resourceArn: Arn @httpLabel required @length min 31 @pattern ^arn:...$, tags: TagMap required @length 1..50 sensitive }; output TagResourceResponse with no members; errors AccessDeniedException(403), InternalServerException(500), ResourceNotFoundException(404), ThrottlingException(429), ValidationException(400).
- UntagResource shares the same empty-output shape (DELETE /v2/tags/{resourceArn}), so the same serializer branch frames its response too.
No per-run value is part of this requirement: the resource ARN, tag keys/values, request id and timestamps in the recording differ every run; the requirement is only that the body-less success declares its length.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from this checkout.
2. Replay the wire corpus simparity-wire-aws-v1 and inspect fixture resiliencehubv2-policy-tagging, step task#3.
3. Directly: POST /v2/create-policy to create a policy, then POST /v2/tags/{policyArn} with a body of {"tags": {"k": "v"}}.
4. Inspect the RAW response headers of that tagging call (e.g. with a client that prints them, not an SDK that hides framing). The status and the empty body match AWS, but no Content-Length header is present; AWS's response for this body-less success carries Content-Length: 0.
5. Same check on DELETE /v2/tags/{policyArn}?tagKeys=k (UntagResource), which is framed by the same empty-output serializer path.
