Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sts.deleting-the-role-invalidates-credentials-already-issued-and-recreating-it-by-name-does-not-restore-them sts.a-chained-session-outlasts-the-parent-role-and-expires-later-than-the-parent-session
Issue-Shape-Id: com.amazonaws.sts#AWSSecurityTokenServiceV20110615
Issue-Key: sts.deleting-the-role-invalidates-credentials-already-issued-and-recreating-it-by-name-does-not-restore-them|run#44|AssertionError|assert code_after_delete == "InvalidAccessKeyId", (
Issue-Key: sts.a-chained-session-outlasts-the-parent-role-and-expires-later-than-the-parent-session|_poll#57|Timeout|time.sleep(interval_s)
Issue-Commit: cd64e37d3e3efbbf828b0eac083377c231c88f4f

# REOPENS sts: a session minted from a DELETED role is still refused AccessDenied instead of InvalidAccessKeyId at credential resolution (1 cause)

> _Note: this issue was identified against the code base at commit `cd64e37d3e3efbbf828b0eac083377c231c88f4f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AssumeRole

## Description
REOPENS 20261106T172031Z-sts-5-net-new-behavioral-root-causes-a.md: its fix left the call refused. (The same cause is also claimed by the earlier fixed issue 20261106T081405Z-sts-3-net-new-root-causes-a-session.md.) This is not a new defect: it is Cause 1 of that resolved issue, observed again on this checkout with the identical refusal code, and the replay report marks neither grounding check ALREADY FILED nor OWNED.

ONE cause, grounded in 2 failing checks under the suite heading `### sts` (sts answers 126/158 of its checks, so this is a fidelity finding, not a bootstrap gap).

Cause 1 of 1: a temporary session whose role has since been deleted is still resolved to a live principal and refused at the AUTHORIZATION stage.

AWS: a session's identity is the role's RoleId, not its name (Smithy: com.amazonaws.sts#AssumeRole -> AssumeRoleResponse.Credentials + AssumedRoleUser; AssumeRole declares no error for this and GetCallerIdentity declares no errors at all, so the credential-resolution ordering is recorded behavior only). Once iam:DeleteRole removes the role, the session's AccessKeyId resolves to no principal, so the CALLED service refuses before any policy is consulted: HTTP 403, error code `InvalidAccessKeyId`, message `The AWS Access Key Id you provided does not exist in our records.` — that sentence is entirely FIXED text and carries no request or state values. Recreating the role under the same name with byte-identical trust and inline policies (identical ARN, new RoleId) does not revive the old session, while a session assumed from the recreated role works normally.

Emulator: the call is refused, but with code `AccessDenied` — the authorization-stage verdict — so the dead key is still resolved to a live principal and only the grant is judged missing. Replayed assertion, verbatim: "a session issued from a role that has since been deleted must be rejected with InvalidAccessKeyId -- at credential resolution, before any policy is consulted; got 'AccessDenied'". The second check never reaches an assertion at all: it polls the parent session's S3 GetObject for exactly `InvalidAccessKeyId` after deleting the parent role (3 s interval, 180 s cap) and is cut at the harness's 60 s budget. Hypothesis: that poll is the one that stalls, because the emulator answers the same `AccessDenied` the first check records, and the predicate `code == "InvalidAccessKeyId"` can therefore never be satisfied.

Why: the two codes mean opposite things to a client. `InvalidAccessKeyId` is terminal and tells an SDK credential provider to stop using the credential and refresh; `AccessDenied` reads as a policy problem on a credential still believed valid, so retry/refresh logic keeps a dead (possibly leaked) session in rotation and an operator hunting a permissions bug never learns the role is gone. It also hides the role-identity-is-the-RoleId guarantee, and it blocks every scenario that uses the parent session's death as its barrier (the chained-session check cannot even begin to assert that a chained session outlives its parent role).

Fix: the credential-resolution step shared by the data-plane handlers (the resolve-the-signing-identity path for session credentials) — a session whose role no longer exists, or whose recorded RoleId no longer matches the role now at that ARN, must FAIL resolution, and the called service must serialize HTTP 403 / `InvalidAccessKeyId` / the fixed sentence above ahead of any policy evaluation; the revocation hook belongs on IAM's DeleteRole side of the shared session directory.

Traces: sts.deleting-the-role-invalidates-credentials-already-issued-and-recreating-it-by-name-does-not-restore-them (assertion), sts.a-chained-session-outlasts-the-parent-role-and-expires-later-than-the-parent-session (60 s budget exhausted polling for InvalidAccessKeyId).

## Evidence
Suite simparity-behavioral-aws-v1 replayed against this checkout: 190/268 overall; `sts` 126/158 (32 failed). Both ids below are listed under `### sts` and are marked neither ALREADY FILED nor OWNED.

[mismatch] sts.deleting-the-role-invalidates-credentials-already-issued-and-recreating-it-by-name-does-not-restore-them
  Frame (check_sts_deleting_the_role_...py:222): `assert code_after_delete == "InvalidAccessKeyId", (`
  AssertionError: a session issued from a role that has since been deleted must be rejected with InvalidAccessKeyId -- at credential resolution, before any policy is consulted; got 'AccessDenied'
  The positive control ran FIRST and passed (the session read the object while the role existed), so the credential was live and the grant had propagated; the only thing changed was iam:DeleteRole. The next, unreached assertion requires exactly ('InvalidAccessKeyId', 'The AWS Access Key Id you provided does not exist in our records.'), and the check then requires that recreating the role by name does NOT change that outcome.

[mismatch] sts.a-chained-session-outlasts-the-parent-role-and-expires-later-than-the-parent-session
  Reported detail, verbatim and in full: "Error: Check timed out after 60.0s". No assertion text is recorded.
  What the check does, so it is clear which poll can stall: creates bucket+object, a parent role trusted by the account root and a child role trusted by the parent role ARN; assumes the parent (900 s), proves its GetObject works, chains into the child (3600 s), proves the child's GetObject works and that the child's Expiration lands later than the parent's; deletes the parent ROLE; then polls the parent session's GetObject until its error code is exactly `InvalidAccessKeyId` (interval 3 s, cap 180 s) as the barrier before asserting the child session keeps reading. That barrier is the poll an emulator answering `AccessDenied` can never pass.

## Steps to Reproduce
Build and boot the emulator from this checkout; point sts, iam and s3 clients at it and sign as a principal permitted to call IAM.

1. s3:CreateBucket B and PutObject O. iam:CreateRole R trusted by `arn:aws:iam::<account>:root`, with inline policy Allow s3:GetObject on `arn:aws:s3:::B/*`.
2. sts:AssumeRole R; with the returned credentials confirm GetObject(B,O) succeeds (positive control).
3. Delete R's inline policy, iam:DeleteRole R, and poll iam:GetRole to NoSuchEntity.
4. Repeat GetObject(B,O) with the SAME session credentials. AWS: HTTP 403, code `InvalidAccessKeyId`, message `The AWS Access Key Id you provided does not exist in our records.`. Emulator: code `AccessDenied`.
5. Recreate R by name with identical trust and inline policies; confirm the RoleId differs and the ARN is identical; confirm the OLD session is STILL refused InvalidAccessKeyId, and that a session assumed from the recreated role reads O and carries the NEW RoleId in its UserId.
6. For the second id: build parent/child roles as described in the evidence, chain parent -> child, delete the parent role, and confirm the parent session's GetObject begins failing with `InvalidAccessKeyId` within seconds (the emulator keeps answering `AccessDenied`, so the check's barrier never clears).
7. Re-replay both ids in trace_ids and confirm each completes inside its budget.
