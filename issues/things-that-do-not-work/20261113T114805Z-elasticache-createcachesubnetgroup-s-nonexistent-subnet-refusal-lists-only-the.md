Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elasticache.subnet-group-replace-semantics-and-cross-vpc-rejected
Issue-Shape-Id: com.amazonaws.elasticache#AmazonElastiCacheV9
Issue-Key: elasticache.subnet-group-replace-semantics-and-cross-vpc-rejected|run#40|AssertionError|assert absent_message == (ab if absent_message.startswith(f"Some input subnets in :[{subnet_b},") else ba), (
Issue-Commit: 95f1e080048228d599071943e64424bac0fa7206

# CreateCacheSubnetGroup's nonexistent-subnet refusal lists only the unresolved subnet ids instead of the whole requested SubnetIds list

> _Note: this issue was identified against the code base at commit `95f1e080048228d599071943e64424bac0fa7206`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCacheSubnetGroup

## Description
One net-new root cause in the elasticache emulator. The suite marked this check neither ALREADY FILED nor OWNED.

Cause 1 of 1: the `InvalidParameterValue` message for a SubnetIds entry that names no subnet of the account enumerates only the ids that failed to resolve, not the request's whole subnet list.
  AWS: Smithy `CreateCacheSubnetGroup` (com.amazonaws.elasticache#AmazonElastiCacheV9), input member `SubnetIds` (SubnetIdentifierList, required). For an id that names no subnet of the account ElastiCache answers `InvalidParameterValueException` / awsQuery code `InvalidParameterValue`, HTTP 400 — this operation's own modeled `errors` list omits it, and the emulator's own handler doc records it as widened in via `extra_errors` for exactly this case. Recorded behavior for the message: AWS words it `Some input subnets in :[<id>, <id>, ...] are invalid.` The FIXED text is `Some input subnets in :[`, the `, ` separator, and `] are invalid.`; the VARIABLE part is the ENTIRE `SubnetIds` list as sent in the request (every id, valid ones included), in either order — not just the subset AWS could not resolve. The check accepts either ordering of the full list and nothing else.
  Emulator: refuses with the right code but names only the unresolvable ids: `Some input subnets in :[subnet-0123456789abcdef0] are invalid.` for a request whose SubnetIds held one existing subnet plus one nonexistent id, where AWS answers `Some input subnets in :[<existing subnet id>, subnet-0123456789abcdef0] are invalid.` (either order).
  Why: the message is the only place this refusal reports WHICH subnets the service considered, and AWS reports the submitted list. A client (or operator log) that parses the bracketed list to echo back what it asked for, diffs it against its request, or counts the ids, sees a shorter list than it sent and concludes the service only received/evaluated part of its SubnetIds — on AWS the list always matches the request. Message-matching clients never match at all.
  Fix: the nonexistent-subnet refusal path of CreateCacheSubnetGroup's handler — services/service-com-amazonaws-elasticache-amazonelasticachev9/src/ops/create_cache_subnet_group.rs lines 287-304 pass only `unresolved` (the Err payload of `crate::state::ec2_subnets::resolve_requested_subnets`) into `crate::state::invalid_subnets_message`, where the requested list (`requested`, built at line 287) is what AWS enumerates. The wording helper itself (`crate::state::invalid_subnets_message` in src/state.rs) produces the correct fixed text; only the argument is wrong. ModifyCacheSubnetGroup's equivalent path should be kept consistent with whatever is chosen here.
  Traces: elasticache.subnet-group-replace-semantics-and-cross-vpc-rejected.

Note: the sibling cross-VPC refusals in the same check (`Subnets <a> and <b> are not in the same VPC.`, InvalidSubnet) passed, so this is scoped to the unresolved-subnet message only. Open issue 20261109T151249Z-elasticache-refusal-messages-are-the-emulator-s-own--r2.md covers other elasticache refusal-wording causes; it does not carry this check or this message.

## Evidence
elasticache.subnet-group-replace-semantics-and-cross-vpc-rejected (mismatch):

check_elasticache_subnet_group_replace_semantics_and_cross_vpc_rejected.py line 288:
AssertionError: the nonexistent-subnet refusal: expected 'Some input subnets in :[subnet-0000000000000009a, subnet-0123456789abcdef0] are invalid.' or 'Some input subnets in :[subnet-0123456789abcdef0, subnet-0000000000000009a] are invalid.', got 'Some input subnets in :[subnet-0123456789abcdef0] are invalid.'

(subnet-0000000000000009a is the existing account subnet the request also named; subnet-0123456789abcdef0 is the fixed placeholder id that names no subnet. The ids are per-run values filling the template; the requirement is that the bracketed list is the request's whole SubnetIds list.)

The error code was correct (InvalidParameterValue, distinct from the InvalidSubnet used for the cross-VPC case) and the rejected group was correctly not materialised; only the message's subnet enumeration diverges.

## Steps to Reproduce
1. Create/choose an existing subnet in the account, e.g. via EC2 DescribeSubnets; call its id SUBNET_OK.
2. ElastiCache CreateCacheSubnetGroup(CacheSubnetGroupName='<new name>', CacheSubnetGroupDescription='x', SubnetIds=[SUBNET_OK, 'subnet-0123456789abcdef0']) where the second id names no subnet of the account.
3. Inspect Error.Code and Error.Message: code is InvalidParameterValue (correct); AWS's message is 'Some input subnets in :[SUBNET_OK, subnet-0123456789abcdef0] are invalid.' (the two ids in either order), the emulator answers 'Some input subnets in :[subnet-0123456789abcdef0] are invalid.'
