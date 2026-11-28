Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: d9773982178bffc206ea9531196a8841a020d5e3

# A chime meeting ARN carrying no MeetingId is tagged instead of refused as an invalid meeting id

_Filed: 2026-11-21T18:35:56Z_
_Operation: TagResource | Class: validation_

> _Note: this issue was identified against the code base at commit `d9773982178bffc206ea9531196a8841a020d5e3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::tagging_refusal` is the one place this crate decides whether a tagging call may address an ARN. It asks `service_common::arn::resource_id_of_type(arn, MEETING_RESOURCE_TYPE)` for the meeting id in the ARN and treats `None` as "the ARN names another family member's resource type or is no family ARN at all" — in which case `addresses_taggable_resource` takes the sibling's resource at its word and the call is served.

But `resource_id_of_type` also answers `None` for an ARN of THIS service's own resource type written without an id separator — `arn:aws:chime:<region>:<account>:meeting` — because it requires a `/` or `:` after the type. So that ARN, which names a meeting this store holds no meeting under, is classified as a sibling's resource and served: `TagResource` answers its modelled 204 and stores the pairs in the shared ARN-keyed directory, `ListTagsForResource` then reports those tags back as the tags of a meeting that does not exist, and `UntagResource` answers 204 too.

Every other spelling of a chime meeting ARN whose meeting the store does not hold is refused with the modelled `BadRequestException` (400, "Invalid meeting id") — including `…:meeting/` (empty id) and `…:meeting:<id>`. Only the separator-less form slips through, so one nonexistent meeting gets two opposite verdicts depending on how its ARN is spelled, and the directory accumulates tags under an ARN no `DeleteMeeting` will ever withdraw.

## Evidence

The decision, `src/state.rs:556-576` — the comment states what `None` means, and the code's `None` also covers an ARN of THIS service's own type:
```
556:    pub fn tagging_refusal(&self, arn: &str) -> Option<TaggingRefusal> {
557:        // `Some(true)`/`Some(false)` when the ARN names a MEETING of this family, `None`
558:        // when it names another member's resource type or is no family ARN at all.
559:        let own_resource_exists = ::service_common::arn::is_resource_arn_of(
...
563:        .then(|| ::service_common::arn::resource_id_of_type(arn, MEETING_RESOURCE_TYPE))
564:        .flatten()
...
570:        if ::service_common::chime_tags::addresses_taggable_resource(arn, own_resource_exists) {
571:            return None;
572:        }
573:        if own_resource_exists == Some(false) {
574:            return Some(TaggingRefusal::InvalidMeetingId);
```
The parse it relies on refuses the id-less form, `services/common/src/arn.rs:190-193`:
```
190: pub fn resource_id_of_type<'a>(value: &'a str, resource_type: &str) -> Option<&'a str> {
191:     let rest = resource(value)?.strip_prefix(resource_type)?;
192:     rest.strip_prefix('/').or_else(|| rest.strip_prefix(':'))
193: }
```
while the SAME shared parse documents that exactly this form still names a resource type, `services/common/src/arn.rs:170-176`: "The TYPE of resource the ARN `value` names: … and the whole resource field when it carries no id (`arn:aws:iot:…:job`)."

And a `None` verdict is served, not refused — `services/common/src/chime_tags.rs:292-295`:
```
292: pub fn addresses_taggable_resource(resource_arn: &str, own_resource_exists: Option<bool>) -> bool {
293:     crate::arn::is_resource_arn_of(resource_arn, CHIME_ARN_NAMESPACE)
294:         && own_resource_exists.unwrap_or(true)
```

The crate's own rule for this ARN, `src/state.rs:55-58`: "The resource TYPE a meeting's ARN carries (`…:chime:<region>:<account>:meeting/<id>`, as [`meeting_arn`] derives it). It is the one resource type this service mints, so it is the one an ARN has to carry for this store to say whether the resource exists." and `src/state.rs:193-199`: "A meeting's tags live exactly as long as the meeting, so an ARN whose meeting this store does not hold — one that never named a meeting here, or one that has ended — addresses no resource and the refusal reports the id in it as invalid."

The handler that acts on the verdict, `src/ops/tag_resource.rs:158-173`, maps only the two refusal variants and otherwise stores the pairs (`src/ops/tag_resource.rs:199-205`): `let outcome = directory.lock().tag_within(&input.resource_arn, pairs, crate::state::MAX_TAGS_PER_RESOURCE);` → `Tagged::Stored => Ok(TagResourceOutput {})`.

Existing coverage states the rule this input breaks but never spells the ARN without an id — `tests/all/resource_tagging.rs:471-475`: "RULE: a tagging call whose ARN names a meeting this store holds no meeting under is refused with the modelled 400 reporting the id as invalid and writes nothing", with rows at `tests/all/resource_tagging.rs:492-499` (`format!("{arn}x")`, the ARN in another Region, an ended meeting's ARN) — all of which carry `meeting/<id>` and are refused at lines 514-523.

## Steps to Reproduce

1. POST /tags?operation=tag-resource signed for `chime` with {"ResourceARN":"arn:aws:chime:us-east-1:123456789012:meeting","Tags":[{"Key":"k","Value":"v"}]} — actual: HTTP 204 and the pairs are written; expected by the crate's own rule (src/state.rs:193-199, tests/all/resource_tagging.rs:471-475): HTTP 400 BadRequestException with Message "Invalid meeting id", writing nothing
2. GET /tags?arn=arn%3Aaws%3Achime%3Aus-east-1%3A123456789012%3Ameeting — actual: HTTP 200 with {"Tags":[{"Key":"k","Value":"v"}]}, i.e. the tags of a meeting that was never created; expected: the same 400 refusal
3. Repeat step 1 with "arn:aws:chime:us-east-1:123456789012:meeting/" and with "arn:aws:chime:us-east-1:123456789012:meeting/abc" — both answer HTTP 400 BadRequest "Invalid meeting id", showing the two spellings of the same absent meeting get opposite verdicts
4. POST /tags?operation=untag-resource with the id-less ARN and {"TagKeys":["k"]} — HTTP 204 from the same unguarded path
