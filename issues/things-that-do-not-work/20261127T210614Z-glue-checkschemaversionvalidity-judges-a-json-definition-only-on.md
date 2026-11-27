Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: glue.check-schema-version-validity-json-schema-errors-exact
Issue-Shape-Id: com.amazonaws.glue#AWSGlue
Issue-Key: glue.check-schema-version-validity-json-schema-errors-exact|_assert_result#8|AssertionError|assert resp["Valid"] is valid, f"{label} must report Valid={valid}: {resp}"
Issue-Commit: 152ee7a3ceaeab79fbebf740702a826bb86176c3

# glue CheckSchemaVersionValidity judges a JSON definition only on JSON well-formedness, so a document that breaks the JSON Schema meta-schema is reported Valid=true with no Error

> _Note: this issue was identified against the code base at commit `152ee7a3ceaeab79fbebf740702a826bb86176c3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CheckSchemaVersionValidity

## Description
Cause 1 of 1: for DataFormat=JSON, CheckSchemaVersionValidity only asks whether the SchemaDefinition is parseable JSON; it never asks whether the parsed document is a valid JSON Schema, so a schema whose `type` keyword carries a value the JSON Schema meta-schema does not admit is answered as valid.
  AWS:      Smithy (com.amazonaws.glue#AWSGlue): CheckSchemaVersionValidity input {DataFormat (enum AVRO|JSON|PROTOBUF, required), SchemaDefinition (SchemaDefinitionString, required, @length 1..170000, @pattern \S)}; output {Valid (IsVersionValid, required, default false), Error (SchemaValidationError, @length 1..5000)}; modeled errors AccessDeniedException/InvalidInputException (400), InternalServiceException (500). The model documents Error only as "A validation failure error message" and Valid as "Return true, if the schema is valid and false otherwise", so the wording is recorded behavior only. The recording (HTTP 200 in every case — this operation carries the verdict in the body, never in an error) shows AWS validating a JSON definition against the JSON Schema meta-schema and not merely parsing it: a draft-07 document whose `properties.id.type` is the bare word `notarealtype` is answered Valid=false with Error being the validator's finding list, bracketed and comma-separated, each entry `<JSON path>: <finding>` — `[$.properties.id.type: does not have a value in the enumeration [array, boolean, integer, null, number, object, string], $.properties.id.type: should be valid to any of the schemas array]`. Fixed text vs values: `does not have a value in the enumeration [...]` (with the draft-07 simpleTypes list) and `should be valid to any of the schemas array` are the validator's fixed findings; the `$.<path>` prefixes are derived from the position of the offending keyword in the SUBMITTED definition, so a definition that puts the bad `type` elsewhere reports that path instead. The same recording's truncated-definition case still answers Valid=false with the Jackson parse reading (`Unexpected end-of-input within/between Object entries\n at [Source: REDACTED (`StreamReadFeature.INCLUDE_SOURCE_IN_LOCATION` disabled); line: 1, column: 19]`), and the well-formed draft-07 control answers Valid=true with no Error — so parse faults and SCHEMA faults are two separate readings AWS reports on the same 200.
  Emulator: the parse fault is reported correctly (the truncated-definition assertion passes), but the schema fault is not reported at all: the unknown `type` keyword value is answered HTTP 200 with body {"Valid": true} and no Error member. Replay: "AssertionError: a schema with an unknown `type` keyword value must report Valid=False: {'Valid': True, ...}".
  Why:      a client uses this operation as the pre-flight gate before RegisterSchemaVersion/CreateSchema. Told Valid=true, it registers (or ships to producers/consumers) a document AWS considers no JSON Schema at all, and the failure surfaces later at registration or at serialization time instead of at the validation call; a tool that reports the Error text to a user shows nothing to fix. The Valid/Error pair is also the only channel this operation has, so losing the schema-level verdict silently inverts the answer rather than degrading it.
  Fix:      the JSON branch of the definition reading that the handler calls — `crate::state::schema_definition_fault`'s JSON path, whose only JSON check today is well-formedness (`unreadable_definition_json`, a `serde_json::from_str` plus a Jackson-shaped diagnostic). A meta-schema validation pass for DataFormat=JSON belongs beside it in state, producing the bracketed finding list as the Error, so a schema-registry WRITE of the same definition and this report share one reading. The handler itself (valid = fault.is_none(), Error = the reading) is already right.
  Traces:   check glue.check-schema-version-validity-json-schema-errors-exact. Handler: services/service-com-amazonaws-glue-awsglue/src/ops/check_schema_version_validity.rs lines 167-186; the JSON reading it dispatches to: services/service-com-amazonaws-glue-awsglue/src/state/schema_definition_json.rs lines 27-34 (doc comment lines 27-29 states outright that "a definition that IS JSON and describes no schema of its format" is not judged here).

Note on neighbouring issues: the resolved 20261120T202955Z-glue-2-net-new-causes-checkschemaversionvalidity-s-json.md covered this operation's TRUNCATED-definition Error text, which now matches — that cause is not re-raised here. This failure is the separate, never-filed cause that no JSON Schema validation is performed at all; it is a 200 with a wrong body, not a refusal.

## Evidence
Check glue.check-schema-version-validity-json-schema-errors-exact (verdict: mismatch), failing at check_glue_check_schema_version_validity_json_schema_errors_exact.py line 56 (_assert_result for "a schema with an unknown `type` keyword value"):
  AssertionError: a schema with an unknown `type` keyword value must report Valid=False: {'Valid': True, 'ResponseMetadata': {... 'HTTPStatusCode': 200 ...}}

Request: DataFormat="JSON", SchemaDefinition='{"$schema":"http://json-schema.org/draft-07/schema#","type":"object","properties":{"id":{"type":"notarealtype"}}}'

Expected (AWS, recorded): HTTP 200, Valid=false, Error="[$.properties.id.type: does not have a value in the enumeration [array, boolean, integer, null, number, object, string], $.properties.id.type: should be valid to any of the schemas array]"
Actual (emulator):        HTTP 200, Valid=true, Error absent (content-length 14, i.e. the body is {"Valid":true})

Passed before this assertion (so these are NOT in question): the well-formed draft-07 control -> Valid=true with no Error; the truncated definition '{"type": "object",' -> Valid=false with Error "Unexpected end-of-input within/between Object entries\n at [Source: REDACTED (`StreamReadFeature.INCLUDE_SOURCE_IN_LOCATION` disabled); line: 1, column: 19]".

Not reached because the check aborts here: the Error text for the unknown-type case. Re-replay the check after the meta-schema pass is added.

## Steps to Reproduce
1. Boot the emulator and point an AWS SDK/CLI at it.
2. Control: aws glue check-schema-version-validity --data-format JSON --schema-definition '{"$schema":"http://json-schema.org/draft-07/schema#","type":"object","properties":{"id":{"type":"integer"}},"required":["id"],"additionalProperties":false}' -> 200, Valid=true, no Error (correct).
3. aws glue check-schema-version-validity --data-format JSON --schema-definition '{"type": "object",' -> 200, Valid=false with the Jackson end-of-input reading (correct).
4. aws glue check-schema-version-validity --data-format JSON --schema-definition '{"$schema":"http://json-schema.org/draft-07/schema#","type":"object","properties":{"id":{"type":"notarealtype"}}}'
5. Observe 200 with Valid=true and no Error; AWS answers Valid=false with Error "[$.properties.id.type: does not have a value in the enumeration [array, boolean, integer, null, number, object, string], $.properties.id.type: should be valid to any of the schemas array]".
6. Or replay the suite check glue.check-schema-version-validity-json-schema-errors-exact.
