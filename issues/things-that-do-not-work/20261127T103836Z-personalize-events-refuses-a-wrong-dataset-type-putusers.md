Issue-Source: trace-sweep-divergence
Issue-Trace: personalize-events-dataset-type-mismatch
Issue-Step: task#9
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: personalize-events-dataset-type-mismatch
Issue-Shape-Id: com.amazonaws.personalizeevents#AmazonPersonalizeEvents
Issue-Key: personalize-events|PutUsers|4xx->4xx|$.message
Issue-Key: personalize-events|PutItems|4xx->4xx|$.message
Issue-Commit: 7bfd99e3999e8b9fa7ee2b4f0c70a66ae974be21

# personalize-events refuses a wrong-dataset-type PutUsers/PutItems with its own invented message instead of AWS's "Invalid datasetArn"

> _Note: this issue was identified against the code base at commit `7bfd99e3999e8b9fa7ee2b4f0c70a66ae974be21`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutUsers

## Description
When the `datasetArn` supplied to a typed PUT-ingestion operation names a dataset whose datasetType is not the one that operation writes to, the emulator refuses the call with the right HTTP status (400) and error class but with a message it composes itself, naming the ARN and both dataset types. Real Personalize Events answers the generic field-level message `Invalid datasetArn` for this class of refusal. The emulator's message is the SAME invented template in both affected operations, so this is one serialization/validation fault with two call sites. 2 causes, both net-new (the open personalize-events issues 20261124T193003Z-... and 20261125T165618Z-... concern dataset-export CSV column naming, and 20261127T095255Z-personalize-events-putusers-does-not-validate-user-properties-against-the.md concerns PutUsers *schema* validation of `properties` — none of them covers the wrong-dataset-type refusal message).

Cause 1 of 2: PutUsers against a non-Users dataset ARN (fixture personalize-events-dataset-type-mismatch, step task#9).
AWS: Smithy: PutUsers POST /users, success 200, `datasetArn` required, target Arn (@length 0..256, @pattern ^arn:([a-z\d-]+):personalize:.*:.*:.+$), documented as "The Amazon Resource Name (ARN) of the Users dataset you are adding the user or users to"; modeled errors InvalidInputException (errorKind client, httpError 400), ResourceInUseException (409), ResourceNotFoundException (404). InvalidInputException's own doc is "Provide a valid value for the field or parameter." The recorded response for an ARN that resolves to a dataset of another type is HTTP 400 with message `Invalid datasetArn` — recorded behavior only for the exact text. That message is entirely FIXED text: it is the name of the offending input member (`datasetArn`) prefixed with `Invalid `, and it carries NO value from the request or from emulator state — no ARN, no dataset type.
Emulator: answers HTTP 400 (status correct) but with `$.message` = `arn:aws:personalize:<region>:<account>:dataset/<datasetGroup>/ITEMS is not a valid USERS dataset.` — i.e. a sentence built from the request's ARN plus the resolved and expected dataset types. Replay detail: `$.message: value_mismatch (expected 'Invalid datasetArn', got 'arn:aws:personalize:us-west-2:000000000000:dataset/sim2real-probe-pevt-tm-dsg-9c51f5b5/ITEMS is not a valid USERS dataset.')`.

Cause 2 of 2: PutItems against a non-Items dataset ARN (fixture personalize-events-dataset-type-mismatch, step task#10).
AWS: Smithy: PutItems POST /items, success 200, `datasetArn` required with the same Arn constraints, same modeled errors (InvalidInputException client/400, ResourceInUseException 409, ResourceNotFoundException 404). Recorded behavior: HTTP 400 with the same all-fixed message `Invalid datasetArn`.
Emulator: HTTP 400 with `$.message` = `arn:aws:personalize:<region>:<account>:dataset/<datasetGroup>/USERS is not a valid ITEMS dataset.` Replay detail: `$.message: value_mismatch (expected 'Invalid datasetArn', got 'arn:aws:personalize:us-west-2:000000000000:dataset/sim2real-probe-pevt-tm-dsg-9c51f5b5/USERS is not a valid ITEMS dataset.')`. The region, account id and dataset-group name in the quoted strings are per-environment values and are not part of the requirement; the divergence is the template itself.

Why: the error message is part of the API contract for this error class. A client that matches on the message to distinguish "you pointed at the wrong kind of dataset" from other InvalidInputException causes (log assertions, user-facing error mapping, retry/branch logic, test fixtures recorded against AWS) sees text AWS never emits, so the branch never fires and the condition is reported as an unknown input error. Worse, the emulator's message echoes the full resource ARN back to the caller for an input-validation refusal, which AWS deliberately does not do here — code or tooling that redacts/parses AWS messages gets a shape it was not written for.

Fix: the dataset-type check shared by the PUT-ingestion handlers in the personalize-events service crate (the guard that resolves `datasetArn` to a dataset and compares its datasetType against the one the operation ingests), and the InvalidInputException message it constructs. It must raise InvalidInputException/400 with the fixed message `Invalid datasetArn` — the member name only, with no interpolation of the ARN or of either dataset type — for PutUsers, PutItems and any other operation sharing the same guard. Status 400 and the error class are already correct and must stay.

Traces: personalize-events-dataset-type-mismatch#task#9 (Cause 1, PutUsers), personalize-events-dataset-type-mismatch#task#10 (Cause 2, PutItems).

## Evidence
Replay of trace set simparity-wire-aws-v1 (6 fixtures, 49 steps, 47 passed, 2 mismatched, score 95.9%).

Cluster 1 — service personalize-events, operation PutUsers, verdict mismatch, fixture personalize-events-dataset-type-mismatch, step task#9, affected: [personalize-events-dataset-type-mismatch#task#9]:
  expected_status 400, actual_status 400
  detail: $.message: value_mismatch (expected 'Invalid datasetArn', got 'arn:aws:personalize:us-west-2:000000000000:dataset/sim2real-probe-pevt-tm-dsg-9c51f5b5/ITEMS is not a valid USERS dataset.')
  divergence_key: service=personalize-events|PutUsers|status=400->400|keys=$.message
  stub_path: (none), owned_by: (none)

Cluster 2 — service personalize-events, operation PutItems, verdict mismatch, fixture personalize-events-dataset-type-mismatch, step task#10, affected: [personalize-events-dataset-type-mismatch#task#10]:
  expected_status 400, actual_status 400
  detail: $.message: value_mismatch (expected 'Invalid datasetArn', got 'arn:aws:personalize:us-west-2:000000000000:dataset/sim2real-probe-pevt-tm-dsg-9c51f5b5/USERS is not a valid ITEMS dataset.')
  divergence_key: service=personalize-events|PutItems|status=400->400|keys=$.message
  stub_path: (none), owned_by: (none)

Expected (AWS, per recording): HTTP 400, InvalidInputException, message exactly `Invalid datasetArn` (all fixed text, no request/state values).
Actual (emulator): HTTP 400, message `<datasetArn> is not a valid <EXPECTED_TYPE> dataset.` — the ARN from the request plus the resolved and expected dataset types.

Model evidence (Smithy, com-amazonaws-personalizeevents-amazonpersonalizeevents.json):
  PutUsers: POST /users, code 200; input datasetArn (required, Arn, length 0..256, pattern ^arn:([a-z\d-]+):personalize:.*:.*:.+$), users (required, 1..10); errors InvalidInputException (client, 400), ResourceInUseException (409), ResourceNotFoundException (404). datasetArn doc: "The Amazon Resource Name (ARN) of the Users dataset you are adding the user or users to."
  PutItems: POST /items, code 200; input datasetArn (same constraints), items (required, 1..10); same three errors.
  InvalidInputException: structure, errorKind client, httpError 400, member message (ErrorMessage); doc "Provide a valid value for the field or parameter."

## Steps to Reproduce
Cause 1 — PutUsers with an Items dataset ARN (fixture personalize-events-dataset-type-mismatch, step task#9):
1. personalize.CreateSchema for a Users record and for an Items record.
2. personalize.CreateDatasetGroup; poll DescribeDatasetGroup to ACTIVE.
3. personalize.CreateDataset(datasetType="Users", schemaArn=<users schema>) and personalize.CreateDataset(datasetType="Items", schemaArn=<items schema>) in that dataset group; poll both to ACTIVE.
4. personalize-events.PutUsers(datasetArn=<the ITEMS dataset ARN>, users=[{userId:"u-1", properties:'{...}'}]).
   Expect: HTTP 400, InvalidInputException, Message exactly "Invalid datasetArn".
   Observed: HTTP 400 with Message "<items dataset arn> is not a valid USERS dataset."

Cause 2 — PutItems with a Users dataset ARN (same fixture, step task#10):
5. personalize-events.PutItems(datasetArn=<the USERS dataset ARN>, items=[{itemId:"i-1", properties:'{...}'}]).
   Expect: HTTP 400, InvalidInputException, Message exactly "Invalid datasetArn".
   Observed: HTTP 400 with Message "<users dataset arn> is not a valid ITEMS dataset."

Regression guard: PutUsers against the Users dataset and PutItems against the Items dataset must still answer HTTP 200 with an empty body (both pass today).

Replay: replay_wire_corpus and confirm personalize-events-dataset-type-mismatch steps task#9 and task#10 both pass.
