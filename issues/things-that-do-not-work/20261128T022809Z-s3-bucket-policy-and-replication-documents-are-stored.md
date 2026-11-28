Issue-Source: trace-sweep-divergence
Issue-Trace: bucket-policy-document-syntax-refusals-keep-stored-policy
Issue-Step: task#9
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bucket-policy-document-syntax-refusals-keep-stored-policy s3-bucket-policy-missing-resource-element s3-bucket-policy-version-element-validation s3-cfoac-oac-grant-policy-document-mistakes s3-bucket-lifecycle-notifications-replication s3-replication-sse-kms-selection-status-enum-refused s3-replication-and-filter-repeated-tag-key-refused s3-replication-metrics-status-outside-enum-refused s3-replication-rule-existing-object-replication-refused-at-any-status s3-replication-owner-override-closed-enum s3-replication-rule-status-outside-enum-refused s3-replication-time-control-status-closed-enum
Issue-Shape-Id: com.amazonaws.s3#AmazonS3
Issue-Key: s3|PutBucketPolicy|4xx->2xx|$
Issue-Key: s3|PutBucketPolicy|4xx->2xx|status
Issue-Key: s3|PutBucketPolicy|4xx->4xx|$.Error.Message
Issue-Key: s3|GetBucketPolicy|2xx->2xx|$.Statement[*].Action
Issue-Key: s3|GetBucketPolicy|2xx->2xx|$.Statement[*].Sid
Issue-Key: s3|GetBucketPolicy|4xx->2xx|$.Error
Issue-Key: s3|GetBucketPolicy|4xx->2xx|$headers.content-type
Issue-Key: s3|GetBucketPolicy|4xx->2xx|status
Issue-Key: s3|PutBucketPolicy|4xx->4xx|$.Error.Detail
Issue-Key: s3|GetBucketPolicy|2xx->2xx|$.Statement[*]
Issue-Key: s3|PutBucketReplication|4xx->4xx|$.Error.Message
Issue-Key: s3|PutBucketReplication|4xx->2xx|$
Issue-Key: s3|PutBucketReplication|4xx->2xx|status
Issue-Key: s3|GetBucketReplication|4xx->2xx|$.Error
Issue-Key: s3|GetBucketReplication|4xx->2xx|status
Issue-Key: s3|PutBucketVersioning|4xx->2xx|$
Issue-Key: s3|PutBucketVersioning|4xx->2xx|status
Issue-Commit: 8e0150f548083d70861fe6ec8aea207c225cbc47

# s3 bucket policy and replication documents are stored without validation and not returned verbatim

> _Note: this issue was identified against the code base at commit `8e0150f548083d70861fe6ec8aea207c225cbc47`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutBucketPolicy

## Description
10 net-new causes sharing ONE fault: the two document-valued bucket sub-resources — the bucket policy (PutBucketPolicy, a raw JSON httpPayload per Smithy PutBucketPolicyRequest.Policy) and the replication configuration (PutBucketReplication, an XML httpPayload) — are persisted without the per-document semantic validation AWS performs, the stored document is not returned byte-identical by the paired Get, and the cross-resource invariant that replication locks bucket versioning is not enforced. The Smithy model declares errors: [] on PutBucketPolicy, GetBucketPolicy, PutBucketReplication, GetBucketReplication and PutBucketVersioning, so every code/message below is recorded behavior only; quoted messages are the fixed text of their error class, with any names/ids coming from the submitted document or from emulator state.

Cause 1 of 10 — PutBucketPolicy accepts a statement with no Principal. AWS: refused 400 with Code 'MalformedPolicy' and Message 'Missing required field Principal' (fixed text naming the absent field), and the previously stored policy is left untouched. Emulator: 204 No Content with an empty body; the policy is stored. Why: a caller probing whether a generated policy is legal is told yes, then relies on a bucket policy AWS would have refused — a silent authorization gap. Fix: the PutBucketPolicy document validator, which must check required statement fields before the state write. Traces: bucket-policy-document-syntax-refusals-keep-stored-policy#task#9 (and #task#10), s3-bucket-policy-missing-resource-element#task#2, s3-bucket-policy-version-element-validation#task#2, s3-cfoac-oac-grant-policy-document-mistakes#task#6 (and #task#7).

Cause 2 of 10 — PutBucketPolicy reports the wrong message for a JSON-syntax refusal. AWS: a body that is not JSON (or whose first byte is not '{') is refused with Message "Policies must be valid JSON and the first byte must be '{'" — fixed text, no variable part. Emulator: the same 400 but with its own sentence about a non-empty Statement list, Effect of Allow or Deny and an Action, i.e. the semantic-shape message is used for a syntax failure. Why: the message tells the caller to fix the policy's semantics when the actual problem is that the payload is not JSON at all. Fix: the PutBucketPolicy validator must separate the JSON-parse stage from the statement-shape stage and emit each stage's own message. Traces: bucket-policy-document-syntax-refusals-keep-stored-policy#task#4 (and #task#5, #task#6, #task#7, #task#8).

Cause 3 of 10 — GetBucketPolicy does not return the stored policy document verbatim. AWS: Smithy GetBucketPolicyOutput.Policy is the httpPayload — the document is echoed exactly as submitted, so a statement's Sid and its Action are the ones the caller wrote. Emulator: the returned document carries a generated Sid and an empty Action list in place of the submitted ones. Why: policy-as-code tooling diffs the read against its source of truth and sees a permanent drift it cannot reconcile; an empty Action silently changes what the policy grants. Fix: the bucket-policy state must hold the submitted document and GetBucketPolicy must serve it unmodified rather than re-serializing a parsed model. Traces: bucket-policy-document-syntax-refusals-keep-stored-policy#task#11.

Cause 4 of 10 — GetBucketPolicy answers 200 with a JSON content-type where AWS answers 404. AWS: when no policy is set the read is refused 404 with Code 'NoSuchBucketPolicy', Message 'The bucket policy does not exist' (fixed text) and <BucketName> = the bucket as addressed, served as application/xml — every S3 error body is XML. Emulator: 200, no Error element, content-type 'application/json'. Why: the error body is unparseable by an XML-aware S3 client even once the status is fixed. Fix: GetBucketPolicy's not-found branch, plus the error serializer's content-type. Traces: s3-bucket-policy-missing-resource-element#task#3, s3-bucket-policy-version-element-validation#task#3.

Cause 5 of 10 — PutBucketPolicy's MalformedPolicy refusal omits <Detail>. AWS: the refusal carries <Detail> naming the offending element of the submitted document, e.g. for an unknown service principal the quoted pair '"SERVICE" : "<service>"' — fixed punctuation with the service name taken from the document. Recorded behavior only. Emulator: the status and code match but Detail is absent. Why: the caller cannot locate which statement element of a large policy failed. Fix: the PutBucketPolicy validator's error serializer. Traces: s3-cfoac-oac-grant-policy-document-mistakes#task#5.

Cause 6 of 10 — a bucket policy refused by Cause 1 is nevertheless visible on the subsequent GetBucketPolicy. AWS: a refused PutBucketPolicy leaves the stored policy exactly as it was, so the read returns only the statements that were accepted. Emulator: the read returns an extra statement — the one from the rejected write. Why: a rejected authorization change takes effect anyway, which is the opposite of fail-closed. Fix: follows from Cause 1 — validation must run before the state write, not after. Traces: s3-cfoac-oac-grant-policy-document-mistakes#task#8.

Cause 7 of 10 — PutBucketReplication validates rule members in the wrong order. AWS: for a V2 replication configuration the missing-member refusal names the FIRST member missing in AWS's own check order; with both Priority and DeleteMarkerReplication absent the message is 'Priority must be specified for this version of Cross Region Replication configuration schema. Please refer to S3 Developer Guide for more information.' (fixed text; only the member name varies with which member is missing). Emulator: the same status but the message names DeleteMarkerReplication. Why: a caller repairing the configuration one message at a time adds the wrong member first and loops. Fix: the replication-configuration validator's member check order. Traces: s3-bucket-lifecycle-notifications-replication#task#11, s3-replication-sse-kms-selection-status-enum-refused#task#9.

Cause 8 of 10 — PutBucketReplication accepts an And-filter with a repeated Tag Key. AWS: refused 400 with Code 'InvalidRequest' and Message 'Duplicate Tag Keys are not allowed.' (fixed text), nothing stored. Emulator: 200 with an empty body; the configuration is stored. Why: a replication rule with duplicate tag keys has undefined matching semantics; the caller believes it was accepted. Fix: the replication filter validator (duplicate-key check across the And block). Traces: s3-replication-and-filter-repeated-tag-key-refused#task#7, s3-replication-metrics-status-outside-enum-refused#task#9, s3-replication-rule-existing-object-replication-refused-at-any-status#task#8.

Cause 9 of 10 — GetBucketReplication answers 200 where AWS reports the configuration absent. AWS: with no replication configuration the read is refused 404 with Code 'ReplicationConfigurationNotFoundError', Message 'The replication configuration was not found' (fixed text) and <BucketName> = the bucket as addressed. Emulator: 200 with no Error element. Why: the read confirms a configuration the service never accepted (Cause 8), so reconciliation never converges. Fix: GetBucketReplication's not-found branch; follows from Cause 8 for the state part. Traces: s3-replication-metrics-status-outside-enum-refused#task#10, s3-replication-rule-existing-object-replication-refused-at-any-status#task#9.

Cause 10 of 10 — PutBucketVersioning does not refuse a versioning change while a replication configuration exists. AWS: refused 409 with Code 'InvalidBucketState' and Message 'A replication configuration is present on this bucket, so you cannot change the versioning state. To change the versioning state, first delete the replication configuration.' (fixed text). Emulator: 200 with an empty body; the versioning state changes. Why: replication requires versioning, so suspending it leaves the bucket in a state AWS makes unreachable, and a test asserting the guard passes wrongly. Fix: the PutBucketVersioning handler must consult the bucket's replication state before applying the change. Traces: s3-replication-owner-override-closed-enum#task#10, s3-replication-rule-existing-object-replication-refused-at-any-status#task#11, s3-replication-rule-status-outside-enum-refused#task#11, s3-replication-time-control-status-closed-enum#task#10.

Related open issues (neither covers these clusters — the replay reports them unowned): 20261125T125732Z-s3-putbucketpolicy-refuses-a-valid-bucket-policy-with-400.md (a valid policy refused) and 20261102T111455Z-s3-refusal-envelopes-omit-aws-contextual-error-members.md (Cause 9 of that issue notes MalformedPolicy's Detail is never reached).

## Evidence
Replay of simparity-wire-aws-v1 against the emulator.
Representative bucket-policy-document-syntax-refusals-keep-stored-policy#task#9 (PutBucketPolicy): status expected 400, got 204; body empty_body — expected {'Error': {'Code': 'MalformedPolicy', 'Message': 'Missing required field Principal', ...}}, got {}.
Other causes, same shape:
- #task#4: $.Error.Message expected "Policies must be valid JSON and the first byte must be '{'", got 'Policy must be a JSON policy document with a non-empty Statement list, each statement carrying an Effect of Allow or Deny and an Action'.
- #task#11 GetBucketPolicy: $.Statement[0].Sid expected 'OwnAccountRead' got 'S1'; $.Statement[0].Action type_mismatch (expected the submitted action string, got []).
- s3-bucket-policy-missing-resource-element#task#3 GetBucketPolicy: status expected 404 got 200; $.Error missing (expected Code 'NoSuchBucketPolicy', Message 'The bucket policy does not exist', BucketName = the addressed bucket); $headers.content-type expected 'application/xml' got 'application/json'.
- s3-cfoac-oac-grant-policy-document-mistakes#task#5: $.Error.Detail missing (expected the quoted '"SERVICE" : "cloudfront"' pair).
- s3-cfoac-oac-grant-policy-document-mistakes#task#8 GetBucketPolicy: $.Statement[1] extra — the statement from the rejected write is present.
- s3-bucket-lifecycle-notifications-replication#task#11 PutBucketReplication: Message expected the 'Priority must be specified ...' sentence, got the same sentence naming DeleteMarkerReplication.
- s3-replication-and-filter-repeated-tag-key-refused#task#7: status expected 400 got 200; expected Error Code 'InvalidRequest', Message 'Duplicate Tag Keys are not allowed.'
- s3-replication-metrics-status-outside-enum-refused#task#10 GetBucketReplication: status expected 404 got 200; $.Error missing (Code 'ReplicationConfigurationNotFoundError', Message 'The replication configuration was not found', BucketName).
- s3-replication-owner-override-closed-enum#task#10 PutBucketVersioning: status expected 409 got 200; expected Error Code 'InvalidBucketState' with the replication-present sentence.

## Steps to Reproduce
1. Replay fixture bucket-policy-document-syntax-refusals-keep-stored-policy. Step task#9 must refuse the Principal-less policy 400 MalformedPolicy and leave the stored policy unchanged; task#4 must carry the JSON-syntax message; task#11's GetBucketPolicy must return the submitted document byte-for-byte (Sid and Action as written).
2. Replay s3-bucket-policy-missing-resource-element: task#3 must be 404 NoSuchBucketPolicy served as application/xml.
3. Replay s3-cfoac-oac-grant-policy-document-mistakes: task#5's MalformedPolicy must carry Detail; task#8's GetBucketPolicy must not contain the statement from the rejected write.
4. Replay s3-bucket-lifecycle-notifications-replication (task#11), s3-replication-and-filter-repeated-tag-key-refused (task#7), s3-replication-metrics-status-outside-enum-refused (task#10) and s3-replication-owner-override-closed-enum (task#10) and assert the recorded status, code and message for each.
5. Confirm every refused write leaves the prior document intact and that a refused configuration is invisible to the paired Get.
