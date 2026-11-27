Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events.anything-but-excludes-only-values-of-its-operand-type events.exists-verdicts-are-complementary-across-scalar-and-array-element-types events.exists-matches-on-the-resources-field-inside-or-and-beside-a-value-alternative events.suffix-matches-any-array-element-any-listed-operand-and-the-whole-value events.suffix-never-matches-non-string-values-or-near-miss-strings
Issue-Shape-Id: com.amazonaws.eventbridge#AWSEvents
Issue-Key: events.anything-but-excludes-only-values-of-its-operand-type|_collect#12|AssertionError|raise AssertionError(f"matching events never arrived: {sorted(wanted - got)!r}")
Issue-Key: events.exists-verdicts-are-complementary-across-scalar-and-array-element-types|_delivered#13|AssertionError|raise AssertionError(f"matching events never reached their rule's queue: {missing!r}")
Issue-Key: events.exists-matches-on-the-resources-field-inside-or-and-beside-a-value-alternative|_collect#14|AssertionError|raise AssertionError(f"matching events never arrived: {sorted(wanted - got)!r}")
Issue-Key: events.suffix-matches-any-array-element-any-listed-operand-and-the-whole-value|_collect#12|AssertionError|raise AssertionError(f"matching events never arrived: {sorted(wanted - got)!r}")
Issue-Key: events.suffix-never-matches-non-string-values-or-near-miss-strings|_collect#15|AssertionError|raise AssertionError(f"the string twins never arrived: {sorted(wanted - got)!r}")
Issue-Commit: 7e5e0efa347a2e327a9b3588e8ef7ff7fc53eee8

# events: 3 causes — the event-pattern matcher drops events AWS matches for anything-but, exists and suffix operators

> _Note: this issue was identified against the code base at commit `7e5e0efa347a2e327a9b3588e8ef7ff7fc53eee8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutEvents

## Description
Cause 1 of 3: `anything-but` excludes values of types other than its operand's type. AWS: anything-but is type-aware — a numeric operand excludes only the equal NUMBER, a string operand only the equal STRING, so the "twin" of the other type still matches and is delivered; a list operand excludes only the listed values of their own types. Recorded behavior only (EventPattern is an opaque string in Smithy, @length 0..4096 on PutRule.EventPattern; the semantics are documented in the EventBridge content-filtering guide). Emulator: the events the pattern should still match never reach the rule's queue — 9 of the matrix cases (c15, c16, c17, c19, c2, c3, c4, c5, c7) were never delivered, i.e. the matcher over-excludes across types. Checks: events.anything-but-excludes-only-values-of-its-operand-type.

Cause 2 of 3: `exists` verdicts are wrong for array elements, nested/empty arrays, objects and null leaves, including on the top-level `resources` field inside an $or and beside a value alternative. AWS: exists:true matches when the leaf is PRESENT (including JSON null, booleans, numbers, strings and scalar array elements) and exists:false when it is absent; an object-valued or array-of-objects leaf does not satisfy exists:true, and the two verdicts are exactly complementary. Recorded behavior only (documented content-filtering semantics). Emulator: the matching events never reach their rules — `{'present': ['array-of-mixed-scalars','array-of-null','array-of-numbers','boolean-true','json-null','negative-float','plain-string'], 'absent': ['array-of-objects','leaf-missing','nested-empty-array','plain-object']}` all undelivered, and 6 of the $or/resources cases (c0, c2, c3, c4, c6, c7) never arrived. Checks: events.exists-verdicts-are-complementary-across-scalar-and-array-element-types, events.exists-matches-on-the-resources-field-inside-or-and-beside-a-value-alternative.

Cause 3 of 3: `suffix` does not match array elements, listed operands or the whole value, and its string-only typing is wrong. AWS: suffix matches when ANY element of an array leaf ends with the operand, when ANY operand of a list of suffixes matches, and when the operand equals the whole value; it never matches non-string values (numbers, booleans, null) nor near-miss strings. Recorded behavior only (documented content-filtering semantics). Emulator: the events that must match never arrive — ['c0','c1','c10','c2','c5','c6','c8'] and the string twins ['c0'..'c6'] were never delivered. Checks: events.suffix-matches-any-array-element-any-listed-operand-and-the-whole-value, events.suffix-never-matches-non-string-values-or-near-miss-strings.

Why (all three): a client's rule silently stops matching events AWS routes, so targets are starved with no error anywhere — the most dangerous failure mode for a router, and filtering logic validated against AWS behaves differently here. Note the open issue 20261126T215914Z-events-sqs-the-empty-string-operand-pattern-matrix.md covers the empty-string operand matrix only; these three operators are not covered by it.
Fix: the event-pattern matcher in the events crate (the comparison operators anything-but, exists and suffix, and their type handling for array leaves).
Traces: the five check ids listed per cause above.

## Evidence
`AssertionError: matching events never arrived: ['c15', 'c16', 'c17', 'c19', 'c2', 'c3', 'c4', 'c5', 'c7']` (anything-but)
`AssertionError: matching events never reached their rule's queue: {'present': ['array-of-mixed-scalars', 'array-of-null', 'array-of-numbers', 'boolean-true', 'json-null', 'negative-float', 'plain-string'], 'absent': ['array-of-objects', 'leaf-missing', 'nested-empty-array', 'plain-object']}` (exists)
`AssertionError: matching events never arrived: ['c0', 'c2', 'c3', 'c4', 'c6', 'c7']` (exists inside $or on resources)
`AssertionError: matching events never arrived: ['c0', 'c1', 'c10', 'c2', 'c5', 'c6', 'c8']` and `AssertionError: the string twins never arrived: ['c0', 'c1', 'c2', 'c3', 'c4', 'c5', 'c6']` (suffix)

## Steps to Reproduce
1. events CreateEventBus; PutRule with a pattern using {"anything-but": <number|string|list>} / {"exists": true|false} / {"suffix": "..."} on a detail or resources field; PutTargets naming an SQS queue.
2. PutEvents with the matrix of typed values (numbers, strings, booleans, null, arrays of scalars, arrays of objects, objects, missing leaves).
3. Receive from the queue and compare which case markers arrived against AWS's verdicts.
