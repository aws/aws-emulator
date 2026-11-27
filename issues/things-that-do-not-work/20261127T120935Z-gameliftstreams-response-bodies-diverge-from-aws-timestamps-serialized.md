Issue-Source: trace-sweep-divergence
Issue-Trace: gameliftstreams-application-shader-caches-identifier-validation
Issue-Step: setup#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: gameliftstreams-application-shader-caches-identifier-validation gameliftstreams-disassociate-applications-request-validation
Issue-Shape-Id: com.amazonaws.gameliftstreams#GameLiftStreams
Issue-Key: gameliftstreams|CreateApplication|2xx->2xx|$.CreatedAt
Issue-Key: gameliftstreams|CreateApplication|2xx->2xx|$.LastUpdatedAt
Issue-Key: gameliftstreams|GetApplication|2xx->2xx|$.CreatedAt
Issue-Key: gameliftstreams|GetApplication|2xx->2xx|$.LastUpdatedAt
Issue-Key: gameliftstreams|CreateStreamGroup|2xx->2xx|$.CreatedAt
Issue-Key: gameliftstreams|CreateStreamGroup|2xx->2xx|$.ExpiresAt
Issue-Key: gameliftstreams|CreateStreamGroup|2xx->2xx|$.LastUpdatedAt
Issue-Key: gameliftstreams|CreateStreamGroup|2xx->2xx|$.LocationStates[*].OnDemandCapacity
Issue-Key: gameliftstreams|GetStreamGroup|2xx->2xx|$.CreatedAt
Issue-Key: gameliftstreams|GetStreamGroup|2xx->2xx|$.ExpiresAt
Issue-Key: gameliftstreams|GetStreamGroup|2xx->2xx|$.LastUpdatedAt
Issue-Key: gameliftstreams|GetStreamGroup|2xx->2xx|$.LocationStates[*].OnDemandCapacity
Issue-Commit: 9038a283ab0a537b35729cff6dea9cea13a768a9

# gameliftstreams response bodies diverge from AWS: timestamps serialized as epoch numbers and a location's OnDemandCapacity reported as 0

> _Note: this issue was identified against the code base at commit `9038a283ab0a537b35729cff6dea9cea13a768a9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateApplication, GetApplication, CreateStreamGroup, GetStreamGroup

## Description
Two independent root causes make the SUCCESS bodies of the gameliftstreams control-plane reads and creates differ from AWS. The statuses are right (201 on create, 200 on read); the member values are wrong. Both must be closed before this issue is submitted; each cause names its own fixtures on its Traces line.

=== Cause 1 of 2: Timestamp members are written as bare JSON epoch numbers, where this service reports ISO-8601 date-time strings ===
AWS: every Timestamp member of an application or a stream group is reported as an ISO-8601 date-time string. The model documents it on the members themselves — CreateApplication/GetApplication CreatedAt and LastUpdatedAt: "A timestamp that indicates when this resource was created. Timestamps are expressed using in ISO8601 format, such as: 2022-12-27T22:29:40+00:00 (UTC)." (Smithy: members target the prelude smithy.api#Timestamp, which carries no constraints of its own; CreateStreamGroup/GetStreamGroup report CreatedAt, LastUpdatedAt and ExpiresAt the same way.) The instant itself is per-run and is NOT a requirement; the WIRE FORM of the member is.
Emulator: writes a bare JSON number with a sub-second fraction — CreateApplication answered CreatedAt = 1795780980.0914598 and LastUpdatedAt = 1795780980.0914598, GetApplication answered CreatedAt = 1795780980.0914598 / LastUpdatedAt = 1795780980.5914598, CreateStreamGroup answered CreatedAt = 1795780992.07646, LastUpdatedAt = 1795780992.07646, ExpiresAt = 1827316992.07646, and GetStreamGroup the same values. The fixtures assert the SHAPE of these members (not the instant) and every one of them is rejected.
Why: a client that reads these members as strings (a UI, a log line, a template that interpolates CreatedAt, a strict deserialiser for a date-time-typed field) gets a number and either fails to parse it or renders a raw epoch; conversely code written against the emulator's number breaks on AWS. The microsecond fraction is also finer than anything the service reports.
Fix: the service's timestamp serialisation — services/service-com-amazonaws-gameliftstreams-gameliftstreams/src/state.rs::timestamp (line ~2464) hands a std SystemTime to aws_smithy_types::DateTime and the restJson1 body codec writes it in its default epoch-seconds form; the members must go on the wire as ISO-8601 date-time strings (the date-time format), for every Timestamp member of this service's outputs, not just the four operations below.
Traces: gameliftstreams-application-shader-caches-identifier-validation#setup#3 (CreateApplication; also gameliftstreams-disassociate-applications-request-validation#setup#3), gameliftstreams-application-shader-caches-identifier-validation#task#7 (GetApplication; also gameliftstreams-disassociate-applications-request-validation#task#10), gameliftstreams-disassociate-applications-request-validation#setup#4 (CreateStreamGroup), gameliftstreams-disassociate-applications-request-validation#task#11 (GetStreamGroup).

=== Cause 2 of 2: a stream-group location reports OnDemandCapacity 0 where the configured on-demand capacity is 2 ===
AWS: CreateStreamGroup (POST /streamgroups, 201) and GetStreamGroup (GET /streamgroups/{Identifier}, 200) report each location as a LocationState whose OnDemandCapacity is "the streaming capacity that Amazon GameLift Streams can allocate in response to stream requests, and then de-allocate when the session has terminated" — the capacity the request's LocationConfigurations asked for, carried through the create into every later read (Smithy: LocationConfiguration.OnDemandCapacity, integer @range min 0, @deprecated since 2025-12-17 "in favor of explicit MaximumCapacity values"; LocationState.OnDemandCapacity, integer @range min 0). The recorded create/read pair reports LocationStates[0].OnDemandCapacity = 2, with the other capacity members of the same location agreeing with the emulator.
Emulator: answers LocationStates[0].OnDemandCapacity = 0 from the create AND from the later read, so the value is lost on the way in or on the way out, not merely stale. state.rs projects the member as slot.on_demand_capacity.unwrap_or(0).max(0) (line ~1206) over LocationSlot.on_demand_capacity, which CreateStreamGroup fills from LocationConfiguration::activating (line ~848) — i.e. the stored slot is holding None.
Why: capacity is the one thing a caller sizes a stream group by. A client that creates a group with an on-demand capacity and reads it back sees a group with no on-demand capacity at all, so it either re-issues the configuration or concludes the request was ignored; anything that derives cost or concurrency from the reported capacity is wrong by the whole configured amount.
Hypothesis for the mechanism (the fixture's request body is not in the replay output, so this is not asserted): either the @deprecated OnDemandCapacity input member is not populated when the LocationConfigurations list is deserialised, so the slot keeps None, or AWS reports the legacy OnDemandCapacity as a view of the location's elastic capacity (the MaximumCapacity that deprecates it) while the emulator reports a bare 0 for it. Whichever it is, the create's own response and every later read must report the capacity the request configured.
Fix: the CreateStreamGroup input path and the location projection in service state — services/service-com-amazonaws-gameliftstreams-gameliftstreams/src/ops/create_stream_group.rs (the LocationSlot::activating mapping) and src/state.rs (LocationSlot::activating line ~848, the LocationState projection line ~1206), so the configured on-demand capacity is stored and echoed.
Traces: gameliftstreams-disassociate-applications-request-validation#setup#4 (CreateStreamGroup), gameliftstreams-disassociate-applications-request-validation#task#11 (GetStreamGroup).

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (13 fixtures, 98 steps, 71 passed, 27 mismatched, score 72.4%).

Cause 1 — Timestamp wire form (statuses matched, only these keys diverged):
  gameliftstreams-application-shader-caches-identifier-validation#setup#3 — CreateApplication 201 -> 201:
    $.CreatedAt      expected <shaped assertion>, got 1795780980.0914598
    $.LastUpdatedAt  expected <shaped assertion>, got 1795780980.0914598
    (same cluster: gameliftstreams-disassociate-applications-request-validation#setup#3)
  gameliftstreams-application-shader-caches-identifier-validation#task#7 — GetApplication 200 -> 200:
    $.CreatedAt 1795780980.0914598, $.LastUpdatedAt 1795780980.5914598
    (same cluster: gameliftstreams-disassociate-applications-request-validation#task#10)
  gameliftstreams-disassociate-applications-request-validation#setup#4 — CreateStreamGroup 201 -> 201:
    $.CreatedAt 1795780992.07646, $.ExpiresAt 1827316992.07646, $.LastUpdatedAt 1795780992.07646
  gameliftstreams-disassociate-applications-request-validation#task#11 — GetStreamGroup 200 -> 200:
    $.CreatedAt 1795780992.07646, $.ExpiresAt 1827316992.07646, $.LastUpdatedAt 1795780992.57646
  The instants above are this run's clock and are not the finding; the finding is that the member is a JSON number with a microsecond fraction instead of the ISO-8601 date-time string the model documents ("Timestamps are expressed using in ISO8601 format, such as: 2022-12-27T22:29:40+00:00 (UTC)" — CreatedAt/LastUpdatedAt on CreateApplication/GetApplication, model com-amazonaws-gameliftstreams-gameliftstreams.json).
  source: src/state.rs line ~2464 `pub fn timestamp(at: SystemTime) -> ::aws_smithy_types::DateTime { DateTime::from(at) }`, consumed by every output (e.g. ops/create_application.rs lines 468-469).

Cause 2 — LocationStates[0].OnDemandCapacity:
  gameliftstreams-disassociate-applications-request-validation#setup#4 — CreateStreamGroup 201 -> 201: $.LocationStates[0].OnDemandCapacity expected 2, got 0
  gameliftstreams-disassociate-applications-request-validation#task#11 — GetStreamGroup 200 -> 200: $.LocationStates[0].OnDemandCapacity expected 2, got 0
  No other capacity member of that location diverged, so the group's remaining LocationState members already agree.
  model: LocationConfiguration.OnDemandCapacity (integer, @range min 0, @deprecated since 2025-12-17) and LocationState.OnDemandCapacity (integer, @range min 0).
  source: src/state.rs LocationSlot::activating (line ~848) and the LocationState projection (line ~1206, `on_demand_capacity: Some(slot.on_demand_capacity.unwrap_or(0).max(0))`).

Not covered by any open gameliftstreams issue: 20261127T103056Z-gameliftstreams-8-control-plane-fidelity-defects-6-character.md is a behavioral bundle about id width, status guards, NextToken validation, capacity multiples and stream-URL revocation, and says nothing about the wire form of Timestamp members or about OnDemandCapacity being dropped.

## Steps to Reproduce
Boot the emulator and point a gameliftstreams client at it.

Cause 1: CreateApplication(Description='d', RuntimeEnvironment={'Type':'UBUNTU','Version':'22_04_LTS'}, ExecutablePath='run.sh', ApplicationSourceUri='s3://<bucket>/app/') and then GetApplication on the returned id; CreateStreamGroup(Description='d', StreamClass=<accepted class>, LocationConfigurations=[{'LocationName':'us-west-2','OnDemandCapacity':2}]) and then GetStreamGroup. Read the RAW response body (not through an SDK that re-formats timestamps): CreatedAt / LastUpdatedAt / ExpiresAt come back as JSON numbers such as 1795780992.07646 instead of ISO-8601 date-time strings.

Cause 2: in the same CreateStreamGroup call above, read LocationStates[0].OnDemandCapacity in the 201 body and again in the GetStreamGroup 200 body -> the emulator reports 0 for a location configured with an on-demand capacity of 2.

Or re-replay the fixtures/steps named in Issue-Trace/Issue-Step and Issue-Trace-Ids.
