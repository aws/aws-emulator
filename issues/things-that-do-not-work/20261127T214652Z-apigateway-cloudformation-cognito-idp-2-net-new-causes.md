Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__cloudformation__cognito-idp.deploy-cfn-apigateway-authorizer-rest-api-move-replaces-authorizer apigateway__cloudformation__cognito-idp.deploy-cfn-apigateway-authorizer-move-to-unresolved-api-rejected apigateway__cloudformation__cognito-idp.deploy-cfn-authorizer-id-assigned-by-service-and-stable-across-update
Issue-Key: apigateway__cloudformation__cognito-idp.deploy-cfn-apigateway-authorizer-rest-api-move-replaces-authorizer|run#19|AssertionError|assert [(r["LogicalResourceId"], r["Action"], r["Replacement"]) for r in rows] == [
Issue-Key: apigateway__cloudformation__cognito-idp.deploy-cfn-apigateway-authorizer-move-to-unresolved-api-rejected|run#23|AssertionError|assert status == "UPDATE_COMPLETE", f"the control move left the stack {status!r}"
Issue-Key: apigateway__cloudformation__cognito-idp.deploy-cfn-authorizer-id-assigned-by-service-and-stable-across-update|run#23|AssertionError|assert _outputs(desc)["AuthorizerId"] == auth_id, "the Name update changed the AuthorizerId"
Issue-Commit: 9bfc3dcca28986b1e030db2b6d5b17f08a507141

# apigateway__cloudformation__cognito-idp: 2 net-new causes — an AWS::ApiGateway::Authorizer RestApiId change is not a replacement, and a Name update changes the AuthorizerId

> _Note: this issue was identified against the code base at commit `9bfc3dcca28986b1e030db2b6d5b17f08a507141`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStack

## Description
Two independent causes in the CloudFormation provider for AWS::ApiGateway::Authorizer (COGNITO_USER_POOLS authorizer over a Cognito user pool).

Cause 1 of 2: changing an authorizer's RestApiId is planned and executed as an in-place Modify instead of a Replacement.
AWS: recorded behavior only. RestApiId is create-only on AWS::ApiGateway::Authorizer, so moving the authorizer onto another AWS::ApiGateway::RestApi previews a change with `Replacement: True`, and executing it creates the authorizer on the new API and deletes the old one, leaving the stack UPDATE_COMPLETE. When the new RestApiId cannot be resolved the update is refused and the stack rolls back — but that is the negative leg, and it must be reached from an update that AWS would otherwise plan as a replacement.
Emulator: the change set previews `[('PoolGate', 'False')]` — Replacement False — and the control move (onto a resolvable API) leaves the stack `UPDATE_ROLLBACK_COMPLETE` instead of UPDATE_COMPLETE, so the authorizer is never moved.
Why: a client previewing the update sees no replacement warning for a create-only property, and the update then fails and rolls back, so an authorizer can never be moved between APIs by a stack update.
Fix: the AWS::ApiGateway::Authorizer resource provider's create-only property set (RestApiId) and the change-set Replacement computation, plus its replacement execution path.
Traces: apigateway__cloudformation__cognito-idp.deploy-cfn-apigateway-authorizer-rest-api-move-replaces-authorizer, apigateway__cloudformation__cognito-idp.deploy-cfn-apigateway-authorizer-move-to-unresolved-api-rejected

Cause 2 of 2: an update that only changes the authorizer's Name changes its AuthorizerId.
AWS: recorded behavior only. The AuthorizerId is assigned by API Gateway at create and is the resource's physical id; Name is an updatable (not create-only) property, so a Name-only update is applied in place and the AuthorizerId is unchanged.
Emulator: after the Name update the AuthorizerId is different — the provider re-created the authorizer (or re-minted the id) for an updatable property.
Why: every reference to that authorizer (a method's authorizerId, a stack output, an exported value, anything a client stored) silently points at an id that no longer exists after a cosmetic rename.
Fix: the AWS::ApiGateway::Authorizer resource provider's update action — update Name in place and keep the service-assigned AuthorizerId as the physical id.
Traces: apigateway__cloudformation__cognito-idp.deploy-cfn-authorizer-id-assigned-by-service-and-stable-across-update

## Evidence
Cause 1 — "move-gate planned [('PoolGate', 'False')]" (expected a Replacement True row for the RestApiId change); "the control move left the stack 'UPDATE_ROLLBACK_COMPLETE'" (expected UPDATE_COMPLETE).
Cause 2 — 'the Name update changed the AuthorizerId' (expected the service-assigned AuthorizerId to be stable across a Name-only update).

## Steps to Reproduce
1. CreateStack with a Cognito user pool, two AWS::ApiGateway::RestApi resources and an AWS::ApiGateway::Authorizer (Type COGNITO_USER_POOLS) bound to the first API.
2. CreateChangeSet moving the authorizer's RestApiId to the second API; DescribeChangeSet and read the Replacement flag, then ExecuteChangeSet and read the terminal StackStatus (Cause 1).
3. UpdateStack changing only the authorizer's Name; compare DescribeStackResource PhysicalResourceId / GetAuthorizers before and after (Cause 2).
