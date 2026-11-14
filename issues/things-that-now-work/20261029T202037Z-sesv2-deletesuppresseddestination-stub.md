Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sesv2.delete-suppressed-destination-unknown-rejected
Issue-Shape-Id: com.amazonaws.sesv2#SimpleEmailService_v2
Issue-Key: sesv2.delete-suppressed-destination-unknown-rejected|run#5|AssertionError|assert err == expected, f"expected {expected!r}, got {err!r}"
Issue-Commit: 78295efc92da5c3ab6dc723b39af1e4944870efd

# Implement sesv2 DeleteSuppressedDestination: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `78295efc92da5c3ab6dc723b39af1e4944870efd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteSuppressedDestination

## Description
`DeleteSuppressedDestination` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-sesv2-simpleemailservice-v2/src/ops/delete_suppressed_destination.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `DeleteSuppressedDestination` call, so each one unblocks only when `DeleteSuppressedDestination` works: `sesv2.delete-suppressed-destination-unknown-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-sesv2-simpleemailservice-v2/src/ops/delete_suppressed_destination.rs`. Replaying the corpus, the emulator refused the `DeleteSuppressedDestination` call: File "run.py", line 189, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ^^^^^^^
  File "check_sesv2_delete_suppressed_destination_unknown_rejected.py", line 34, in run
    assert err == expected, f"expected {expected!r}, got {err!r}"
           ^^^^^^^^^^^^^^^
AssertionError: expected ('NotFoundException', 'Email address sim2real-suppress-neverd1a8df1c109d@example.com does not exist on your suppression list.'), got ('NotImplemented', 'operation DeleteSuppressedDestination is scaffolded but not implemented in this emulator')

## Steps to Reproduce
Boot the sesv2 emulator
Call DeleteSuppressedDestination and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
