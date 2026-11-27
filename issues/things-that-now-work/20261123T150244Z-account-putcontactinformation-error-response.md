Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: 6c4ddbfafc8219d6aa6558c1dbbd2baee1b70bbe

# PutContactInformation decides its own ContactInformation 400 before the account-target rule, so the own-account 403 is outranked

_Filed: 2026-11-23T15:02:44Z_
_Operation: PutContactInformation | Class: error-response_

> _Note: this issue was identified against the code base at commit `6c4ddbfafc8219d6aa6558c1dbbd2baee1b70bbe`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`PutContactInformation` validates the primary contact structure (`crate::state::validated_contact_information`, which decides the country-code and `StateOrRegion` rules the model states in prose) at the top of `handle`, and only afterwards resolves the account-id member through `crate::state::target_account_id`. This crate states the account-target rule as one decision taken whole before any refusal a handler makes of its own, and its siblings `StartPrimaryEmailUpdate` / `AcceptPrimaryEmailUpdate` resolve the account id first and refuse their own member afterwards.

Consequence: the same `AccountId` member earns two different answers depending on the contact structure that travels with it.

- `{"AccountId": <the caller's own id>, "ContactInformation": {... "CountryCode": "US", "StateOrRegion": "WA" ...}}` -> 403 `AccessDeniedException`, the refusal tests/all/own_account_target_refusal.rs pins for this operation.
- `{"AccountId": <the caller's own id>, "ContactInformation": {... "CountryCode": "ZZ" ...}}` -> 400 `ValidationException` `CountryCode "ZZ" is not an ISO-3166 two-letter country code`; the 403 is never reached.

So a request whose real fault is that the management account named its own `AccountId` is reported as a bad address instead. The defect is the ordering, not either refusal: `target_account_id` is a pure function of the member and touches no store, so deciding it first leaves every standalone put (the only shape the contact suites drive) answering exactly as it does today, and still satisfies this handler's own promise that "a refused put writes nothing".

## Evidence

The rule as this crate's own suite states it — services/service-com-amazonaws-account-account/tests/all/account_target_composition_is_shared.rs:17-21:

```
//! The rule is also ONE decision taken WHOLE: it is decided before any refusal
//! a handler makes of its own, so a request that is both an account-target
//! refusal and carries a member such a handler would refuse answers that same
//! 403 — for either cause — while the identical request on an account the store
//! holds is refused for its member.
```

The sibling that applies it in that order — services/service-com-amazonaws-account-account/src/ops/start_primary_email_update.rs:186-195 then :212-217:

```
        // The authorization rule comes FIRST, before the proposed address is
        // looked at or stored: ...
        let account_id = match crate::state::target_account_id(Some(&input.account_id)) {
...
        if let Err(refusal) = crate::state::validated_email_address(
            crate::state::PRIMARY_EMAIL_MEMBER,
            &input.primary_email,
        ) {
```

PutContactInformation inverts it — services/service-com-amazonaws-account-account/src/ops/put_contact_information.rs:192-196:

```
        if let Err(invalid) =
            crate::state::validated_contact_information(&input.contact_information)
        {
            return Err(PutContactInformationErrorModeled::ValidationException(invalid).into());
        }
```

and only then put_contact_information.rs:203-208:

```
        let account_id = match crate::state::target_account_id(input.account_id.as_deref()) {
            Ok(account_id) => account_id,
            Err(denied) => {
                return Err(PutContactInformationErrorModeled::AccessDeniedException(denied).into())
            }
        };
```

The refusal the inverted order answers instead — services/service-com-amazonaws-account-account/src/state.rs:295-297 and :316-327:

```
    if !is_country_code(&contact.country_code) {
        return Err(unknown_country_code(&contact.country_code));
    }
...
pub fn unknown_country_code(country_code: &str) -> ValidationException {
    ValidationException {
        message: format!(
            "{COUNTRY_CODE_MEMBER} {country_code:?} is not an ISO-3166 two-letter country code"
        ),
```

The 403 pinned for this operation — services/service-com-amazonaws-account-account/tests/all/own_account_target_refusal.rs:163-169:

```
        ("PutContactInformation", "/putContactInformation", |account| {
            with_account(
                json!({"ContactInformation": contact_information()}),
                "AccountId",
                account,
            )
        }),
```

whose helper (own_account_target_refusal.rs:120-134) is deliberately a VALID US address "so a row that is refused is refused for the account it targets, not for its address" — so the split is unexercised. Nothing pins the inverted order either: the address-rule suite puts with no account-id member at all, tests/all/primary_contact_address_rules.rs:174-179:

```
async fn put(router: &MultiServiceRouter, contact: Value) -> Answer {
    call(
        router,
        "/putContactInformation",
        json!({ "ContactInformation": contact }),
```

and "ZZ" is one of the values that suite lists as not a country code (primary_contact_address_rules.rs:79).

## Steps to Reproduce

1. Register `EmulatedService` for service-com-amazonaws-account-account on a `MultiServiceRouter` over a fresh `engine::InMemoryEngine` with `set_request_authentication(false)`.
2. POST /putContactInformation with body {"AccountId": <state::default_account_id()>, "ContactInformation": {"FullName": "Ada Lovelace", "AddressLine1": "1 Analytical Way", "City": "Seattle", "StateOrRegion": "WA", "PostalCode": "98109", "CountryCode": "US", "PhoneNumber": "+12065550100"}} — observe 403 AccessDeniedException, the refusal tests/all/own_account_target_refusal.rs pins for PutContactInformation.
3. POST /putContactInformation with the same body but "CountryCode": "ZZ" (and no StateOrRegion needed) — observe 400 ValidationException `CountryCode "ZZ" is not an ISO-3166 two-letter country code` instead of the identical 403 from step 2.
4. For contrast, POST /acceptPrimaryEmailUpdate with body {"AccountId": <state::default_account_id()>, "PrimaryEmail": "not-an-email-address", "Otp": "A1b2C3"} — observe 403, because that sibling decides the account-target rule before refusing its own member.
