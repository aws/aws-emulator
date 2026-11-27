Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: b67daa45c46ff33986818a3ecce820dd32cc81e2

# A tag-map refusal is attributed by splitting the caller's own tag key on '.', so a key ending in ".member" is reported against `tags` instead of the entry — the one case the helper's own doc says it keeps unchanged

_Filed: 2026-11-25T09:38:04Z_
_Operation: TagResource | Class: error-response_

> _Note: this issue was identified against the code base at commit `b67daa45c46ff33986818a3ecce820dd32cc81e2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate words every framework-level constraint refusal itself (`crate::state::constraint_validation_error`, reached from `Service::override_refusal` / `ModelledRefusal`). Inside it, `clause()` decides whether a violation belongs to a COLLECTION ELEMENT by asking `collection_holding(&violation.path)`, and when that answers `Some(collection)` it REPLACES the member path with the collection's and bundles the requirement as `Member must satisfy constraint: [<requirement>]`.

`collection_holding` recognises an element by stripping a trailing `member` segment and then taking everything before the LAST remaining dot. For a map, the segment before `member` is the caller's own tag key, echoed verbatim by the framework — so the function is reading caller-chosen text as structure.

The framework does NOT append `member` when the map VALUE is what failed: it reports the entry by its key alone (`tags.<key>`). `collection_holding`'s own doc states exactly that, and states that this is the one case it cannot place and therefore leaves alone. But a tag key that itself ends in `.member` produces the path `tags.<prefix>.member`, which the function cannot tell from a real entry step: it answers `Some("tags")`, so the refusal names the map member `tags` and words the clause as if the 256-character limit applied to the map's elements, instead of keeping the path the framework built (`tags.<key>`) as the doc promises.

The modelled `TagKey` pattern `^[\w.:/=+-@]+$` admits `.`, so `env.member` is a perfectly valid tag key, and `TagValue` has a violable `@length(0..256)`. The result is that two requests differing only in the spelling of a caller-chosen key get different member paths in `fieldList`, one of which points at a member the caller did not get wrong. The same renderer serves `CreateEnvironment`'s `tags` map, so the defect reaches every caller-keyed map of this service.

The sibling function in the same module is already hardened against exactly this hazard — `requirement_of` takes the LAST marker occurrence precisely because "for a map entry, whose path segment is the key as sent", caller text can fall inside the path — while `collection_holding`, consuming the same path, splits it without that care.

## Evidence

The rule the helper states for a map value, services/service-com-amazonaws-evs-amazonelasticvmwareservice/src/state.rs:4204-4207:

    /// A map value that fails a constraint of its own shape is the one case this cannot
    /// place: the framework names it by its key alone (`tags.someKey`), with no collection
    /// step to tell it apart from a member of a structure, so it keeps the path the framework
    /// built.

The code under it, state.rs:4208-4212:

    fn collection_holding(path: &str) -> Option<&str> {
        let through_step = path.strip_suffix(COLLECTION_STEP)?.strip_suffix('.')?;
        let (collection, _element) = through_step.rsplit_once('.')?;
        Some(collection)
    }

For `path = "tags.env.member"` (a value violation on the key `env.member`) this answers `Some("tags")`, not `None` — so the path is replaced and the requirement bundled, state.rs:4093-4100:

    let (path, requirement, element) = match collection_holding(&violation.path) {
        Some(collection) => (
            collection.to_string(),
            format!("Member must satisfy constraint: [{requirement}]"),
            true,
        ),
        None => (violation.path.clone(), requirement, false),
    };

The framework really does name a failing map VALUE by its key alone, vendor/smithy-schema-server/src/constraints.rs:1209-1213:

                    // The entry's value IS what failed. A service reports this at
                    // the map member with a bundled requirement and no key at all;
                    // we report the key instead. See
                    // `map_value_pattern_reports_key_path`.
                    CollectionChild::Terminal => Some(key),

and its own test pins that path, vendor/smithy-schema-server/src/constraints.rs:2283:

        assert_eq!(violations[0].path, "tags.env");

while a real entry step is the key plus `member`, vendor/smithy-schema-server/src/constraints.rs:91-96:

    /// A map entry names its key, then the collection step. The key is a value
    /// the CALLER chose, so it is echoed exactly as sent — never re-cased the
    /// way a model name is.
    pub fn entry(key: &str) -> String {
        format!("{key}.{COLLECTION_STEP}")
    }

The sibling in the same module that IS hardened against caller text in that same segment, state.rs:4140-4145:

    /// that clause. Both halves of the framework's clause can carry text the CALLER chose:
    /// the value it rejected, and — for a map entry, whose path segment is the key as sent —
    /// the member path. A requirement never contains the marker, so the last occurrence is
    /// the real separator whatever either half spells, while the first could fall inside
    /// caller text and copy the rest of it into the message.

The model makes the input reachable (`smithy_query(query="shape", shape="RequestTagMap")`): key `TagKey` `length 1..128`, `pattern ^[\w.:/=+-@]+$` (a dot is admitted); value `TagValue` `length 0..256`.

Test coverage: tests/all/validation_message_rendering.rs owns this renderer's envelope, but every collection row is a LIST element — e.g. lines 265-304 expect `Value at 'settings.1.member.value' …` — and lines 464-491 cover a list-of-enums element and a plain member. No row carries a map entry at all, so neither the documented map-value path nor the `.member`-suffixed key is pinned by any test.

## Steps to Reproduce

1. Create an environment (CreateEnvironment with a valid vpcId/serviceAccessSubnetId/initialVlans) and let it settle to CREATED; note its environmentArn.
2. Call TagResource with that resourceArn and tags = { "env.other": "<257 characters of [A-Za-z]>" }. The boundary validator refuses it 400 ValidationException and the fieldList entry names `tags.env.other` — the path the framework built, as `collection_holding`'s doc says it keeps.
3. Repeat the call with tags = { "env.member": "<257 characters of [A-Za-z]>" } — the same violation, on a key the modelled TagKey pattern equally admits.
4. The refusal now names `tags` in fieldList and words the clause as `Value at 'tags' failed to satisfy constraint: [Member must satisfy constraint: [...]]`, i.e. it is reported as a violation of the map's elements instead of keeping `tags.env.member` — contradicting the helper's own stated rule, and telling the caller to fix a member that is not what failed.
5. The same substitution in CreateEnvironment's `tags` member reproduces it there, since both go through crate::state::constraint_validation_error.
