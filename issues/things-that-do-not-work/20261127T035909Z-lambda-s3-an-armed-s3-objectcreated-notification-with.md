Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__s3.armed-notification-converts-csv-and-suppresses-uppercase-suffix
Issue-Key: lambda__s3.armed-notification-converts-csv-and-suppresses-uppercase-suffix|_watch_absent#26|Timeout|return found
Issue-Commit: e9aa0118b8d3778084a8f5f6890b0994ca6ea9d7

# lambda__s3: an armed s3:ObjectCreated:* notification with a prefix/suffix filter never invokes its Lambda target, so the CSV→XLSX conversion never happens and the case-sensitivity negative cannot be graded

> _Note: this issue was identified against the code base at commit `e9aa0118b8d3778084a8f5f6890b0994ca6ea9d7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutBucketNotificationConfiguration

## Description
One net-new root cause in the lambda__s3 surface.

Cause 1 of 1: a bucket notification configuration with a LambdaFunctionConfigurations entry is accepted, but an upload that matches its filter never invokes the function, so the event-driven path produces no output object at all.

AWS: PutBucketNotificationConfiguration with LambdaFunctionConfigurations[{LambdaFunctionArn, Events:["s3:ObjectCreated:*"], Filter.Key.FilterRules:[{Name:prefix,Value:raw/},{Name:suffix,Value:.csv}]}] is validated against the function's resource policy at configuration time (S3 answers 400 InvalidArgument "Unable to validate the following destination configurations" while the lambda:InvokeFunction grant for Principal s3.amazonaws.com with SourceArn arn:aws:s3:::<bucket> is not yet visible) and, once accepted, every PutObject whose key starts with the prefix and ends with the suffix causes S3 to invoke that function asynchronously with an S3 event record (Records[].s3.bucket.name, Records[].s3.object.key). The prefix rule is a literal string match, not a directory boundary, so raw/nested/<name>.csv matches prefix=raw/; the suffix rule is literal and case-SENSITIVE, so suffix=".csv" does not match a key ending ".CSV". AWS documents the configuration change as taking effect in "usually about five minutes" and events fired before it arms are dropped, which is why the check re-uploads a fresh key each round rather than waiting on one. Recorded behavior only for the delivery itself: the S3 notification→Lambda invoke path is not described by any Smithy operation (s3 PutBucketNotificationConfiguration models only the configuration document; lambda AddPermission models only the resource policy), so the requirement is the recorded delivery.
Emulator: the configuration is accepted and the function is created and reaches Active, but no object ever appears under the converted/ output prefix: the check uploads raw/<token>-freshN.csv and raw/nested/<token>-deepN.csv every 15 s and polls ListObjectsV2 on converted/ until anything new appears, and the replay ends with no new key at all — the check is killed at the replay budget ("Check timed out after 60.0s") before its first positive observation. Every assertion after that point (the workbook's cell values and OPC parts, and the negative that a ".CSV" key must be suppressed by the case-sensitive suffix rule) is therefore never reached. Hypothesis: the notification configuration is stored but no delivery is ever emitted to a Lambda destination, the same shape of failure already reported for sibling scenarios.
Why: a client that wires an upload-driven pipeline (upload to raw/, read the derived artifact from converted/) gets a bucket that accepts the wiring and silently drops every event. No error is returned anywhere: PutBucketNotificationConfiguration answers 200, GetBucketNotificationConfiguration reads the configuration back, the function exists and is Active, and only the artifact is missing — so the failure is indistinguishable from "the handler produced nothing" and is undetectable by any control-plane read. It also makes the filter semantics unobservable: an emulator that never delivers cannot be shown to honour, or to break, the literal case-sensitive suffix rule, so a client relying on S3 (not the handler) to suppress ".CSV" keys has no signal either way.
Fix: the S3 notification dispatch path behind PutBucketNotificationConfiguration — the component that, on a completed object write, evaluates the stored LambdaFunctionConfigurations (event-type match, then the literal prefix/suffix FilterRules) and performs the asynchronous lambda Invoke with the S3 event record; plus whatever state records the bucket's notification configuration for that dispatcher to read.
Traces: lambda__s3.armed-notification-converts-csv-and-suppresses-uppercase-suffix.

Open issues the replay report does NOT record as owning this check, and that do not cover it: 20261029T180026Z-dynamodb-lambda-s3-s3-notifications-never-invoke-lambda-are-accepted-without.md (same family of symptom, but its replay keys are the dynamodb__lambda__s3 ESM/notification checks), 20261126T050240Z-iam-lambda-s3-the-event-driven-csv-to-xlsx-pipeline-never.md (the CDK-deployed csv-to-xlsx pipeline under iam__lambda__s3), 20261118T064915Z-lambda-s3-stepfunctions-a-bucket-s-lambdafunctionconfigurations-notification.md and 20261121T221736Z-s3-2-net-new-causes-a-prefix-suffix.md (prefix+suffix-filtered delivery under the s3 heading). None of them carries this check's key, so this check is ungraded until the lambda__s3 filtered-delivery path works.

## Evidence
Behavioral suite replay against the emulator built from this checkout: 2377/2681 passed (88.7%). This is one of only two failing checks the report returns with no owning issue and no blocking stub.

[mismatch] lambda__s3.armed-notification-converts-csv-and-suppresses-uppercase-suffix
Error: Check timed out after 60.0s

The check's own structure shows what was never reached:
  lam.add_permission(FunctionName=..., StatementId="s3invoke", Action="lambda:InvokeFunction", Principal="s3.amazonaws.com", SourceArn=f"arn:aws:s3:::{bucket}")
  _retry_client_error(lambda: s3.put_bucket_notification_configuration(Bucket=bucket, NotificationConfiguration={"LambdaFunctionConfigurations": [{"LambdaFunctionArn": pipe["arn"], "Events": ["s3:ObjectCreated:*"], "Filter": {"Key": {"FilterRules": [{"Name": "prefix", "Value": "raw/"}, {"Name": "suffix", "Value": ".csv"}]}}}]}), codes=("InvalidArgument",), timeout_s=180.0)
  ... per round: put_object(raw/<token>-caseN.CSV), put_object(raw/nested/<token>-deepN.csv), put_object(raw/<token>-freshN.csv), then poll `set(_list_keys(ctx, bucket, prefix="converted/")) - before` every 5 s for 15 s
  assert observed is not None, "no CSV uploaded under raw/ produced any object under 'converted/' ... the notification never armed"
The replay ends with no key under converted/ having appeared, so `observed` is still None and the budget fires before the assertion can even be evaluated. The subsequent graded claims — _sheet_rows(workbook) == the probe CSV's rows, _opc_parts(workbook) >= {"[Content_Types].xml", "xl/worksheets/sheet1.xml"}, and _watch_absent(...) == [] for the ".CSV" keys the case-sensitive suffix rule must suppress — are never exercised.

The per-run values here are not part of the requirement: the bucket/function/role names are ctx.unique_name()-generated, the token is per run, and the 60 s figure is this runner's budget, not a published AWS latency. The requirement is that a matching upload produces the handler's derived object under converted/ with no further API call, and that a ".CSV" key produces none.

## Steps to Reproduce
1. Create a bucket and a python3.12 Lambda function whose role trusts lambda.amazonaws.com and allows s3:GetObject/s3:PutObject on arn:aws:s3:::<bucket>/*; wait for the function to report State=Active. The handler reads the object named in Records[].s3.object.key and PutObjects a derived key under converted/.
2. lambda AddPermission(FunctionName=<fn>, StatementId="s3invoke", Action="lambda:InvokeFunction", Principal="s3.amazonaws.com", SourceArn="arn:aws:s3:::<bucket>").
3. s3 PutBucketNotificationConfiguration(Bucket=<bucket>, NotificationConfiguration={"LambdaFunctionConfigurations":[{"LambdaFunctionArn":<fn arn>,"Events":["s3:ObjectCreated:*"],"Filter":{"Key":{"FilterRules":[{"Name":"prefix","Value":"raw/"},{"Name":"suffix","Value":".csv"}]}}}]}), retrying while it answers InvalidArgument.
4. PutObject three keys with identical CSV bytes: raw/<t>-case1.CSV, raw/nested/<t>-deep1.csv, raw/<t>-fresh1.csv.
5. Poll ListObjectsV2(Bucket=<bucket>, Prefix="converted/"). AWS: once armed, converted/<t>-fresh1.xlsx and converted/<t>-deep1.xlsx appear with no further API call (the nested key matches because prefix is a literal string), each a readable OPC package whose xl/worksheets/sheet1.xml rows equal the CSV rows; converted/<t>-case1.xlsx never appears, because suffix=".csv" is literal and case-sensitive. Emulator: the prefix stays empty — no object is ever written, so nothing can be graded.
6. Re-replay to confirm: lambda__s3.armed-notification-converts-csv-and-suppresses-uppercase-suffix.
