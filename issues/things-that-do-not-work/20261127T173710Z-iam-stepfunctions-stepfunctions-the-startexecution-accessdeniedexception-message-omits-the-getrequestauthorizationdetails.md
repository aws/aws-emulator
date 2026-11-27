Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam__stepfunctions.start-execution-grant-on-execution-arn-denied-until-scoped-to-state-machine
Issue-Key: iam__stepfunctions.start-execution-grant-on-execution-arn-denied-until-scoped-to-state-machine|run#21|AssertionError|assert refused == denial(scoped_arn, other, refused[1]), (
Issue-Commit: cc8c6d4514b1c20009096c9d1c08086c7dabca5f

# stepfunctions: the StartExecution AccessDeniedException message omits the GetRequestAuthorizationDetails pointer trailer

> _Note: this issue was identified against the code base at commit `cc8c6d4514b1c20009096c9d1c08086c7dabca5f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartExecution

## Description
=== Cause 1 of 1: the denial message is missing its authorization-details trailer ===
AWS: recorded behavior only (no Smithy member carries the message text). When an assumed-role caller holds a `states:StartExecution` grant scoped to ONE state machine ARN and starts a DIFFERENT machine, Step Functions refuses `AccessDeniedException` with two sentences. The first names the caller, the action and the resource: `User: <caller assumed-role arn> is not authorized to perform: states:StartExecution on resource: <stateMachineArn> because no identity-based policy allows the states:StartExecution action`. The second is the authorization-details pointer: `. Go to https://<region>.console.aws.amazon.com/iam/home?region=<region>#/authorization-details/<authorization-id> for complete details, or call the GetRequestAuthorizationDetails API with the following authorization id: <authorization-id>`. Fixed text: everything except the caller ARN, the state-machine ARN, the region and the per-request authorization id — those four are taken from the request, the emulator's state and the request's own authorization record (the id is per-request and is NOT part of the requirement; its PRESENCE is).
Emulator: the first sentence is served correctly and in full, with the right code; the pointer sentence is absent entirely. The same divergence appears on the mis-scoped leg, where a grant written on EXECUTION ARNs is used to start the state machine.
Why: the pointer is how a caller gets from a refusal to the full authorization record — a client (or a support/debug tool) that follows it, or that calls `GetRequestAuthorizationDetails` with the id it carries, has nothing to follow and cannot diagnose which statement withheld the call.
Fix: the Step Functions authorization-refusal serializer — append the authorization-details pointer to `AccessDeniedException` for this denial class, from the same request-authorization record IAM's denials use.
Related: the open issue 20261126T150000Z-iam-3-causes-still-open-access-key-id-class-denial-trailer-and-unpublished-policy-arn.md analyses WHICH denial classes carry this trailer (its cause 2) for `iam` itself. It does not carry this check's replay key and is filed under a different service; this check requires the trailer on a `states:StartExecution` refusal, where the emulator omits it.
Traces: iam__stepfunctions.start-execution-grant-on-execution-arn-denied-until-scoped-to-state-machine.

## Evidence
From one replay of the behavioral suite (service heading `### iam__stepfunctions`, 0/1 passed).

[mismatch] iam__stepfunctions.start-execution-grant-on-execution-arn-denied-until-scoped-to-state-machine
  File "...start_execution_grant_on_execution_arn_denied_until_scoped_to_state_machine.py", line 192, in run
    assert refused == denial(scoped_arn, other, refused[1]), (...)
  AssertionError: starting the machine the grant does not name gave ('AccessDeniedException', 'User: arn:aws:sts::<account>:assumed-role/<role>/starter is not authorized to perform: states:StartExecution on resource: arn:aws:states:<region>:<account>:stateMachine:<machine> because no identity-based policy allows the states:StartExecution action')

The check builds the expected message as that same sentence PLUS the trailer it finds at the end of the observed message:
  _AUTHZ_TAIL = r"\. Go to https://[a-z0-9-]+\.console\.aws\.amazon\.com/iam/home\?region=[a-z0-9-]+#/authorization-details/(\w+) for complete details, or call the GetRequestAuthorizationDetails API with the following authorization id: \1"
and falls back to the literal `. Go to <authorization-details pointer>` when none is present — which is why the comparison fails: the observed message ends at `... action` with no trailer at all. The code (`AccessDeniedException`) and the first sentence match; the region, account, role, session and machine names are per-run.

## Steps to Reproduce
1. Create two state machines G and O, and two IAM roles trusted by sts:AssumeRole: role S granted `states:StartExecution` on G's state-machine ARN, role M granted `states:StartExecution` on G's EXECUTION ARNs only.
2. AssumeRole S and call `StartExecution(stateMachineArn=G)` — it must succeed (positive control).
3. As S, call `StartExecution(stateMachineArn=O)`. Expect `AccessDeniedException` whose message is `User: <S session arn> is not authorized to perform: states:StartExecution on resource: <O arn> because no identity-based policy allows the states:StartExecution action` FOLLOWED BY `. Go to https://<region>.console.aws.amazon.com/iam/home?region=<region>#/authorization-details/<id> for complete details, or call the GetRequestAuthorizationDetails API with the following authorization id: <id>`. Observed: the first sentence only, no trailer.
4. As M, call `StartExecution(stateMachineArn=G)` — same required message shape, same missing trailer.
