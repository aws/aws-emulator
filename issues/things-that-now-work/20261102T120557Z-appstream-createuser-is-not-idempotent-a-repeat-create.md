Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appstream.concurrent-user-creation-duplicate-name
Issue-Shape-Id: com.amazonaws.appstream#PhotonAdminProxyService
Issue-Key: appstream.concurrent-user-creation-duplicate-name|run#9|AssertionError|assert len(successes) == racers, f"expected all {racers} racers to succeed, got: {results}"
Issue-Commit: ef6e0f5e66c899ede0df1e04050c78d39daac904

# appstream CreateUser is not idempotent: a repeat create of the same UserName+AuthenticationType is refused ResourceAlreadyExistsException (1 cause)

> _Note: this issue was identified against the code base at commit `ef6e0f5e66c899ede0df1e04050c78d39daac904`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateUser

## Description
One net-new root cause in the appstream CreateUser handler.

Cause 1 of 1: CreateUser refuses a repeat create of an identity that already exists with exactly the same UserName and AuthenticationType, instead of answering success.
AWS: recorded behavior only — Smithy (services/service-com-amazonaws-appstream-photonadminproxyservice/smithy-model/com-amazonaws-appstream-photonadminproxyservice.json) models CreateUser with input {UserName (required, Username, @length 1..128, sensitive), AuthenticationType (required enum), MessageAction (enum), FirstName, LastName}, an EMPTY output shape (CreateUserResult has no members), and the error set {InvalidAccountStatusException, InvalidParameterCombinationException, LimitExceededException, OperationNotPermittedException, ResourceAlreadyExistsException} — the model says nothing about re-create semantics, and its documentation is only "Creates a new user in the user pool." The recording shows AWS treats CreateUser as idempotent for an IDENTICAL identity: N concurrent CreateUser calls on one (UserName, AuthenticationType) pair all return HTTP 200 with an empty body, and exactly one enabled user exists for that UserName afterwards (DescribeUsers on that AuthenticationType returns one entry with Enabled=true). ResourceAlreadyExistsException is reserved for a conflicting create, not an identical one.
Emulator: exactly one of six concurrent identical CreateUser calls succeeds; the other five are refused with error code 'ResourceAlreadyExistsException' (replay detail: "expected all 6 racers to succeed, got: [('ok', None), ('error', 'ResourceAlreadyExistsException'), ('error', 'ResourceAlreadyExistsException'), ('error', 'ResourceAlreadyExistsException'), ('error', 'ResourceAlreadyExistsException'), ('error', 'ResourceAlreadyExistsException')]"). The handler treats any existing record for the name as a conflict rather than comparing the requested identity against the stored one.
Why: client code that provisions users declaratively (a reconcile loop, a retried onboarding job, or several workers that may each create the same user) is written against the idempotent contract and does not catch ResourceAlreadyExistsException for its own identical request. On the emulator every such retry or concurrent duplicate fails the caller even though the desired state is already in place, so provisioning that succeeds on AWS aborts here; worse, a client that DOES swallow the error cannot distinguish "already as requested" from a genuine identity conflict.
Fix: the CreateUser handler and its user-pool state in services/service-com-amazonaws-appstream-photonadminproxyservice — on an existing (UserName, AuthenticationType) whose stored attributes match the request, return the empty success result instead of raising ResourceAlreadyExistsException, and keep the existence check + insert atomic so concurrent identical creates all succeed and allocate the identity once.
Traces: appstream.concurrent-user-creation-duplicate-name.

## Evidence
Behavioral suite simparity-behavioral-aws-v1 replayed against the booted emulator; per-service rate appstream 7/24 (29%).

Check appstream.concurrent-user-creation-duplicate-name [mismatch]:
  6 concurrent appstream.create_user(UserName='<unique>@example.com', AuthenticationType='USERPOOL', MessageAction='SUPPRESS')
  expected: all 6 calls succeed (HTTP 200, empty CreateUserResult), then DescribeUsers(AuthenticationType='USERPOOL') reports exactly one entry for that UserName with Enabled=true
  got:      AssertionError: expected all 6 racers to succeed, got: [('ok', None), ('error', 'ResourceAlreadyExistsException'), ('error', 'ResourceAlreadyExistsException'), ('error', 'ResourceAlreadyExistsException'), ('error', 'ResourceAlreadyExistsException'), ('error', 'ResourceAlreadyExistsException')]

The user name in the recording is a per-run unique value and is not part of the requirement; the requirement is that any repeat of an identical (UserName, AuthenticationType) succeeds.

Smithy evidence: CreateUser input UserName (required) + AuthenticationType (required) + MessageAction; output CreateUserResult = {} (no members); modeled errors include ResourceAlreadyExistsException (httpError 400). No idempotency trait and no documentation of the re-create outcome — the idempotent outcome is grounded in the recording.

## Steps to Reproduce
1. git clone the project repo and boot the emulator built from it.
2. Run the behavioral suite (simparity-behavioral-aws-v1).
3. Replay check appstream.concurrent-user-creation-duplicate-name.
4. Equivalently by hand: call appstream CreateUser twice with the identical UserName and AuthenticationType='USERPOOL' (MessageAction='SUPPRESS'). Observe the second call is refused HTTP 400 ResourceAlreadyExistsException instead of returning an empty 200, and that firing six such calls concurrently yields one success and five refusals.
