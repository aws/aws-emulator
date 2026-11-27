Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: e8f5c30adcdb5500cfbc4ddcd5357551c6a713aa

# A resumed ListAccounts page serves an account twice when an account with a smaller id is associated between pages

_Filed: 2026-11-27T02:20:21Z_
_Operation: ListAccounts | Class: pagination_

> _Note: this issue was identified against the code base at commit `e8f5c30adcdb5500cfbc4ddcd5357551c6a713aa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

ListAccounts pages over `accounts_in_order`, a sequence the handler re-derives per call by sorting the enrollment store ASCENDING BY ACCOUNT ID, and it resumes a walk by a positional offset counted off the front of that freshly recomputed sequence (`walk_page`). AssociateAccounts creates an enrollment entry keyed by an account id taken FROM THE REQUEST, so an association performed between two pages of one walk inserts a new element anywhere in that sorted sequence — including BEFORE the position the outstanding `nextToken` names. Every element from the insertion point on then shifts one position later, so the cursor's offset now denotes an account the previous page already served, and the resumed page reports it a second time.

Concretely, with the caller's own account `123456789012` enrolled and member `999999999999` associated, `maxResults: 1` serves [123456789012] and mints a cursor at offset 1. AssociateAccounts {"accountIds":["000000000001"]} makes the sequence [000000000001, 123456789012, 999999999999]. Resuming with the token counts 1 item (000000000001) as already served and serves [123456789012] again — an account the first page already reported. The token's fingerprint matches (this listing binds only the operation name, `page_fingerprint(Self::NAME, &())`), so the cursor is accepted rather than refused.

This is the insertion case; the open ListAutomationRules pagination issue covers only a sequence being SHORTENED from the middle by DeleteAutomationRule, and explicitly reasons that ListAutomationRules "is the one whose sequence a modelled operation can shorten from the middle" — it does not consider a listing whose sequence a modelled operation can LENGTHEN from the middle, which is what AssociateAccounts does to this one. Nothing in the crate justifies the case and no test associates an account between two pages of one walk.

## Evidence

The sequence is re-sorted by account id on every call, and the crate states what paging over it requires:
`src/state.rs:3546-3549` (doc of `accounts_in_order`): "The store is a map, whose iteration order is its own business, so the ORDER is imposed here: ascending account id is the same sequence on every call, which is what paging over it requires — the pages of one walk must be windows onto ONE sequence rather than samples of a shifting one."
`src/state.rs:3560`: `accounts.sort_by(|left, right| left.account_id.cmp(&right.account_id));`

The listing pages that sequence with a caller-supplied positional cursor:
`src/ops/list_accounts.rs:220`: `let accounts = guard.accounts_in_order(at);`
`src/ops/list_accounts.rs:226-227`: `let served: Option<(Vec<AccountInfo>, Option<String>)> =\n            crate::state::resume_page(accounts, offset, page_size, query_fingerprint);`
and the resume is purely positional:
`src/state.rs:4357`: `let before = items.by_ref().take(offset).count();`

The writer that inserts into the middle of that sorted sequence, keyed by a REQUEST-supplied id:
`src/ops/associate_accounts.rs:255-256`: `for account_id in &requested {\n                    guard.associate_account(account_id, associated_at);`
`src/state.rs:3760` (in `associate_account`): `let enrollment = self.enrollments.entry(account_id.to_string()).or_default();`
`src/state.rs:106-107` (doc of MAX_ENROLLED_ACCOUNTS): "`AssociateAccounts` takes the account ids it stores FROM THE REQUEST (any id matching `AccountId`'s `^[0-9]{12}$`)".

The walk rule this breaks, pinned by the crate's own shared-walk suite:
`tests/all/shared_page_walk.rs:5-10`: "The rule: a page is the slice of the already-selected sequence that starts where the previous page stopped and holds at most `maxResults` items, and a `nextToken` is carried exactly while items remain after it, resuming at the item the page stopped before. So walking a listing at ANY page size yields the same items, in the same order, with no item served twice and none skipped."

Test coverage: `tests/all/shared_page_walk.rs` drives the walk over static sequences only; `tests/all/account_listing.rs` and `tests/all/account_association.rs` never associate an account between two pages of one ListAccounts walk (no test in `tests/all/` does).

## Steps to Reproduce

1. UpdateEnrollmentConfiguration {"status":"Active"} — the caller's account (123456789012) now has an enrollment entry.
2. AssociateAccounts {"accountIds":["999999999999"]} — the store holds two accounts.
3. ListAccounts {"maxResults":1} — observe the page [123456789012] and a nextToken.
4. AssociateAccounts {"accountIds":["000000000001"]} — a third entry, sorting FIRST.
5. ListAccounts {"maxResults":1,"nextToken":<token from step 3>} — the page reports 123456789012 again, an account the first page already served; continuing the walk reports it twice in one walk.
