Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 54e739a086e6a8cc29813a5e33a0aae0b96f568c

# The "resolve the account id, then create-on-first-touch or refuse" composition is written out six times instead of once beside its read/no-create siblings

_Filed: 2026-11-20T06:46:02Z_
_Operation: PutAccountName | Class: maintainability_

> _Note: this issue was identified against the code base at commit `54e739a086e6a8cc29813a5e33a0aae0b96f568c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The state module states the account-target composition ONCE for the two cases that must not create a record (`target_account` for a read, `target_account_mut` for a write), and its module doc says that composition "is what those handlers call, keeping only the mapping into their own operation's error enum". The third case — a write that MAY bring the record into existence — has no such composition, so the identical three-step rule (resolve with `state::target_account_id`, get-or-create with `store.account_mut(&account_id, now)`, map the `None` arm to `state::account_store_full(&account_id)` as this operation's `AccessDeniedException`) is hand-written in six handlers: PutAccountName, PutAlternateContact, PutContactInformation, EnableRegion, DisableRegion and StartPrimaryEmailUpdate.

Six copies of one rule can drift: one of them could keep resolving but stop mapping the store-full arm, or order the own-account refusal differently from the others, with nothing in the crate to notice. Stating it once — a `target_account_or_create(requested, now) -> Result<&mut Account, AccessDeniedException>` on `ComAmazonawsAccountAccountState`, beside the two existing compositions — leaves every response byte-identical (the two refusals and their messages are already produced by the same two shared helpers) and every existing test unchanged; each handler keeps only the `map_err` into its own error enum, exactly as the reading handlers already do.

## Evidence

The crate's own statement that this composition belongs in one place — services/service-com-amazonaws-account-account/src/state.rs:25-33:

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

Copy 1 — services/service-com-amazonaws-account-account/src/ops/put_account_name.rs:191-204:

```
        let account_id = match crate::state::target_account_id(input.account_id.as_deref()) {
            Ok(account_id) => account_id,
            Err(denied) => {
                return Err(PutAccountNameErrorModeled::AccessDeniedException(denied).into())
            }
        };
        let now = ::std::time::SystemTime::now();
        let mut store = state.lock();
        let Some(account) = store.account_mut(&account_id, now) else {
            return Err(PutAccountNameErrorModeled::AccessDeniedException(
                crate::state::account_store_full(&account_id),
            )
            .into());
        };
```

Copy 2 — services/service-com-amazonaws-account-account/src/ops/enable_region.rs:212-225:

```
        let account_id = match crate::state::target_account_id(input.account_id.as_deref()) {
            Ok(account_id) => account_id,
            Err(denied) => {
                return Err(EnableRegionErrorModeled::AccessDeniedException(denied).into())
            }
        };
        let now = ::std::time::SystemTime::now();
        let mut store = state.lock();
        let Some(account) = store.account_mut(&account_id, now) else {
            return Err(EnableRegionErrorModeled::AccessDeniedException(
                crate::state::account_store_full(&account_id),
            )
            .into());
        };
```

The same span appears again verbatim (only the error enum changes) at src/ops/disable_region.rs:213-226, src/ops/put_alternate_contact.rs:213-233, src/ops/put_contact_information.rs:203-216 and src/ops/start_primary_email_update.rs:195-249.

Contrast the single-call form the no-create siblings use — src/ops/delete_alternate_contact.rs:210-212 and src/ops/verify_phone_number.rs:194-196:

```
        let account = store
            .target_account_mut(input.account_id.as_deref())
            .map_err(DeleteAlternateContactErrorModeled::AccessDeniedException)?;
```

Tests that already cover every one of the six copies (responses must stay unchanged): tests/all/account_target_composition_is_shared.rs:312-340 (`an_account_the_store_holds_is_resolved_by_every_operation`, which creates the member record through PutAccountName) and :346-397 (`the_composition_lands_on_the_account_the_member_names`); tests/all/unknown_account_refusal.rs:298-329 and :388-456; tests/all/own_account_target_refusal.rs; tests/all/region_opt_in.rs and tests/all/region_opt_in_listing.rs for the two Region-opt copies; tests/all/primary_email_update.rs for the StartPrimaryEmailUpdate copy.

## Steps to Reproduce

1. grep -n "account_store_full" services/service-com-amazonaws-account-account/src/ops — six handlers each carry their own copy of the `store.account_mut(..) else account_store_full(..)` arm.
2. grep -n "target_account_id" services/service-com-amazonaws-account-account/src/ops — the same six handlers each re-resolve the member themselves, while the eight no-create handlers call `target_account`/`target_account_mut` once.
3. Read services/service-com-amazonaws-account-account/src/state.rs:25-33 and :1365-1398: the read and no-create compositions exist as named methods; the create-on-first-touch composition does not.
4. Confirm the behaviour is identical in all six copies by running `cargo test -p service-com-amazonaws-account-account` (account_target_composition_is_shared, unknown_account_refusal, own_account_target_refusal, region_opt_in, primary_email_update) before and after extracting the helper.
