Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iot__s3.bulk-thing-registration-task-registers-things-from-s3
Issue-Key: iot__s3.bulk-thing-registration-task-registers-things-from-s3|run#38|AssertionError|assert set(thing_names) <= reported_things, (
Issue-Commit: d4edb10874cfe8cae1b981ecc37a41d1433fca3c

# iot__s3: a completed bulk thing-registration task's RESULTS report contains no per-thing records, so the registered things' ARNs cannot be read back from the report

> _Note: this issue was identified against the code base at commit `d4edb10874cfe8cae1b981ecc37a41d1433fca3c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListThingRegistrationTaskReports

## Description
Cause 1 of 1: a thing-registration task that completes successfully (status Completed, successCount equal to the number of input rows, failureCount 0, and each thing readable with DescribeThing) publishes a RESULTS report whose body holds no registration records at all — the report's lines name none of the things the task created.

AWS: ListThingRegistrationTaskReports(taskId, reportType="RESULTS") returns resourceLinks to the task's result file(s); each line of that file is a JSON object for one successfully registered row whose response.ResourceArns.thing is the ARN of the thing created, so the set of thing names derivable from the report equals the set of things the task registered. Smithy: iot#ListThingRegistrationTaskReports takes taskId and reportType and returns {resourceLinks, reportType, nextToken}; the report file's per-line record shape is recorded behavior only.
Emulator: the resourceLinks are served and fetchable, but parsing every line yields an empty set of thing ARNs — the report carries no response.ResourceArns.thing for any of the registered things, although DescribeThing answers for each of them and the task's own successCount matches the input.
Why: bulk provisioning is audited and reconciled from the RESULTS report; a client that reads the report to learn which things exist (and to map input rows to created ARNs) concludes nothing was registered, while the registry says otherwise — so the two sources of truth disagree and automation built on the report does no follow-up work.
Fix: the thing-registration task's results-report writer (the report body generated for reportType RESULTS) in the IoT bulk-registration state, so each successful row emits its record with response.ResourceArns.thing.
Traces: iot__s3.bulk-thing-registration-task-registers-things-from-s3.

The resolved issue 20261103T154858Z-iot-listthingregistrationtaskreports-stub.md made the operation answer rather than stub out; the call now succeeds, so this is the report's content, not a refusal.

## Evidence
iot__s3.bulk-thing-registration-task-registers-things-from-s3 — AssertionError: success report named [], expected it to include ['sim2real-bulk-thing-<id-a>', 'sim2real-bulk-thing-<id-b>'] (names are per-run values from the input file).
Expected per AWS: for every row of the task's input file, one line of the RESULTS report carries response.ResourceArns.thing = arn:aws:iot:<region>:<account>:thing/<thingName>, so the report's thing set includes every registered thing name.
Actual: the report's lines yield no thing ARNs at all, even though DescribeThing answers for each name and DescribeThingRegistrationTask reports status Completed with successCount == number of input rows and failureCount 0.

## Steps to Reproduce
1. Put a newline-delimited input file of thing parameters in S3 and create a role iot.amazonaws.com can assume to read it, plus a provisioning template.
2. StartThingRegistrationTask and poll DescribeThingRegistrationTask until status Completed (successCount equals the row count).
3. Call ListThingRegistrationTaskReports(taskId, reportType="RESULTS") and fetch each resourceLink: AWS's report names each created thing's ARN under response.ResourceArns.thing; the emulator's report names none.
