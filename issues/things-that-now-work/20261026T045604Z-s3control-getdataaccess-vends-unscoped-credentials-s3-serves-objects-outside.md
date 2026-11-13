Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3control.access-grants-get-data-access-returns-scoped-credentials
Issue-Shape-Id: com.amazonaws.s3control#AWSS3ControlServiceV20180820
Issue-Key: s3control.access-grants-get-data-access-returns-scoped-credentials|_error#28|AssertionError|raise AssertionError("call unexpectedly succeeded; an error was expected")
Issue-Commit: 7a0bc98906b5e3a7c6d3ce3f998a75b2f2365c60

# GetDataAccess vends unscoped credentials: S3 serves objects outside the granted prefix

> _Note: this issue was identified against the code base at commit `7a0bc98906b5e3a7c6d3ce3f998a75b2f2365c60`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetDataAccess

## Description
Cause 1 of 1: the temporary credentials returned by S3 Access Grants `GetDataAccess` are not scoped to the grant that was matched. The emulator mints a session that behaves like the calling identity, so the S3 data plane serves ANY object in the bucket to it, not just objects under the granted prefix.

AWS: `GetDataAccess` (Smithy: s3control `GetDataAccess`, GET /v20180820/accessgrantsinstance/dataaccess, input `Target` (S3Prefix, required), `Permission` (required), `Privilege`, `DurationSeconds` @range 900..43200; output `Credentials` (sensitive) + `MatchedGrantTarget` + `Grantee`) vends an STS session whose effective permissions are the INTERSECTION of the location's IAM role and a session policy derived from the matched grant's scope and permission. The model itself does not describe the session-policy derivation (`errors: []` on the operation), so this is documented/recorded behavior only: on real AWS, using those credentials to `GetObject` a key OUTSIDE the matched grant scope is refused with HTTP 403 and error code `AccessDenied`, message

  User: <vended-session-arn> is not authorized to perform: s3:GetObject on resource: "arn:aws:s3:::<bucket>/<key>" because no session policy allows the s3:GetObject action

Fixed text of that message: `User: `, ` is not authorized to perform: `, ` on resource: `, ` because no session policy allows the `, ` action`. Variable parts taken from the request and state: the vended session's ARN (the identity `sts:GetCallerIdentity` reports for those very credentials — Access Grants mints a new session per `GetDataAccess` call, so it must never be a pinned/recorded value), the action (`s3:GetObject` here, the action actually attempted), and the resource ARN built from the requested bucket and key. A read of a key INSIDE the matched grant scope succeeds and returns the object's exact bytes.

Emulator: the in-grant read succeeds (correct), but the out-of-grant read also succeeds. The check's `_error()` helper, which expects a `ClientError`, raised `AssertionError: call unexpectedly succeeded; an error was expected` at the `scoped.get_object(Bucket=bucket, Key="ungranted/outside.txt")` call — i.e. the emulator returned 200 with the object body where AWS returns 403 `AccessDenied`. Everything up to that point matched: the instance/location/grant chain was built, `CreateAccessGrant` reported `GrantScope == s3://<bucket>/granted/*`, `GetDataAccess` reported the correct `MatchedGrantTarget` and a `Credentials` struct carrying `AccessKeyId`, `SecretAccessKey`, `SessionToken` and `Expiration`. So the envelope is right and only the credentials' AUTHORITY is wrong — they carry no grant-derived restriction at all.

Because the run stopped at that assertion, the later arms of the same chain were never exercised and may hide further divergence behind this one cause; they are part of closing it: `GetDataAccess` with `Permission=WRITE` against a READ-only grant must be refused `AccessDenied` with message `You do not have WRITE permissions to the requested S3 Prefix: s3://<bucket>/<prefix>` (fixed text `You do not have `, ` permissions to the requested S3 Prefix: `; variables = the requested permission and the requested target prefix); after `DeleteAccessGrant`, `GetDataAccess` for the same target must be refused `AccessDenied` with the same template for the requested permission; and `GetAccessGrant` on the deleted id must be refused `NoSuchAccessGrantError` with the fixed message `The specified access grant does not exist`.

Why: a client relying on AWS's behavior treats a `GetDataAccess` session as a least-privilege credential it can hand to a less-trusted consumer (a job, a tenant, a downstream process) in the confidence that S3 itself will refuse anything outside the granted prefix. Against this emulator those credentials are effectively account-wide for the bucket, so any test or policy-regression suite that asserts cross-tenant or cross-prefix isolation passes here and fails on AWS — the exact class of bug the emulator is meant to catch. Code paths that expect the 403 (fallback logic, error-handling branches, audit assertions) are never taken.

Fix: belongs in the Access Grants credential-vending path of the s3control service — `GetDataAccess`'s handler must derive a session policy from the matched grant (its `GrantScope`/location scope and `Permission`, narrowed by `Privilege` and `TargetType`) and record it on the minted session in shared credential/session state; and in the S3 data-plane authorizer, which must evaluate that session policy for `GetObject` (and the other object actions) and emit the 403 `AccessDenied` with the message template above. The refusal arms of `GetDataAccess` itself (permission mismatch, revoked grant) belong in that same handler's validation, and the `NoSuchAccessGrantError` arm in `GetAccessGrant`'s lookup.

Traces: s3control.access-grants-get-data-access-returns-scoped-credentials (mismatch). No existing open issue covers Access Grants credential scoping — `list_existing_issues(query='access-grants')` returns nothing, and the s3control issues already open concern public-access-block enforcement, access points/directory buckets, generic TagResource, and unimplemented `UpdateAccessGrantsLocation`/`ListJobs`/`DeleteStorageLensConfigurationTagging`.

## Evidence
Check: s3control.access-grants-get-data-access-returns-scoped-credentials — verdict mismatch.

Expected (real AWS): with credentials from GetDataAccess(Target="s3://<bucket>/granted/*", Permission="READ"):
  - GetObject(Bucket=<bucket>, Key="granted/inside.txt") -> 200, body = the object's bytes
  - GetObject(Bucket=<bucket>, Key="ungranted/outside.txt") -> HTTP 403, Error.Code = "AccessDenied", Error.Message = 'User: <vended-session-arn> is not authorized to perform: s3:GetObject on resource: "arn:aws:s3:::<bucket>/ungranted/outside.txt" because no session policy allows the s3:GetObject action'

Actual (emulator):
  - the in-grant GetObject succeeded (matches)
  - the out-of-grant GetObject ALSO succeeded — no ClientError raised. Replay output:
      File "check_s3control_access_grants_get_data_access_returns_scoped_credentials.py", line 85, in run
        outside_code, outside_msg = _error(lambda: scoped.get_object(Bucket=bucket, Key="ungranted/outside.txt"))
      File "...", line 156, in _error
        raise AssertionError("call unexpectedly succeeded; an error was expected")
      AssertionError: call unexpectedly succeeded; an error was expected

Already-correct parts of the same response (so the defect is isolated to credential authority): CreateAccessGrant.GrantScope == "s3://<bucket>/granted/*"; GetDataAccess.MatchedGrantTarget == "s3://<bucket>/granted/*"; GetDataAccess.Credentials contains AccessKeyId, SecretAccessKey, SessionToken, Expiration.

Unreached-but-required arms (blocked by this same cause, assert after the failing line): GetDataAccess(Permission="WRITE") on a READ grant -> AccessDenied / "You do not have WRITE permissions to the requested S3 Prefix: s3://<bucket>/granted/*"; GetDataAccess(READ) after DeleteAccessGrant -> AccessDenied / "You do not have READ permissions to the requested S3 Prefix: s3://<bucket>/granted/*"; GetAccessGrant(deleted id) -> NoSuchAccessGrantError / "The specified access grant does not exist".

Smithy cited: s3control GetDataAccess — GET /v20180820/accessgrantsinstance/dataaccess, hostPrefix {AccountId}.; input AccountId (required, pattern ^\d{12}$), Target (S3Prefix, required, length 1..2000), Permission (enum, required), DurationSeconds (range 900..43200), Privilege, TargetType, AuditContext; output Credentials (sensitive), MatchedGrantTarget, Grantee; no modeled errors (refusal codes are recorded behavior).

## Steps to Reproduce
1. Create a general-purpose bucket and put two objects in it: key "granted/inside.txt" and key "ungranted/outside.txt".
2. Create an IAM role trusted by access-grants.s3.amazonaws.com (sts:AssumeRole, sts:SetSourceIdentity, sts:SetContext) with an inline policy allowing s3:GetObject and s3:ListBucket on the bucket and its objects.
3. s3control CreateAccessGrantsInstance for the account (skip if one exists).
4. s3control CreateAccessGrantsLocation with LocationScope="s3://<bucket>/granted/" and IAMRoleArn=<role arn>.
5. s3control CreateAccessGrant with that location id, AccessGrantsLocationConfiguration={"S3SubPrefix": "*"}, Grantee={GranteeType: IAM, GranteeIdentifier: <caller arn>}, Permission="READ". Confirm GrantScope == "s3://<bucket>/granted/*".
6. s3control GetDataAccess(AccountId=<account>, Target="s3://<bucket>/granted/*", Permission="READ"). Build an S3 client from the returned Credentials (AccessKeyId / SecretAccessKey / SessionToken).
7. With that scoped client, GetObject(Key="granted/inside.txt") — succeeds on both AWS and the emulator.
8. With the SAME scoped client, GetObject(Key="ungranted/outside.txt"). AWS: HTTP 403 AccessDenied with the "...because no session policy allows the s3:GetObject action" message naming the vended session ARN (read it via sts:GetCallerIdentity with the same credentials). Emulator: returns 200 and the object body — the divergence.
9. Then exercise the blocked arms: GetDataAccess(Permission="WRITE") on the READ grant; DeleteAccessGrant followed by GetDataAccess(READ); GetAccessGrant on the deleted grant id.
