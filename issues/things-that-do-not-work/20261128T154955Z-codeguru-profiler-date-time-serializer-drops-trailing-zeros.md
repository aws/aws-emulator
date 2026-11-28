Issue-Source: trace-sweep-divergence
Issue-Trace: codeguruprofiler-recommendations-missing-group-not-found
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: codeguruprofiler-recommendations-missing-group-not-found
Issue-Shape-Id: com.amazonaws.codeguruprofiler#CodeGuruProfiler
Issue-Key: codeguru-profiler|GetRecommendations|2xx->2xx|$.profileEndTime
Issue-Key: codeguru-profiler|GetRecommendations|2xx->2xx|$.profileStartTime
Issue-Commit: 72f48f42de416ff1071fa90a732a0bc13a8c145e

# codeguru-profiler date-time serializer drops trailing zeros from millisecond fraction

> _Note: this issue was identified against the code base at commit `72f48f42de416ff1071fa90a732a0bc13a8c145e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetRecommendations

## Description
Cause 1 of 1: the emulator's `date-time` timestamp serializer renders the sub-second fraction with trailing zeros stripped, so CodeGuru Profiler response timestamps come back with fewer than three fractional digits.

AWS: Smithy model `com-amazonaws-codeguruprofiler-codeguruprofiler`, operation `GetRecommendations` (GET /internal/profilingGroups/{profilingGroupName}/recommendations, success 200). Its output members `profileStartTime` and `profileEndTime` are both `@required` and target `Timestamp` with `timestampFormat: date-time`. Recorded behavior: AWS renders every `date-time` output value with a fixed, three-digit millisecond fraction and a `Z` zone designator — i.e. the shape `YYYY-MM-DDThh:mm:ss.sssZ`, always exactly three digits after the decimal point, including when the millisecond component ends in one or two zeros. The recorded response carries `profileStartTime` and `profileEndTime` in exactly that three-digit form.

Emulator: the same two members are serialized with the fraction's trailing zeros removed, producing a two-digit fraction when the millisecond value is a multiple of ten (and, by the same rule, a one-digit fraction for a multiple of a hundred and no fraction at all for a whole second). Replay reports, on the 200 response, `$.profileStartTime: value_mismatch` and `$.profileEndTime: value_mismatch`: where the recording has `...:19.140Z` the emulator emits `...:19.14Z`, and where it has `...:19.140Z` for the end time it emits `...:19.14Z`. The instant conveyed is the same; only the rendering differs. The status (200) and the rest of the payload match, so this is purely the serializer's fractional-second formatting, not a state or clock difference. The specific date, time and group name above are values of this one recording; the requirement is the fixed three-digit fraction and the `Z` suffix, not those digits.

Why: a client that compares, hashes, signs or caches on the literal response bytes, or that parses `date-time` with a strict fixed-width `.SSS` pattern (common in generated and hand-rolled deserializers, log pipelines and recorded-response assertions), rejects or mis-parses the emulator's value even though it would accept AWS's. Because the defect lives in the shared `date-time` serializer rather than in this one operation, every timestamp this service emits whose millisecond value happens to end in a zero is rendered differently from AWS — roughly one response in ten by chance — making the divergence intermittent and hard for a client author to attribute.

Fix: the `date-time` timestamp serializer used by this service's response serialization path (reached from the `GetRecommendations` output serializer) must pad the sub-second fraction to exactly three digits rather than emitting the shortest representation. Apply it where the timestamp is formatted, so every `date-time`-formatted output member of the service is corrected at once, not just these two.

Traces: codeguruprofiler-recommendations-missing-group-not-found#task#2 (GetRecommendations, 200, `$.profileStartTime` and `$.profileEndTime`). Serializer entry point: services/service-com-amazonaws-codeguruprofiler-codeguruprofiler/src/ops/get_recommendations.rs.

Not covered by any open issue: 20261128T063424Z-codeguru-profiler-error-responses-carry-non-aws-message.md covers error-message text on 404/400 responses, 20261022T213013Z-codeguru-profiler-postagentprofile-accepts-an-invalid-agent-profile-with-204.md covers PostAgentProfile validation, and 20261127T103329Z-codeguruprofiler-batchgetframemetricdata-computes-aggregatedrelativetotaltime-against-the-wrong-denominator.md covers frame-metric arithmetic. None of them concerns timestamp rendering on a 200 response.

## Evidence
Fixture codeguruprofiler-recommendations-missing-group-not-found, step task#2, operation GetRecommendations, verdict mismatch, expected status 200, actual status 200.

Divergence detail as reported by replay:
  $.profileStartTime: value_mismatch (expected '2026-11-28T13:48:19.140Z', got '2026-11-28T13:48:19.14Z')
  $.profileEndTime: value_mismatch (expected '2026-11-28T15:38:19.140Z', got '2026-11-28T15:38:19.14Z')

Expected (AWS): `date-time` output members rendered as YYYY-MM-DDThh:mm:ss.sssZ with the fractional part padded to exactly three digits, so a millisecond value of 140 renders as `.140`.
Actual (emulator): the fractional part is emitted in shortest form with trailing zeros removed, so a millisecond value of 140 renders as `.14`.

Fixed text of the requirement: the `YYYY-MM-DDThh:mm:ss.sssZ` layout, three fractional digits, literal `Z`. Variable parts: the date, time-of-day and millisecond digits themselves, which come from the emulator's state/clock and legitimately differ per run; the recorded 2026-11-28 values are only this run's filling of the template.

Smithy citation: GetRecommendations output members profileStartTime and profileEndTime -> Timestamp, @required, timestampFormat: date-time; http { method: GET, uri: /internal/profilingGroups/{profilingGroupName}/recommendations, code: 200 }.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it.
2. Replay the wire fixture corpus simparity-wire-aws-v1 (replay_wire_corpus), or replay fixture `codeguruprofiler-recommendations-missing-group-not-found` alone.
3. Observe step `task#2`: a GetRecommendations call (GET /internal/profilingGroups/<profilingGroupName>/recommendations?startTime=...&endTime=...) that returns 200.
4. Compare the response's `profileStartTime` / `profileEndTime` against the recording: whenever the millisecond component ends in a zero, the emulator emits a two-digit fraction (e.g. `.14Z`) where AWS emits the three-digit `.140Z`.
5. Any GetRecommendations (or other codeguru-profiler operation returning a `date-time` member) whose timestamp lands on a millisecond value that is a multiple of ten reproduces it; a millisecond value with no trailing zero renders identically and passes, which is why only some steps diverge.
