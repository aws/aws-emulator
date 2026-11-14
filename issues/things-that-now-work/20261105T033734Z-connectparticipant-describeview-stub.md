Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: connectparticipant.describe-view-unknown-token-rejected
Issue-Shape-Id: com.amazonaws.connectparticipant#AmazonConnectParticipantServiceLambda
Issue-Key: connectparticipant.describe-view-unknown-token-rejected|run#6|AssertionError|assert err == ("AccessDeniedException", "Access denied"), f"got {err!r}"
Issue-Commit: f788da56fca8042d5f8354f4ce9bfd6169341434

# Implement connectparticipant DescribeView: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `f788da56fca8042d5f8354f4ce9bfd6169341434`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeView

## Description
`DescribeView` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-connectparticipant-amazonconnectparticipantservicelambda/src/ops/describe_view.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `DescribeView` call, so each one unblocks only when `DescribeView` works: `connectparticipant.describe-view-unknown-token-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-connectparticipant-amazonconnectparticipantservicelambda/src/ops/describe_view.rs`. Replaying the corpus, the emulator refused the `DescribeView` call: File "run.py", line 348, in <lambda>
    body = calls.in_phase(calls.BODY, lambda: fn(ctx))
                                              ^^^^^^^
  File "check_connectparticipant_describe_view_unknown_token_rejected.py", line 28, in run
    assert err == ("AccessDeniedException", "Access denied"), f"got {err!r}"
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: got ('NotImplemented', 'operation DescribeView is scaffolded but not implemented in this emulator')

## Steps to Reproduce
Boot the connectparticipant emulator
Call DescribeView and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
