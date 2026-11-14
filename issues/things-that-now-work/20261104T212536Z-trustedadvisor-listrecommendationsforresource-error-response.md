Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: d9cf74aafe764d4a8806a821a1189d14d75bb9bb

# ValidationException clause echoes the framework's elided value for an over-256-byte invalid checkArn/awsResourceArn, on a premise the model refutes

_Filed: 2026-11-04T21:25:36Z_
_Operation: ListRecommendationsForResource | Class: error-response_

> _Note: this issue was identified against the code base at commit `d9cf74aafe764d4a8806a821a1189d14d75bb9bb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate words its OWN `ValidationException` message for every operation (`InModelledEnvelope` in `src/service.rs`), and deliberately leaves one thing to the framework: the value a clause echoes, when the framework's constraint walk stored it shortened past 256 bytes as `<prefix>... [<n> bytes]`. The crate states that marker is "text AWS never emits" and then justifies leaving it alone with a factual claim about this service's own model: "Every constrained member this service models is far shorter than that cap except `nextToken` (`@length(4, 10000)`), so the shortened echo is reachable only by a value that is itself past a modelled maximum" (src/service.rs:263-265), repeated in src/service/validation.rs:81-82 and in the suite at tests/all/validation_message_rendering.rs:534-535.

The model refutes that premise. Three NON-sensitive input members of this service declare `@length` maximum 2048 — eight times the 256-byte cap — and each also carries a `@pattern`:

* `ListRecommendationsForResource$awsResourceArn` (`@httpLabel`, len 20..2048, pattern `^arn:aws(-\w+)*:...$`)
* `ListRecommendationsForResource$checkArn` (`@httpQuery(checkArn)`, len 20..2048, pattern `^arn:[\w-]+:trustedadvisor:::check\/[\w-]+$`)
* `RecommendationResourceExclusion$arn` (BatchUpdateRecommendationResourceExclusion entries, len 20..2048, same shape family)

So a 300-byte value that is INSIDE its modelled maximum but breaks the `@pattern` raises a Pattern violation whose stored echo is already elided, and this service's own sentence then interpolates `<256-byte prefix>... [300 bytes]` — exactly the wire text the crate says AWS never emits — for a request that never exceeded any modelled maximum. (`updateReason`, the fourth member past the cap at `@length(10, 4096)`, is `@sensitive`, so it is never echoed and is not affected.)

The second half of the stated reason does not hold either: the obstacle named is that "recovering it needs a second walk of the same input", but `modelled_refusal` ALREADY makes a second walk of that same built input on the same line that words the refusal — `rendered_collections(&input, O::INPUT_SCHEMA)` (src/service.rs:306), described two paragraphs earlier as "ONE reader of the SAME built input, which checks nothing" (src/service.rs:254-256). So within this crate the premise that bounds the gap is wrong, and the mechanism it says is missing is already running.

Nothing here asks for a new bound or a new rendering rule: the defect is that the reachability claim the crate rests on is false for its own `@length(20, 2048)` members, so a class of requests the crate believes impossible is answered with a message it classifies as wrong.

## Evidence

The premise, in the function that words every pre-handler refusal — services/service-com-amazonaws-trustedadvisor-trustedadvisor/src/service.rs:258-265:
```
/// What it does NOT do is restore an elided value echo. The framework's walk stores the
/// rejected value shortened past 256 bytes (a prefix plus `... [<n> bytes]`), which is
/// text AWS never emits; recovering it needs a second walk of the same input, and the
/// place that belongs is the frozen crate — filed as
/// `issues/things-that-do-not-work/20261022T133000Z-gap-smithy-schema-server-constraint-violation-stores-only-the-elided-rejected-value.md`.
/// Every constrained member this service models is far shorter than that cap except
/// `nextToken` (`@length(4, 10000)`), so the shortened echo is reachable only by a value
/// that is itself past a modelled maximum.
```
Same claim in the module that owns the words — src/service/validation.rs:77-82:
```
//! What this module does NOT do is lengthen the SCALAR echo the framework stores: past
//! 256 bytes the frozen crate keeps a prefix plus `... [<n> bytes]`, which is text AWS
//! never emits, and carrying the whole value belongs in that crate — filed as
//! `issues/.../20261022T133000Z-gap-...md`.
//! Only `nextToken` (`@length(4, 10000)`) can hold a value that long at all, and only
//! past its own modelled maximum.
```
and in the suite — tests/all/validation_message_rendering.rs:529-535:
```
/// Every row's value is inside the framework's own 256-byte echo cap, which is the only
/// size at which the echo is the submitted bytes: past it the framework stores a prefix
/// plus `... [<n> bytes]`, ...
/// Only `nextToken` (`@length(4, 10000)`) can reach that cap at all, and only with a value
/// already past its modelled maximum.
```

The model, which refutes it — `smithy_query(query="operation", operation="ListRecommendationsForResource")`:
```
"awsResourceArn": {"target": "AwsResourceArn", "httpLabel": true, "required": true, "length": {"min": 20, "max": 2048}, "pattern": "^arn:aws(-\\w+)*:[\\w\\d-]+:([\\w\\d-]*)?:[\\w\\d_-]*([:/].+)*$"},
"checkArn": {"target": "CheckArn", "httpQuery": "checkArn", "length": {"min": 20, "max": 2048}, "pattern": "^arn:[\\w-]+:trustedadvisor:::check\\/[\\w-]+$"}
```
and `smithy_query(query="shape", shape="RecommendationResourceExclusion")`:
```
"arn": {"target": "RecommendationResourceArn", "required": true, "length": {"min": 20, "max": 2048}, "pattern": "^arn:[\\w-]+:trustedadvisor::\\d{12}:recommendation-resource\\/[\\w-]+\\/[\\w-]+$"}
```
A 300-byte value for any of these satisfies `@length(20, 2048)` and fails `@pattern`, so it is past the 256-byte echo cap WITHOUT being past a modelled maximum.

The second walk the comment says is missing, already on the same path — src/service.rs:254-256 and :306:
```
/// The violations are then put through ONE reader of the SAME built input, which checks
/// nothing: [`rendered_collections`] renders each collection member the way a clause
/// states it, which the framework's walk stores as an element count.
```
```
    let message = refusal_message(&violations, &rendered_collections(&input, O::INPUT_SCHEMA))?;
```

Test coverage: tests/all/validation_message_rendering.rs:537-584 drives only values "inside the framework's own 256-byte echo cap" (its longest row is `"t".repeat(250)` at :540), so no row pins the over-cap answer either way — the behaviour is unpinned, and the stated reason for leaving it is the premise quoted above.

## Steps to Reproduce

1. Start the emulator with EMULATOR_TRUSTEDADVISOR_SUPPORT_PLAN=business so the support-level gate admits requests (the refusal here precedes the gate anyway).
2. GET /v1/recommendations-for-resource/arn%3Aaws%3Aec2%3Aus-east-1%3A<emulator account>%3Ainstance%2Fi-0abc1de23f4567890?checkArn=xxxxx…x — with a `checkArn` of 300 'x' characters: inside the modelled @length(20, 2048) maximum, outside the modelled @pattern.
3. Observe HTTP 400 ValidationException whose message clause echoes a 256-byte prefix of the value followed by `... [300 bytes]` — the `... [<n> bytes]` marker src/service.rs:259 and src/service/validation.rs:78 call "text AWS never emits" — even though the value never exceeded a modelled maximum, contradicting src/service.rs:263-265.
4. Repeat with a 300-byte invalid `awsResourceArn` in the path label, and with a 300-byte invalid `arn` in a PUT /v1/batch-update-recommendation-resource-exclusion entry, for the same elided echo from the other two members whose @length maximum is 2048.
5. Compare with the rows tests/all/validation_message_rendering.rs:537-584 drive (all ≤250 bytes), which echo the submitted value in full — so the sentence shape changes only for values past the cap that the crate believes unreachable.
