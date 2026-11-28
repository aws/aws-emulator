Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kinesis__kinesisanalyticsv2.add-application-output-kinesis-stream-on-flink-application-rejected
Issue-Key: kinesis__kinesisanalyticsv2.add-application-output-kinesis-stream-on-flink-application-rejected|run#20|AssertionError|assert code == "InvalidArgumentException", (
Issue-Commit: 40df241c1ce9889c9be50646fd3e82dae29dae7f

# AddApplicationOutput answers 501 NotImplemented instead of refusing an SQL output on a Flink application with InvalidArgumentException

> _Note: this issue was identified against the code base at commit `40df241c1ce9889c9be50646fd3e82dae29dae7f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AddApplicationOutput

## Description
One root cause. Cause 1 of 1: `AddApplicationOutput` is an unimplemented scaffold stub in the kinesisanalyticsv2 emulator, so every call to it is answered with a scaffold 501 instead of the operation's modelled behaviour. The behavioural check that reaches it exercises the negative case: adding an SQL output (an in-application stream named DESTINATION_SQL_STREAM with a `KinesisStreamsOutput` destination) to an application whose `RuntimeEnvironment` is a Flink runtime (FLINK-1_20). Failing check id: kinesis__kinesisanalyticsv2.add-application-output-kinesis-stream-on-flink-application-rejected.

AWS: Smithy model com.amazonaws.kinesisanalyticsv2#AddApplicationOutput declares input members ApplicationName (required, @length 1..128, @pattern ^[a-zA-Z0-9_.-]+$), CurrentApplicationVersionId (required, @range 1..999999999) and Output (required), output members ApplicationARN / ApplicationVersionId / OutputDescriptions, and the errors ConcurrentModificationException (HTTP 409), InvalidArgumentException (HTTP 400, "The specified input parameter value is not valid."), InvalidRequestException (HTTP 400), ResourceInUseException (HTTP 400) and ResourceNotFoundException (HTTP 400). The operation's own documentation scopes it to "your SQL-based Kinesis Data Analytics application", so an application on a Flink runtime has no SQL configuration to extend. Recorded behavior for that case: HTTP 400 with error code `InvalidArgumentException` and a message whose fixed text is the sentence "You cannot add sql configuration <...> to a Flink application." — the leading clause "You cannot add sql configuration" and the trailing "to a Flink application." are fixed text; any part between them is filled from the request (the configuration/output being added) and from the application's state (its runtime), never from one run's literal values. The recording further shows the refusal is side-effect free: DescribeApplication afterwards still reports ApplicationVersionId 1 and carries no SqlApplicationConfigurationDescription.

Emulator: the request is answered HTTP 501 with error code `NotImplemented`. Replay assertion: "AddApplicationOutput on a FLINK-1_20 application answered code 'NotImplemented'". The handler body returns `Scaffolded::not_implemented("AddApplicationOutput")` unconditionally, so no input is ever inspected, no modelled error is ever produced, and the success path is unreachable too — a valid AddApplicationOutput against an SQL-capable application would also get 501.

Why: a client cannot distinguish "this runtime does not accept SQL outputs" from "the service is broken". `NotImplemented` is not in the operation's modelled error set, so botocore surfaces it as an unmodelled ClientError with a 501 status: retry and error-classification logic keyed on InvalidArgumentException (a permanent, caller-fixable 400) never fires, and SDK code that branches on `client.exceptions.InvalidArgumentException` misses entirely. Any caller that legitimately adds an output to an SQL application is blocked outright, and because the operation never touches state, callers also cannot rely on the version-increment contract ("any configuration update ... results in a new version of the application").

Fix: implement the operation in its handler — `services/service-com-amazonaws-kinesisanalyticsv2-kinesisanalytics-20180523/src/ops/add_application_output.rs` (the `handle` body, currently the scaffold `Err(...)` at the end of the file) — and delete the SCAFFOLDED marker. It needs: application lookup (ResourceNotFoundException when absent), a version check against CurrentApplicationVersionId (ConcurrentModificationException on mismatch), the runtime-environment gate that refuses an SQL output on a Flink runtime with InvalidArgumentException and the message above, and, for an SQL application, appending the Output to the application's SqlApplicationConfiguration state, bumping ApplicationVersionId and returning ApplicationARN plus the resulting OutputDescriptions. The refusal path must leave state untouched.

Note on scope: the same check goes on to assert DeleteApplicationOutput's "There are no outputs ... that you would like to delete" refusal and an UpdateApplication runtime-property round trip, but the replay aborts at the 501 above, so those steps were never exercised and are not claimed here as separate defects. Neither existing open kinesisanalyticsv2 issue (20261022T071206Z-kinesis-analytics-v2-is-entirely-unreachable-router-refuses.md, 20261109T143515Z-cloudformation-glue-iam-kinesisanalytics-...md) covers this operation.

## Evidence
Expected (AWS, from the recording and the Smithy model):
  HTTP 400, Error.Code = "InvalidArgumentException"
  Error.Message = "You cannot add sql configuration <configuration from the request> to a Flink application."
    fixed text: "You cannot add sql configuration" ... "to a Flink application."
    variable: the SQL configuration named in the request / the application's runtime state
  then DescribeApplication(ApplicationName) -> ApplicationDetail.ApplicationVersionId == 1 and no
  ApplicationConfigurationDescription.SqlApplicationConfigurationDescription

Actual (emulator):
  HTTP 501, Error.Code = "NotImplemented"
  Replay: AssertionError: AddApplicationOutput on a FLINK-1_20 application answered code 'NotImplemented'
  (check_kinesis__kinesisanalyticsv2_add_application_output_kinesis_stream_on_flink_application_rejected.py line 121)

Handler: services/service-com-amazonaws-kinesisanalyticsv2-kinesisanalytics-20180523/src/ops/add_application_output.rs
  line 4: "//! SCAFFOLDED, NOT IMPLEMENTED: this operation answers HTTP 501 `NotImplemented`"
  line 188: Err(crate::scaffolded::Scaffolded::not_implemented("AddApplicationOutput"))

Smithy (com-amazonaws-kinesisanalyticsv2-kinesisanalytics-20180523): AddApplicationOutput errors =
  ConcurrentModificationException(409), InvalidArgumentException(400), InvalidRequestException(400),
  ResourceInUseException(400), ResourceNotFoundException(400). NotImplemented is not among them.

## Steps to Reproduce
1. kinesis: CreateStream(StreamName=<unique>, ShardCount=1); wait for stream_exists; DescribeStreamSummary to get StreamARN.
2. iam: CreateRole with a trust policy allowing sts:AssumeRole for Service kinesisanalytics.amazonaws.com; keep the Role ARN.
3. kinesisanalyticsv2: CreateApplication(ApplicationName=<unique>, RuntimeEnvironment="FLINK-1_20", ServiceExecutionRole=<role arn>) and confirm ApplicationDetail.ApplicationVersionId == 1.
4. kinesisanalyticsv2: AddApplicationOutput(ApplicationName=<app>, CurrentApplicationVersionId=1, Output={"Name": "DESTINATION_SQL_STREAM", "KinesisStreamsOutput": {"ResourceARN": <stream arn>}, "DestinationSchema": {"RecordFormatType": "JSON"}}).
   Expected: ClientError with HTTP 400 / Code "InvalidArgumentException" and a message beginning "You cannot add sql configuration" and ending "to a Flink application."
   Observed: ClientError with HTTP 501 / Code "NotImplemented".
5. kinesisanalyticsv2: DescribeApplication(ApplicationName=<app>) and check ApplicationVersionId is still 1 with no SqlApplicationConfigurationDescription.

Or replay the single check: kinesis__kinesisanalyticsv2.add-application-output-kinesis-stream-on-flink-application-rejected
