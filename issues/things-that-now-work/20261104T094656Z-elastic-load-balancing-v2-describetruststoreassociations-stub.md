Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elbv2.deploy-trust-store-cfn-association-enforced
Issue-Shape-Id: com.amazonaws.elasticloadbalancingv2#ElasticLoadBalancing_v10
Issue-Key: elastic-load-balancing-v2|DescribeTrustStoreAssociations|NotImplemented
Issue-Commit: 7fd8acd3be042b6d00a1996e0a0d05df0b6f87ae

# Implement elastic-load-balancing-v2 DescribeTrustStoreAssociations: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `7fd8acd3be042b6d00a1996e0a0d05df0b6f87ae`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeTrustStoreAssociations

## Description
`DescribeTrustStoreAssociations` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-elasticloadbalancingv2-elasticloadbalancing-v10/src/ops/describe_trust_store_associations.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `DescribeTrustStoreAssociations` call, so each one unblocks only when `DescribeTrustStoreAssociations` works: `elbv2.deploy-trust-store-cfn-association-enforced`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-elasticloadbalancingv2-elasticloadbalancing-v10/src/ops/describe_trust_store_associations.rs`. Replaying the corpus, the emulator refused the `DescribeTrustStoreAssociations` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
           ^^^^^^^^^^^^^^^^^^^^^
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the DescribeTrustStoreAssociations operation: operation DescribeTrustStoreAssociations is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the elastic-load-balancing-v2 emulator
Call DescribeTrustStoreAssociations and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
