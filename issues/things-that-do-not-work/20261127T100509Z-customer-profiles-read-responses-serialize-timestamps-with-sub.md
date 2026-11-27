Issue-Source: trace-sweep-divergence
Issue-Trace: customer-profiles-get-stream-for-segments-not-found-without-an-associated-stream
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: customer-profiles-get-stream-for-segments-not-found-without-an-associated-stream get-stream-for-segments-rejects-a-domain-name-outside-its-pattern customer-profiles-stop-upload-job-refuses-a-job-that-was-never-started customer-profiles-stop-upload-job-unknown-job
Issue-Shape-Id: com.amazonaws.customerprofiles#CustomerProfiles_20200815
Issue-Key: customer-profiles|GetDomain|2xx->2xx|$.CreatedAt
Issue-Key: customer-profiles|GetDomain|2xx->2xx|$.LastUpdatedAt
Issue-Key: customer-profiles|GetDomain|2xx->2xx|$.Stats.PendingIngestionObjectCount
Issue-Key: customer-profiles|GetUploadJob|2xx->2xx|$.CreatedAt
Issue-Commit: 1d46c777556b138301c0ca0bdf752665685bd84a

# customer-profiles read responses serialize timestamps with sub-second precision and omit a recorded Stats member

> _Note: this issue was identified against the code base at commit `1d46c777556b138301c0ca0bdf752665685bd84a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetDomain

## Description
Two response-body serialization defects on customer-profiles read operations: the emulator's epoch-second timestamps do not have the shape AWS returns, and GetDomain's Stats structure is missing a member AWS populates. THIS ISSUE HAS 2 CAUSES; both must be fixed before it is closed.

Cause 1 of 2: GetDomain returns timestamps with fractional-second precision and omits Stats.PendingIngestionObjectCount.
AWS: Smithy GetDomain -> http {method: GET, uri: /domains/{DomainName}, code: 200}; output GetDomainResponse has required CreatedAt and LastUpdatedAt, both targeting the service's `timestamp` shape. That shape carries NO @timestampFormat trait, so under restJson1 they serialize as epoch-seconds numbers. Recorded behavior: AWS emits those epoch-second numbers at whole-second granularity (the fixture asserts the value's shape, not a literal instant, because the instant differs per run). Stats targets DomainStats; recorded behavior only - the response carries Stats.PendingIngestionObjectCount (0 for a domain with nothing queued), a member the checked-in model's DomainStats (ProfileCount, MeteringProfileCount, ObjectCount, TotalSize) does not declare, so the recording is the authority for it. Like the other DomainStats counters it is a count that is always present, defaulting to 0.
Emulator: status 200 (correct) but CreatedAt and LastUpdatedAt are emitted as epoch seconds carrying sub-second fractional digits, which fails the recorded timestamp shape, and Stats.PendingIngestionObjectCount is absent (expected 0, got None).
Why: a client that reads Stats.PendingIngestionObjectCount to decide whether ingestion has drained gets a missing field instead of a number - in a typed SDK that is a null/absent counter, so ingestion-drain polling and dashboard rendering break. The timestamp shape divergence means deserializers and assertions that expect AWS's whole-second epoch numbers see a different numeric shape for a required member.
Fix: the GetDomain response serializer and the domain state that backs it - serialize `timestamp` members in the shape AWS returns (whole-second epoch seconds) and always populate the full DomainStats member set, including PendingIngestionObjectCount, defaulting to 0.
Traces: customer-profiles-get-stream-for-segments-not-found-without-an-associated-stream#task#2, get-stream-for-segments-rejects-a-domain-name-outside-its-pattern#task#4.

Cause 2 of 2: GetUploadJob returns CreatedAt with the same fractional-second precision.
AWS: Smithy GetUploadJob -> http {method: GET, uri: /domains/{DomainName}/upload-jobs/{JobId}, code: 200}, readonly; output member CreatedAt (jsonName CreatedAt) targets the same `timestamp` shape with no @timestampFormat, so restJson1 epoch seconds. Recorded behavior: whole-second epoch granularity, asserted by shape because the instant is per-run.
Emulator: status 200 (correct) but $.CreatedAt is an epoch-seconds number with sub-second fractional digits, diverging from the recorded shape. This is the same serializer defect as Cause 1 on a different operation, so confirm it separately - fixing GetDomain alone does not close it.
Why: a client that formats or compares upload-job creation times against AWS-shaped values sees a differently shaped number for the same field.
Fix: the GetUploadJob response serializer (and any shared timestamp-serialization helper it uses with GetDomain).
Traces: customer-profiles-stop-upload-job-refuses-a-job-that-was-never-started#task#4, customer-profiles-stop-upload-job-unknown-job#task#5.

Note: the exact epoch values in the replay output are per-run clock readings and are not requirements - the requirement is the shape/granularity of the serialized timestamp. No open issue covers either cause; the open issue "Implement customer-profiles StopUploadJob: unimplemented scaffold stub" concerns the StopUploadJob operation itself, not GetUploadJob's response body.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up):

Cause 1 - GetDomain, fixture customer-profiles-get-stream-for-segments-not-found-without-an-associated-stream step task#2 (cluster also covers get-stream-for-segments-rejects-a-domain-name-outside-its-pattern#task#4): verdict mismatch, status 200 both sides.
  $.CreatedAt: value_mismatch (expected '<shaped-not-asserted>', got 1795773754.9171891)
  $.LastUpdatedAt: value_mismatch (expected '<shaped-not-asserted>', got 1795773754.9171891)
  $.Stats.PendingIngestionObjectCount: missing (expected 0, got None)

Cause 2 - GetUploadJob, fixture customer-profiles-stop-upload-job-refuses-a-job-that-was-never-started step task#4 (cluster also covers customer-profiles-stop-upload-job-unknown-job#task#5): verdict mismatch, status 200 both sides.
  $.CreatedAt: value_mismatch (expected '<shaped-not-asserted>', got 1795773755.758359)

(The 1795773754.9171891 / 1795773755.758359 readings are this run's clock values; the divergence is the fractional sub-second shape, not the instant.)

Model evidence: smithy_query operation=GetDomain,GetUploadJob and shape=DomainStats,timestamp on services/service-com-amazonaws-customerprofiles-customerprofiles-20200815/smithy-model/com-amazonaws-customerprofiles-customerprofiles-20200815.json - `timestamp` has no @timestampFormat (restJson1 epoch-seconds) and DomainStats declares no PendingIngestionObjectCount, so that member is grounded in the recording alone.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay the wire corpus (trace set simparity-wire-aws-v1).
3. Cause 1: replay fixture customer-profiles-get-stream-for-segments-not-found-without-an-associated-stream, step task#2 - GET /domains/{DomainName} for a created domain. Inspect CreatedAt / LastUpdatedAt (must be AWS-shaped whole-second epoch numbers, not sub-second floats) and Stats.PendingIngestionObjectCount (must be present, 0 when nothing is queued). Also re-check get-stream-for-segments-rejects-a-domain-name-outside-its-pattern#task#4.
4. Cause 2: replay fixture customer-profiles-stop-upload-job-refuses-a-job-that-was-never-started, step task#4 - GET /domains/{DomainName}/upload-jobs/{JobId} for an existing job, and inspect CreatedAt for the same fractional-second shape. Also re-check customer-profiles-stop-upload-job-unknown-job#task#5.
5. Both causes must pass before closing.
