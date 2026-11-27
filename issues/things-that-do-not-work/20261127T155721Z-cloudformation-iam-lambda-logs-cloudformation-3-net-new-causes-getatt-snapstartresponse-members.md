Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__iam__lambda__logs.cfn-lambda-function-getatt-snapstartresponse-matches-the-function cloudformation__iam__lambda__logs.custom-resource-failed-delete-response-records-delete-failed cloudformation__iam__lambda__logs.sam-tracing-active-attaches-the-xray-write-policy
Issue-Key: cloudformation__iam__lambda__logs.cfn-lambda-function-getatt-snapstartresponse-matches-the-function|run#14|AssertionError|assert (out.get("ApplyOn"), out.get("Status")) == ("None", "Off"), (
Issue-Key: cloudformation__iam__lambda__logs.custom-resource-failed-delete-response-records-delete-failed|run#17|AssertionError|assert "DELETE_COMPLETE" not in statuses, (
Issue-Key: cloudformation__iam__lambda__logs.sam-tracing-active-attaches-the-xray-write-policy|run#13|AssertionError|assert attached == want, f"generated role policies {attached!r}, expected {want!r}"
Issue-Commit: 7300602cd0fc37394a6328fdd869591f4a36c031

# cloudformation: 3 net-new causes — GetAtt SnapStartResponse members are null and mis-named, a custom resource's FAILED delete is recorded DELETE_COMPLETE, and SAM Tracing: Active omits the X-Ray write policy

> _Note: this issue was identified against the code base at commit `7300602cd0fc37394a6328fdd869591f4a36c031`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 3 — `Fn::GetAtt` on `AWS::Lambda::Function` `SnapStartResponse` returns nulls and a wrong member name.
AWS: `SnapStartResponse` is a composite attribute with members `ApplyOn` and `OptimizationStatus`. For a function created without SnapStart they read `ApplyOn = 'None'` and `OptimizationStatus = 'Off'`, matching GetFunctionConfiguration. Smithy (lambda): `SnapStartResponse` members are `ApplyOn` (enum SnapStartApplyOn: PublishedVersions|None) and `OptimizationStatus` (enum SnapStartOptimizationStatus: On|Off).
Emulator: GetAtt yields `{'ApplyOn': None, 'Status': None}` — both members null, and the second member is named `Status` instead of `OptimizationStatus`.
Traces: cloudformation__iam__lambda__logs.cfn-lambda-function-getatt-snapstartresponse-matches-the-function.

Cause 2 of 3 — a custom resource that answers FAILED to a Delete is recorded as DELETE_COMPLETE.
AWS: when a `Custom::` resource's provider posts `Status: FAILED` to the pre-signed response URL during a Delete, CloudFormation records `DELETE_FAILED` for that logical resource (with the provider's Reason) and the stack delete fails. Recorded behavior only; the custom-resource response contract is documented, not modelled.
Emulator: the resource's events are `[('DELETE_IN_PROGRESS', None), ('DELETE_COMPLETE', None)]` — the FAILED response is ignored and the delete is reported successful.
Traces: cloudformation__iam__lambda__logs.custom-resource-failed-delete-response-records-delete-failed.

Cause 3 of 3 — SAM `Tracing: Active` does not attach the X-Ray write policy to the generated execution role.
AWS: SAM's `AWS::Serverless::Function` with `Tracing: Active` adds `arn:aws:iam::aws:policy/AWSXrayWriteOnlyAccess` to the generated role alongside `arn:aws:iam::aws:policy/service-role/AWSLambdaBasicExecutionRole`. Recorded behavior only (a SAM transform rule).
Emulator: the generated role carries only `['arn:aws:iam::aws:policy/service-role/AWSLambdaBasicExecutionRole']`.
Traces: cloudformation__iam__lambda__logs.sam-tracing-active-attaches-the-xray-write-policy.

Why: cause 1 breaks any template that wires SnapStart state into Outputs or into another resource — the consumer gets a null and a member name the SDK does not model. Cause 2 is the more serious: a stack delete that should fail and stop reports success, so a client believes resources were released when the provider said it could not release them, and rollback/teardown logic silently diverges from reality. Cause 3 leaves a traced function unable to write segments, with no error at deploy time.

Fix: cause 1 — the AWS::Lambda::Function resource provider's GetAtt attribute projection; cause 2 — the custom-resource response handling in the stack delete path; cause 3 — the SAM transform's policy synthesis for the generated execution role.

## Evidence
Cause 1: "GetAtt SnapStartResponse outputs {'ApplyOn': None, 'Status': None}, expected ApplyOn 'None' and OptimizationStatus 'Off'".
Cause 2: "Widget events of the refused delete include DELETE_COMPLETE: [('DELETE_IN_PROGRESS', None), ('DELETE_COMPLETE', None)]" — expected DELETE_FAILED.
Cause 3: "generated role policies ['arn:aws:iam::aws:policy/service-role/AWSLambdaBasicExecutionRole'], expected ['arn:aws:iam::aws:policy/AWSXrayWriteOnlyAccess', 'arn:aws:iam::aws:policy/service-role/AWSLambdaBasicExecutionRole']".

## Steps to Reproduce
1. Deploy a template with an AWS::Lambda::Function and an Output using Fn::GetAtt [Fn, SnapStartResponse]; read the Output — members are null and the second is named Status.
2. Deploy a stack with a Custom:: resource whose provider posts Status: FAILED on Delete; delete the stack and read DescribeStackEvents — DELETE_COMPLETE instead of DELETE_FAILED.
3. Deploy a SAM template with AWS::Serverless::Function and Tracing: Active; list the generated role's attached policies — AWSXrayWriteOnlyAccess is absent.
