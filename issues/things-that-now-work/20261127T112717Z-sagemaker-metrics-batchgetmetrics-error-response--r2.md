Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: f73c567c518c4ea32fbc68f3ca5cb3fd81df2cf2

# The item-count bound is documented as applied "before any item is read" but runs after the per-item unreadable walk, so an over-sized batch earns the deserialization refusal instead

_Filed: 2026-11-27T11:27:17Z_
_Operation: BatchGetMetrics | Class: error-response_

> _Note: this issue was identified against the code base at commit `f73c567c518c4ea32fbc68f3ca5cb3fd81df2cf2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate's envelope layer carries one bound of its own — `crate::state::MAX_ITEMS_PER_REQUEST` — and states, twice, that it is applied BEFORE any item of the batch is read. The code applies it last of the two per-item passes: `refusal()` calls `unreadable_member()` first, and `unreadable_member()` walks EVERY element of the batch member and every declared field of every element. So the ordering rule the module header states is broken by the function the header describes.

Two consequences follow from the quoted text alone:

1. Refusal precedence. A request carrying more than `MAX_ITEMS_PER_REQUEST` items in which ANY item carries a value its member cannot be read from is answered with `Refusal::Unreadable` — HTTP 400 `SerializationException`, sentence under the capital-`M` `Message` member — instead of the internal-failure refusal (HTTP 500 `InternalFailure`, `message: "Unhandled internal failure"`) that the header promises for an over-sized batch. The inline comment at `envelope.rs:355` ("Then the work one call may ask for, before any item of it is read.") sits directly under the call that already read every item, so the two comments in this one file cannot both be true of this code.

2. The purpose of the bound is defeated for that path. The bound exists because "Taking the modelled `@length` of a batch off the request boundary takes with it the only thing bounding the WORK one call asks for" — yet the unbounded per-item walk (every element, every field of every element, through `reads_into`) is performed in full before the item count is ever looked at.

This is distinct from the FIXED BatchGetMetrics error-response issue (a whitespace-only `ResourceArn` failing the whole call) and from the open `--r2` BatchGetMetrics wrong-behavior issue (an integral member sent as an integral-valued JSON float): both of those are about WHICH values `reads_into` calls readable, while this one is about the ORDER in which the envelope's two per-item passes run.

## Evidence

The rule the layer states — `services/service-com-amazonaws-sagemakermetrics-sagemakermetricsservice/src/service/envelope.rs:39-42`:
```
//! Taking the modelled `@length` of a batch off the request boundary takes with it the only
//! thing bounding the WORK one call asks for, so this layer carries that bound itself: a
//! request carrying more than [`crate::state::MAX_ITEMS_PER_REQUEST`] items is answered
//! with the internal-failure refusal above, before any item is read.
```
The code under it — `src/service/envelope.rs:346-359`:
```
fn refusal(definition: &dyn ErasedServiceDefinition, req: &RtRequest) -> Option<Refusal> {
    let key = definition.protocol().routing_key(req).ok()?;
    let input = super::input_schema(key)?;
    let members = submitted_members(req)?;
    // A value that never became a value comes first: a member of the wrong JSON type is
    // not a member carrying nothing.
    if let Some(unreadable) = unreadable_member(input, &members) {
        return Some(unreadable);
    }
    // Then the work one call may ask for, before any item of it is read.
    if asks_for_more_items_than_served(input, &members) {
        return Some(Refusal::UnhandledInternalFailure);
    }
    unset_member_refusal(input, &members)
}
```
and the pass that runs first DOES read every item — `src/service/envelope.rs:479-490`:
```
        let item = item_schema(member.member_name()?)?;
        let Value::Array(elements) = held else {
            return None;
        };
        elements.iter().find_map(|element| match element {
            // An item that is not an object cannot be read as the shape it is bound to.
            Value::Object(fields) => declared_members(item, fields).find_map(|(field, held)| {
                (!reads_into(field, held)).then(|| Refusal::Unreadable(unexpected_json_type(held)))
            }),
```
whose own doc confirms it reads items — `src/service/envelope.rs:471-472`:
```
/// The body's own top level is read first, then the members of each ITEM of a batch, which
/// is as deep as this service's inputs nest.
```
The two refusals that are therefore swapped — `src/service/envelope.rs:178-180`:
```
            Self::Unreadable(_) | Self::InvalidResource => 400,
            Self::UnhandledInternalFailure | Self::NoDetail => 500,
```
with `src/service/envelope.rs:88` (`pub const SERIALIZATION_EXCEPTION: &str = "SerializationException";`) and `:108` (`pub const UNHANDLED_INTERNAL_FAILURE: &str = "Unhandled internal failure";`).

Test coverage leaves the precedence unpinned, so a fix changes no existing assertion: `tests/all/envelope_and_per_item_reporting.rs:696-742` (`the_bounds_this_emulator_sets_for_itself_are_enforced`) sends only WELL-FORMED items past the bound — `("one item past the bound", MAX_ITEMS_PER_REQUEST + 1, 500)` built from `point("bounded", step, 1.0)` / `query("bounded", component, ITERATION_NUMBER, ITERATION_NUMBER)` — and the unreadable-member test rows (`tests/all/envelope_and_per_item_reporting.rs:303-382`) all send single-item batches.

## Steps to Reproduce

1. Read src/service/envelope.rs:39-42: an over-sized batch is stated to be answered with the internal-failure refusal "before any item is read", and src/service/envelope.rs:355 repeats it ("Then the work one call may ask for, before any item of it is read.").
2. Read src/service/envelope.rs:346-359 together with :471-472 and :479-490: unreadable_member() is consulted FIRST and it walks every element of the batch member and every declared field of each element.
3. POST /BatchGetMetrics with MetricQueries carrying 201 queries (MAX_ITEMS_PER_REQUEST + 1), where query 0 is {"MetricName": 7, "ResourceArn": "arn:aws:sagemaker:us-east-1:123456789012:experiment-trial-component/tc-1", "MetricStat": "Last", "Period": "OneHour", "XAxisType": "Timestamp"} — a number bound to a string member.
4. Observed: HTTP 400 with {"Message": "NUMBER_VALUE is not an expected Json type"} (Refusal::Unreadable, src/service/envelope.rs:352-354, 178), after the layer has walked all 201 items.
5. Expected from the quoted rule: the item bound is applied before any item is read, so the call is refused HTTP 500 InternalFailure with {"message": "Unhandled internal failure"} (src/service/envelope.rs:356-357, :108).
6. Repeat on the write side (PUT /BatchPutMetrics with 201 MetricData entries, one carrying "Value": "not-a-number") for the same swap; then confirm the existing bound test (tests/all/envelope_and_per_item_reporting.rs:705-742) only covers well-formed over-sized batches, so nothing pins the current precedence.
