Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront__kinesis.realtime-log-config-delivers-edge-request-record-to-kinesis cloudfront__kinesis.obs-cloudfront-3-realtime-log-role-without-put-record-delivers-nothing cloudfront__kinesis.xsvc-cloudfront-3-realtime-log-role-denied-putrecord-delivers-nothing
Issue-Key: cloudfront__kinesis.realtime-log-config-delivers-edge-request-record-to-kinesis|_poll_until#43|Timeout|time.sleep(interval_s)
Issue-Key: cloudfront__kinesis.obs-cloudfront-3-realtime-log-role-without-put-record-delivers-nothing|_poll_until#31|Timeout|time.sleep(interval_s)
Issue-Key: cloudfront__kinesis.xsvc-cloudfront-3-realtime-log-role-denied-putrecord-delivers-nothing|_poll_until#41|Timeout|time.sleep(interval_s)
Issue-Commit: cfbdbd19120bc4eb7cd4848a49718d75d65d840b

# cloudfront__kinesis: CloudFront real-time log configs never deliver a record to the configured Kinesis stream, so all three real-time-logging checks hang to the 60 s cap

> _Note: this issue was identified against the code base at commit `cfbdbd19120bc4eb7cd4848a49718d75d65d840b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateRealtimeLogConfig

## Description
Cause 1 of 1: CloudFront real-time logging has no delivery path — a distribution whose cache behaviour names a RealtimeLogConfig produces no log record on the configured Kinesis stream, and none of the three checks that depend on that delivery can make progress; each is killed at the harness's 60 s cap with nothing observed.

AWS: Smithy: CreateRealtimeLogConfig, HTTP POST /2020-05-31/realtime-log-config, success code 201; required input members EndPoints (EndPointList — the Kinesis stream ARN plus the IAM role CloudFront assumes to write it), Fields (FieldList), Name, SamplingRate; modeled errors AccessDenied (403), InvalidArgument (400), RealtimeLogConfigAlreadyExists (409), TooManyRealtimeLogConfigs (400). The model says nothing about delivery, so for delivery this is recorded behavior only; the recordings (replayed against real AWS in us-west-2) establish: (a) CreateRealtimeLogConfig validates neither the role's permissions nor the stream's existence/assumability at create time, so a config naming a role without kinesis:PutRecord is ACCEPTED; (b) it stores exactly the requested field SET but reorders it into CloudFront's own canonical order, and the response's Fields is the authority for positions; (c) SamplingRate is echoed as requested (100); (d) with the config attached to a CachingDisabled behaviour, each viewer request served at the edge produces exactly ONE tab-delimited record on the stream within a 300 s window, whose fields follow the config's reported order and carry cs-uri-stem == "/<the requested key>", sc-status == "200", x-edge-response-result-type == "Miss" on a cold object, a non-empty c-ip, and a timestamp inside the caller's own clock window; (e) when the role is denied kinesis:PutRecord (or lacks it entirely) NOTHING is delivered to that stream, while a control stream on the same distribution still receives every record — the absence is the contract.

Emulator: none of the three checks produces any of the above. All three are reported as "Check timed out after 60.0s" with no response and no record: the real-time-logging flow (create stream -> wait ACTIVE -> create role -> CreateRealtimeLogConfig -> create distribution -> wait Deployed -> fetch the probe object through the edge domain -> read the stream) never reaches a delivery assertion, so no real-time log record is observable at all and the deny-path's "delivers nothing" contract is unverifiable because the positive control delivers nothing either. Hypothesis: there is no component that emits an edge request as a record to a RealtimeLogConfig's EndPoints stream; consistent with the already-filed sibling findings that this build's CloudFront edge domain never answers a viewer request (e.g. 20261024T182555Z-cloudfront-s3-distributions-over-s3-origins-serve-nothing.md, 20261025T054735Z-cloudfront-lambda-a-deployed-distribution-s-edge-never.md), and with the Kinesis side of the flow (CreateStream/DescribeStreamSummary -> ACTIVE, GetShardIterator/GetRecords) not yet being stood up in this build — its bootstrap issue is still open. The suite attributes these checks to the leading service token cloudfront, which does answer.

Why: a client that wires CloudFront real-time logs to Kinesis to drive dashboards, anomaly detection or per-request auditing gets an empty stream against the emulator while the same configuration produces one record per viewer request on AWS. Worse, the negative direction is indistinguishable: because the healthy configuration also delivers nothing, a client (or a test) cannot tell "my role is missing kinesis:PutRecord" from "logging works", so a permissions regression that AWS would make visible as an empty stream-versus-control divergence silently passes.

Fix: CloudFront's real-time-log state plus the edge data-plane emission path — services/service-com-amazonaws-cloudfront-cloudfront2020-05-31/src/ops/create_realtime_log_config.rs must persist the config (field set, canonical field order, SamplingRate, EndPoints) and the distribution/cache-behaviour association must be honoured by the component that serves an edge request, which has to emit one tab-delimited record per sampled request to the configured Kinesis stream via the named role (and emit nothing when that role cannot PutRecord). The Kinesis ingest side of the same path (CreateStream reaching ACTIVE, GetShardIterator/GetRecords returning the written records) must be serving for the record to be readable back.

Traces: cloudfront__kinesis.realtime-log-config-delivers-edge-request-record-to-kinesis, cloudfront__kinesis.obs-cloudfront-3-realtime-log-role-without-put-record-delivers-nothing, cloudfront__kinesis.xsvc-cloudfront-3-realtime-log-role-denied-putrecord-delivers-nothing.

## Evidence
Expected (AWS, recorded): CreateRealtimeLogConfig(Name, SamplingRate=100, Fields=["timestamp","c-ip","cs-uri-stem","sc-status","x-edge-response-result-type"], EndPoints=[{StreamType:"Kinesis", KinesisStreamConfig:{RoleARN, StreamARN}}]) -> 201 echoing the same field SET (reordered into CloudFront's canonical order) and SamplingRate 100; after the distribution is Deployed, ONE viewer GET of the probe object yields exactly one tab-delimited record on that stream within 300 s, with cs-uri-stem == "/<probe key>", sc-status == "200", x-edge-response-result-type == "Miss", non-empty c-ip and a timestamp inside the run's clock window. With a role that lacks/was denied kinesis:PutRecord, that stream stays empty while a control stream on the same distribution receives every record.

Actual (emulator): all three checks end as `Error: Check timed out after 60.0s` with no response and no record delivered — the flow never reaches its first delivery assertion, so neither the positive record nor the deny-path absence can be observed.

Model evidence: smithy_query(operation="CreateRealtimeLogConfig") -> POST /2020-05-31/realtime-log-config, success 201, required EndPoints/Fields/Name/SamplingRate, errors AccessDenied(403)/InvalidArgument(400)/RealtimeLogConfigAlreadyExists(409)/TooManyRealtimeLogConfigs(400); the model specifies no delivery behaviour, hence "recorded behavior only" for (a)-(e) above.

## Steps to Reproduce
1. Create a Kinesis stream (ShardCount 1) and poll DescribeStreamSummary until ACTIVE.
2. Create an IAM role CloudFront can assume with kinesis:DescribeStreamSummary/DescribeStream and kinesis:PutRecord/PutRecords on that stream.
3. Call CreateRealtimeLogConfig with SamplingRate 100, the field set ["timestamp","c-ip","cs-uri-stem","sc-status","x-edge-response-result-type"] and EndPoints naming that stream ARN and role ARN; note the Fields order in the response.
4. Create an S3 bucket with a probe object, create a distribution over it using the managed CachingDisabled cache policy and attach the real-time log config ARN to the default cache behaviour; wait for Status == "Deployed".
5. Open a shard iterator at the stream's horizon, GET /<probe key> through the distribution's edge domain, then poll GetRecords.
6. Observe the emulator never completes this flow: no record is ever delivered and the check is killed at the 60 s cap.
7. Repeat with a role whose policy omits (or explicitly denies) kinesis:PutRecord plus a second, correctly-permitted control stream: on AWS the broken stream stays empty while the control receives every record; on the emulator neither stream receives anything, so the absence proves nothing.

Replay: run the behavioral checks cloudfront__kinesis.realtime-log-config-delivers-edge-request-record-to-kinesis, cloudfront__kinesis.obs-cloudfront-3-realtime-log-role-without-put-record-delivers-nothing and cloudfront__kinesis.xsvc-cloudfront-3-realtime-log-role-denied-putrecord-delivers-nothing.
