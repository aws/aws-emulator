Issue-Severity: HIGH
Issue-Gap-Class: stub
Issue-Commit: 723e24d0ff9083736505f1fa49c337be40d1d9c1

# Implement billing ListContractualInvestmentSummaries: unimplemented scaffold stub

_Filed: 2026-11-27T19:40:30Z_
_Operation: ListContractualInvestmentSummaries | Class: stub_

> _Note: this issue was identified against the code base at commit `723e24d0ff9083736505f1fa49c337be40d1d9c1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListContractualInvestmentSummaries` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-billing-awsbilling/src/ops/list_contractual_investment_summaries.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

## Evidence

The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-billing-awsbilling/src/ops/list_contractual_investment_summaries.rs`.

## Steps to Reproduce

1. Boot the billing emulator
2. Call ListContractualInvestmentSummaries and observe HTTP 501 with no state persisted
