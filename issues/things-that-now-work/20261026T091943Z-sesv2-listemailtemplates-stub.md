Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sesv2.v1-template-token-continues-the-v2-list
Issue-Shape-Id: com.amazonaws.sesv2#SimpleEmailService_v2
Issue-Key: sesv2|ListEmailTemplates|NotImplemented
Issue-Commit: 8ac25d866c3499bf9b2f284cb79ae4218e5e0f1c

# Implement sesv2 ListEmailTemplates: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `8ac25d866c3499bf9b2f284cb79ae4218e5e0f1c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListEmailTemplates

## Description
`ListEmailTemplates` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-sesv2-simpleemailservice-v2/src/ops/list_email_templates.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `ListEmailTemplates` call, so each one unblocks only when `ListEmailTemplates` works: `sesv2.v1-template-token-continues-the-v2-list`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-sesv2-simpleemailservice-v2/src/ops/list_email_templates.rs`. Replaying the corpus, the emulator refused the `ListEmailTemplates` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the ListEmailTemplates operation: operation ListEmailTemplates is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the sesv2 emulator
Call ListEmailTemplates and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
