Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__sagemaker.cfn-notebook-lifecycle-config-invalid-name-rejected
Issue-Key: cloudformation__sagemaker.cfn-notebook-lifecycle-config-invalid-name-rejected|run#12|AssertionError|assert got == _REJECTED, f"handler reason {got!r}, expected {_REJECTED!r}"
Issue-Commit: d0f76c0ba03e1debbd23029512a6c9d3948c5ea6

# cloudformation__sagemaker: a resource handler's failure reason drops the AWS SDK service/status/request-id suffix, so a SageMaker validation refusal is not reported as CloudFormation reports it

> _Note: this issue was identified against the code base at commit `d0f76c0ba03e1debbd23029512a6c9d3948c5ea6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateNotebookInstanceLifecycleConfig

## Description
ONE net-new behavioral root cause. The failing check is not marked ALREADY FILED or OWNED by the replay.

Cause 1 of 1: when a SageMaker API refuses a CloudFormation-declared resource, the resource's CREATE_FAILED ResourceStatusReason carries only the service's message sentence and omits the AWS SDK diagnostic suffix CloudFormation's registry handler appends.
AWS: Smithy (services/service-com-amazonaws-sagemaker-sagemaker/smithy-model/com-amazonaws-sagemaker-sagemaker.json): CreateNotebookInstanceLifecycleConfig input NotebookInstanceLifecycleConfigName (required, @length 0..63, pattern ^[a-zA-Z0-9](-*[a-zA-Z0-9])*$); a name violating that pattern is refused with ValidationException/400 and the standard single-error sentence "1 validation error detected: Value '<name>' at 'notebookInstanceLifecycleConfigName' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-zA-Z0-9](-*[a-zA-Z0-9])*". CloudFormation surfaces that refusal through the resource handler, which wraps the message as 'Resource handler returned message: "<service message> (Service: SageMaker, Status Code: 400, Request ID: <id>) (SDK Attempt Count: <n>)" (RequestToken: <token>, HandlerErrorCode: InvalidRequest)'. The suffix is fixed text apart from the service id, the HTTP status of the underlying refusal, and the per-call request id and attempt count — it is the AWS SDK's own exception rendering, not prose the service invents. The stack ends ROLLBACK_COMPLETE and the handler error code is InvalidRequest.
Emulator: the HandlerErrorCode (InvalidRequest) and the service sentence are right, but the handler reason ends after the regular-expression pattern: the "(Service: SageMaker, Status Code: 400, Request ID: <id>) (SDK Attempt Count: <n>)" suffix is absent, so the reason carries neither the originating service, nor the HTTP status of the refusal, nor a request id.
Why: the resource-handler reason is the only place a CloudFormation client sees WHICH service refused a resource and with what HTTP status; operators and automation parse that suffix to route a stack failure to the right service and to quote a request id in a support case. With it missing, a stack failure caused by a downstream 400 is indistinguishable from one the handler itself produced, and there is no request id to correlate.
Fix: the CloudFormation resource-handler error path that renders a downstream AWS API exception into ResourceStatusReason — it must append the SDK-style suffix (service id, status code, request id, attempt count) around the service's own message, for SageMaker resource types and any other registry handler that wraps a service refusal.
Traces: cloudformation__sagemaker.cfn-notebook-lifecycle-config-invalid-name-rejected.

## Evidence
cloudformation__sagemaker.cfn-notebook-lifecycle-config-invalid-name-rejected [mismatch]:
  assert got == _REJECTED, f"handler reason {got!r}, expected {_REJECTED!r}"
  AssertionError: handler reason ("1 validation error detected: Value '<name>' at 'notebookInstanceLifecycleConfigName' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-zA-Z0-9](-*[a-zA-Z0-9])*", 'InvalidRequest'),
  expected ("1 validation error detected: Value '<name>' at 'notebookInstanceLifecycleConfigName' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-zA-Z0-9](-*[a-zA-Z0-9])* (Service: SageMaker, Status Code: 400, Request ID: <id>) (SDK Attempt Count: <n>)", 'InvalidRequest')
The check parses the CREATE_FAILED ResourceStatusReason with ^Resource handler returned message: "(?P<message>.*)" \(RequestToken: [0-9a-f-]+, HandlerErrorCode: (?P<code>[A-Za-z]+)\)$ and masks exactly the per-run parts before comparing: Request ID -> <id>, RequestToken -> <token>, SDK Attempt Count -> <n>, the account and the run-generated names -> <account>/<name>. So the resource name, request id, token and attempt count are NOT requirements; the fixed text 'Member must satisfy regular expression pattern: ...', '(Service: SageMaker, Status Code: 400, ...)' and '(SDK Attempt Count: ...)', and the HandlerErrorCode InvalidRequest, are.

Smithy consulted (services/service-com-amazonaws-sagemaker-sagemaker/smithy-model/com-amazonaws-sagemaker-sagemaker.json): CreateNotebookInstanceLifecycleConfig — NotebookInstanceLifecycleConfigName required, @length 0..63, @pattern ^[a-zA-Z0-9](-*[a-zA-Z0-9])*$; OnCreate/OnStart @length 0..1; only ResourceLimitExceeded is modeled, so the ValidationException comes from the service-common error set (recorded behavior for the message).

## Steps to Reproduce
Replay the behavioral suite (run_behavioral_suite) and re-run cloudformation__sagemaker.cfn-notebook-lifecycle-config-invalid-name-rejected. Manually: CreateStack with a template containing one AWS::SageMaker::NotebookInstanceLifecycleConfig whose NotebookInstanceLifecycleConfigName violates ^[a-zA-Z0-9](-*[a-zA-Z0-9])*$ (e.g. a trailing '-' or an underscore) and a base64 OnCreate script; wait for the stack to reach ROLLBACK_COMPLETE, then read the resource's CREATE_FAILED row from DescribeStackEvents. The ResourceStatusReason must read 'Resource handler returned message: "1 validation error detected: Value '<name>' at 'notebookInstanceLifecycleConfigName' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-zA-Z0-9](-*[a-zA-Z0-9])* (Service: SageMaker, Status Code: 400, Request ID: <id>) (SDK Attempt Count: <n>)" (RequestToken: <token>, HandlerErrorCode: InvalidRequest)'; the emulator omits the '(Service: ...)' and '(SDK Attempt Count: ...)' clauses.
