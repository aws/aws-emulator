Issue-Source: trace-sweep-divergence
Issue-Trace: application-signals-instrumentation-config-errors
Issue-Step: teardown#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: application-signals-instrumentation-config-errors application-signals-discovery-and-service-topology application-signals-instrumentation-config-lifecycle application-signals-slo-exclusion-windows
Issue-Shape-Id: com.amazonaws.applicationsignals#ApplicationSignals
Issue-Key: application-signals|DeleteInstrumentationConfiguration|2xx->4xx|$.DeletionStatus
Issue-Key: application-signals|DeleteInstrumentationConfiguration|2xx->4xx|status
Issue-Key: application-signals|GetService|2xx->2xx|$.Service.KeyAttributes
Issue-Key: application-signals|GetService|2xx->2xx|$.Service.MetricReferences
Issue-Key: application-signals|CreateInstrumentationConfiguration|2xx->2xx|$.ARN
Issue-Key: application-signals|CreateInstrumentationConfiguration|2xx->2xx|$.LocationHash
Issue-Key: application-signals|ListServiceLevelObjectiveExclusionWindows|2xx->2xx|$.ExclusionWindows[*]
Issue-Commit: 53bc876257fb785b9638e86a310c855107cbc39b

# application-signals responses diverge: fabricated, dropped and mis-derived response members

> _Note: this issue was identified against the code base at commit `53bc876257fb785b9638e86a310c855107cbc39b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteInstrumentationConfiguration

## Description
Replay of the application-signals wire corpus (simparity-wire-aws-v1) surfaced FOUR independent response-fidelity defects in this service. They are bundled because they share one service and one general fault family: the emulator's response bodies are not built from what AWS actually returns for these operations -- it fabricates members AWS omits, omits members AWS returns, derives a modeled identifier differently, and loses a created resource. ALL FOUR causes must be fixed and re-replayed before this issue is closed.

=== Cause 1 of 4: DeleteInstrumentationConfiguration returns 404 ResourceNotFoundException for a configuration that was successfully created ===
AWS: Smithy: DeleteInstrumentationConfiguration -> POST /delete-instrumentation-configuration, http code 200, output DeletionStatus (required, enum DynamicInstrumentationDeletionStatus whose only value is "DELETED"; doc: "The value is DELETED after a successful deletion"). Input is InstrumentationType, Service, Environment, SignalType and LocationIdentifier (a union of CodeLocation -- doc "will be hashed internally" -- or a pre-computed 16-char LocationHash). ResourceNotFoundException (httpError 404) is modeled only for a configuration that does not exist. For a configuration created earlier in the same account/region with the same (InstrumentationType, Service, Environment, SignalType, location) tuple, AWS deletes it and answers 200 with DeletionStatus=DELETED; the recorded step confirms 200/DELETED.
Emulator: answers HTTP 404 and no DeletionStatus member at all ($.DeletionStatus: missing, expected 'DELETED', got None) for the configuration created earlier in the same fixture. The delete's lookup key does not match the key the create stored under -- Hypothesis: the delete path resolves LocationIdentifier (CodeLocation vs LocationHash arm) through a different derivation than CreateInstrumentationConfiguration used when it keyed the record, so the record is never found.
Why: a client cannot delete an instrumentation configuration it just created. Teardown of any client, test harness or CloudFormation-style flow fails with a spurious ResourceNotFoundException, configurations leak in emulator state forever, and retry logic that treats 404 as "already gone" silently diverges from real accounts where the resource is still present. The required DeletionStatus member being absent also breaks SDK deserialization for callers that read it.
Fix: the DeleteInstrumentationConfiguration handler and the instrumentation-configuration state keying -- make the delete lookup use exactly the key derivation Create used (resolving both LocationIdentifier union arms to the same stored key), and return 200 with DeletionStatus=DELETED on success, reserving 404 ResourceNotFoundException for a tuple that was never created.
Traces: application-signals-instrumentation-config-errors#teardown#5.

=== Cause 2 of 4: CreateInstrumentationConfiguration computes a different LocationHash (and therefore a different ARN) than AWS ===
AWS: Smithy: CreateInstrumentationConfiguration output LocationHash (required, String @length min 16 max 16) and ARN (required, InstrumentationConfigurationArn @pattern ^arn:[^:]+:application-signals:[^:]+:[0-9]{12}:instrumentationConfig/.+/[0-9a-f]{16}$). Smithy: LocationIdentifier$CodeLocation doc "The full code location specification (will be hashed internally)"; LocationIdentifier$LocationHash doc "The pre-computed location hash (16-character hex string)". The hash is therefore a DETERMINISTIC function of the request's Location/CodeLocation -- it must be pre-computable by the client and stable across calls for the same code location, and it is the last ARN path segment. The recorded response for the probe's code location is LocationHash 'fc14dd5a04207b41' with ARN '...:instrumentationConfig/<service>/<environment>/SNAPSHOT/fc14dd5a04207b41'.
Emulator: returns a different 16-hex value for the same CodeLocation ('52db650af7aae0bf'), and the ARN's trailing segment follows it. Format and ARN shape are correct; the derivation is not AWS's.
Why: LocationHash is a client-addressable identifier. A client that pre-computes the hash from its code location (which the API explicitly supports) and passes it as LocationIdentifier$LocationHash to Get/GetStatus/Delete addresses a configuration the emulator does not have, and a client that stores the ARN from a real account cannot resolve it against the emulator. The variable part here is the hash of the submitted code location; the recorded literal value is one example and is NOT the requirement -- the requirement is that the same CodeLocation yields the same hash AWS yields, and that the ARN's last segment equals the returned LocationHash.
Fix: the CreateInstrumentationConfiguration handler's location-hash derivation (and the shared helper the Get/GetStatus/Delete paths use), so one deterministic AWS-compatible hash of the CodeLocation feeds both LocationHash and the ARN's trailing segment.
Traces: application-signals-instrumentation-config-errors#setup#1, application-signals-instrumentation-config-lifecycle#setup#1.

=== Cause 3 of 4: GetService fabricates a discovered service instead of returning the empty Service AWS returns ===
AWS: Smithy: GetService -> POST /service, output Service (required), StartTime, EndTime, LogGroupReferences; errors only ValidationException (400) and ThrottlingException (429) -- there is no not-found error, because Application Signals services are DISCOVERED from telemetry rather than created. Recorded behavior: for KeyAttributes that have no telemetry in the requested StartTime/EndTime window, AWS answers 200 with a Service carrying neither KeyAttributes nor MetricReferences (both absent/null in the recording) -- it does not echo the requested key attributes back as a discovered service. Hypothesis: the members are populated only from observed telemetry for the window, so an undiscovered key yields them empty.
Emulator: answers 200 but populates the Service: $.Service.KeyAttributes is a fabricated map echoing the request's key attributes ({'Type': 'Service', 'Name': <requested service name>, 'Environment': <requested environment>}) and $.Service.MetricReferences is serialized as an empty list [] where the recording has no value at all.
Why: a client uses GetService to decide whether Application Signals has actually discovered and is monitoring a service. Against the emulator every key attribute set looks discovered, with an echo of the client's own request as evidence, so discovery/onboarding checks, dashboards and "is this service instrumented yet" gates all pass against the emulator and fail in a real account. Serializing an empty MetricReferences list rather than omitting the member additionally makes "no metrics" indistinguishable from "metrics present but empty" for callers that test for member presence.
Fix: the GetService handler and its Service serializer -- populate KeyAttributes/MetricReferences only from state actually observed/registered for the requested window, and omit (not empty-serialize) collection members that have no values.
Traces: application-signals-discovery-and-service-topology#task#3.

=== Cause 4 of 4: ListServiceLevelObjectiveExclusionWindows drops the RecurrenceRule member from each exclusion window ===
AWS: Smithy: ListServiceLevelObjectiveExclusionWindows -> GET /slo/{Id}/exclusion-windows, output ExclusionWindows (required, list of ExclusionWindow, @length max 10). Smithy: ExclusionWindow members are Window (required), StartTime ("Defaults to current time if not specified"), RecurrenceRule ("The recurrence rule for the SLO time window exclusion. Supports both cron and rate expressions") and Reason; RecurrenceRule is a structure with a required Expression (@length 1..1024). The recorded response's exclusion window carries the RecurrenceRule member alongside Window, StartTime and Reason.
Emulator: returns the element with Window, StartTime and Reason only -- RecurrenceRule is absent entirely (the comparator reports $.ExclusionWindows[0] missing/extra against the recorded element). Hypothesis: the recurrence rule supplied to BatchUpdateExclusionWindows is not persisted on the exclusion window, or the List serializer does not emit the member.
Why: a client that sets a recurring exclusion window (cron/rate) and then reads it back sees no recurrence rule, so it concludes the window is one-shot: round-trip read-modify-write loses the recurrence, SLO attainment is computed over periods the client believes are excluded, and drift detection reports a spurious change on every poll. NOTE: the StartTime difference inside the same element is a per-run clock value (AWS defaults StartTime to the current time) and is expressly NOT part of this finding; the missing RecurrenceRule member is.
Fix: the exclusion-window state (BatchUpdateExclusionWindows persistence) plus the ListServiceLevelObjectiveExclusionWindows serializer, so RecurrenceRule round-trips on every stored exclusion window.
Traces: application-signals-slo-exclusion-windows#task#4, application-signals-slo-exclusion-windows#task#6.

Not covered here: the CreateServiceLevelObjective ConflictException message divergence seen in application-signals-slo-error-paths#task#5 is already owned by open issue 20261105T081430Z-application-signals-createservicelevelobjective-conflictexception-message-is-generic-instead-of-naming.md and is deliberately excluded from this bundle.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout: emulator_up=true, 10 fixtures replayed, 62 steps, 55 passed, 7 mismatched, score 88.7%.

Cause 1 -- DeleteInstrumentationConfiguration (fixture application-signals-instrumentation-config-errors, step teardown#5):
  expected: HTTP 200, body $.DeletionStatus = 'DELETED'
  actual:   HTTP 404 (ResourceNotFoundException), $.DeletionStatus missing (None)
  divergence_key: service=application-signals|DeleteInstrumentationConfiguration|status=200->404|keys=$.DeletionStatus,status

Cause 2 -- CreateInstrumentationConfiguration (fixture application-signals-instrumentation-config-errors, step setup#1; also application-signals-instrumentation-config-lifecycle#setup#1):
  expected: $.LocationHash = 'fc14dd5a04207b41'; $.ARN = 'arn:aws:application-signals:us-west-2:000000000000:instrumentationConfig/<service>/probe-env/SNAPSHOT/fc14dd5a04207b41'
  actual:   $.LocationHash = '52db650af7aae0bf'; $.ARN = '...instrumentationConfig/<service>/probe-env/SNAPSHOT/52db650af7aae0bf'
  both HTTP 200; both values are 16 hex chars, so the @length/ARN @pattern constraints hold -- only the derivation from the submitted CodeLocation differs. The two listed fixtures expect the SAME hash for the same code location, confirming the value is deterministic and not per-run.

Cause 3 -- GetService (fixture application-signals-discovery-and-service-topology, step task#3):
  expected: HTTP 200, $.Service.KeyAttributes absent (None), $.Service.MetricReferences absent (None)
  actual:   HTTP 200, $.Service.KeyAttributes = {'Type': 'Service', 'Name': '<requested service name>', 'Environment': 'probe-env'} (type_mismatch), $.Service.MetricReferences = [] (type_mismatch)
  divergence_key: service=application-signals|GetService|status=200->200|keys=$.Service.KeyAttributes,$.Service.MetricReferences

Cause 4 -- ListServiceLevelObjectiveExclusionWindows (fixture application-signals-slo-exclusion-windows, steps task#4 and task#6):
  expected $.ExclusionWindows[0]: {'Reason': 'scheduled maintenance', 'RecurrenceRule': {'Expression': <expression>}, 'StartTime': <ts>, 'Window': {'Duration': 2, 'DurationUnit': 'HOUR'}}
  actual   $.ExclusionWindows[0]: {'Window': {'DurationUnit': 'HOUR', 'Duration': 2}, 'StartTime': <ts>, 'Reason': 'scheduled maintenance'}  -- no RecurrenceRule member
  both HTTP 200. The StartTime values differ only as a per-run clock reading and are excluded from the finding.

Model grounding (services/service-com-amazonaws-applicationsignals-applicationsignals/smithy-model/com-amazonaws-applicationsignals-applicationsignals.json): Service$KeyAttributes/Service$MetricReferences; CreateInstrumentationConfiguration$LocationHash @length 16..16 and $ARN @pattern .../[0-9a-f]{16}$; LocationIdentifier union (CodeLocation "will be hashed internally" / LocationHash "pre-computed ... 16-character hex string"); DeleteInstrumentationConfiguration output DeletionStatus required, enum values [DELETED], error ResourceNotFoundException httpError 404; ExclusionWindow$RecurrenceRule -> RecurrenceRule$Expression required.

## Steps to Reproduce
Boot the emulator from this checkout and replay the application-signals wire fixtures (replay_wire_corpus on trace set simparity-wire-aws-v1).

Cause 1: replay fixture application-signals-instrumentation-config-errors through step teardown#5 -- POST /create-instrumentation-configuration for (InstrumentationType, Service, Environment, SignalType=SNAPSHOT, Location.CodeLocation), then POST /delete-instrumentation-configuration with the same tuple and the matching LocationIdentifier. Expect 200 {"DeletionStatus":"DELETED"}; observe 404 ResourceNotFoundException with no DeletionStatus.

Cause 2: replay application-signals-instrumentation-config-errors#setup#1 (and application-signals-instrumentation-config-lifecycle#setup#1) -- POST /create-instrumentation-configuration with the fixture's Location.CodeLocation and compare $.LocationHash and the trailing ARN segment against the recorded 'fc14dd5a04207b41'; observe a different 16-hex hash for the identical code location.

Cause 3: replay application-signals-discovery-and-service-topology#task#3 -- POST /service with StartTime/EndTime and KeyAttributes for a service that has never reported telemetry; observe Service.KeyAttributes echoed back and Service.MetricReferences serialized as [] where the recording carries neither.

Cause 4: replay application-signals-slo-exclusion-windows#task#4 and #task#6 -- create an SLO, BatchUpdateExclusionWindows with a window carrying a RecurrenceRule plus Reason, then GET /slo/{Id}/exclusion-windows; observe the returned ExclusionWindows[0] has no RecurrenceRule member (ignore the per-run StartTime).
