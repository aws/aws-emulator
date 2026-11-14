Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-identity.vended-credentials-bounded-by-role-policy cognito-identity.developer-authenticated-credentials-write-and-read-dynamodb cognito-identity.get-identity-pool-roles-rules-evaluated-in-declared-order cognito-identity.server-side-role-selection-ignores-client-supplied-hints-rejected cognito-identity.server-side-sts-role-session-tags-enforced-in-trust-policy
Issue-Shape-Id: com.amazonaws.cognitoidentity#AWSCognitoIdentityService
Issue-Key: cognito-identity.vended-credentials-bounded-by-role-policy|_denial#16|AssertionError|raise AssertionError(f"{what} must be refused, but the call succeeded")
Issue-Key: cognito-identity.developer-authenticated-credentials-write-and-read-dynamodb|_denial#38|AssertionError|raise AssertionError(f"expected the call to be refused, but it SUCCEEDED and returned {result!r}")
Issue-Key: cognito-identity.get-identity-pool-roles-rules-evaluated-in-declared-order|run#24|AssertionError|assert f"assumed-role/{role_b_name}/" in arn2, (
Issue-Key: cognito-identity.server-side-role-selection-ignores-client-supplied-hints-rejected|run#14|AssertionError|assert code == "InvalidParameterException", f"expected InvalidParameterException, got {code}"
Issue-Key: cognito-identity.server-side-sts-role-session-tags-enforced-in-trust-policy|_retry_get_credentials#17|Timeout|time.sleep(interval_s)
Issue-Commit: 3b8e580b43dad03fd4ed613a27e30f57a2ae4b6e

# cognito-identity: 4 net-new root causes — a vended session is authorization-unbounded, rules-based role mapping ignores declared order, CustomRoleArn is refused with the wrong error, and principal tags never become requested session tags

> _Note: this issue was identified against the code base at commit `3b8e580b43dad03fd4ed613a27e30f57a2ae4b6e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetCredentialsForIdentity

## Description
Four distinct net-new root causes in the cognito-identity credential-vending path. Each is listed separately with its own failing check ids; all four must be closed before this issue is submitted.

Related open issues that do NOT cover these (they describe other causes in the same area and none of these checks is marked OWNED by them): 20261025T005354Z-cognito-identity-getcredentialsforidentity-never-performs-a-real-assumerolewithwebidentity.md, 20261025T005354Z-cognito-identity-getid-admits-unvalidatable-login-tokens-setprincipaltagattributemap.md, 20261025T061337Z-cognito-identity-6-net-new-root-causes-cfn.md.

Cause 1 of 4: the STS session GetCredentialsForIdentity vends is not bounded by the mapped role's policies — calls AWS refuses succeed.
  AWS: the credentials GetCredentialsForIdentity returns are an sts:AssumeRoleWithWebIdentity session of the pool's mapped role, and every subsequent call made with them is authorized against that role's identity-based policy (plus the session policy Cognito injects). A call the role is not granted is REFUSED: DynamoDB answers HTTP 400 `AccessDeniedException` and S3 answers HTTP 403 `AccessDenied`, with AWS's fixed sentence `User: <principal-arn> is not authorized to perform: <action> on resource: <resource> because no identity-based policy allows the <action> action` (the `on resource: <resource>` clause is absent for resource-free actions such as s3:ListAllMyBuckets, and the ceiling clause reads `because no session policy allows the <action> action` when the injected session policy is what stopped it). Fixed text: `is not authorized to perform:`, `because no identity-based policy allows the`/`because no session policy allows the`, ` action`. Variable parts, taken from the request and emulator state: the vended principal ARN `arn:<partition>:sts::<account-id>:assumed-role/<mapped-role-name>/CognitoIdentityCredentials`, the action, and the resource ARN. Smithy: IAM/STS authorization is not modeled on these operations — recorded behavior only; the recordings show the refusals above.
  Emulator: both calls succeed with HTTP 200. `dynamodb:Scan` under a developer-authenticated vended session returned `{'Items': [...], 'Count': 1, 'ScannedCount': 1}` (HTTPStatusCode 200) where the recording has a refusal; `s3:ListAllMyBuckets` under a session of a role that carries NO permission policy at all also returned success (`s3:ListAllMyBuckets under credentials for a role with no permission policy must be refused, but the call succeeded`).
  Why: a client that relies on an identity pool's role policy as its tenant boundary gets none. Every guest/authenticated session the emulator vends is effectively unlimited, so least-privilege regressions (a role with no policy, a role scoped to one prefix/table) pass silently against the emulator and fail only in production — the exact class of bug this scenario exists to catch.
  Fix: the authorization decision for requests signed with a Cognito-vended role session — the engine's shared IAM/STS session authorization layer that the s3 and dynamodb request paths consult, not the cognito-identity handler, which does mint a session (see Traces).
  Traces: cognito-identity.vended-credentials-bounded-by-role-policy, cognito-identity.developer-authenticated-credentials-write-and-read-dynamodb. Session minting lives in services/service-com-amazonaws-cognitoidentity-awscognitoidentityservice/src/ops/get_credentials_for_identity.rs:369-425 (`mint_vended_role_session`), so the gap is on the consuming side.

Cause 2 of 4: RulesConfiguration rules are never evaluated, so declared rule order does not decide the vended role.
  AWS: Smithy `RulesConfigurationType.Rules` (list of `MappingRule`, up to 25 per provider) is documented: "Rules are evaluated in order. The first one to match specifies the role." With two rules that both match every token, the FIRST declared rule's `RoleARN` is assumed, and reordering the array changes which role the vended session stands for — observable through sts:GetCallerIdentity returning `arn:<partition>:sts::<account-id>:assumed-role/<first-matching-rule-role>/CognitoIdentityCredentials`.
  Emulator: after the two rules are reordered so role B's rule is first, the vended session still resolves to role A: `expected role B to win after it became the first declared rule, got arn:aws:sts::000000000000:assumed-role/sim2real-behav-role-f74c74f57702/CognitoIdentityCredentials`. The handler resolves the role only from `Roles{authenticated|unauthenticated}` (`configured.role_for(!providers.is_empty())`), so `RoleMappings`/`RulesConfiguration` are not consulted at all.
  Why: a pool that selects roles by token claim (admin vs member, tenant vs tenant) always gets the pool-wide default role instead, so rule ordering and claim-based privilege separation are unobservable and untestable.
  Fix: role resolution in the GetCredentialsForIdentity handler — it must consult the pool's stored `RoleMappings` for the provider key and apply the first matching `MappingRule` in declared array order before falling back to the `Roles` map.
  Traces: cognito-identity.get-identity-pool-roles-rules-evaluated-in-declared-order; role resolution at services/service-com-amazonaws-cognitoidentity-awscognitoidentityservice/src/ops/get_credentials_for_identity.rs:301-319.

Cause 3 of 4: a CustomRoleArn the pool does not support is refused with the wrong error code and message.
  AWS: Smithy models both `InvalidParameterException` (HTTP 400) and `InvalidIdentityPoolConfigurationException` (HTTP 400) on GetCredentialsForIdentity, and `CustomRoleArn` is documented as supported only "for identity providers that do not support role customization" being excluded. For a pool whose provider has no RoleMappings, a CustomRoleArn is a bad INPUT, not a broken pool: AWS answers HTTP 400 `InvalidParameterException` with the fixed sentence `Only SAML providers and providers with RoleMappings support custom role ARN.` (entirely fixed text — no request- or state-derived parts).
  Emulator: answers `InvalidIdentityPoolConfigurationException` (message `Invalid identity pool configuration. Check assigned IAM roles for this pool.`) because the handler treats "the CustomRoleArn is not one the pool names" as "no role resolvable" and falls into the pool-configuration branch.
  Why: the two codes mean opposite things to a client. `InvalidParameterException` is a caller bug to be fixed in the request; `InvalidIdentityPoolConfigurationException` is documented as the pool/role-trust being broken and is exactly the code SDK retry/propagation loops treat as transient (the suite's own helpers retry on it). A client that passes a bad CustomRoleArn therefore sees a retried, mis-attributed operator error instead of a deterministic parameter rejection.
  Fix: the CustomRoleArn branch of the GetCredentialsForIdentity handler's role resolution — distinguish "the request named a custom role this provider cannot customize" (InvalidParameterException, AWS's sentence above) from "the pool names no assumable role" (InvalidIdentityPoolConfigurationException).
  Traces: cognito-identity.server-side-role-selection-ignores-client-supplied-hints-rejected; services/service-com-amazonaws-cognitoidentity-awscognitoidentityservice/src/ops/get_credentials_for_identity.rs:302-319.

Cause 4 of 4: the assume carries no REQUESTED session tags, so a role trust policy gated on aws:RequestTag/<key> can never admit a Cognito vend.
  AWS: a pool with a PrincipalTagAttributeMap performs AssumeRoleWithWebIdentity *with TagSession*, so the mapped attribute values arrive as REQUESTED session tags and the assumed role's own trust policy can gate them with `aws:RequestTag/<key>`. An identity whose mapped tag value satisfies the condition gets credentials (HTTP 200 with Credentials); an identity whose value does not is refused at assumption time with HTTP 400 `InvalidIdentityPoolConfigurationException` and AWS's fixed sentence `Invalid identity pool configuration. Check assigned IAM roles for this pool.` Smithy: `GetOpenIdTokenForDeveloperIdentity`/`SetPrincipalTagAttributeMap` model `PrincipalTags` (map, 0..50); the trust-policy decision itself is recorded behavior only.
  Emulator: the check never completes and is killed at the 60 s harness cap. Its only unbounded wait is the propagation retry that polls GetCredentialsForIdentity for the MATCHING-tag identity (its positive control) before any denial is believed, so the matching identity never obtains credentials. Hypothesis, from the handler: the trust decision is made by `state::role_admits_web_identity` against a context built by `state::web_identity_condition_context(identity_pool_id, identity_id, providers)` only, which carries no requested session tags — so an `aws:RequestTag/Department` condition is unsatisfiable and BOTH identities are refused with InvalidIdentityPoolConfigurationException, which the retry helper treats as propagation and loops on.
  Why: session-tag-based multi-tenancy (trust policies and downstream policies keyed on aws:RequestTag/aws:PrincipalTag) cannot be exercised at all — the matching tenant is locked out rather than the mismatching one being refused, so the check can neither pass nor report a clean divergence.
  Fix: the vend path's assume — build the condition context from the pool's PrincipalTagAttributeMap-derived requested session tags and expose them as aws:RequestTag/<key> to the trust-policy evaluation, then the mismatch refusal falls out with the modeled error above.
  Traces: cognito-identity.server-side-sts-role-session-tags-enforced-in-trust-policy; services/service-com-amazonaws-cognitoidentity-awscognitoidentityservice/src/ops/get_credentials_for_identity.rs:358-365.

## Evidence
Cause 1 — cognito-identity.vended-credentials-bounded-by-role-policy:
  expected: s3:ListAllMyBuckets refused (HTTP 403 AccessDenied, "User: <assumed-role arn>/CognitoIdentityCredentials is not authorized to perform: s3:ListAllMyBuckets because no identity-based policy allows the s3:ListAllMyBuckets action")
  actual:   AssertionError: s3:ListAllMyBuckets under credentials for a role with no permission policy must be refused, but the call succeeded
Cause 1 — cognito-identity.developer-authenticated-credentials-write-and-read-dynamodb:
  expected: dynamodb:Scan refused (HTTP 400 AccessDeniedException, "... is not authorized to perform: dynamodb:Scan on resource: <table-arn> because no identity-based policy allows the dynamodb:Scan action")
  actual:   AssertionError: expected the call to be refused, but it SUCCEEDED and returned {'Items': [{'pk': {'S': '...'}, 'blob': {...}}], 'Count': 1, 'ScannedCount': 1, 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}

Cause 2 — cognito-identity.get-identity-pool-roles-rules-evaluated-in-declared-order:
  expected: after reordering, the vended session is assumed-role/<role-B>/CognitoIdentityCredentials
  actual:   AssertionError: expected role B to win after it became the first declared rule, got arn:aws:sts::000000000000:assumed-role/sim2real-behav-role-f74c74f57702/CognitoIdentityCredentials (role A, i.e. the pool's authenticated role, every time)

Cause 3 — cognito-identity.server-side-role-selection-ignores-client-supplied-hints-rejected:
  expected: HTTP 400 InvalidParameterException, message "Only SAML providers and providers with RoleMappings support custom role ARN."
  actual:   AssertionError: expected InvalidParameterException, got InvalidIdentityPoolConfigurationException (message "Invalid identity pool configuration. Check assigned IAM roles for this pool.")

Cause 4 — cognito-identity.server-side-sts-role-session-tags-enforced-in-trust-policy:
  expected: matching-tag identity vends credentials (200); mismatching-tag identity refused HTTP 400 InvalidIdentityPoolConfigurationException "Invalid identity pool configuration. Check assigned IAM roles for this pool."
  actual:   Check timed out after 60.0s — the positive-control propagation retry on GetCredentialsForIdentity for the MATCHING identity never succeeds, so no credential is ever vended for a role whose trust policy carries an aws:RequestTag/Department condition.

## Steps to Reproduce
1. Build and boot the emulator, then replay these behavioral checks:
   - cognito-identity.vended-credentials-bounded-by-role-policy
   - cognito-identity.developer-authenticated-credentials-write-and-read-dynamodb
   - cognito-identity.get-identity-pool-roles-rules-evaluated-in-declared-order
   - cognito-identity.server-side-role-selection-ignores-client-supplied-hints-rejected
   - cognito-identity.server-side-sts-role-session-tags-enforced-in-trust-policy
2. Cause 1: create an identity pool with AllowUnauthenticatedIdentities=true, map it to a role whose trust policy admits cognito-identity.amazonaws.com for that pool but which carries NO permission policy; GetId then GetCredentialsForIdentity; call s3:ListBuckets (and, for the developer-identity variant, dynamodb:Scan on a table the role is not granted) with the vended credentials. Both return 200 instead of a refusal.
3. Cause 2: SetIdentityPoolRoles with RoleMappings["<provider>:<clientId>"].Type=Rules and two rules that both always match (aud=<clientId>, token_use=id), role A first; vend and read sts:GetCallerIdentity; then SetIdentityPoolRoles again with role B's rule first and vend again. The assumed-role ARN never changes.
4. Cause 3: pool with only Roles={unauthenticated: roleA} and no RoleMappings; GetCredentialsForIdentity(IdentityId=..., CustomRoleArn=<roleB that independently trusts the same pool>). Observe InvalidIdentityPoolConfigurationException instead of InvalidParameterException.
5. Cause 4: SetPrincipalTagAttributeMap(PrincipalTags={"Department": "custom:dept"}, UseDefaults=false) on a pool fronted by a user-pool provider, map it to a role whose trust policy adds Condition StringEquals aws:RequestTag/Department=eng, then GetCredentialsForIdentity for an identity whose token carries custom:dept=eng. It is refused instead of vending.
