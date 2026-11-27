Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: f667faacbc6ecad8870388d49cf0b2c06890cb64

# The tagging-ARN refusal rule is written out three times across TagResource, UntagResource and ListTagsForResource

_Filed: 2026-11-19T23:36:12Z_
_Operation: TagResource | Class: maintainability_

> _Note: this issue was identified against the code base at commit `f667faacbc6ecad8870388d49cf0b2c06890cb64`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The branch that decides how a tagging call is refused — `state.lock().tagging_refusal(arn)` mapped from `TaggingRefusal::InvalidMeetingId`/`UnresolvedResource` onto `BadRequestException`/`ResourceNotFoundException`, followed by the shared directory's `withdrawn_refusal(arn)` mapped from `WithdrawnClass::InvalidRequest`/`Forbidden` onto `BadRequestException`/`ForbiddenException` — is written out once per tagging handler, three times in this crate, differing only in the name of the generated error enum (~35 lines per copy). The crate already centralises the PAYLOADS of these refusals in `state.rs` for exactly this reason, but not the two-step branch that chooses between them, so the three copies can drift (one handler gaining a case or swapping the order of the two steps would make one ARN get two verdicts depending on which route was called). A fix must leave every response and every existing test unchanged: resolve the verdict once (e.g. a helper answering the `TaggingRefusal`/`WithdrawnClass` decision for an ARN) and let each handler only wrap it in its own error enum.

## Evidence

The crate states the intent it does not yet apply to the branch — src/state.rs:307-311:
```
/// `ResourceNotFoundException`'s payload for a tagging call whose `ResourceARN` names no
/// resource this service holds. Read by all three generic tagging operations, so the
/// three cannot word one refusal three ways.
```

Copy 1 — src/ops/tag_resource.rs:158-192:
```
158        if let Some(refusal) = state.lock().tagging_refusal(&input.resource_arn) {
159            return Err(match refusal {
163                crate::state::TaggingRefusal::InvalidMeetingId => {
164                    TagResourceErrorModeled::BadRequestException(crate::state::invalid_meeting_id())
166                crate::state::TaggingRefusal::UnresolvedResource => {
167                    TagResourceErrorModeled::ResourceNotFoundException(
...
178        let withdrawn = directory
179            .lock()
180            .withdrawn_refusal(&input.resource_arn)
```

Copy 2 — src/ops/untag_resource.rs:164-202, the same two blocks with `UntagResourceErrorModeled`:
```
164        if let Some(refusal) = state.lock().tagging_refusal(&input.resource_arn) {
...
186        let withdrawn = directory
187            .lock()
188            .withdrawn_refusal(&input.resource_arn)
```

Copy 3 — src/ops/list_tags_for_resource.rs:159-199, the same two blocks with `ListTagsForResourceErrorModeled`:
```
159        if let Some(refusal) = state.lock().tagging_refusal(&input.resource_arn) {
...
181        let withdrawn = directory
182            .lock()
183            .withdrawn_refusal(&input.resource_arn)
```

Existing tests that must stay green unchanged: tests/all/resource_tagging.rs — `tags_round_trip_through_the_shared_tagging_routes` (line 242), `tags_supplied_at_create_time_are_listed_and_can_be_changed` (line 375) and `a_tagging_call_this_service_cannot_serve_is_refused_and_writes_nothing` (line 477).

## Steps to Reproduce

1. Open src/ops/tag_resource.rs:158-192 and src/ops/untag_resource.rs:164-202 side by side: the two blocks differ only in the error-enum name
2. Open src/ops/list_tags_for_resource.rs:159-199 for the third copy of the same two blocks
3. grep -n 'withdrawn_refusal' services/service-com-amazonaws-chimesdkmeetings-chimemeetingssdkservice/src: one occurrence in each of the three handlers and nowhere else in the crate
