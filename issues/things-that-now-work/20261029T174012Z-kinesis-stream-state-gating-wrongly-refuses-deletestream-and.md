Issue-Source: trace-sweep-divergence
Issue-Trace: kinesis-channel-create-invalid-role-error
Issue-Step: teardown#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: kinesis-channel-create-invalid-role-error kinesis-channel-list-empty kinesis-duplicate-create-and-delete-nonexistent-stream kinesis-resource-policy-stream-not-active kinesis-channel-lifecycle-to-s3
Issue-Shape-Id: com.amazonaws.kinesis#Kinesis_20131202
Issue-Key: kinesis|DeleteStream|2xx->4xx|status
Issue-Key: kinesis|CreateChannel|4xx->4xx|$.__type
Issue-Key: kinesis|CreateChannel|4xx->4xx|$.message
Issue-Commit: a8a8dc0cc1ff1737eab503998484303fbbf09d5b

# kinesis: stream-state gating wrongly refuses DeleteStream and preempts CreateChannel role validation

> _Note: this issue was identified against the code base at commit `a8a8dc0cc1ff1737eab503998484303fbbf09d5b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteStream

## Description
Two net-new root causes with one shared mechanism: the emulator gates operations on the stream being in StreamStatus ACTIVE in places AWS does not, so a lifecycle call that AWS accepts is refused and a validation AWS performs is never reached. Neither is marked ALREADY FILED or OWNED in the replay report, and no open kinesis issue names the DeleteStream non-ACTIVE refusal or CreateChannel's service-execution-role validation (the open Channel-related issues cover the Channel scaffold stubs and account settings, not role assumability).

=== Cause 1 of 2: DeleteStream is refused with HTTP 400 for a stream that is not yet ACTIVE, where AWS accepts it with HTTP 200 ===
AWS: Smithy: DeleteStream { StreamName, StreamARN, StreamId, EnforceConsumerDeletion } -> output Unit; errors AccessDeniedException, InvalidArgumentException, LimitExceededException, ResourceInUseException, ResourceNotFoundException (all HTTP 400). StreamStatus doc: CREATING "the stream is being created", ACTIVE "ready for read and write operations or deletion", DELETING, UPDATING. A delete request for an existing stream is accepted with HTTP 200 and an empty body and moves the stream to DELETING; AWS does not require the stream to have reached ACTIVE first, and the modeled ResourceInUseException is for the consumers-present case (EnforceConsumerDeletion unset), not for a freshly created stream.
Emulator: the delete is refused with HTTP 400. The sibling CreateChannel step shows the gate's text — "Stream <streamName> under account <accountId> not ACTIVE, instead in state CREATING" — so the refusal is the generic must-be-ACTIVE precondition applied to DeleteStream. Four fixtures hit it in teardown (channel-create-invalid-role-error, channel-list-empty, duplicate-create-and-delete-nonexistent-stream, resource-policy-stream-not-active), i.e. every fixture that deletes a stream it did not first observe as ACTIVE. Hypothesis: DeleteStream shares the ACTIVE-state guard used by the mutating control-plane operations, and/or streams are left in CREATING longer than the recording's streams were.
Why: teardown and cleanup code cannot delete a stream it just created — the single most common lifecycle sequence (create, use, delete) fails, leaving streams and their quota consumed; a client receives a 400 for a request AWS answers 200, so error handling treats a successful intent as a permanent client fault.
Fix: the DeleteStream handler's precondition check in the kinesis service crate — accept any existing stream that is not already DELETING and transition it to DELETING, reserving the 400 ResourceInUseException for the consumers-present case.

=== Cause 2 of 2: CreateChannel never validates ServiceExecutionRoleARN assumability; it refuses on stream state instead of the modeled ValidationException ===
AWS: Smithy: CreateChannel { ChannelName (required), ServiceExecutionRoleARN (required, @length 1..512, @pattern ^arn:aws[-a-z0-9]*:iam::\\d{12}:role/[a-zA-Z_0-9+=,.@\\-_/]+$), StreamConfigurationList (required, 1..10000), S3DestinationConfiguration, ... } -> CreateChannelOutput { ChannelDescription required }; errors include ValidationException and ResourceInUseException (HTTP 400). When the supplied role cannot be assumed by the Kinesis service principal, AWS answers 400 ValidationException with the fixed message "Unable to assume the service execution role. Verify that the role exists and the trust policy allows Kinesis service principal and sts:AssumeRole permissions." — this message has no variable parts; it is entirely fixed text and does not echo the role ARN.
Emulator: answers 400 ResourceInUseException with "Stream <streamName> under account <accountId> not ACTIVE, instead in state CREATING" (fixed frame; stream name and account id come from the request and state). The role is never examined, so the role-assumability check that AWS performs has no implementation behind the stream-state gate; the same cluster covers the CreateChannel steps of the channel-lifecycle-to-s3 fixture, so successful channel creation is blocked by the same gate.
Why: a client misconfiguring a channel's execution role — the single most common CreateChannel failure — is told its *stream* is busy, with a different error code; it will retry forever instead of fixing the trust policy. Conversely, a correctly configured channel cannot be created at all.
Fix: the CreateChannel handler — resolve the stream without requiring ACTIVE where AWS does not, then validate ServiceExecutionRoleARN against the IAM role's existence and its trust policy for the Kinesis service principal, raising ValidationException with AWS's fixed message when assumption is impossible.

## Evidence
Cause 1 — fixture kinesis-channel-create-invalid-role-error, step teardown#4, DeleteStream:
  status: expected 200, got 400
  cluster (divergence_key service=kinesis|DeleteStream|status=200->400) also covers:
    kinesis-channel-list-empty#teardown#3, kinesis-duplicate-create-and-delete-nonexistent-stream#teardown#4, kinesis-resource-policy-stream-not-active#teardown#5
  Expected: HTTP 200 with an empty body; the stream moves to DELETING.

Cause 2 — fixture kinesis-channel-create-invalid-role-error, step task#3, CreateChannel, expected 400 / actual 400:
  $.__type: expected 'ValidationException', got 'ResourceInUseException'
  $.message: expected 'Unable to assume the service execution role. Verify that the role exists and the trust policy allows Kinesis service principal and sts:AssumeRole permissions.',
             got 'Stream <streamName> under account <accountId> not ACTIVE, instead in state CREATING'
  cluster also covers kinesis-channel-lifecycle-to-s3#task#1 and #task#2
  The expected message is wholly fixed text; the emulator's message is the generic must-be-ACTIVE refusal with the request's stream name and the account id substituted in.

## Steps to Reproduce
Cause 1: CreateStream(StreamName=<name>, ShardCount=1) and immediately call DeleteStream(StreamName=<name>) without first polling DescribeStreamSummary for StreamStatus=ACTIVE. Observe HTTP 400 instead of HTTP 200; read the error body to see the must-be-ACTIVE refusal. Repeat after the stream has reached ACTIVE to confirm the gate is the only difference.

Cause 2: CreateStream(StreamName=<name>, ShardCount=1), then CreateChannel(ChannelName=<channel>, ServiceExecutionRoleARN=arn:aws:iam::<account>:role/<a role whose trust policy does not allow kinesis.amazonaws.com>, StreamConfigurationList=[{<the stream>}], S3DestinationConfiguration={...}). Observe 400 ResourceInUseException naming the stream state instead of 400 ValidationException 'Unable to assume the service execution role. ...'. Then retry with a role that IS assumable by the Kinesis service principal and observe that CreateChannel still does not succeed.
