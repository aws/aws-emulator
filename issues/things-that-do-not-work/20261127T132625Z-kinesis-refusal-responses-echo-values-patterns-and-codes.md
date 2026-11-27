Issue-Source: trace-sweep-divergence
Issue-Trace: delete-channel-arn-account-token-grammar
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: delete-channel-arn-account-token-grammar kinesis-delete-channel-arn-length-bounds kinesis-list-channels-stream-filter-size-bounds kinesis-record-distribution-strategy-enum-values kinesis-deregister-consumer-arn-without-creation-suffix-rejected kinesis-deregister-stream-consumer-arn-length-bounds kinesis-create-stream-record-distribution-strategy kinesis-json-protocol-wrong-type-and-malformed-base64-rejected
Issue-Shape-Id: com.amazonaws.kinesis#Kinesis_20131202
Issue-Key: kinesis|DeleteChannel|4xx->4xx|$.message
Issue-Key: kinesis|ListChannels|4xx->4xx|$.message
Issue-Key: kinesis|UpdateStreamRecordDistributionStrategy|4xx->4xx|$.message
Issue-Key: kinesis|DeregisterStreamConsumer|4xx->4xx|$.message
Issue-Key: kinesis|CreateStream|4xx->4xx|$.message
Issue-Key: kinesis|DescribeStreamSummary|4xx->4xx|$.Message
Issue-Key: kinesis|PutRecord|4xx->4xx|$.__type
Issue-Key: kinesis|PutRecord|4xx->4xx|$.message
Issue-Commit: 40df241c1ce9889c9be50646fd3e82dae29dae7f

# Kinesis refusal responses echo values, patterns and codes AWS never serves

> _Note: this issue was identified against the code base at commit `40df241c1ce9889c9be50646fd3e82dae29dae7f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteChannel

## Description
Seven root causes in one place: the responses Kinesis serves when it REFUSES a request. In every one of them the emulator reaches the right decision (and in six of seven the right HTTP status and error code) but renders a message — or, in cause 7, an error code — that the service does not serve for that error class. All seven are wire divergences replayed from the corpus; none is marked blocked by a stub and none is owned by an open issue.

=== Cause 1 of 7: DeleteChannel's channelARN pattern refusal echoes the offending ARN, which AWS omits ===
AWS: Smithy: DeleteChannel.ChannelARN @required, target ChannelARN (string, @length min 1 max 2048, @pattern "^arn:aws.*:kinesis:.*:\\d{12}:channel/\\S+$"); DeleteChannel binds ValidationException (client error, HTTP 400). Recorded behavior: HTTP 400, __type "ValidationException", message "1 validation error detected: Value at 'channelARN' failed to satisfy constraint: Member must satisfy regular expression pattern: arn:aws.*:kinesis:.*:\\d{12}:channel/\\S+". The fixed text of this template is "1 validation error detected: Value at '" + "' failed to satisfy constraint: Member must satisfy regular expression pattern: "; the variable parts are the wire member name (channelARN, lower-camel as the wire names it) and the pattern as the service declares it for that member. The offending VALUE is not part of the template for this member at all.
Emulator: same status and code, but the renderer always interpolates the value: "1 validation error detected: Value '<the submitted ARN>' at 'channelARN' failed to satisfy constraint: Member must satisfy regular expression pattern: arn:aws.*:kinesis:.*:\\d{12}:channel/\\S+". The same cluster covers an over-length channelARN (kinesis-delete-channel-arn-length-bounds#task#2), which diverges identically.
Why: clients, CLI users and log scrapers that match AWS's constraint sentence see a sentence with an extra quoted segment, and an over-long or binary-ish ARN is reflected back into a diagnostic AWS deliberately keeps value-free.
Fix: the shared constraint-violation message renderer behind the awsJson1_1 request validator — it must emit the value-free form for this member rather than always interpolating the submitted value.
Hypothesis: AWS omits the rendered value per member/shape (it IS echoed for consumerARN, see cause 4), so the choice is a property of the member, not of the request.
Traces: delete-channel-arn-account-token-grammar#task#1 (representative), kinesis-delete-channel-arn-length-bounds#task#2.

=== Cause 2 of 7: ListChannels' streamFilter length refusal echoes the empty list ===
AWS: Smithy: ListChannels.StreamFilter targets StreamFilterList, @length min 1 max 10000; ListChannels binds ValidationException (400). Recorded behavior: HTTP 400, __type "ValidationException", message "1 validation error detected: Value at 'streamFilter' failed to satisfy constraint: Member must have length greater than or equal to 1" — fixed text apart from the member name and the modelled minimum, with no rendering of the list value.
Emulator: "1 validation error detected: Value '[]' at 'streamFilter' failed to satisfy constraint: Member must have length greater than or equal to 1" — the same always-interpolate defect as cause 1, here on a list member, where a rendered collection is never part of AWS's message.
Why: same as cause 1; additionally a client cannot rely on the message being bounded in size, because a large rejected collection would be serialized into it.
Fix: the same constraint-violation renderer, list-member path, invoked from ListChannels' input validation.
Traces: kinesis-list-channels-stream-filter-size-bounds#task#5.

=== Cause 3 of 7: UpdateStreamRecordDistributionStrategy's enum refusal echoes the bad enum value ===
AWS: Smithy: UpdateStreamRecordDistributionStrategy.RecordDistributionStrategy @required, target RecordDistributionStrategy (enum, values [AUTO, USER_PARTITION_KEY]); the operation binds ValidationException (400). Recorded behavior: HTTP 400, __type "ValidationException", message "1 validation error detected: Value at 'recordDistributionStrategy' failed to satisfy constraint: Member must satisfy enum value set: [AUTO, USER_PARTITION_KEY]" — fixed text apart from the member name and the modelled value set, which is rendered comma-and-space separated inside square brackets in model order.
Emulator: "1 validation error detected: Value 'ROUND_ROBIN' at 'recordDistributionStrategy' failed to satisfy constraint: Member must satisfy enum value set: [AUTO, USER_PARTITION_KEY]" — the submitted value is interpolated where AWS has no slot for it. (The quoted value is only the one this recording happened to send; the defect is the extra slot, not that string.)
Why: same as cause 1, on the enum path.
Fix: the same renderer, enum path, invoked from UpdateStreamRecordDistributionStrategy's input validation.
Traces: kinesis-record-distribution-strategy-enum-values#task#3, kinesis-record-distribution-strategy-enum-values#task#4.

=== Cause 4 of 7: DeregisterStreamConsumer's consumerARN pattern refusal quotes the pattern with its leading anchor stripped ===
AWS: Smithy: DeregisterStreamConsumer.ConsumerARN targets ConsumerARN, @length min 1 max 2048, @pattern "^(arn):aws.*:kinesis:.*:\\d{12}:.*stream\\/[a-zA-Z0-9_.-]+\\/consumer\\/[a-zA-Z0-9_.-]+:[0-9]+$". Recorded behavior: HTTP 400, __type "ValidationException", message "1 validation error detected: Value '<the submitted ARN>' at 'consumerARN' failed to satisfy constraint: Member must satisfy regular expression pattern: ^(arn):aws.*:kinesis:.*:\\d{12}:.*stream\\/[a-zA-Z0-9_.-]+\\/consumer\\/[a-zA-Z0-9_.-]+:[0-9]+". Here the value IS echoed, and the pattern is reproduced as the service declares it for this member — including the leading "^".
Emulator: identical except that the quoted pattern begins "(arn):aws.*...": the renderer normalizes the member's pattern by stripping both anchors before printing it, so the leading "^" that AWS's text carries is lost. The requirement is that the pattern be reproduced verbatim as declared for the member, not re-derived from a normalized regex.
Why: the pattern in this message is the only machine-readable statement of the accepted ARN grammar; a client or test that compiles or compares it gets a regex that differs from AWS's, and the anchor difference changes what the regex means.
Fix: the pattern-constraint branch of the same renderer — print the member's declared pattern text rather than the anchor-stripped form used for matching.
Traces: kinesis-deregister-consumer-arn-without-creation-suffix-rejected#task#5, kinesis-deregister-stream-consumer-arn-length-bounds#task#3.

=== Cause 5 of 7: CreateStream's RecordDistributionStrategy-on-a-provisioned-stream refusal uses emulator-authored prose ===
AWS: Smithy: CreateStream.RecordDistributionStrategy is an optional RecordDistributionStrategy enum and StreamModeDetails is a separate optional member, so the "on-demand only" pairing rule is a runtime rule, not a shape constraint; CreateStream binds InvalidArgumentException and ValidationException (both 400). StreamDescriptionSummary.RecordDistributionStrategy documents the rule: "This field is only present for streams that use the on-demand capacity mode." Recorded behavior: HTTP 400 with the message "RecordDistributionStrategy can only be set for On-Demand streams" — entirely fixed text; nothing from the request is interpolated.
Emulator: same status and code, message "RecordDistributionStrategy is only supported for a ON_DEMAND stream" — emulator-authored wording for the same refusal.
Why: the message is part of the contract for this error class; anything matching or displaying AWS's sentence (including the ungrammatical-looking but exact "On-Demand streams" form) does not match.
Fix: the capacity-mode/RecordDistributionStrategy pairing check in the CreateStream handler's validation. Open issue 20261103T071946Z-kinesis-4-net-new-root-causes-listchannels-misclassifies.md reports the same class of invented CreateStream refusal text for the PROVISIONED-without-ShardCount rule; it does not cover this rule and does not carry this replay key.
Traces: kinesis-create-stream-record-distribution-strategy#task#1.

=== Cause 6 of 7: the awsJson1_1 wrong-type deserialization message names the Java-ish target type and the literal value ===
AWS: recorded behavior only — this is a protocol-level (pre-validation) refusal, so no modelled error on DescribeStreamSummary describes it; Kinesis speaks awsJson1_1 (sdkId Kinesis, Kinesis_20131202). Recorded behavior: sending a JSON number where DescribeStreamSummary.StreamName (a string member) is expected is answered HTTP 400 with the top-level member "Message" (capitalized) carrying "NUMBER_VALUE can not be converted to a String". The fixed text is " can not be converted to a "; the variable parts are the JSON token CLASS of the offending node (NUMBER_VALUE) and the name of the target type as the protocol names it (String) — not the literal value the client sent.
Emulator: the same status and the same __type, but "Message" reads "'12' can not be converted to a CharSequence": the literal value is substituted for the token class, and the target type is named CharSequence instead of String.
Why: this is the diagnostic every SDK surfaces for a mis-typed request body; a client cannot key on the token class, and "CharSequence" names a type the Kinesis wire protocol never mentions.
Fix: the awsJson1_1 request deserializer's type-coercion error path — report the offending node's JSON token class and the protocol-level target type name.
Traces: kinesis-json-protocol-wrong-type-and-malformed-base64-rejected#task#3.

=== Cause 7 of 7: an undecodable base64 blob is refused as a ValidationException with a message instead of a bare SerializationException ===
AWS: Smithy: PutRecord.Data @required, target Data (blob, @length min 0 max 10485760), carried base64-encoded in the awsJson1_1 body; a body that cannot be decoded never reaches validation. Recorded behavior: HTTP 400, __type "SerializationException", and NO message member in the body at all (the error is reported by code alone).
Emulator: HTTP 400 with __type "ValidationException" and message "invalid base64: failed to decode base64" — a modelled request-validation error, plus emulator-authored prose, for what AWS classifies as a protocol serialization failure.
Why: the two classes are not interchangeable. SerializationException tells a client its serializer produced a body Kinesis cannot parse (a bug to fix client-side, never retried); ValidationException tells it a well-formed request carried a bad field value. A client that distinguishes them — or that reads the message member AWS does not send — behaves differently against the emulator than against AWS.
Fix: the base64 decode step of the awsJson1_1 blob deserializer — fail with SerializationException and no message member, ahead of the PutRecord handler's input validation.
Traces: kinesis-json-protocol-wrong-type-and-malformed-base64-rejected#task#4.

## Evidence
Cause 1 — delete-channel-arn-account-token-grammar#task#1, DeleteChannel, status 400 expected and 400 observed; $.message value_mismatch: expected "1 validation error detected: Value at 'channelARN' failed to satisfy constraint: Member must satisfy regular expression pattern: arn:aws.*:kinesis:.*:\\d{12}:channel/\\S+", got "1 validation error detected: Value 'arn:aws:kinesis:us-west-2:0000000000000:channel/sim2real-probe-kch-8667df87/x' at 'channelARN' failed to satisfy constraint: Member must satisfy regular expression pattern: arn:aws.*:kinesis:.*:\\d{12}:channel/\\S+". Cluster also covers kinesis-delete-channel-arn-length-bounds#task#2.

Cause 2 — kinesis-list-channels-stream-filter-size-bounds#task#5, ListChannels, 400 expected and observed; $.message: expected "1 validation error detected: Value at 'streamFilter' failed to satisfy constraint: Member must have length greater than or equal to 1", got "1 validation error detected: Value '[]' at 'streamFilter' failed to satisfy constraint: Member must have length greater than or equal to 1".

Cause 3 — kinesis-record-distribution-strategy-enum-values#task#3 (and #task#4), UpdateStreamRecordDistributionStrategy, 400 expected and observed; $.message: expected "1 validation error detected: Value at 'recordDistributionStrategy' failed to satisfy constraint: Member must satisfy enum value set: [AUTO, USER_PARTITION_KEY]", got "1 validation error detected: Value 'ROUND_ROBIN' at 'recordDistributionStrategy' failed to satisfy constraint: Member must satisfy enum value set: [AUTO, USER_PARTITION_KEY]".

Cause 4 — kinesis-deregister-consumer-arn-without-creation-suffix-rejected#task#5 (and kinesis-deregister-stream-consumer-arn-length-bounds#task#3), DeregisterStreamConsumer, 400 expected and observed; $.message: expected pattern text "^(arn):aws.*:kinesis:.*:\\d{12}:.*stream\\/[a-zA-Z0-9_.-]+\\/consumer\\/[a-zA-Z0-9_.-]+:[0-9]+", got the same text with the leading "^" removed: "(arn):aws.*:kinesis:.*:\\d{12}:.*stream\\/[a-zA-Z0-9_.-]+\\/consumer\\/[a-zA-Z0-9_.-]+:[0-9]+". The echoed value and the rest of the sentence match.

Cause 5 — kinesis-create-stream-record-distribution-strategy#task#1, CreateStream, 400 expected and observed; $.message: expected 'RecordDistributionStrategy can only be set for On-Demand streams', got 'RecordDistributionStrategy is only supported for a ON_DEMAND stream'.

Cause 6 — kinesis-json-protocol-wrong-type-and-malformed-base64-rejected#task#3, DescribeStreamSummary, 400 expected and observed; $.Message: expected 'NUMBER_VALUE can not be converted to a String', got "'12' can not be converted to a CharSequence".

Cause 7 — kinesis-json-protocol-wrong-type-and-malformed-base64-rejected#task#4, PutRecord, 400 expected and observed; $.__type: expected 'SerializationException', got 'ValidationException'; $.message: unexpected_present — expected absent, got 'invalid base64: failed to decode base64'.

## Steps to Reproduce
1. Cause 1: POST X-Amz-Target Kinesis_20131202.DeleteChannel with a ChannelARN whose account id is not 12 digits (e.g. a 13-digit account segment) and read $.message — expect the value-free form "1 validation error detected: Value at 'channelARN' failed to satisfy constraint: Member must satisfy regular expression pattern: arn:aws.*:kinesis:.*:\\d{12}:channel/\\S+"; observe the submitted ARN interpolated after "Value ". Repeat with a channelARN longer than 2048 characters (same cluster).
2. Cause 2: kinesis.list_channels(StreamFilter=[]) and read $.message — expect "1 validation error detected: Value at 'streamFilter' failed to satisfy constraint: Member must have length greater than or equal to 1"; observe "Value '[]' at 'streamFilter' ...".
3. Cause 3: kinesis.update_stream_record_distribution_strategy(StreamARN=<arn of an on-demand stream>, RecordDistributionStrategy='ROUND_ROBIN') and read $.message — expect "Value at 'recordDistributionStrategy' ..."; observe the value interpolated.
4. Cause 4: kinesis.deregister_stream_consumer(ConsumerARN=<a consumer ARN with no ':<creation-epoch>' suffix>) and compare the regular-expression text in $.message with the member's declared pattern — expect it to start with "^(arn):"; observe "(arn):".
5. Cause 5: kinesis.create_stream(StreamName=N, ShardCount=1, RecordDistributionStrategy='USER_PARTITION_KEY') (i.e. a provisioned stream) and read $.message — expect 'RecordDistributionStrategy can only be set for On-Demand streams'; observe 'RecordDistributionStrategy is only supported for a ON_DEMAND stream'.
6. Cause 6: POST Kinesis_20131202.DescribeStreamSummary with body {"StreamName": 12} and read $.Message — expect 'NUMBER_VALUE can not be converted to a String'; observe "'12' can not be converted to a CharSequence".
7. Cause 7: POST Kinesis_20131202.PutRecord with "Data" set to a string that is not valid base64 and read $.__type and $.message — expect 'SerializationException' and NO message member; observe 'ValidationException' with 'invalid base64: failed to decode base64'.
8. Re-replay every fixture#step listed in Issue-Trace/Issue-Step and trace_ids.
