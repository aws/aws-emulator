Issue-Source: trace-sweep-divergence
Issue-Trace: gameliftstreams-get-stream-group-identifier-validation
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: gameliftstreams-get-stream-group-identifier-validation gameliftstreams-application-shader-caches-identifier-validation gameliftstreams-export-stream-session-files-output-uri-validation gameliftstreams-list-stream-urls-query-validation gameliftstreams-terminate-stream-session-identifier-validation gameliftstreams-disassociate-applications-request-validation gameliftstreams-list-stream-groups-max-results-validation
Issue-Shape-Id: com.amazonaws.gameliftstreams#GameLiftStreams
Issue-Key: gameliftstreams|GetStreamGroup|4xx->4xx|$.Message
Issue-Key: gameliftstreams|ListApplicationShaderCaches|4xx->4xx|$.Message
Issue-Key: gameliftstreams|ExportStreamSessionFiles|4xx->4xx|$.Message
Issue-Key: gameliftstreams|ListStreamUrls|4xx->4xx|$.Message
Issue-Key: gameliftstreams|TerminateStreamSession|4xx->4xx|$.Message
Issue-Key: gameliftstreams|DisassociateApplications|4xx->4xx|$.Message
Issue-Key: gameliftstreams|ListStreamGroups|4xx->4xx|$.Message
Issue-Key: gameliftstreams|ListStreamGroups|4xx->4xx|$headers.x-amzn-errortype
Issue-Commit: 9038a283ab0a537b35729cff6dea9cea13a768a9

# gameliftstreams ValidationException bodies diverge from AWS: raw @pattern echoed, collection value rendered as a count, and a query-parameter coercion failure carries no Message

> _Note: this issue was identified against the code base at commit `9038a283ab0a537b35729cff6dea9cea13a768a9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetStreamGroup, ListApplicationShaderCaches, ExportStreamSessionFiles, ListStreamUrls, TerminateStreamSession, DisassociateApplications, ListStreamGroups

## Description
Three independent root causes make the emulator's HTTP 400 input-refusal BODIES differ from the bodies AWS returns for the same requests. Every one of them answers the right status (400); the divergence is in the error body, which is part of the contract. All three must be closed before this issue is submitted; each cause names its own fixtures on its Traces line.

=== Cause 1 of 3: the @pattern violation clause echoes the model's raw pattern string instead of the pattern as AWS quotes it ===
AWS: a value that fails a member's @pattern is refused with ValidationException / HTTP 400 and the message "1 validation error detected: Value '<the value from the request>' at '<the member's lowerCamel path>' failed to satisfy constraint: Member must satisfy regular expression pattern: <the member's pattern>". The leading sentence, " at '", "' failed to satisfy constraint: Member must satisfy regular expression pattern: " are FIXED text; the echoed value and the member path are taken from the request; the quoted pattern is fixed per member. As AWS quotes it, the pattern is the member's pattern WITHOUT the outer ^...$ that wraps the whole expression, and with no control character in it. Smithy: com.amazonaws.gameliftstreams#Identifier (@length 1..128, @pattern "^(^[a-zA-Z0-9-]+$)|(^arn:aws:gameliftstreams:([^:<LF>]*):([0-9]{12}):([^:<LF>]*)$)$", where <LF> is a LITERAL newline character in the model string) is the shape behind GetStreamGroup.Identifier, ListApplicationShaderCaches.Identifier, TerminateStreamSession.Identifier and ListStreamUrls.StreamGroupIdentifier; com.amazonaws.gameliftstreams#OutputUri (@length 0..1024, @pattern "^s3://.*(/|\\.zip|\\.ZIP)$") is ExportStreamSessionFiles.OutputUri. Every one of those operations models ValidationException httpError 400. The recordings show AWS quoting those two patterns as "(^[a-zA-Z0-9-]+$)|(^arn:aws:gameliftstreams:([^: ]*):([0-9]{12}):([^: ]*)$)" and "s3://.*(/|\\.zip|\\.ZIP)" — i.e. the outer anchors stripped, and the literal newline inside the ARN character class rendered as a space.
Emulator: the clause interpolates the schema's pattern byte-for-byte, so it answers "...Member must satisfy regular expression pattern: ^(^[a-zA-Z0-9-]+$)|(^arn:aws:gameliftstreams:([^:\n]*):([0-9]{12}):([^:\n]*)$)$" (outer ^ and $ present, and a raw newline character embedded in the JSON message) and "...pattern: ^s3://.*(/|\\.zip|\\.ZIP)$". Everything else in the message — the count envelope, the echoed value, the lowerCamel member path — already matches.
Why: the message text is the only machine-readable statement of WHICH rule the request broke. A client, test or support tool that matches the documented sentence does not match the emulator's, and the emulator additionally emits a raw newline inside a JSON string value, so a consumer that logs or displays the message gets a line break AWS never sends.
Fix: the constraint-violation message renderer in the shared validation layer — vendor/smithy-schema-server/src/constraints.rs, Violation::message's ViolationKind::Pattern arm, which does format!("Member must satisfy regular expression pattern: {pattern}"). Quote the pattern the way the services do: drop a leading ^ and trailing $ that wrap the whole expression and normalise an embedded control character to a space, in that one place, so every service's pattern clause agrees.
Traces: gameliftstreams-get-stream-group-identifier-validation#task#1 (also #task#3), gameliftstreams-application-shader-caches-identifier-validation#task#4, gameliftstreams-terminate-stream-session-identifier-validation#task#1 (also #task#2, #task#4), gameliftstreams-list-stream-urls-query-validation#task#5, gameliftstreams-export-stream-session-files-output-uri-validation#task#1 (also #task#2, #task#3). The export fixture's step refuses the OutputUri before the handler runs, so it is NOT covered by the open stub issue 20261127T103056Z-gameliftstreams-exportstreamsessionfiles-stub.md — that issue is about the unimplemented handler, this one about the refusal body the validation layer already produces.

=== Cause 2 of 3: a collection's @length violation renders the offending value as an element COUNT instead of the collection itself ===
AWS: a list member that violates its @length bound is refused with ValidationException / HTTP 400 and the same envelope, the echoed value being the COLLECTION as it was sent: "1 validation error detected: Value '[]' at 'applicationIdentifiers' failed to satisfy constraint: Member must have length greater than or equal to 1". Fixed text: the envelope, " at '", "' failed to satisfy constraint: Member must have length greater than or equal to " and the bound (1) that the model states; variable part: the collection value from the request and the member path. Smithy: DisassociateApplications.ApplicationIdentifiers targets com.amazonaws.gameliftstreams#Identifiers (@length min 1, max 50), required, and the operation models ValidationException httpError 400. Recorded behavior: an empty list is echoed as "[]", i.e. AWS renders the collection in bracketed form rather than describing its size. Hypothesis: a non-empty over-length list is echoed as its bracketed elements by the same rule; only the empty case is recorded here.
Emulator: answers "1 validation error detected: Value '0 items' at 'applicationIdentifiers' failed to satisfy constraint: Member must have length greater than or equal to 1" — the count phrase "0 items" where AWS puts the value.
Why: the clause no longer quotes what the caller sent, so a client cannot tell from the message which value was rejected, and an assertion on the AWS sentence fails on a refusal that is otherwise correct.
Fix: the collection-length path of the same shared renderer — vendor/smithy-schema-server/src/constraints.rs, ValidatingSerializer::walk_list (and the walk_map twin) which pass format!("{index} items") as the displayed value into check_collection_length. Render the collection in the bracketed form AWS echoes, keeping the existing MAX_ECHOED_VALUE elision.
Traces: gameliftstreams-disassociate-applications-request-validation#task#5 (also #task#7, #task#8).

=== Cause 3 of 3: a MaxResults query value that is not an integer is answered with an EMPTY ValidationException body instead of the service's coercion error ===
AWS: ListStreamGroups binds MaxResults to the MaxResults query parameter (Smithy: GET /streamgroups, 200, @paginated, MaxResults -> com.amazonaws.gameliftstreams#MaxResults, integer @range 1..100). A query value that is not an integer at all fails COERCION before any constraint is judged, and the recorded refusal is an HTTP 400 whose body carries Message "'ten' can not be converted to Integer" — fixed text "' can not be converted to Integer" after the quoted value, with the rejected query value from the request as its only variable part — and whose x-amzn-errortype header is NOT ValidationException (the recording's shape assertion on that header rejects the value 'ValidationException'). Hypothesis: the frontend reports it as a serialization/coercion fault (SerializationException) rather than as a modelled validation error; the recorded header value is what the fixture asserts, so take the code from the recording.
Emulator: answers HTTP 400 with x-amzn-errortype: ValidationException and a body with NO Message member at all ($.Message is absent). ValidationException.Message is modelled @required, so the emulator's own body is also not a serialisable instance of the error it claims to be returning.
Why: a client gets an error it cannot explain — no message to log or surface, and an error code that routes it into the modelled-validation branch rather than the request-serialisation branch. Any SDK that reads Message for its exception string reports an empty error.
Fix: the query-parameter deserialisation path for integer-bound members (the generated deserialiser's coercion step and the pre-handler refusal it raises, vendor/smithy-schema-server + vendor/smithy-server-runtime pre_dispatch rendering) — emit the coercion error with the recorded error code and the "'<value>' can not be converted to Integer" Message, and never serialise a ValidationException without its required Message.
Traces: gameliftstreams-list-stream-groups-max-results-validation#task#3.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (13 fixtures, 98 steps, 71 passed, 27 mismatched, score 72.4%; emulator_up=true, bootstrap_signal=false).

Cause 1 — @pattern clause (status 400 -> 400, key $.Message):
  fixture gameliftstreams-get-stream-group-identifier-validation step task#1 (GetStreamGroup, identifier 'sg_bad.id')
  expected: "1 validation error detected: Value 'sg_bad.id' at 'identifier' failed to satisfy constraint: Member must satisfy regular expression pattern: (^[a-zA-Z0-9-]+$)|(^arn:aws:gameliftstreams:([^: ]*):([0-9]{12}):([^: ]*)$)"
  actual:   "1 validation error detected: Value 'sg_bad.id' at 'identifier' failed to satisfy constraint: Member must satisfy regular expression pattern: ^(^[a-zA-Z0-9-]+$)|(^arn:aws:gameliftstreams:([^:\n]*):([0-9]{12}):([^:\n]*)$)$"
  same divergence, same two deltas (outer ^...$ added, literal newline where AWS shows a space):
    gameliftstreams-application-shader-caches-identifier-validation#task#4 (ListApplicationShaderCaches, 'a_9ZY8X7Wv9')
    gameliftstreams-terminate-stream-session-identifier-validation#task#1 (TerminateStreamSession, 'sg_9ZY8X7Wv6'; also task#2, task#4)
    gameliftstreams-list-stream-urls-query-validation#task#5 (ListStreamUrls, streamGroupIdentifier 'sg_1AB2C3De9')
  and for OutputUri:
    gameliftstreams-export-stream-session-files-output-uri-validation#task#1 (also task#2, task#3)
    expected: "...at 'outputUri' failed to satisfy constraint: Member must satisfy regular expression pattern: s3://.*(/|\\.zip|\\.ZIP)"
    actual:   "...at 'outputUri' failed to satisfy constraint: Member must satisfy regular expression pattern: ^s3://.*(/|\\.zip|\\.ZIP)$"
  source: vendor/smithy-schema-server/src/constraints.rs line 152 — ViolationKind::Pattern(pattern) => format!("Member must satisfy regular expression pattern: {pattern}")
  model: Identifier / OutputUri @pattern as quoted in the description (smithy_query on services/service-com-amazonaws-gameliftstreams-gameliftstreams/smithy-model/com-amazonaws-gameliftstreams-gameliftstreams.json)

Cause 2 — collection @length clause (status 400 -> 400, key $.Message):
  fixture gameliftstreams-disassociate-applications-request-validation step task#5 (DisassociateApplications, ApplicationIdentifiers=[])
  expected: "1 validation error detected: Value '[]' at 'applicationIdentifiers' failed to satisfy constraint: Member must have length greater than or equal to 1"
  actual:   "1 validation error detected: Value '0 items' at 'applicationIdentifiers' failed to satisfy constraint: Member must have length greater than or equal to 1"
  also task#7, task#8 of the same fixture
  source: vendor/smithy-schema-server/src/constraints.rs line 1309 — this.check_collection_length(schema, index, &format!("{index} items")) (asserted by its own tests at lines 2436, 2581, 2599)

Cause 3 — ListStreamGroups MaxResults='ten' (status 400 -> 400):
  fixture gameliftstreams-list-stream-groups-max-results-validation step task#3
  expected: body Message "'ten' can not be converted to Integer"; x-amzn-errortype a value that is NOT 'ValidationException'
  actual:   no $.Message member in the body at all (got None); x-amzn-errortype: 'ValidationException'
  model: ListStreamGroups GET /streamgroups, MaxResults httpQuery MaxResults -> MaxResults integer @range 1..100; ValidationException.Message is @required

## Steps to Reproduce
Boot the emulator and point a gameliftstreams client (or plain HTTP) at it.

Cause 1: GET /streamgroups/sg_bad.id -> read Error.Message and compare the quoted regex with the AWS form (no outer ^...$, no embedded newline). Same with GET /applications/a_9ZY8X7Wv9/shadercaches, DELETE /streamgroups/<id>/streamsessions/sg_9ZY8X7Wv6, GET /streamurls?StreamGroupIdentifier=sg_1AB2C3De9, and PUT /streamgroups/<id>/streamsessions/<session>/exportfiles with OutputUri 's3://amzn-s3-demo-destination-bucket/session-files.tar'.

Cause 2: POST /streamgroups/<identifier>/disassociations with {"ApplicationIdentifiers": []} -> Error.Message echoes '0 items' where AWS echoes '[]'.

Cause 3: GET /streamgroups?MaxResults=ten -> emulator answers 400 with x-amzn-errortype: ValidationException and an empty body; AWS answers 400 carrying Message "'ten' can not be converted to Integer" under a non-ValidationException error type.

Or re-replay the fixtures/steps named in Issue-Trace/Issue-Step and Issue-Trace-Ids.
