Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: e4f45eba1b7e0829bfedb4a437f2a9c9eaec6d3e

# payerAccountFlag aggregation is gated on the requested accountId instead of on the caller, so a payer asking about a family member gets no family aggregate

_Filed: 2026-11-01T16:30:40Z_
_Operation: GetCredits | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `e4f45eba1b7e0829bfedb4a437f2a9c9eaec6d3e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The model conditions whole-family aggregation on WHO IS CALLING: when `payerAccountFlag` is true and the caller is the management account, the response aggregates credits across the entire consolidated billing family; only when the flag is false or omitted is the answer narrowed to "the specified accountId". The handler instead ANDs the flag with `input.account_id == engine::emulator_account_id()`, i.e. it decides the question from a REQUEST MEMBER rather than from the caller. In this crate the caller is always the emulated account (state.rs defines it that way) and that account is always the billing family's management account (`billing_family_management_account_id` is seeded to it), so `payerAccountFlag: true` must always aggregate. As written, a request with `payerAccountFlag: true` that names a family MEMBER in `accountId` silently answers with that one account's credits and no family aggregate at all — the branch the model reserves for `false`/omitted. The crate already owns the predicate the doc describes (`is_billing_family_management_account`, used by UpdateBillingPreferences for the very same "management account of a consolidated billing family" rule); GetCredits never calls it and substitutes an accountId equality test, so the flag's documented meaning (a property of the caller) is silently re-read as a property of the request.

## Evidence

Smithy, GetCreditsRequest$payerAccountFlag (`smithy_query(query="doc", shape="GetCreditsRequest", member="payerAccountFlag")`): "When `true` and the caller is the management account, the response aggregates credits across the entire consolidated billing family. When `false` or omitted, returns only credits for the specified `accountId`." — the two branches are keyed on the CALLER vs. "the specified accountId", so the doc itself distinguishes them.

The handler quotes that rule and then applies a different one — services/service-com-amazonaws-billing-awsbilling/src/ops/get_credits.rs:215-222:
```
        // "When the caller is the management account of a consolidated billing
        // family and payerAccountFlag is true, the response aggregates credits
        // across the entire family. Otherwise, the response includes only credits
        // owned by the account specified in accountId." The emulator's own account
        // is the management account of its family, so a request naming any other
        // account is answered from that account's own credits alone.
        let whole_family = input.payer_account_flag.unwrap_or(false)
            && input.account_id == ::engine::emulator_account_id();
```
The caller is NOT the request's `accountId` in this crate — services/service-com-amazonaws-billing-awsbilling/src/state.rs:914-920:
```
/// The account every caller of this emulator authenticates as: every request
/// is signed with the emulator's own credentials, so the caller's account is
/// [`engine::emulator_account_id`] ...
pub fn caller_account_id() -> &'static str {
    ::engine::emulator_account_id()
}
```
and that caller IS the family's management account — state.rs:620 `billing_family_management_account_id: caller_account_id().to_string(),` with state.rs:722-728: "Whether `account_id` is the management (payer) account of the consolidated billing family — the only identity that may set a sharing preference."

The crate already applies the documented predicate elsewhere for the same phrase — src/ops/update_billing_preferences.rs:291-301:
```
        let caller = crate::state::caller_account_id();
        ...
            if !state.is_billing_family_management_account(caller) {
                return Err(access_denied(format!(
                    "Account {caller} is not the management account of a consolidated billing \
                     family, so it cannot set the sharing preference {}",
```

Test coverage: tests/all/credits.rs:577-589 pins today's reading with the case labels "linked account cannot aggregate the family" (accountId=LINKED_ACCOUNT_ID, flag=true -> only credit "22") and "another member account, flag true" (accountId=UNKNOWN_ACCOUNT_ID, flag=true -> only credit "23"); the stated reason for those rows is the handler's own re-reading of "the caller" as `accountId`, which the model sentence above contradicts. The rows for the management account (credits.rs:569-576) keep passing under the documented rule.

## Steps to Reproduce

1. Boot the emulator and point an awsJson1_0 `billing` client at it.
2. Seed two credits in `ComAmazonawsBillingAwsbillingState::credits` (state.rs:879 `insert_credit`): one owned by 000000000000 (the emulated caller / family management account) and one owned by a family member, e.g. 111122223333 — exactly what tests/all/credits.rs:558-565 does.
3. Call GetCredits(accountId="111122223333", startDate=<30 days ago>, payerAccountFlag=true).
4. Observe only the member account's own credit is returned. The caller is the management account (state.rs:620), so per GetCreditsRequest$payerAccountFlag the response must aggregate credits across the entire consolidated billing family.
5. For contrast, call GetCredits(accountId="000000000000", ..., payerAccountFlag=true) and observe the family aggregate — the only accountId value for which the flag has any effect (get_credits.rs:221-222).
