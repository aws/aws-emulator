Issue-Source: trace-sweep-divergence
Issue-Trace: arc-zonal-shift-nlb-zone-weight-shift-and-cancel
Issue-Step: task#8
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: arc-zonal-shift-nlb-zone-weight-shift-and-cancel
Issue-Shape-Id: com.amazonaws.arczonalshift#PercDataPlane
Issue-Key: arc-zonal-shift|StartZonalShift|2xx->2xx|$.expiryTime
Issue-Key: arc-zonal-shift|StartZonalShift|2xx->2xx|$.startTime
Issue-Key: arc-zonal-shift|StartZonalShift|2xx->2xx|$.zonalShiftId
Issue-Key: arc-zonal-shift|GetManagedResource|2xx->2xx|$.zonalShifts[*].expiryTime
Issue-Key: arc-zonal-shift|GetManagedResource|2xx->2xx|$.zonalShifts[*].startTime
Issue-Key: arc-zonal-shift|ListZonalShifts|2xx->2xx|$.items[*].expiryTime
Issue-Key: arc-zonal-shift|ListZonalShifts|2xx->2xx|$.items[*].startTime
Issue-Key: arc-zonal-shift|CancelZonalShift|2xx->2xx|$.expiryTime
Issue-Key: arc-zonal-shift|CancelZonalShift|2xx->2xx|$.startTime
Issue-Commit: 075f5affd9be323818b79ff1d792bbcf981ada1a

# arc-zonal-shift reports minted zonal-shift values with non-AWS wire shapes

> _Note: this issue was identified against the code base at commit `075f5affd9be323818b79ff1d792bbcf981ada1a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartZonalShift

## Description
Every value this service MINTS for a zonal shift — the shift's identifier and the two instants it reports — leaves the emulator in a shape no ARC response takes, so each operation that reports a zonal shift diverges from the recorded wire on the same members. 5 causes, all in arc-zonal-shift.

Cause 1 of 5: StartZonalShift mints a zonalShiftId that is a sequential counter, not an identifier shaped the way ARC issues one.
  AWS: Smithy (services/service-com-amazonaws-arczonalshift-percdataplane/smithy-model/com-amazonaws-arczonalshift-percdataplane.json) — StartZonalShift output member `zonalShiftId` targets `ZonalShiftId`, a string with @length min 6 max 36 and @pattern ^[A-Za-z0-9-]+$, @required; HTTP 201 POST /zonalshifts. ARC issues this identifier itself (the caller cannot supply one: it is not an input member), and the recorded response carries a 36-character UUID-shaped value — lowercase hex in 8-4-4-4-12 groups, which is exactly what the 36-char maximum and the [A-Za-z0-9-] pattern describe. The fixture asserts the member's SHAPE rather than the recorded literal.
  Emulator: answers a monotonic per-store counter rendered as `zs-` plus ten zero-padded decimal digits — the replay reports `$.zonalShiftId: value_mismatch (expected '<shaped-not-asserted>', got 'zs-0000000001')`. It satisfies the model's length and pattern constraints but is not the identifier shape ARC issues, and it restarts from the same first value in every fresh store.
  Why: a client that round-trips, stores or keys on an ARC shift id — or validates it as a UUID before putting it in a CancelZonalShift/UpdateZonalShift path, a CloudWatch dimension, or an EventBridge correlation — rejects or mis-handles `zs-0000000001`. Because the counter restarts per store, two different shifts in two runs also collide on one id, which a client treating the id as globally unique records as the same shift.
  Fix: the operation's state — the id minting helper the StartZonalShift handler calls to allocate a shift id (it is the single site that produces one).
  Traces: arc-zonal-shift-nlb-zone-weight-shift-and-cancel#task#8. Code: services/service-com-amazonaws-arczonalshift-percdataplane/src/state.rs:446-449 (`next_zonal_shift_id`, `format!("zs-{:010}", self.next_shift_seq)`), called from src/ops/start_zonal_shift.rs:278.

Cause 2 of 5: StartZonalShift serializes startTime/expiryTime with full f64 sub-second resolution.
  AWS: Smithy — StartZonalShift output members `startTime` (`StartTime`) and `expiryTime` (`ExpiryTime`) are both plain `timestamp` shapes, @required, with NO @timestampFormat trait; the service's protocol is restJson1, whose default document timestamp format is epoch-seconds, so both are JSON numbers of seconds since the epoch. Recorded behavior only for the fraction: ARC's recorded values carry no microsecond tail — an AWS epoch-seconds timestamp is serialized at most to millisecond precision (three fractional digits), and ARC's shift instants are whole/at-most-millisecond seconds. The fixture asserts the members' SHAPE, not the recorded instants (which are per-run values and are not a requirement).
  Emulator: emits the raw f64 seconds-since-epoch of the host clock, i.e. seven fractional digits — `$.startTime: value_mismatch (expected '<shaped-not-asserted>', got 1795163764.2275424)` and `$.expiryTime: value_mismatch (expected '<shaped-not-asserted>', got 1795165564.2275424)`, both at HTTP 201 (status itself matches).
  Why: a client that parses an ARC timestamp against AWS's epoch-seconds shape — a fixed-precision decimal/millis parser, a schema that bounds the fractional digits, or code that converts to an integer millisecond instant — either loses the tail silently or fails to parse. Hypothesis: the surplus precision also makes `expiryTime - startTime` differ from the whole number of seconds the caller's `expiresIn` names once a client rounds, so an expiry a caller asked for in whole minutes no longer reads as one.
  Fix: the operation's serializer path — the shared stored-instant-to-wire-timestamp conversion the handler applies to both members (truncate to the precision AWS serializes, rather than passing the full f64 fraction through).
  Traces: arc-zonal-shift-nlb-zone-weight-shift-and-cancel#task#8. Code: services/service-com-amazonaws-arczonalshift-percdataplane/src/state.rs:681-686 (`as_date_time`, `DateTime::from_secs_f64(since_epoch.as_secs_f64())`), called from src/ops/start_zonal_shift.rs:302-303.

Cause 3 of 5: GetManagedResource reports the same over-precise instants for every shift nested in the resource.
  AWS: Smithy — GetManagedResource (GET /managedresources/{resourceIdentifier}, 200) returns @required `zonalShifts` (`ZonalShiftsInResource`), whose member type carries `startTime` and `expiryTime` as the same unformatted `timestamp` shapes, so each nested shift's instants take the same restJson1 epoch-seconds shape as the top-level ones in Cause 2.
  Emulator: `$.zonalShifts[0].startTime: value_mismatch (expected '<shaped-not-asserted>', got 1795163764.2275424)` and `$.zonalShifts[0].expiryTime: value_mismatch (expected '<shaped-not-asserted>', got 1795165564.2275424)` at HTTP 200.
  Why: same as Cause 2, on the read path a client polls — the member a console or controller reads to decide whether a shift is still in effect is the one carrying the non-AWS fraction.
  Fix: the operation's serializer path — the nested-shift projection shares the same conversion helper, so it must take the corrected one rather than keep its own call site.
  Traces: arc-zonal-shift-nlb-zone-weight-shift-and-cancel#task#9. Code: services/service-com-amazonaws-arczonalshift-percdataplane/src/state.rs:645-646 (`zonal_shifts_in_resource`), served by src/ops/get_managed_resource.rs.

Cause 4 of 5: ListZonalShifts reports the same over-precise instants for every summary item.
  AWS: Smithy — ListZonalShifts (GET /zonalshifts, 200, @paginated items=items) returns `items` (`ZonalShiftSummaries`); each summary carries `startTime` and `expiryTime` as unformatted `timestamp` shapes, hence the same restJson1 epoch-seconds shape.
  Emulator: `$.items[0].startTime: value_mismatch (expected '<shaped-not-asserted>', got 1795163764.2275424)` and `$.items[0].expiryTime: value_mismatch (expected '<shaped-not-asserted>', got 1795165564.2275424)` at HTTP 200.
  Why: same as Cause 2, multiplied across a page — a client that sorts or filters a shift listing by instant is parsing a fraction AWS never emits.
  Fix: the operation's serializer path — the summary projection in the ListZonalShifts handler, through the same corrected conversion.
  Traces: arc-zonal-shift-nlb-zone-weight-shift-and-cancel#task#10. Code: services/service-com-amazonaws-arczonalshift-percdataplane/src/ops/list_zonal_shifts.rs, via state.rs:681-686.

Cause 5 of 5: CancelZonalShift reports the same over-precise instants on the cancelled shift it echoes.
  AWS: Smithy — CancelZonalShift (DELETE /zonalshifts/{zonalShiftId}, 200) returns the `ZonalShift` output shape, @required `startTime`/`expiryTime`, unformatted `timestamp`s, restJson1 epoch-seconds.
  Emulator: `$.startTime: value_mismatch (expected '<shaped-not-asserted>', got 1795163764.2275424)` and `$.expiryTime: value_mismatch (expected '<shaped-not-asserted>', got 1795165564.2275424)` at HTTP 200.
  Why: same as Cause 2, on the terminal response a client records as the shift's final state.
  Fix: the operation's serializer path — the CancelZonalShift handler's output projection, through the same corrected conversion.
  Traces: arc-zonal-shift-nlb-zone-weight-shift-and-cancel#task#11. Code: services/service-com-amazonaws-arczonalshift-percdataplane/src/ops/cancel_zonal_shift.rs, via state.rs:681-686.

None of the open arc-zonal-shift issues covers these: 20261029T170702Z (AccessDeniedException 403 body), 20261111T135546Z (CreatePracticeRunConfiguration 403 Message), 20261120T043806Z (StartPracticeRun 403-vs-404), 20261104T064145Z (ELBv2 opt-in registration), 20261109T190734Z (CancelPracticeRun stub) and 20261109T123922Z (CloudFormation resource types) are all about error paths, registration or unimplemented surface, not about the shape of the values a successful zonal-shift response carries.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up, /__admin/health 200): 34 steps, 24 passed, 10 mismatched, 8 divergence clusters. Four clusters are net-new (the other four are owned by open issues) and all four are arc-zonal-shift, all on the happy path with the HTTP status matching:

1. fixture arc-zonal-shift-nlb-zone-weight-shift-and-cancel step task#8 — StartZonalShift, expected 201 / got 201:
   $.zonalShiftId: value_mismatch (expected '<shaped-not-asserted>', got 'zs-0000000001')
   $.startTime:    value_mismatch (expected '<shaped-not-asserted>', got 1795163764.2275424)
   $.expiryTime:   value_mismatch (expected '<shaped-not-asserted>', got 1795165564.2275424)
   divergence_key: service=arc-zonal-shift|StartZonalShift|status=201->201|keys=$.expiryTime,$.startTime,$.zonalShiftId

2. same fixture step task#9 — GetManagedResource, expected 200 / got 200:
   $.zonalShifts[0].startTime:  value_mismatch (expected '<shaped-not-asserted>', got 1795163764.2275424)
   $.zonalShifts[0].expiryTime: value_mismatch (expected '<shaped-not-asserted>', got 1795165564.2275424)

3. same fixture step task#10 — ListZonalShifts, expected 200 / got 200:
   $.items[0].startTime:  value_mismatch (expected '<shaped-not-asserted>', got 1795163764.2275424)
   $.items[0].expiryTime: value_mismatch (expected '<shaped-not-asserted>', got 1795165564.2275424)

4. same fixture step task#11 — CancelZonalShift, expected 200 / got 200:
   $.startTime:  value_mismatch (expected '<shaped-not-asserted>', got 1795163764.2275424)
   $.expiryTime: value_mismatch (expected '<shaped-not-asserted>', got 1795165564.2275424)

Expected (AWS): zonalShiftId is an ARC-issued 36-character UUID-shaped string (model: ZonalShiftId, @length 6..36, @pattern ^[A-Za-z0-9-]+$); startTime/expiryTime are restJson1 epoch-seconds JSON numbers whose fraction never exceeds millisecond precision (model: StartTime/ExpiryTime, plain timestamp, no @timestampFormat).
Actual (emulator): zonalShiftId is `zs-` + a 10-digit per-store sequence counter; both instants are the host clock's f64 seconds carried to seven fractional digits.
The literal instants and the digit 1 in `zs-0000000001` above are per-run values, not requirements; the requirement is the shape — a UUID-shaped ARC id, and an epoch-seconds number with no sub-millisecond tail.
Source: state.rs:446-449 (next_zonal_shift_id) and state.rs:681-686 (as_date_time → DateTime::from_secs_f64(since_epoch.as_secs_f64())).

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay fixture arc-zonal-shift-nlb-zone-weight-shift-and-cancel from trace set simparity-wire-aws-v1 (steps task#8 through task#11). Each step reproduces one cause:
   - task#8: POST /zonalshifts with a resourceIdentifier of an ARC-managed NLB, an awayFrom zone the resource is deployed in, a whole-number expiresIn (e.g. "30m") and a comment. The 201 body's zonalShiftId comes back as `zs-0000000001` instead of a UUID-shaped id, and startTime/expiryTime come back with seven fractional digits.
   - task#9: GET /managedresources/{resourceIdentifier} for that same resource — zonalShifts[0].startTime/expiryTime carry the same seven-digit fraction.
   - task#10: GET /zonalshifts — items[0].startTime/expiryTime likewise.
   - task#11: DELETE /zonalshifts/{zonalShiftId} for the shift started in task#8 — the 200 body's startTime/expiryTime likewise.
3. Compare each member against the recorded shape: the id must be an ARC-issued UUID-shaped string, and each timestamp must be an epoch-seconds number with no sub-millisecond fraction. Restart the emulator and repeat step 2 to see the id counter restart at the same first value.
