Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__cloudformation.deploy-cfn-apigateway-apikey-generate-distinct-id-change-replaces-key apigateway__cloudformation.deploy-cfn-apigateway-apikey-name-change-replaces-key apigateway__cloudformation.deploy-cfn-apigateway-apikey-change-set-stale-after-replacement-rejected apigateway__cloudformation.deploy-cfn-apigateway-later-failure-rolls-back-usage-plan-bound-to-stage apigateway__cloudformation.deploy-cfn-gateway-response-type-change-replaces-response apigateway__cloudformation.deploy-cfn-gateway-response-released-type-delete-rejected
Issue-Key: apigateway__cloudformation.deploy-cfn-apigateway-apikey-generate-distinct-id-change-replaces-key|run#17|AssertionError|assert summary == [(_KEY, "Modify", "True")], f"change set planned {summary!r}"
Issue-Key: apigateway__cloudformation.deploy-cfn-apigateway-apikey-name-change-replaces-key|_apply#15|AssertionError|assert status == "UPDATE_COMPLETE", f"executing {label} left the stack {status!r}"
Issue-Key: apigateway__cloudformation.deploy-cfn-apigateway-apikey-change-set-stale-after-replacement-rejected|run#19|AssertionError|assert got == ("ChangeSetNotFound", "ChangeSet [edit-note] does not exist"), (
Issue-Key: apigateway__cloudformation.deploy-cfn-apigateway-later-failure-rolls-back-usage-plan-bound-to-stage|run#14|AssertionError|assert bad["StackStatus"] == "ROLLBACK_COMPLETE", (
Issue-Key: apigateway__cloudformation.deploy-cfn-gateway-response-type-change-replaces-response|run#13|AssertionError|assert observed == (False, "401", {_MEDIA: _PAYLOAD}), (
Issue-Key: apigateway__cloudformation.deploy-cfn-gateway-response-released-type-delete-rejected|run#10|AssertionError|assert _state(apigw, api_id, "QUOTA_EXCEEDED") == (False, "418"), (
Issue-Commit: 9bfc3dcca28986b1e030db2b6d5b17f08a507141

# apigateway__cloudformation: 3 net-new causes — an ApiKey Name/GenerateDistinctId change is planned as Modify instead of Replacement, a usage-plan rollback fails, and AWS::ApiGateway::GatewayResponse is never applied to the API

> _Note: this issue was identified against the code base at commit `9bfc3dcca28986b1e030db2b6d5b17f08a507141`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStack

## Description
Three independent causes in how CloudFormation updates and rolls back AWS::ApiGateway::* resources. Distinct from the already-open 20261109T123922Z-apigateway-cloudformation-6-net-new-causes-a-not, which owns the documentation / gateway-response round-trip and usage-plan-key causes.

Cause 1 of 3: changing an AWS::ApiGateway::ApiKey's Name (or GenerateDistinctId) is planned and executed as an in-place Modify, not a Replacement.
AWS: recorded behavior only. Both Name and GenerateDistinctId are create-only on AWS::ApiGateway::ApiKey, so a change to either previews `Replacement: True` in the change set, and executing it creates a new key and deletes the old one, leaving the stack UPDATE_COMPLETE. A change set planned BEFORE that replacement becomes OBSOLETE afterwards, and executing it is refused with InvalidChangeSetStatus naming the OBSOLETE status — which is the behavior the stale-change-set check asserts once the replacement actually happens.
Emulator: the change set previews `[('DistinctKey', 'Modify', 'False')]` — Modify with Replacement False — and executing the Name change leaves the stack `UPDATE_ROLLBACK_COMPLETE` instead of UPDATE_COMPLETE. Consequently the stale-change-set scenario never reaches the state it grades (it reports the change set as OBSOLETE without the replacement having happened).
Why: a client that previews an update sees no replacement warning for a create-only property, and then the update fails and rolls back instead of applying.
Fix: the AWS::ApiGateway::ApiKey resource provider's create-only property set and the change-set Replacement computation, plus the replacement execution path.
Traces: apigateway__cloudformation.deploy-cfn-apigateway-apikey-generate-distinct-id-change-replaces-key, apigateway__cloudformation.deploy-cfn-apigateway-apikey-name-change-replaces-key, apigateway__cloudformation.deploy-cfn-apigateway-apikey-change-set-stale-after-replacement-rejected

Cause 2 of 3: rolling back a stack whose AWS::ApiGateway::UsagePlan is bound to a stage leaves ROLLBACK_FAILED.
AWS: recorded behavior only. When a later resource fails during create, the rollback deletes the usage plan's stage association before deleting the plan, so the stack reaches ROLLBACK_COMPLETE.
Emulator: the stack ends `ROLLBACK_FAILED` with `The following resource(s) failed to delete: [Plan]. ` because DeleteUsagePlan is refused `Cannot delete Usage Plan <plan id> because there are API Stages associated with it, including <api id>:<stage>` — the rollback never detaches the apiStages first.
Why: a failed deployment leaves the stack in ROLLBACK_FAILED, which blocks every subsequent update and requires manual intervention; AWS leaves it cleanly rolled back.
Fix: the AWS::ApiGateway::UsagePlan resource provider's delete action during rollback (detach apiStages, or order the dependent delete first).
Traces: apigateway__cloudformation.deploy-cfn-apigateway-later-failure-rolls-back-usage-plan-bound-to-stage

Cause 3 of 3: an AWS::ApiGateway::GatewayResponse created by a stack is not written into the REST API.
AWS: recorded behavior only. After the stack reaches CREATE_COMPLETE, GetGatewayResponse for the customized responseType reports `defaultResponse: false`, the template's statusCode and the template's responseTemplates; every other responseType still reports `defaultResponse: true`. A ResponseType change then replaces the resource (the old type reverts to the default, the new type becomes customized), and deleting a released type is refused.
Emulator: GetGatewayResponse for the stack's type still reports the built-in default — `(defaultResponse=True, statusCode='401', responseTemplates={'application/json': '{"message":$context.error.messageString}'})` where the stack set statusCode 401 with its own template, and `(defaultResponse=True, statusCode='429')` where the stack set QUOTA_EXCEEDED to 418 — so the resource provider reported success without customizing the gateway response.
Why: gateway-response customization declared in a template has no effect on the deployed API, and nothing downstream (the replace-on-type-change and delete-refusal semantics) can work.
Fix: the AWS::ApiGateway::GatewayResponse resource provider's create/update/delete actions — call PutGatewayResponse / DeleteGatewayResponse against the API.
Traces: apigateway__cloudformation.deploy-cfn-gateway-response-type-change-replaces-response, apigateway__cloudformation.deploy-cfn-gateway-response-released-type-delete-rejected

## Evidence
Cause 1 — "change set planned [('DistinctKey', 'Modify', 'False')]" (expected a Replacement True row); "executing describe-only left the stack 'UPDATE_ROLLBACK_COMPLETE'"; and the stale-change-set leg reported ('InvalidChangeSetStatus', 'ChangeSet [...] cannot be executed in its current execution status of [OBSOLETE]') without a replacement having occurred.
Cause 2 — "stack ended 'ROLLBACK_FAILED' ('The following resource(s) failed to delete: [Plan]. '); resource failures: [('Bad','CREATE_FAILED',...), ('Plan','DELETE_FAILED','Resource handler returned message: \"Cannot delete Usage Plan <id> because there are API Stages associated with it, including <api>:<stage> ...\"')]".
Cause 3 — expected GetGatewayResponse UNAUTHORIZED == (False, '401', {<media>: <stack template>}); got (True, '401', {'application/json': '{"message":$context.error.messageString}'}). Expected QUOTA_EXCEEDED == (False, '418'); got (True, '429').

## Steps to Reproduce
1. CreateStack with an AWS::ApiGateway::ApiKey (Name set / GenerateDistinctId set); CreateChangeSet changing Name (and GenerateDistinctId) and inspect the planned Replacement, then ExecuteChangeSet and read the terminal StackStatus (Cause 1).
2. CreateStack with a RestApi + Stage + UsagePlan bound to that stage plus a resource that fails to create (e.g. an invalid ClientCertificateId); wait for the terminal status (Cause 2).
3. CreateStack with an AWS::ApiGateway::GatewayResponse customizing UNAUTHORIZED (401 + responseTemplates) and QUOTA_EXCEEDED (418); after CREATE_COMPLETE call GetGatewayResponse for those types (Cause 3).
