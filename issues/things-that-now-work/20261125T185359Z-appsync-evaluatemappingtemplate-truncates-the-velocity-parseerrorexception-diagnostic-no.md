Issue-Source: trace-sweep-divergence
Issue-Trace: appsync-unit-resolver-code-evaluation
Issue-Step: task#6
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: appsync-unit-resolver-code-evaluation
Issue-Shape-Id: com.amazonaws.appsync#AWSDeepdishControlPlaneService
Issue-Key: appsync|EvaluateMappingTemplate|2xx->2xx|$.error.message
Issue-Commit: 104107cc43bbc6c46135da3b49b9e3149b60f2be

# AppSync EvaluateMappingTemplate truncates the Velocity ParseErrorException diagnostic: no velocity[line,column] location and no "Was expecting one of" token list

> _Note: this issue was identified against the code base at commit `104107cc43bbc6c46135da3b49b9e3149b60f2be`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
EvaluateMappingTemplate

## Description
One net-new root cause in EvaluateMappingTemplate: the 200 error envelope IS produced for an unparseable VTL template, but `error.message` carries an emulator-abbreviated one-line diagnostic instead of the Apache Velocity ParseErrorException diagnostic AWS returns, and the reported column is off by one.

This is NOT covered by the open issues on this operation. 20261121T235951Z-appsync-evaluatemappingtemplate-a-vtl-parse-failure-is-a.md (cause 1) is about a parse failure being refused HTTP 400 BadRequestException with emulator-authored prose instead of a 200 envelope; here the status and envelope are already correct (expected 200, actual 200) and only the message CONTENT diverges, so closing that issue's status/envelope cause leaves this text defect in place. 20261124T211349Z-appsync-evaluatemappingtemplate-the-vtl-evaluator-cannot-parse-a.md is about `#foreach` over a range expression `[a..b]`, a different template construct. 20261029T174359Z-appsync-evaluatemappingtemplate-stub.md describes a 501 scaffold stub and no longer describes this operation.

Cause 1 of 1: the ParseErrorException text in `error.message` is abbreviated and its position is wrong.
AWS: Smithy (appsync) EvaluateMappingTemplate — POST /v1/dataplane-evaluatetemplate, HTTP success code 200; required input `template` (Template, @length 2..65536, @pattern ^[\s\S]*$) and `context` (Context, @length 2..28000); output members `evaluationResult` (@length 0..65536), `error` (ErrorDetail{message}), `logs`, `stash`, `outErrors`; modeled errors AccessDeniedException|403, BadRequestException|400, InternalFailureException|500. Because `error` is an OUTPUT member of a 200 response, a template that fails to parse is a SERVED call whose diagnostic is the body. For the message text itself: recorded behavior only. The recording shows AWS relaying the Apache Velocity parser's own diagnostic verbatim, which has a fixed skeleton and variable parts:
  - fixed text: the exception class name `org.apache.velocity.exception.ParseErrorException: `, then `Encountered "` … `" at velocity[line `, `, column `, `]`, then a newline and the literal line `Was expecting one of:`, then one indented (four-space) `<TOKEN> ...` / `"literal" ...` line per token the grammar would accept at that position.
  - variable parts, all taken from the submitted template: the offending token as the parser saw it (here the end-of-input token `<EOF>`), the 1-based line number and the 1-based column number of that token, and the accepted-token list the grammar yields at that point (for an unterminated reference/expression it is the operator and grouping set: `"["`, `<RPAREN>`, `<WHITESPACE>`, `"-"`, `"+"`, `"*"`, `"/"`, `"%"`, `<LOGICAL_AND>`, `<LOGICAL_OR>`, `<LOGICAL_LT>`, `<LOGICAL_LE>`, `<LOGICAL_GT>`, `<LOGICAL_GE>`, `<LOGICAL_EQUALS>`, `<LOGICAL_NOT_EQUALS>`, …).
Note in particular that AWS words the position as `at velocity[line L, column C]` — `velocity` is the parser's template name and is part of the fixed text — and that the diagnostic is MULTI-LINE.
Emulator: answers 200 with a single-line, truncated message of the form `org.apache.velocity.exception.ParseErrorException: Encountered "<EOF>" at line L, column C.` — i.e. it drops the `velocity[...]` template-name wrapper around the position, terminates the position with a period instead of `]`, omits the entire `Was expecting one of:` section and every accepted-token line, and reports the column one position further right than the parser does (the recorded step shows column 23 where AWS reports column 22, because the emulator points at the character after end-of-input rather than at the offending token).
Why: this message is the only machine-readable description of WHERE and WHY a template is broken, and it is exactly what template editors, linters and CI checks parse. A client that extracts the line/column with the documented `velocity[line L, column C]` shape finds no match and reports the template as failing for an unknown reason; a client that surfaces the expected-token list to the developer has nothing to show; and a client that highlights the reported column underlines the wrong character. Because the status is 200 either way, nothing else signals that the diagnostic is incomplete.
Fix: the VTL parse-error formatting in the EvaluateMappingTemplate path — the evaluator's parse-failure-to-ErrorDetail conversion must render the Velocity ParseErrorException diagnostic in full (class name, `Encountered "<token>" at velocity[line L, column C]`, the `Was expecting one of:` header and one indented line per accepted token) and must report the offending token's own column rather than the following one. It belongs in the error-detail construction of the operation's handler/VTL engine, not in the response serializer, which already emits the 200 envelope correctly.
Traces: appsync-unit-resolver-code-evaluation#task#6.

## Evidence
Wire corpus simparity-wire-aws-v1 replayed against the emulator built from this checkout (743 steps, 703 passed, 40 mismatched, score 94.6%; emulator_up true, bootstrap_signal false). Divergence cluster service=appsync|EvaluateMappingTemplate|status=200->200|keys=$.error.message, verdict mismatch, affected_count 1, stub_path empty, owned_by empty (not already filed and not owned by any open issue).

Fixture appsync-unit-resolver-code-evaluation, step task#6 — EvaluateMappingTemplate, expected HTTP 200, actual HTTP 200.
$.error.message: value_mismatch
  expected: 'org.apache.velocity.exception.ParseErrorException: Encountered "<EOF>" at velocity[line 1, column 22]\nWas expecting one of:\n    "[" ...\n    <RPAREN> ...\n    <WHITESPACE> ...\n    "-" ...\n    "+" ...\n    "*" ...\n    "/" ...\n    "%" ...\n    <LOGICAL_AND> ...\n    <LOGICAL_OR> ...\n    <LOGICAL_LT> ...\n    <LOGICAL_LE> ...\n    <LOGICAL_GT> ...\n    <LOGICAL_GE> ...\n    <LOGICAL_EQUALS> ...\n    <LOGICAL_NOT_EQUALS> ...\n    '
  actual:   'org.apache.velocity.exception.ParseErrorException: Encountered "<EOF>" at line 1, column 23.'

Reading of that pair: fixed text that the emulator must reproduce exactly = 'org.apache.velocity.exception.ParseErrorException: Encountered "', '" at velocity[line ', ', column ', ']', the newline, 'Was expecting one of:', and each four-space-indented accepted-token line. Values taken from the request (the submitted template) = the offending token '<EOF>', the line number, the column number and the accepted-token list the grammar produces at that position. The literal numbers 1/22 in this recording are an instance of the template, not a requirement; the DEFECTS are (a) the missing 'velocity[...]' position wrapper and trailing ']' (replaced by a period), (b) the wholly missing 'Was expecting one of:' section and token lines, and (c) the column being reported as 23 where the parser's own position is 22.

Smithy cited: appsync EvaluateMappingTemplate — POST /v1/dataplane-evaluatetemplate, code 200; template @required @length 2..65536 @pattern ^[\s\S]*$; context @required @length 2..28000; output evaluationResult (@length 0..65536) / error (ErrorDetail) / logs / stash / outErrors; errors AccessDeniedException|403, BadRequestException|400, InternalFailureException|500.

## Steps to Reproduce
Against the emulator, as a principal allowed appsync:EvaluateMappingTemplate:
1. Replay fixture appsync-unit-resolver-code-evaluation and stop at step task#6, or issue the equivalent call directly: POST /v1/dataplane-evaluatetemplate (aws appsync evaluate-mapping-template --template <tpl> --context '{"arguments":{}}') with a template whose reference/expression is left unterminated so the VTL parser hits end-of-input mid-expression (the fixture's template ends after an open reference at line 1).
2. Observe HTTP 200 with an `error.message` of 'org.apache.velocity.exception.ParseErrorException: Encountered "<EOF>" at line 1, column 23.' and no `evaluationResult`.
3. Expected: the same HTTP 200 and empty `evaluationResult`, but `error.message` carrying the full Velocity diagnostic — 'org.apache.velocity.exception.ParseErrorException: Encountered "<EOF>" at velocity[line 1, column 22]', then a newline, then 'Was expecting one of:' followed by one four-space-indented line per accepted token ('"[" ...', '<RPAREN> ...', '<WHITESPACE> ...', '"-" ...', '"+" ...', '"*" ...', '"/" ...', '"%" ...', '<LOGICAL_AND> ...', '<LOGICAL_OR> ...', '<LOGICAL_LT> ...', '<LOGICAL_LE> ...', '<LOGICAL_GT> ...', '<LOGICAL_GE> ...', '<LOGICAL_EQUALS> ...', '<LOGICAL_NOT_EQUALS> ...').
4. Control: a well-formed template on the same context renders 200 with a populated `evaluationResult` and no `error`, so only the parse-diagnostic text diverges.
5. Re-replay appsync-unit-resolver-code-evaluation#task#6 to confirm the fix.
