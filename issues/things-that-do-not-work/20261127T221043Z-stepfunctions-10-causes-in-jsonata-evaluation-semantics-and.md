Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: stepfunctions.jsonata-contains-with-a-number-as-its-string-fails-the-execution-rejected stepfunctions.jsonata-count-with-a-second-argument-fails-signature-rejected stepfunctions.jsonata-exists-with-a-second-argument-fails-the-execution-rejected stepfunctions.jsonata-filter-non-function-predicate-fails-execution-rejected stepfunctions.jsonata-join-of-numbers-fails-execution-with-query-evaluation-error-rejected stepfunctions.jsonata-lookup-non-string-key-fails-execution-rejected stepfunctions.jsonata-number-of-an-object-fails-execution-rejected stepfunctions.jsonata-reduce-non-function-argument-rejected stepfunctions.jsonata-replace-non-string-argument-fails-execution-rejected stepfunctions.jsonata-round-with-numeric-string-argument-rejected stepfunctions.jsonata-sort-with-non-function-comparator-rejected stepfunctions.jsonata-split-non-string-argument-rejected stepfunctions.jsonata-substring-non-string-or-non-number-argument-fails-execution-rejected stepfunctions.jsonata-sum-over-an-array-of-strings-fails-the-execution-rejected stepfunctions.jsonata-uuid-given-an-argument-fails-execution-rejected stepfunctions.jsonata-type-names-each-json-kind-function-and-undefined stepfunctions.jsonata-type-names-each-json-value-kind stepfunctions.jsonata-number-empty-unicode-digit-and-one-element-array-rejected stepfunctions.jsonata-number-parses-hex-but-not-octal-or-binary-strings stepfunctions.jsonata-round-edge-arguments-zero-negative-half-single-element-and-absent stepfunctions.jsonata-sort-orders-empty-single-numeric-and-unicode-arrays stepfunctions.jsonata-reduce-edges-empty-single-init-index-and-unicode stepfunctions.jsonata-uppercase-of-a-number-fails-execution-rejected stepfunctions.jsonata-string-with-a-string-prettify-fails-signature-rejected stepfunctions.jsonata-substring-edge-offsets-and-lengths stepfunctions.jsonata-map-items-literal-array-elements-evaluated stepfunctions.jsonata-task-query-evaluation-error-caught-by-the-catcher-naming-it
Issue-Shape-Id: com.amazonaws.sfn#AWSStepFunctions
Issue-Key: stepfunctions.jsonata-contains-with-a-number-as-its-string-fails-the-execution-rejected|run#9|AssertionError|assert got == (
Issue-Key: stepfunctions.jsonata-count-with-a-second-argument-fails-signature-rejected|run#8|AssertionError|assert (bad.get("error"), bad.get("cause")) == (_EVAL_ERROR, cause), (
Issue-Key: stepfunctions.jsonata-exists-with-a-second-argument-fails-the-execution-rejected|run#13|AssertionError|assert (bad.get("error"), bad.get("cause")) == ("States.QueryEvaluationError", expected_cause), (
Issue-Key: stepfunctions.jsonata-filter-non-function-predicate-fails-execution-rejected|run#12|AssertionError|assert bad.get("cause") == (
Issue-Key: stepfunctions.jsonata-join-of-numbers-fails-execution-with-query-evaluation-error-rejected|run#11|AssertionError|assert got == ("States.QueryEvaluationError", _CAUSE), (
Issue-Key: stepfunctions.jsonata-lookup-non-string-key-fails-execution-rejected|run#12|AssertionError|assert got == ("FAILED", "States.QueryEvaluationError", cause), f"a number key gave {got!r}"
Issue-Key: stepfunctions.jsonata-number-of-an-object-fails-execution-rejected|run#12|AssertionError|assert (bad.get("error"), bad.get("cause")) == ("States.QueryEvaluationError", want_cause), (
Issue-Key: stepfunctions.jsonata-reduce-non-function-argument-rejected|run#11|AssertionError|assert (bad.get("error"), bad.get("cause")) == ("States.QueryEvaluationError", want_cause), (
Issue-Key: stepfunctions.jsonata-replace-non-string-argument-fails-execution-rejected|_assert_evaluation_failure#7|AssertionError|assert desc.get("cause") == expected_cause, f"[{label}] unexpected cause {desc.get('cause')!r}"
Issue-Key: stepfunctions.jsonata-round-with-numeric-string-argument-rejected|run#21|AssertionError|assert not missing, (
Issue-Key: stepfunctions.jsonata-sort-with-non-function-comparator-rejected|run#21|AssertionError|assert not missing, (
Issue-Key: stepfunctions.jsonata-split-non-string-argument-rejected|run#24|AssertionError|assert got == want, f"$split of {wrong!r} ended {got!r}, expected {want!r}"
Issue-Key: stepfunctions.jsonata-substring-non-string-or-non-number-argument-fails-execution-rejected|run#4|AssertionError|assert got == want, f"input {payload!r} gave {got!r}, expected {want!r}"
Issue-Key: stepfunctions.jsonata-sum-over-an-array-of-strings-fails-the-execution-rejected|run#9|AssertionError|assert got == (
Issue-Key: stepfunctions.jsonata-uuid-given-an-argument-fails-execution-rejected|_expect_refused#4|AssertionError|assert not missing, f"$uuid({arg!r}) failure cause lacks {missing!r}: {cause!r}"
Issue-Key: stepfunctions.jsonata-type-names-each-json-kind-function-and-undefined|run#11|AssertionError|assert desc["status"] == "SUCCEEDED", (
Issue-Key: stepfunctions.jsonata-type-names-each-json-value-kind|run#4|AssertionError|assert desc["status"] == "SUCCEEDED", (
Issue-Key: stepfunctions.jsonata-number-empty-unicode-digit-and-one-element-array-rejected|run#11|AssertionError|assert got == ("int", want), (
Issue-Key: stepfunctions.jsonata-number-parses-hex-but-not-octal-or-binary-strings|run#5|AssertionError|assert resp["status"] == "SUCCEEDED", (
Issue-Key: stepfunctions.jsonata-round-edge-arguments-zero-negative-half-single-element-and-absent|run#11|AssertionError|assert got_failure == ("FAILED", "States.QueryEvaluationError", cause), (
Issue-Key: stepfunctions.jsonata-sort-orders-empty-single-numeric-and-unicode-arrays|run#10|AssertionError|assert json.dumps(out.get(key)) == json.dumps(want), (
Issue-Key: stepfunctions.jsonata-reduce-edges-empty-single-init-index-and-unicode|run#9|AssertionError|assert out == expected, f"$reduce produced {out!r}, expected {expected!r}"
Issue-Key: stepfunctions.jsonata-uppercase-of-a-number-fails-execution-rejected|run#8|AssertionError|assert bad["status"] == "FAILED", (
Issue-Key: stepfunctions.jsonata-string-with-a-string-prettify-fails-signature-rejected|run#5|AssertionError|assert out.get("pretty") == json.dumps(_DOC, indent=2), f"pretty form {out.get('pretty')!r}"
Issue-Key: stepfunctions.jsonata-substring-edge-offsets-and-lengths|run#19|AssertionError|assert desc["status"] == "SUCCEEDED", (
Issue-Key: stepfunctions.jsonata-map-items-literal-array-elements-evaluated|run#4|AssertionError|assert json.loads(resp["output"]) == [1, 2, 3], f"Items elements must be evaluated, got {resp['output']!r}"
Issue-Key: stepfunctions.jsonata-task-query-evaluation-error-caught-by-the-catcher-naming-it|run#17|AssertionError|assert isinstance(handled["Cause"], str) and "'WhoAmI'" in handled["Cause"], (
Issue-Commit: 3c63a583c399d299d4ea298f38dfae7b708152ae

# stepfunctions: 10 causes in JSONata evaluation semantics and QueryEvaluationError wording — unquoted function names in T0410/T0412, $type of an absent value, $number/$round/$sort/$reduce/$uppercase/$string edges, unevaluated Items literals

> _Note: this issue was identified against the code base at commit `3c63a583c399d299d4ea298f38dfae7b708152ae`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartExecution

## Description
Cause 1 of 10: the JSONata signature-error messages do not quote the function name (and T0412 does not pluralise the type), and one of them names the wrong argument index.
AWS: a signature failure fails the execution with error 'States.QueryEvaluationError' and cause "An error occurred while executing the state '<state>' (entered at the event id #<n>). The JSONata expression '<expression>' specified for the field '<field>' threw an error during evaluation. T0410: Argument <i> of function \"<name>\" does not match function signature" — the function name is in double quotes. The T0412 form is "T0412: Argument <i> of function \"<name>\" must be an array of \"<type>s\"" (quoted, plural). The argument index is the position AWS reports — for $substring($text, $start, $length) with a non-string first argument and a bad length, AWS reports Argument 3.
Emulator: the name is unquoted and the type is singular — "T0410: Argument 1 of function contains does not match function signature", "T0412: Argument 1 of function join must be an array of string" (AWS: 'of function \"join\" must be an array of \"strings\"'), and for $substring it reports Argument 1 where AWS reports Argument 3.
Why: a client that matches the cause (or extracts the failing function/argument from it) to route error handling reads a different sentence here, and the argument index points at the wrong argument.
Fix: the JSONata error-message formatter in the Step Functions execution engine.
Traces: stepfunctions.jsonata-contains-with-a-number-as-its-string-fails-the-execution-rejected, stepfunctions.jsonata-count-with-a-second-argument-fails-signature-rejected, stepfunctions.jsonata-exists-with-a-second-argument-fails-the-execution-rejected, stepfunctions.jsonata-filter-non-function-predicate-fails-execution-rejected, stepfunctions.jsonata-join-of-numbers-fails-execution-with-query-evaluation-error-rejected, stepfunctions.jsonata-lookup-non-string-key-fails-execution-rejected, stepfunctions.jsonata-number-of-an-object-fails-execution-rejected, stepfunctions.jsonata-reduce-non-function-argument-rejected, stepfunctions.jsonata-replace-non-string-argument-fails-execution-rejected, stepfunctions.jsonata-round-with-numeric-string-argument-rejected, stepfunctions.jsonata-sort-with-non-function-comparator-rejected, stepfunctions.jsonata-split-non-string-argument-rejected, stepfunctions.jsonata-substring-non-string-or-non-number-argument-fails-execution-rejected, stepfunctions.jsonata-sum-over-an-array-of-strings-fails-the-execution-rejected, stepfunctions.jsonata-uuid-given-an-argument-fails-execution-rejected.

Cause 2 of 10: $type of an absent path fails the expression instead of yielding the undefined classification.
AWS: $type over every JSON kind evaluates, and for an absent path it classifies the value rather than failing the expression — the execution SUCCEEDs and the output records the absent/undefined case.
Emulator: FAILED, 'States.QueryEvaluationError', cause "... '$type($states.input.nowhere)' specified for the field 'Output/absentRaw' returned nothing (undefined)."
Why: a query that probes for optional fields fails the whole execution.
Fix: undefined handling in the JSONata evaluator ($type and the assignment of an undefined result to an output field).
Traces: stepfunctions.jsonata-type-names-each-json-kind-function-and-undefined, stepfunctions.jsonata-type-names-each-json-value-kind.

Cause 3 of 10: $number's numeric parsing diverges — '-0' and hex strings.
AWS: $number('-0') yields the JSON integer 0, and $number parses hexadecimal strings ('0x12' → 18) while refusing octal/binary forms.
Emulator: $number('-0') yields the float -0.0, and $number('0x12') fails the execution with "D3030: Unable to cast value to a number: \"0x12\"".
Why: numeric values change type (integer vs float, and -0.0 serialises differently), and hex-encoded numbers cannot be parsed.
Fix: $number in the JSONata function library.
Traces: stepfunctions.jsonata-number-empty-unicode-digit-and-one-element-array-rejected, stepfunctions.jsonata-number-parses-hex-but-not-octal-or-binary-strings.

Cause 4 of 10: $round refuses a single-element array argument.
AWS: JSONata sequence semantics unwrap a one-element sequence, so $round([3.5]) evaluates like $round(3.5); the edge set {0, -2.5, 0.004, [3.5], 1.25} all evaluate.
Emulator: FAILED with "T0410: Argument 1 of function round does not match function signature" for the singleton.
Why: expressions that pass a filtered single-element result into a numeric function fail.
Fix: sequence-to-singleton unwrapping in the JSONata evaluator's argument binding.
Traces: stepfunctions.jsonata-round-edge-arguments-zero-negative-half-single-element-and-absent.

Cause 5 of 10: $sort orders non-BMP and full-width characters differently from AWS.
AWS: $sort of ['A','Z','a','ab','z','é','ｚ','😀'] yields ['A','Z','a','ab','z','é','😀','ｚ'] — the comparison is over UTF-16 code units, so the surrogate-pair emoji sorts before U+FF5A.
Emulator: yields ['A','Z','a','ab','z','é','ｚ','😀'].
Why: sorted output differs for any data containing astral-plane characters, which changes downstream ordering and Map item order.
Fix: the string comparison used by $sort in the JSONata function library.
Traces: stepfunctions.jsonata-sort-orders-empty-single-numeric-and-unicode-arrays.

Cause 6 of 10: $reduce's index argument and unicode string accumulation diverge.
AWS: for the check's inputs $reduce yields indexBare 13 and glyphs '😀中ßé'.
Emulator: indexBare 11 and glyphs '😀中ßéé' — the index passed to the reducer and the string accumulation over combining/astral characters are both wrong.
Why: reductions produce different results, silently, for index-aware reducers and unicode strings.
Fix: $reduce in the JSONata function library (reducer argument binding and string concatenation over code points).
Traces: stepfunctions.jsonata-reduce-edges-empty-single-init-index-and-unicode.

Cause 7 of 10: $uppercase accepts a number and $string accepts a second (prettify) argument that must fail the signature.
AWS: $uppercase requires a string — $uppercase(7) fails the execution with a signature error; and the two-argument $string(value, true) prettify form is not in Step Functions' signature, so it fails rather than returning pretty-printed JSON.
Emulator: $uppercase of a number SUCCEEDs with output '{"kept":7}', and $string(x, true) returns the pretty form '{\n  "sku": "A-17",\n  ...}'.
Why: calls AWS refuses succeed here, so a client's validation path is never taken and the data shape differs.
Fix: the $uppercase/$string signatures in the JSONata function library.
Traces: stepfunctions.jsonata-uppercase-of-a-number-fails-execution-rejected, stepfunctions.jsonata-string-with-a-string-prettify-fails-signature-rejected.

Cause 8 of 10: $substring with offsets past the end of the string is refused with an emulator-authored budget error.
AWS: $substring(word, 10) on a shorter word evaluates to the documented edge result (an empty string), and the execution SUCCEEDs.
Emulator: FAILED, 'States.QueryEvaluationError', cause "evaluating '$substring($states.input.word, 10)' asked for more than the 8388608 bytes this emulator evaluates one expression in, or could not be parsed at all" — a sentence of the emulator's own and a refusal AWS never produces.
Why: ordinary substring edge cases fail, with a cause no client can interpret.
Fix: $substring in the JSONata function library (and the expression-cost guard that fires on it).
Traces: stepfunctions.jsonata-substring-edge-offsets-and-lengths.

Cause 9 of 10: literal array elements in a Map state's Items are not evaluated.
AWS: in JSONata mode, each element of a literal Items array is itself evaluated, so Items [1, "{% 1 + 1 %}", "{% $states.input.three %}"] becomes [1, 2, 3] and drives three iterations.
Emulator: the elements are passed through as the raw strings — '[1,"{% 1 + 1 %}","{% $states.input.three %}"]'.
Why: a Map fans out over template strings instead of evaluated values, so each iteration receives the wrong item.
Fix: Items evaluation in the Map state executor (per-element JSONata evaluation).
Traces: stepfunctions.jsonata-map-items-literal-array-elements-evaluated.

Cause 10 of 10: a JSONata failure in a state's TimeoutSeconds field loses the state preamble in its cause.
AWS: the cause of a query-evaluation failure always begins "An error occurred while executing the state '<state>' (entered at the event id #<n>). " before the expression/field clause, so a Catch handler and an operator can tell which state failed.
Emulator: for a TimeoutSeconds field the cause begins directly with "The JSONata expression '$number($states.input.timeout)' specified for the field 'TimeoutSeconds' threw an error during evaluation. D3030: ..." — the state name is missing.
Why: the failing state cannot be identified from the cause, which is the only place it appears for this failure class.
Fix: the cause composition for query-evaluation failures in the execution engine (apply the state preamble to every evaluated field, not just Output/Arguments).
Traces: stepfunctions.jsonata-task-query-evaluation-error-caught-by-the-catcher-naming-it.

## Evidence
Cause 1: got "T0410: Argument 1 of function contains does not match function signature" / "T0412: Argument 1 of function join must be an array of string"; expected 'of function "contains"' and 'of function "join" must be an array of "strings"'; for $substring expected 'Argument 3 of function "substring"', got 'Argument 1 of function substring'. Two checks assert the missing quotes directly: 'missing [\'"round"\']', 'missing [\'"sort"\']', '$uuid(42) failure cause lacks [\'"uuid"\']'.
Cause 2: "'$type($states.input.nowhere)' ... returned nothing (undefined)."
Cause 3: "$number('-0') gave ('float', '-0.0'), expected the JSON integer 0"; "'$number('0x12')' ... D3030: Unable to cast value to a number: \"0x12\"".
Cause 4: "'$round($states.input.singleton)' ... T0410: Argument 1 of function round does not match function signature".
Cause 5: "words: $sort gave ['A','Z','a','ab','z','é','ｚ','😀'], expected ['A','Z','a','ab','z','é','😀','ｚ']".
Cause 6: "$reduce produced {... 'indexBare': 11, ... 'glyphs': '😀中ßéé'}, expected {... 'indexBare': 13, ... 'glyphs': '😀中ßé'}".
Cause 7: "$uppercase of a number must fail the execution, got 'SUCCEEDED' output='{\"kept\":7}'"; "pretty form '{\\n  \"sku\": \"A-17\", ...}'".
Cause 8: "evaluating '$substring($states.input.word, 10)' asked for more than the 8388608 bytes this emulator evaluates one expression in, or could not be parsed at all".
Cause 9: "Items elements must be evaluated, got '[1,\"{% 1 + 1 %}\",\"{% $states.input.three %}\"]'".
Cause 10: "the caught cause must name the state 'WhoAmI', got 'The JSONata expression '$number($states.input.timeout)' specified for the field 'TimeoutSeconds' threw an error during evaluation. D3030: ...'".

## Steps to Reproduce
1. Run JSONata expressions that fail their signatures ($contains/$count/$exists/$filter/$join/$lookup/$number/$reduce/$replace/$round/$sort/$split/$substring/$sum/$uuid) and compare the T0410/T0412 cause text and argument index (cause 1).
2. Evaluate $type over an absent path (cause 2); $number('-0') and $number('0x12') (cause 3); $round([3.5]) (cause 4); $sort over ['A','Z','a','ab','z','é','ｚ','😀'] (cause 5); the $reduce edge set (cause 6); $uppercase(7) and $string(obj, true) (cause 7); $substring(word, 10) past the end (cause 8).
3. Run a JSONata Map whose Items is a literal array with '{% ... %}' elements (cause 9), and a Task whose TimeoutSeconds expression fails (cause 10).
4. Or replay the check ids in trace_ids.
