Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codedeploy.skip-wait-time-already-completed-rejected codedeploy.skip-wait-time-deployment-already-completed-rejected codedeploy.skip-wait-time-triggers-immediate-termination
Issue-Shape-Id: com.amazonaws.codedeploy#CodeDeploy_20141006
Issue-Key: codedeploy.skip-wait-time-already-completed-rejected|run#13|AssertionError|assert refusal == expected, f"expected {expected!r}, got {refusal!r}"
Issue-Key: codedeploy.skip-wait-time-deployment-already-completed-rejected|run#10|AssertionError|assert refusal == expected, f"expected {expected!r}, got {refusal!r}"
Issue-Key: codedeploy.skip-wait-time-triggers-immediate-termination|run#16|AssertionError|assert refusal == expected, (
Issue-Commit: 23d7c3ca3e68cb70ca93342274fc19d1394baa26

# REOPENS: SkipWaitTimeForInstanceTermination's DeploymentAlreadyCompletedException message is still emulator-authored ("is already in state X" instead of AWS's "has already completed, current state X.")

> _Note: this issue was identified against the code base at commit `23d7c3ca3e68cb70ca93342274fc19d1394baa26`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SkipWaitTimeForInstanceTermination

## Description
REOPENS 20261102T084651Z-codedeploy-skipwaittimeforinstancetermination-refusal-messages-are-emulator-authored-deploymentalreadycompletedexception-and.md: its fix left the call refused. That issue's Cause 1 covered exactly these three replay keys; the fix adopted the uppercase INTERNAL state token (Stopped -> ABORTED, Failed -> FAILED, Succeeded -> SUCCEEDED, which now matches AWS) but did NOT adopt AWS's sentence, so all three checks still fail on the message text. This is one cause.

Cause 1 of 1: the DeploymentAlreadyCompletedException message for a terminal deployment does not use AWS's sentence template.

AWS: Smithy (services/service-com-amazonaws-codedeploy-codedeploy-20141006/smithy-model/com-amazonaws-codedeploy-codedeploy-20141006.json) binds SkipWaitTimeForInstanceTermination -> errors DeploymentAlreadyCompletedException, DeploymentDoesNotExistException, DeploymentIdRequiredException, DeploymentNotStartedException, InvalidDeploymentIdException, UnsupportedActionForDeploymentTypeException, every one errorKind=client at HTTP 400. The model carries no message text (only the generic exception-message doc), so the wording is recorded behavior only. Recorded behavior: calling the operation on a deployment that has already reached a terminal state is refused HTTP 400, code DeploymentAlreadyCompletedException, message template "The deployment <deploymentId> has already completed, current state <STATE>." The FIXED text is "The deployment ", " has already completed, current state " and the trailing period "."; the VARIABLE parts are <deploymentId>, taken verbatim from the request, and <STATE>, the deployment's internal state name in upper case taken from emulator/service state (Succeeded -> SUCCEEDED, Failed -> FAILED, Stopped -> ABORTED). The d-XXXXXXXXX ids in the evidence below are per-run values that fill the template; they are not part of it.

Emulator: same status and same code, but the message is "The deployment <deploymentId> is already in state <STATE>" — AWS's fixed text "has already completed, current state " is replaced by locally authored "is already in state ", and the trailing period is missing. The <STATE> token itself is now correct (ABORTED/FAILED/SUCCEEDED all appeared as AWS words them), so only the surrounding sentence diverges. The text is produced by the shared helper crate::state::deployment_already_completed (see Traces for the file/line), which SkipWaitTimeForInstanceTermination calls from its terminal-state guard.

Why: a client, deployment-automation script or retry matcher written against AWS's refusal text does not recognise the refusal, because none of AWS's fixed phrase is present; code that parses the state out of the sentence by splitting on "current state " finds nothing and reads no state at all, even though the emulator does know it.

Fix: the already-completed branch of the SkipWaitTimeForInstanceTermination handler and the shared refusal-text helper it calls (the message construction, not the guard's condition, which is correct). Note the shared helper is also reached by StopDeployment, whose own re-authored DeploymentAlreadyCompletedException message is the subject of the separate open issue 20261111T154845Z-codedeploy-stopdeployment-s-deploymentalreadycompletedexception-message-is-re-authored.md; that issue does not carry these three SkipWaitTimeForInstanceTermination replay keys, so this one is filed independently, but a single-site fix likely settles both.

Traces: codedeploy.skip-wait-time-already-completed-rejected, codedeploy.skip-wait-time-deployment-already-completed-rejected, codedeploy.skip-wait-time-triggers-immediate-termination. Message site: services/service-com-amazonaws-codedeploy-codedeploy-20141006/src/state.rs:330 (fn deployment_already_completed, format string at line 332); call site: services/service-com-amazonaws-codedeploy-codedeploy-20141006/src/ops/skip_wait_time_for_instance_termination.rs:210-221.

## Evidence
Behavioral suite simparity-behavioral-aws-v1 replayed against this checkout: 119/146 passed, bootstrap_signal=false (codedeploy is serving — 116/139 of its checks pass). Expected = AWS behavior asserted by the check; Actual = the emulator's response. None of the three checks is marked ALREADY FILED or OWNED by an open issue.

1. codedeploy.skip-wait-time-already-completed-rejected — StopDeployment drives a blue/green deployment to Stopped, then SkipWaitTimeForInstanceTermination(deploymentId=<that id>):
   expected ('DeploymentAlreadyCompletedException', 'The deployment d-GWDBIFG95 has already completed, current state ABORTED.')
   got      ('DeploymentAlreadyCompletedException', 'The deployment d-GWDBIFG95 is already in state ABORTED')

2. codedeploy.skip-wait-time-deployment-already-completed-rejected — the deployment reaches Failed, then the same call:
   expected ('DeploymentAlreadyCompletedException', 'The deployment d-9IM8XLFBZ has already completed, current state FAILED.')
   got      ('DeploymentAlreadyCompletedException', 'The deployment d-9IM8XLFBZ is already in state FAILED')

3. codedeploy.skip-wait-time-triggers-immediate-termination — the deployment reaches a terminal state (Succeeded in this run), then the same call:
   expected ('DeploymentAlreadyCompletedException', 'The deployment d-WAO8L2B34 has already completed, current state SUCCEEDED.')
   got      ('DeploymentAlreadyCompletedException', 'The deployment d-WAO8L2B34 is already in state SUCCEEDED')

In all three the HTTP status (400) and the error code are correct, the state token (ABORTED/FAILED/SUCCEEDED) is correct, and the deployment's status is correctly left unchanged by the rejected call; only the sentence around the state token diverges. The d-... ids are minted per run and are the template's variable part.

Emulator source: services/service-com-amazonaws-codedeploy-codedeploy-20141006/src/state.rs:330-335
  pub fn deployment_already_completed(deployment_id: &str, status: &str) -> String {
      format!("The deployment {deployment_id} is already in state {}", deployment_internal_state(status))
  }
AWS's template requires "The deployment {deployment_id} has already completed, current state {INTERNAL_STATE}."

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the behavioral suite; all three checks reproduce.
2. Stopped -> ABORTED: CreateApplication(computePlatform='Server'); CreateDeploymentGroup with deploymentStyle {deploymentType: BLUE_GREEN, deploymentOption: WITH_TRAFFIC_CONTROL} and tag filters matching no instances; CreateDeployment with an S3 revision; StopDeployment(deploymentId) and poll GetDeployment until status == 'Stopped'; then call SkipWaitTimeForInstanceTermination(deploymentId=<that id>) and read Error.Code and Error.Message.
3. Failed -> FAILED: same setup, but let the deployment reach 'Failed' on its own, then make the same call.
4. Succeeded -> SUCCEEDED: on a Lambda-platform application and deployment group, create a deployment that reaches a terminal state, then make the same call.
5. In each case compare the message to AWS's template 'The deployment <deploymentId> has already completed, current state <SUCCEEDED|FAILED|ABORTED>.' — the emulator answers 'The deployment <deploymentId> is already in state <STATE>' (no 'has already completed, current state' phrase, no trailing period).
6. Confirm the status is unchanged afterwards with GetDeployment (this part already matches AWS).
