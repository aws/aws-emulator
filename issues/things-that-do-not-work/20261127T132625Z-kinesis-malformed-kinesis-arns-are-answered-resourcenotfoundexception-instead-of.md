Issue-Source: trace-sweep-divergence
Issue-Trace: delete-channel-arn-account-token-grammar
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: delete-channel-arn-account-token-grammar kinesis-delete-channel-arn-length-bounds kinesis-deregister-stream-consumer-arn-length-bounds
Issue-Shape-Id: com.amazonaws.kinesis#Kinesis_20131202
Issue-Key: kinesis|DeleteChannel|4xx->4xx|$.__type
Issue-Key: kinesis|DeregisterStreamConsumer|4xx->4xx|$.__type
Issue-Related: 20261127T132625Z-kinesis-refusal-responses-echo-values-patterns-and-codes.md
Issue-Commit: 40df241c1ce9889c9be50646fd3e82dae29dae7f

# Malformed Kinesis ARNs are answered ResourceNotFoundException instead of an ARN-format refusal

> _Note: this issue was identified against the code base at commit `40df241c1ce9889c9be50646fd3e82dae29dae7f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteChannel

## Description
Two root causes sharing one fault: for an ARN that satisfies the member's modelled @length/@pattern constraints but is not a well-formed Kinesis ARN of the required kind, the emulator skips the ARN-grammar check entirely, parses whatever trails the resource type as a resource NAME, looks that name up, and refuses with ResourceNotFoundException. AWS refuses the ARN's FORMAT, with a different error code and a fixed message that names no resource.

=== Cause 1 of 2: DeleteChannel answers ResourceNotFoundException for a channel ARN whose resource part is not a single channel name ===
AWS: Smithy: DeleteChannel.ChannelARN @required, target ChannelARN, @length min 1 max 2048, @pattern "^arn:aws.*:kinesis:.*:\\d{12}:channel/\\S+$"; DeleteChannel binds ValidationException, InvalidArgumentException, ResourceNotFoundException, AccessDeniedException, LimitExceededException (all client, HTTP 400). The pattern's "\\S+" tail admits further '/' separators, so passing the pattern does NOT make the ARN well formed; AWS applies an ARN-grammar check after it. Recorded behavior: an ARN that matches the pattern but whose channel resource part carries an extra path segment is answered HTTP 400, __type "ValidationException", message "The format of the channel ARN is invalid." — entirely fixed text: no ARN, channel name or account id is interpolated.
Emulator: HTTP 400 with __type "ResourceNotFoundException" and message "Channel <last path segment> under account <account id> not found." — it has taken the trailing segment of the malformed ARN to be the channel name and reported a lookup miss. The status matches; the error code and the whole message do not.
Why: the two codes mean opposite things to a caller. ResourceNotFoundException says "this ARN is fine, the channel is gone" — idempotent deleters treat it as success and converge loops stop retrying — whereas ValidationException says the caller built a bad ARN and must fix it. A client fed a corrupt ARN by a template or a string-concatenation bug sees a clean "already deleted" against the emulator and a hard validation failure against AWS. The leaked account id in the emulator's message is also information AWS does not put in this response.
Fix: DeleteChannel's handler — validate the ARN's grammar (partition/service/region/account plus a channel resource part of exactly "channel/<name>" with no further separators) and raise ValidationException "The format of the channel ARN is invalid." before any channel lookup.
Traces: delete-channel-arn-account-token-grammar#task#2 (representative), kinesis-delete-channel-arn-length-bounds#task#1.

=== Cause 2 of 2: DeregisterStreamConsumer answers ResourceNotFoundException for a consumer ARN that is not a consumer ARN ===
AWS: Smithy: DeregisterStreamConsumer.ConsumerARN targets ConsumerARN, @length min 1 max 2048, @pattern "^(arn):aws.*:kinesis:.*:\\d{12}:.*stream\\/[a-zA-Z0-9_.-]+\\/consumer\\/[a-zA-Z0-9_.-]+:[0-9]+$"; the operation binds ONLY InvalidArgumentException, LimitExceededException and ResourceNotFoundException — it does not bind ValidationException, so its format refusal is InvalidArgumentException, not ValidationException as for DeleteChannel. Recorded behavior: a consumerARN that is within the length bound but is not a well-formed consumer ARN is answered HTTP 400, __type "InvalidArgumentException", message "The format of the consumer ARN is invalid." — entirely fixed text.
Emulator: HTTP 400 with __type "ResourceNotFoundException" and a message of the form "Consumer <the junk taken as a consumer name> ...", i.e. the whole malformed value is treated as a consumer name and echoed back in a not-found sentence; in the replayed step this reflects a multi-hundred-character run of the submitted string into the message.
Why: as in cause 1, a deregistering client reads ResourceNotFoundException as "already deregistered" and succeeds where AWS fails it; and the refusal echoes an unbounded slice of the request into the error message, which AWS's fixed sentence never does.
Fix: DeregisterStreamConsumer's handler — run the consumer-ARN grammar check (stream name, "/consumer/", consumer name, ':' plus the creation epoch) ahead of the consumer lookup and raise InvalidArgumentException "The format of the consumer ARN is invalid.".
Traces: kinesis-deregister-stream-consumer-arn-length-bounds#task#1.

## Evidence
Cause 1 — delete-channel-arn-account-token-grammar#task#2, DeleteChannel, status 400 expected and 400 observed; $.__type value_mismatch: expected 'ValidationException', got 'ResourceNotFoundException'; $.message value_mismatch: expected 'The format of the channel ARN is invalid.', got 'Channel x under account 000000000000 not found.' (the 'x' is the extra trailing path segment of the submitted ARN, and the account id is state the response should not carry). Cluster also covers kinesis-delete-channel-arn-length-bounds#task#1.

Cause 2 — kinesis-deregister-stream-consumer-arn-length-bounds#task#1, DeregisterStreamConsumer, status 400 expected and 400 observed; $.__type value_mismatch: expected 'InvalidArgumentException', got 'ResourceNotFoundException'; $.message value_mismatch: expected 'The format of the consumer ARN is invalid.', got 'Consumer caaaaaaa…aaaa' — the submitted value echoed as a consumer name in a not-found sentence.

## Steps to Reproduce
1. Cause 1: create a channel, then POST X-Amz-Target Kinesis_20131202.DeleteChannel with the channel's ARN plus an extra path segment appended to the resource part (arn:aws:kinesis:<region>:<12-digit account>:channel/<name>/x) — it matches the member's @pattern. Read $.__type and $.message: expect 'ValidationException' and 'The format of the channel ARN is invalid.'; observe 'ResourceNotFoundException' and 'Channel x under account <account> not found.'
2. Cause 2: POST Kinesis_20131202.DeregisterStreamConsumer with a ConsumerARN that is within 1..2048 characters but is not a consumer ARN (e.g. a long run of characters with no stream/consumer segments). Read $.__type and $.message: expect 'InvalidArgumentException' and 'The format of the consumer ARN is invalid.'; observe 'ResourceNotFoundException' with the submitted value echoed as the consumer name.
3. Confirm the well-formed paths still behave: DeleteChannel on a real channel ARN succeeds, and DeregisterStreamConsumer on a registered consumer's ARN succeeds.
4. Re-replay delete-channel-arn-account-token-grammar#task#2 and kinesis-deregister-stream-consumer-arn-length-bounds#task#1.
