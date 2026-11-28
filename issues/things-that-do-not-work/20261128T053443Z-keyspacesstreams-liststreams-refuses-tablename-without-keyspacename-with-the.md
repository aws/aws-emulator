Issue-Source: trace-sweep-divergence
Issue-Trace: keyspacesstreams-list-streams-scope-filter-edges
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: keyspacesstreams-list-streams-scope-filter-edges
Issue-Shape-Id: com.amazonaws.keyspacesstreams#KeyspacesStreams
Issue-Key: keyspacesstreams|ListStreams|4xx->4xx|$.message
Issue-Commit: cb20f68e94bbc6cb070c362820a8401b3373aae4

# keyspacesstreams ListStreams refuses tableName-without-keyspaceName with the generic parameter-values message instead of naming the required KeyspaceName

> _Note: this issue was identified against the code base at commit `cb20f68e94bbc6cb070c362820a8401b3373aae4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListStreams

## Description
One root cause in the KeyspacesStreams emulator's ListStreams input validation: the scope-filter combination refusal (tableName supplied with no keyspaceName) is worded with this service's generic catch-all parameter sentence rather than with the specific sentence AWS states for that combination.

Cause 1 of 1: ListStreams(tableName=<tbl>) with keyspaceName omitted is refused HTTP 400 ValidationException with the wrong message text.
  AWS: the Smithy model (services/service-com-amazonaws-keyspacesstreams-keyspacesstreams/smithy-model/com-amazonaws-keyspacesstreams-keyspacesstreams.json) states for ListStreams.tableName (TableName, @length 1..48, pattern ^[a-zA-Z0-9][a-zA-Z0-9_]{0,47}$): "The name of the table for which to list streams. Must be used together with keyspaceName." keyspaceName is optional on its own, so the constraint is a cross-member one the operation itself must enforce; the model's refusal shape for it is ValidationException (httpError 400, members message + errorCode of type ValidationExceptionType). For the message text this is recorded behavior only: the recording shows the refusal worded "KeyspaceName is required when TableName is specified." That whole sentence is FIXED text — the two member names in it are the model's member names (KeyspaceName, TableName), not values taken from the request, and the sentence interpolates nothing from the request or from state (no keyspace name, no table name, no ARN). The recorded status is 400 and the recorded error shape is ValidationException.
  Emulator: services/service-com-amazonaws-keyspacesstreams-keyspacesstreams/src/ops/list_streams.rs lines 206-210 detect the combination correctly (`if input.table_name.is_some() && input.keyspace_name.is_none()`) but answer with `invalid_parameters()` (lines 314-320), which words the refusal with this service's shared catch-all constant `crate::service::refusals::INVALID_PARAMETER_VALUES` = "One or more parameter values were invalid" (src/service/refusals.rs line 27), errorCode "InvalidFormat". The replay reports, at the same 400 and the same error shape, $.message value_mismatch: expected 'KeyspaceName is required when TableName is specified.', got 'One or more parameter values were invalid'.
  Why: the message is the only thing that tells a caller WHICH parameter combination the service rejected — the status (400), the shape (ValidationException) and the errorCode (InvalidFormat) are shared by every other value refusal of this operation (a malformed nextToken length, an out-of-range maxResults, a bad filter). A client or SDK that branches on, logs, or surfaces the refusal text cannot distinguish "you must also send keyspaceName" from "one of your values is malformed", so it retries the same request with a corrected value instead of adding the missing keyspaceName, and a user-facing tool shows an unactionable generic error where AWS names the member to supply. Tests and error-handling code written against AWS's sentence fail against the emulator.
  Fix: this operation's input validation in its handler — the cross-member check in ListStreams::handle must raise its OWN ValidationException message for the tableName-requires-keyspaceName combination instead of reusing the generic INVALID_PARAMETER_VALUES refusal, with that sentence carried as its own constant alongside the other message constants in this service's refusals module (so the same combination is worded identically wherever else it is enforced). The status, the error shape and the errorCode are already right; only the wording is wrong. Note: the open issue "KeyspacesStreams: a freshly enabled CDC stream has only one shard, and a nextToken replayed under a changed filter is refused with the wrong message" (20261127T132105Z) is about a DIFFERENT refusal collapse in the same file (PageTokenError::WrongQuery vs NotIssued at lines 220-231) and does not cover this cross-member combination refusal.
  Traces: keyspacesstreams-list-streams-scope-filter-edges#task#2 (cluster representative), keyspacesstreams-list-streams-scope-filter-edges#task#3 (same cause, same cluster).

## Evidence
Fixture keyspacesstreams-list-streams-scope-filter-edges, step task#2 (cluster of 2 steps: task#2, task#3), verdict mismatch, divergence_key service=keyspacesstreams|ListStreams|status=400->400|keys=$.message.

Expected (recorded AWS response): HTTP 400, ValidationException, message "KeyspaceName is required when TableName is specified." — fixed text, no variable parts; the names KeyspaceName/TableName are the model's member names, nothing is interpolated from the request.
Actual (emulator): HTTP 400, ValidationException, message "One or more parameter values were invalid", errorCode "InvalidFormat".
Replay detail: "$.message: value_mismatch (expected 'KeyspaceName is required when TableName is specified.', got 'One or more parameter values were invalid')".

Status and error shape agree; only the message diverges.

Source: src/ops/list_streams.rs:206-210 returns invalid_parameters(); src/ops/list_streams.rs:314-320 words it with crate::service::refusals::INVALID_PARAMETER_VALUES; src/service/refusals.rs:27 defines that constant as the service-wide catch-all "One or more parameter values were invalid".
Model: smithy_query(query="operation", operation="ListStreams") — input members keyspaceName/tableName both optional; ListStreams.tableName doc "Must be used together with keyspaceName."; errors include ValidationException (httpError 400).

## Steps to Reproduce
1. Boot the emulator and point a KeyspacesStreams client at its data listener.
2. Call keyspacesstreams.ListStreams with tableName set to any syntactically valid table name (1..48 chars, ^[a-zA-Z0-9][a-zA-Z0-9_]{0,47}$) and keyspaceName OMITTED — e.g. ListStreams(tableName=<tbl>), and again with maxResults also set (the second step of the same fixture).
3. Observe HTTP 400 ValidationException with message "One or more parameter values were invalid" and errorCode "InvalidFormat".
4. AWS answers HTTP 400 ValidationException with message "KeyspaceName is required when TableName is specified."
5. Control: ListStreams(keyspaceName=<ks>, tableName=<tbl>) must still succeed, and ListStreams() with neither filter must still list every stream in the region — only the tableName-without-keyspaceName combination is refused.
