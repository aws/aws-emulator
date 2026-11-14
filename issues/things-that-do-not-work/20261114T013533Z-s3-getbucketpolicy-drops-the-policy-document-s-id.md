Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-vfd-s3-reference-route-and-destination-filters
Issue-Step: task#10
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-vfd-s3-reference-route-and-destination-filters
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: s3|GetBucketPolicy|2xx->2xx|$.Id
Issue-Key: s3|GetBucketPolicy|2xx->2xx|$.Statement[*].Sid
Issue-Commit: 588eab0496abf0a5bc1b8cec7adc2c989702bf8c

# s3 GetBucketPolicy drops the policy document's Id and rewrites statement Sids (1 cause)

> _Note: this issue was identified against the code base at commit `588eab0496abf0a5bc1b8cec7adc2c989702bf8c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetBucketPolicy

## Description
Cause 1 of 1: GetBucketPolicy does not return the bucket policy document as stored - the document's top-level Id member is absent and each statement's Sid comes back without the suffix it was stored with.

AWS: recorded behavior only (the policy document is an opaque JSON string member; S3 does not model its internals). GetBucketPolicy answers HTTP 200 with the policy document byte-for-byte as it was last put, so a log-delivery policy on a flow-log destination bucket reads back with its top-level Id ('AWSLogDeliveryWrite20150319' in the recording) and with each statement's Sid exactly as stored ('AWSLogDeliveryWrite1' and 'AWSLogDeliveryAclCheck1'). The particular Id and Sid strings are the request's/log-delivery template's values, not a requirement; the requirement is that Id survives the round trip and that Sid values are returned unmodified.
Emulator: $.Id is missing from the returned document, and the statement Sids are returned as 'AWSLogDeliveryWrite' and 'AWSLogDeliveryAclCheck' - the trailing index of each stored Sid is gone. Hypothesis: the bucket policy is not stored verbatim but re-serialized from a parsed/normalized representation that has no slot for Id and regenerates Sids from a statement template.
Why: policy documents are compared, hashed and re-put by IaC and audit tooling; a document that reads back different from what was written makes every drift check report a permanent diff, and an attach/detach that keys a statement by Sid cannot find it.
Fix: bucket-policy storage and the GetBucketPolicy serializer in the s3 crate - keep the document as written instead of reconstructing it.
Traces: ec2-vfd-s3-reference-route-and-destination-filters#task#10.

## Evidence
Replay of trace set simparity-wire-aws-v1 (1 divergence cluster, service=s3, operation=GetBucketPolicy, verdict=mismatch, status 200->200, stub_path empty, owned_by empty): fixture ec2-vfd-s3-reference-route-and-destination-filters step task#10 - $.Id missing (expected 'AWSLogDeliveryWrite20150319', got None); $.Statement[0].Sid value_mismatch (expected 'AWSLogDeliveryWrite1', got 'AWSLogDeliveryWrite'); $.Statement[1].Sid value_mismatch (expected 'AWSLogDeliveryAclCheck1', got 'AWSLogDeliveryAclCheck').

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Replay fixture ec2-vfd-s3-reference-route-and-destination-filters up to step task#10: the fixture provisions the flow-log destination bucket with its log-delivery bucket policy, then calls GetBucketPolicy.
3. Compare the returned document with the one that was stored - the top-level Id is absent and both statement Sids come back without their trailing index.
