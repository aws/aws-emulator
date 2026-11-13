Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lexv2-models.delete-bot-locale-cascades-to-intents
Issue-Shape-Id: com.amazonaws.lexmodelsv2#LexModelBuildingServiceV2
Issue-Key: lex-models-v2|DeleteBotLocale|NotImplemented
Issue-Commit: 30fa5e7e24c2e7ff08a1ee60d32c6c6e65abb481

# Implement lex-models-v2 DeleteBotLocale: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `30fa5e7e24c2e7ff08a1ee60d32c6c6e65abb481`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteBotLocale

## Description
`DeleteBotLocale` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-lexmodelsv2-lexmodelbuildingservicev2/src/ops/delete_bot_locale.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `DeleteBotLocale` call, so each one unblocks only when `DeleteBotLocale` works: `lex-models-v2.delete-bot-locale-cascades-to-intents`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-lexmodelsv2-lexmodelbuildingservicev2/src/ops/delete_bot_locale.rs`. Replaying the corpus, the emulator refused the `DeleteBotLocale` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
           ^^^^^^^^^^^^^^^^^^^^^
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the DeleteBotLocale operation: operation DeleteBotLocale is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the lex-models-v2 emulator
Call DeleteBotLocale and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
