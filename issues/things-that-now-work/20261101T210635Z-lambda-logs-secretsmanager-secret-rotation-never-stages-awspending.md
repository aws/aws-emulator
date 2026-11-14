Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__logs__secretsmanager.deep-secrets-and-params-rotation-token-is-staged-awspending-at-every-step-before-awscurrent-moves lambda__logs__secretsmanager.concurrent-reader-during-rotation-never-observes-a-broken-current-credential
Issue-Key: lambda__logs__secretsmanager.deep-secrets-and-params-rotation-token-is-staged-awspending-at-every-step-before-awscurrent-moves|_poll_until#18|Timeout|last = probe()
Issue-Key: lambda__logs__secretsmanager.concurrent-reader-during-rotation-never-observes-a-broken-current-credential|_poll_until#22|Timeout|last = probe()
Issue-Commit: 62e4c5cd3531cb5ea16934ee4e8b2e43c8dd65a3

# lambda__logs__secretsmanager: secret rotation never stages AWSPENDING before moving AWSCURRENT, so neither the staging ladder nor the concurrent-reader guarantee is observable

> _Note: this issue was identified against the code base at commit `62e4c5cd3531cb5ea16934ee4e8b2e43c8dd65a3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RotateSecret

## Description
Two distinct root causes in the Secrets Manager rotation path. Both end in `Error: Check timed out after 60.0s`.

Cause 1 of 2 — version-stage ladder during rotation: AWS: recorded behavior only (the Secrets Manager model defines RotateSecret and the version-stage labels but not the per-step staging order). AWS behaviour exercised by the check: at every step of a rotation (createSecret, setSecret, testSecret, finishSecret) the new secret version carries the AWSPENDING stage BEFORE AWSCURRENT moves to it; only at finishSecret does AWSCURRENT move and AWSPENDING get removed, and the rotation token is the same across the four steps.
Emulator: the rotation never reaches the states the check polls for — the check is killed at the 60s budget, so the staging order is never observable.
Failing check: lambda__logs__secretsmanager.deep-secrets-and-params-rotation-token-is-staged-awspending-at-every-step-before-awscurrent-moves.

Cause 2 of 2 — concurrent reader during rotation: AWS: recorded behavior only. A reader calling GetSecretValue (AWSCURRENT) concurrently with a rotation never observes a broken or half-written credential: every read returns a complete, usable value — either the old AWSCURRENT or, after finishSecret, the new one.
Emulator: the rotation never completes, so the concurrent-reader invariant can never be exercised; the check times out at 60s.
Failing check: lambda__logs__secretsmanager.concurrent-reader-during-rotation-never-observes-a-broken-current-credential.

Why: rotation is the entire point of a managed secret. With the staging ladder unobservable and rotation never finishing, a client cannot tell whether a rotation is in progress, completed, or stuck; a rotation Lambda written against the documented four-step protocol cannot be validated; and applications cannot establish that readers are never handed a torn credential.

Fix: the RotateSecret orchestration — invoking the rotation function for each step with a stable ClientRequestToken, applying AWSPENDING to the new version at createSecret, and moving AWSCURRENT (removing AWSPENDING) only at finishSecret — together with the GetSecretValue read path that must keep serving a complete AWSCURRENT value throughout.

Traces: the two check ids listed above.

## Evidence
Both failures report `Error: Check timed out after 60.0s` (replay verdicts [mismatch]).
- deep-secrets-and-params-rotation-token-is-staged-awspending-at-every-step-before-awscurrent-moves: expected the new version to hold AWSPENDING at createSecret/setSecret/testSecret and AWSCURRENT only after finishSecret, with one rotation token throughout; actual: the states never materialise within 60s.
- concurrent-reader-during-rotation-never-observes-a-broken-current-credential: expected every concurrent GetSecretValue(AWSCURRENT) to return a complete credential; actual: rotation never completes, so no verdict within 60s.

## Steps to Reproduce
1. Create a secret and a rotation Lambda implementing createSecret/setSecret/testSecret/finishSecret.
2. Call RotateSecret and, at each step, DescribeSecret/ListSecretVersionIds to observe VersionStages on the new version (AWSPENDING before AWSCURRENT moves) and the ClientRequestToken passed to each step.
3. In parallel, call GetSecretValue with VersionStage=AWSCURRENT repeatedly and assert every value is complete.
4. Against the emulator the rotation never reaches those states and both checks are killed at the 60s harness budget.
