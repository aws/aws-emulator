Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-idp__lambda__sqs.pre-authentication-sees-user-not-found-for-an-unknown-user-under-existence-error-prevention
Issue-Key: cognito-idp__lambda__sqs.pre-authentication-sees-user-not-found-for-an-unknown-user-under-existence-error-prevention|_drain_until_both_present#26|AssertionError|assert not missing, f"no PreAuthentication event arrived for {missing!r} within {deadline_s}s; seen {seen!r}"
Issue-Commit: 388bf0aeef5b87cdb8b8e5fe5948d049b57af517

# The PreAuthentication Lambda trigger is not invoked for an unknown user when PreventUserExistenceErrors is enabled

> _Note: this issue was identified against the code base at commit `388bf0aeef5b87cdb8b8e5fe5948d049b57af517`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
InitiateAuth

## Description
One cause.

Cause 1 of 1: authenticating a user that does not exist against a user pool client with PreventUserExistenceErrors ENABLED does not fire the pool's PreAuthentication trigger.

AWS: recorded behavior only for the trigger payload (the Smithy model describes the APIs, not the trigger contract). With `PreventUserExistenceErrors: ENABLED`, Cognito does not reveal non-existence to the caller, but it still invokes the pool's PreAuthentication Lambda trigger for the attempted username, with the request carrying the user-not-found indication, so the trigger sees both the known and the unknown username. The known-user leg fires the trigger too.

Emulator: no PreAuthentication event arrives for the unknown username within the check's 45 s window; only the known username was seen at all (`seen {'<known-user>': False}`), so the trigger is skipped for the non-existent user.

Why: a client that uses PreAuthentication to count or throttle failed sign-in attempts, to audit credential-stuffing against unknown usernames, or to gate access centrally loses every event for non-existent users — exactly the case PreventUserExistenceErrors exists to make indistinguishable from the outside.

Fix: the InitiateAuth/AdminInitiateAuth handler's PreAuthentication trigger invocation, which must run before the user-existence decision when PreventUserExistenceErrors is ENABLED.

Traces: cognito-idp__lambda__sqs.pre-authentication-sees-user-not-found-for-an-unknown-user-under-existence-error-prevention.

## Evidence
Expected: a PreAuthentication trigger invocation for the unknown username (and for the known one).
Actual: `AssertionError: no PreAuthentication event arrived for ['sim2real-xsvc-preauth-unknown-f503f18adb95'] within 45.0s; seen {'sim2real-xsvc-preauth-known-ff011f5d6eff': False}`. The usernames are per-run values; the requirement is that both attempted usernames produce an invocation.

## Steps to Reproduce
1. Create a user pool with a PreAuthentication Lambda trigger that records the event, and an app client with PreventUserExistenceErrors=ENABLED.
2. Create one user; leave a second username unregistered.
3. Call InitiateAuth for each username and inspect the trigger's recorded events — no event is recorded for the unregistered username.
Replay: cognito-idp__lambda__sqs.pre-authentication-sees-user-not-found-for-an-unknown-user-under-existence-error-prevention.
