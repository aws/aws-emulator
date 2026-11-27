Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: datazone.search-next-token-pages-every-glossary-once datazone.search-sorted-by-name-places-a-new-glossary-in-order datazone.search-text-finds-only-the-matching-asset-until-deleted datazone.search-unissued-next-token-rejected
Issue-Shape-Id: com.amazonaws.datazone#DataZone
Issue-Key: datazone|Search|NotImplemented
Issue-Commit: cd3ed932ac942c4bb826a6f713c43cf93575b0d9

# Implement datazone Search: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `cd3ed932ac942c4bb826a6f713c43cf93575b0d9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Search

## Description
`Search` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-datazone-datazone/src/ops/search.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

4 recorded unit(s) fail on their `Search` call, so each one unblocks only when `Search` works: `datazone.search-next-token-pages-every-glossary-once`, `datazone.search-sorted-by-name-places-a-new-glossary-in-order`, `datazone.search-text-finds-only-the-matching-asset-until-deleted`, `datazone.search-unissued-next-token-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-datazone-datazone/src/ops/search.rs`. Replaying the corpus, the emulator refused the `Search` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
           ^^^^^^^^^^^^^^^^^^^^^
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the Search operation: operation Search is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the datazone emulator
Call Search and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
