Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: customer-profiles.detect-profile-object-type-accepts-five-objects-and-rejects-six customer-profiles.detect-profile-object-type-rejects-a-domain-that-was-never-created customer-profiles.detect-profile-object-type-rejects-an-object-that-is-not-json
Issue-Shape-Id: com.amazonaws.customerprofiles#CustomerProfiles_20200815
Issue-Key: customer-profiles|DetectProfileObjectType|NotImplemented
Issue-Key: customer-profiles.detect-profile-object-type-rejects-an-object-that-is-not-json|run#6|AssertionError|assert got == ("BadRequestException", "Could not parse object string into Json."), (
Issue-Commit: cd3ed932ac942c4bb826a6f713c43cf93575b0d9

# Implement customer-profiles DetectProfileObjectType: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `cd3ed932ac942c4bb826a6f713c43cf93575b0d9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DetectProfileObjectType

## Description
`DetectProfileObjectType` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-customerprofiles-customerprofiles-20200815/src/ops/detect_profile_object_type.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

3 recorded unit(s) fail on their `DetectProfileObjectType` call, so each one unblocks only when `DetectProfileObjectType` works: `customer-profiles.detect-profile-object-type-accepts-five-objects-and-rejects-six`, `customer-profiles.detect-profile-object-type-rejects-a-domain-that-was-never-created`, `customer-profiles.detect-profile-object-type-rejects-an-object-that-is-not-json`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-customerprofiles-customerprofiles-20200815/src/ops/detect_profile_object_type.rs`. Replaying the corpus, the emulator refused the `DetectProfileObjectType` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
           ^^^^^^^^^^^^^^^^^^^^^
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the DetectProfileObjectType operation: operation DetectProfileObjectType is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the customer-profiles emulator
Call DetectProfileObjectType and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
