Issue-Source: trace-sweep-divergence
Issue-Trace: codedeploy-lambda-all-at-once-deployment-shifts-alias
Issue-Step: task#14
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: codedeploy-lambda-all-at-once-deployment-shifts-alias
Issue-Shape-Id: com.amazonaws.codedeploy#CodeDeploy_20141006
Issue-Key: codedeploy|GetDeployment|2xx->2xx|$.deploymentInfo.deploymentOverview
Issue-Key: codedeploy|GetDeployment|2xx->2xx|$.deploymentInfo.deploymentStatusMessages
Issue-Key: codedeploy|GetDeployment|2xx->2xx|$.deploymentInfo.fileExistsBehavior
Issue-Key: codedeploy|GetDeployment|2xx->2xx|$.deploymentInfo.revision.appSpecContent.sha256
Issue-Commit: 79638dde6e7b7a4551e5f19ade44db4b484cdd66

# CodeDeploy GetDeployment omits deploymentOverview, deploymentStatusMessages, fileExistsBehavior and revision.appSpecContent.sha256

> _Note: this issue was identified against the code base at commit `79638dde6e7b7a4551e5f19ade44db4b484cdd66`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetDeployment

## Description
GetDeployment returns a 200 whose deploymentInfo structure is missing four members that AWS populates for a Lambda-platform deployment, so a client reading the deployment's instance/overview summary, its status messages, its file-exists policy or its revision hash gets null for all four.

Cause 1 of 1: deploymentInfo is serialized without deploymentOverview, deploymentStatusMessages, fileExistsBehavior and revision.appSpecContent.sha256.

AWS: Smithy: GetDeployment -> GetDeploymentOutput.deploymentInfo : DeploymentInfo. DeploymentInfo.deploymentOverview : DeploymentOverview ("A summary of the deployment status of the instances in the deployment"), whose six members Pending/InProgress/Succeeded/Failed/Skipped/Ready each target InstanceCount (long) with @default 0 — so AWS always renders the full six-key object, with the counts taken from the deployment's own target state (for an all-at-once Lambda deployment that completed, the single target is counted under Succeeded and the other five are 0). DeploymentInfo.deploymentStatusMessages : DeploymentStatusMessageList ("Messages that contain information about the status of a deployment") — AWS emits the list, empty when the deployment produced no status messages, not an absent member. DeploymentInfo.fileExistsBehavior : enum FileExistsBehavior with values DISALLOW | OVERWRITE | RETAIN; the model documents DISALLOW as "also the default behavior if no option is specified", so a deployment created without fileExistsBehavior reports DISALLOW. DeploymentInfo.revision : RevisionLocation, and for revisionType AppSpecContent the appSpecContent member is AppSpecContent{content, sha256}, where sha256 : RawStringSha256 is "The SHA256 hash value of the revision content" — AWS computes it from the AppSpec content supplied at CreateDeployment and returns it on every read of the revision.

Emulator: the replayed GetDeployment answers 200 but all four are absent/None: $.deploymentInfo.deploymentOverview expected the six-key object {Failed:0, InProgress:0, Pending:0, Ready:0, Skipped:0, Succeeded:1} and got None; $.deploymentInfo.deploymentStatusMessages expected [] and got None; $.deploymentInfo.fileExistsBehavior expected 'DISALLOW' and got None; $.deploymentInfo.revision.appSpecContent.sha256 expected a SHA256 hex digest of the revision content (the recording's concrete digest is not asserted) and got None. The counts, the DISALLOW value and the digest above are the shapes of the values, not literal requirements: the counts must reflect the deployment's own targets, and the digest must be the SHA256 of the AppSpec content the request carried.

Why: a client that polls GetDeployment to decide whether a deployment finished reads deploymentOverview to size progress (succeeded vs failed vs pending targets); with the member absent, every count reads as unset and such a client either crashes on a null dereference or concludes zero targets were deployed. deploymentStatusMessages absent instead of [] breaks the common "iterate the messages" path the same way. fileExistsBehavior absent means a caller cannot read back the file-overwrite policy a deployment is running under, and a missing revision.appSpecContent.sha256 defeats revision-identity checks: tooling that compares the deployed revision's hash against a locally computed one to detect drift (or to skip a redundant deployment) sees no hash and treats every revision as changed.

Fix: the GetDeployment output serializer / deployment-record projection for the codedeploy service. deploymentOverview must be derived from the deployment's target state and always rendered with all six @default-0 counts; deploymentStatusMessages must always be rendered (empty list when there are none); fileExistsBehavior must be persisted at CreateDeployment and defaulted to DISALLOW when the request omits it; and the AppSpecContent revision must carry a sha256 computed from its content at creation and echoed on read. The same projection should be used wherever DeploymentInfo is returned (e.g. BatchGetDeployments) so reads agree.

Traces: codedeploy-lambda-all-at-once-deployment-shifts-alias#task#14 (GetDeployment, expected 200 / actual 200, four missing deploymentInfo members).

## Evidence
Fixture codedeploy-lambda-all-at-once-deployment-shifts-alias, step task#14, operation GetDeployment, verdict mismatch, expected_status 200 / actual_status 200.

Expected vs actual (from the replay detail):
  $.deploymentInfo.deploymentOverview: expected {'Failed': 0, 'InProgress': 0, 'Pending': 0, 'Ready': 0, 'Skipped': 0, 'Succeeded': 1}, got None
  $.deploymentInfo.deploymentStatusMessages: expected [], got None
  $.deploymentInfo.fileExistsBehavior: expected 'DISALLOW', got None
  $.deploymentInfo.revision.appSpecContent.sha256: expected a SHA256 digest string (value shape-asserted, not literal), got None

Model evidence: Smithy com.amazonaws.codedeploy#DeploymentInfo members deploymentOverview (-> DeploymentOverview, six InstanceCount members all @default 0), deploymentStatusMessages (-> DeploymentStatusMessageList), fileExistsBehavior (-> enum FileExistsBehavior [DISALLOW, OVERWRITE, RETAIN], DISALLOW documented as the default when unspecified), revision (-> RevisionLocation.appSpecContent -> AppSpecContent.sha256 : RawStringSha256, "The SHA256 hash value of the revision content").

No open codedeploy issue covers this response-shape gap: the existing open issues concern deployment lifecycle/alias shifting, trigger notifications, error messages and rejection paths (e.g. "CodeDeploy Lambda deployments reach Succeeded without shifting the alias...", "CodeDeploy rejection paths return the right error codes with the wrong error messages"), none of them the missing DeploymentInfo members on a 200 GetDeployment.

## Steps to Reproduce
1. Create a CodeDeploy application and a Lambda-platform deployment group using the CodeDeployDefault.LambdaAllAtOnce deployment configuration.
2. Call CreateDeployment for that group with revision.revisionType = AppSpecContent and an AppSpecContent whose content is the Lambda AppSpec (function name, CurrentVersion alias, TargetVersion), and WITHOUT specifying fileExistsBehavior.
3. Wait for the deployment to reach a terminal state, then call GetDeployment with that deploymentId.
4. Inspect the response body: deploymentInfo.deploymentOverview, deploymentInfo.deploymentStatusMessages, deploymentInfo.fileExistsBehavior and deploymentInfo.revision.appSpecContent.sha256 are all absent (null), where AWS returns the full six-count overview object, an (empty) message list, 'DISALLOW', and the SHA256 of the AppSpec content.

Replay: replay_wire_corpus, fixture codedeploy-lambda-all-at-once-deployment-shifts-alias, step task#14.
