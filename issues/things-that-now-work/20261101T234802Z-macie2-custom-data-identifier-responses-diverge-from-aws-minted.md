Issue-Source: trace-sweep-divergence
Issue-Trace: macie2-custom-data-identifier-not-found
Issue-Step: setup#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: macie2-custom-data-identifier-not-found
Issue-Shape-Id: com.amazonaws.macie2#Macie2
Issue-Key: macie2|CreateCustomDataIdentifier|2xx->2xx|$.customDataIdentifierId
Issue-Key: macie2|GetCustomDataIdentifier|4xx->4xx|$.message
Issue-Key: macie2|GetCustomDataIdentifier|2xx->2xx|$.ignoreWords
Issue-Key: macie2|GetCustomDataIdentifier|2xx->2xx|$.keywords
Issue-Key: macie2|GetCustomDataIdentifier|2xx->2xx|$.severityLevels
Issue-Commit: 81e1f22df7dda30cfdd920fff09643b1094d9644

# Custom data identifier responses diverge from AWS: minted id format, not-found message wording, omitted response members

> _Note: this issue was identified against the code base at commit `81e1f22df7dda30cfdd920fff09643b1094d9644`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetCustomDataIdentifier

## Description
Three independent root causes in the macie2 custom-data-identifier plane, all grounded in replayed wire steps of fixture `macie2-custom-data-identifier-not-found`. Status codes are right in all three; what diverges is the identity, the error wording and the body of the custom-data-identifier responses. A fixer must close all three before submitting.

Coverage note: the open issue `20261101T224706Z-macie2-3-net-new-behavioral-root-causes-createmember.md` describes the not-found message wording (its Cause 3) from the BEHAVIORAL suite, but it carries no wire replay key, so the wire step below (`task#4`) is not covered by it and is re-filed here with its fixture binding. Causes 1 and 3 appear in no open issue. The scaffold-stub issues open for macie2 name CreateAllowList, CreateClassificationJob, CreateFindingsFilter, CreateSampleFindings, GetAutomatedDiscoveryConfiguration, GetClassificationExportConfiguration, GetFindingsPublicationConfiguration, ListMembers, TestCustomDataIdentifier and UpdateAutomatedDiscoveryConfiguration — none of the operations below.

=== Cause 1 of 3: CreateCustomDataIdentifier mints an id in the wrong format ===
AWS: Smithy (services/service-com-amazonaws-macie2-macie2/smithy-model/com-amazonaws-macie2-macie2.json) models CreateCustomDataIdentifier as POST /custom-data-identifiers, success 200, output `{customDataIdentifierId}` targeting a bare `__string` with no pattern, so the model pins no grammar — recorded behavior only. Macie assigns a custom data identifier a UUID: the recording returns a canonical lowercase 8-4-4-4-12 hex UUID (36 characters, four hyphens), and the same fixture later addresses `GET /custom-data-identifiers/{id}` with a UUID-shaped id. The requirement is the FORMAT (a canonical UUID string); the particular UUID in any one run is a per-run value and is not part of it.
Emulator: the minted id is 22 lowercase base-36 characters with no hyphens (the replay observed `g23qyx7a3zx06j253i38ox`). The handler mints it with `crate::state::new_resource_id`, which is documented to follow the `^[a-z0-9]{22}$` grammar the model pins for CreateAllowListResponse$id — that is the ALLOW-LIST id grammar, applied here to a resource whose ids AWS issues as UUIDs.
Why: a client that round-trips, stores or validates the id breaks. Anything that parses the returned value as a UUID (a typed UUID column, a `uuid.UUID(...)` call, an id-format assertion, an ARN whose resource segment is matched against a UUID pattern) rejects a 22-character base-36 token, so a create that AWS reports as succeeding fails inside the client. Id-shaped fixtures and golden files recorded against AWS also stop matching.
Fix: the id minting that CreateCustomDataIdentifier's handler calls — `new_resource_id` in services/service-com-amazonaws-macie2-macie2/src/state.rs and the `RESOURCE_ID_LEN`/grammar comment above it — must produce a canonical UUID for a custom data identifier (uniqueness can stay seed-derived; only the rendering changes), and the ARN built by `custom_data_identifier_arn` from the same id must carry it unchanged.
Traces: macie2-custom-data-identifier-not-found#setup#3.

=== Cause 2 of 3: the GetCustomDataIdentifier not-found message is emulator-authored, not AWS's wording ===
AWS: Smithy models GetCustomDataIdentifier as GET /custom-data-identifiers/{id} with ResourceNotFoundException among its errors (httpError 404, client kind). For an id that was never created, the recorded response is HTTP 404 with message "Custom Data Identifier not found for id: <id>". The fixed text is "Custom Data Identifier not found for id: "; the only variable part is the id, taken verbatim from the request's `{id}` path label.
Emulator: HTTP 404 and the error code match, but the message is reworded: "The request failed because the custom data identifier <id> doesn't exist." — fixed text "The request failed because the custom data identifier " + the request id + " doesn't exist.". The string is authored in services/service-com-amazonaws-macie2-macie2/src/ops/get_custom_data_identifier.rs (the `ResourceNotFoundException` built when `state.identifier(...)` returns `None`, lines 202-212).
Why: the message is part of the contract. A client, log scraper or support tool that matches AWS's sentence — or surfaces it to a user — sees text no AWS region produces, so message-keyed branching and diagnostics diverge even though the error class is right.
Fix: the error construction in that operation's handler — build the ResourceNotFoundException message from AWS's template, interpolating only the requested id.
Traces: macie2-custom-data-identifier-not-found#task#4.

=== Cause 3 of 3: GetCustomDataIdentifier omits keywords, ignoreWords and the defaulted severityLevels ===
AWS: Smithy models the GetCustomDataIdentifier output with `keywords` and `ignoreWords` (`__listOf__string`) and `severityLevels` (`SeverityLevelList`) among its members. Recorded behavior: a read of an identifier created WITHOUT keywords, ignore words or severity levels answers 200 with `keywords: []` and `ignoreWords: []` PRESENT as empty arrays, and with `severityLevels` carrying the service-applied default — one level, `{"occurrencesThreshold": 1, "severity": "MEDIUM"}`. The threshold/severity pair is the documented default Macie assigns when the create names no severityLevels (the same way it applies the documented `maximumMatchDistance` default of 50), so it is a rule, not a recorded sample.
Emulator: all three members are absent from the 200 body (the replay reports `$.keywords: missing (expected [], got None)`, `$.ignoreWords: missing (expected [], got None)`, `$.severityLevels: missing (expected [{'occurrencesThreshold': 1, 'severity': 'MEDIUM'}], got None)`). Two defects combine: `CustomDataIdentifier::keywords_output`/`ignore_words_output`/`severity_levels_output` in services/service-com-amazonaws-macie2-macie2/src/state.rs route every list through `non_empty`, which deliberately maps an empty list to `None`; and CreateCustomDataIdentifier stores `severity_levels` as `input.severity_levels.unwrap_or_default()`, i.e. empty, so no severity-level default is ever applied even though the neighbouring `maximum_match_distance` default is.
Why: two different client-visible failures. First, a client that reads the collections gets `None`/absent where AWS gives an empty list, so straightforward code (`for w in resp["keywords"]`, `len(resp["ignoreWords"])`) raises a KeyError/NoneType error instead of iterating nothing. Second, and worse, the effective severity configuration of the identifier is unreadable: a client that inspects `severityLevels` to learn what severity findings will carry, or that reads-modifies-writes the level set, sees no configuration at all and concludes none is in effect, when on AWS a MEDIUM level at threshold 1 is.
Fix: two places. In the state projection (src/state.rs), report the modeled list members as present-and-empty rather than collapsing an empty list to `None` for the members AWS always serializes; and in CreateCustomDataIdentifier's handler (src/ops/create_custom_data_identifier.rs, where the record is built), apply the service default severity level when the request names none, so every later read projects it out of state. `ListCustomDataIdentifiers` summaries must stay consistent with whatever the read reports.
Traces: macie2-custom-data-identifier-not-found#task#5.

## Evidence
Replay of trace set simparity-wire-aws-v1: 9 steps, 3 passed, 5 mismatched, 1 blocked; score 33.3%. Emulator up (/__admin/health 200). All three causes below are mismatch verdicts on fixture `macie2-custom-data-identifier-not-found`, service `macie2`, none carrying a stub_path or owned_by.

Cause 1 — step setup#3, CreateCustomDataIdentifier [mismatch], status 200 -> 200:
  divergence_key: service=macie2|CreateCustomDataIdentifier|status=200->200|keys=$.customDataIdentifierId
  detail: $.customDataIdentifierId: value_mismatch (expected '<shaped-not-asserted>', got 'g23qyx7a3zx06j253i38ox')
  Expected: a canonical UUID (36 chars, lowercase hex, 8-4-4-4-12).
  Actual:   22 lowercase base-36 characters, no hyphens. The specific token is per-run; the FORMAT is the divergence.

Cause 2 — step task#4, GetCustomDataIdentifier [mismatch], status 404 -> 404:
  divergence_key: service=macie2|GetCustomDataIdentifier|status=404->404|keys=$.message
  detail: $.message: value_mismatch (expected 'Custom Data Identifier not found for id: 5b0d7c4e-3f2a-4e61-9c8d-1a2b3c4d5e6f', got "The request failed because the custom data identifier 5b0d7c4e-3f2a-4e61-9c8d-1a2b3c4d5e6f doesn't exist.")
  Expected message: fixed prefix "Custom Data Identifier not found for id: " + the id from the request path.
  Actual message:   "The request failed because the custom data identifier " + that id + " doesn't exist."
  HTTP 404 / ResourceNotFoundException match; only the wording diverges. The uuid shown is the fixture's requested id, not part of the requirement.

Cause 3 — step task#5, GetCustomDataIdentifier [mismatch], status 200 -> 200:
  divergence_key: service=macie2|GetCustomDataIdentifier|status=200->200|keys=$.ignoreWords,$.keywords,$.severityLevels
  detail:
    $.ignoreWords: missing (expected [], got None)
    $.keywords: missing (expected [], got None)
    $.severityLevels: missing (expected [{'occurrencesThreshold': 1, 'severity': 'MEDIUM'}], got None)
  Code paths: src/state.rs `non_empty` (lines 225-231) behind `keywords_output`/`ignore_words_output`/`severity_levels_output` (lines 194-206); src/ops/create_custom_data_identifier.rs line 279 stores `input.severity_levels.clone().unwrap_or_default()` with no default applied, while line 276 does apply `DEFAULT_MAXIMUM_MATCH_DISTANCE`.

## Steps to Reproduce
Boot the emulator and replay fixture `macie2-custom-data-identifier-not-found` of trace set simparity-wire-aws-v1, or reproduce by hand with a botocore macie2 client pointed at the emulator (enable_macie first if get_macie_session answers AccessDeniedException "Macie is not enabled."):

Cause 1 (step setup#3):
  r = macie.create_custom_data_identifier(name="any-name", regex="[0-9]{3}-[0-9]{2}-[0-9]{4}")
  -> observe r["customDataIdentifierId"] is 22 lowercase base-36 characters; expected a canonical UUID. `uuid.UUID(r["customDataIdentifierId"])` must succeed and does not.

Cause 2 (step task#4):
  macie.get_custom_data_identifier(id="5b0d7c4e-3f2a-4e61-9c8d-1a2b3c4d5e6f")  # never created
  -> observe HTTP 404 ResourceNotFoundException with message "The request failed because the custom data identifier <id> doesn't exist."; expected "Custom Data Identifier not found for id: <id>".

Cause 3 (step task#5):
  g = macie.get_custom_data_identifier(id=r["customDataIdentifierId"])  # the identifier created above, no keywords/ignoreWords/severityLevels given
  -> observe "keywords", "ignoreWords" and "severityLevels" all ABSENT from the 200 response; expected keywords == [] and ignoreWords == [] present, and severityLevels == [{"occurrencesThreshold": 1, "severity": "MEDIUM"}] from the service-applied default.
