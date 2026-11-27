Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bcm-data-exports.list-tables-unissued-next-token-rejected bcm-data-exports.list-executions-unissued-next-token-rejected
Issue-Shape-Id: com.amazonaws.bcmdataexports#AWSBillingAndCostManagementDataExports
Issue-Key: bcm-data-exports.list-tables-unissued-next-token-rejected|run#12|AssertionError|assert got == ("ValidationException", "", 400), (
Issue-Key: bcm-data-exports.list-executions-unissued-next-token-rejected|run#12|AssertionError|assert got == expected, f"an unissued NextToken gave {got!r}, expected {expected!r}"
Issue-Commit: d1c103039c63f2296b4493ce7c42775d3faa3fcc

# bcm-data-exports: every listing refuses an unissued NextToken with one shared emulator sentence, where AWS words it per operation (empty for ListTables, a pagination-token sentence for ListExecutions)

> _Note: this issue was identified against the code base at commit `d1c103039c63f2296b4493ce7c42775d3faa3fcc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListTables

## Description
Two net-new root causes, both from the single shared refusal sentence `UNISSUED_PAGE_TOKEN_MESSAGE` that every paginated listing of this crate uses (services/service-com-amazonaws-bcmdataexports-awsbillingandcostmanagementdataexports/src/state.rs:1016-1018, reached via `page_or_token_refusal`). AWS does not use one sentence for both operations, so one constant cannot be right for both.

Cause 1 of 2: ListTables.
AWS: Smithy `ListTablesRequest.NextToken` is `@length(0,8192) @pattern ^[\S\s]*$`, and `ValidationException.Message` is @required with `@length(0,1024)` — so an EMPTY message is a legal, modelled value. Recorded behavior only: a NextToken this listing never issued is refused HTTP 400 with error code `ValidationException` and an EMPTY `Message` (""). The token the listing DID issue keeps returning the same next page before and after the refusal.
Emulator: HTTP 400 and `ValidationException` are right, but Message is `NextToken is not a pagination token issued by this service.`
Why: a client that branches on the error message (or asserts the refusal carries no explanatory text) sees text AWS never sends; anything that logs or surfaces the message to a user shows an emulator-authored sentence as if it came from AWS.
Fix: the ListTables handler's page-token refusal — it must not take the shared sentence; give the per-operation message at the `page_or_token_refusal` call site in the ListTables handler (the shared helper in `crate::state` stays, parameterised).
Traces: bcm-data-exports.list-tables-unissued-next-token-rejected.

Cause 2 of 2: ListExecutions.
AWS: `ListExecutionsRequest.NextToken` is the same modelled shape. Recorded behavior only: a NextToken this listing never issued is refused HTTP 400 `ValidationException` with Message `Pagination token exception in operation '<OperationName>': invalid payload wrapping schema <schema>` — fixed text `Pagination token exception in operation '`, `': invalid payload wrapping schema `; `<OperationName>` is the operation the call was made against (`ListExecutions`) and `<schema>` is a value derived FROM THE REQUEST's token: the first two bytes of the base64-decoded token read as a signed big-endian 16-bit integer. The tokenless call on the same export still lists, and GetExport still reports the export, after the refusal.
Emulator: HTTP 400 and `ValidationException` are right, Message is the shared `NextToken is not a pagination token issued by this service.`
Why: the message is the only place AWS reports WHY the token was rejected and which operation rejected it; a client or operator matching AWS's sentence cannot tell a malformed-token refusal from any other ValidationException.
Fix: the ListExecutions handler's page-token refusal — the message must be built from the operation name and the schema number decoded out of the submitted token.
Traces: bcm-data-exports.list-executions-unissued-next-token-rejected.

## Evidence
Cause 1 (ListTables) — expected ('ValidationException', '', 400); actual ('ValidationException', 'NextToken is not a pagination token issued by this service.', 400). Reproduced with NextToken='dW5pc3N1ZWQ=' after a MaxResults=1 first page.

Cause 2 (ListExecutions) — expected ('ValidationException', "Pagination token exception in operation 'ListExecutions': invalid payload wrapping schema <schema>", 400) where <schema> is int.from_bytes(base64decode(token)[:2], 'big', signed=True) for the token in the request (the suite's run computed 29545 for its token — that number is a function of the token sent, not a fixed value); actual ('ValidationException', 'NextToken is not a pagination token issued by this service.', 400).

Both messages come from the one constant UNISSUED_PAGE_TOKEN_MESSAGE in src/state.rs:1016-1018, handed to every listing by `page_or_token_refusal`.

## Steps to Reproduce
1. ListTables(MaxResults=1) to get an issued NextToken, then ListTables(MaxResults=1, NextToken='dW5pc3N1ZWQ=') -> inspect Error.Code/Error.Message and the HTTP status; AWS sends ValidationException with an empty Message.
2. CreateExport a valid export, then ListExecutions(ExportArn=<arn>, MaxResults=5, NextToken=<a base64 string this service never issued>) -> inspect Error.Message; AWS sends "Pagination token exception in operation 'ListExecutions': invalid payload wrapping schema <N>" with N decoded from the submitted token's first two bytes.
