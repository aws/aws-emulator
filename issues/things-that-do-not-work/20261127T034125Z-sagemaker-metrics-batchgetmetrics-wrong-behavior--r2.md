Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 416322dbc7f431756d96520e75fd2eee4a187ea3

# The envelope refuses an integral member sent as an integral-valued JSON float (Start/End, Step), though the codec reads it — so a servable request is answered 400 SerializationException

_Filed: 2026-11-27T03:41:25Z_
_Operation: BatchGetMetrics | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `416322dbc7f431756d96520e75fd2eee4a187ea3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate's envelope layer decides, ahead of the modelled operation, which values "never became a value" and refuses the whole call for them. Its own rule is that this set IS the codec's reading: `envelope.rs:21-24` says "Which values those are is the codec's own reading ... this layer asks what the codec asks ([`reads_into`])", and `reads_into`'s doc closes with "Both corrections ask what this protocol's codec asks, so a value this layer calls readable is one the codec goes on to read".

For the integral members of both inputs — `MetricQuery.Start` / `MetricQuery.End` (target `Long`) and `RawMetricData.Step` (target `Integer`) — the two DISAGREE on one JSON form the model admits: a whole number written as a float literal (`1.0`, `0.0`, `-5.0`, `1e3`, `1700000000.0`).

* `reads_into` delegates to `service_common::json_wire_values::reads_as`, whose `fits_integral` asks `serde_json::Number::as_i64()`. That answers `None` for a number parsed from a float literal (it only reports a number stored as an integer), so the value is judged UNREADABLE; the fallthrough arm in `reads_into` rescues only `Float | Double | BigDecimal` members carrying a quoted non-finite text, so an `Integer`/`Long` member gets `false`.
* The codec the claim is measured against reads that same value fine: `number_to_i64` has an explicit `Number::Float(v) => if v.fract() == 0.0 && in-range { Ok(v as i64) }` arm, which exists precisely to accept this form, and `read_integer` is `read_long` + `i32::try_from`.

So `refusal()` returns `Refusal::Unreadable` (HTTP 400, `SerializationException`, capital-`Message` "NUMBER_VALUE is not an expected Json type") for a request the modelled operation would have served: a `BatchGetMetrics` whose window is sent as `"Start": 1700000000.0` is refused outright instead of being answered per query, and a `BatchPutMetrics` point sent with `"Step": 1.0` is refused instead of being ingested (or reported at its own index), which is the in-band contract this crate states everywhere else (`envelope.rs:8-13`).

Every other disagreement direction checks out — a float past the member's width (`"Step": 2147483648`), a number past `i64`, and a non-numeric class are refused by BOTH the envelope and the codec — so the defect is confined to the integral-valued float literal. It is distinct from the FIXED BatchGetMetrics issue about `StdDev` of an infinite bucket (a reduction result) and from the FIXED BatchPutMetrics `Timestamp`-string issue (a value the envelope called readable and the codec then refused: the opposite direction of this same seam).

## Evidence

The rule this crate states — `services/service-com-amazonaws-sagemakermetrics-sagemakermetricsservice/src/service/envelope.rs:18-24`:
```
//! * a value the member it is bound to cannot be read from never became a value, which is a
//!   DESERIALIZATION failure: HTTP 400 [`SERIALIZATION_EXCEPTION`], with the sentence in the
//!   body member spelled `Message`, naming the CLASS of JSON that arrived
//!   ([`UNEXPECTED_JSON_TYPE`]). Which values those are is the codec's own reading, so for
//!   the two kinds of member whose wire form decides it — a float, which reads a quoted
//!   non-finite number, and a timestamp, which reads only text its own `@timestampFormat`
//!   parses — this layer asks what the codec asks ([`reads_into`]);
```
and `src/service/envelope.rs:420-421`:
```
/// Both corrections ask what this protocol's codec asks, so a value this layer calls
/// readable is one the codec goes on to read.
```
The code under it — `src/service/envelope.rs:422-435`:
```
fn reads_into(member: &Schema, held: &Value) -> bool {
    if matches!(member.shape_type(), ShapeType::Timestamp) {
        return reads_as_instant(member, held);
    }
    if ::service_common::json_wire_values::reads_as(member.shape_type(), held) {
        return true;
    }
    matches!(
        member.shape_type(),
        ShapeType::Float | ShapeType::Double | ShapeType::BigDecimal
    ) && held
        .as_str()
        .is_some_and(|text| NON_FINITE_TEXTS.contains(&text))
}
```
what it delegates to — `services/common/src/json_wire_values.rs:199-203` and `:228-232`:
```
        Value::Number(number) => match target {
...
            ShapeType::Integer => fits_integral(number, i32::MIN.into(), i32::MAX.into()),
            ShapeType::Long | ShapeType::BigInteger => fits_integral(number, i64::MIN, i64::MAX),
...
fn fits_integral(number: &serde_json::Number, min: i64, max: i64) -> bool {
    number
        .as_i64()
        .is_some_and(|held| (min..=max).contains(&held))
}
```
(its own doc, `services/common/src/json_wire_values.rs:184-185`, states the intended rule as "An integral shape reads only a whole number its width holds" — and `1.0` IS a whole number `i32` holds).

The codec that "the two cannot disagree" with — `vendor/smithy-schema-server/src/json.rs:746-761`:
```
fn number_to_i64(n: Number) -> Result<i64, SerdeError> {
    match n {
...
        Number::Float(v) => {
            if v.fract() == 0.0 && (i64::MIN as f64..=i64::MAX as f64).contains(&v) {
                Ok(v as i64)
```
reached from `vendor/smithy-schema-server/src/json.rs:915-928` (`read_integer` = `read_long` + `i32::try_from`; `read_long` = `number_to_i64`).

What the request is answered with instead — `src/service/envelope.rs:352-354` (`unreadable_member` is consulted first and returns `Refusal::Unreadable`), the per-item walk that reaches a point's `Step`/a query's `Start` at `src/service/envelope.rs:485-487`:
```
            Value::Object(fields) => declared_members(item, fields).find_map(|(field, held)| {
                (!reads_into(field, held)).then(|| Refusal::Unreadable(unexpected_json_type(held)))
            }),
```
and the envelope it is rendered in — `src/service/envelope.rs:178` (`Self::Unreadable(_) | Self::InvalidResource => 400`) with `src/service/envelope.rs:88` (`pub const SERIALIZATION_EXCEPTION: &str = "SerializationException";`).

Test coverage leaves it unpinned, so a fix changes no existing assertion: every row of `tests/all/envelope_and_per_item_reporting.rs:307-382` sends a string, a boolean, an array, an object or a non-object item (e.g. `:379` `json!({"TrialComponentName": "tc-types", "MetricData": [7]})`), never a float for an integral member; the only fractional-number row in the crate is for a TIMESTAMP member — `src/service/envelope.rs:834`: `("no format: a fraction of a second", None, json!(7.5), true)`; and the roundtrip tests always send `Step`/`Start` as integer literals (`tests/all/metrics_roundtrip.rs:123-140`).

## Steps to Reproduce

1. Read src/service/envelope.rs:21-24 and :420-421: the set of values this layer refuses is stated to be the codec's own reading.
2. Send POST /BatchGetMetrics with the raw body {"MetricQueries":[{"MetricName":"loss","ResourceArn":"arn:aws:sagemaker:us-east-1:123456789012:experiment-trial-component/tc-1","MetricStat":"Last","Period":"OneHour","XAxisType":"Timestamp","Start":1700000000.0,"End":1700003600.0}]} — note Start/End are written as float literals (what json.dumps of a Python float, or any producer holding epochs as doubles, emits).
3. Observed: HTTP 400 with {"Message":"NUMBER_VALUE is not an expected Json type"} — src/service/envelope.rs:485-487 judges the Long member unreadable because services/common/src/json_wire_values.rs:230 asks serde_json::Number::as_i64(), which answers None for a float-parsed number.
4. Expected from the quoted rule: the codec reads that value (vendor/smithy-schema-server/src/json.rs:751-758 accepts a Float whose fract() == 0.0 and is in range), so the envelope must not refuse it and the query must be answered in-band (200, one MetricQueryResult).
5. Repeat on the write side: PUT /BatchPutMetrics with {"TrialComponentName":"tc-1","MetricData":[{"MetricName":"loss","Timestamp":1700000000,"Step":1.0,"Value":2.5}]} — same 400 SerializationException, and nothing is ingested, instead of the point being stored at Step 1 (or reported at its own index).
6. Confirm the other direction still agrees, so only this form is at issue: the same requests with "Step":2147483648 or "Start":9223372036854775808 are refused by the envelope AND by the codec.
