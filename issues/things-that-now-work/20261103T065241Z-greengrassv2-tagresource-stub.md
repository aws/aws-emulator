Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: greengrassv2.component-tag-untag-list-round-trip greengrassv2.list-tags-for-resource-malformed-arn-rejected greengrassv2.tag-resource-nonexistent-arn-not-found greengrassv2.tag-resource-round-trip-on-deployment-v2 greengrassv2.untag-resource-empty-tag-keys-validation-rejected
Issue-Shape-Id: com.amazonaws.greengrassv2#GreengrassV2
Issue-Key: greengrassv2|TagResource|NotImplemented
Issue-Commit: 3ecb0d6b353755c874093f3eab62bbd62a85b9a2

# Implement greengrassv2 TagResource: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `3ecb0d6b353755c874093f3eab62bbd62a85b9a2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource

## Description
`TagResource` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-greengrassv2-greengrassv2/src/ops/tag_resource.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

5 recorded unit(s) fail on their `TagResource` call, so each one unblocks only when `TagResource` works: `greengrassv2.component-tag-untag-list-round-trip`, `greengrassv2.list-tags-for-resource-malformed-arn-rejected`, `greengrassv2.tag-resource-nonexistent-arn-not-found`, `greengrassv2.tag-resource-round-trip-on-deployment-v2`, `greengrassv2.untag-resource-empty-tag-keys-validation-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-greengrassv2-greengrassv2/src/ops/tag_resource.rs`. Replaying the corpus, the emulator refused the `TagResource` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
           ^^^^^^^^^^^^^^^^^^^^^
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the TagResource operation: operation TagResource is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the greengrassv2 emulator
Call TagResource and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
