Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: qconnect.get-next-message-token-from-another-session-rejected qconnect.list-messages-pages-cover-session-without-repeats qconnect.list-messages-shows-sent-message-and-its-reply qconnect.list-messages-unissued-next-token-rejected qconnect.send-message-reply-pairs-with-customer-turn qconnect.send-message-through-foreign-assistant-rejected qconnect.send-message-without-knowledge-base-association-rejected
Issue-Shape-Id: com.amazonaws.qconnect#WisdomService
Issue-Key: qconnect|SendMessage|NotImplemented
Issue-Key: qconnect.send-message-through-foreign-assistant-rejected|run#8|AssertionError|assert got == _REFUSAL, f"SendMessage through the other assistant answered {got!r}"
Issue-Key: qconnect.send-message-without-knowledge-base-association-rejected|run#9|AssertionError|assert got == ("ResourceNotFoundException", _NO_ASSOCIATION), (
Issue-Commit: c4056ad47c33d82108e7436e28d6f183e635f0d8

# Implement qconnect SendMessage: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `c4056ad47c33d82108e7436e28d6f183e635f0d8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SendMessage

## Description
`SendMessage` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-qconnect-wisdomservice/src/ops/send_message.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

7 recorded unit(s) fail on their `SendMessage` call, so each one unblocks only when `SendMessage` works: `qconnect.get-next-message-token-from-another-session-rejected`, `qconnect.list-messages-pages-cover-session-without-repeats`, `qconnect.list-messages-shows-sent-message-and-its-reply`, `qconnect.list-messages-unissued-next-token-rejected`, `qconnect.send-message-reply-pairs-with-customer-turn`, `qconnect.send-message-through-foreign-assistant-rejected`, `qconnect.send-message-without-knowledge-base-association-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-qconnect-wisdomservice/src/ops/send_message.rs`. Replaying the corpus, the emulator refused the `SendMessage` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
           ^^^^^^^^^^^^^^^^^^^^^
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the SendMessage operation: operation SendMessage is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the qconnect emulator
Call SendMessage and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
