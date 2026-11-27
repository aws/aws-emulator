Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appconfig.op-contract-stopDeployment
Issue-Shape-Id: com.amazonaws.appconfig#AmazonAppConfig
Issue-Key: appconfig.op-contract-stopDeployment|run#24|AssertionError|assert re.sub(
Issue-Commit: 15ef1adb3fb7a9db5ce7069e30860cdce3c8580a

# AppConfig StopDeployment's unknown-DeploymentNumber ResourceNotFoundException message omits AWS's service/status/error-code/request-id trailer

> _Note: this issue was identified against the code base at commit `15ef1adb3fb7a9db5ce7069e30860cdce3c8580a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StopDeployment

## Description
Cause 1 of 1: StopDeployment on a DeploymentNumber that does not exist is refused with the right error class, but the Message is truncated — it stops after the sentence and omits the parenthesised trailer AWS appends to this refusal. Failing check ids: appconfig.op-contract-stopDeployment.

AWS: Smithy (services/service-com-amazonaws-appconfig-amazonappconfig/smithy-model/com-amazonaws-appconfig-amazonappconfig.json) models StopDeployment as DELETE /applications/{ApplicationId}/environments/{EnvironmentId}/deployments/{DeploymentNumber}, success 202, with errors BadRequestException (400), ResourceNotFoundException (404) and InternalServerException (500); ApplicationId, EnvironmentId and DeploymentNumber are @required @httpLabel members. Recorded behavior (the message text itself is not modelled): naming a DeploymentNumber that was never created is refused with ResourceNotFoundException / HTTP 404 whose Message is, as a template:
  "Deployment with Application Id <applicationId>, Environment Id <environmentId>, and deploymentNumber <deploymentNumber> could not be found for account <accountId>. (Service: AWSAppConfigData; Status Code: 404; Error Code: <backendErrorCode>; Request ID: <requestId>; Proxy: null)"
Fixed text: "Deployment with Application Id ", ", Environment Id ", ", and deploymentNumber ", " could not be found for account ", ". (Service: AWSAppConfigData; Status Code: 404; ", "Error Code: ", "; Request ID: ", "; Proxy: null)". Variable parts taken from the request and the caller's identity: the ApplicationId, EnvironmentId and DeploymentNumber labels from the URI, the caller's account id, and — per call — the backend error code and request id (the check masks exactly those two, so they are the only per-run values; the Service token "AWSAppConfigData", the "Status Code: 404" and the "Proxy: null" tail are fixed text and part of the contract).

Emulator: answers ResourceNotFoundException (correct class and status) but with the trailer-less Message "Deployment with Application Id <applicationId>, Environment Id <environmentId>, and deploymentNumber <deploymentNumber> could not be found for account <accountId>." — the whole " (Service: AWSAppConfigData; Status Code: ...; Error Code: ...; Request ID: ...; Proxy: null)" suffix is missing. The rest of the check passed: StopDeployment on the in-flight deployment reached ROLLING_BACK/ROLLED_BACK, StartedAt stayed set, CompletedAt stayed unset and PercentageComplete stayed below 100.

Why: the message is part of the contract for this error class. A client, test harness or support tool that matches, logs or displays AWS's wording sees text that does not match, and the omitted trailer is the only place the refusal carries the originating service token, the HTTP status and the backend request id a caller quotes when escalating a 404 — so correlation against AppConfig's own diagnostics is lost.

Fix: the StopDeployment handler's deployment-lookup failure path in the appconfig service, and the shared AppConfig error-message rendering it uses — the ResourceNotFoundException it raises must carry the full AWS message including the "(Service: ...; Status Code: ...; Error Code: ...; Request ID: ...; Proxy: null)" trailer, with the status taken from the error's own HTTP status and the request id from the serving request.

Related open issue, which does NOT cover this check: 20261029T195212Z-appconfig-control-plane-refusals-wrong-error-classes-and.md bundles ten appconfig refusal-wording causes and its Cause 9 concerns StopDeployment's TERMINAL-STATE ("cannot be stopped"/"cannot be reverted") refusal; it carries appconfig.stop-deployment-allow-revert-reverts-completed-deployment, not this check's replay key, and says nothing about the not-found message or its trailer.

Traces: appconfig.op-contract-stopDeployment.

## Evidence
Check appconfig.op-contract-stopDeployment, verdict [mismatch], failing at the message assertion (check_appconfig_op_contract_stopDeployment.py line 103).

Expected (AWS), after the check masks only the per-call backend error code and request id:
  Code 'ResourceNotFoundException', Message 'Deployment with Application Id <appId>, Environment Id <envId>, and deploymentNumber 999 could not be found for account <account>. (Service: AWSAppConfigData; Status Code: 404; Error Code: <code>; Request ID: <id>; Proxy: null)'

Actual (emulator):
  AssertionError: ResourceNotFoundException message 'Deployment with Application Id ho65pb1, Environment Id algcbw8, and deploymentNumber 999 could not be found for account 000000000000.'
  — the error Code was 'ResourceNotFoundException' (that assertion passed), the sentence matches, and the entire parenthesised trailer is absent.

The ids 'ho65pb1'/'algcbw8' and the account '000000000000' in that line are per-run values that merely fill the template; 999 is the DeploymentNumber the request named. The requirement is the presence and fixed text of the trailer, not those values.

Earlier legs of the same check passed against the emulator, so this is the only divergence it reports: StopDeployment returned State in (ROLLING_BACK, ROLLED_BACK), GetDeployment settled at ROLLED_BACK with StartedAt set, CompletedAt unset and PercentageComplete < 100.

Smithy consulted: StopDeployment — DELETE /applications/{ApplicationId}/environments/{EnvironmentId}/deployments/{DeploymentNumber}, success code 202, errors BadRequestException 400 / ResourceNotFoundException 404 / InternalServerException 500.

## Steps to Reproduce
1. Point an appconfig client at the emulator's data listener.
2. CreateApplication, CreateEnvironment, CreateConfigurationProfile(LocationUri='hosted', Type='AWS.Freeform') and CreateHostedConfigurationVersion(Content=b'{"flag":true}').
3. CreateDeploymentStrategy(DeploymentDurationInMinutes=2, GrowthFactor=5, FinalBakeTimeInMinutes=0, ReplicateTo='NONE') and StartDeployment with that strategy and version.
4. StopDeployment on the started deployment and poll GetDeployment to ROLLED_BACK (this part already matches AWS).
5. Call StopDeployment(ApplicationId=<appId>, EnvironmentId=<envId>, DeploymentNumber=999) for a deployment number that was never created.
   EXPECTED: ResourceNotFoundException / HTTP 404 with Message 'Deployment with Application Id <appId>, Environment Id <envId>, and deploymentNumber 999 could not be found for account <account>. (Service: AWSAppConfigData; Status Code: 404; Error Code: <code>; Request ID: <id>; Proxy: null)'.
   OBSERVED: the same sentence with no trailer at all.
6. Re-replay appconfig.op-contract-stopDeployment to confirm the fix.
