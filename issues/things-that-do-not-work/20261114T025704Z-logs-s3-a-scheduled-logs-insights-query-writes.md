Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: logs__s3.xsvc-logs-3-scheduled-query-s3-object-rows-equal-seeded-events logs__s3.xsvc-logs-3-scheduled-query-s3-put-denied-records-destination-error
Issue-Key: logs__s3.xsvc-logs-3-scheduled-query-s3-object-rows-equal-seeded-events|run#39|AssertionError|assert key.endswith(f"{rec['queryId']}.json"), (
Issue-Key: logs__s3.xsvc-logs-3-scheduled-query-s3-put-denied-records-destination-error|run#33|JSONDecodeError|grows = json.loads(s3.get_object(Bucket=bucket, Key=gkey)["Body"].read())
Issue-Commit: d4edb10874cfe8cae1b981ecc37a41d1433fca3c

# logs__s3: a scheduled Logs Insights query writes its S3 result object as <queryId>.csv with a CSV body instead of AWS's <queryId>.json JSON array

> _Note: this issue was identified against the code base at commit `d4edb10874cfe8cae1b981ecc37a41d1433fca3c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateScheduledQuery

## Description
Cause 1 of 1: for a scheduled query with an s3Configuration destination, the delivered object is named with a .csv suffix and its body is not JSON, so neither the object key nor the result rows match what AWS delivers.

AWS: a scheduled Logs Insights query whose destinationConfiguration.s3Configuration names s3://<bucket>/<prefix>/ writes one object per settled run under that prefix, keyed <prefix>/<queryId>.json where <queryId> is the execution's queryId as reported by GetScheduledQueryHistory, and the destination entry's processedIdentifier is that object's s3 URI. The object's body is a JSON array of result rows, one object per matching event carrying exactly the fields the query projected (here @message and @timestamp), in the query's sort order. Recorded behavior only — the logs Smithy model describes the scheduled-query and destination shapes, not the delivered object's key suffix or body encoding.
Emulator: the object is keyed <prefix>/<queryId>.csv (the key's fixed parts are the configured prefix and the execution's queryId; only the suffix diverges) and its body is not a JSON document at all — json.loads over it fails at the first byte — so a consumer reading the published result cannot parse it.
Why: every consumer of a scheduled query's output addresses the object by the documented <queryId>.json key and parses it as a JSON array; against the emulator the expected key does not exist (and a prefix listing yields a differently-suffixed object) and the body that is there is unparseable, so result delivery is unusable even though the run reports COMPLETE with a processedIdentifier.
Fix: the scheduled-query S3 destination writer (object key construction and result serialization) in the CloudWatch Logs scheduled-query execution path.
Traces: logs__s3.xsvc-logs-3-scheduled-query-s3-object-rows-equal-seeded-events, logs__s3.xsvc-logs-3-scheduled-query-s3-put-denied-records-destination-error.

## Evidence
logs__s3.xsvc-logs-3-scheduled-query-s3-object-rows-equal-seeded-events — AssertionError: object key 'results/<query-name>/<queryId>.csv' does not end with the execution's queryId '<queryId>' + '.json' (the query name and queryId are per-run values; the divergence is the .csv suffix).
logs__s3.xsvc-logs-3-scheduled-query-s3-put-denied-records-destination-error — json.decoder.JSONDecodeError: Expecting value: line 1 column 1 (char 0) while json.loads()-ing the body of the positive control's delivered object, i.e. the body is not the JSON array AWS writes.
Expected per AWS: key == f"{prefix}/{queryId}.json", processedIdentifier == f"s3://{bucket}/{key}", and body == a JSON array of rows holding only the projected fields (@message, @timestamp) for exactly the matching events, in timestamp order, with no non-matching event present.

## Steps to Reproduce
1. Seed a log group with matching events (and one decoy), and create a bucket plus a role the scheduler can assume with s3:PutObject on it.
2. CreateScheduledQuery with queryLanguage CWLI, a filter query, scheduleExpression cron(* * * * ? *) and destinationConfiguration {s3Configuration: {destinationIdentifier: "s3://<bucket>/<prefix>/", roleArn: <role>}}.
3. Poll GetScheduledQueryHistory for the first settled execution and read its destinations[0].processedIdentifier.
4. Compare: AWS's key ends with "<queryId>.json" and GetObject returns a JSON array of the projected rows; the emulator's key ends with "<queryId>.csv" and its body is not JSON.
