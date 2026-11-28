Issue-Source: trace-sweep-divergence
Issue-Trace: iotjobsdata-describe-job-execution-malformed-request-refusals
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iotjobsdata-describe-job-execution-malformed-request-refusals
Issue-Shape-Id: com.amazonaws.iotjobsdataplane#IotLaserThingJobManagerExternalService
Issue-Key: iot-jobs-data|DescribeJobExecution|4xx->4xx|$.message
Issue-Key: iot-jobs-data|DescribeJobExecution|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: iot-jobs-data|DescribeJobExecution|4xx->4xx|status
Issue-Commit: 4866adc0993f8fa67cc0ae020b08b6d4f21694d6

# iot-jobs-data DescribeJobExecution answers an out-of-range executionNumber as 404 ResourceNotFoundException

> _Note: this issue was identified against the code base at commit `4866adc0993f8fa67cc0ae020b08b6d4f21694d6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeJobExecution

## Description
Cause 1 of 1: DescribeJobExecution confuses a request-level refusal of the executionNumber query parameter with a missing-resource refusal, so it answers the wrong HTTP status, the wrong error code and the wrong message.

AWS: DescribeJobExecution binds executionNumber as an httpQuery parameter targeting ExecutionNumber (a long) and models both InvalidRequestException (httpError 400, "The contents of the request were invalid.") and ResourceNotFoundException (httpError 404) (Smithy: DescribeJobExecution.executionNumber httpQuery=executionNumber target ExecutionNumber:long; errors InvalidRequestException 400 / ResourceNotFoundException 404). The recorded response for an executionNumber the service cannot interpret as a valid execution ordinal is HTTP 400 with the fixed message text "executionNumber is out of range" — that sentence is entirely fixed text, with no part taken from the request or from state — and the x-amzn-errortype header of the modeled 400 request-validation error rather than ResourceNotFoundException. The refusal is decided from the request alone, before any job/thing lookup, so it does not depend on whether the named job exists for the named thing.

Emulator: answers HTTP 404 with x-amzn-errortype: ResourceNotFoundException and an emulator-authored message of the form "Job <jobId> not found for <thingName>" (recorded instance: "Job sim2real-probe-ijd-dvjob-04c48d99 not found for sim2real-probe-ijd-dvdev-ba0958b6"; the job id and thing name are values taken from the request, the rest is the emulator's own fixed text). It therefore reaches the job-lookup path with an executionNumber it should already have refused, and reports the lookup miss instead of the parameter refusal.

Why: a client that submits a bad executionNumber is told the job does not exist. Retry and error-classification logic keys off this: ResourceNotFoundException at 404 is a terminal "resource gone" signal that makes a device agent stop polling or re-provision, where InvalidRequestException at 400 tells it to correct the request. Code paths that branch on the error type (the AWS SDKs model these as distinct exception classes) take the wrong branch, and the 404 also hides the fact that a well-formed job exists.

Fix: DescribeJobExecution's request validation in the operation's handler — the executionNumber range check must run as a pre-flight refusal before the thing/job execution is resolved from state, and must emit the 400 InvalidRequestException envelope (code InvalidRequestException, message "executionNumber is out of range") instead of falling through to the not-found path.

Traces: iotjobsdata-describe-job-execution-malformed-request-refusals#task#6 (representative) and #task#8 — both members of the same cluster (service=iot-jobs-data|DescribeJobExecution|status=400->404).

## Evidence
Fixture iotjobsdata-describe-job-execution-malformed-request-refusals, step task#6 (cluster also covers task#8).
Expected (recorded AWS): status 400; $.message = "executionNumber is out of range"; $headers.x-amzn-errortype = the modeled 400 request-validation error type (asserted as shaped).
Actual (emulator): status 404; $headers.x-amzn-errortype = "ResourceNotFoundException"; $.message = "Job sim2real-probe-ijd-dvjob-04c48d99 not found for sim2real-probe-ijd-dvdev-ba0958b6".
Replay detail: "status: expected 400, got 404; $.message: value_mismatch; $headers.x-amzn-errortype: shape_mismatch".

## Steps to Reproduce
1. Boot the emulator from this checkout and replay fixture iotjobsdata-describe-job-execution-malformed-request-refusals.
2. At step task#6, GET /things/{thingName}/jobs/{jobId}?executionNumber=<out-of-range value> for a thing/job pair in the fixture's seeded state.
3. Observe 404 ResourceNotFoundException with "Job ... not found for ..." where AWS returns 400 with message "executionNumber is out of range". Step task#8 of the same fixture reproduces it identically.
