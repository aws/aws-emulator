Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: e5dfc362f7e10f15c9ea986ec22a5550bf857429

# Two stated rules about the tag-plane 404 are contradicted by the code under them: it reports no resourceId and is not worded by the one not-found template

_Filed: 2026-11-27T14:05:33Z_
_Operation: ListTagsForResource | Class: error-response_

> _Note: this issue was identified against the code base at commit `e5dfc362f7e10f15c9ea986ec22a5550bf857429`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate states, in two places, a rule about the not-found refusal the ARN-addressed tag operations (`ListTagsForResource`, `TagResource`, `UntagResource`) answer, and the code directly under each statement does the opposite.

1. `tests/all/error_message_templates.rs` heads its suite with the rules it asserts. One of them says the tag plane "states that a resource was not found and reports the ARN through `resourceId`". The assertion in the same file for exactly that case asserts the OPPOSITE: on the tag plane `(resourceId, resourceType)` must be `(None, None)`, and the same file's own constant doc 100 lines further down says the sentence names "neither the resource kind nor the ARN the request carried". So the header bullet of the suite states the reverse of what the suite enforces and of what the service does (`crate::state::tagged_resource_not_found` goes out through `Scaffolded::unmodeled` with a fixed sentence and no members at all).

2. `src/state.rs` documents `resource_not_found` as "The ONE place this service words a not-found: every resource it cannot resolve is reported with the same sentence ... so a caller reads one template whichever resource went missing". Thirty lines below, the same file words a SECOND not-found for the tag plane — `TAGGED_RESOURCE_NOT_FOUND` / `tagged_resource_not_found` — with a different sentence carrying neither the resource nor the ARN, and three of this service's nine operations answer that one. A caller therefore reads two templates, not one.

The RESPONSE must not change: the unmodelled `NotFoundException` with its fixed, member-less sentence is pinned by six suites in this crate and by the resolution of earlier issues. What is defective is the two rule statements, which tell a reader (and a future fixer) the opposite of what the code does — the same drift already filed for the `TagResource`/`UntagResource` handler headers, at the two further sites that issue explicitly left out ("`ListTagsForResource`'s own header doc, by contrast, makes no such claim").

## Evidence

Stated rule #1 — tests/all/error_message_templates.rs:22-25:
```
//! * a resource this service cannot resolve is a 404: the contact's own readers and
//!   writers state `<Resource> with arn <arn> not found`, and the tag plane — which takes
//!   the ARN of any taggable resource — states that a resource was not found and reports
//!   the ARN through `resourceId`;
```
Contradicted by the assertion for that very case in the same file, tests/all/error_message_templates.rs:516-524:
```
            let reported_id = body.get("resourceId").and_then(Value::as_str);
            let reported_type = body.get("resourceType").and_then(Value::as_str);
            if on_the_tag_plane {
                assert_eq!(
                    (reported_id, reported_type),
                    (None, None),
                    "case {case:?}: the tagging frontend's refusal carries no resource members, \
                     got {body:?}"
                );
```
and by the same file's own constant doc, tests/all/error_message_templates.rs:462-464:
```
/// What the tag plane states for a resource it holds no record of: fixed text, naming neither
/// the resource kind nor the ARN the request carried.
const TAGGED_RESOURCE_NOT_FOUND: &str = "Resource was not found";
```

Stated rule #2 — src/state.rs:646-653:
```
/// The modelled 404 for the `title` resource named by `arn`, which this store does not
/// hold, reported under `resource_type`.
///
/// The ONE place this service words a not-found: every resource it cannot resolve is
/// reported with the same sentence — the resource as this service names it, the ARN the
/// request carried, and the fact that nothing resolves it — so a caller reads one
/// template whichever resource went missing.
fn resource_not_found(title: &str, arn: &str, resource_type: &str) -> ResourceNotFoundException {
```
Contradicted by the second not-found wording in the same file, src/state.rs:680-700:
```
/// What the tag plane states for a resource it holds no record of: FIXED text, naming
/// neither the resource it looked for nor the ARN the request asked about.
...
const TAGGED_RESOURCE_NOT_FOUND: &str = "Resource was not found";
...
pub fn tagged_resource_not_found<E>() -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::unmodeled(
        TAGGING_NOT_FOUND_CODE,
        TAGGING_NOT_FOUND_STATUS,
        TAGGED_RESOURCE_NOT_FOUND,
    )
```
and that second wording is what the three tag operations actually answer — src/ops/list_tags_for_resource.rs:162-164:
```
        let contact = store
            .contact(&input.arn)
            .ok_or_else(crate::state::tagged_resource_not_found)?;
```
(identical at src/ops/tag_resource.rs:181-183 and src/ops/untag_resource.rs:179-181).

Test coverage proving the response side is the pinned one and only the prose may move: tests/all/error_message_templates.rs:471-536 (`an_unresolvable_contact_is_reported_with_the_not_found_template`), plus tests/all/resource_tags.rs, tests/all/resource_tag_writes.rs, tests/all/tag_mutation_success_status.rs, tests/all/tag_key_constraints.rs, tests/all/untag_resource_tag_keys.rs and tests/all/refusal_wording.rs, all of which assert the tag plane's `NotFoundException` with its fixed sentence.

## Steps to Reproduce

1. Read tests/all/error_message_templates.rs lines 22-25: the suite's stated rule says the tag plane's 404 "reports the ARN through `resourceId`".
2. Read the assertion for that case in the same file, lines 513-524: on the tag plane it requires `(resourceId, resourceType) == (None, None)` — the opposite of the header bullet — and lines 462-464 of the same file say the sentence names neither the resource kind nor the ARN.
3. Read src/state.rs lines 646-653: `resource_not_found` is documented as "The ONE place this service words a not-found", with "every resource it cannot resolve" reported through that one template.
4. Read src/state.rs lines 680-700 in the same file: a second not-found is worded there for the tag plane, with a different, member-less sentence, and src/ops/list_tags_for_resource.rs:162-164 (plus the two tag mutations) shows that is the refusal three of this service's operations actually answer.
5. Conclude the prose is the stale side: the member-less `NotFoundException` is asserted by six suites in this crate, so the two rule statements are what must be corrected — the response must stay exactly as it is.
