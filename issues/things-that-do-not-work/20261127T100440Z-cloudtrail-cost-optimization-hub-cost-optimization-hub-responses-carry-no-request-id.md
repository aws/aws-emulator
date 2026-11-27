Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudtrail__cost-optimization-hub.unpermitted-update-enrollment-status-is-denied-and-audited-as-access-denied-rejected cloudtrail__cost-optimization-hub.update-enrollment-status-is-audited-as-a-write-and-its-read-as-read-only cloudtrail__cost-optimization-hub.update-preferences-is-audited-with-the-commitment-it-set
Issue-Key: cloudtrail__cost-optimization-hub.unpermitted-update-enrollment-status-is-denied-and-audited-as-access-denied-rejected|_join_key#25|KeyError|rid = response["ResponseMetadata"]["RequestId"]
Issue-Key: cloudtrail__cost-optimization-hub.update-enrollment-status-is-audited-as-a-write-and-its-read-as-read-only|_rid#9|KeyError|rid = response["ResponseMetadata"]["RequestId"]
Issue-Key: cloudtrail__cost-optimization-hub.update-preferences-is-audited-with-the-commitment-it-set|_request_id#13|KeyError|rid = response["ResponseMetadata"]["RequestId"]
Issue-Commit: 1d46c777556b138301c0ca0bdf752665685bd84a

# Cost Optimization Hub responses carry no request-id header, so ResponseMetadata.RequestId is absent and no call can be correlated to its CloudTrail event

> _Note: this issue was identified against the code base at commit `1d46c777556b138301c0ca0bdf752665685bd84a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateEnrollmentStatus

## Description
Cause 1 of 1: every Cost Optimization Hub response the emulator serves omits the protocol-level request-id response header, so an SDK builds a response whose `ResponseMetadata` has no `RequestId` key at all. Failing checks: cloudtrail__cost-optimization-hub.unpermitted-update-enrollment-status-is-denied-and-audited-as-access-denied-rejected, cloudtrail__cost-optimization-hub.update-enrollment-status-is-audited-as-a-write-and-its-read-as-read-only, cloudtrail__cost-optimization-hub.update-preferences-is-audited-with-the-commitment-it-set.

AWS: Smithy: service `com.amazonaws.costoptimizationhub#CostOptimizationHubService`, version 2022-07-26, protocol `awsJson1_0`, awsService sdkId "Cost Optimization Hub" / endpointPrefix `cost-optimization-hub`, sigv4Name `cost-optimization-hub`. The request id is not a modelled output member — it is a protocol-level response header, so this is recorded behavior only: for an `awsJson1_0` AWS service every response, success or error, carries a request-id header (`x-amzn-RequestId`; botocore also accepts `x-amzn-request-id`), and the AWS SDKs surface it as `ResponseMetadata.RequestId`. The recorded behavior the checks encode is that the value is a lowercase UUID (8-4-4-4-12 hex, 36 chars) and that it is NEWLY MINTED PER CALL — two calls, even the same operation with the same input, never answer with the same id — and that CloudTrail stores exactly that value as the event's `requestID`, which is what makes an API call joinable to its audit record. The affected operations are all of the ones these checks touch: ListEnrollmentStatuses, UpdateEnrollmentStatus, UpdatePreferences and GetPreferences; nothing in the evidence suggests the omission is operation-specific rather than service-wide, and the error path is in the same position (the 400 `AccessDeniedException` the denied UpdateEnrollmentStatus answers also had no request id for the check to read).

Emulator: the header is absent, so `response["ResponseMetadata"]["RequestId"]` raises `KeyError: 'RequestId'`. The operations themselves answer correctly — the hub accepted the opt-in, returned `status` "Active"/"Inactive" as sent, echoed `preferredCommitment`, and refused the ungranted update — so this is purely the response envelope: the body is right and the correlating header is missing. The same omission appears on the success responses of ListEnrollmentStatuses (check 1, line 174), UpdateEnrollmentStatus (check 2, line 104) and UpdatePreferences (check 3, line 114), and on the `ClientError` response of the denied UpdateEnrollmentStatus, which the first check reads through the same accessor.

Why: a client cannot do the one thing a request id exists for — name the call in a support case or join it to its audit trail. Any CloudTrail-based audit of who changed enrollment or savings preferences breaks: the caller has no key to look the event up by, and `requestID` on the delivered record matches nothing the caller holds. Code that logs `ResponseMetadata['RequestId']` on every call, which is the standard retry/diagnostic idiom, raises `KeyError` against the emulator while working against AWS — a crash, not a degraded value. A per-call id also must be unique; serving one shared or static id would reintroduce the same defect in a quieter form, since two different calls would then be indistinguishable in the trail.

Because the three checks abort at the first request-id read, they never reach their CloudTrail assertions, so the emulator's auditing of Cost Optimization Hub events (eventSource `cost-optimization-hub.amazonaws.com`, `managementEvent` true, `readOnly` false for the writes and true for the reads, `requestParameters`/`responseElements` carrying the status and the commitment, `errorCode` "AccessDenied" on the refusal) is UNVERIFIED by this run rather than known-good. Expect those assertions to be exercised for the first time once the header is served.

Fix: the response-serialization seam for this service's `awsJson1_0` protocol — the framework layer that renders a handler's output (and a refusal) into an HTTP response must mint a fresh UUID per request and set it as the request-id response header on success and error paths alike, and the same value must be what the CloudTrail event recorder stores as `requestID`. Nothing in the Cost Optimization Hub service crate touches request ids today (its `src/service.rs` documents only the five routing/refusal seams and has no request-id mention; `src/ops/mod.rs` is a generated include), which is consistent with the header being a framework responsibility rather than a per-operation one.

Traces: the three failing checks listed above; each one's traceback ends at `response["ResponseMetadata"]["RequestId"]` with `KeyError: 'RequestId'`.

## Evidence
Expected (AWS): every Cost Optimization Hub response carries a request-id header, so botocore exposes `response["ResponseMetadata"]["RequestId"]` as a lowercase 36-char UUID, distinct for each call, and CloudTrail's record for that call has `requestID` equal to it.

Actual (emulator): `ResponseMetadata` contains no `RequestId` key.

  check: cloudtrail__cost-optimization-hub.update-enrollment-status-is-audited-as-a-write-and-its-read-as-read-only
    UpdateEnrollmentStatus(status="Inactive") returned status "Inactive" (body correct), then
      line 104, in run: write_id = _rid(write)
      line  33, in _rid: rid = response["ResponseMetadata"]["RequestId"]
      KeyError: 'RequestId'

  check: cloudtrail__cost-optimization-hub.update-preferences-is-audited-with-the-commitment-it-set
    UpdatePreferences(preferredCommitment=...) echoed the commitment sent, then
      line 114, in run: write_id = _request_id(write)
      line  37, in _request_id: rid = response["ResponseMetadata"]["RequestId"]
      KeyError: 'RequestId'

  check: cloudtrail__cost-optimization-hub.unpermitted-update-enrollment-status-is-denied-and-audited-as-access-denied-rejected
    the role-scoped ListEnrollmentStatuses succeeded, then
      line 174, in run: allowed_id = _join_key(allowed)
      line  64, in _join_key: rid = response["ResponseMetadata"]["RequestId"]
      KeyError: 'RequestId'

Note on the per-run values: the UUID itself, and the specific status/commitment used by a run, are values a run produces and are NOT the requirement. The requirement is the header's presence on every response, its UUID shape, its uniqueness per call, and its equality with the CloudTrail event's `requestID`.

## Steps to Reproduce
1. Point an AWS SDK client at the emulator for service `cost-optimization-hub` in us-east-1 (the hub serves a single region-pinned endpoint).
2. Call ListEnrollmentStatuses. Read `response["ResponseMetadata"]["RequestId"]` -> KeyError: 'RequestId'. Against AWS this is a lowercase UUID.
3. Call UpdateEnrollmentStatus(status="Active"), then UpdateEnrollmentStatus(status="Inactive"). Both bodies echo the status sent, but neither response exposes a RequestId, so the two calls cannot be told apart by id.
4. Call UpdatePreferences(preferredCommitment={"term": "OneYear", "paymentOption": "NoUpfront"}) and then GetPreferences. Same: the commitment round-trips, no RequestId on either response.
5. Trigger the refusal path: assume a role granted only `cost-optimization-hub:ListEnrollmentStatuses` and call UpdateEnrollmentStatus with its credentials. The AccessDeniedException is raised, but `exc.response["ResponseMetadata"]["RequestId"]` is likewise absent.
6. Replay the three checks named in trace_ids to confirm: each aborts at its first request-id read today, and should proceed into its CloudTrail lookup once the header is served.
