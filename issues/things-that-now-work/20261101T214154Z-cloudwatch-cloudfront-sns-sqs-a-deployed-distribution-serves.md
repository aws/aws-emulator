Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront__cloudwatch__sns__sqs.obs-cloudfront-2-4xx-error-rate-alarm-notifies-sqs-untouched-control-stays-ok
Issue-Key: cloudfront__cloudwatch__sns__sqs.obs-cloudfront-2-4xx-error-rate-alarm-notifies-sqs-untouched-control-stays-ok|_poll_until#18|Timeout|raise AssertionError(f"timed out waiting for {description} after {timeout_s}s (last: {last!r})")
Issue-Commit: 446e5d40b4aa72232e72e50ee85b856a1859ebd6

# cloudwatch__cloudfront__sns__sqs: a Deployed distribution serves no viewer traffic, so AWS/CloudFront 4xxErrorRate is never produced and the alarm→SNS→SQS notification path is unreachable

> _Note: this issue was identified against the code base at commit `446e5d40b4aa72232e72e50ee85b856a1859ebd6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CloudFront edge GET (distribution DomainName) / GetMetricStatistics (AWS/CloudFront 4xxErrorRate)

## Description
This issue carries 1 net-new root cause in the cloudwatch__cloudfront__sns__sqs behavioral service. The failing check was not marked ALREADY FILED and is not OWNED by any open issue in the replay report. The adjacent open issue 20261024T182555Z-cloudwatch-cloudfront-sns-sqs-putmetricalarm-accepts-a-cross.md covers a different cause (PutMetricAlarm accepting a cross-region SNS AlarmAction) and does not carry this check's replay key; 20261024T182555Z-cloudfront-cloudwatch-no-edge-traffic-is-ever-served.md is named for context only.

=== Cause 1 of 1: no viewer request is ever served at the edge, so the 4xx-error-rate alarm can neither fire nor be shown to stay OK ===
AWS: recorded behavior only for the edge data plane — CloudFront's viewer request path is not described by the Smithy model. Real AWS: once GetDistribution reports Distribution.Status == "Deployed" (Smithy: GetDistribution, GET /2020-05-31/distribution/{Id}), viewer requests to the distribution's DomainName are served — a GET for a key that exists in the OAC-protected S3 origin answers 200 with the object bytes, and a GET for a missing key answers a 4xx. Those requests are counted in the AWS/CloudFront namespace in us-east-1 with Region=Global, where 4xxErrorRate is the percentage of viewer requests in the period that answered 4xx. A metric alarm on 4xxErrorRate with a threshold crossed by that burst transitions to ALARM, publishes its alarm action to the co-regional SNS topic, and the subscribed SQS queue receives the notification; the untouched control distribution's alarm stays OK over the same window. Fixed parts: the namespace AWS/CloudFront, the metric name 4xxErrorRate, the Region=Global dimension and the alarm state names OK/ALARM; variable parts: distribution ids, threshold, period, topic/queue identifiers.
Emulator: the distribution's edge domain answers nothing, so neither the 4xx burst nor the single 200 positive control is ever served. No viewer requests exist to be counted, 4xxErrorRate is never produced, the alarm never leaves its initial state, nothing is published to the SNS topic and the queue stays empty. The run is cut off at the suite's 60 s per-check cap while still waiting on that chain (the check's own alarm budget is 1200 s). The check's DATAPLANE_UNIMPLEMENTED marker exists precisely to classify this as an edge data-plane failure rather than a transport error.
Why: a client building CloudFront alerting cannot do so at all. The firing path (traffic → metric → alarm → SNS → SQS) and its negative control (an untouched distribution's alarm must remain OK) are indistinguishable here, because both ends produce the same emptiness: an alarm that never fires cannot be told apart from one that correctly stays OK, so monitoring built against AWS silently "passes" against the emulator while measuring nothing.
Fix: the CloudFront edge data-plane request path (serving a Deployed distribution's DomainName, including the 4xx for a missing origin key and the 200 for a present one) and the AWS/CloudFront metric emission in us-east-1 with Region=Global that feeds the CloudWatch alarm → SNS → SQS chain.
Traces: cloudwatch__cloudfront__sns__sqs.obs-cloudfront-2-4xx-error-rate-alarm-notifies-sqs-untouched-control-stays-ok

## Evidence
Suite: simparity-behavioral-aws-v1, service cloudwatch__cloudfront__sns__sqs — 1/2 passed (50%), 1 failed, and that failure is neither ALREADY FILED nor OWNED.

[mismatch] cloudwatch__cloudfront__sns__sqs.obs-cloudfront-2-4xx-error-rate-alarm-notifies-sqs-untouched-control-stays-ok
  Expected: with the managed CachingDisabled policy so no request is absorbed by a cache hit, a burst of 5 viewer requests for a missing key answers 4xx and 1 request for a present key answers 200, putting the minute's AWS/CloudFront 4xxErrorRate (Region=Global, us-east-1) at >= 5/6 and above the 20.0 threshold; the alarm transitions to ALARM, its SNS alarm action publishes, and the subscribed SQS queue receives the notification, while the untouched control distribution's alarm stays OK.
  Actual: the Deployed distribution's edge domain serves nothing, so no 4xx and no 200 are ever produced, 4xxErrorRate never materialises, and the alarm→SNS→SQS chain is never exercised; "Error: Check timed out after 60.0s" (the check's own alarm budget is 1200 s).

## Steps to Reproduce
In us-east-1: create a private bucket with one object, CreateOriginAccessControl, then CreateDistributionWithTags for two distributions (subject + untouched control) each with an S3OriginConfig origin carrying the OAC id and DefaultCacheBehavior.CachePolicyId = the managed CachingDisabled policy. Wait until GetDistribution reports Status == Deployed for both. Create an SNS topic with an SQS subscription, and PutMetricAlarm on Namespace AWS/CloudFront, MetricName 4xxErrorRate, Dimensions DistributionId=<subject>,Region=Global, Period 60, Threshold 20.0, with the topic as AlarmAction; create the same alarm for the control distribution. Issue 5 viewer GETs for a key that does not exist in the bucket (expect 4xx each) and 1 GET for the key that does (expect 200) against the subject distribution's DomainName, then poll the SQS queue for the ALARM notification and confirm the control alarm stays OK.
