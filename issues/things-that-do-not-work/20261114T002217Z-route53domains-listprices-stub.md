Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: route53domains.unsupported-tld-rejected-consistently-across-surfaces
Issue-Shape-Id: com.amazonaws.route53domains#Route53Domains_v20140515
Issue-Key: route53domains.unsupported-tld-rejected-consistently-across-surfaces|run#11|AssertionError|assert code == "UnsupportedTLD", f"ListPrices: expected UnsupportedTLD, got {code}: {message!r}"
Issue-Commit: 2d7f7670df77ecd4cdc187296b9dd85ee99606ba

# Implement route53domains ListPrices: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `2d7f7670df77ecd4cdc187296b9dd85ee99606ba`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListPrices

## Description
`ListPrices` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-route53domains-route53domains-v20140515/src/ops/list_prices.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `ListPrices` call, so each one unblocks only when `ListPrices` works: `route53domains.unsupported-tld-rejected-consistently-across-surfaces`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-route53domains-route53domains-v20140515/src/ops/list_prices.rs`. Replaying the corpus, the emulator refused the `ListPrices` call: File "run.py", line 445, in <lambda>
    body = calls.in_phase(calls.BODY, lambda: fn(ctx))
                                              ^^^^^^^
  File "check_route53domains_unsupported_tld_rejected_consistently_across_surfaces.py", line 52, in run
    assert code == "UnsupportedTLD", f"ListPrices: expected UnsupportedTLD, got {code}: {message!r}"
           ^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: ListPrices: expected UnsupportedTLD, got NotImplemented: 'operation ListPrices is scaffolded but not implemented in this emulator'

## Steps to Reproduce
Boot the route53domains emulator
Call ListPrices and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
