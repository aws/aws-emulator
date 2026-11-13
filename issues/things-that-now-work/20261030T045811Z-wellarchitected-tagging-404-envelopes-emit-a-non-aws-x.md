Issue-Source: trace-sweep-divergence
Issue-Trace: wellarchitected-workload-tagging-envelopes
Issue-Step: task#7
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: wellarchitected-workload-tagging-envelopes
Issue-Shape-Id: com.amazonaws.wellarchitected#WellArchitectedApiServiceLambda
Issue-Key: wellarchitected|ListTagsForResource|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: wellarchitected|TagResource|4xx->4xx|$headers.x-amzn-errortype
Issue-Commit: 887058cb9fb4e791f5e5f0373480985b14410007

# Tagging 404 envelopes emit a non-AWS x-amzn-errortype header shape

> _Note: this issue was identified against the code base at commit `887058cb9fb4e791f5e5f0373480985b14410007`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListTagsForResource

## Description
The wellarchitected tag-plane operations that address a workload by ARN return the right HTTP status and the right JSON body for a workload ARN that does not exist, but the restJson1 error-type HEADER they emit does not have the shape the recorded AWS response carries. Both causes below are the same defect in the error envelope these two operations share: the body serializer is right, the header serializer is not.

Cause 1 of 2: ListTagsForResource on a workload ARN that names no workload (fixture wellarchitected-workload-tagging-envelopes, step task#7).
AWS: Smithy: ListTagsForResource (GET /tags/{WorkloadArn}, WorkloadArn @httpLabel @required) models exactly one client error, ResourceNotFoundException with @httpError 404. The recording asserts the response as 404 with the ResourceNotFoundException body, and asserts the x-amzn-errortype header by SHAPE rather than by literal value, so the requirement is the FORM of that header, not one run's text. For restJson1 AWS services the header carries the error's shape name, optionally qualified by a trailing ':' plus the service's Coral/namespace URI, and SDK clients resolve the exception class by taking the part before the first ':' (and before any '#'). Hypothesis: the recorded header for this service's tag plane is the qualified form (shape name + ':' + namespace URI) rather than the bare shape name.
Emulator: serves 404 with a body that matches the recording, but the header value is the bare 'ResourceNotFoundException', which fails the recorded shape assertion ('$headers.x-amzn-errortype: shape_mismatch (expected <shaped-not-asserted>, got ResourceNotFoundException)').
Why: the error-type header is the only thing an SDK or a hand-written client reads before it has parsed the body, so a header whose shape AWS never produces is a header a strict client can fail to split and map; such a client surfaces an unmodelled/opaque 404 instead of ResourceNotFoundException, and retry/not-found branches keyed on the typed exception never fire.
Fix: the restJson1 error serializer behind the tag-plane handlers — the component that stamps x-amzn-errortype for a thrown modelled error — must render the header in the same form AWS does for this service, for every modelled error, not just the 404 path.
Traces: wellarchitected-workload-tagging-envelopes#task#7.

Cause 2 of 2: TagResource on a workload ARN that names no workload (fixture wellarchitected-workload-tagging-envelopes, step task#8).
AWS: Smithy: TagResource (POST /tags/{WorkloadArn}, WorkloadArn @httpLabel @required, Tags @required TagMap @length 1..50) models the same single client error ResourceNotFoundException @httpError 404; the recording again asserts 404, the matching body, and the x-amzn-errortype header by shape.
Emulator: 404 and body match; the header is again the bare 'ResourceNotFoundException' and fails the same shape assertion ('$headers.x-amzn-errortype: shape_mismatch (expected <shaped-not-asserted>, got ResourceNotFoundException)').
Why: same client consequence as cause 1, on the write side of the tag plane — a tagging call against a deleted or mistyped workload ARN does not present as a typed not-found to the caller.
Fix: same serializer; the two tag-plane operations share it, so one correction closes both, and the fix must be exercised on both steps.
Traces: wellarchitected-workload-tagging-envelopes#task#8.

No open issue owns these two steps: the replay reports them with an empty owner, and the superficially related open issues '20261029T192731Z-wellarchitected-lens-review-and-question-lookups-raise-resourcenotfoundexception-where.md' (wrong error CLASS on lens/review lookups) and '20261029T192731Z-wellarchitected-review-template-duplicate-name-is-accepted-and-workload.md' (not-found MESSAGE wording and workload envelope members) are about the status/class and the body, not about the shape of the error-type header on the tag plane.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up, /__admin/health 200):

Step wellarchitected-workload-tagging-envelopes#task#7, ListTagsForResource: expected_status 404, actual_status 404, verdict mismatch, detail '$headers.x-amzn-errortype: shape_mismatch (expected '<shaped-not-asserted>', got 'ResourceNotFoundException')'. divergence_key service=wellarchitected|ListTagsForResource|status=404->404|keys=$headers.x-amzn-errortype. owned_by: [] (net-new), stub_path: ''.

Step wellarchitected-workload-tagging-envelopes#task#8, TagResource: expected_status 404, actual_status 404, verdict mismatch, detail '$headers.x-amzn-errortype: shape_mismatch (expected '<shaped-not-asserted>', got 'ResourceNotFoundException')'. divergence_key service=wellarchitected|TagResource|status=404->404|keys=$headers.x-amzn-errortype. owned_by: [] (net-new), stub_path: ''.

In both clusters every other asserted field (status and body members) matched, so the divergence is isolated to the error-type header's form. Fixed text of the requirement: the header name 'x-amzn-errortype' and the error shape name 'ResourceNotFoundException'; the variable part is the namespace/URI qualification AWS appends for this service — never a value copied from one recorded run.

Model evidence (services/service-com-amazonaws-wellarchitected-wellarchitectedapiservicelambda/smithy-model/com-amazonaws-wellarchitected-wellarchitectedapiservicelambda.json): ListTagsForResource errors = [InternalServerException 500, ResourceNotFoundException 404], http GET /tags/{WorkloadArn}; TagResource errors = [InternalServerException 500, ResourceNotFoundException 404], http POST /tags/{WorkloadArn}.

## Steps to Reproduce
1. Boot the emulator from the mainline checkout and replay trace set simparity-wire-aws-v1.
2. Cause 1: replay fixture wellarchitected-workload-tagging-envelopes step task#7 — GET /tags/{WorkloadArn} with a well-formed workload ARN in the emulator's account/region that names no existing workload. Observe 404 with the ResourceNotFoundException body, then inspect the response's x-amzn-errortype header: it is the bare 'ResourceNotFoundException' and fails the fixture's shape assertion.
3. Cause 2: replay the same fixture's step task#8 — POST /tags/{WorkloadArn} with a valid Tags map (1..50 entries) against the same nonexistent workload ARN. Observe the same 404 body and the same bare header value failing the same assertion.
4. Both steps must pass after the fix; re-replay wellarchitected-workload-tagging-envelopes#task#7 AND #task#8.
