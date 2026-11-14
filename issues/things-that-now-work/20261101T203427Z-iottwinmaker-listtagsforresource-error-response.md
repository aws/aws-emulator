Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: cd1ff1c075c03af4b96f1d8fe21bfc329bc355d4

# The tagging plane's not-found refusal interpolates the 2048-character resourceARN without the crate's clamp, so ResourceNotFoundException.message leaves the modelled @length(0..2048)

_Filed: 2026-11-01T20:34:27Z_
_Operation: ListTagsForResource | Class: error-response_

> _Note: this issue was identified against the code base at commit `cd1ff1c075c03af4b96f1d8fe21bfc329bc355d4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListTagsForResource`, `TagResource` and `UntagResource` all address a resource by `resourceARN`, whose target `TwinMakerArn` is `@length(min: 20, max: 2048)`. When the ARN names no resource the store holds, all three refuse through the shared `crate::state::resource_not_found_message`, which wraps the ARN in 17 fixed characters (`"Resource "` + `" not found."`). Their `ResourceNotFoundException$message` targets `ErrorMessage`, which is `@length(min: 0, max: 2048)` — so an ARN anywhere above 2031 characters produces a `message` longer than the member admits, and a 2048-character ARN produces one of 2065 characters.

None of the three handlers passes the message through `crate::state::clamped_message`, the helper this crate wrote for exactly this case and whose rule the sibling `DeleteComponentType` states applies to "every other refusal of this service". The boundary validator does not police output, so the over-long message reaches the wire.

This is one shared-helper defect on one call path; it is filed against `ListTagsForResource` as the first of the three tagging operations that calls it, and the same unclamped call sits in `tag_resource.rs:211` and `untag_resource.rs:197`.

## Evidence

THE MODEL — `smithy_query(query="operation", operation="ListTagsForResource")`:
```
"resourceARN": {"target": "TwinMakerArn", "required": true, "length": {"min": 20, "max": 2048}, "pattern": "^arn:((aws)|(aws-cn)|(aws-us-gov)):iottwinmaker:[a-z0-9-]+:[0-9]{12}:[\\/a-zA-Z0-9_\\-\\.:]+$"}
...
"errors": [{"shape": "AccessDeniedException"...}, {"shape": "ResourceNotFoundException", "errorKind": "client", "httpError": 404}]
```
and `smithy_query(query="shape", shape="ResourceNotFoundException,ErrorMessage")`:
```
"ResourceNotFoundException": {... "members": {"message": {"target": "ErrorMessage", "length": {"min": 0, "max": 2048}}}}
"ErrorMessage": {"shape_id": "com.amazonaws.iottwinmaker#ErrorMessage", "type": "string", "length": {"min": 0, "max": 2048}}
```
(`TagResource` and `UntagResource` carry the identical `resourceARN` member and the identical `ResourceNotFoundException`.)

THE SHARED BUILDER — services/service-com-amazonaws-iottwinmaker-awsiottwinmaker/src/state.rs:4788-4795:
```
/// The message for a resource no ARN in this service's index names.
///
/// Built from the request, naming the ARN that was looked for — the same
/// construction as [`entity_not_found_message`], one level up from an id to
/// the ARN the tagging operations address a resource by.
pub fn resource_not_found_message(resource_arn: &str) -> String {
    format!("Resource {resource_arn} not found.")
}
```

THE THREE UNCLAMPED CALL SITES — src/ops/list_tags_for_resource.rs:186-193:
```
        let Some(tags) = state.tags_for_resource(resource_arn) else {
            return Err(ListTagsForResourceErrorModeled::ResourceNotFoundException(
                ResourceNotFoundException {
                    message: Some(crate::state::resource_not_found_message(resource_arn)),
                },
            )
            .into());
        };
```
src/ops/tag_resource.rs:208-215:
```
        let Some(tags) = state.tags_for_resource_mut(&resource_arn) else {
            return Err(TagResourceErrorModeled::ResourceNotFoundException(
                ResourceNotFoundException {
                    message: Some(crate::state::resource_not_found_message(&resource_arn)),
                },
            )
            .into());
```
src/ops/untag_resource.rs:194-201:
```
        let Some(tags) = state.tags_for_resource_mut(&resource_arn) else {
            return Err(UntagResourceErrorModeled::ResourceNotFoundException(
                ResourceNotFoundException {
                    message: Some(crate::state::resource_not_found_message(&resource_arn)),
                },
            )
            .into());
```

THE RULE THE CRATE STATES FOR ITSELF — src/ops/delete_component_type.rs:236-249:
```
        // ... The refusal names as many ids as
        // reference it, so it is stated within the bound the member it travels
        // on declares ([`crate::state::clamped_message`]), as every other
        // refusal of this service is.
...
                    message: Some(crate::state::clamped_message(
                        message,
                        crate::state::ERROR_MESSAGE_MAX_CHARS,
                    )),
```
and the helper that exists for exactly this case — src/state.rs:4754-4760:
```
/// A message names what the request named, and an identifier the model bounds
/// as widely as — or more widely than — the member the message travels on
/// renders text past that member's own bound: a `ComponentPath` of 2048
/// characters, or a `Name` of 256, stated on a member capped at 256.
```

TEST COVERAGE: `tests/all/resource_tags.rs:783-795` asserts only the status and error code of the three tagging not-found refusals (`"case {case:?}: TagResource expected a 404 ResourceNotFoundException, got {tagged:?}"`, and the same for ListTagsForResource and UntagResource) and never their `message`. `tests/all/error_message_bounds.rs:33-34` scopes its message-bound rows to "the five operations that accept a `componentPath`", none of which is a tagging operation, so nothing pins these messages against `ErrorMessage`'s maximum.

## Steps to Reproduce

1. Start the emulator; create nothing (or create an unrelated workspace).
2. Build a legal but unheld ARN of exactly 2048 characters: `arn:aws:iottwinmaker:us-east-1:123456789012:` followed by enough `a` characters to reach 2048 — admitted by `TwinMakerArn`'s `@length(20..2048)` and its `[\/a-zA-Z0-9_\-\.:]+` resource pattern.
3. ListTagsForResource(resourceARN="<that 2048-char ARN>").
4. Observe HTTP 404 `ResourceNotFoundException` whose `message` is 2065 characters ("Resource " + 2048 + " not found.") — 17 characters past the `@length(0..2048)` of `ErrorMessage`, the member it travels on.
5. Repeat with TagResource(resourceARN=<same>, tags={"k":"v"}) and UntagResource(resourceARN=<same>, tagKeys=["k"]): both answer the same over-long message, because src/ops/tag_resource.rs:211 and src/ops/untag_resource.rs:197 call `resource_not_found_message` without `clamped_message`, unlike src/ops/delete_component_type.rs:246.
