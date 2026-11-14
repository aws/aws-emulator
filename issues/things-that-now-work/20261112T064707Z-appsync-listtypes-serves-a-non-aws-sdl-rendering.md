Issue-Source: trace-sweep-divergence
Issue-Trace: appsync-graphql-api-and-type-lifecycle
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: appsync-graphql-api-and-type-lifecycle appsync-list-types-after-create-type appsync-list-types-lifecycle
Issue-Shape-Id: com.amazonaws.appsync#AWSDeepdishControlPlaneService
Issue-Key: appsync|ListTypes|2xx->2xx|$.types[arn="arn:aws:appsync:us-west-2:000000000000:apis/fdh4iesrhyi2lg7pbfkvrxqdbg/types/Blog"].definition
Issue-Key: appsync|ListTypes|2xx->2xx|$.types[arn="arn:aws:appsync:us-west-2:000000000000:apis/ansu3y45vh4khfa4v7cklclv54/types/Widget"].definition
Issue-Key: appsync|ListTypes|2xx->2xx|$.types[arn="arn:aws:appsync:us-west-2:000000000000:apis/mj2zgkngsvirtiocecygadzl4n/types/Query"].definition
Issue-Commit: 9a53e127c8f44bbc45a6320e6c70fd06205845c1

# AppSync ListTypes serves a non-AWS SDL rendering of Type$definition (3 causes)

> _Note: this issue was identified against the code base at commit `9a53e127c8f44bbc45a6320e6c70fd06205845c1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListTypes

## Description
One shared fault, three net-new clusters: ListTypes does not re-render Type$definition the way AWS does. AWS prints the type from its own parsed model in a fixed layout; the emulator serves either the client's raw SDL text or its own pretty-printer's output. All three clusters are service=appsync, none is stub-blocked and none is owned by an open issue (owned_by was empty for all three in this replay). All three must be closed before this issue is submitted.

Related open issues that do NOT cover these: 20261112T042459Z-appsync-type-projections-diverge-list-responses-serve-the.md describes the same behaviour, but its recorded replay keys embed the per-run api id in the matched JSON path (appsync|ListTypes|2xx->2xx|$.types[arn="...apis/<apiId>/types/Blog"].definition), so it does not own these clusters and a replay driven from it does not cover them - the ownership must be re-attached to the keys this replay reports. 20261108T071728Z-appsync-type-operations-diverge-deleted-types-stay-readable.md covers the non-rendered definition on GetType only, and 20261111T092526Z-appsync-getintrospectionschema-returns-a-non-canonical-sdl-the.md covers GetIntrospectionSchema - both different operations. 20261112T042459Z-appsync-listtypes-refuses-an-unresolvable-apiid-as-400.md covers ListTypes' not-found refusal, not its body.

Cause 1 of 3 - ListTypes (format=SDL) echoes the submitted single-line definition instead of AWS's rendering (fixture appsync-graphql-api-and-type-lifecycle, step task#4).
AWS: recorded behavior only. Smithy: ListTypes = GET /v1/apis/{apiId}/types with required query `format` (TypeDefinitionFormat SDL|JSON), output types: TypeList + nextToken; Type = {name, description, arn, definition (String, "The type definition"), format} with no formatting trait on definition. The recording shows AWS re-prints the parsed type rather than echoing the request: "type <Name>\n\n{\n" then each field as "<field>: <Type>" with a blank line after it and an extra blank line between consecutive fields, closing with "}\n" - the document is multi-line and newline-terminated. The type and field names are request values; the layout is the requirement.
Emulator: 200 with every other Type member correct, but types[].definition is the raw request string - expected 'type <Name>\n\n{\n<f1>: <T1>\n\n\n<f2>: <T2>\n\n}\n', got 'type <Name> { <f1>: <T1> <f2>: <T2> }\n' (recorded instance: expected 'type Blog\n\n{\nid: ID!\n\n\ntitle: String\n\n}\n', got 'type Blog { id: ID! title: String }\n').
Why: a client that diffs, hashes or line-parses the SDL returned by ListTypes - schema-drift checks in CI, SDL round-trips through the `format` parameter, codegen that splits the definition into lines - sees a document AWS would never emit, so a comparison against a real account fails although nothing changed.
Fix: the Type response projection behind the ListTypes handler - render Type$definition from the parsed type model for the requested format instead of returning the stored request text.
Traces: appsync-graphql-api-and-type-lifecycle#task#4 (same cluster: #task#5).

Cause 2 of 3 - the same divergence for a CreateType-created type, additionally missing the terminating newline (fixture appsync-list-types-after-create-type, step task#5).
AWS: as Cause 1 - the re-rendered, newline-terminated multi-line document.
Emulator: returns the submitted one-line SDL with no trailing newline - expected 'type <Name>\n\n{\n<f1>: <T1>\n\n\n<f2>: <T2>\n\n}\n', got 'type <Name> { <f1>: <T1> <f2>: <T2> }' (recorded instance: expected 'type Widget\n\n{\nid: ID!\n\n\nname: String\n\n}\n', got 'type Widget { id: ID! name: String }'). Distinct cluster and replay key from Cause 1, so Cause 1's step does not exercise it.
Why: same as Cause 1, and because the document is not newline-terminated even a client that normalises internal whitespace still sees a different byte string.
Fix: same projection; the renderer must terminate every emitted type document with a newline.
Traces: appsync-list-types-after-create-type#task#5 (same cluster: #task#6).

Cause 3 of 3 - for a schema-derived type ListTypes emits the emulator's own indented rendering (fixture appsync-list-types-lifecycle, step task#3).
AWS: as Cause 1; for a single-field type the rendered document is 'type <Name>\n\n{\n<field>: <Type>\n\n}\n'.
Emulator: emits two-space-indented output of its own printer - expected 'type Query\n\n{\ngetItem: String\n\n}\n', got 'type Query {\n  getItem: String\n}' ('Query'/'getItem' are schema values). This shows the fault is not only "echo the request": where the emulator does render, it uses its own layout and still omits the terminating newline.
Why: the definition AWS returns is a stable, service-defined rendering, so no ListTypes response - echoed (Causes 1-2) or self-rendered (this one) - is byte-comparable with AWS.
Fix: one shared SDL renderer behind the Type projection, used for types derived from a StartSchemaCreation schema as well as for types created through CreateType.
Traces: appsync-list-types-lifecycle#task#3.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up=true, /__admin/health 200, data listener accepting). 733 steps: 522 passed, 201 mismatched, 10 blocked, score 71.2%, 64 divergence clusters, all service=appsync; 56 owned by open issues, 0 stub-blocked, 8 net-new - the 3 below plus the 5 CreateFunction/CreateResolver validation clusters filed separately.

1) appsync-graphql-api-and-type-lifecycle#task#4 - ListTypes, expected 200, actual 200, owned_by []. divergence_key service=appsync|ListTypes|status=200->200|keys=$.types[arn="arn:aws:appsync:us-west-2:000000000000:apis/fdh4iesrhyi2lg7pbfkvrxqdbg/types/Blog"].definition. detail: value_mismatch (expected 'type Blog\n\n{\nid: ID!\n\n\ntitle: String\n\n}\n', got 'type Blog { id: ID! title: String }\n'). affected_count 2: also appsync-graphql-api-and-type-lifecycle#task#5. The api id inside the ARN is a per-run value.

2) appsync-list-types-after-create-type#task#5 - ListTypes, 200 vs 200, owned_by []. detail: $.types[arn="...apis/ansu3y45vh4khfa4v7cklclv54/types/Widget"].definition: value_mismatch (expected 'type Widget\n\n{\nid: ID!\n\n\nname: String\n\n}\n', got 'type Widget { id: ID! name: String }'). affected_count 2: also appsync-list-types-after-create-type#task#6.

3) appsync-list-types-lifecycle#task#3 - ListTypes, 200 vs 200, owned_by []. detail: $.types[arn="...apis/mj2zgkngsvirtiocecygadzl4n/types/Query"].definition: value_mismatch (expected 'type Query\n\n{\ngetItem: String\n\n}\n', got 'type Query {\n  getItem: String\n}'). affected_count 1.

Model evidence (services/service-com-amazonaws-appsync-awsdeepdishcontrolplaneservice/smithy-model/com-amazonaws-appsync-awsdeepdishcontrolplaneservice.json): ListTypes GET /v1/apis/{apiId}/types, required `format` query (TypeDefinitionFormat SDL|JSON), maxResults 0..25, paginated on types/nextToken; errors BadRequestException(400)/NotFoundException(404)/ConcurrentModificationException(409)/UnauthorizedException(401)/InternalFailureException(500). Type = {name: ResourceName, description, arn, definition: String "The type definition", format: TypeDefinitionFormat} - no formatting or serialization trait on definition, so the layout above is grounded in recorded behavior.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Cause 1: replay appsync-graphql-api-and-type-lifecycle step task#4 (cluster also covers #task#5). CreateGraphqlApi, CreateType with a single-line SDL object type with two fields, then ListTypes?format=SDL. Expect each types[].definition re-rendered as 'type <Name>\n\n{\n<f1>: <T1>\n\n\n<f2>: <T2>\n\n}\n'; the emulator returns the submitted one-line string.
3. Cause 2: replay appsync-list-types-after-create-type step task#5 (cluster also covers #task#6). Same shape; the emulator additionally drops the terminating newline.
4. Cause 3: replay appsync-list-types-lifecycle step task#3. Create the type through StartSchemaCreation, then ListTypes?format=SDL. Expect AWS's layout; the emulator returns its own two-space-indented rendering.
5. All three steps must pass before this issue is closed.
