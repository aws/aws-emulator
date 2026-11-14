Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kinesisanalyticsv2.studio-notebook-tags-at-create-and-after-round-trip-and-untag-removes-one-key
Issue-Shape-Id: com.amazonaws.kinesisanalyticsv2#KinesisAnalytics_20180523
Issue-Key: kinesis-analytics-v2|ListTagsForResource|NotImplemented
Issue-Commit: dc7d1cb059ddc05800728693f0165ec8a2a1b13e

# Implement kinesisanalyticsv2 ListTagsForResource: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `dc7d1cb059ddc05800728693f0165ec8a2a1b13e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListTagsForResource

## Description
`ListTagsForResource` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-kinesisanalyticsv2-kinesisanalytics-20180523/src/ops/list_tags_for_resource.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `ListTagsForResource` call, so each one unblocks only when `ListTagsForResource` works: `kinesisanalyticsv2.studio-notebook-tags-at-create-and-after-round-trip-and-untag-removes-one-key`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-kinesisanalyticsv2-kinesisanalytics-20180523/src/ops/list_tags_for_resource.rs`. Replaying the corpus, the emulator refused the `ListTagsForResource` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
           ^^^^^^^^^^^^^^^^^^^^^
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the ListTagsForResource operation: operation ListTagsForResource is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the kinesisanalyticsv2 emulator
Call ListTagsForResource and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
