Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: greengrassv2.create-deployment-nonexistent-target-not-found greengrassv2.create-deployment-nonexistent-thing-group-target-rejected greengrassv2.deployment-supersede-marks-previous-inactive greengrassv2.list-deployments-history-filter-latest-only-vs-all greengrassv2.create-component-version-access-denied-rejected
Issue-Shape-Id: com.amazonaws.greengrassv2#GreengrassV2
Issue-Key: greengrassv2.create-deployment-nonexistent-target-not-found|_error_code#6|AssertionError|raise AssertionError(f"real AWS accepted this call, but it should have been rejected; got {result!r}")
Issue-Key: greengrassv2.create-deployment-nonexistent-thing-group-target-rejected|_error_code#7|AssertionError|raise AssertionError(f"expected ValidationException, but the call SUCCEEDED: {result!r}")
Issue-Key: greengrassv2.deployment-supersede-marks-previous-inactive|_poll_until#17|Timeout|time.sleep(interval_s)
Issue-Key: greengrassv2.list-deployments-history-filter-latest-only-vs-all|_poll_until#10|Timeout|time.sleep(interval_s)
Issue-Key: greengrassv2.create-component-version-access-denied-rejected|_error_code#12|AssertionError|raise AssertionError(
Issue-Commit: 3ecb0d6b353755c874093f3eab62bbd62a85b9a2

# greengrassv2 CreateDeployment accepts nonexistent targets and never supersedes the previous deployment, and CreateComponentVersion is not authorized at all (3 causes)

> _Note: this issue was identified against the code base at commit `3ecb0d6b353755c874093f3eab62bbd62a85b9a2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDeployment

## Description
Three state/authorization behaviours of the implemented greengrassv2 write paths diverge: requests AWS refuses are accepted, and a deployment lifecycle transition AWS performs never happens. 3 causes to close. (Distinct from the open issue 20261029T172342Z-greengrassv2-createdeployment-refused-with-non-aws-ambiguousservice-code.md, which is about a non-AWS `AmbiguousService` code under concurrent CreateDeployment calls; here CreateDeployment answers 201.)

Cause 1 of 3: CreateDeployment does not check that the target exists.
AWS: Smithy: CreateDeployment POST /greengrass/v2/deployments, `targetArn` required (TargetARN, pattern ^arn:[^:]*:iot:[^:]*:[0-9]+:(thing|thinggroup)/.+$), success 201, modeled errors include ValidationException (HTTP 400) — "The request isn't valid." Recorded behavior: a syntactically valid `targetArn` naming an IoT thing OR an IoT thing group that does not exist is refused 400 `ValidationException` with message `Invalid Input: Target not found` — entirely fixed text, no variable part — and no deployment record is created (ListDeployments for that target is empty). Creating the named thing/thing group and repeating the identical call then succeeds with 201 and an ACTIVE deployment.
Emulator: answers 201 with a `deploymentId`, `iotJobId` and `iotJobArn`, i.e. it creates a deployment (and an IoT job) against a target that was never created.
Why: a typo'd or deleted target silently produces a live deployment; a client never learns the target is missing, and the deployment list is polluted with records AWS would not have.
Fix: target-existence validation in CreateDeployment's handler (look the thing/thinggroup up before recording the deployment) and the ValidationException it must raise.
Traces: greengrassv2.create-deployment-nonexistent-target-not-found, greengrassv2.create-deployment-nonexistent-thing-group-target-rejected.

Cause 2 of 3: a second CreateDeployment for the same target does not supersede the first.
AWS: Smithy: GetDeployment output carries `deploymentStatus` (DeploymentStatus enum) and `isLatestForTarget` (boolean, default false); ListDeployments takes `historyFilter` LATEST_ONLY/ALL. Recorded behavior: creating a second deployment for a target that already has one supersedes the first — the prior deployment moves from `ACTIVE`/`isLatestForTarget=true` to `INACTIVE`/`isLatestForTarget=false`, while the new deployment is the `ACTIVE`, `isLatestForTarget=true` one for that target; ListDeployments(historyFilter=LATEST_ONLY) then returns only the new deployment while historyFilter=ALL returns both.
Emulator: the first deployment stays `ACTIVE` indefinitely — both checks poll GetDeployment for the prior deployment to leave ACTIVE and hit the suite's 60 s limit without the status ever changing, so no INACTIVE/isLatestForTarget=false state is ever reachable.
Why: the supersede semantics that define Greengrass deployment history are absent: a client can never tell which deployment is current for a target, LATEST_ONLY and ALL cannot differ, and code that waits for the previous revision to go INACTIVE hangs.
Fix: CreateDeployment's handler must, when recording a deployment, flip the existing latest deployment for the same `targetArn` to INACTIVE/isLatestForTarget=false in the deployment state, and GetDeployment/ListDeployments must report those fields from it.
Traces: greengrassv2.deployment-supersede-marks-previous-inactive, greengrassv2.list-deployments-history-filter-latest-only-vs-all.

Cause 3 of 3: CreateComponentVersion performs no authorization, so a caller with no permissions succeeds.
AWS: Smithy: CreateComponentVersion models AccessDeniedException (HTTP 403) — "You don't have permission to perform the action." Recorded behavior: a caller using credentials from an assumed IAM role that has no attached policy is refused 403 `AccessDeniedException` with message `User: <assumed-role ARN> is not authorized to perform: greengrass:CreateComponentVersion on resource: arn:aws:greengrass:<region>:<account>:components:<componentName> because no identity-based policy allows the greengrass:CreateComponentVersion action` — fixed text `User: `, ` is not authorized to perform: greengrass:CreateComponentVersion on resource: `, ` because no identity-based policy allows the greengrass:CreateComponentVersion action`; the variables are the caller's assumed-role ARN (`arn:aws:sts::<account>:assumed-role/<roleName>/<sessionName>`), the region/account, and the component name from the recipe. No component is registered (ListComponents(scope=PRIVATE) does not contain it).
Emulator: answers 201 Created and registers the component version (`status.componentState=REQUESTED`) for the unprivileged role's credentials.
Why: the emulator is overly permissive — IAM-denial paths cannot be tested at all, and a caller that should be blocked mutates the registry.
Fix: the authorization check in CreateComponentVersion's handler (identity-based policy evaluation for the request's credentials) and the AccessDeniedException message it must compose.
Traces: greengrassv2.create-component-version-access-denied-rejected.

## Evidence
Cause 1 — create-deployment-nonexistent-target-not-found:
  expected: 400 `ValidationException`, message `Invalid Input: Target not found`, and no deployment recorded for that target
  actual: `AssertionError: real AWS accepted this call, but it should have been rejected; got {'ResponseMetadata': {'HTTPStatusCode': 201, ...}, 'deploymentId': '<uuid>', 'iotJobId': '<uuid>', 'iotJobArn': 'arn:aws:iot:<region>:<account>:job/<uuid>'}`
  create-deployment-nonexistent-thing-group-target-rejected (same, thinggroup/ ARN):
  actual: `AssertionError: expected ValidationException, but the call SUCCEEDED: {'ResponseMetadata': {'HTTPStatusCode': 201, ...}, 'deploymentId': '<uuid>', ...}`

Cause 2 — deployment-supersede-marks-previous-inactive and list-deployments-history-filter-latest-only-vs-all:
  expected: after the second CreateDeployment for the same thing-group target, GetDeployment(dep1) reports `deploymentStatus='INACTIVE'` and `isLatestForTarget=False`, GetDeployment(dep2) `ACTIVE`/`True`; ListDeployments(LATEST_ONLY) contains only dep2, ListDeployments(ALL) contains both
  actual: `Error: Check timed out after 60.0s` — both checks poll `get_deployment(deploymentId=dep1)['deploymentStatus']` for any non-ACTIVE value and it stays `ACTIVE` for the whole 60 s window, so the INACTIVE/isLatestForTarget=false state is never reached

Cause 3 — create-component-version-access-denied-rejected:
  expected: 403 `AccessDeniedException`, message `User: arn:aws:sts::<account>:assumed-role/<roleName>/<sessionName> is not authorized to perform: greengrass:CreateComponentVersion on resource: arn:aws:greengrass:<region>:<account>:components:<componentName> because no identity-based policy allows the greengrass:CreateComponentVersion action`
  actual: `AssertionError: expected the no-permission role's call to be denied, but it SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 201, ...}, 'arn': 'arn:aws:greengrass:<region>:<account>:components:<name>:versions:1.0.0', 'componentName': '<name>', 'componentVersion': '1.0.0', 'status': {'componentState': 'REQUESTED', 'vendorGuidance': 'ACTIVE'}}`

(ids, uuids and timestamps above are per-run values from the recording, not requirements.)

## Steps to Reproduce
1. Cause 1: `create_deployment(targetArn="arn:aws:iot:<region>:<account>:thing/<never-created-thing>")` and `create_deployment(targetArn="arn:aws:iot:<region>:<account>:thinggroup/<never-created-group>")` — expect 400 ValidationException `Invalid Input: Target not found`; observe 201 with a deploymentId. Then create the named thing/thing group and repeat the identical call: it must be the call that succeeds.
2. Cause 2: create an IoT thing group; `create_deployment(targetArn=<groupArn>, deploymentName='rev1')`, assert GetDeployment reports ACTIVE/isLatestForTarget=True; `create_deployment(targetArn=<groupArn>, deploymentName='rev2')`; poll GetDeployment(rev1) — expect it to settle `INACTIVE` with `isLatestForTarget=False` (and ListDeployments LATEST_ONLY vs ALL to differ); observe rev1 stays ACTIVE until the check times out.
3. Cause 3: create an IAM role with a trust policy for the account root and NO attached permission policy, `sts assume_role` it, build a greengrassv2 client with those credentials, and call `create_component_version(inlineRecipe=<valid recipe>)` — expect 403 AccessDeniedException with the authorization sentence and no component in `list_components(scope='PRIVATE')`; observe 201 and the component registered.
