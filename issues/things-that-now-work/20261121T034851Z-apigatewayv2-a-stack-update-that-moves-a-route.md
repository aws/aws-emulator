Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2.deploy-apigatewayv2-1-http-api-cors-removed-in-place-then-api-replaced
Issue-Shape-Id: com.amazonaws.apigatewayv2#ApiGatewayV2
Issue-Key: apigatewayv2.deploy-apigatewayv2-1-http-api-cors-removed-in-place-then-api-replaced|run#44|AssertionError|assert preview.get("CorsApi", {}).get("action") == "Remove", (
Issue-Commit: d5ceaf476328a7e82fef92afef1bcc0e0278d2e9

# apigatewayv2: a stack update that moves a Route onto a new AWS::ApiGatewayV2::Api previews no Remove/Add for the condition-flipped Api resources, so the replacement is never shown

> _Note: this issue was identified against the code base at commit `d5ceaf476328a7e82fef92afef1bcc0e0278d2e9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateChangeSet

## Description
One net-new root cause, grounded in one failing check.

Cause 1 of 1: an UPDATE change set whose new template drops one condition-guarded AWS::ApiGatewayV2::Api logical id (CorsApi) and introduces another (ReplacementApi), repointing the Route's ApiId at the new one, is previewed with NO change entry for either Api. The emulator's DescribeChangeSet Changes list only the dependents, and classifies the Route that must be recreated as replacement Conditional.

AWS: Smithy: cloudformation#ChangeAction is an enum of Add, Modify, Remove, Import, Dynamic, SyncWithActual; cloudformation#Replacement is an enum of True, False, Conditional; ResourceChange.Details carries ResourceTargetDefinition.RequiresRecreation (Never | Always | Conditionally), and ResourceChange.Replacement is documented as "if the RequiresRecreation field is Always and the Evaluation field is Static, Replacement is True". Recorded behavior: for this update the change set reaches CREATE_COMPLETE and its Changes carry, by logical id, a Remove entry for the Api that the new template no longer declares, an Add entry for the newly declared Api, and a Modify entry for the Route with Replacement True and a detail whose target is the ApiId property with RequiresRecreation Always. Executing it then creates a new ApiId, replaces the Route (new physical id), and deletes the old Api (a later GetApi on it is refused NotFoundException "Invalid API identifier specified <account>:<apiId>" — fixed text "Invalid API identifier specified " plus the account and the ApiId from the request).

Emulator: the preview for the same update lists only {'DefaultStage': Modify/Conditional [('ApiId','Always')], 'EchoIntegration': Modify/Conditional [('ApiId','Always')], 'InvokePermission': Modify/False [('SourceArn','Never')], 'RouteA': Modify/Conditional [('ApiId','Always'), ('Target','Never')]}. There is no entry at all for the removed Api logical id and none for the added one, and the Route whose ApiId detail is RequiresRecreation Always is reported Replacement 'Conditional' rather than 'True'.

Why: a client reading the change set sees an in-place modification of a few dependents and no API-level churn, when executing the plan will in fact destroy one HTTP API (its ApiId, its endpoint hostname and every route on it) and create another. Replacement review — the one gate CloudFormation offers before a destructive update — passes a plan whose destruction it never disclosed, and a Replacement of 'Conditional' instead of 'True' tells approval tooling the recreation may not happen.

Fix: the change-set differ that builds ResourceChange entries for an UPDATE (it must evaluate template Conditions before diffing the resource set, so a logical id that leaves the resource set yields Action=Remove and one that enters it yields Action=Add), and the Replacement classification that derives Replacement from the most impactful RequiresRecreation of the collected details.

Related but not the same as the open cloudformation issue 20261117T184012Z-cloudformation-9-net-new-cloudformation-causes-change-sets-miss (its Cause 1 is a condition-flip-only diff being reported as FAILED "didn't contain changes"); here the change set does reach CREATE_COMPLETE and silently omits the Add/Remove pair.

Traces: apigatewayv2.deploy-apigatewayv2-1-http-api-cors-removed-in-place-then-api-replaced

## Evidence
Check apigatewayv2.deploy-apigatewayv2-1-http-api-cors-removed-in-place-then-api-replaced, update #2 (Phase=replaced):

Expected (recorded AWS): DescribeChangeSet Changes contain CorsApi -> {action: 'Remove'}, ReplacementApi -> {action: 'Add'}, RouteA -> {action: 'Modify', replacement: 'True', details including ('ApiId', 'Always')}.

Actual (emulator):
AssertionError: update #2 must remove the old Api resource, the change set lists {'DefaultStage': {'action': 'Modify', 'replacement': 'Conditional', 'details': [('ApiId', 'Always')]}, 'EchoIntegration': {'action': 'Modify', 'replacement': 'Conditional', 'details': [('ApiId', 'Always')]}, 'InvokePermission': {'action': 'Modify', 'replacement': 'False', 'details': [('SourceArn', 'Never')]}, 'RouteA': {'action': 'Modify', 'replacement': 'Conditional', 'details': [('ApiId', 'Always'), ('Target', 'Never')]}}

So: no ResourceChange with Action=Remove for the Api logical id the new template no longer declares, no ResourceChange with Action=Add for the Api it newly declares, and the Route carrying a RequiresRecreation=Always ApiId detail is Replacement='Conditional' where the recording requires 'True'. Earlier legs of the same check (create, and update #1 that removes CorsConfiguration in place) passed, so the divergence is confined to the change-set diff of this condition flip.

## Steps to Reproduce
Replay the behavioral suite and re-run exactly this check:
1. apigatewayv2.deploy-apigatewayv2-1-http-api-cors-removed-in-place-then-api-replaced

Manually: deploy the check's deployments/http-api-lambda-cors-update-replace.yaml with Phase=cors (an AWS::ApiGatewayV2::Api logical id CorsApi guarded by a Phase condition, plus Route/Integration/Stage referencing it), wait for CREATE_COMPLETE, then CreateChangeSet(ChangeSetType=UPDATE) on the same stack with the same template and Phase=replaced (CorsApi's condition false, ReplacementApi's condition true, RouteA.ApiId repointed at ReplacementApi). DescribeChangeSet and inspect Changes: the entries for CorsApi (Action Remove) and ReplacementApi (Action Add) are absent, and RouteA reports Replacement 'Conditional' with Details target ApiId / RequiresRecreation Always.
