Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ssm.create-ops-item-operational-data-reserved-key-prefix-rejected ssm.opsitem-operational-data-key-under-the-reserved-aws-prefix-rejected ssm.create-ops-item-planned-time-on-issue-type-rejected ssm.opsitem-severity-outside-one-to-four-rejected ssm.ops-item-summary-agrees-with-get-ops-item-across-update ssm.create-association-schedule-offset-without-cron-interval-only-rejected ssm.create-patch-baseline-client-token-reused-with-other-description-rejected ssm.delete-inventory-client-token-reused-with-other-option-rejected ssm.describe-available-patches-page-size-below-one-rejected ssm.list-documents-item-agrees-with-describe-document-across-default-version ssm.put-compliance-items-refuses-oversized-and-over-count-writes ssm.ssm-parameter-policy-with-an-invalid-attribute-value-rejected
Issue-Shape-Id: com.amazonaws.ssm#AmazonSSM
Issue-Key: ssm.create-ops-item-operational-data-reserved-key-prefix-rejected|run#12|AssertionError|assert (code, message) == ("OpsItemInvalidParameterException", expected), (
Issue-Key: ssm.opsitem-operational-data-key-under-the-reserved-aws-prefix-rejected|run#7|AssertionError|assert got == ("OpsItemInvalidParameterException", _FORBIDDEN), (
Issue-Key: ssm.create-ops-item-planned-time-on-issue-type-rejected|_refusal#10|AssertionError|raise AssertionError(f"expected CreateOpsItem to be refused, but it succeeded: {result!r}")
Issue-Key: ssm.opsitem-severity-outside-one-to-four-rejected|_assert_severity_refusal#7|AssertionError|assert msg.startswith(_HEAD) and msg.endswith(_TAIL), f"{label}: message was {msg!r}"
Issue-Key: ssm.ops-item-summary-agrees-with-get-ops-item-across-update|_compare#11|AssertionError|assert row["OperationalData"] == searchable, (
Issue-Key: ssm.create-association-schedule-offset-without-cron-interval-only-rejected|_refusal#16|AssertionError|raise AssertionError(f"expected CreateAssociation to be refused, but it succeeded: {result!r}")
Issue-Key: ssm.create-patch-baseline-client-token-reused-with-other-description-rejected|run#16|AssertionError|assert got == ("IdempotentParameterMismatch", _MISMATCH_MESSAGE), (
Issue-Key: ssm.delete-inventory-client-token-reused-with-other-option-rejected|_refusal#10|AssertionError|raise AssertionError(f"expected a refusal, but the call succeeded: {result!r}")
Issue-Key: ssm.describe-available-patches-page-size-below-one-rejected|run#4|AssertionError|assert len(ids) == 100, f"a MaxResults of 100 returned {len(ids)} patches, expected 100"
Issue-Key: ssm.list-documents-item-agrees-with-describe-document-across-default-version|run#22|AssertionError|assert (stamps["listed"], stamps["described"]) == (stamps["version 1"], stamps["version 2"]), (
Issue-Key: ssm.put-compliance-items-refuses-oversized-and-over-count-writes|_error#11|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: ssm.ssm-parameter-policy-with-an-invalid-attribute-value-rejected|_expect_error#10|AssertionError|raise AssertionError(f"expected {(code, message)!r}, but the call succeeded: {result!r}")
Issue-Commit: a24aa969c7ede1b0812774a0cc80ce835cb01596

# ssm: 9 causes — OpsItem refusal messages and planned-time/severity validation, DescribeOpsItems summary leaks String data, idempotency tokens unenforced, association ScheduleOffset accepted, patch catalog too small, document CreatedDate drift, PutComplianceItems and parameter-policy validation missing

> _Note: this issue was identified against the code base at commit `a24aa969c7ede1b0812774a0cc80ce835cb01596`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateOpsItem

## Description
Nine net-new causes across OpsItems, associations, compliance, patching, documents and parameter policies.

Cause 1 of 9: the reserved operational-data-key refusal message is emulator-authored.
AWS: OperationalData keys under a reserved prefix are refused OpsItemInvalidParameterException with `One or more Operational data keys includes the following forbidden strings: '/amazon', '/aws', '/amzn', '/ssm', 'amazon', 'aws', 'ssm', or 'amzn'.` followed by AWS's service/status/code/request-id suffix — the fixed text is the sentence and the prefix list; only the request id varies.
Emulator: `Operational data keys can't begin with the following: amazon, aws, amzn, ssm, /amazon, /aws, /amzn, /ssm`.
Why: clients matching AWS's text do not recognise the refusal.
Fix: the OperationalData validator shared by ops/create_ops_item.rs and ops/update_ops_item.rs.
Traces: ssm.create-ops-item-operational-data-reserved-key-prefix-rejected, ssm.opsitem-operational-data-key-under-the-reserved-aws-prefix-rejected.

Cause 2 of 9: CreateOpsItem accepts PlannedStartTime/PlannedEndTime on an OpsItemType that does not take them.
AWS: planned times are only valid for the scheduled-change types; on an issue-type OpsItem the request is refused (HTTP 400, OpsItemInvalidParameterException).
Emulator: returns 200 with OpsItemId/OpsItemArn and stores the item.
Why: an OpsItem AWS would reject exists, with fields AWS never stores for that type.
Fix: the OpsItemType/planned-time cross-check in ops/create_ops_item.rs.
Traces: ssm.create-ops-item-planned-time-on-issue-type-rejected.

Cause 3 of 9: the Severity refusal message is emulator-authored.
AWS: an out-of-range Severity is refused with AWS's wording for that class; the submitted value and the 1-4 range are the variable parts.
Emulator: `Severity 0 isn't one of the severity options, which range from 1 to 4`.
Why: the refusal text diverges from AWS for the same request.
Fix: the Severity validator in ops/create_ops_item.rs / ops/update_ops_item.rs.
Traces: ssm.opsitem-severity-outside-one-to-four-rejected.

Cause 4 of 9: the DescribeOpsItems summary carries non-searchable OperationalData.
AWS: an OpsItemSummary's OperationalData holds only the entries whose Type is SearchableString; String entries are returned by GetOpsItem alone.
Emulator: the summary returns both — `{'/sim2real/searchable': {...SearchableString}, 'plainNote': {...'String'}}`.
Why: list responses leak data AWS withholds and are larger/different from AWS's, breaking clients that treat the summary as the searchable projection.
Fix: the OpsItemSummary serializer in ops/describe_ops_items.rs.
Traces: ssm.ops-item-summary-agrees-with-get-ops-item-across-update.

Cause 5 of 9: CreateAssociation accepts a ScheduleOffset with a schedule that cannot carry one.
AWS: ScheduleOffset is only valid with a cron/rate schedule used with ApplyOnlyAtCronInterval; with an interval-only schedule and ApplyOnlyAtCronInterval false the request is refused (HTTP 400).
Emulator: returns 200 with the AssociationDescription (ScheduleOffset 2, ApplyOnlyAtCronInterval False).
Why: an association AWS rejects is created and will schedule differently from AWS.
Fix: the schedule/offset cross-validation in ops/create_association.rs.
Traces: ssm.create-association-schedule-offset-without-cron-interval-only-rejected.

Cause 6 of 9: idempotency-token reuse is mis-messaged on CreatePatchBaseline and unenforced on DeleteInventory.
AWS: reusing a ClientToken with a different request body is refused — for CreatePatchBaseline with AWS's IdempotentParameterMismatch wording, and for DeleteInventory with a refusal rather than a second deletion.
Emulator: CreatePatchBaseline answers `('IdempotentParameterMismatch', "The parameters don't match the original call to the API with the same idempotency token.")` (emulator-authored text), and DeleteInventory with a reused token and a different option answers 200 with a fresh DeletionId and DeletionSummary.
Why: a retried call with changed parameters is silently honoured, so a client's idempotency contract does not hold.
Fix: the ClientToken store/compare used by ops/create_patch_baseline.rs and ops/delete_inventory.rs.
Traces: ssm.create-patch-baseline-client-token-reused-with-other-description-rejected, ssm.delete-inventory-client-token-reused-with-other-option-rejected.

Cause 7 of 9: the available-patch catalog cannot fill a page.
AWS: Smithy DescribeAvailablePatches MaxResults @range 1..100 and the operation is paginated; with MaxResults 100 AWS returns a full page of 100 patches for a product filter.
Emulator: MaxResults 100 returns 12 patches — the catalog holds only a dozen entries, so page-size behaviour (and anything derived from a full page) cannot be reproduced.
Why: callers that page the patch catalog see a result set AWS would never return, and page-size bounds are untestable.
Fix: the patch catalog seed/data source behind ops/describe_available_patches.rs.
Traces: ssm.describe-available-patches-page-size-below-one-rejected.

Cause 8 of 9: a document's CreatedDate changes with its versions.
AWS: CreatedDate is the document's creation instant and is the same in ListDocuments, DescribeDocument and every DescribeDocument(DocumentVersion=n) read.
Emulator: the listed/described CreatedDate equals version 2's timestamp while version 1 reports an earlier one, so the four reads disagree.
Why: a client that keys or sorts documents by CreatedDate sees the value move when a version is added.
Fix: the document-version store and the CreatedDate it reports (ops/list_documents.rs, ops/describe_document.rs).
Traces: ssm.list-documents-item-agrees-with-describe-document-across-default-version.

Cause 9 of 9: PutComplianceItems accepts oversized and over-count writes, and PutParameter accepts an invalid parameter-policy attribute value.
AWS: PutComplianceItems refuses a write past the per-call item count / payload bound (HTTP 400), and a parameter policy whose attribute value is not a timestamp is refused `InvalidPolicyAttributeException` — `Invalid timestamp format: <value>.`
Emulator: PutComplianceItems answers an empty 200, and PutParameter with policy `ExpiresAfter`/`Expiration` holding `tomorrow` answers 200 {'Version': 1, 'Tier': 'Advanced'}.
Why: compliance data AWS rejects is stored, and a parameter carries a policy AWS would never accept — the policy then never fires.
Fix: ops/put_compliance_items.rs bounds and the parameter-policy attribute parser used by ops/put_parameter.rs.
Traces: ssm.put-compliance-items-refuses-oversized-and-over-count-writes, ssm.ssm-parameter-policy-with-an-invalid-attribute-value-rejected.

## Evidence
Cause 1: got ('OpsItemInvalidParameterException', "Operational data keys can't begin with the following: amazon, aws, amzn, ssm, /amazon, /aws, /amzn, /ssm"), expected ('OpsItemInvalidParameterException', "One or more Operational data keys includes the following forbidden strings: '/amazon', '/aws', '/amzn', '/ssm', 'amazon', 'aws', 'ssm', or 'amzn'. (Service: AmazonSSM; Status Code: 400; Error Code: OpsItemInvalidParameterException; Request ID: <id>; Proxy: null)").
Cause 2: CreateOpsItem answered 200 {'OpsItemId': 'oi-a5f238c4f1e8', 'OpsItemArn': 'arn:aws:ssm:us-west-2:000000000000:opsitem/oi-a5f238c4f1e8'}.
Cause 3: message was "Severity 0 isn't one of the severity options, which range from 1 to 4".
Cause 4: summary OperationalData {'/sim2real/searchable': {'Value':'disk-latency','Type':'SearchableString'}, 'plainNote': {'Value':'first-pass','Type':'String'}}, expected only the SearchableString entries.
Cause 5: CreateAssociation answered 200 with ScheduleExpression 'cron(30 23 ? * TUE#3 *)', ApplyOnlyAtCronInterval False, ScheduleOffset 2.
Cause 6: got ('IdempotentParameterMismatch', "The parameters don't match the original call to the API with the same idempotency token."); DeleteInventory answered 200 with DeletionId e6d01926-… .
Cause 7: 'a MaxResults of 100 returned 12 patches, expected 100'.
Cause 8: CreatedDate values {'listed': 12:33:56.591, 'described': 12:33:56.591, 'version 1': 12:33:56.039, 'version 2': 12:33:56.591}.
Cause 9: PutComplianceItems answered 200 (content-length 2); expected ('InvalidPolicyAttributeException', 'Invalid timestamp format: tomorrow.'), got 200 {'Version': 1, 'Tier': 'Advanced'}.

## Steps to Reproduce
1. CreateOpsItem/UpdateOpsItem with an OperationalData key 'awsRunbook'; with Severity 0; with PlannedStartTime on an issue-type item.
2. CreateOpsItem with one SearchableString and one String OperationalData entry, then compare DescribeOpsItems summary with GetOpsItem.
3. CreateAssociation with ScheduleExpression cron(...), ApplyOnlyAtCronInterval false and ScheduleOffset 2.
4. CreatePatchBaseline twice with one ClientToken and different Descriptions; DeleteInventory twice with one ClientToken and different options.
5. DescribeAvailablePatches with MaxResults 100.
6. CreateDocument, UpdateDocument to version 2, then read CreatedDate from ListDocuments, DescribeDocument and DescribeDocument for versions 1 and 2.
7. PutComplianceItems with an over-count/oversized item list; PutParameter with Policies containing an Expiration attribute value 'tomorrow'.
