Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ses.template-lifecycle ses.template-render-applies-handlebars-conditional-branches ses.template-render-rejects-a-missing-rendering-attribute ses.test-render-template-invalid-json-fails ses.test-render-template-missing-attribute-fails ses.test-render-template-success
Issue-Shape-Id: com.amazonaws.ses#SimpleEmailService
Issue-Key: ses|TestRenderTemplate|NotImplemented
Issue-Commit: bb568fea88423ace8b4c8a03ddba0a6ee612948a

# Implement ses TestRenderTemplate: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `bb568fea88423ace8b4c8a03ddba0a6ee612948a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TestRenderTemplate

## Description
`TestRenderTemplate` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-ses-simpleemailservice/src/ops/test_render_template.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

6 recorded unit(s) fail on their `TestRenderTemplate` call, so each one unblocks only when `TestRenderTemplate` works: `ses.template-lifecycle`, `ses.template-render-applies-handlebars-conditional-branches`, `ses.template-render-rejects-a-missing-rendering-attribute`, `ses.test-render-template-invalid-json-fails`, `ses.test-render-template-missing-attribute-fails`, `ses.test-render-template-success`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-ses-simpleemailservice/src/ops/test_render_template.rs`. Replaying the corpus, the emulator refused the `TestRenderTemplate` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the TestRenderTemplate operation: operation TestRenderTemplate is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the ses emulator
Call TestRenderTemplate and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
