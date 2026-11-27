Issue-Source: trace-sweep-divergence
Issue-Trace: opensearch-reserved-instance-offering-describe-and-purchase-errors
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: opensearch-reserved-instance-offering-describe-and-purchase-errors
Issue-Shape-Id: com.amazonaws.opensearch#AmazonOpenSearchService
Issue-Key: es|DescribeReservedInstanceOfferings|2xx->2xx|$.NextToken
Issue-Commit: 7f3d8acfd1d18a62a70e68f35eeb77bb7bd3316f

# es DescribeReservedInstanceOfferings omits NextToken, so a truncated offerings page claims to be the whole catalog

> _Note: this issue was identified against the code base at commit `7f3d8acfd1d18a62a70e68f35eeb77bb7bd3316f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeReservedInstanceOfferings

## Description
ONE net-new cause on the es-token reserved-instance read surface (service token `es`, Smithy service com.amazonaws.opensearch#AmazonOpenSearchService).

Cause 1 of 1: DescribeReservedInstanceOfferings answers a page of offerings but sends no NextToken, so the response states that the listing is complete when it is not.
AWS: Smithy com.amazonaws.opensearch#DescribeReservedInstanceOfferings - GET /2021-01-01/opensearch/reservedInstanceOfferings, success 200; input ReservedInstanceOfferingId (@httpQuery offeringId, GUID), MaxResults (@httpQuery maxResults, @default 0, range max 100), NextToken (@httpQuery nextToken); output ReservedInstanceOfferings -> ReservedInstanceOfferingList and NextToken; the operation is @paginated{inputToken NextToken, outputToken NextToken, pageSize MaxResults}. The contract of that trait is that when the selected listing continues past the page just returned, the response carries NextToken, and the caller passes it back to read the next page; NextToken is absent only on the last page. Recorded behavior for this step: HTTP 200 whose body carries the SAME offerings the emulator returns PLUS an opaque continuation token (the recording's value begins 'AAIAAbSrw5EA4FPFRnHYzyMRKbZRq87dZ9XnUvMBW9-...'; the token's bytes are a per-run value, its PRESENCE is the requirement).
Emulator: answers 200 with a body whose only diverging key is $.NextToken - expected a token, got None. The offerings list matches the recorded page exactly, so the emulator cut the listing to the same page and then reported no continuation.
Why: a client paginating this catalog - the standard `while NextToken: ...` loop, or a paginator from any AWS SDK - stops after the first page and treats the offerings it has as the complete set. It therefore never sees the instance types, terms and payment options published past that page, and silently reports or budgets against a partial catalog; against AWS the same code keeps reading. Unlike a value divergence this one cannot be noticed by the client, because a missing NextToken is indistinguishable from a genuinely exhausted listing.
Fix: the pagination step of the DescribeReservedInstanceOfferings handler in the opensearch service crate - whatever bounds the page it returns must emit the matching outputToken whenever rows remain beyond it, and omit NextToken only when the page it just served exhausts the selected listing. Note the history: the resolution of 20261124T122846Z-opensearch-instance-type-and-reserved-instance-catalogs-serve.md (now RESOLVED, so it owns no cluster here) deliberately made an unbounded request return the whole listing with no token; the recording for this step shows a bounded page WITH a token, so the token must be emitted whenever the answer is actually cut.
Traces: opensearch-reserved-instance-offering-describe-and-purchase-errors#task#1 (service es, operation DescribeReservedInstanceOfferings, verdict mismatch, 200 -> 200).

Related open issues that do NOT cover this cluster (the replay reports owned_by empty): 20261126T155222Z-es-instance-type-catalog-ordering-and-service-software.md carries the es ListInstanceTypeDetails / DescribeInstanceTypeLimits / DescribeDomain keys; 20261029T190009Z-es-package-vpc-endpoint-tagging-and-reserved-instance.md carries the es package/VPC-endpoint/tagging refusal keys, not this pagination key.

## Evidence
Replay of corpus simparity-wire-aws-v1 (emulator_up true, 99 fixtures, 492 steps, 365 passed, 127 mismatched, score 74.2%). Cluster: verdict mismatch, service es, stub_path empty, owned_by empty (net-new).

fixture opensearch-reserved-instance-offering-describe-and-purchase-errors, step task#1, operation DescribeReservedInstanceOfferings, expected_status 200, actual_status 200, affected_count 1:
  $.NextToken: missing (expected 'AAIAAbSrw5EA4FPFRnHYzyMRKbZRq87dZ9XnUvMBW9-59bp97Bc1dZdW8tQT4ZUdd674eH-3zfnluI8=', got None)
  divergence_key: service=es|DescribeReservedInstanceOfferings|status=200->200|keys=$.NextToken

Note that $.NextToken is the ONLY diverging key in the cluster: every offering in the body matched the recorded page, so the page boundary itself is right and only the continuation token is missing. The recorded token's bytes are a per-run, opaque value and are not the requirement; the requirement is that a response whose page does not exhaust the selected listing carries a NextToken that, passed back as the nextToken query parameter, returns the following page.

Smithy cited (smithy_query, model_path services/service-com-amazonaws-opensearch-amazonopensearchservice/smithy-model/com-amazonaws-opensearch-amazonopensearchservice.json): DescribeReservedInstanceOfferings GET /2021-01-01/opensearch/reservedInstanceOfferings, code 200, input ReservedInstanceOfferingId/MaxResults(@default 0, range max 100)/NextToken as @httpQuery, output {ReservedInstanceOfferings -> ReservedInstanceOfferingList, NextToken}, @paginated{inputToken NextToken, outputToken NextToken, pageSize MaxResults}; errors ValidationException(400), DisabledOperationException(409), ResourceNotFoundException(409), InternalException(500).

## Steps to Reproduce
1. git_clone, build and boot the emulator from the checkout, then point an es/opensearch client at the data listener.
2. Replay fixture opensearch-reserved-instance-offering-describe-and-purchase-errors step task#1 - GET /2021-01-01/opensearch/reservedInstanceOfferings with the fixture's query parameters (boto3: client('opensearch').describe_reserved_instance_offerings(**the step's parameters)).
3. Confirm HTTP 200 and read the response keys: the body must carry NextToken alongside ReservedInstanceOfferings, because the page returned does not exhaust the offerings catalog. The emulator returns the page with NextToken absent.
4. Drive the standard pagination loop (get_paginator('describe_reserved_instance_offerings')) and count the offerings reached: against the emulator the loop ends after one page; it must keep following the token until the final page, which is the only page allowed to omit NextToken.
5. Re-replay opensearch-reserved-instance-offering-describe-and-purchase-errors#task#1 and confirm it passes before submitting.
