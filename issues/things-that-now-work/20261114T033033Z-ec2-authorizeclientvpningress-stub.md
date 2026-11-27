Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2.op-contract-revoke-client-vpn-ingress
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|AuthorizeClientVpnIngress|NotImplemented
Issue-Commit: 1d457c6418d8d9996f08d1176214615fe9686e31

# Implement ec2 AuthorizeClientVpnIngress: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `1d457c6418d8d9996f08d1176214615fe9686e31`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AuthorizeClientVpnIngress

## Description
`AuthorizeClientVpnIngress` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-ec2-amazonec2/src/ops/authorize_client_vpn_ingress.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `AuthorizeClientVpnIngress` call, so each one unblocks only when `AuthorizeClientVpnIngress` works: `ec2.op-contract-revoke-client-vpn-ingress`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-ec2-amazonec2/src/ops/authorize_client_vpn_ingress.rs`. Replaying the corpus, the emulator refused the `AuthorizeClientVpnIngress` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
           ^^^^^^^^^^^^^^^^^^^^^
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the AuthorizeClientVpnIngress operation: operation AuthorizeClientVpnIngress is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the ec2 emulator
Call AuthorizeClientVpnIngress and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
