Issue-Source: trace-sweep-divergence
Issue-Trace: pinpoint-sms-voice-v2-protect-configuration-association-lifecycle
Issue-Step: task#10
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: pinpoint-sms-voice-v2-protect-configuration-association-lifecycle pinpoint-sms-voice-v2-protect-configuration-deletion-protection-and-single-association pinpoint-sms-voice-v2-protect-configuration-rule-set-override-lifecycle pinpoint-sms-voice-v2-event-destination-lifecycle pinpoint-sms-voice-v2-protect-configuration-number-override-lifecycle
Issue-Shape-Id: com.amazonaws.pinpointsmsvoicev2#PinpointSMSVoiceV2
Issue-Key: pinpoint-sms-voice-v2|DescribeProtectConfigurations|4xx->2xx|$.Message
Issue-Key: pinpoint-sms-voice-v2|DescribeProtectConfigurations|4xx->2xx|$.ResourceId
Issue-Key: pinpoint-sms-voice-v2|DescribeProtectConfigurations|4xx->2xx|$.ResourceType
Issue-Key: pinpoint-sms-voice-v2|DescribeProtectConfigurations|4xx->2xx|$.__type
Issue-Key: pinpoint-sms-voice-v2|DescribeProtectConfigurations|4xx->2xx|status
Issue-Key: pinpoint-sms-voice-v2|DeleteEventDestination|2xx->2xx|$.EventDestination.Enabled
Issue-Key: pinpoint-sms-voice-v2|PutProtectConfigurationRuleSetNumberOverride|4xx->4xx|$.Fields
Issue-Key: pinpoint-sms-voice-v2|PutProtectConfigurationRuleSetNumberOverride|4xx->4xx|$.Message
Issue-Key: pinpoint-sms-voice-v2|PutProtectConfigurationRuleSetNumberOverride|4xx->4xx|$.Reason
Issue-Commit: 7012ff15aecebd44e821216c3792d3327cf3e3b1

# pinpoint-sms-voice-v2: deleted and disabled resources stay live in reads, and a bad DestinationPhoneNumber is refused with an empty body

> _Note: this issue was identified against the code base at commit `7012ff15aecebd44e821216c3792d3327cf3e3b1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeProtectConfigurations

## Description
This bundle carries 3 causes, all in the pinpoint-sms-voice-v2 (AWS End User Messaging SMS and Voice, sms-voice, awsJson1_0) crate. They share one fault: a read or a refusal reports something other than the resource's current state as the service defines it — two reads answer 200 from a record that AWS no longer holds (or no longer holds in that state), and one refusal answers 400 with none of the modeled error's members. A fixer must close all 3 before submitting.

Cause 1 of 3 — a protect configuration that has been deleted is still reported with HTTP 200 instead of ResourceNotFoundException (DescribeProtectConfigurations).
AWS: Smithy (services/service-com-amazonaws-pinpointsmsvoicev2-pinpointsmsvoicev2/smithy-model/com-amazonaws-pinpointsmsvoicev2-pinpointsmsvoicev2.json) models DescribeProtectConfigurations with ProtectConfigurationIds (ProtectConfigurationIdList, len ..5) as a SELECTOR and declares ResourceNotFoundException (errorKind client, HTTP 400; members Message, ResourceType, ResourceId). An id that names no stored protect configuration — including one a prior DeleteProtectConfiguration removed — is refused, not reported. The recording shows HTTP 400, __type 'com.amazonaws.smsvoice.exceptions#ResourceNotFoundException', ResourceType 'protect-configuration', ResourceId the id taken from the request, and Message rendered from those members: the fixed text 'Resource Not Found - ResourceType="<ResourceType>" ResourceId="<ResourceId>"', where the literal prefix and the two quoted clause names are fixed and both values come from the request/state (the recorded run's id value protect-configuration-00000000000000000000000000000001 is a per-run value, not part of the requirement).
Emulator: HTTP 200 with the deleted protect configuration still present in ProtectConfigurations; no $.__type, $.Message, $.ResourceId or $.ResourceType at all.
Hypothesis: the Describe handler itself HAS the correct not-found arm (src/ops/describe_protect_configurations.rs:191-205, rendering exactly that Message through crate::state::resource_not_found_message), so it answers 200 only because the record is still in the store — DeleteProtectConfiguration is an unimplemented scaffold stub that returns 501 and removes nothing (src/ops/delete_protect_configuration.rs:179), so no protect configuration ever leaves crate::state.
Why: a client that deletes a protect configuration and then polls or re-reads it sees it alive forever — teardown loops that wait for the not-found never terminate, and code that branches on ResourceNotFoundException to confirm a delete concludes the delete failed. Worse, the stale record stays usable as a target for associations and account-default settings that AWS would refuse.
Fix: DeleteProtectConfiguration's handler and the protect-configuration state it owns — the delete must remove the record (and its dependent bookkeeping) from crate::state so the existing not-found arm in DescribeProtectConfigurations fires.
Traces: pinpoint-sms-voice-v2-protect-configuration-association-lifecycle#task#10, and the same cluster covers pinpoint-sms-voice-v2-protect-configuration-deletion-protection-and-single-association#task#16, #task#17 and pinpoint-sms-voice-v2-protect-configuration-rule-set-override-lifecycle#task#9.

Cause 2 of 3 — the deleted event destination echoed back is still Enabled=true after the destination was disabled (DeleteEventDestination).
AWS: Smithy models DeleteEventDestinationResult.EventDestination as 'The event destination object that was deleted', i.e. the record as it stood at deletion, and EventDestination.Enabled is a @required Boolean ('When set to true events will be logged'). CreateEventDestination models NO Enabled member, and UpdateEventDestination models Enabled — so a destination's enablement is whatever the last UpdateEventDestination set, and a destination disabled before deletion is echoed back with Enabled false. The recording shows HTTP 200 with $.EventDestination.Enabled false.
Emulator: HTTP 200, but $.EventDestination.Enabled is true. CreateEventDestination hard-codes enabled: true into the stored record (src/ops/create_event_destination.rs:387-397) and UpdateEventDestination is an unimplemented scaffold stub (src/ops/update_event_destination.rs:189), so a destination can never be disabled; DeleteEventDestination faithfully echoes the stored record (src/ops/delete_event_destination.rs:207-223), which has never changed.
Why: a client that turns event logging off and then tears the destination down reads back a record saying logging was still on. Audit and teardown flows that assert on the deleted object's final state (or that use the returned object to recreate the destination in the same state) carry the wrong enablement.
Fix: UpdateEventDestination's handler must persist Enabled (and the other updatable members) onto the stored destination in crate::state, so every read of it — including the record DeleteEventDestination echoes — reports the destination's current enablement.
Traces: pinpoint-sms-voice-v2-event-destination-lifecycle#task#7.

Cause 3 of 3 — a DestinationPhoneNumber that violates the modeled pattern is refused with the framework's synthetic ValidationException (no Message, Reason or Fields) instead of the service's modeled one (PutProtectConfigurationRuleSetNumberOverride).
AWS: Smithy models PutProtectConfigurationRuleSetNumberOverrideRequest.DestinationPhoneNumber as @required, target PhoneNumber, len 1..20, pattern ^\+?[1-9][0-9]{1,18}$, and the operation declares ValidationException (HTTP 400) whose members are Message, Reason (ValidationExceptionReason, which includes FIELD_VALIDATION_FAILED) and Fields (list of ValidationExceptionField{Name, Message}, both @required). The recording shows HTTP 400 with Reason 'FIELD_VALIDATION_FAILED', Message "The following parameter is null, empty or invalid: ['destinationPhoneNumber']" and Fields [{Name: 'destinationPhoneNumber', Message: 'Member must satisfy regular expression pattern: \\+?[1-9][0-9]{1,18}'}]. Fixed text: 'The following parameter is null, empty or invalid: ' followed by the bracketed, single-quoted list of offending member names, and 'Member must satisfy regular expression pattern: ' followed by the member's modeled pattern with the ^ and $ anchors stripped. Variable parts: the offending member's wire name (lowerCamel, here destinationPhoneNumber) and its pattern, both taken from the request's offending member.
Emulator: HTTP 400, but $.Message, $.Reason and $.Fields are all absent. The boundary constraint validator refuses the request with the synthetic smithy.framework#ValidationException whose only member serializes as lowercase 'message' (src/scaffolded.rs:117-178), and on awsJson1_0 this service's modeled member names are Message/Reason/Fields, so an SDK client reads no message and no field list.
Why: every AWS SDK surfaces Message for a 400, and a caller that inspects Reason or Fields to tell the user which member was wrong gets nothing; the refusal is also indistinguishable from any other constraint violation on the operation.
Fix: the service's boundary/constraint-validation error path must render THIS service's modeled ValidationException — Message from the offending member list, Reason FIELD_VALIDATION_FAILED, and one Fields entry per violated member — rather than the framework's single-member synthetic shape.
Related: the open issue 20261110T085439Z-pinpoint-sms-voice-v2-error-responses-are-emulator.md describes this same renderer defect for CreateConfigurationSet.ConfigurationSetName and PutOptedOutNumber.OptedOutNumber, but it carries no replay key for this operation (the replay reported owned_by empty for this cluster), so this cause is filed here with the fixture binding that exercises it.
Traces: pinpoint-sms-voice-v2-protect-configuration-number-override-lifecycle#task#3, and the same cluster covers pinpoint-sms-voice-v2-protect-configuration-rule-set-override-lifecycle#task#2.

## Evidence
Cause 1 — fixture pinpoint-sms-voice-v2-protect-configuration-association-lifecycle step task#10, DescribeProtectConfigurations. Expected (recorded AWS): HTTP 400, $.__type 'com.amazonaws.smsvoice.exceptions#ResourceNotFoundException', $.ResourceType 'protect-configuration', $.ResourceId the requested protect-configuration id, $.Message 'Resource Not Found - ResourceType="protect-configuration" ResourceId="<id>"'. Actual (emulator): HTTP 200 with the configuration still listed; $.__type, $.Message, $.ResourceId, $.ResourceType all missing (None). Cluster also covers pinpoint-sms-voice-v2-protect-configuration-deletion-protection-and-single-association#task#16 and #task#17 and pinpoint-sms-voice-v2-protect-configuration-rule-set-override-lifecycle#task#9. Code: src/ops/delete_protect_configuration.rs:179 (scaffold stub, removes nothing), src/ops/describe_protect_configurations.rs:191-205 (the not-found arm that never fires).

Cause 2 — fixture pinpoint-sms-voice-v2-event-destination-lifecycle step task#7, DeleteEventDestination. Expected: HTTP 200 with $.EventDestination.Enabled false (the destination had been disabled before deletion). Actual: HTTP 200 with $.EventDestination.Enabled true. Code: src/ops/create_event_destination.rs:387-397 (enabled hard-coded true), src/ops/update_event_destination.rs:189 (scaffold stub — the only path that can set Enabled), src/ops/delete_event_destination.rs:207-223 (echoes the stored record).

Cause 3 — fixture pinpoint-sms-voice-v2-protect-configuration-number-override-lifecycle step task#3, PutProtectConfigurationRuleSetNumberOverride. Expected: HTTP 400 with $.Reason 'FIELD_VALIDATION_FAILED', $.Message "The following parameter is null, empty or invalid: ['destinationPhoneNumber']", $.Fields [{'Name': 'destinationPhoneNumber', 'Message': 'Member must satisfy regular expression pattern: \\+?[1-9][0-9]{1,18}'}]. Actual: HTTP 400 with $.Message, $.Reason and $.Fields all missing (None). Cluster also covers pinpoint-sms-voice-v2-protect-configuration-rule-set-override-lifecycle#task#2. Code: src/scaffolded.rs:117-178 (synthetic ValidationException with a single lowercase 'message' member).

All paths are relative to services/service-com-amazonaws-pinpointsmsvoicev2-pinpointsmsvoicev2/.

## Steps to Reproduce
Boot the emulator and point an sms-voice (pinpoint-sms-voice-v2, awsJson1_0) client at it.

Cause 1: create_protect_configuration() -> note ProtectConfigurationId. delete_protect_configuration(ProtectConfigurationId=<id>). Then describe_protect_configurations(ProtectConfigurationIds=[<id>]). Observe HTTP 200 listing the configuration instead of a 400 ResourceNotFoundException with ResourceType 'protect-configuration' and ResourceId <id>.

Cause 2: create_configuration_set(ConfigurationSetName='setA'); create_event_destination(ConfigurationSetName='setA', EventDestinationName='dest1', MatchingEventTypes=['TEXT_DELIVERED'], SnsDestination={TopicArn: <arn>}); update_event_destination(ConfigurationSetName='setA', EventDestinationName='dest1', Enabled=false); delete_event_destination(ConfigurationSetName='setA', EventDestinationName='dest1'). Read $.EventDestination.Enabled in the delete response: true instead of false.

Cause 3: POST awsJson1_0 PutProtectConfigurationRuleSetNumberOverride with a valid ProtectConfigurationId, Action='ALLOW' and DestinationPhoneNumber='not-a-number' (violates ^\+?[1-9][0-9]{1,18}$). Observe a 400 whose JSON body has no Message, Reason or Fields key.

Re-replay: pinpoint-sms-voice-v2-protect-configuration-association-lifecycle#task#10, pinpoint-sms-voice-v2-event-destination-lifecycle#task#7, pinpoint-sms-voice-v2-protect-configuration-number-override-lifecycle#task#3.
