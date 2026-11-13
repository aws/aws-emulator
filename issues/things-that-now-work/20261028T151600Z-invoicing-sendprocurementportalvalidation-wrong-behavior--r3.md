Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: 2b443a1d32fe9865f4d2b32010552c8e62eefe3f

# The validation code's clock mixing is annihilated by its own arithmetic, so every issued code is the bare sequence counter ("000001", "000002", …)

_Filed: 2026-10-28T15:16:00Z_
_Operation: SendProcurementPortalValidation | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `2b443a1d32fe9865f4d2b32010552c8e62eefe3f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ComAmazonawsInvoicingInvoicingState::issue_portal_validation_code` — the helper `SendProcurementPortalValidation` calls to mint the outstanding code — states that the code is "Minted from a monotonic counter mixed with the nanosecond clock". The mix is arithmetically inert: the only clock term is `now().subsec_nanos()`, and `crate::state::now()` is deliberately built with MILLISECOND precision (`since_epoch.subsec_millis() * 1_000_000`), so `subsec_nanos()` is always an exact multiple of 1_000_000. That value is multiplied by 1_000_003 — and 1_000_000 * 1_000_003 is itself a multiple of 1_000_000 — so the whole clock term vanishes under the final `% 1_000_000`. The expression reduces exactly to `next_portal_validation_code % 1_000_000`.

The consequence is that the codes this service issues are a fully predictable, run-independent sequence: the first code a fresh emulator ever issues is `000001`, the second `000002`, and so on. `VerifyProcurementPortalValidation` is written to gate on the code the service issued precisely so that "completing the validation would otherwise assert a connectivity check that never happened" — but with a deterministic sequence a caller that never received anything out of band completes the validation by submitting `000001` straight after the first send. The stated mechanism (mixing the clock in) is dead code that contributes nothing.

This is distinct from the two FIXED issues on this operation: the handler now does call the minting helper and does record the code; the defect here is inside the mint itself, which has never produced anything but the counter.

## Evidence

The mint and its own doc — services/service-com-amazonaws-invoicing-invoicing/src/state.rs:422-441:
```
    /// A procurement-portal validation code: six decimal digits, the form the
    /// portal-validation flow's `Code` is quoted in.
    ///
    /// Minted from a monotonic counter mixed with the nanosecond clock, so two
    /// validations issued in one run do not share a code. ...
    pub fn issue_portal_validation_code(&mut self) -> String {
        self.next_portal_validation_code = self.next_portal_validation_code.wrapping_add(1);
        let mixed = u64::from(now().subsec_nanos())
            .wrapping_mul(1_000_003)
            .wrapping_add(self.next_portal_validation_code);
        format!("{:06}", mixed % 1_000_000)
    }
```

The clock it mixes in, in the same module — state.rs:1063-1077:
```
/// Now, as the timestamp an invoice unit records in `LastModified`.
///
/// Built from whole seconds and MILLISECONDS rather than through an `f64`, so two writes
/// a few milliseconds apart are ordered exactly by [`instant_key`] ...
pub fn now() -> DateTime {
    ...
    DateTime::from_secs_and_nanos(
        i64::try_from(since_epoch.as_secs()).unwrap_or(i64::MAX),
        since_epoch.subsec_millis() * 1_000_000,
    )
}
```
— so `now().subsec_nanos()` ∈ {0, 1_000_000, 2_000_000, …, 999_000_000}. For `nanos = k * 1_000_000`, `nanos * 1_000_003 = k * 1_000_003_000_000 = k * 1_000_000 * 1_000_003`, which is ≡ 0 (mod 1_000_000). Hence `mixed % 1_000_000 == self.next_portal_validation_code % 1_000_000`, and the counter starts from the `Default` 0 (`next_portal_validation_code`, state.rs:148-149) so the first code is `000001`.

The only caller, which puts that code in state as the one a submission must match — src/ops/send_procurement_portal_validation.rs:229 and :251-255:
```
        let code = store.issue_portal_validation_code();
...
        stored.validation = Some(crate::state::IssuedPortalValidation {
            code,
            issued_at,
            verified_at: None,
        });
```

The gate the predictable sequence defeats — src/ops/verify_procurement_portal_validation.rs:260-270:
```
            // A code is accepted only when it is the one this service issued for THIS
            // preference and is still outstanding. Anything else — no send has been
            // made, or a code that does not match the issued one — is refused with the
            // modelled 400, because completing the validation would otherwise assert a
            // connectivity check that never happened.
```

Test coverage: tests/all/portal_validation_code_issue.rs:202-215 asserts only that an issued code is `CODE_DIGITS` decimal digits and that "each send issues a code of its own" (distinctness); tests/all/refused_procurement_portal_writes.rs and tests/all/retired_units_and_portal_validation.rs read the outstanding code out of state rather than predicting it. No test anywhere in tests/all/ asserts a particular code value, so nothing pins the counter-only output.

## Steps to Reproduce

1. Start the emulator (a fresh process, so `next_portal_validation_code` is at its `Default` 0).
2. POST / with `X-Amz-Target: Invoicing.CreateProcurementPortalPreference` and body `{"ProcurementPortalName":"ZIP","BuyerDomain":"NetworkID","BuyerIdentifier":"AN01","SupplierDomain":"NetworkID","SupplierIdentifier":"AN02","EinvoiceDeliveryEnabled":true,"PurchaseOrderRetrievalEnabled":true,"Contacts":[{"Name":"Ops","Email":"ops@example.com"}]}`; note the returned `ProcurementPortalPreferenceArn`.
3. POST / with `X-Amz-Target: Invoicing.SendProcurementPortalValidation` and body `{"ProcurementPortalPreferenceArn":"<arn>"}` — observe HTTP 200.
4. POST / with `X-Amz-Target: Invoicing.VerifyProcurementPortalValidation` and body `{"ProcurementPortalPreferenceArn":"<arn>","Code":"000001"}` without ever having read the issued code: observe HTTP 200, the validation completes.
5. Repeat steps 2-4 for a second preference and submit `"000002"`: it completes too — the code is the sequence counter, and the `now().subsec_nanos()` term in `issue_portal_validation_code` contributes nothing to it.
