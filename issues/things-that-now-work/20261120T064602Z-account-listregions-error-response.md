Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: 54e739a086e6a8cc29813a5e33a0aae0b96f568c

# ListRegions splits the account-target composition around its NextToken check, so the unknown-account 403 loses to the token 400 while the own-account 403 wins

_Filed: 2026-11-20T06:46:02Z_
_Operation: ListRegions | Class: error-response_

> _Note: this issue was identified against the code base at commit `54e739a086e6a8cc29813a5e33a0aae0b96f568c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every other reader of this service resolves its account through the one composition the state module states once — `ComAmazonawsAccountAccountState::target_account`, which applies BOTH halves of the account-target rule (the caller's own account, and an id the store holds no record of) before the handler looks at anything else. `ListRegions` is the only reader that does not call it: it calls the first half (`state::target_account_id`) at the top of `handle`, then decodes/refuses `NextToken`, and only afterwards applies the second half (`store.accessible_account`).

Because the two halves straddle the token check, the two refusals the crate's own comment calls "the same modelled authorization refusal" land on opposite sides of the `NextToken` refusal:

- `{"AccountId": "<unknown member id>", "NextToken": "<not a token this op issued>"}` -> 400 `ValidationException` ("NextToken is not a pagination token that ListRegions issued"); the 403 the composition states is never reached.
- `{"AccountId": "<the caller's own id>", "NextToken": "<the same bad token>"}` -> 403 `AccessDeniedException`.

So one half of one rule outranks the token refusal and the other does not, and the same request shape that `GetRegionOptStatus`/`GetAccountInformation`/`GetPrimaryEmail` answer 403 for, `ListRegions` answers 400 for. The defect is the split, not the token refusal itself: routing `ListRegions` through `store.target_account(input.account_id.as_deref())` the way its siblings do would make both halves behave alike.

## Evidence

The rule the crate states once — services/service-com-amazonaws-account-account/src/state.rs:25-33:

```
//! account id goes through. An operation that acts on an account which must
//! ALREADY exist refuses an id this emulator holds no record of in one place
//! too ([`ComAmazonawsAccountAccountState::accessible_account`], and
//! [`ComAmazonawsAccountAccountState::accessible_account_mut`] where the record
//! is then changed), and the COMPOSITION of the two —
//! [`ComAmazonawsAccountAccountState::target_account`] for a read,
//! [`ComAmazonawsAccountAccountState::target_account_mut`] for a write that
//! must not create a record — is what those handlers call, keeping only the
//! mapping into their own operation's error enum.
```

and state.rs:1370-1376 on `target_account`:

```
    /// `Err` carries this service's two account-target refusals, both of them
    /// the modelled `AccessDeniedException`: the member named the caller's own
    /// account ([`cannot_target_own_account`]), or it named an account this
    /// emulator holds no record of ([`account_not_accessible`]). The refusal is
    /// decided HERE, before anything about that account's sub-resources is
    /// looked at, and a caller keeps only the mapping into its own operation's
    /// error enum.
```

A sibling reader that does call it — services/service-com-amazonaws-account-account/src/ops/get_region_opt_status.rs:189-191:

```
        let account = store
            .target_account(input.account_id.as_deref())
            .map_err(GetRegionOptStatusErrorModeled::AccessDeniedException)?;
```

ListRegions instead splits it — services/service-com-amazonaws-account-account/src/ops/list_regions.rs:246-247:

```
        let account_id = crate::state::target_account_id(input.account_id.as_deref())
            .map_err(ListRegionsErrorModeled::AccessDeniedException)?;
```

then list_regions.rs:252-257:

```
        let resume_after = match input.next_token.as_deref() {
            None => None,
            Some(token) => Some(decode_page_token(token).ok_or_else(|| {
                ListRegionsErrorModeled::ValidationException(invalid_next_token())
            })?),
        };
```

and only then list_regions.rs:259-268, whose own comment asserts the second half is "the same modelled authorization refusal":

```
        // The other half of the account-target rule, stated once in
        // `state::ComAmazonawsAccountAccountState::accessible_account`: an id
        // this emulator holds no record of is the same modelled authorization
        // refusal, decided before the Region list is projected.
        let store = state.lock();
        let account = store
            .accessible_account(&account_id)
            .map_err(ListRegionsErrorModeled::AccessDeniedException)?;
```

Test coverage: services/service-com-amazonaws-account-account/tests/all/account_target_composition_is_shared.rs:156-165 lists `ListRegions` (plain, and "filtered and paged") among the rows that must answer ONE identical refusal payload, and tests/all/unknown_account_refusal.rs:149-164 pins `ListRegions` at 403 for an unknown account — but no row combines an unknown account with a NextToken the operation never issued, so the split is not exercised. tests/all/pagination_token_spelling.rs drives the token refusal only on the caller's own account.

## Steps to Reproduce

1. Register `EmulatedService` for `service-com-amazonaws-account-account` on a `MultiServiceRouter` with a fresh `engine::InMemoryEngine` (as tests/all/pagination_token_spelling.rs:39-45 does).
2. POST /listRegions with body {"AccountId": "210987654321"} — observe 403 AccessDeniedException, the refusal tests/all/unknown_account_refusal.rs pins for an account the store holds no record of.
3. POST /listRegions with body {"AccountId": "210987654321", "NextToken": "zz"} — observe 400 ValidationException naming NextToken; the unknown-account 403 is never reached because `accessible_account` runs after `decode_page_token`.
4. POST /listRegions with body {"AccountId": <service_state default_account_id()>, "NextToken": "zz"} — observe 403 AccessDeniedException: the OTHER half of the same rule still outranks the identical token refusal.
5. POST /getRegionOptStatus with body {"AccountId": "210987654321", "RegionName": "eu-north-1"} — observe 403, the answer the shared composition gives for the same unknown account.
