Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: accessanalyzer__cloudformation.deploy-accessanalyzer-analyzer-invalid-type-rejected-and-rolled-back
Issue-Key: accessanalyzer__cloudformation.deploy-accessanalyzer-analyzer-invalid-type-rejected-and-rolled-back|run#21|AssertionError|assert got_reason == _REJECTED, f"handler reason {got_reason!r}, expected {_REJECTED!r}"
Issue-Commit: 37bf0949a3a203e7d1b7ebeb80108d58d67e6afd

# AWS::AccessAnalyzer::Analyzer invalid Type: CREATE_FAILED handler reason is emulator-authored instead of AWS's JSON-schema enum refusal

> _Note: this issue was identified against the code base at commit `37bf0949a3a203e7d1b7ebeb80108d58d67e6afd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAnalyzer

## Description
One net-new cause.

Cause 1 of 1: the ResourceStatusReason CloudFormation records on the CREATE_FAILED row of an AWS::AccessAnalyzer::Analyzer whose Type is not a modelled value is worded by the emulator, not by Access Analyzer.

AWS: Smithy (com.amazonaws.accessanalyzer) models CreateAnalyzer input member `type` against enum `Type` with values [ACCOUNT, ORGANIZATION, ACCOUNT_UNUSED_ACCESS, ORGANIZATION_UNUSED_ACCESS, ACCOUNT_INTERNAL_ACCESS, ORGANIZATION_INTERNAL_ACCESS]; the CloudFormation resource schema for the type enumerates those plus ACCOUNT_INTERNAL_ACCESS_V2 and ORGANIZATION_INTERNAL_ACCESS_V2. The recorded refusal is the JSON-schema form: `[instance value ("<value>") not found in enum (possible values: ["ACCOUNT","ORGANIZATION","ACCOUNT_UNUSED_ACCESS","ORGANIZATION_UNUSED_ACCESS","ACCOUNT_INTERNAL_ACCESS","ORGANIZATION_INTERNAL_ACCESS","ACCOUNT_INTERNAL_ACCESS_V2","ORGANIZATION_INTERNAL_ACCESS_V2"])] (Service: AccessAnalyzer, Status Code: 400, Request ID: <id>) (SDK Attempt Count: <n>)`. Fixed text: the bracketed `[instance value (...) not found in enum (possible values: [...])]` wrapper, the service name `AccessAnalyzer`, status code 400, and the `(SDK Attempt Count: ...)` suffix. Variable parts: the rejected value from the request, the request id, the attempt count. The stack reaches ROLLBACK_COMPLETE and DescribeEvents emits no VALIDATION_ERROR row (the refusal comes from the handler, not early validation) — both of which the emulator already gets right.

Emulator: records `Resource handler returned message: "1 validation error detected: Value '<value>' at 'type' failed to satisfy constraint: Member must satisfy enum value set: [ACCOUNT, ORGANIZATION, ACCOUNT_UNUSED_ACCESS, ORGANIZATION_UNUSED_ACCESS, ACCOUNT_INTERNAL_ACCESS, ORGANIZATION_INTERNAL_ACCESS]" (RequestToken: <token>, HandlerErrorCode: InvalidRequest)` — a different message class (SDK-v1 "N validation error(s) detected" wording), a different value set (the two *_V2 members missing), and the `Resource handler returned message:`/`HandlerErrorCode` envelope instead of the `(Service: ..., Status Code: ..., Request ID: ...)` suffix.

Why: a client or CI gate that matches the CREATE_FAILED reason to classify the failure (or that surfaces the valid value set to a user) sees neither AWS's wording nor AWS's full enum, so it reports the wrong set of allowed analyzer types and cannot recognise the refusal.

Fix: the AWS::AccessAnalyzer::Analyzer resource handler's property validation and the reason it hands back to the CloudFormation event recorder.

Traces: accessanalyzer__cloudformation.deploy-accessanalyzer-analyzer-invalid-type-rejected-and-rolled-back.

## Evidence
Expected handler reason: '[instance value ("NOT_A_REAL_TYPE") not found in enum (possible values: ["ACCOUNT","ORGANIZATION","ACCOUNT_UNUSED_ACCESS","ORGANIZATION_UNUSED_ACCESS","ACCOUNT_INTERNAL_ACCESS","ORGANIZATION_INTERNAL_ACCESS","ACCOUNT_INTERNAL_ACCESS_V2","ORGANIZATION_INTERNAL_ACCESS_V2"])] (Service: AccessAnalyzer, Status Code: 400, Request ID: <id>) (SDK Attempt Count: <n>)'

Actual: 'Resource handler returned message: "1 validation error detected: Value \'NOT_A_REAL_TYPE\' at \'type\' failed to satisfy constraint: Member must satisfy enum value set: [ACCOUNT, ORGANIZATION, ACCOUNT_UNUSED_ACCESS, ORGANIZATION_UNUSED_ACCESS, ACCOUNT_INTERNAL_ACCESS, ORGANIZATION_INTERNAL_ACCESS]" (RequestToken: <token>, HandlerErrorCode: InvalidRequest)'

## Steps to Reproduce
1. CreateStack with a template holding one AWS::AccessAnalyzer::Analyzer whose Properties are {AnalyzerName: <name>, Type: NOT_A_REAL_TYPE}.
2. Wait for the stack to reach ROLLBACK_COMPLETE.
3. DescribeStackEvents and read the ResourceStatusReason of the single CREATE_FAILED row for the Analyzer logical id.
4. Compare with the expected string above (masking only Request ID, RequestToken and SDK Attempt Count).
