Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__cognito-idp__lambda.cognito-pre-token-v2-trigger-group-override
Issue-Key: cognito-identity-provider|AdminInitiateAuth|UnexpectedLambdaException
Issue-Commit: 055568fd9ce9a7be471ce8dc9e6772f2075ef53d

# apigateway__cognito-idp__lambda: a PreTokenGeneration V2_0 trigger makes AdminInitiateAuth fail with UnexpectedLambdaException instead of issuing an ID token with the overridden cognito:groups claim (1 cause)

> _Note: this issue was identified against the code base at commit `055568fd9ce9a7be471ce8dc9e6772f2075ef53d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AdminInitiateAuth

## Description
Cause 1 of 1: a Cognito user pool whose LambdaConfig declares PreTokenGenerationConfig {LambdaVersion: V2_0} cannot complete an authentication, because the emulator cannot run the trigger function's deployment package and surfaces that as a failed trigger invocation.
AWS: AdminInitiateAuth with AuthFlow=ADMIN_USER_PASSWORD_AUTH on an ESSENTIALS-tier pool invokes the PreTokenGeneration trigger synchronously and then issues tokens. When the trigger returns response.claimsAndScopeOverrideDetails.groupOverrideDetails.groupsToOverride, Cognito replaces the cognito:groups claim baked into the issued ID token with exactly that list; the call returns AuthenticationResult with IdToken (Smithy: cognito-idp AdminInitiateAuth -> AdminInitiateAuthResponse.AuthenticationResult.IdToken; UnexpectedLambdaException is modelled for the case where AWS itself gets an unexpected exception FROM the trigger Lambda, not for a runtime that cannot execute). The decoded ID-token payload must carry cognito:groups == the groupsToOverride list.
Emulator: AdminInitiateAuth is refused. Recorded error: status/class `UnexpectedLambdaException` with the message `PreTokenGeneration failed with error Invoke resolved <function arn> at version $LATEST, but running its python3.12 deployment package is not implemented in this emulator: there is no in-process runtime for it, and no response payload is returned that the function did not produce. InvocationType=DryRun validates an invocation without running the function.` Fixed text in AWS's own wording for this class would be `PreTokenGeneration failed with error <error from the Lambda>` — here the variable part is not an error the function produced but an emulator notice that no runtime exists; the function ARN and version are the request/state-derived parts. The underlying cause is Lambda Invoke (RequestResponse) refusing every real invocation, which open issue 20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md names and which owns many other checks in this suite; this check carries no OWNED/ALREADY FILED marker, and the closely related open issues 20261028T162751Z-apigateway-cognito-idp-lambda-the-postconfirmation-trigger-produces.md (PostConfirmation) and 20261029T182116Z-apigateway-cognito-idp-lambda-execute-api-invocations-answer.md do not carry its replay key either — the V2_0 PreTokenGeneration claim/scope override path is distinct from both.
Why: a client that authenticates against a pool with any PreTokenGeneration trigger gets no tokens at all where AWS returns a full AuthenticationResult, and group/claim/scope customisation (the entire V2_0 claimsAndScopeOverrideDetails contract) can never be observed: an emulator that ignored groupOverrideDetails and issued the stored groups would be indistinguishable from a correct one.
Fix: the Lambda invoke path used by the Cognito trigger dispatcher (lambda Invoke RequestResponse must actually execute the uploaded deployment package and return its payload), plus the cognito-idp token-issuance code that applies response.claimsAndScopeOverrideDetails.groupOverrideDetails.groupsToOverride to the cognito:groups claim of the issued ID token for LambdaVersion V2_0.
Traces: apigateway__cognito-idp__lambda.cognito-pre-token-v2-trigger-group-override.

## Evidence
Failing check apigateway__cognito-idp__lambda.cognito-pre-token-v2-trigger-group-override [error]:

Expected (AWS): AdminInitiateAuth(AuthFlow=ADMIN_USER_PASSWORD_AUTH) returns AuthenticationResult.IdToken whose decoded payload has cognito:groups == ['sim2real-override-group'] (the trigger's groupsToOverride).

Actual (emulator): botocore.errorfactory.UnexpectedLambdaException: An error occurred (UnexpectedLambdaException) when calling the AdminInitiateAuth operation: PreTokenGeneration failed with error Invoke resolved arn:aws:lambda:<region>:<account>:function:<fn> at version $LATEST, but running its python3.12 deployment package is not implemented in this emulator: there is no in-process runtime for it, and no response payload is returned that the function did not produce. InvocationType=DryRun validates an invocation without running the function.

The preceding legs succeeded: CreateFunction, CreateUserPool with PreTokenGenerationConfig {LambdaVersion: V2_0} and UserPoolTier ESSENTIALS, AddPermission for cognito-idp.amazonaws.com, CreateUserPoolClient with ALLOW_ADMIN_USER_PASSWORD_AUTH, AdminCreateUser and AdminSetUserPassword.

## Steps to Reproduce
1. iam CreateRole trusted by lambda.amazonaws.com; lambda CreateFunction (python3.12) whose handler sets event['response']['claimsAndScopeOverrideDetails']['groupOverrideDetails']['groupsToOverride'] = ['sim2real-override-group'] for triggerSource starting with 'TokenGeneration'.
2. cognito-idp CreateUserPool with LambdaConfig {PreTokenGenerationConfig: {LambdaArn: <fn arn>, LambdaVersion: 'V2_0'}} and UserPoolTier='ESSENTIALS'; lambda AddPermission so cognito-idp may invoke it; CreateUserPoolClient with ExplicitAuthFlows ['ALLOW_ADMIN_USER_PASSWORD_AUTH'].
3. AdminCreateUser (MessageAction=SUPPRESS) + AdminSetUserPassword (Permanent=true).
4. AdminInitiateAuth with AuthFlow='ADMIN_USER_PASSWORD_AUTH' — observe UnexpectedLambdaException naming the unimplemented python3.12 runtime instead of an AuthenticationResult; the ID token's cognito:groups claim is therefore never overridden.
5. Or replay the check id in trace_ids.
