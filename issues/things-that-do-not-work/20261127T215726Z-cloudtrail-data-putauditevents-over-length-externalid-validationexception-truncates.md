Issue-Source: trace-sweep-divergence
Issue-Trace: cloudtrail-data-external-id-constraints
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cloudtrail-data-external-id-constraints
Issue-Shape-Id: com.amazonaws.cloudtraildata#CloudTrailDataService
Issue-Key: cloudtrail-data|PutAuditEvents|4xx->4xx|$.message
Issue-Commit: 1a8d6da9d066bb6b81849373d117fc388454286a

# cloudtrail-data PutAuditEvents over-length externalId ValidationException truncates the echoed value and appends an emulator-authored "... [N bytes]" marker

> _Note: this issue was identified against the code base at commit `1a8d6da9d066bb6b81849373d117fc388454286a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutAuditEvents

## Description
One root cause, in the shared constraint-violation message serializer (the renderer of the template's '<value>' slot) as reached by PutAuditEvents request validation of the `externalId` query parameter. The status (400), the error code and every other slot of the sentence are already correct; the defect is confined to how the REJECTED VALUE is rendered into the message when it is long.

Note on existing issues: no OPEN issue covers this. The open issue 20261028T004339Z-cloudtrail-data-putauditevents-validationexception-renders-the-modelled-pattern.md concerns a DIFFERENT slot of the SAME template (the '<pattern>' slot of a @pattern violation, where the modelled regex keeps its ^...$ anchors) and does not cover the value slot. The resolved issues 20261027T001848Z-cloudtrail-data-putauditevents-validationexception-for-an-over-length.md (how a LIST value is summarized into the value slot) and 20261030T140738Z-config-getresourceconfighistory-s-over-length-resourceid-validationexception-clips.md (the same clip-and-annotate defect in another service) are resolved and therefore cover nothing here; the cloudtrail-data string-value path still clips.

=== Cause 1 of 1: the '<value>' slot of the constraint-violation message is clipped and annotated with a byte count instead of echoing the request's value verbatim ===
AWS: PutAuditEvents refuses an `externalId` longer than the modelled maximum with HTTP 400 and error code ValidationException, worded with the standard constraint-violation template. Smithy (services/service-com-amazonaws-cloudtraildata-cloudtraildataservice/smithy-model/com-amazonaws-cloudtraildata-cloudtraildataservice.json): PutAuditEventsRequest$externalId -> ExternalId, httpQuery "externalId", @length {min 2, max 1224}, @pattern "^[\\w+=,.@:\\/-]*$"; ValidationException is NOT one of the operation's modelled errors (ChannelInsufficientPermission, ChannelNotFound, ChannelUnsupportedSchema, DuplicatedAuditEventId, InvalidChannelARN, UnsupportedOperationException), so the refusal is produced by the shared constraint layer ahead of the handler. Fixed text of the template for a @length-max violation: "<N> validation error(s) detected: Value '<value>' at '<member>' failed to satisfy constraint: Member must have length less than or equal to <max>". Its variable parts are all derived from the request and the model: the violation count, the rejected value taken VERBATIM and IN FULL from the request, the member name ('externalId'), and the modelled maximum (1224). The rule for the value slot is that the whole rejected value appears, however long it is — AWS does not abbreviate, clip or summarize it, and emits no byte count. Recorded behavior confirms it: the recording's expected message carries the complete over-length externalId.
Emulator: gets the status (400), the violation count (1), the member path ('externalId') and the limit (1224) right, then renders the value slot as a PREFIX of the rejected value followed by an emulator-authored marker of the form "... [<n> bytes]" — in the recorded step, the value appears as roughly its first ~250 characters followed by "... [1225 bytes]" instead of the full 1225-character value, so the message is "1 validation error detected: Value 'kkkk…kkk... [1225 bytes]' at 'externalId' failed to satisfy constraint: Member must have length less than or equal to 1224" where AWS emits the same sentence with the entire value in the quotes. The "... [N bytes]" annotation is not part of any AWS message.
Why: the error message is a requirement. A client, SDK or recorder that reconstructs the refusal from its own request plus the model — or that parses the quoted value back out of the ValidationException to report which input it sent was rejected — gets a value that does not compare equal to what it sent and is not a value AWS would ever emit, and it additionally sees a bracketed byte count that no AWS message contains. Because the slot is produced by the SHARED serializer, the divergence is not confined to externalId: every long string value rejected by a @length constraint in this service (and any service routed through the same serializer) is reported clipped.
Fix: the shared constraint-violation message serializer in the framework validation layer, in the branch that formats the '<value>' slot for a string @length violation — it must interpolate the rejected value verbatim and in full, with no length cap and no appended "... [N bytes]" annotation. The length ENFORCEMENT itself, the status, the error code, the violation count, the member path and the modelled maximum are already correct and must not regress; neither must the ARN-before-constraint rejection ordering nor the InvalidChannelARN wording that the rest of this fixture clears.
Traces: cloudtrail-data-external-id-constraints#task#4 (PutAuditEvents with an externalId of 1225 characters).

## Evidence
From the wire-fixture replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (26 steps, 25 passed, 1 mismatch).

Fixture cloudtrail-data-external-id-constraints, step task#4 — service cloudtrail-data, operation PutAuditEvents (POST /PutAuditEvents?channelArn=...&externalId=...).
Verdict: mismatch. Expected status 400, actual status 400 — the ONLY diverging key is $.message.

Expected (AWS, from the recording):
  "1 validation error detected: Value '<the full 1225-character externalId, echoed verbatim>' at 'externalId' failed to satisfy constraint: Member must have length less than or equal to 1224"

Actual (emulator):
  "1 validation error detected: Value '<the first ~250 characters of the same externalId>... [1225 bytes]' at 'externalId' failed to satisfy constraint: Member must have length less than or equal to 1224"

Which parts are fixed text and which are fill-ins:
  fixed text   -> "<N> validation error detected: Value '" … "' at '" … "' failed to satisfy constraint: Member must have length less than or equal to " … (and the plural "validation errors detected" when N > 1)
  from request -> the violation count (1), the rejected value (echoed VERBATIM and IN FULL), the member name ('externalId')
  from model   -> the maximum (1224), from ExternalId's @length trait
  emulator-only, not part of the template -> the clip of the value plus the "... [1225 bytes]" suffix
Only the value slot diverges. Nothing per-run (no id, timestamp or request id) is part of this requirement; the specific repeated-'k' value in the recording merely FILLS the slot and is not itself the requirement.

Model evidence (smithy_query on services/service-com-amazonaws-cloudtraildata-cloudtraildataservice/smithy-model/com-amazonaws-cloudtraildata-cloudtraildataservice.json): PutAuditEvents input member externalId -> ExternalId, httpQuery "externalId", @length {min 2, max 1224}, @pattern "^[\\w+=,.@:\\/-]*$"; the operation models no ValidationException, so the message comes from the shared constraint layer.

The other 3 steps of this fixture (and the other 6 fixtures in the corpus) passed, so the 400/ValidationException classification and the rest of the externalId constraint handling are already right.

## Steps to Reproduce
1. Boot the emulator built from this checkout and point a cloudtrail-data client at it, constructed with client-side parameter validation DISABLED (botocore Config(parameter_validation=False)) so the service, not the SDK, rejects the value.
2. Build any well-formed channel ARN for the call's channelArn (the refusal under test is produced by the constraint layer ahead of channel resolution, so the channel need not exist).
3. Build an externalId that satisfies ExternalId's @pattern but exceeds its modelled maximum: any string of 1225 characters drawn from [\\w+=,.@:/-] (e.g. 'k' * 1225).
4. Call PutAuditEvents(channelArn=<that ARN>, externalId=<that 1225-char value>, auditEvents=[one well-formed event]).
5. Expect HTTP 400, code ValidationException, and message "1 validation error detected: Value '<the entire 1225-character value>' at 'externalId' failed to satisfy constraint: Member must have length less than or equal to 1224" — the value echoed in full, with no ellipsis and no byte count.
6. Observe instead that the quoted value is truncated after a few hundred characters and followed by "... [1225 bytes]", with every other slot of the sentence correct.
7. Generalise the fix past this one member: repeat with other over-length string values rejected by a @length constraint in this service and confirm their messages also echo the value in full — the slot is produced by the shared serializer, not by this member.
8. Re-replay fixture cloudtrail-data-external-id-constraints, step task#4 to confirm, and confirm the fixture's other steps still pass.
