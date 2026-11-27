Issue-Source: trace-sweep-divergence
Issue-Trace: pinpoint-sms-voice-v2-tag-resource-lifecycle
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: pinpoint-sms-voice-v2-tag-resource-lifecycle
Issue-Shape-Id: com.amazonaws.pinpointsmsvoicev2#PinpointSMSVoiceV2
Issue-Key: pinpoint-sms-voice-v2|ListTagsForResource|2xx->2xx|$.Tags[*]
Issue-Commit: 3996f901063d22c400f1655bb242a5d1c6eab093

# pinpoint-sms-voice-v2 tag mutations never reach a resource's tag set, so ListTagsForResource reports the create-time tags forever

> _Note: this issue was identified against the code base at commit `3996f901063d22c400f1655bb242a5d1c6eab093`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListTagsForResource

## Description
The tag plane of this service is read-only: a resource's tag set is written once, when the resource is created, and no later tag mutation is ever reflected. ListTagsForResource answers 200 with the stale create-time set, so the whole tag lifecycle (add a tag, then remove a tag) is unobservable. 2 causes, both on the same fixture.

Cause 1 of 2 - a tag added after create is absent from ListTagsForResource (fixture pinpoint-sms-voice-v2-tag-resource-lifecycle, step task#4).
AWS: Smithy model com.amazonaws.pinpointsmsvoicev2 - TagResource documents "Adds or overwrites only the specified tags for the specified resource. When you specify an existing tag key, the value is overwritten with the new value. Each tag consists of a key and an optional value. Tag keys must be unique per resource." TagResource takes ResourceArn (AmazonResourceName, @required, length 20..256, pattern ^arn:[A-Za-z0-9_:/-]+$) and Tags (NonEmptyTagList, @required, length 1..200) and returns an empty TagResourceResult on success; ListTagsForResource returns ResourceArn plus Tags (TagList, length max 200). So after a successful TagResource that adds a second key to a resource that already carries one, ListTagsForResource must report BOTH key/value pairs - the union of the create-time set and the added tag, with the added key's value overwriting any same-key entry.
Emulator: ListTagsForResource answers 200 but its Tags list holds only the single create-time entry; the second, later-added tag is missing entirely ($.Tags[1]: expected a {Key, Value} entry, got None). The underlying cause is that the tag set lives only on the stored resource record (services/service-com-amazonaws-pinpointsmsvoicev2-pinpointsmsvoicev2/src/state.rs - ConfigurationSet/OptOutList/PhoneNumberRecord/ProtectConfiguration/RcsAgent/Registration/RegistrationAttachment each own a `tags: Vec<Tag>`, read back by `tags_at`, state.rs:881), and the state module exposes NO mutator for it, so nothing can change a record's tags after the create that populated them.
Why: a client that tags a configuration set (or any taggable sms-voice resource) and then reads its tags back sees the tag it just wrote silently missing. Tag-driven inventory, cost allocation and tag-condition logic all read an incomplete set, and an idempotent "ensure this tag exists" loop never converges because the write reports success while the read never shows it.
Fix: the tag write path and the per-resource tag state - a tag mutator on ComAmazonawsPinpointsmsvoicev2Pinpointsmsvoicev2State that upserts by key into the addressed record's `tags` (resolving the ARN exactly as `tags_at`/`resource_at` already do), called from the TagResource handler. ListTagsForResource's own handler already projects whatever the record holds and needs no change beyond that.
Traces: pinpoint-sms-voice-v2-tag-resource-lifecycle#task#4 (expected 200 with 2 tags, got 200 with 1).

Cause 2 of 2 - a tag removed by UntagResource is still reported, and the surviving tag is not (fixture pinpoint-sms-voice-v2-tag-resource-lifecycle, step task#6).
AWS: Smithy model com.amazonaws.pinpointsmsvoicev2 - UntagResource documents "Removes the association of the specified tags from a resource." and takes ResourceArn (@required) plus TagKeys (TagKeyList, @required, length 1..200), returning an empty UntagResourceResult. After a successful UntagResource naming one key of a two-key resource, ListTagsForResource must report exactly the remaining key/value pair and must NOT report the removed key.
Emulator: ListTagsForResource answers 200 with the removed key still present and the surviving key absent - at $.Tags[0] the expected remaining entry is missing and the entry that should have been deleted is reported instead (expected {Key: <surviving key>, Value: <its value>}, got {Key: <removed key>, Value: <its value>}). This is the same read-only tag state as cause 1: the untag never lands, so the resource still carries precisely its create-time set.
Why: a client cannot retract a tag. Removing a tag used as an access-control or cost-allocation condition appears to succeed yet the tag keeps reading back, so cleanup and re-tagging workflows observe the wrong tag set indefinitely.
Fix: the same tag state and the UntagResource handler - the mutator must delete the named keys from the addressed record's `tags` and leave the rest, and keys not present must be a no-op rather than an error.
Traces: pinpoint-sms-voice-v2-tag-resource-lifecycle#task#6 (expected 200 with only the surviving tag, got 200 with only the removed tag).

Note on related open issues: no existing open pinpoint-sms-voice-v2 issue covers the tag plane - 20261120T235619Z-pinpoint-sms-voice-v2-deleted-and-disabled-resources.md and 20261126T183918Z-pinpoint-sms-voice-v2-deleteprotectconfiguration-refuses-a-deletable-protect-configuration-with-400.md are about deleted/disabled resource reads and DeleteProtectConfiguration, and 20261120T210447Z-pinpoint-sms-voice-v2-associateprotectconfiguration-stub.md is a different operation's stub. The TagResource and UntagResource handlers themselves are still scaffold stubs that answer 501 (services/.../src/ops/tag_resource.rs:166, src/ops/untag_resource.rs), which is why nothing mutates the state; the divergence filed here is the observable one on the IMPLEMENTED read, ListTagsForResource, which must reflect those writes once they exist.

## Evidence
Replay of fixture pinpoint-sms-voice-v2-tag-resource-lifecycle against the emulator (trace set simparity-wire-aws-v1):

Step task#4 - ListTagsForResource, expected HTTP 200, got HTTP 200. Body diff: $.Tags[1]: missing (expected {'Key': 'team', 'Value': 'sms'}, got None). The recorded response lists two tag entries; the emulator lists one (only the create-time tag).

Step task#6 - ListTagsForResource, expected HTTP 200, got HTTP 200. Body diff: $.Tags[0]: missing (expected {'Key': 'team', 'Value': 'sms'}, got {'Key': 'env', 'Value': 'test'}) and $.Tags[0]: extra (same pair). After the untag of key 'env', the recorded response lists only 'team'; the emulator lists only 'env'.

(The recorded key/value strings above are this fixture's values, not a requirement: the requirement is set algebra - after TagResource the reported set is the union with same-key overwrite, after UntagResource it is the set minus the named keys.)

Status and error code do not diverge on either step (200 vs 200, no error body); the divergence is entirely the Tags projection.

Code: services/service-com-amazonaws-pinpointsmsvoicev2-pinpointsmsvoicev2/src/state.rs:881 `tags_at` clones the tag vector off the stored record, and no method on the state writes that vector after the create; src/ops/tag_resource.rs:166 and src/ops/untag_resource.rs still return Scaffolded::not_implemented.

## Steps to Reproduce
1. Boot the emulator from this checkout and point an sms-voice (pinpoint-sms-voice-v2) client at it.
2. Create a taggable resource with one tag, e.g. CreateConfigurationSet with Tags=[{Key: env, Value: test}], and note the returned ConfigurationSetArn.
3. Call TagResource{ResourceArn: <that ARN>, Tags: [{Key: team, Value: sms}]}.
4. Call ListTagsForResource{ResourceArn: <that ARN>} -> cause 1: the reply carries only {env,test}; the AWS recording carries both {env,test} and {team,sms}.
5. Call UntagResource{ResourceArn: <that ARN>, TagKeys: [env]}.
6. Call ListTagsForResource{ResourceArn: <that ARN>} again -> cause 2: the reply carries only {env,test}; the AWS recording carries only {team,sms}.

Or replay the fixture directly: pinpoint-sms-voice-v2-tag-resource-lifecycle, steps task#4 and task#6.
