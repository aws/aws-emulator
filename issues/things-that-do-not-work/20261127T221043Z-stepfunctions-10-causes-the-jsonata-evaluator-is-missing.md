Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: stepfunctions.jsonata-contains-empty-unicode-and-literal-pattern-edges stepfunctions.jsonata-contains-non-string-subject-rejected stepfunctions.jsonata-regex-literals-and-distinct-evaluate stepfunctions.jsonata-distinct-keeps-first-occurrences-by-deep-equality stepfunctions.jsonata-distinct-keeps-first-of-deep-equal-values-without-coercion stepfunctions.jsonata-distinct-returns-a-non-array-argument-unchanged stepfunctions.jsonata-hash-each-algorithm-matches-computed-digest stepfunctions.jsonata-hash-empty-and-unicode-inputs-match-utf8-digests stepfunctions.jsonata-hash-numeric-source-fails-execution-rejected stepfunctions.jsonata-parse-deserializes-json-text-from-execution-input stepfunctions.jsonata-parse-non-string-argument-fails-execution-rejected stepfunctions.jsonata-parse-of-malformed-json-text-rejected stepfunctions.jsonata-partition-chunks-drive-map-iterations stepfunctions.jsonata-partition-edge-sizes-and-unicode-elements stepfunctions.jsonata-partition-string-argument-fails-execution-rejected stepfunctions.jsonata-step-functions-provided-functions-evaluate stepfunctions.jsonata-range-edges-single-descending-overshoot-fractional stepfunctions.jsonata-range-string-bound-fails-execution-rejected stepfunctions.jsonata-random-seed-repeats-value stepfunctions.jsonata-random-string-seed-rejected stepfunctions.jsonata-random-unseeded-values-vary-within-unit-interval stepfunctions.jsonata-random-zero-seed-repeats-across-executions stepfunctions.jsonata-standard-library-numeric-and-object-functions-evaluate stepfunctions.jsonata-to-millis-parses-picture-strings stepfunctions.jsonata-now-is-one-utc-instant-inside-the-execution-window stepfunctions.jsonata-now-non-string-picture-fails-execution-rejected stepfunctions.jsonata-now-one-timestamp-per-expression-inside-execution-window stepfunctions.jsonata-millis-given-an-argument-fails-execution-rejected
Issue-Shape-Id: com.amazonaws.sfn#AWSStepFunctions
Issue-Key: stepfunctions.jsonata-contains-empty-unicode-and-literal-pattern-edges|run#7|AssertionError|assert desc["status"] == "SUCCEEDED", (
Issue-Key: stepfunctions.jsonata-contains-non-string-subject-rejected|run#6|AssertionError|assert good["status"] == "SUCCEEDED", (
Issue-Key: stepfunctions.jsonata-regex-literals-and-distinct-evaluate|run#5|AssertionError|assert resp["status"] == "SUCCEEDED", (
Issue-Key: stepfunctions.jsonata-distinct-keeps-first-occurrences-by-deep-equality|run#8|AssertionError|assert desc["status"] == "SUCCEEDED", (
Issue-Key: stepfunctions.jsonata-distinct-keeps-first-of-deep-equal-values-without-coercion|run#8|AssertionError|assert done["status"] == "SUCCEEDED", (
Issue-Key: stepfunctions.jsonata-distinct-returns-a-non-array-argument-unchanged|run#3|AssertionError|assert desc["status"] == "SUCCEEDED", (
Issue-Key: stepfunctions.jsonata-hash-each-algorithm-matches-computed-digest|run#4|AssertionError|assert done["status"] == "SUCCEEDED", (
Issue-Key: stepfunctions.jsonata-hash-empty-and-unicode-inputs-match-utf8-digests|run#5|AssertionError|assert desc["status"] == "SUCCEEDED", (
Issue-Key: stepfunctions.jsonata-hash-numeric-source-fails-execution-rejected|run#4|AssertionError|assert good["status"] == "SUCCEEDED", (
Issue-Key: stepfunctions.jsonata-parse-deserializes-json-text-from-execution-input|run#13|AssertionError|assert desc["status"] == "SUCCEEDED", (
Issue-Key: stepfunctions.jsonata-parse-non-string-argument-fails-execution-rejected|run#3|AssertionError|assert ok["status"] == "SUCCEEDED", (
Issue-Key: stepfunctions.jsonata-parse-of-malformed-json-text-rejected|run#5|AssertionError|assert desc["status"] == "SUCCEEDED", (
Issue-Key: stepfunctions.jsonata-partition-chunks-drive-map-iterations|run#6|AssertionError|assert desc["status"] == "SUCCEEDED", (
Issue-Key: stepfunctions.jsonata-partition-edge-sizes-and-unicode-elements|run#9|AssertionError|assert desc["status"] == "SUCCEEDED", (
Issue-Key: stepfunctions.jsonata-partition-string-argument-fails-execution-rejected|run#5|AssertionError|assert good["status"] == "SUCCEEDED", (
Issue-Key: stepfunctions.jsonata-step-functions-provided-functions-evaluate|run#5|AssertionError|assert resp["status"] == "SUCCEEDED", (
Issue-Key: stepfunctions.jsonata-range-edges-single-descending-overshoot-fractional|run#4|AssertionError|assert done["status"] == "SUCCEEDED", (
Issue-Key: stepfunctions.jsonata-range-string-bound-fails-execution-rejected|run#4|AssertionError|assert good["status"] == "SUCCEEDED", (
Issue-Key: stepfunctions.jsonata-random-seed-repeats-value|run#3|AssertionError|assert resp["status"] == "SUCCEEDED", (
Issue-Key: stepfunctions.jsonata-random-string-seed-rejected|run#4|AssertionError|assert ok["status"] == "SUCCEEDED", (
Issue-Key: stepfunctions.jsonata-random-unseeded-values-vary-within-unit-interval|run#4|AssertionError|assert desc["status"] == "SUCCEEDED", (
Issue-Key: stepfunctions.jsonata-random-zero-seed-repeats-across-executions|_draw#3|AssertionError|assert done["status"] == "SUCCEEDED", (
Issue-Key: stepfunctions.jsonata-standard-library-numeric-and-object-functions-evaluate|run#5|AssertionError|assert resp["status"] == "SUCCEEDED", (
Issue-Key: stepfunctions.jsonata-to-millis-parses-picture-strings|run#5|AssertionError|assert resp["status"] == "SUCCEEDED", (
Issue-Key: stepfunctions.jsonata-now-is-one-utc-instant-inside-the-execution-window|run#11|AssertionError|assert out.get("pictured") == first[:19], (
Issue-Key: stepfunctions.jsonata-now-non-string-picture-fails-execution-rejected|run#11|AssertionError|assert out["day"] in days, (
Issue-Key: stepfunctions.jsonata-now-one-timestamp-per-expression-inside-execution-window|_stamp#3|AssertionError|assert out["day"] == instant.strftime("%Y-%m-%d"), (
Issue-Key: stepfunctions.jsonata-millis-given-an-argument-fails-execution-rejected|run#18|AssertionError|assert misuse["status"] == "FAILED", (
Issue-Commit: 3c63a583c399d299d4ea298f38dfae7b708152ae

# stepfunctions: 10 causes — the JSONata evaluator is missing library functions ($distinct, $hash, $parse, $partition, $range, $formatNumber), refuses regex literals, and ignores $random/$now/$millis/$toMillis arguments

> _Note: this issue was identified against the code base at commit `3c63a583c399d299d4ea298f38dfae7b708152ae`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartExecution

## Description
Cause 1 of 10: regular-expression literals in a JSONata expression are refused instead of evaluated.
AWS: a JSONata query may use a regex literal (/pattern/) as an argument to $contains, $replace, $match etc., and the expression evaluates normally; the execution SUCCEEDs.
Emulator: the execution ends FAILED with error 'States.QueryEvaluationError' and a cause of the emulator's own: "'$contains($states.input.word, /./)' uses a regular-expression literal, whose cost this emulator cannot bound, so it was not evaluated" (and the same sentence for $replace).
Why: every JSONata query using a regex fails, and the cause is a sentence no AWS client can interpret or match.
Fix: the JSONata evaluator in the Step Functions execution engine — support regex literals.
Traces: stepfunctions.jsonata-contains-empty-unicode-and-literal-pattern-edges, stepfunctions.jsonata-contains-non-string-subject-rejected, stepfunctions.jsonata-regex-literals-and-distinct-evaluate.

Cause 2 of 10: $distinct is refused instead of evaluated.
AWS: $distinct keeps the first occurrence of each value by deep equality without type coercion, and returns a non-array argument unchanged; the execution SUCCEEDs.
Emulator: FAILED, 'States.QueryEvaluationError', cause "'$distinct(...)' uses $distinct, whose cost this emulator cannot bound, so it was not evaluated" — even for $distinct([]) and $distinct('banana').
Why: a documented standard function is unusable, with a non-AWS cause.
Fix: the JSONata function library in the execution engine.
Traces: stepfunctions.jsonata-distinct-keeps-first-occurrences-by-deep-equality, stepfunctions.jsonata-distinct-keeps-first-of-deep-equal-values-without-coercion, stepfunctions.jsonata-distinct-returns-a-non-array-argument-unchanged.

Cause 3 of 10: $hash is not registered as a function.
AWS: Step Functions' JSONata library provides $hash(text, algorithm) for MD5/SHA-1/SHA-256/SHA-384/SHA-512, returning the hex digest of the UTF-8 bytes; a numeric first argument fails with a signature error naming the function.
Emulator: every call fails with 'States.QueryEvaluationError' and cause "... threw an error during evaluation. T1006: Attempted to invoke a non-function" — the name is unbound.
Why: digest-based workflows cannot run, and the failure reports an unbound-name error rather than the service's result or signature error.
Fix: the JSONata function library in the execution engine.
Traces: stepfunctions.jsonata-hash-each-algorithm-matches-computed-digest, stepfunctions.jsonata-hash-empty-and-unicode-inputs-match-utf8-digests, stepfunctions.jsonata-hash-numeric-source-fails-execution-rejected.

Cause 4 of 10: $parse is not registered as a function.
AWS: $parse(text) deserializes JSON text; a non-string argument fails with a signature error, and malformed JSON text fails with a parse error naming the function.
Emulator: every call fails 'States.QueryEvaluationError' with "T1006: Attempted to invoke a non-function".
Why: JSON text arriving in execution input cannot be decoded, and all three outcomes (success, signature error, parse error) collapse into one unbound-name error.
Fix: the JSONata function library in the execution engine.
Traces: stepfunctions.jsonata-parse-deserializes-json-text-from-execution-input, stepfunctions.jsonata-parse-non-string-argument-fails-execution-rejected, stepfunctions.jsonata-parse-of-malformed-json-text-rejected.

Cause 5 of 10: $partition is not registered as a function.
AWS: $partition(array, size) chunks an array into batches (used to drive Map iterations), handles edge sizes and unicode elements, and fails with a signature error for a string argument.
Emulator: every call fails 'States.QueryEvaluationError' with "T1006: Attempted to invoke a non-function" — including $partition([1,2,3,4,5], 2).
Why: the documented batching helper is unusable, so Map fan-out built on it cannot run.
Fix: the JSONata function library in the execution engine.
Traces: stepfunctions.jsonata-partition-chunks-drive-map-iterations, stepfunctions.jsonata-partition-edge-sizes-and-unicode-elements, stepfunctions.jsonata-partition-string-argument-fails-execution-rejected, stepfunctions.jsonata-step-functions-provided-functions-evaluate.

Cause 6 of 10: $range is not registered as a function.
AWS: $range(start, end, step) builds a numeric sequence, handling single-element, descending, overshooting and fractional bounds, and fails with a signature error for a string bound.
Emulator: every call fails 'States.QueryEvaluationError' with "T1006: Attempted to invoke a non-function" — including $range(0, 3, 1).
Why: sequence generation inside a JSONata query is impossible.
Fix: the JSONata function library in the execution engine.
Traces: stepfunctions.jsonata-range-edges-single-descending-overshoot-fractional, stepfunctions.jsonata-range-string-bound-fails-execution-rejected.

Cause 7 of 10: $random rejects its seed argument.
AWS: Step Functions' $random accepts an optional integer seed — $random(42) and $random(0) evaluate, a given seed repeats the same value across executions, an unseeded call varies within [0,1), and a string seed fails with a signature error.
Emulator: any seeded call fails 'States.QueryEvaluationError' with "... T0410: Argument 0 of function random does not match function signature", so even the unseeded-variation check cannot complete.
Why: deterministic, seeded randomness — the whole point of the seed form — is unavailable.
Fix: the $random signature in the JSONata function library.
Traces: stepfunctions.jsonata-random-seed-repeats-value, stepfunctions.jsonata-random-string-seed-rejected, stepfunctions.jsonata-random-unseeded-values-vary-within-unit-interval, stepfunctions.jsonata-random-zero-seed-repeats-across-executions.

Cause 8 of 10: $formatNumber is not registered as a function.
AWS: the JSONata standard library's $formatNumber(number, picture) evaluates — $formatNumber(1234.5, '#,##0.00') returns the formatted string.
Emulator: 'States.QueryEvaluationError', "T1006: Attempted to invoke a non-function".
Why: standard-library numeric formatting inside a query fails.
Fix: the JSONata function library in the execution engine.
Traces: stepfunctions.jsonata-standard-library-numeric-and-object-functions-evaluate.

Cause 9 of 10: $toMillis ignores its picture string and returns nothing.
AWS: $toMillis(text, picture) parses the text according to the picture — $toMillis('13/04/2020', '[D01]/[M01]/[Y0001]') returns the epoch millis of that date.
Emulator: the execution fails with 'States.QueryEvaluationError' and cause "... specified for the field 'Output/r0' returned nothing (undefined)."
Why: dates in any non-ISO format cannot be parsed in a query.
Fix: picture-string support for $toMillis in the JSONata function library.
Traces: stepfunctions.jsonata-to-millis-parses-picture-strings.

Cause 10 of 10: $now ignores its picture argument and $millis accepts an argument it must refuse.
AWS: $now(picture) formats the current instant with the picture ($now('[Y0001]-[M01]-[D01]') yields '2026-11-27'), one instant is used for the whole expression, and $millis takes no arguments — $millis(x) fails the execution with a signature error naming the function.
Emulator: $now returns the default ISO instant '2026-11-27T21:43:59.370Z' whatever picture is given (and a non-string picture is not refused), and $millis with an argument SUCCEEDs returning {"now": <epoch millis>}.
Why: formatted timestamps in a query come out in the wrong format, and an invalid $millis call that AWS fails runs here.
Fix: the $now/$millis signatures and picture formatting in the JSONata function library.
Traces: stepfunctions.jsonata-now-is-one-utc-instant-inside-the-execution-window, stepfunctions.jsonata-now-non-string-picture-fails-execution-rejected, stepfunctions.jsonata-now-one-timestamp-per-expression-inside-execution-window, stepfunctions.jsonata-millis-given-an-argument-fails-execution-rejected.

## Evidence
Cause 1: cause "'$contains($states.input.word, /./)' uses a regular-expression literal, whose cost this emulator cannot bound, so it was not evaluated".
Cause 2: "'$distinct([])' uses $distinct, whose cost this emulator cannot bound, so it was not evaluated".
Cause 3/4/5/6/8: "... threw an error during evaluation. T1006: Attempted to invoke a non-function" for $hash, $parse, $partition, $range, $formatNumber (e.g. "$partition([1,2,3,4,5], 2)").
Cause 7: "... '$random(42)' ... T0410: Argument 0 of function random does not match function signature".
Cause 9: "'$toMillis('13/04/2020', '[D01]/[M01]/[Y0001]')' ... returned nothing (undefined)."
Cause 10: "$now with '[Y0001]-[M01]-[D01]' gave '2026-11-27T21:50:39.991Z', expected one of ['2026-11-27']"; "$millis with an argument ended 'SUCCEEDED', output='{\"now\":1795815987339}'".

## Steps to Reproduce
1. Create a QueryLanguage=JSONata state machine whose Output/Arguments use a regex literal, $distinct, $hash, $parse, $partition, $range, $formatNumber, $random(seed), $now(picture), $millis(arg) and $toMillis(text,picture).
2. Start an execution for each and read status/error/cause: the regex and $distinct cases are refused with emulator-authored causes, the missing functions report T1006, $random rejects its seed, $now/$toMillis ignore pictures, $millis accepts an argument.
3. Or replay the check ids in trace_ids.
