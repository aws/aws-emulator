Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam.condition-key-presence-per-service-through-null-session-policies iam.bucket-policy-grant-to-role-session-or-account-under-a-boundary-or-session-policy iam.deny-not-principal-levels-in-queue-and-bucket-policies iam.trust-policy-principal-of-deleted-role-reads-back-as-unique-id iam.session-token-of-a-deleted-user-against-resource-policies iam.op-contract-resync-mfa-device iam.principal-arn-of-a-role-under-a-path-in-trust-and-resource-conditions iam.session-of-a-deleted-role-against-resource-policies iam.unqualified-operator-on-multivalued-key-never-matches
Issue-Shape-Id: com.amazonaws.iam#AWSIdentityManagementV20100508
Issue-Key: iam.condition-key-presence-per-service-through-null-session-policies|run#50|AssertionError|assert not wrong, text.replace(account, "<account>")
Issue-Key: iam.bucket-policy-grant-to-role-session-or-account-under-a-boundary-or-session-policy|_await_marker#19|AssertionError|raise AssertionError(
Issue-Key: iam.deny-not-principal-levels-in-queue-and-bucket-policies|_await_version#21|AssertionError|raise AssertionError(
Issue-Key: iam.trust-policy-principal-of-deleted-role-reads-back-as-unique-id|run#15|AssertionError|assert shown == trusted_id, (
Issue-Key: iam.session-token-of-a-deleted-user-against-resource-policies|run#46|AssertionError|assert not wrong, "; ".join(wrong).replace(account, "<account>")
Issue-Key: iam.op-contract-resync-mfa-device|_wait_past_totp_window#15|Timeout|time.sleep(1)
Issue-Key: iam.principal-arn-of-a-role-under-a-path-in-trust-and-resource-conditions|_await#27|Timeout|raise AssertionError(f"{what}: not reached within {_WAIT_S}s")
Issue-Key: iam.session-of-a-deleted-role-against-resource-policies|_make_role#37|Timeout|ctx.on_cleanup(_teardown(lambda: iam.delete_role_policy(RoleName=name, PolicyName=_POLICY),
Issue-Key: iam.unqualified-operator-on-multivalued-key-never-matches|_until_allowed#19|TimeoutError|raise TimeoutError("an allowed TagQueue: the run budget ended first; not graded")
Issue-Commit: cc8c6d4514b1c20009096c9d1c08086c7dabca5f

# iam: six policy-evaluation causes — global condition keys absent from the signed request context, account-root and NotPrincipal resource-policy grants never effective, deleted principals not rewritten to unique ids, and four propagation polls that never settle

> _Note: this issue was identified against the code base at commit `cc8c6d4514b1c20009096c9d1c08086c7dabca5f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AssumeRole

## Description
Six net-new root causes in how the emulator evaluates IAM policy against a signed request. Distinct from the already-filed `iam` issues that own other checks in this heading (20261126T141645Z-..., 20261126T150000Z-..., 20261102T091623Z-..., 20261123T114935Z-...).

=== Cause 1 of 6: the signed-request context omits five global condition keys, so a Null-conditioned Deny fires on every call ===
AWS: a direct SigV4 call carries `aws:SourceIp`, `aws:SecureTransport`, `aws:PrincipalOrgID`, `aws:ResourceOrgID` and `aws:ViaAWSService` in its request context, and does NOT carry `aws:CalledVia` (IAM User Guide, "AWS global condition context keys"). A session policy of the form `{Effect: Deny, Action: "*", Resource: "*", Condition: {Null: {<key>: "true"}}}` therefore must NOT match for the five present keys — every probe (`dynamodb:DescribeTable`, `sqs:GetQueueAttributes`, `sqs:ListQueues`, `s3:ListBucket`, `kms:DescribeKey`, `lambda:GetAccountSettings`) succeeds — and must match only for `aws:CalledVia`, where each probe is refused with that service's AccessDenied class and the explicit-session-policy-deny wording.
Emulator: all five present keys behave as ABSENT, so the Null deny matches and every probe is refused, e.g. `('AccessDeniedException', 'User: <role>/sim2real-sourceip is not authorized to perform: dynamodb:DescribeTable on resource: <table> with an explicit deny in a session policy')` and `('AccessDenied', 'User: <role>/sim2real-principalorgid is not authorized to perform: s3:ListBucket on resource: "<bucket>" with an explicit deny in a session policy')`. The control session (no Null deny) passes every probe, so the probes themselves are sound.
Why: any policy that keys on IP, TLS, org membership or service-initiated calls evaluates against an empty context. Guardrails written with `Null`, `StringEquals` or `IpAddress` on these keys either deny everything or allow everything, in both cases disagreeing with AWS.
Fix: the IAM authorization gate's request-context builder — populate `aws:SourceIp`, `aws:SecureTransport`, `aws:PrincipalOrgID`, `aws:ResourceOrgID` and `aws:ViaAWSService` for a directly signed request (and leave `aws:CalledVia` unset), for every service that consults the gate.
Traces: iam.condition-key-presence-per-service-through-null-session-policies.

=== Cause 2 of 6: an S3 bucket policy granting the account-root principal never becomes effective ===
AWS: when the caller's identity-based grant is withheld by a permissions boundary or a session policy, a bucket policy naming the caller's ACCOUNT (`arn:aws:iam::<account>:root`) delegates to the account, so the identity-based policy still has to allow the action; the read is then refused `AccessDenied` with `... because no identity-based policy allows the s3:GetObject action`. When the bucket policy instead names the ROLE, the refusal must name the withholding policy type (`permissions boundary` / `session policy`), and when it names the SESSION the read succeeds. The check polls until the newly put bucket policy is in effect before grading.
Emulator: the `account` form never settles — the poll runs its full 150 s and the last observation is `('AccessDenied', 'OK')`, i.e. the account-principal grant is never reflected at all.
Why: delegating to the account root in a resource policy is the most common cross-principal grant there is. Against the emulator such a bucket policy never takes effect, so a client's reads stay denied no matter how it writes the policy.
Fix: the resource-policy principal matcher — resolve an `arn:aws:iam::<account>:root` principal to any identity in that account, and make a `PutBucketPolicy` observable to the authorization gate.
Traces: iam.bucket-policy-grant-to-role-session-or-account-under-a-boundary-or-session-policy.

=== Cause 3 of 6: a Deny with NotPrincipal in a queue or bucket policy never becomes effective at the account+role level ===
AWS: `NotPrincipal` with `Effect: Deny` in an SQS queue policy or an S3 bucket policy denies everyone EXCEPT the listed principals, and the exception list is matched at each principal level (account root, role ARN, assumed-role session ARN). With the account AND the role both excepted, the role's own calls must pass on both services.
Emulator: that version of the policy never comes into effect — the poll exhausts 180 s and the last observation is `('OK', 'OK', 'AccessDenied', 'OK')`, i.e. one of the four graded calls stays denied under a policy whose `NotPrincipal` excepts it.
Why: `NotPrincipal`-based denies are how accounts write "deny everyone but us" resource policies. Against the emulator such a policy locks out a principal it explicitly excepts.
Fix: the resource-policy evaluator's `NotPrincipal` matching — compare the caller at every principal level (account root, role ARN, assumed-role session ARN) before treating the Deny as applying.
Traces: iam.deny-not-principal-levels-in-queue-and-bucket-policies.

=== Cause 4 of 6: a deleted principal is not rewritten to its unique id, and a deleted user's session token still authorizes ===
AWS: when the entity a policy's `Principal` names is deleted, IAM rewrites the stored principal to the entity's unique id — a role to its `RoleId` (`AROA...`), a user to its `UserId` (`AIDA...`) — and `GetRole`'s `AssumeRolePolicyDocument` (and a resource policy's document) read back that unique id, not the old ARN. Separately, the credentials of a DELETED user no longer authorize: a call made with that user's session token is refused, not served.
Emulator: (a) after deleting the trusted role, the trust policy's principal still reads back as `arn:aws:iam::<account>:role/<role-name>` instead of its `RoleId`; (b) a deleted user's session token still serves calls — the `identity` and `send_user_grant` legs answer `('OK', None)` where a refusal is required, and the stored principal reads back as the `user-arn` form rather than the unique id.
Why: the unique-id rewrite is what makes a policy safe against name reuse — recreating an entity with the same name must NOT re-grant it. Against the emulator it silently does. And a credential belonging to a deleted user keeps working, so deleting a user does not revoke anything.
Fix: the IAM entity-delete path (rewrite every stored `Principal` naming the entity to its unique id, in trust policies and in resource policies) and the credential-validation step that must stop honouring a session whose underlying user no longer exists.
Traces: iam.trust-policy-principal-of-deleted-role-reads-back-as-unique-id, iam.session-token-of-a-deleted-user-against-resource-policies.

=== Cause 5 of 6: ResyncMFADevice never settles — the op-contract check exhausts its budget ===
AWS: `ResyncMFADevice` takes two consecutive TOTP codes for the device and either resynchronises it (200) or is refused `InvalidAuthenticationCode` with `Authentication code for device is not valid.` (Smithy: iam `ResyncMFADevice`, errors `InvalidAuthenticationCodeException`, `NoSuchEntityException`, `LimitExceededException`, `ConcurrentModificationException`). Either way the call answers promptly; the check grades the happy path and the bad-code refusal.
Emulator: the check never reaches a verdict — it ends `Check timed out after 60.0s`, so neither the resync nor the documented refusal is ever served.
Why: a client cannot resynchronise an MFA device at all, and gets no error class to react to — just a call that does not complete.
Fix: the `ResyncMFADevice` handler and the virtual-MFA TOTP window it validates against.
Traces: iam.op-contract-resync-mfa-device.

=== Cause 6 of 6: three policy/trust changes never become effective, so the propagation polls never settle ===
AWS: a trust-policy or identity-policy change becomes effective within seconds, and each of these checks polls only to absorb that propagation delay before grading. (a) A trust policy naming a role under a PATH as the principal admits `AssumeRole` for that role's sessions; (b) a session minted from a role that is then DELETED stops being honoured against resource policies, with the refusal classes the check enumerates; (c) a policy that allows `sqs:TagQueue` makes `TagQueue` succeed.
Emulator: none of the three ever reaches the state the check waits for. (a) and (b) end `Check timed out after 60.0s`; (c) ends `TimeoutError: an allowed TagQueue: the run budget ended first; not graded` — the `sqs:TagQueue` the policy allows never becomes allowed.
Why: a client cannot rely on a policy or trust-policy change ever taking effect: the grant it wrote is never observed by the authorization gate, so the call it enabled stays refused indefinitely.
Fix: the propagation path between the IAM policy/trust store and the authorization gate — a committed `PutRolePolicy` / `UpdateAssumeRolePolicy` / `DeleteRole` must be visible to the next authorization decision (and to the per-service gates, here SQS's).
Traces: iam.principal-arn-of-a-role-under-a-path-in-trust-and-resource-conditions, iam.session-of-a-deleted-role-against-resource-policies, iam.unqualified-operator-on-multivalued-key-never-matches.

## Evidence
From one replay of the behavioral suite (service heading `### iam`).

[mismatch] iam.condition-key-presence-per-service-through-null-session-policies
  AssertionError: ('aws:SourceIp', 'dynamodb:DescribeTable'): got ('AccessDeniedException', 'User: <role>/sim2real-sourceip is not authorized to perform: dynamodb:DescribeTable on resource: <table> with an explicit deny in a session policy'); ('aws:SecureTransport', 'dynamodb:DescribeTable'): got (same shape); ('aws:PrincipalOrgID', 'dynamodb:DescribeTable'): got (same shape); ('aws:PrincipalOrgID', 's3:ListBucket'): got ('AccessDenied', '... with an explicit deny in a session policy'); ('aws:ResourceOrgID', ...), ('aws:ViaAWSService', ...) likewise
  Expected ('OK', None) for every probe under all five of those keys; only aws:CalledVia may deny.

[mismatch] iam.bucket-policy-grant-to-role-session-or-account-under-a-boundary-or-session-policy
  AssertionError: bucket policy for 'account' not in effect within 150.0s; last ('AccessDenied', 'OK')

[mismatch] iam.deny-not-principal-levels-in-queue-and-bucket-policies
  AssertionError: policy version 'account+role' not in effect within 180.0s; last ('OK', 'OK', 'AccessDenied', 'OK')

[mismatch] iam.trust-policy-principal-of-deleted-role-reads-back-as-unique-id
  AssertionError: after deleting the trusted role the principal must read back as its RoleId 'AROA...' , got 'arn:aws:iam::<account>:role/<role-name>'
  (the RoleId and role name are per-run; the requirement is the AROA-class unique id in place of the ARN)

[mismatch] iam.session-token-of-a-deleted-user-against-resource-policies
  AssertionError: ('deleted', 'identity'): got ('OK', None); ('deleted', 'send_user_grant'): got ('OK', None); ('deleted', 'read_identity_grant'): got ('AccessDenied', 'User: arn:aws:iam::<account>:user/<user> is not authorized to perform: sqs:getqueueattributes on resource: arn:aws:sqs:<region>:<account>:<queue> because no identity-based policy allows the sqs:getqueueattributes action'); stored principals 'user-arn'

[mismatch] iam.op-contract-resync-mfa-device
  Error: Check timed out after 60.0s

[mismatch] iam.principal-arn-of-a-role-under-a-path-in-trust-and-resource-conditions
  Error: Check timed out after 60.0s
[mismatch] iam.session-of-a-deleted-role-against-resource-policies
  Error: Check timed out after 60.0s
[error] iam.unqualified-operator-on-multivalued-key-never-matches
  TimeoutError: an allowed TagQueue: the run budget ended first; not graded  (raised from `_until_allowed`, which polls for the sqs:TagQueue the attached policy allows)

## Steps to Reproduce
1. Cause 1: CreateRole, AssumeRole with a session policy `[{Allow *}, {Deny *, Condition: {Null: {<key>: "true"}}}]` for each of aws:SourceIp, aws:SecureTransport, aws:PrincipalOrgID, aws:ResourceOrgID, aws:ViaAWSService and aws:CalledVia. With that session call dynamodb:DescribeTable, sqs:GetQueueAttributes, sqs:ListQueues, s3:ListBucket, kms:DescribeKey and lambda:GetAccountSettings. Only the aws:CalledVia session may be denied; observed every one of the five is denied with the explicit-session-policy-deny message.
2. Cause 2: put an identity policy on a role allowing s3:GetObject but withhold it with a permissions boundary or a session policy; PutBucketPolicy naming `arn:aws:iam::<account>:root` as Principal; GetObject as that session and poll — the outcome must settle (AccessDenied naming the identity-based policy); observed the account form never settles within 150 s.
3. Cause 3: put an SQS queue policy and an S3 bucket policy each with `{Effect: Deny, NotPrincipal: {AWS: [<account root>, <role arn>]}, Action: *, Resource: *}`; call both services as that role; all four graded calls must pass; observed one stays AccessDenied for 180 s.
4. Cause 4: (a) CreateRole T, CreateRole B whose trust policy names T's ARN, DeleteRole T, then GetRole B and read `AssumeRolePolicyDocument`'s Principal — it must be T's RoleId; observed T's ARN. (b) CreateUser U, GetSessionToken as U, DeleteUser U, then call SQS with that session token — it must be refused; observed 200.
5. Cause 5: CreateVirtualMFADevice + EnableMFADevice, then ResyncMFADevice with two consecutive TOTP codes and with a bad code — both must answer promptly (200 / InvalidAuthenticationCode 'Authentication code for device is not valid.'); observed the check never completes in 60 s.
6. Cause 6: (a) CreateRole under `/sim2real/behav/` and a second role whose trust policy names it, then AssumeRole and poll; (b) AssumeRole a role, DeleteRole it, then use the session against a resource policy and poll; (c) attach a policy allowing sqs:TagQueue and poll TagQueue until allowed. All three must settle within seconds; observed none does.
