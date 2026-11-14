Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudcontrol.client-token-idempotent-retry-same-params-succeeds cloudcontrol.client-token-replay-on-a-live-resource-performs-no-second-native-write cloudcontrol.client-token-reuse-across-operation-types-rejected cloudcontrol.cloudcontrol-client-token-replay-after-delete-returns-cached-event-not-resurrected cloudcontrol.cloudcontrol-create-ssm-parameter-visible-to-native-ssm-getparameter cloudcontrol.cloudcontrol-patch-document-bumps-native-ssm-parameter-version-exactly-once cloudcontrol.cloudcontrol-update-absent-identifier-denied-and-writes-nothing-native cloudcontrol.fabricated-type-version-id-ignored-for-a-public-type cloudcontrol.request-status-polls-to-success cloudcontrol.role-arn-service-role-policy-governs-the-downstream-delete
Issue-Shape-Id: com.amazonaws.cloudcontrol#CloudApiService
Issue-Key: ssm|GetParameter|ParameterNotFound
Issue-Commit: 7446e1137d863a6c5f0117bdbe1b0c65f0c299cf

# cloudcontrol: a Cloud Control request on AWS::SSM::Parameter reaches SUCCESS without ever writing SSM — GetParameter answers ParameterNotFound for the parameter the create/patch reported

> _Note: this issue was identified against the code base at commit `7446e1137d863a6c5f0117bdbe1b0c65f0c299cf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateResource

## Description
ONE net-new root cause in the cloudcontrol (Cloud Control API) service, grounded in 10 failing behavioral checks that all die at the same place: the independent read at the owning service.

Cause 1 of 1: CreateResource/UpdateResource of TypeName "AWS::SSM::Parameter" drive the request to a terminal OperationStatus SUCCESS, but the effect is recorded only in Cloud Control's own store — Systems Manager never receives the parameter, so every subsequent native read of it is refused.

AWS: Smithy (com.amazonaws.cloudcontrol#CloudApiService) models CreateResource with required TypeName (pattern ^[A-Za-z0-9]{2,64}::[A-Za-z0-9]{2,64}::[A-Za-z0-9]{2,64}$) and required DesiredState, and UpdateResource with required Identifier + PatchDocument; both answer a ProgressEvent. Cloud Control is a facade that invokes the resource type's own handler, and its OperationStatus SUCCESS is documented as "The resource operation has successfully completed" — i.e. the resource exists in the owning service. Concretely, for AWS::SSM::Parameter: after a SUCCESSful create, ssm:GetParameter(Name=<the Name from DesiredState>) returns the Parameter with the Value/Type the desired state asked for and Version 1, and GetParameterHistory holds exactly one entry; a ClientToken replay of the same create does not write a second version (Version stays 1); each successful UpdateResource patch of /Value bumps Version by exactly one; after a SUCCESSful DeleteResource, GetParameter is refused with ParameterNotFound. When the handler cannot make the resource real, Cloud Control must reach a terminal FAILED ProgressEvent (ErrorCode from the handler, e.g. HandlerFailureException/NotStabilizedException/GeneralServiceException — all modeled on these operations), never SUCCESS. The modeled errors for this operation are the only refusal vocabulary; there is no modeled outcome in which SUCCESS coexists with an absent resource.

Emulator: the Cloud Control request reports SUCCESS, and the very next native read raises botocore.errorfactory.ParameterNotFound: "An error occurred (ParameterNotFound) when calling the GetParameter operation:" (the message body is empty; the fixed text of this error class is AWS's ParameterNotFound code at HTTP 400 for the requested parameter Name, and the Name is the value from the request). This happens for the straight create (cloudcontrol-create-ssm-parameter-visible-to-native-ssm-getparameter, request-status-polls-to-success), for creates carrying a ClientToken (client-token-idempotent-retry-same-params-succeeds, client-token-replay-on-a-live-resource-performs-no-second-native-write, client-token-reuse-across-operation-types-rejected, cloudcontrol-client-token-replay-after-delete-returns-cached-event-not-resurrected), for a create carrying an unregistered TypeVersionId and for its no-TypeVersionId positive control (fabricated-type-version-id-ignored-for-a-public-type), for a create made with a RoleArn (role-arn-service-role-policy-governs-the-downstream-delete), and for the patch paths whose positive control creates/patches a parameter through Cloud Control (cloudcontrol-patch-document-bumps-native-ssm-parameter-version-exactly-once, cloudcontrol-update-absent-identifier-denied-and-writes-nothing-native). In every case Cloud Control's own ProgressEvent is SUCCESS, so nothing in the Cloud Control exchange reveals the no-op.

Why: Cloud Control exists so a client can provision a resource without knowing the owning API and then USE that resource through the owning API. A SUCCESS for a parameter SSM does not have makes every Cloud Control provisioning flow a silent no-op: configuration written through Cloud Control is unreadable by the application that consumes it, idempotency/versioning semantics (Version, GetParameterHistory) cannot be observed at all, deletes cannot be distinguished from never-created, and the FAILED/handler-error path a client would retry or alarm on never fires.

Fix: the request-effect materialisation in the cloudcontrol crate (services/service-com-amazonaws-cloudcontrol-cloudapiservice — the CreateResource/UpdateResource/DeleteResource handlers and the effect application in the service state) must apply the AWS::SSM::Parameter effect to the SSM service's own state (the same store ssm:PutParameter/GetParameter/GetParameterHistory read and write, including the native version counter) before the request may become SUCCESS, and must terminate the request FAILED with a modeled handler ErrorCode when it cannot. The idempotency (ClientToken) and RoleArn paths must route through that same single native write so a replay writes once and a denied downstream action leaves the native value untouched.

Related already-open issue (does NOT cover these checks; named here so the two are not fixed twice over): 20261102T095426Z-cloudcontrol-6-net-new-causes-cloud-control-s.md, whose Cause 1 describes the same split-store family for Logs/CloudWatch/SQS and for SSM GetParameterHistory under a different set of checks. The 10 checks below are not carried by it or by any other open issue.

Traces: the 10 failing check ids listed under Failing checks / Traces (all under the suite's ### cloudcontrol heading).

## Evidence
Suite simparity-behavioral-aws-v1, all 10 checks verdict [error], identical terminating exception:

  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
  botocore.errorfactory.ParameterNotFound: An error occurred (ParameterNotFound) when calling the GetParameter operation:

Expected (AWS): after the Cloud Control request reports OperationStatus=SUCCESS, ssm.get_parameter(Name=<DesiredState Name>) returns {"Parameter": {"Name": <that name>, "Type": "String", "Value": <the DesiredState Value>, "Version": 1}}; ssm.get_parameter_history(Name=...) lists exactly [1]; each successful patch of /Value yields Version 2, 3, ...; ParameterNotFound is correct only after a SUCCESSful DeleteResource.
Actual (emulator): the ProgressEvent is SUCCESS and GetParameter is refused with code ParameterNotFound, so the parameter never existed in SSM. Checks that read the native store to prove the write assert this explicitly, e.g. "SSM holds Value <x> for <name>, expected <y> -- the terminal SUCCESS ... ", "SSM reports Version <n> after a same-token, same-params retry; an idempotent retry must not write the native parameter a second time", and "the positive control parameter <name> is not readable from SSM with the value the desired state asked for".

The parameter names, request tokens and ClientTokens in the recordings are per-run values; the requirement is the status/code pair (SUCCESS implies a readable native parameter; ParameterNotFound only after a successful delete), not any one recorded name or value.

## Steps to Reproduce
Boot the emulator; cc = client("cloudcontrol"), ssm = client("ssm").

1. Basic write-through: cc.create_resource(TypeName="AWS::SSM::Parameter", DesiredState=json.dumps({"Name": n, "Type": "String", "Value": "v1"})); poll cc.get_resource_request_status(RequestToken=...) to a terminal OperationStatus. If it is SUCCESS, ssm.get_parameter(Name=n) MUST return Value "v1" and Version 1 (today it raises ParameterNotFound); if the handler cannot write SSM, the terminal event MUST be FAILED with a modeled ErrorCode.
2. Idempotency: repeat the identical create with the same ClientToken, poll to SUCCESS, then ssm.get_parameter(Name=n)["Parameter"]["Version"] MUST still be 1 and get_parameter_history(Name=n) MUST list exactly one version.
3. Patch: cc.update_resource(TypeName="AWS::SSM::Parameter", Identifier=n, PatchDocument=json.dumps([{"op":"replace","path":"/Value","value":"patched"}])), poll to SUCCESS, then ssm.get_parameter(Name=n) MUST report Value "patched" and Version 2.
4. Delete: cc.delete_resource(TypeName="AWS::SSM::Parameter", Identifier=n), poll to SUCCESS, then ssm.get_parameter(Name=n) MUST be refused ParameterNotFound — and a ClientToken replay of the earlier create MUST NOT resurrect it.
5. Same as (1) with TypeVersionId="00000099" (a well-formed, unregistered version for a public type) and, separately, with RoleArn=<a service role>: both MUST still produce the real native parameter.
