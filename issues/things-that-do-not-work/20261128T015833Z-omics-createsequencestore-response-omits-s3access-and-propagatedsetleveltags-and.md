Issue-Source: trace-sweep-divergence
Issue-Trace: omics-get-read-set-rejects-out-of-range-part-and-unknown-file
Issue-Step: setup#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: omics-get-read-set-rejects-out-of-range-part-and-unknown-file
Issue-Shape-Id: com.amazonaws.omics#Omics
Issue-Key: omics|CreateSequenceStore|2xx->2xx|$.propagatedSetLevelTags
Issue-Key: omics|CreateSequenceStore|2xx->2xx|$.s3Access
Issue-Key: omics|CreateSequenceStore|2xx->2xx|$.status
Issue-Commit: 429bdfecbf56545571d755c8b6ad852b4e3ed1bc

# omics CreateSequenceStore response omits s3Access and propagatedSetLevelTags and reports CREATING instead of ACTIVE

> _Note: this issue was identified against the code base at commit `429bdfecbf56545571d755c8b6ad852b4e3ed1bc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateSequenceStore

## Description
Three net-new root causes in the CreateSequenceStore response. The call succeeds with HTTP 200 and the right id/arn/creationTime, but three output members diverge at once, so a caller cannot use the store it just created the way AWS lets it. Each cause must be closed on its own before submitting. The open omics issues (20261127T141502Z-omics-7-net-new-root-causes-read-set.md, 20261029T184954Z-omics-4-net-new-root-causes-startrun-refuses.md, 20261124T224008Z-omics-tagresource-on-a-deleted-resource-s-arn.md, 20261124T171120Z-omics-getreadsetmetadata-returns-a-malformed-sequence-store-s3.md, 20261126T233811Z-omics-getreadset-serves-the-read-set-part-with.md and the scaffold-stub issues) are NOT these causes and do not cover them -- the GetReadSetMetadata issue concerns the FORM of an s3Uri inside a read set's files, whereas Cause 2 here is the complete ABSENCE of the store-level s3Access structure from CreateSequenceStore's own response.

=== Cause 1 of 3: a newly created sequence store is reported CREATING, where AWS reports it ACTIVE ===
AWS: Smithy: CreateSequenceStore.status -> SequenceStoreStatus, whose values are CREATING | ACTIVE | UPDATING | DELETING | FAILED; success status 200. Recorded behavior: the response to the create itself carries status "ACTIVE" -- the store is usable as soon as the create returns, with no non-terminal state a caller has to poll through, and the recorded fixture goes straight on to use the store (CreateMultipartReadSetUpload / UploadReadSetPart / GetReadSet) in the same session.
Emulator: the handler deliberately mints the record in the modelled non-terminal CREATING and owes a clock-driven transition to ACTIVE (services/service-com-amazonaws-omics-omics/src/ops/create_sequence_store.rs, status: crate::state::SEQUENCE_STORE_STATUS_CREATING with status_due_at_ms = now + TRANSITION_DELAY), so a caller that reads the create response observes CREATING.
Why: a client that waits for ACTIVE before using the store blocks for the emulator's transition delay where AWS makes it immediately usable, and a client that asserts the created store is ACTIVE (or that branches on CREATING meaning "not ready") takes a path AWS never puts it on. The lifecycle state of a freshly created resource is observable API behavior, not an internal detail.
Fix: the status the CreateSequenceStore handler stamps on the new record (and the transition it schedules) in that operation's handler plus the sequence-store record in crate::state -- a created store must be reported in the status AWS reports.

=== Cause 2 of 3: the s3Access structure is absent from the response ===
AWS: Smithy: CreateSequenceStore.s3Access -> SequenceStoreS3Access { s3Uri (@pattern ^s3://([a-z0-9][a-z0-9-.]{1,61}[a-z0-9])/(.{1,1024})$), s3AccessPointArn (@pattern ^arn:[^:]*:s3:[^:]*:[^:]*:accesspoint/.*$), accessLogLocation }, documented as "The S3 access metadata of the sequence store." Recorded behavior: the create response ALWAYS carries s3Access with a populated s3Uri and s3AccessPointArn (their concrete values are environment-specific and were recorded as shaped-not-asserted), and accessLogLocation null when no s3AccessConfig was supplied -- i.e. the access point is provisioned by the create, independently of whether the caller configured access logging.
Emulator: s3Access is omitted entirely (the response member is None). The handler only retains input.s3AccessConfig.access_log_location on the record, so the store-level S3 access metadata is never synthesized and reported.
Why: the S3 URI and access-point ARN of a sequence store are how a client reaches its read-set objects directly (bulk download, bucket policy, data-pipeline wiring). A caller that reads s3Access off the create response gets nothing from the emulator and fails with a missing-key/null error, and the documented capability is unobservable.
Fix: the response construction for this operation (the reported() projection in CreateSequenceStore's handler and the s3_access_output() projection on the sequence-store record in crate::state) -- a created store must report an s3Access whose s3Uri and s3AccessPointArn satisfy their modelled patterns, with accessLogLocation only when one was configured.

=== Cause 3 of 3: propagatedSetLevelTags the request supplied are not reported back ===
AWS: Smithy: CreateSequenceStore input propagatedSetLevelTags -> PropagatedSetLevelTags @length(0..50), documented as "The tags keys to propagate to the S3 objects associated with read sets in the sequence store", and the OUTPUT declares the same member. Recorded behavior: the create echoes the list the request carried, in the request's order (the recorded step supplied two tag keys and got both back).
Emulator: the member is missing from the response (None) even though the request carried a non-empty list, so the accepted configuration is not confirmed anywhere in the create's answer.
Why: a client that round-trips the create response to verify the store was configured as asked sees no propagated tag keys and must conclude the setting was dropped; anything that re-sends the response's members (infrastructure reconcilers, idempotent create wrappers) loses the configuration entirely.
Fix: the same response projection for this operation -- the stored propagatedSetLevelTags must be carried into the output (and the record must retain what the request supplied).

Traces for all three causes: omics-get-read-set-rejects-out-of-range-part-and-unknown-file#setup#1 (one replay step, three diverging response keys).

## Evidence
Wire corpus simparity-wire-aws-v1 replayed against the emulator built from this checkout (126 steps, 51 passed, 61 mismatched, 12 clusters, all service=omics). Fixture omics-get-read-set-rejects-out-of-range-part-and-unknown-file, step setup#1, CreateSequenceStore, 200 -> 200, three diverging keys in one cluster:
- $.status: value_mismatch -- expected 'ACTIVE', got 'CREATING'.
- $.s3Access: missing -- expected {'accessLogLocation': None, 's3AccessPointArn': '<shaped-not-asserted>', 's3Uri': '<shaped-not-asserted>'}, got None. (The recorded ARN/URI values are environment-specific and asserted only by shape; their PRESENCE and their modelled patterns are the requirement.)
- $.propagatedSetLevelTags: missing -- expected ['omics:sampleId', 'omics:subjectId'], got None. (Those two keys are the ones the recorded request supplied; the requirement is that the request's list is echoed, not these particular keys.)
Handler evidence: services/service-com-amazonaws-omics-omics/src/ops/create_sequence_store.rs -- the record is built with status: crate::state::SEQUENCE_STORE_STATUS_CREATING and status_due_at_ms = now_ms + TRANSITION_DELAY (lines 307-315), and the reported() projection (lines 342-357) fills s3_access from store.s3_access_output() and propagated_set_level_tags from the record. crate::state (lines 150-154) defines SEQUENCE_STORE_STATUS_CREATING / SEQUENCE_STORE_STATUS_ACTIVE. Model evidence: smithy_query operation CreateSequenceStore (output members status, s3Access, propagatedSetLevelTags) and shape SequenceStoreS3Access / SequenceStoreStatus.

## Steps to Reproduce
1. Call CreateSequenceStore(name=<any valid name>, propagatedSetLevelTags=["omics:subjectId", "omics:sampleId"]) with no s3AccessConfig (POST /sequencestore on the control-storage endpoint). The call answers HTTP 200.
2. Inspect the response body: status must be "ACTIVE" (observe "CREATING"); s3Access must be present with a populated s3Uri matching ^s3://([a-z0-9][a-z0-9-.]{1,61}[a-z0-9])/(.{1,1024})$ and a populated s3AccessPointArn matching ^arn:[^:]*:s3:[^:]*:[^:]*:accesspoint/.*$ and accessLogLocation null (observe the whole member absent); propagatedSetLevelTags must echo the two keys the request supplied (observe the member absent).
3. Re-replay fixture omics-get-read-set-rejects-out-of-range-part-and-unknown-file step setup#1 to confirm all three causes are closed.
