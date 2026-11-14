Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 679439ce7e13db169223db2ff82d790a290f816f

# A change set started without a ChangeSetName reports no name at all, though the model says one is set by default

_Filed: 2026-11-14T03:14:27Z_
_Operation: DescribeChangeSet | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `679439ce7e13db169223db2ff82d790a290f816f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`DescribeChangeSetResponse$ChangeSetName` is documented as "The optional name provided in the StartChangeSet request. If you do not provide a name, one is set by default." The emulator never sets that default: `StartChangeSet` stores `name: input.change_set_name` verbatim (so `None` when the request named none), and `DescribeChangeSet` reports `change_set_name: record.name.clone()`, i.e. the member is omitted from the response body entirely for every change set started without a name.

The same stored `None` also reaches the `ListChangeSets` projection (`ChangeSetSummaryListItem$ChangeSetName` is `self.name.clone()` in `crate::state`) and the `ChangeSetName` filter of `ListChangeSets`, whose `matches_filter` arm requires `self.name` to be `Some` — so an unnamed change set carries no name anywhere, cannot be reported by name and cannot be selected by name.

The crate applies exactly this rule for the SIBLING member governed by the sibling sentence of the same doc paragraph: `DescribeChangeSetResponse$Intent` ("If you do not provide an intent, APPLY is set by default") IS defaulted in the handler (`intent = input.intent.clone().unwrap_or_else(|| crate::state::INTENT_APPLY.to_string())`), while the name sentence is left unimplemented even though the handler's own doc block quotes it.

## Evidence

Smithy model (smithy_query query="doc" shape="DescribeChangeSetResponse" member="ChangeSetName"): "<p>The optional name provided in the <code>StartChangeSet</code> request. If you do not\n            provide a name, one is set by default.</p>" — the same sentence is carried in the handler's own modeled-contract header: `services/service-com-amazonaws-marketplacecatalog-awsmpseymour/src/ops/describe_change_set.rs:62-64`: "//! * `ChangeSetName` (ChangeSetName, len 1..100, pattern ^[\w\s+=.:@-]+$) — The optional name //! provided in the StartChangeSet request. If you do not provide a name, one is set by //! default."

Code that does the opposite:
- `services/service-com-amazonaws-marketplacecatalog-awsmpseymour/src/ops/start_change_set.rs:692`: "            name: input.change_set_name," (no default applied; the very next line, 693, is "            intent," — the value defaulted at start_change_set.rs:393-396 "let intent = input.intent.clone().unwrap_or_else(|| crate::state::INTENT_APPLY.to_string());" for the sibling sentence of the same paragraph).
- `services/service-com-amazonaws-marketplacecatalog-awsmpseymour/src/ops/describe_change_set.rs:208`: "            change_set_name: record.name.clone()," → member absent from the 200 body.
- `services/service-com-amazonaws-marketplacecatalog-awsmpseymour/src/state.rs:1024`: "            change_set_name: self.name.clone()," (same hole in the `ListChangeSets` summary).
- `services/service-com-amazonaws-marketplacecatalog-awsmpseymour/src/state.rs:1053-1056`: "            \"ChangeSetName\" => self\n                .name\n                .as_deref()\n                .is_some_and(|held| values.iter().any(|value| value == held))," — an unnamed change set matches no `ChangeSetName` filter value.

Test coverage that pins today's behaviour (without addressing the documented default): `services/service-com-amazonaws-marketplacecatalog-awsmpseymour/tests/all/change_set_lifecycle.rs:403-408` asserts `running.text("/ChangeSetName") == request["ChangeSetName"].as_str().unwrap_or_default()` — "the described name must be the submitted one" — and the case table at lines 295-310 ("one change, details as a document") submits NO `ChangeSetName`, so that row asserts the described name is the empty string, i.e. the member is absent.

## Steps to Reproduce

1. POST /StartChangeSet with body {"Catalog":"AWSMarketplace","ChangeSet":[{"ChangeType":"CreateProduct","Entity":{"Type":"AmiProduct@1.0"},"Details":"{\"ProductTitle\":\"nameless\"}"}]} — note NO ChangeSetName member. Record the returned ChangeSetId.
2. GET /DescribeChangeSet?catalog=AWSMarketplace&changeSetId=<id> — the 200 body carries no `ChangeSetName` member at all; the model says "If you do not provide a name, one is set by default", so a name is expected.
3. POST /ListChangeSets with {"Catalog":"AWSMarketplace"} — the `ChangeSetSummaryListItem` for that change set likewise carries no `ChangeSetName`.
4. POST /ListChangeSets with a {"Name":"ChangeSetName","ValueList":[<the default name>]} filter — nothing can select the change set, because no name was ever stored.
