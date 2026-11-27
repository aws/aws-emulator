Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: chime-sdk-messaging.batch-membership-all-entries-refused-still-answered chime-sdk-messaging.batch-membership-errors-keyed-to-each-refused-member chime-sdk-messaging.batch-membership-unknown-user-refused-beside-accepted chime-sdk-messaging.batch-membership-unknown-user-reported-in-errors chime-sdk-messaging.deleted-message-read-back-forbidden-rejected chime-sdk-messaging.elastic-channel-membership-preferences-rejected chime-sdk-messaging.list-sub-channels-pages-every-sub-channel-once chime-sdk-messaging.read-marker-reaches-own-membership-view
Issue-Shape-Id: com.amazonaws.chimesdkmessaging#ChimeMessagingService
Issue-Key: chime-sdk-messaging|BatchCreateChannelMembership|NotImplemented
Issue-Commit: 736b28b40b20576fe554bad4abfa7fe4ba2cddb1

# Implement chime-sdk-messaging BatchCreateChannelMembership: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `736b28b40b20576fe554bad4abfa7fe4ba2cddb1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchCreateChannelMembership

## Description
`BatchCreateChannelMembership` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-chimesdkmessaging-chimemessagingservice/src/ops/batch_create_channel_membership.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

8 recorded unit(s) fail on their `BatchCreateChannelMembership` call, so each one unblocks only when `BatchCreateChannelMembership` works: `chime-sdk-messaging.batch-membership-all-entries-refused-still-answered`, `chime-sdk-messaging.batch-membership-errors-keyed-to-each-refused-member`, `chime-sdk-messaging.batch-membership-unknown-user-refused-beside-accepted`, `chime-sdk-messaging.batch-membership-unknown-user-reported-in-errors`, `chime-sdk-messaging.deleted-message-read-back-forbidden-rejected`, `chime-sdk-messaging.elastic-channel-membership-preferences-rejected`, `chime-sdk-messaging.list-sub-channels-pages-every-sub-channel-once`, `chime-sdk-messaging.read-marker-reaches-own-membership-view`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-chimesdkmessaging-chimemessagingservice/src/ops/batch_create_channel_membership.rs`. Replaying the corpus, the emulator refused the `BatchCreateChannelMembership` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
           ^^^^^^^^^^^^^^^^^^^^^
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the BatchCreateChannelMembership operation: operation BatchCreateChannelMembership is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the chime-sdk-messaging emulator
Call BatchCreateChannelMembership and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
