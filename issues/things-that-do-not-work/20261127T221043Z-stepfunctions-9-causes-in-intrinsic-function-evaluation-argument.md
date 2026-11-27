Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: stepfunctions.array-get-item-with-one-or-three-arguments-rejected-at-run-time stepfunctions.base64-encode-with-zero-or-two-arguments-rejected-at-run-time stepfunctions.intrinsic-array-contains-wrong-arity-fails-at-run-rejected stepfunctions.intrinsic-array-length-wrong-argument-count-fails-execution stepfunctions.intrinsic-array-partition-wrong-argument-count-rejected stepfunctions.intrinsic-array-range-wrong-argument-count-fails-execution stepfunctions.intrinsic-array-unique-wrong-argument-count-fails-execution stepfunctions.intrinsic-base64-decode-wrong-arity-fails-at-run-rejected stepfunctions.intrinsic-format-surplus-arguments-fail-execution-rejected stepfunctions.intrinsic-hash-wrong-argument-count-fails-execution stepfunctions.intrinsic-json-merge-wrong-argument-count-rejected stepfunctions.intrinsic-math-add-wrong-argument-count-rejected stepfunctions.intrinsic-math-random-wrong-argument-count-fails-execution stepfunctions.intrinsic-math-random-range-edges-one-value-seed-and-empty stepfunctions.intrinsic-string-to-json-wrong-argument-count-fails-execution-rejected stepfunctions.string-split-wrong-argument-count-rejected stepfunctions.intrinsic-format-non-string-template-fails-execution stepfunctions.intrinsic-json-to-string-wrong-argument-count-rejected stepfunctions.intrinsic-math-add-rounds-non-integer-arguments stepfunctions.intrinsic-math-add-sum-past-int32-max-fails stepfunctions.intrinsic-math-add-outside-int32-range-fails stepfunctions.intrinsic-array-range-truncates-non-integer-arguments stepfunctions.intrinsic-array-unique-empty-single-unicode-and-mixed-type-elements stepfunctions.intrinsic-hash-null-data-argument-fails-execution stepfunctions.intrinsic-string-to-json-parses-empty-scalar-and-unicode-documents stepfunctions.intrinsic-nesting-beyond-ten-functions-rejected stepfunctions.intrinsic-states-array-object-literal-argument-rejected
Issue-Shape-Id: com.amazonaws.sfn#AWSStepFunctions
Issue-Key: stepfunctions.array-get-item-with-one-or-three-arguments-rejected-at-run-time|run#16|AssertionError|assert bad["status"] == "FAILED", (
Issue-Key: stepfunctions.base64-encode-with-zero-or-two-arguments-rejected-at-run-time|run#16|AssertionError|assert bad["status"] == "FAILED", (
Issue-Key: stepfunctions.intrinsic-array-contains-wrong-arity-fails-at-run-rejected|run#13|AssertionError|assert bad["status"] == "FAILED", (
Issue-Key: stepfunctions.intrinsic-array-length-wrong-argument-count-fails-execution|run#7|AssertionError|assert desc["status"] == "FAILED", (
Issue-Key: stepfunctions.intrinsic-array-partition-wrong-argument-count-rejected|run#7|AssertionError|assert [n for _, n in entered] == [state] and exited == [], (
Issue-Key: stepfunctions.intrinsic-array-range-wrong-argument-count-fails-execution|_assert_refused#14|AssertionError|assert desc["status"] == "FAILED", (
Issue-Key: stepfunctions.intrinsic-array-unique-wrong-argument-count-fails-execution|_assert_refused#15|AssertionError|assert desc["status"] == "FAILED", (
Issue-Key: stepfunctions.intrinsic-base64-decode-wrong-arity-fails-at-run-rejected|run#14|AssertionError|assert bad["status"] == "FAILED", (
Issue-Key: stepfunctions.intrinsic-format-surplus-arguments-fail-execution-rejected|run#12|AssertionError|assert failed["status"] == "FAILED", (
Issue-Key: stepfunctions.intrinsic-hash-wrong-argument-count-fails-execution|run#9|AssertionError|assert bad["status"] == "FAILED", (
Issue-Key: stepfunctions.intrinsic-json-merge-wrong-argument-count-rejected|run#20|AssertionError|assert bad["status"] == "FAILED", (
Issue-Key: stepfunctions.intrinsic-math-add-wrong-argument-count-rejected|run#16|AssertionError|assert desc["status"] == "FAILED", (
Issue-Key: stepfunctions.intrinsic-math-random-wrong-argument-count-fails-execution|run#4|AssertionError|assert bad["status"] == "FAILED", (
Issue-Key: stepfunctions.intrinsic-math-random-range-edges-one-value-seed-and-empty|run#8|AssertionError|assert bad["status"] == "FAILED", (
Issue-Key: stepfunctions.intrinsic-string-to-json-wrong-argument-count-fails-execution-rejected|run#13|AssertionError|assert failed["status"] == "FAILED", (
Issue-Key: stepfunctions.string-split-wrong-argument-count-rejected|run#11|AssertionError|assert got == want, f"{arity} argument(s): expected {want!r}, got {got!r}"
Issue-Key: stepfunctions.intrinsic-format-non-string-template-fails-execution|run#12|AssertionError|assert got == want, f"error and cause {got!r}, expected {want!r}"
Issue-Key: stepfunctions.intrinsic-json-to-string-wrong-argument-count-rejected|_assert_refused#20|AssertionError|assert cause.endswith(tail), f"cause for {expr} should end with {tail!r}, got {cause!r}"
Issue-Key: stepfunctions.intrinsic-math-add-rounds-non-integer-arguments|run#7|AssertionError|assert out.get(f"r{i}") == want, f"{expr} must yield {want!r}, got {out.get(f'r{i}')!r}"
Issue-Key: stepfunctions.intrinsic-math-add-sum-past-int32-max-fails|run#5|AssertionError|assert over["status"] == "FAILED", (
Issue-Key: stepfunctions.intrinsic-math-add-outside-int32-range-fails|run#7|AssertionError|assert bad["status"] == "FAILED", f"an out-of-range argument must fail, got {bad['status']!r} {bad.get('output')!r}"
Issue-Key: stepfunctions.intrinsic-array-range-truncates-non-integer-arguments|run#4|AssertionError|assert json.loads(resp["output"]) == {"r": [1, 2, 3, 4]}, f"expected [1..4], got {resp['output']!r}"
Issue-Key: stepfunctions.intrinsic-array-unique-empty-single-unicode-and-mixed-type-elements|run#4|AssertionError|assert _canon(out["items"]) == _canon(want), (
Issue-Key: stepfunctions.intrinsic-hash-null-data-argument-fails-execution|run#5|AssertionError|assert ok["status"] == "SUCCEEDED", (
Issue-Key: stepfunctions.intrinsic-string-to-json-parses-empty-scalar-and-unicode-documents|run#12|AssertionError|assert done["status"] == "SUCCEEDED", (
Issue-Key: stepfunctions.intrinsic-nesting-beyond-ten-functions-rejected|run#13|AssertionError|raise AssertionError(f"an 11-deep intrinsic chain was accepted as {made!r}")
Issue-Key: stepfunctions.intrinsic-states-array-object-literal-argument-rejected|_refusal#15|AssertionError|raise AssertionError(f"expected the call to be refused, but it returned {result!r}")
Issue-Commit: 3c63a583c399d299d4ea298f38dfae7b708152ae

# stepfunctions: 9 causes in intrinsic-function evaluation — argument counts are never checked, and MathAdd/ArrayRange/ArrayUnique/Hash/StringToJson semantics and nesting limits diverge

> _Note: this issue was identified against the code base at commit `3c63a583c399d299d4ea298f38dfae7b708152ae`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartExecution

## Description
Cause 1 of 9: intrinsic functions accept the wrong number of arguments and succeed instead of failing the execution.
AWS: each intrinsic has a fixed arity. Calling one with too few or too many arguments fails the execution with error 'States.Runtime' and cause "An error occurred while executing the state '<state>' (entered at the event id #<n>). There was an error while evaluating the intrinsic function: <the call as written>. Invalid arguments in <States.Fn>" — the fixed text is AWS's, the state name, event id and call text come from the definition and the run. This applies to States.ArrayGetItem (2), States.ArrayContains (2), States.ArrayLength (1), States.ArrayPartition (2), States.ArrayRange (3), States.ArrayUnique (1), States.Base64Encode (1), States.Base64Decode (1), States.Hash (2), States.JsonMerge (3), States.StringToJson (1), States.StringSplit (2), States.MathAdd (2), States.MathRandom (2 or 3), and States.Format (template plus exactly as many arguments as '{}' placeholders).
Emulator: every one of those calls is evaluated and the execution SUCCEEDs — e.g. States.ArrayGetItem($.items, $.at, 1) returns {"picked":42}, States.Base64Encode($.text, $.text) returns an encoding, States.Hash($.text,'SHA-256','extra') returns a digest, States.MathRandom(10,11,$.seed,5) returns 10, States.MathRandom(5,5) returns 5, States.StringSplit($.line,'|','|') succeeds, States.StringToJson($.first,$.second) returns a document.
Why: a definition AWS fails at run time runs to success here, so a client's error path is never exercised and surplus/missing arguments are silently ignored.
Fix: argument-count validation in the intrinsic-function evaluator of the Step Functions execution engine.
Traces: stepfunctions.array-get-item-with-one-or-three-arguments-rejected-at-run-time, stepfunctions.base64-encode-with-zero-or-two-arguments-rejected-at-run-time, stepfunctions.intrinsic-array-contains-wrong-arity-fails-at-run-rejected, stepfunctions.intrinsic-array-length-wrong-argument-count-fails-execution, stepfunctions.intrinsic-array-partition-wrong-argument-count-rejected, stepfunctions.intrinsic-array-range-wrong-argument-count-fails-execution, stepfunctions.intrinsic-array-unique-wrong-argument-count-fails-execution, stepfunctions.intrinsic-base64-decode-wrong-arity-fails-at-run-rejected, stepfunctions.intrinsic-format-surplus-arguments-fail-execution-rejected, stepfunctions.intrinsic-hash-wrong-argument-count-fails-execution, stepfunctions.intrinsic-json-merge-wrong-argument-count-rejected, stepfunctions.intrinsic-math-add-wrong-argument-count-rejected, stepfunctions.intrinsic-math-random-wrong-argument-count-fails-execution, stepfunctions.intrinsic-math-random-range-edges-one-value-seed-and-empty, stepfunctions.intrinsic-string-to-json-wrong-argument-count-fails-execution-rejected, stepfunctions.string-split-wrong-argument-count-rejected.

Cause 2 of 9: when an intrinsic failure IS produced, the cause stops short of AWS's trailing explanation.
AWS: the cause ends with the reason, not just 'Invalid arguments in <fn>' — e.g. "... Invalid arguments in States.Format: number of arguments do not match the occurrences of {}." and "... Invalid arguments in States.JsonToString: only a single argument is allowed." The sentence after the colon is fixed text for that reason class.
Emulator: the cause ends at "Invalid arguments in States.Format" / "Invalid arguments in States.JsonToString".
Why: a client that reads or matches the cause to tell which argument rule it broke loses that information.
Fix: the intrinsic-function error messages in the execution engine's intrinsic evaluator.
Traces: stepfunctions.intrinsic-format-non-string-template-fails-execution, stepfunctions.intrinsic-json-to-string-wrong-argument-count-rejected.

Cause 3 of 9: States.MathAdd does not round its arguments and does not enforce the 32-bit signed range.
AWS: MathAdd rounds non-integer arguments to integers before adding (MathAdd(2.6, 1) is 4), and fails the execution with States.Runtime when an argument, or the sum, falls outside the 32-bit signed integer range (MathAdd(2147483647, 1), and an argument past 2147483648).
Emulator: MathAdd($.v, 1) returns 3.6; MathAdd(2147483647, 1) SUCCEEDs with 2147483648; an out-of-range argument SUCCEEDs with 2147483649.
Why: arithmetic results differ silently and an overflow AWS refuses is carried forward into the workflow's data.
Fix: States.MathAdd in the intrinsic evaluator (rounding plus int32 range checks).
Traces: stepfunctions.intrinsic-math-add-rounds-non-integer-arguments, stepfunctions.intrinsic-math-add-sum-past-int32-max-fails, stepfunctions.intrinsic-math-add-outside-int32-range-fails.

Cause 4 of 9: States.ArrayRange does not truncate non-integer arguments.
AWS: ArrayRange truncates its start/end/step arguments to integers, so a fractional start yields integer elements (expected [1,2,3,4]).
Emulator: the result keeps the fraction — {"r":[1.2,2.2,3.2,4.2]}.
Why: downstream states receive floats where AWS produces integers, which changes comparisons, Map item types and serialized output.
Fix: States.ArrayRange in the intrinsic evaluator.
Traces: stepfunctions.intrinsic-array-range-truncates-non-integer-arguments.

Cause 5 of 9: States.ArrayUnique does not compare elements by deep equality.
AWS: ArrayUnique removes duplicates by structural (deep) equality regardless of member order inside objects, so [{'a':1,'b':[2]}, {'b':[2],'a':1}, [1,2], [2,1], [1,2]] reduces to [{'a':1,'b':[2]}, [1,2], [2,1]].
Emulator: the two equal objects with different member order both survive — [{'a':1,'b':[2]}, {'b':[2],'a':1}, [1,2], [2,1]].
Why: a de-duplication step leaves duplicates, so item counts and Map fan-out differ from AWS.
Fix: the element-equality used by States.ArrayUnique in the intrinsic evaluator.
Traces: stepfunctions.intrinsic-array-unique-empty-single-unicode-and-mixed-type-elements.

Cause 6 of 9: States.Hash refuses non-string JSON data.
AWS: States.Hash accepts any JSON value as its data argument (a number, null) and hashes its serialized form, so hashing 42 succeeds.
Emulator: the execution fails with States.Runtime, cause "... There was an error while evaluating the intrinsic function: States.Hash($.payload, 'SHA-256'). Invalid arguments in States.Hash".
Why: a workflow that fingerprints arbitrary JSON values fails here.
Fix: argument coercion for States.Hash in the intrinsic evaluator.
Traces: stepfunctions.intrinsic-hash-null-data-argument-fails-execution.

Cause 7 of 9: States.StringToJson refuses empty/scalar documents it must parse.
AWS: StringToJson parses any JSON document text, including bare scalars and unicode documents, and an absent/empty input yields the documented result rather than a parse failure.
Emulator: the execution fails with States.Runtime, cause "... States.StringToJson($.text.no_text). Cannot convert String to Json in States.StringToJson, caused by: Unexpected end-of-input: expected a value\n at [Source: REDACTED ...]" — a Jackson parser message, not AWS's.
Why: valid edge-case documents fail, and the leaked parser diagnostic is not an AWS cause.
Fix: States.StringToJson in the intrinsic evaluator.
Traces: stepfunctions.intrinsic-string-to-json-parses-empty-scalar-and-unicode-documents.

Cause 8 of 9: an intrinsic chain nested more than ten deep is accepted at create.
AWS: intrinsic functions may be nested at most ten deep; CreateStateMachine refuses an 11-deep chain with InvalidDefinition and a SCHEMA_VALIDATION_FAILED message naming the field's path.
Emulator: CreateStateMachine returns a stateMachineArn.
Why: the definition deploys here and is rejected on AWS.
Fix: definition validation in the CreateStateMachine handler (intrinsic nesting depth).
Traces: stepfunctions.intrinsic-nesting-beyond-ten-functions-rejected.

Cause 9 of 9: States.Array with an object-literal argument is accepted at create.
AWS: an intrinsic argument must be a literal scalar, a path or another intrinsic call; a JSON object literal passed to States.Array is refused at CreateStateMachine with InvalidDefinition / SCHEMA_VALIDATION_FAILED naming the field.
Emulator: CreateStateMachine succeeds and returns a stateMachineArn.
Why: same as cause 8 — an invalid definition is admitted.
Fix: definition validation in the CreateStateMachine handler (intrinsic argument grammar).
Traces: stepfunctions.intrinsic-states-array-object-literal-argument-rejected.

## Evidence
Cause 1 (samples): "States.ArrayGetItem($.items, $.at, 1) must fail the execution, got 'SUCCEEDED' '{\"picked\":42}'"; "States.Hash($.text, 'SHA-256', 'extra') must fail the execution, got 'SUCCEEDED' output='{\"digest\":\"b856d6...\"}'"; "States.MathRandom(5, 5) must fail the execution, got 'SUCCEEDED' output='{\"n\":5}'"; "3 argument(s): expected ('FAILED','States.Runtime', \"... States.StringSplit($.line, '|', '|'). Invalid arguments in States.StringSplit\"), got ('SUCCEEDED', None, None)".
Cause 2: got "... Invalid arguments in States.Format", expected "... Invalid arguments in States.Format: number of arguments do not match the occurrences of {}."; and "... Invalid arguments in States.JsonToString: only a single argument is allowed."
Cause 3: "States.MathAdd($.v, 1) must yield 4, got 3.6"; "MathAdd(2147483647, 1) must fail the execution, got 'SUCCEEDED' output='{\"total\":2147483648}'"; "an out-of-range argument must fail, got 'SUCCEEDED' '{\"r\":2147483649}'".
Cause 4: "expected [1..4], got '{\"r\":[1.2,2.2,3.2,4.2]}'".
Cause 5: "ArrayUnique(...) gave [{'a': 1, 'b': [2]}, {'b': [2], 'a': 1}, [1, 2], [2, 1]], expected the elements [{'a': 1, 'b': [2]}, [1, 2], [2, 1]]".
Cause 6: "hashing 42 should succeed, got 'FAILED' error='States.Runtime' cause=\"... Invalid arguments in States.Hash\"".
Cause 7: "States.StringToJson over edge documents must succeed, got 'FAILED' ... Cannot convert String to Json in States.StringToJson, caused by: Unexpected end-of-input".
Cause 8: "an 11-deep intrinsic chain was accepted as 'arn:aws:states:...'".
Cause 9: "expected the call to be refused, but it returned {'stateMachineArn': ...}".

## Steps to Reproduce
1. Create JSONPath state machines whose Pass/Task fields call each listed intrinsic with the wrong argument count and start executions — they succeed instead of failing with States.Runtime (cause 1).
2. Trigger States.Format/States.JsonToString argument failures and read the cause's trailing clause (cause 2).
3. Evaluate States.MathAdd with a fractional argument, with 2147483647+1 and with an out-of-range argument (cause 3); States.ArrayRange with a fractional start (cause 4); States.ArrayUnique over deep-equal objects (cause 5); States.Hash over the number 42 (cause 6); States.StringToJson over an empty/scalar document (cause 7).
4. CreateStateMachine with an 11-deep intrinsic chain (cause 8) and with States.Array given an object literal (cause 9).
5. Or replay the check ids in trace_ids.
