Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sagemaker-featurestore-runtime.list-records-invalid-next-token-rejected sagemaker-featurestore-runtime.list-records-unissued-next-token-rejected sagemaker-featurestore-runtime.list-records-next-token-from-other-group-rejected
Issue-Shape-Id: com.amazonaws.sagemakerfeaturestoreruntime#AmazonSageMakerFeatureStoreRuntime
Issue-Key: sagemaker-featurestore-runtime.list-records-invalid-next-token-rejected|run#14|AssertionError|assert got == want, f"an unissued NextToken answered {got!r}, expected {want!r}"
Issue-Key: sagemaker-featurestore-runtime.list-records-unissued-next-token-rejected|run#21|AssertionError|assert got == ("ValidationError", "Invalid pagination token"), f"forged NextToken answered {got!r}"
Issue-Key: sagemaker-featurestore-runtime.list-records-next-token-from-other-group-rejected|_refusal#16|AssertionError|raise AssertionError(f"the request was accepted instead of refused: {out!r}")
Issue-Commit: 96893890513d5e7e8a4c2241420ddac2d94594cf

# ListRecords misnames its invalid-NextToken refusal and does not bind the page cursor to the feature group that issued it, so another group's token is honoured

> _Note: this issue was identified against the code base at commit `96893890513d5e7e8a4c2241420ddac2d94594cf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListRecords

## Description
Two net-new root causes in the emulator's ListRecords pagination-token handling.

Cause 1 of 2: the refusal message for a NextToken the service never issued is the emulator's own wording, not AWS's.
AWS: Smithy models ListRecords$NextToken (ListRecordsNextToken, length 1..8192) and declares ValidationError as a client error with httpError 400 on this operation. The model does not fix the message text, so for the text this is recorded behavior only: AWS refuses a token it did not issue with HTTP 400, Error Code "ValidationError" and Message "Invalid pagination token". That message is entirely FIXED text — it quotes nothing from the request (notably, it does NOT echo the token, the operation name or the group), so there are no variable parts. The legal neighbour in both checks shows a token the service DID issue must still resume the listing without repeating the first page, and that the refusal must leave the group's records listable.
Emulator: it answers Code "ValidationError" with Message "NextToken is not a ListRecords pagination token" — the literal string at services/service-com-amazonaws-sagemakerfeaturestoreruntime-amazonsagemakerfeaturestoreruntime/src/ops/list_records.rs:179. The code and status are right; the message is not AWS's.
Why: the message is part of the contract. A client (or a test, or a log-matching alarm) that matches on "Invalid pagination token" to distinguish a stale/forged cursor from any other ValidationError does not match, so the refusal is reclassified as an unknown validation failure and either retried blindly or surfaced as an unexpected error.
Traces: sagemaker-featurestore-runtime.list-records-invalid-next-token-rejected, sagemaker-featurestore-runtime.list-records-unissued-next-token-rejected.

Cause 2 of 2: a NextToken issued for one feature group is accepted when replayed against a DIFFERENT feature group.
AWS: recorded behavior only for the rule; the token is scoped to the request that issued it, so sending group A's continuation token with FeatureGroupName=B is refused with HTTP 400, Code "ValidationError", Message "Invalid pagination token" (same fixed text as Cause 1, no variable parts). The positive control shows the same token on its OWN group still resumes the listing with exactly the records the first page did not return.
Emulator: the cross-group request is answered HTTP 200 and lists the other group's record identifiers from the decoded cursor. In list_records.rs the handler decodes the token with PageCursor::decode (line 176-181) and never checks it against the resolved feature group, so any structurally valid cursor is honoured for any group.
Why: a continuation token becomes a cross-group read primitive. A client that reuses or mixes up a token — or a paging loop that carries a token across a group switch — silently enumerates the wrong feature group's record identifiers and believes it enumerated the one it named, instead of being told its token is invalid. That is a correctness and isolation divergence, not just a message one.
Fix: ListRecords' pagination-token handling — the handler in src/ops/list_records.rs together with the cursor codec in crate::state::page_cursor: bind the issuing feature group (and anything else that scopes the page) into the encoded cursor and reject a decoded cursor whose scope does not match the request's resolved group, refusing with the modelled ValidationError (400) and AWS's message text; use that same text for a cursor that fails to decode at all.

## Evidence
Cause 1 — list-records-invalid-next-token-rejected:
  Request: ListRecords(FeatureGroupName=G, NextToken="not-a-real-token")
  Expected: ("ValidationError", "Invalid pagination token")
  Actual:   ("ValidationError", "NextToken is not a ListRecords pagination token")

Cause 1 — list-records-unissued-next-token-rejected:
  Request: ListRecords(FeatureGroupName=G, MaxResults=1, NextToken="eyJjdXJzb3IiOiJmb3JnZWQifQ==") (base64 JSON the service never handed out)
  Expected: ("ValidationError", "Invalid pagination token")
  Actual:   ("ValidationError", "NextToken is not a ListRecords pagination token")
  Source: services/service-com-amazonaws-sagemakerfeaturestoreruntime-amazonsagemakerfeaturestoreruntime/src/ops/list_records.rs:179 — validation("NextToken is not a ListRecords pagination token")

Cause 2 — list-records-next-token-from-other-group-rejected:
  Setup: group SRC holds sku-100/sku-200/sku-300, group OTHER holds sku-900/sku-901/sku-902; token = ListRecords(SRC, MaxResults=1).NextToken
  Request: ListRecords(FeatureGroupName=OTHER, MaxResults=100, NextToken=<SRC's token>)
  Expected: ClientError ("ValidationError", "Invalid pagination token")
  Actual:   HTTP 200, {'RecordIdentifiers': ['sku-900', 'sku-901', 'sku-902']} — the foreign token was honoured
  Source: src/ops/list_records.rs:176-181 decodes the cursor with PageCursor::decode and never validates it against the resolved feature group

## Steps to Reproduce
1. CreateFeatureGroup SRC and OTHER, each RecordIdentifierFeatureName=sku, EventTimeFeatureName=observed, FeatureDefinitions sku(String)/observed(Fractional)/stock(Integral), OnlineStoreConfig.EnableOnlineStore=true; wait for both to reach FeatureGroupStatus=Created.
2. PutRecord sku-100/sku-200/sku-300 into SRC and sku-900/sku-901/sku-902 into OTHER; confirm an unpaged ListRecords on each returns its own three identifiers.
3. Cause 1: call ListRecords(FeatureGroupName=SRC, NextToken="not-a-real-token"). Expect HTTP 400 Code "ValidationError" Message "Invalid pagination token"; the emulator answers Message "NextToken is not a ListRecords pagination token". Repeat with the base64 token "eyJjdXJzb3IiOiJmb3JnZWQifQ==" for the same result.
4. Cause 2: token = ListRecords(FeatureGroupName=SRC, MaxResults=1)["NextToken"], then call ListRecords(FeatureGroupName=OTHER, MaxResults=100, NextToken=token). Expect HTTP 400 "ValidationError" "Invalid pagination token"; the emulator returns HTTP 200 with OTHER's identifiers.
5. Positive control (must keep working): ListRecords(FeatureGroupName=SRC, MaxResults=100, NextToken=token) resumes SRC's listing with the records the first page did not return, and after the refusals every record of each group is still listable.
