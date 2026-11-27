Issue-Source: trace-sweep-divergence
Issue-Trace: cloudformation-aorigin-physical-id-candidate-walk
Issue-Step: task#10
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cloudformation-aorigin-physical-id-candidate-walk
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: glue|GetTags|2xx->4xx|$.Tags
Issue-Key: glue|GetTags|2xx->4xx|status
Issue-Commit: ead21e031399bcf7353b54db1d6238d681197cdf

# Glue GetTags refuses an existing untagged resource with 400 instead of 200 and an empty Tags map

> _Note: this issue was identified against the code base at commit `ead21e031399bcf7353b54db1d6238d681197cdf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetTags

## Description
One root cause in the Glue tagging read path of service-com-amazonaws-glue-awsglue. The replayed step is reported under service heading 'glue' with NO 'owned_by' owner and NO stub_path, so it is net-new; no existing open issue carries its replay key.

Cause 1 of 1: GetTags on the ARN of a Glue resource that EXISTS and carries no tags is refused with HTTP 400 instead of answering HTTP 200 with an empty Tags map.

AWS: Smithy (services/service-com-amazonaws-glue-awsglue/smithy-model/com-amazonaws-glue-awsglue.json): GetTags input {ResourceArn @required GlueResourceArn, @length 1..10240, @pattern ^arn:aws(-(cn|us-gov|iso(-[bef])?))?:glue:}; output {Tags: TagsMap, @length 0..50}; errors EntityNotFoundException (client, 400), InvalidInputException (client, 400), OperationTimeoutException (client, 400), InternalServiceException (server, 500). The modelled ResourceArn constraint is only the prefix every Glue ARN shares — it restricts no resource TYPE — and the modelled output's Tags map admits length 0. Recorded behavior: GetTags on the ARN of a live Glue resource that was created without any Tags answers HTTP 200 with Tags present and EMPTY ({}), i.e. "this resource has no tags" is a successful read of an empty map, never a refusal. A 400 is reserved for an ARN that resolves to no resource at all (EntityNotFoundException) or is not a Glue resource ARN (InvalidInputException).

Emulator: the same call is answered HTTP 400 and the response carries no Tags member at all (replay detail: "status: expected 200, got 400" and "$.Tags: missing (expected {}, got None)"). The step is part of a CloudFormation physical-id candidate walk, so the resource the ARN names was created moments earlier in the same fixture and is live when the read is made.

Hypothesis (neither the model nor the recording states which of the handler's two refusal branches fires, because the replay reports only the status): the handler services/service-com-amazonaws-glue-awsglue/src/ops/get_tags.rs resolves ResourceArn through crate::state::parse_tagged_resource + ComAmazonawsGlueAwsglueState::resource_tags and has exactly two 400 branches, so the refusal is one of them. Either (a) parse_tagged_resource answered None — its resource-type table TAGGED_RESOURCE_TYPES enumerates only 15 kinds (blueprint, catalog, connection, crawler, customEntityType, database, dataQualityRuleset, job, mlTransform, registry, schema, session, trigger, usageProfile, workflow), so a Glue ARN of any other resource form (a catalog table, a user-defined function, a table version, a partition, a dev endpoint) is declared "not a taggable Glue ARN" and refused InvalidInputException with NO message at all (state::INVALID_RESOURCE_ARN_MESSAGE is a hard None); or (b) resource_tags answered None because the kind's store lookup is keyed differently from the identity the ARN carries — the kinds that live beside the catalog tree are keyed by region (jobs, sessions, triggers) or gated on is_served_account, and database/connection/catalog are looked up under the ARN's account segment as a CatalogId — so a resource this emulator really holds can still miss and earn EntityNotFoundException with the fixed state::messages::TAGGED_RESOURCE_NOT_FOUND sentence.

Why: a client that reads a resource's tags to decide anything — a CloudFormation physical-id walk resolving which candidate ARN a stack resource is, tag-driven inventory, cost allocation, a TagResource/UntagResource round trip — gets a hard 400 where AWS hands back a successful empty map. The two answers are not interchangeable: 200 with {} means "the resource is there and has no tags", while 400 means "the ARN is wrong or the resource is gone", so the caller aborts or retries a walk that AWS completes, and any step downstream of the tag read never runs.

Fix: the GetTags handler's ResourceArn resolution in the glue crate — the operation's own handler plus the shared tagging resolution it delegates to (parse_tagged_resource / TAGGED_RESOURCE_TYPES and the tag_set_of! kind-to-store table behind resource_tags). The resolution must reach the record for every Glue resource form whose ARN AWS answers a tag read for, and must key each kind's lookup by the same identity its create stored it under, so a live resource resolves and its absent tag set is serialized as an empty map on the 200 path the handler already has. Note the related open issue 20261117T183344Z-glue-catalog-surface-7-net-new-causes-gettags.md (behavioral corpus, Cause 1: GetTags refusing database and registry ARNs with an empty-message InvalidInputException) — it covers a different corpus and does NOT carry this fixture step's replay key, so this wire divergence is filed separately; a fixer should check whether the two share the resolution above.

Traces: cloudformation-aorigin-physical-id-candidate-walk#task#10 (service glue, operation GetTags, verdict mismatch, expected 200 -> actual 400).

## Evidence
Wire corpus simparity-wire-aws-v1 replayed against the emulator built from this checkout (emulator_up=true, /__admin/health 200; 3257 steps, 1106 passed, score 34.0%). This cluster is the only divergence in the run with an EMPTY owned_by and an EMPTY stub_path.

Divergence cluster (verbatim):
  service: glue
  operation: GetTags
  fixture_id: cloudformation-aorigin-physical-id-candidate-walk
  step_key: task#10
  verdict: mismatch
  expected_status: 200, actual_status: 400
  affected: ["cloudformation-aorigin-physical-id-candidate-walk#task#10"] (affected_count 1)
  divergence_key: service=glue|GetTags|status=200->400|keys=$.Tags,status
  detail: "status: expected 200, got 400\n$.Tags: missing (expected {}, got None)"

Expected vs actual:
  Expected: HTTP 200, body {"Tags": {}} — Tags PRESENT and empty.
  Actual:   HTTP 400, no Tags member in the body (the handler's modelled client refusal).

Model evidence (services/service-com-amazonaws-glue-awsglue/smithy-model/com-amazonaws-glue-awsglue.json): GetTags {ResourceArn @required GlueResourceArn length 1..10240 pattern ^arn:aws(-(cn|us-gov|iso(-[bef])?))?:glue:} -> {Tags: TagsMap length 0..50}; errors [EntityNotFoundException(400), InternalServiceException(500), InvalidInputException(400), OperationTimeoutException(400)]. The output's Tags map is explicitly allowed to be empty, which is how "no tags" is reported on the success path.

Code evidence for the refusal branches: services/service-com-amazonaws-glue-awsglue/src/ops/get_tags.rs lines 177-183 (parse_tagged_resource None -> InvalidInputException with state::INVALID_RESOURCE_ARN_MESSAGE) and lines 184-201 (region mismatch or resource_tags None -> EntityNotFoundException with state::messages::TAGGED_RESOURCE_NOT_FOUND); the 200 path at lines 202-206 already serializes an empty map correctly. services/service-com-amazonaws-glue-awsglue/src/state/tagged_resources.rs lines 83-99 (TAGGED_RESOURCE_TYPES, 15 kinds only; lines 28-34 document that a catalog table, a table version, a user-defined function, a partition and a dev endpoint are deliberately NOT resolvable), line 250 (INVALID_RESOURCE_ARN_MESSAGE = None, so that refusal carries no prose), and lines 274-356 (the tag_set_of! kind-to-store table, with the per-region jobs/sessions/triggers keys and the is_served_account gate at line 298).

## Steps to Reproduce
1. Clone this checkout and boot the emulator built from it.
2. Replay the wire fixture `cloudformation-aorigin-physical-id-candidate-walk` from trace set simparity-wire-aws-v1 and stop at step `task#10` (service glue, operation GetTags).
3. Observe the response: the emulator answers HTTP 400 with no Tags member; the recording has HTTP 200 with body {"Tags": {}}.
4. Equivalent by hand: create a Glue resource with NO Tags (for the fixture's path, let the CloudFormation stack create it and take the ARN the physical-id walk builds for that resource), then call glue GetTags with that ResourceArn. AWS answers 200 with Tags = {}; the emulator refuses with 400.
5. To tell the two refusal branches apart, read the error code and message off the 400: InvalidInputException with an EMPTY message means parse_tagged_resource did not recognise the ARN's resource-type segment (src/state/tagged_resources.rs TAGGED_RESOURCE_TYPES); EntityNotFoundException with the fixed TAGGED_RESOURCE_NOT_FOUND sentence means the kind was recognised but the store lookup in tag_set_of! missed the live record.
6. Re-replay cloudformation-aorigin-physical-id-candidate-walk#task#10 to confirm the fix: the step must pass with HTTP 200 and Tags present as an empty map.
