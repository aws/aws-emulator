Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appstream.user-enable-disable-state-transition
Issue-Shape-Id: com.amazonaws.appstream#PhotonAdminProxyService
Issue-Key: appstream.user-enable-disable-state-transition|_error#11|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Commit: 76feef4fce17d61062306bbcbfb87a5dd013a84e

# appstream CreateUser accepts a duplicate UserName+AuthenticationType with HTTP 200 instead of refusing it ResourceAlreadyExistsException (1 cause)

> _Note: this issue was identified against the code base at commit `76feef4fce17d61062306bbcbfb87a5dd013a84e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateUser

## Description
One net-new root cause in the appstream CreateUser handler.

Cause 1 of 1: a sequential repeat CreateUser for a UserName+AuthenticationType that already exists in the user pool is accepted with an empty HTTP 200 instead of being refused ResourceAlreadyExistsException.
AWS: Smithy (services/service-com-amazonaws-appstream-photonadminproxyservice/smithy-model/com-amazonaws-appstream-photonadminproxyservice.json) models CreateUser with input {UserName (required, @length 1..128, pattern ^[\p{L}\p{M}\p{S}\p{N}\p{P}]+$, sensitive), AuthenticationType (required enum), MessageAction (enum), FirstName, LastName}, an EMPTY output (CreateUserResult has no members) and the error set {InvalidAccountStatusException, InvalidParameterCombinationException, LimitExceededException, OperationNotPermittedException, ResourceAlreadyExistsException}, every one httpError 400; ResourceAlreadyExistsException is documented only as "The specified resource already exists." The recorded behavior supplies the rule: creating a user that is already present in the pool for that AuthenticationType is REFUSED with HTTP 400, error code ResourceAlreadyExistsException and the message "A user with that username already exists." That message is entirely fixed text for this error class — it carries no value from the request or from state (the requested UserName is NOT interpolated into it), so it must be reproduced verbatim.
Emulator: the second create_user(UserName=<same>, AuthenticationType='USERPOOL', MessageAction='SUPPRESS') returns HTTP 200 with an empty body (replay detail: "expected the call to be rejected, but it SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 200, ... 'content-length': '2'}}"), i.e. no refusal at all: the handler does not treat an already-present (UserName, AuthenticationType) as a conflict on the sequential path. Note the open issue 20261102T120557Z-appstream-createuser-is-not-idempotent-a-repeat-create.md asks for the opposite on the CONCURRENT identical-create path (6 racers all 200, one user afterwards); it does NOT cover this check, and whatever reconciliation is chosen must keep the sequential duplicate refused as above while leaving that concurrent case's outcome to that issue.
Why: a client that provisions AppStream users treats the ResourceAlreadyExistsException as the signal that the name is taken — onboarding flows branch on it to pick a different name, to surface "user exists" to an operator, or to stop before overwriting attributes. On the emulator the conflicting create reports success, so the client believes it created a distinct user, never takes its exists-branch, and silently diverges from the pool's real contents; the whole duplicate-detection class of the API is unreachable here.
Fix: the CreateUser handler and its user-pool state in services/service-com-amazonaws-appstream-photonadminproxyservice — look the requested (UserName, AuthenticationType) up in the pool and raise ResourceAlreadyExistsException (HTTP 400) with the fixed message above when it is present, rather than returning the empty success result.
Traces: appstream.user-enable-disable-state-transition.

## Evidence
Behavioral suite simparity-behavioral-aws-v1 replayed against the booted emulator; per-service rate appstream 18/24 (75%), 6 failed (the other five failures are ALREADY FILED / OWNED by existing issues).

Check appstream.user-enable-disable-state-transition [mismatch], at the duplicate-create step:
  1. appstream.create_user(UserName='<unique>@example.com', AuthenticationType='USERPOOL', MessageAction='SUPPRESS')  -> 200, and DescribeUsers shows the user with Enabled=true (this part passes)
  2. the identical appstream.create_user(UserName=<same>, AuthenticationType='USERPOOL', MessageAction='SUPPRESS') again
  expected: HTTP 400, Error.Code='ResourceAlreadyExistsException', Error.Message='A user with that username already exists.'
  got:      HTTP 200 with an empty CreateUserResult —
            AssertionError: expected the call to be rejected, but it SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 200, 'HTTPHeaders': {'content-type': 'application/x-amz-json-1.1', 'content-length': '2', ...}, 'RetryAttempts': 0}}

The user name in the recording is a per-run unique value and is not part of the requirement; the requirement is that a create for any UserName already present under that AuthenticationType is refused. The expected message is fixed text for this error class and contains no per-run value.

Smithy evidence: CreateUser errors include ResourceAlreadyExistsException (errorKind client, httpError 400, single member Message); output CreateUserResult = {} (no members), so the success and the refusal are distinguishable only by status/code.

## Steps to Reproduce
1. git clone the project repo and boot the emulator built from it.
2. Run the behavioral suite (simparity-behavioral-aws-v1).
3. Replay check appstream.user-enable-disable-state-transition.
4. Equivalently by hand: call appstream CreateUser(UserName='someone@example.com', AuthenticationType='USERPOOL', MessageAction='SUPPRESS'); confirm DescribeUsers(AuthenticationType='USERPOOL') lists it with Enabled=true; then issue the identical CreateUser a second time. Observe HTTP 200 with an empty body instead of HTTP 400 ResourceAlreadyExistsException 'A user with that username already exists.'
