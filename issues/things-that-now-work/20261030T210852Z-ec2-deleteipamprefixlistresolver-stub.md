Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-ipam-consolidated-delete-and-deprovision-lifecycle
Issue-Step: task#32
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-ipam-consolidated-delete-and-deprovision-lifecycle
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|stub:DeleteIpamPrefixListResolver
Issue-Commit: 57b303ffac73a2f2fe8b11084ee3a9c28d52eaaf

# Implement ec2 DeleteIpamPrefixListResolver: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `57b303ffac73a2f2fe8b11084ee3a9c28d52eaaf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteIpamPrefixListResolver

## Description
`DeleteIpamPrefixListResolver` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-ec2-amazonec2/src/ops/delete_ipam_prefix_list_resolver.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `DeleteIpamPrefixListResolver` call, so each one unblocks only when `DeleteIpamPrefixListResolver` works: `ec2-ipam-consolidated-delete-and-deprovision-lifecycle#task#32`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-ec2-amazonec2/src/ops/delete_ipam_prefix_list_resolver.rs`. Replaying the corpus, the emulator refused the `DeleteIpamPrefixListResolver` call: status: expected 200, got 501
$.DeleteIpamPrefixListResolverResponse: missing (expected {'requestId': '<shaped-not-asserted>', 'ipamPrefixListResolver': {'addressFamily': 'ipv4', 'ipamArn': 'arn:aws:ec2::000000000000:ipam/ipam-00000000000000025', 'ipamPrefixListResolverArn': 'arn:aws:ec2::000000000000:ipam-prefix-list-resolver/ipam-plr-0000000000000002c', 'ipamPrefixListResolverId': 'ipam-plr-0000000000000002c', 'ipamRegion': 'us-west-2', 'lastVersionCreationStatus': 'success', 'lastVersionCreationStatusMessage': 'Successfully created version 1.', 'ownerId': '000000000000', 'state': 'delete-in-progress'}}, got None)

## Steps to Reproduce
Boot the ec2 emulator
Call DeleteIpamPrefixListResolver and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
