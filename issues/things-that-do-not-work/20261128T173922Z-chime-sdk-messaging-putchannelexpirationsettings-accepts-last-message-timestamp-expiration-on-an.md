Issue-Source: trace-sweep-divergence
Issue-Trace: chime-sdk-messaging-elastic-channel-last-message-expiration-rejected
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: chime-sdk-messaging-elastic-channel-last-message-expiration-rejected
Issue-Shape-Id: com.amazonaws.chimesdkmessaging#ChimeMessagingService
Issue-Key: chime-sdk-messaging|PutChannelExpirationSettings|4xx->2xx|$.Code
Issue-Key: chime-sdk-messaging|PutChannelExpirationSettings|4xx->2xx|$.Message
Issue-Key: chime-sdk-messaging|PutChannelExpirationSettings|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: chime-sdk-messaging|PutChannelExpirationSettings|4xx->2xx|status
Issue-Key: chime-sdk-messaging|DescribeChannel|2xx->2xx|$.Channel.ExpirationSettings
Issue-Commit: 757e19016492578f68fb6ad3f54c4ad74c32c8c1

# PutChannelExpirationSettings accepts LAST_MESSAGE_TIMESTAMP expiration on an elastic channel and persists it

> _Note: this issue was identified against the code base at commit `757e19016492578f68fb6ad3f54c4ad74c32c8c1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutChannelExpirationSettings

## Description
Two net-new wire divergences with one root cause: PutChannelExpirationSettings never enforces the rule that an elastic channel cannot expire on its last message, so the unsupported setting is both accepted and written into channel state, where a later DescribeChannel reports it. Both causes are numbered below and each carries its own fixture and step; both must be closed before this issue is submitted.

Cause 1 of 2: the unsupported combination is accepted instead of refused.
AWS: Smithy models PutChannelExpirationSettings.ExpirationSettings as an optional ExpirationSettings structure whose members are both required — ExpirationDays (integer, @range 1..5475) and ExpirationCriterion (enum ExpirationCriterion with values CREATED_TIMESTAMP and LAST_MESSAGE_TIMESTAMP) — and declares BadRequestException with httpError 400 (doc: 'The input parameters don't match the service's restrictions.'). The model does not express the cross-field rule, so the rule itself is recorded behavior only: a channel created with an ElasticChannelConfiguration (Smithy ElasticChannelConfiguration: MaximumSubChannels, TargetMembershipsPerSubChannel, MinimumMembershipPercentage, all required) may NOT be given ExpirationCriterion 'LAST_MESSAGE_TIMESTAMP'. The recording shows AWS refusing with HTTP 400, Code 'BadRequest' and the message 'Elastic channels do not support expiration settings based on LastMessageTimestamp.' — that entire sentence is fixed text; neither the channel ARN nor the ExpirationDays value is interpolated. The errortype header carries the BadRequestException shape name.
Emulator: HTTP 200 with a success body — no Code, no Message, no x-amzn-errortype — i.e. the elastic/criterion combination is never checked.
Why: a client is told the channel will be garbage-collected on last-message activity, which AWS will never do for an elastic channel. Retention/compliance code that sets this once and trusts the 200 diverges in production (where the call fails outright), and any negative test of the rule silently passes against the emulator.
Fix: PutChannelExpirationSettings' input validation in its handler — after resolving the channel, if it carries an ElasticChannelConfiguration and the submitted ExpirationSettings.ExpirationCriterion is LAST_MESSAGE_TIMESTAMP, raise BadRequestException(400)/Code 'BadRequest' with that sentence, BEFORE any state write.
Traces: chime-sdk-messaging-elastic-channel-last-message-expiration-rejected#task#4.

Cause 2 of 2: the rejected setting is persisted, so DescribeChannel reports an expiration policy AWS would never hold.
AWS: Smithy DescribeChannel returns Channel, whose ExpirationSettings member projects the channel's stored settings. Because AWS refused the write in cause 1, the recording shows the elastic channel's Channel.ExpirationSettings ABSENT (null) on the following DescribeChannel.
Emulator: DescribeChannel answers 200 with Channel.ExpirationSettings = {ExpirationDays: <the submitted days>, ExpirationCriterion: 'LAST_MESSAGE_TIMESTAMP'} — the refused setting was committed to channel state and is now read back.
Why: the bad write is durable, so it is not merely a lenient refusal: every subsequent read of that channel (and anything that reconciles or copies channel configuration) carries a setting AWS cannot represent, and auditing that compares emulator state against AWS sees a phantom retention policy. This is why the fix must reject before the write rather than filter the read.
Fix: the same handler's ordering — validate, then write; DescribeChannel's channel projection needs no change once no unsupported setting can be stored. (If any path can already have stored one, the fix must not paper over it in the serializer.)
Traces: chime-sdk-messaging-elastic-channel-last-message-expiration-rejected#task#5.

## Evidence
Replay of the simparity-wire-aws-v1 corpus against the emulator built from this checkout.

Cause 1 — PutChannelExpirationSettings (fixture chime-sdk-messaging-elastic-channel-last-message-expiration-rejected, task#4):
  status: expected 400, got 200
  $.Code: expected 'BadRequest', got None (absent)
  $.Message: expected 'Elastic channels do not support expiration settings based on LastMessageTimestamp.', got None (absent)
  $headers.x-amzn-errortype: expected present (BadRequestException), got None

Cause 2 — DescribeChannel (same fixture, task#5):
  status 200 on both sides, but
  $.Channel.ExpirationSettings: expected None (absent — AWS never stored it), got {'ExpirationDays': 7, 'ExpirationCriterion': 'LAST_MESSAGE_TIMESTAMP'}

The ExpirationDays value 7 above is just what the fixture submitted; the requirement is that NO ExpirationSettings is stored or returned for an elastic channel after a LAST_MESSAGE_TIMESTAMP write, whatever the day count. AWS's refusal sentence is entirely fixed text — note that it spells the criterion as 'LastMessageTimestamp' in prose, not as the enum token 'LAST_MESSAGE_TIMESTAMP'.

## Steps to Reproduce
Region us-east-1. Create an AppInstance and an AppInstanceUser via chime-sdk-identity, then CreateChannel with an ElasticChannelConfiguration (e.g. MaximumSubChannels=2, TargetMembershipsPerSubChannel=2, MinimumMembershipPercentage=10) and Mode/Privacy as required, using the AppInstanceUser ARN as x-amz-chime-bearer.

1. PUT /channels/{ChannelArn}/expiration-settings with body {"ExpirationSettings": {"ExpirationDays": 7, "ExpirationCriterion": "LAST_MESSAGE_TIMESTAMP"}}. Expect HTTP 400, Code 'BadRequest', Message 'Elastic channels do not support expiration settings based on LastMessageTimestamp.'; the emulator answers HTTP 200.
2. GET /channels/{ChannelArn} (DescribeChannel). Expect Channel.ExpirationSettings to be absent; the emulator returns {"ExpirationDays": 7, "ExpirationCriterion": "LAST_MESSAGE_TIMESTAMP"}.
3. Control: repeat step 1 with ExpirationCriterion 'CREATED_TIMESTAMP' on the same elastic channel — that must still be accepted and must appear in DescribeChannel. And repeat step 1 on a NON-elastic channel — LAST_MESSAGE_TIMESTAMP must be accepted there.
