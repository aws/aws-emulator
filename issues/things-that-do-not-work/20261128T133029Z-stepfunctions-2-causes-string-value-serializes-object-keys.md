Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: stepfunctions.jsonata-string-with-a-string-prettify-fails-signature-rejected stepfunctions.jsonata-task-error-two-levels-deep-propagates-through-map-and-parallel
Issue-Shape-Id: com.amazonaws.sfn#AWSStepFunctions
Issue-Key: stepfunctions.jsonata-string-with-a-string-prettify-fails-signature-rejected|run#6|AssertionError|assert out.get("compact") == json.dumps(_DOC, separators=(",", ":")), (
Issue-Key: stepfunctions.jsonata-task-error-two-levels-deep-propagates-through-map-and-parallel|run#18|AssertionError|assert desc.get("cause") == expected_cause, (
Issue-Commit: ddb75d9687abbf130f37805dd22b3a9d4e702d16

# stepfunctions: 2 causes — $string(value) serializes object keys in sorted order, and a JSONata evaluation failure's cause names the wrong entered-event id for a state nested in Map/Parallel

> _Note: this issue was identified against the code base at commit `ddb75d9687abbf130f37805dd22b3a9d4e702d16`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartExecution

## Description
Two net-new root causes in the Step Functions JSONata/execution-engine surface. Neither is described by an existing issue; see the per-cause notes for the open issues that are adjacent but do not cover them.

Cause 1 of 2: the single-argument JSONata $string() emits a JSON object's members in sorted key order instead of the order the document carries.
AWS: recorded behavior only — the Smithy model describes DescribeExecution.output (SensitiveData, max 262144 bytes) but says nothing about JSONata evaluation; the reference the check names is https://docs.aws.amazon.com/step-functions/latest/dg/transforming-data.html. The recording shows that in a JSONata state machine a Pass state whose Output sets both "pretty": "{% $string($states.input.doc, true) %}" and "compact": "{% $string($states.input.doc) %}" SUCCEEDs, and both forms serialize the object's members in the order of the source document. For a document whose members are declared in the order sku, qty, tags, the compact form is the document's own member order with no separator whitespace, i.e. of the shape {"sku":...,"qty":...,"tags":[...]}, and the prettified form is the same member order indented two spaces. Member order in the emitted text is the requirement; the particular member names and values come from the request's input document.
Emulator: the two-argument prettified form now preserves the document's member order (that part of the recording passes), but the one-argument compact form re-orders the members alphabetically: for the same document it returned '{"qty":3,"sku":"A-17","tags":["x","y"]}' where the document's order requires sku first, then qty, then tags. The execution SUCCEEDs, so there is no status or error code to compare — only the serialized text diverges.
Why: $string() is how a workflow turns structured data into a payload for a downstream call (an SQS body, an SNS message, an HTTP body, a hash/signature input). A client that compares, signs, hashes or diffs that text gets a different string than AWS produces for the same input, and any signature or checksum computed over it will not verify. It also makes the two $string forms mutually inconsistent inside one emulator.
Fix: the JSON serializer behind $string in the JSONata function library of the Step Functions execution engine — the single-argument path must use the same order-preserving object representation the prettify path already uses, rather than a sorted map.
Adjacent open issue (does NOT cover this): 20261127T221043Z-stepfunctions-10-causes-in-jsonata-evaluation-semantics-and.md, Cause 7, asserts the opposite — that AWS refuses the two-argument $string(value, true) prettify form. The recording shows the boolean prettify form succeeding and pretty-printing; that issue's replay key for this check points at the prettify assertion, which now passes, and it never mentions member ordering of the compact form.
Traces: stepfunctions.jsonata-string-with-a-string-prettify-fails-signature-rejected.

Cause 2 of 2: the "(entered at the event id #N)" preamble of a States.QueryEvaluationError cause names an event id other than the failing state's own StateEntered event when that state is nested inside a Map inside a Parallel branch.
AWS: recorded behavior only for the cause wording — DescribeExecution.cause is modeled as SensitiveCause (max 32768 bytes) and the model says nothing about its text. The recording shows that when a JSONata expression in a Task's Arguments field fails, the execution ends FAILED with error 'States.QueryEvaluationError' and a cause of the fixed shape "An error occurred while executing the state '<state name>' (entered at the event id #<id>). The JSONata expression '<expression>' specified for the field '<field>' threw an error during evaluation. <JSONata code>: <JSONata message>". The fixed text is that sentence frame; the variable parts are the failing state's name, the id of that state's OWN StateEntered event in this execution's history, the expression and field from the definition, and the JSONata engine's code and message. The id is required to equal the id of the StateEntered event whose stateEnteredEventDetails.name is the failing state — that is the invariant, not any particular number.
Emulator: the cause carries an id one lower than the failing state's own StateEntered event. In the recording BadTask's StateEntered event has id #10, while the cause read "An error occurred while executing the state 'BadTask' (entered at the event id #9). The JSONata expression '$number('not-a-number')' specified for the field 'Arguments' threw an error during evaluation. D3030: Unable to cast value to a number: \"not-a-number\"" — every other part of the sentence matched; only the event id pointed at the wrong event. The error code 'States.QueryEvaluationError' was correct.
Why: the event id in that sentence is the only pointer from a failure cause back into GetExecutionHistory. An operator or a client that looks up event #N to find the failing state's input lands on a different event (here the Wait state's exit rather than the Task's entry), so automated triage and history correlation resolve the wrong state — and the mismatch appears exactly in the nested Map-inside-Parallel case where correlation matters most.
Fix: the cause composition for query-evaluation failures in the Step Functions execution engine — it must carry the id the history recorder actually assigned to the failing state's StateEntered event for the current container scope, not a counter captured before the nested state's entry event was emitted.
Traces: stepfunctions.jsonata-task-error-two-levels-deep-propagates-through-map-and-parallel.

## Evidence
Cause 1 — stepfunctions.jsonata-string-with-a-string-prettify-fails-signature-rejected: the prettified form matched the document's member order with two-space indent (that assertion passed); the compact form came back as
  actual:   '{"qty":3,"sku":"A-17","tags":["x","y"]}'
  expected: the same members in the input document's order, i.e. sku, then qty, then tags, with no separator whitespace.
AssertionError: compact form '{"qty":3,"sku":"A-17","tags":["x","y"]}'. Execution status SUCCEEDED in both cases, so only the serialized text diverges.

Cause 2 — stepfunctions.jsonata-task-error-two-levels-deep-propagates-through-map-and-parallel: the execution ended FAILED with error 'States.QueryEvaluationError' (correct) and cause
  actual:   "An error occurred while executing the state 'BadTask' (entered at the event id #9). The JSONata expression '$number('not-a-number')' specified for the field 'Arguments' threw an error during evaluation. D3030: Unable to cast value to a number: \"not-a-number\""
  expected: the identical sentence with the id of BadTask's own StateEntered event, which in this run was #10.
AssertionError: the cause must name BadTask's own entered-event id (#10). Fixed text: "An error occurred while executing the state '<name>' (entered at the event id #<id>). The JSONata expression '<expr>' specified for the field '<field>' threw an error during evaluation. " plus the JSONata code/message; variable parts: state name, StateEntered event id from this execution's history, expression and field from the definition, and the D3030 message's value taken from the request's data. The ids #9/#10 are this run's values, not part of the requirement.

## Steps to Reproduce
Cause 1:
1. CreateStateMachine with "QueryLanguage": "JSONata" and a Pass state whose Output is {"pretty": "{% $string($states.input.doc, true) %}", "compact": "{% $string($states.input.doc) %}"}.
2. StartExecution with input {"mode": "flag", "doc": {"sku": "A-17", "qty": 3, "tags": ["x", "y"]}} (any object whose member order is not alphabetical).
3. DescribeExecution: status is SUCCEEDED. The "pretty" value preserves the document's member order; the "compact" value has them sorted alphabetically instead.

Cause 2:
1. CreateStateMachine with "QueryLanguage": "JSONata" and a Parallel state whose first branch is an INLINE Map over one item, whose ItemProcessor is a Wait state followed by a Task named 'BadTask' with "Arguments": "{% $number('not-a-number') %}"; the second branch is a Task on an activity ARN.
2. StartExecution, then GetActivityTask on the activity so the sibling branch's task is genuinely in flight.
3. Wait for the execution to reach FAILED, then GetExecutionHistory and read the id of the StateEntered event whose name is 'BadTask'.
4. Compare DescribeExecution.cause: its "(entered at the event id #N)" names a lower id than that StateEntered event's id.

Or replay the check ids in trace_ids.
