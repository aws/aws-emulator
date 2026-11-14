Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-identity.deploy-cognito-identity-3-cfn-principal-tag-without-tag-session-rejected cognito-identity.deploy-cognito-identity-3-cfn-principal-tag-governs-tenant-access
Issue-Shape-Id: com.amazonaws.cognitoidentity#AWSCognitoIdentityService
Issue-Key: cognito-identity|GetPrincipalTagAttributeMap|ValidationException
Issue-Key: cognito-identity.deploy-cognito-identity-3-cfn-principal-tag-governs-tenant-access|run#17|AssertionError|assert out["PrincipalTagRef"] == f"{pool_id}|{provider}", (
Issue-Commit: 1fce8a5a4e99700c9d3c5ad3e1ef05753020fc4d

# cognito-identity: 2 net-new CloudFormation root causes — Ref of AWS::Cognito::IdentityPool still resolves to the pool NAME, and Ref of AWS::Cognito::IdentityPoolPrincipalTag is a stack-generated name instead of the compound "<poolId>|<providerName>"

> _Note: this issue was identified against the code base at commit `1fce8a5a4e99700c9d3c5ad3e1ef05753020fc4d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetPrincipalTagAttributeMap

## Description
This issue bundles TWO distinct net-new root causes reached through cognito-identity behavioral checks in suite simparity-behavioral-ga-prod-v1. Both must be fixed and the full trace_ids list re-replayed before closing — each cause below carries its own failing check ids.

=== Cause 1 of 2: the physical id (what Ref / a stack Output resolves to) of a CloudFormation AWS::Cognito::IdentityPool is the pool NAME, not the pool id ===
AWS: Ref on AWS::Cognito::IdentityPool resolves to the identity-pool id in REGION:GUID form, i.e. the modelled IdentityPoolId (Smithy: GetPrincipalTagAttributeMap.IdentityPoolId / SetPrincipalTagAttributeMap.IdentityPoolId / GetId.IdentityPoolId, @required @length{1,55} @pattern ^[\w-]+:[0-9a-f-]+$), so a value read from a stack Output built from !Ref can be handed straight to GetId, DescribeIdentityPool, GetPrincipalTagAttributeMap or SetIdentityPoolRoles and is served.
Emulator: the stack reaches a terminal OK status and the Output carries the template's IdentityPoolName, so the next cognito-identity call made with that value is refused HTTP 400 ValidationException with message "1 validation error detected: Value '<the value submitted, here the pool NAME>' at 'identityPoolId' failed to satisfy constraint: Member must satisfy regular expression pattern: [\w-]+:[0-9a-f-]+". Fixed text of that message: "1 validation error detected: Value '", "' at 'identityPoolId' failed to satisfy constraint: Member must satisfy regular expression pattern: [\w-]+:[0-9a-f-]+"; the variable part is the rejected value taken from the request (in the failing check it was the template's IdentityPoolName). The refusal itself is correct for that input — the defect is that CloudFormation handed the client a name where AWS hands back a pool id.
Why: no CloudFormation-deployed identity pool is addressable through the Cognito API, so every infrastructure-as-code flow that deploys a pool and then exercises it (role attachment, principal-tag mapping, TagSession enforcement, stack-update-changes-behavior) is a dead end even though CloudFormation reports success.
Fix: the CloudFormation resource provider for AWS::Cognito::IdentityPool — it must create the pool in cognito-identity state and record the real pool id as the resource's physical resource id (what Ref / Fn::GetAtt / Outputs resolve to), and delete that pool by id on stack delete.
Related but NOT covering: open issue 20261025T061337Z-cognito-identity-6-net-new-root-causes-cfn.md describes this same provider defect as its Cause 1, but its trace list carries only the five deploy-identity-pool / deploy-cfn-role-attachment check ids; the check below is in neither its trace list nor any other open issue's, and the replay report does not mark it OWNED.
Traces: cognito-identity.deploy-cognito-identity-3-cfn-principal-tag-without-tag-session-rejected — the stack Outputs MappedPoolId/UnmappedPoolId are {"Ref": "MappedPool"} / {"Ref": "UnmappedPool"}; the first GetPrincipalTagAttributeMap made with MappedPoolId aborts the check with the ValidationException above.

=== Cause 2 of 2: Ref of AWS::Cognito::IdentityPoolPrincipalTag returns a stack-generated name instead of the resource's compound primary identifier ===
AWS: recorded behavior only (the resource is a CloudFormation type, not a Smithy shape). AWS::Cognito::IdentityPoolPrincipalTag has the compound primary identifier [IdentityPoolId, IdentityProviderName], so its Ref — and its physical resource id — is the two identifiers joined by a pipe: "<identity pool id>|<identity provider name>". Both halves are values from the request/state: the pool id in REGION:GUID form and the provider name (e.g. the user pool's ProviderName).
Emulator: Ref resolves to a synthesized logical name of the form "<stack name>-<logical resource id>-<random suffix>" (replay: 'sim2real-cfn-cip-ptag-5af006dc46c7-PrincipalTag-5BDF3FAD497D'), which contains neither identifier and cannot be split into them.
Why: a client cannot recover the pool id or the provider name of a CFN-declared principal-tag mapping from its Ref, so neither GetPrincipalTagAttributeMap nor any downstream use (import, cross-stack reference, deletion by identifier) can be driven from the stack; the mapping resource is unaddressable even though the stack reports success.
Fix: the CloudFormation resource provider for AWS::Cognito::IdentityPoolPrincipalTag — it must call SetPrincipalTagAttributeMap and record "<pool id>|<provider name>" as the resource's physical resource id.
Traces: cognito-identity.deploy-cognito-identity-3-cfn-principal-tag-governs-tenant-access.

## Evidence
Replay of suite simparity-behavioral-ga-prod-v1 (84/148 passed). Per-cause replay output:

Cause 1 — cognito-identity.deploy-cognito-identity-3-cfn-principal-tag-without-tag-session-rejected [error]: botocore.exceptions.ClientError: An error occurred (ValidationException) when calling the GetPrincipalTagAttributeMap operation: 1 validation error detected: Value 'sim2real_cip_mapped_b73994db12b3' at 'identityPoolId' failed to satisfy constraint: Member must satisfy regular expression pattern: [\w-]+:[0-9a-f-]+. The value came from stack Output MappedPoolId = {"Ref": "MappedPool"}, and 'sim2real_cip_mapped_b73994db12b3' is exactly the IdentityPoolName the template declared for that resource. Expected: the Output carries the pool id (REGION:GUID) and GetPrincipalTagAttributeMap is served, returning PrincipalTags {"tenant": "custom:tenant"}.

Cause 2 — cognito-identity.deploy-cognito-identity-3-cfn-principal-tag-governs-tenant-access [mismatch]: AssertionError: Ref of AWS::Cognito::IdentityPoolPrincipalTag was 'sim2real-cfn-cip-ptag-5af006dc46c7-PrincipalTag-5BDF3FAD497D', expected '<pool id>|<provider name>' = 'sim2real_cip_ptag_7d6ff2a6bc42|cognito-idp.us-west-2.amazonaws.com/sim2real-cip-ptag-up-e7845a63156f'. (The expected string the check built also shows Cause 1 at work: its '<pool id>' half is itself a Ref of the AWS::Cognito::IdentityPool and reads as the pool NAME 'sim2real_cip_ptag_7d6ff2a6bc42'. The names, ids and suffixes in both strings are per-run values; the requirement is the FORM — the pipe-joined pool id and provider name.)

Model facts used (smithy_query on services/service-com-amazonaws-cognitoidentity-awscognitoidentityservice/smithy-model/com-amazonaws-cognitoidentity-awscognitoidentityservice.json): GetPrincipalTagAttributeMap input IdentityPoolId @required @length{1,55} @pattern ^[\w-]+:[0-9a-f-]+$ and IdentityProviderName @required @length{1,128}; errors InvalidParameterException 400, NotAuthorizedException 403, ResourceNotFoundException 404, TooManyRequestsException 429, InternalErrorException 500. SetPrincipalTagAttributeMap has the same IdentityPoolId constraint and a PrincipalTags map @length{0,50}.

Both check ids carry the service token 'cognito-identity', which is bootstrapped in this checkout.

## Steps to Reproduce
1. Clone the repo, boot the emulator and replay suite simparity-behavioral-ga-prod-v1; re-replay exactly the two ids in trace_ids to confirm a fix.
2. Cause 1: CreateStack (Capabilities=[CAPABILITY_NAMED_IAM]) with a template declaring AWS::Cognito::UserPool + AWS::Cognito::UserPoolClient, two AWS::Cognito::IdentityPool resources (MappedPool/UnmappedPool, AllowUnauthenticatedIdentities=false, CognitoIdentityProviders pointing at the user pool), an AWS::Cognito::IdentityPoolPrincipalTag on MappedPool with PrincipalTags={"tenant":"custom:tenant"} and UseDefaults=false, and Outputs MappedPoolId={"Ref":"MappedPool"} / UnmappedPoolId={"Ref":"UnmappedPool"}. Wait for CREATE_COMPLETE, read the Outputs, then call cognito-identity GetPrincipalTagAttributeMap(IdentityPoolId=<MappedPoolId>, IdentityProviderName=<ProviderName>). Expect the mapping to be served; observe the Output holding the pool NAME and the call refused 400 ValidationException on 'identityPoolId'.
3. Cause 2: in the same stack, add an Output PrincipalTagRef={"Ref":"PrincipalTag"} (and/or call DescribeStackResource on that logical id). Expect the value "<pool id>|<identity provider name>"; observe a '<stack>-PrincipalTag-<suffix>' generated name.
