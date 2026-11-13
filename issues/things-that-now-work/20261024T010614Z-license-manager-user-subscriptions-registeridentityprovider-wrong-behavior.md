Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 047f25c8e675653717aa743423e962efe8d38250

# RegisterIdentityProvider resolves AWS-managed DirectoryId against a 3-entry hardcoded directory fleet

_Filed: 2026-10-24T01:06:14Z_
_Operation: RegisterIdentityProvider | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `047f25c8e675653717aa743423e962efe8d38250`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`resolve_directory` refuses any `ActiveDirectoryIdentityProvider.DirectoryId` of the `d-` form that is not present in `account_directories` with stage `Active`, and that map is seeded ONLY from the hardcoded `crate::state::seed_account_directories()` list (`d-9f8e7d6c5b`, `d-1a2b3c4d5e`, `d-4455667788`).

Expected: a directory this account actually has can be registered. In the real service the `DirectoryId` is resolved against AWS Directory Service, so a directory created by `ds:CreateDirectory`/`ds:CreateMicrosoftAD` (both implemented for real in `services/service-com-amazonaws-directoryservice-directoryservice-20150416/src/ops/`) is registrable and the call answers 200 with an `IdentityProviderSummary` in `REGISTERING`.

Actual: registering any directory id other than the three baked-in ones answers the modelled `ValidationException` (400) "<id> directory either cannot be found or is not in active state". The only writer of the fleet is the in-process `register_account_directory` seam, which no API client can reach, so the documented setup flow (create a Managed AD, then register an identity provider for it) is impossible unless the caller happens to guess one of three ids. Workarounds exist only by making the provider look self-managed (`ActiveDirectorySettings` with no `DirectoryId`) or by setting `IsSharedActiveDirectory: true`, both of which skip the check entirely — so the check rejects exactly the realistic case and passes the others.

## Evidence

Spec: `ActiveDirectoryIdentityProvider.DirectoryId` carries `@pattern ^(d|sd)-[0-9a-f]{10}$` (Smithy model) and is documented as the id of the directory to register — any directory of the account, with no closed value set. Handler: `src/ops/register_identity_provider.rs` lines 193 and 330-353 (`resolve_directory` -> `state.resolves_active_directory`). State: `src/state.rs` `seed_account_directories()` (line 456) returns exactly 3 ids; `resolves_active_directory` (line 604) is a pure lookup in that map; the only writer is `register_account_directory` (line 589), exercised only by the test harness (`tests/all/identity_provider_subscriptions.rs` lines 85-111, which seeds `d-0123456789` etc. through the state seam precisely because no API can). Tests: `a_directory_that_does_not_resolve_as_active_is_refused_naming_the_id` pins the refusal for unseeded ids but nothing asserts a directory created via the Directory Service emulator is accepted.

## Steps to Reproduce

1. Start the emulator with Directory Service and license-manager-user-subscriptions mounted.
2. Call ds:CreateMicrosoftAD (or CreateDirectory) and note the returned DirectoryId.
3. Call RegisterIdentityProvider with IdentityProvider.ActiveDirectoryIdentityProvider.DirectoryId = that id, Product=OFFICE_STANDARD, Settings={Subnets:[subnet-0123456789abcdef0], SecurityGroupId: sg-0123456789abcdef0}.
4. Observe HTTP 400 ValidationException '<id> directory either cannot be found or is not in active state'; expected HTTP 200 with an IdentityProviderSummary carrying a minted IdentityProviderArn.
5. Repeat with DirectoryId=d-9f8e7d6c5b and observe success — proving the check keys on a fixed 3-entry list.
