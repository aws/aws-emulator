Issue-Source: trace-sweep-divergence
Issue-Trace: greengrassv2-component-version-lifecycle
Issue-Step: task#6
Issue-Severity: LOW
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: greengrassv2-component-version-lifecycle greengrassv2-deployment-tag-round-trip
Issue-Shape-Id: com.amazonaws.greengrassv2#GreengrassV2
Issue-Key: greengrass|TagResource|2xx->2xx|$headers.content-length
Issue-Key: greengrass|UntagResource|2xx->2xx|$headers.content-length
Issue-Commit: a8ab2e52bd77f613dd033c9fe2ca82b26e42a18c

# greengrassv2 tag mutations omit Content-Length on their 204 No Content responses

> _Note: this issue was identified against the code base at commit `a8ab2e52bd77f613dd033c9fe2ca82b26e42a18c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource

## Description
The two GreengrassV2 tag-mutation operations now answer with the correct HTTP 204 No Content status, but neither emits the `Content-Length: 0` header AWS sends with that empty-body response. This is a response-framing defect shared by both operations, not a status or payload defect. 2 causes in this bundle.

Cause 1 of 2: TagResource response omits Content-Length: 0 (fixture greengrassv2-component-version-lifecycle, step task#6).
  AWS:      Smithy: TagResource -> http {method: POST, uri: "/tags/{resourceArn}", code: 200}; output shape TagResourceResponse has NO members, and the modeled errors are InternalServerException (500), ResourceNotFoundException (404), ValidationException (400). The model therefore says nothing about the success framing beyond "no body"; recorded behavior only for the rest: the recording shows AWS answering the successful tag mutation with status 204 and the response header `content-length: 0`, i.e. an explicitly zero-length body rather than an unframed one. There is no error code or error message on this path.
  Emulator: answers status 204 (matching), but the response carries no `content-length` header at all. Replay detail: `$headers.content-length: missing (expected '0', got None)`.
  Why:      a client relying on AWS's framing cannot tell a declared empty body from an absent/streamed one. HTTP clients and proxies that key on an explicit Content-Length for a response with no body may keep the connection open waiting for data, fall back to read-until-close, or treat the hop as non-reusable, so an SDK call that AWS completes immediately can stall or force connection churn. Strict HTTP intermediaries also reject a 204 whose framing headers were dropped.
  Fix:      the TagResource handler's serializer — the empty-body/204 response path must set `Content-Length: 0` when it emits a no-content response, rather than leaving the header off.
  Traces:   greengrassv2-component-version-lifecycle#task#6 (representative), and the same cluster member greengrassv2-deployment-tag-round-trip#task#4.

Cause 2 of 2: UntagResource response omits Content-Length: 0 (fixture greengrassv2-component-version-lifecycle, step task#8).
  AWS:      Smithy: UntagResource -> http {method: DELETE, uri: "/tags/{resourceArn}", code: 200}; output shape UntagResourceResponse has NO members; modeled errors InternalServerException (500), ResourceNotFoundException (404), ValidationException (400). Recorded behavior only for the framing: AWS answers the successful untag with status 204 and header `content-length: 0`. No error code or message on this path.
  Emulator: answers status 204 (matching) with no `content-length` header. Replay detail: `$headers.content-length: missing (expected '0', got None)`.
  Why:      same client-visible consequence as Cause 1 — the zero-length body is never declared, so clients and intermediaries that require explicit framing on a bodiless response may block, retry or drop the connection on an operation AWS completes cleanly.
  Fix:      the UntagResource handler's serializer — the shared no-content response path should declare `Content-Length: 0`; fixing it in the common empty-response serializer covers both causes.
  Traces:   greengrassv2-component-version-lifecycle#task#8 (representative), and the same cluster member greengrassv2-deployment-tag-round-trip#task#7.

Relation to already-open issues: the open issue "greengrassv2 TagResource answers HTTP 200 instead of AWS's 204 No Content" (20261106T082428Z-greengrass-greengrassv2-tagresource-answers-http-200-instead-of-aws.md) covers the STATUS code only, and the status now matches on replay; it does not cover the missing Content-Length header, and it says nothing about UntagResource. The stub issues 20261103T065241Z-greengrassv2-tagresource-stub.md and 20261105T083344Z-greengrassv2-untagresource-stub.md cover the operations being unimplemented, which is no longer the failure mode here — both operations respond. Neither of these divergences was reported as owned by an open issue in the replay output.

## Evidence
Replay of trace set simparity-wire-aws-v1 (score 81.1%, 14 mismatched steps of 74) reports two mismatch clusters whose only diverging key is the response header:

Cluster 1 — service=greengrass | TagResource | status=204->204 | keys=$headers.content-length
  fixture_id: greengrassv2-component-version-lifecycle, step_key: task#6
  expected_status 204, actual_status 204
  detail: $headers.content-length: missing (expected '0', got None)
  affected (2): greengrassv2-component-version-lifecycle#task#6, greengrassv2-deployment-tag-round-trip#task#4

Cluster 2 — service=greengrass | UntagResource | status=204->204 | keys=$headers.content-length
  fixture_id: greengrassv2-component-version-lifecycle, step_key: task#8
  expected_status 204, actual_status 204
  detail: $headers.content-length: missing (expected '0', got None)
  affected (2): greengrassv2-component-version-lifecycle#task#8, greengrassv2-deployment-tag-round-trip#task#7

Expected (AWS, from the recordings): HTTP/1.1 204 No Content with `content-length: 0` and an empty body.
Actual (emulator): HTTP/1.1 204 with an empty body and NO content-length header.
No status, error code or error message differs on either path — the divergence is purely the dropped framing header. Model citation: TagResource/UntagResource outputs (TagResourceResponse / UntagResourceResponse) declare no members, so an empty body is correct; only its declaration is missing.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Cause 1: replay fixture greengrassv2-component-version-lifecycle, step task#6 — POST /tags/{resourceArn} (TagResource) against an existing component-version ARN with a non-empty tags map. Inspect the raw response headers: the status is 204 but no `content-length` header is present; AWS sends `content-length: 0`. Reproduce the same with fixture greengrassv2-deployment-tag-round-trip step task#4 (deployment ARN).
3. Cause 2: replay fixture greengrassv2-component-version-lifecycle, step task#8 — DELETE /tags/{resourceArn}?tagKeys=... (UntagResource) for a tag key present on the resource. Status is 204, again with no `content-length` header. Reproduce with fixture greengrassv2-deployment-tag-round-trip step task#7.
4. Confirm with a raw HTTP client (not an SDK that tolerates missing framing), e.g. `curl -sv` and read the response header block, or compare against the recorded response headers in the fixtures.
5. After the fix, re-replay greengrassv2-component-version-lifecycle#task#6 and #task#8 (and the two greengrassv2-deployment-tag-round-trip steps) and verify each 204 carries `content-length: 0`.
