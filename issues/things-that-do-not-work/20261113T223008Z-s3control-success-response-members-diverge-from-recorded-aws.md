Issue-Source: trace-sweep-divergence
Issue-Trace: s3c-multi-region-access-point-create-validation-boundary
Issue-Step: task#5
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: s3c-multi-region-access-point-create-validation-boundary s3control-access-grants-grant-lifecycle-and-location-referential-integrity
Issue-Shape-Id: com.amazonaws.s3control#AWSS3ControlServiceV20180820
Issue-Key: s3control|CreateMultiRegionAccessPoint|2xx->2xx|$.CreateMultiRegionAccessPointResult.RequestTokenARN
Issue-Key: s3control|DescribeMultiRegionAccessPointOperation|2xx->2xx|$.DescribeMultiRegionAccessPointOperationResult.AsyncOperation.RequestTokenARN
Issue-Key: s3control|ListAccessGrants|2xx->2xx|$.ListAccessGrantsResult.AccessGrantsList.AccessGrant.UsageLimit
Issue-Commit: b72f61b23186f03a5fd84473140fabd1f2929809

# s3control success response members diverge from recorded AWS bodies

> _Note: this issue was identified against the code base at commit `b72f61b23186f03a5fd84473140fabd1f2929809`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateMultiRegionAccessPoint

## Description
Three net-new wire divergences in s3control, all of them 200-status success bodies whose modeled response members carry values AWS does not produce. This bundle has 3 causes; all three must be closed.

Cause 1 of 3: CreateMultiRegionAccessPoint mints a RequestTokenARN of the wrong form (fixture s3c-multi-region-access-point-create-validation-boundary, step task#5).
AWS: Smithy: CreateMultiRegionAccessPoint -> CreateMultiRegionAccessPointResult.RequestTokenARN, target AsyncRequestTokenARN (string, @length min 1 max 1024, @pattern ^arn:.+$); HTTP POST /v20180820/async-requests/mrap/create, success code 200; input carries required ClientToken (@idempotencyToken, @pattern ^\S+$) and Details (CreateMultiRegionAccessPointInput). The model constrains only the ARN prefix, so for the token's internal form this is recorded behavior only: the recording returns an async-request token ARN of the form arn:aws:s3:<control-region>:<account-id>:async-request/mrap/create/<trailing-segment>, and the fixture asserts that recorded SHAPE rather than a literal value (expected is reported as '<shaped-not-asserted>'). Fixed text of the recorded token is the 'arn:aws:s3:', ':async-request/mrap/create/' scaffolding; the variable parts are the control region, the caller's account id (from the x-amz-account-id header / host prefix) and the trailing request-identifier segment. Hypothesis: the trailing segment in the recording is derived from the request (the Multi-Region Access Point name in Details), not a freshly generated opaque id, which is why a random value fails the shape assertion.
Emulator: returns 200 with $.CreateMultiRegionAccessPointResult.RequestTokenARN set to a locally generated UUID in the trailing segment (reported as 'arn:aws:s3:us-west-2:<account>:async-request/mrap/create/<uuid>'), which does not match the shape the fixture recorded, so the step is a value_mismatch even though the status is right.
Why: a client that stores the token and later parses or correlates it (to learn which MRAP the async request belongs to, or to re-derive the token from the create parameters) gets a token whose structure it cannot interpret; an SDK or test asserting the documented token form fails against the emulator while passing against AWS.
Fix: the CreateMultiRegionAccessPoint handler's async-request token minting (and the state entry it records the token under), so the token is built from the request's own identifying parameters in the recorded layout instead of an unrelated UUID.

Cause 2 of 3: DescribeMultiRegionAccessPointOperation does not echo back the exact RequestTokenARN it was given (fixture s3c-multi-region-access-point-create-validation-boundary, step task#6).
AWS: Smithy: DescribeMultiRegionAccessPointOperation, HTTP GET /v20180820/async-requests/mrap/{RequestTokenARN+} with RequestTokenARN as a required greedy @httpLabel (target AsyncRequestTokenARN, @length 1..1024, @pattern ^arn:.+$); output AsyncOperation, whose RequestTokenARN member is documented as 'The request token associated with the request.' AWS therefore round-trips the token identically: the value in $.DescribeMultiRegionAccessPointOperationResult.AsyncOperation.RequestTokenARN is byte-for-byte the token the client passed in the URI (the same token CreateMultiRegionAccessPoint returned). Its fixed text is the ARN scaffolding; its variable parts (region, account id, trailing request id) come entirely from the request.
Emulator: returns 200, but the AsyncOperation.RequestTokenARN it serializes does not compare equal to the token supplied in the path: the replay reports value_mismatch on $.DescribeMultiRegionAccessPointOperationResult.AsyncOperation.RequestTokenARN even though the two renderings print the same characters, so the difference is in the serialized value itself rather than in the request-derived parts. Hypothesis: the greedy path label is not decoded/re-encoded symmetrically (percent-encoding or XML escaping of the ':' and '/' separators, or surrounding whitespace), so the stored token and the echoed token differ outside the printable rendering.
Why: a client polling an async MRAP request matches the response's RequestTokenARN against the token it holds to pair a poll result with its request; a token that does not round-trip identically breaks that correlation and can make a poll loop never recognise its own operation.
Fix: the DescribeMultiRegionAccessPointOperation handler's greedy-label decoding and the response serializer for AsyncOperation.RequestTokenARN, so the token read from the URI is stored and re-emitted unchanged.

Cause 3 of 3: ListAccessGrants omits the UsageLimit element from each listed grant (fixture s3control-access-grants-grant-lifecycle-and-location-referential-integrity, step task#8).
AWS: Smithy: ListAccessGrants -> ListAccessGrantsResult.AccessGrantsList (list of ListAccessGrantEntry, member @xmlName 'AccessGrant'); the model declares no UsageLimit member on ListAccessGrantEntry, so this is recorded behavior only: the recorded AWS 200 body carries a UsageLimit element inside each <AccessGrant> entry, present with its default numeric value for a grant created without an explicit limit. The element's name is fixed text; its value is state (the grant's usage limit, the default when none was requested).
Emulator: returns 200 but serializes each AccessGrant entry without the element at all - the replay reports $.ListAccessGrantsResult.AccessGrantsList.AccessGrant.UsageLimit as missing (expected '0', got None).
Why: a client that reads the grant's usage limit off a listing gets no element and must treat it as absent/unknown, so it cannot distinguish 'no limit configured' from 'field not returned'; XML binders that expect the element on every entry see a shape narrower than AWS's.
Fix: the ListAccessGrants response serializer for the AccessGrantsList entries (and, if the value is not yet tracked, the access-grant state record it is read from), so every listed grant carries the element the recording shows.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the booted emulator (emulator_up=true, bootstrap_signal=false, 331 steps, 296 passed, score 89.4%). The three unowned s3control divergences:

Cause 1 - fixture s3c-multi-region-access-point-create-validation-boundary step task#5, operation CreateMultiRegionAccessPoint, verdict mismatch, expected_status 200 / actual_status 200:
  $.CreateMultiRegionAccessPointResult.RequestTokenARN: value_mismatch (expected '<shaped-not-asserted>', got 'arn:aws:s3:us-west-2:000000000000:async-request/mrap/create/425f2a3d-d58c-4a7b-8a2a-a00864d34218')
  (the UUID, the account id and the region in that actual value are per-run/per-environment values; the requirement is the recorded token layout, not these literals)

Cause 2 - fixture s3c-multi-region-access-point-create-validation-boundary step task#6, operation DescribeMultiRegionAccessPointOperation, verdict mismatch, expected_status 200 / actual_status 200:
  $.DescribeMultiRegionAccessPointOperationResult.AsyncOperation.RequestTokenARN: value_mismatch (expected 'arn:aws:s3:us-west-2:000000000000:async-request/mrap/create/425f2a3d-d58c-4a7b-8a2a-a00864d34218', got 'arn:aws:s3:us-west-2:000000000000:async-request/mrap/create/425f2a3d-d58c-4a7b-8a2a-a00864d34218')
  Expected here is the token the step sent in the request path (chained from the previous step), so the two renderings print identically while the compared values differ: the echo is not byte-identical to the token supplied.

Cause 3 - fixture s3control-access-grants-grant-lifecycle-and-location-referential-integrity step task#8, operation ListAccessGrants, verdict mismatch, expected_status 200 / actual_status 200:
  $.ListAccessGrantsResult.AccessGrantsList.AccessGrant.UsageLimit: missing (expected '0', got None)

None of the three carries a stub_path, and all three report owned_by = [] - no open issue holds their replay keys. The other 13 clusters in this replay are owned by 20261110T084205Z-s3control-getaccesspoint-success-bodies-diverge-endpoints-map-order.md, 20261106T121347Z-s3control-put-operations-diverge-access-point-policy-validation.md, 20261106T121347Z-s3control-access-point-reads-diverge-authorization-and-existence.md and 20261029T192817Z-s3control-batch-operations-jobs-never-settle-and-refuse.md and are not re-filed here; none of those four covers MRAP async request tokens or ListAccessGrants.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it; confirm /__admin/health answers 200.
2. Replay the wire corpus simparity-wire-aws-v1 (replay_wire_corpus with the corpus bound).
3. Cause 1: replay fixture s3c-multi-region-access-point-create-validation-boundary and inspect step task#5 - CreateMultiRegionAccessPoint (POST /v20180820/async-requests/mrap/create with x-amz-account-id, a ClientToken and Details). Compare the returned $.CreateMultiRegionAccessPointResult.RequestTokenARN against the recorded token's shape: the emulator's trailing path segment is a generated UUID.
4. Cause 2: in the same fixture, step task#6 - take the token returned by step task#5 and GET /v20180820/async-requests/mrap/<RequestTokenARN>. Compare $.DescribeMultiRegionAccessPointOperationResult.AsyncOperation.RequestTokenARN byte-for-byte with the token sent in the path; the replay reports value_mismatch.
5. Cause 3: replay fixture s3control-access-grants-grant-lifecycle-and-location-referential-integrity and inspect step task#8 - ListAccessGrants (GET /v20180820/accessgrantsinstance/grants) after the grant is created. Each <AccessGrant> entry in the body is serialized without the UsageLimit element the recording shows.
6. Re-replay all three steps after a fix: s3c-multi-region-access-point-create-validation-boundary#task#5, s3c-multi-region-access-point-create-validation-boundary#task#6 and s3control-access-grants-grant-lifecycle-and-location-referential-integrity#task#8 must all pass.
