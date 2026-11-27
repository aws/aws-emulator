Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: gameliftstreams.list-stream-sessions-by-account-foreign-next-token-rejected gameliftstreams.list-stream-sessions-by-account-lists-a-session-just-started gameliftstreams.list-stream-sessions-by-account-pages-each-session-once
Issue-Shape-Id: com.amazonaws.gameliftstreams#GameLiftStreams
Issue-Key: gameliftstreams|ListStreamSessionsByAccount|NotImplemented
Issue-Commit: 51355cfc1cfbb5e6f009c7f9575dfb9e65707db6

# Implement gameliftstreams ListStreamSessionsByAccount: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `51355cfc1cfbb5e6f009c7f9575dfb9e65707db6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListStreamSessionsByAccount

## Description
`ListStreamSessionsByAccount` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-gameliftstreams-gameliftstreams/src/ops/list_stream_sessions_by_account.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

3 recorded unit(s) fail on their `ListStreamSessionsByAccount` call, so each one unblocks only when `ListStreamSessionsByAccount` works: `gameliftstreams.list-stream-sessions-by-account-foreign-next-token-rejected`, `gameliftstreams.list-stream-sessions-by-account-lists-a-session-just-started`, `gameliftstreams.list-stream-sessions-by-account-pages-each-session-once`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-gameliftstreams-gameliftstreams/src/ops/list_stream_sessions_by_account.rs`. Replaying the corpus, the emulator refused the `ListStreamSessionsByAccount` call: File "check_gameliftstreams_list_stream_sessions_by_account_foreign_next_token_rejected.py", line 54, in run
    got = _refusal(lambda: gl.list_stream_sessions_by_account(MaxResults=1, NextToken=token))
          ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
  File "check_gameliftstreams_list_stream_sessions_by_account_foreign_next_token_rejected.py", line 74, in _refusal
    assert marker and tail.endswith(")"), f"message lacks its Internal ID suffix: {message!r}"
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: message lacks its Internal ID suffix: 'operation ListStreamSessionsByAccount is scaffolded but not implemented in this emulator'

## Steps to Reproduce
Boot the gameliftstreams emulator
Call ListStreamSessionsByAccount and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
