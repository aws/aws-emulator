Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: 979a2d0ff4bfa74bd31fd0b59f7ed75417ac47f9

# The token-session record's own doc says this service never mints a refresh token, while ExchangeCodeForToken mints one and keys the session by it

_Filed: 2026-11-06T09:02:22Z_
_Operation: ExchangeCodeForToken | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `979a2d0ff4bfa74bd31fd0b59f7ed75417ac47f9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::ProviderTokenSession` is documented as a record whose refresh token is always one a third-party provider issued and that this service never mints. `ExchangeCodeForToken` does the opposite: it mints the refresh token from the credential store and makes that minted value the third element of the `ProviderTokenKey` the session is stored under. The two statements cannot both be true of the same record, and the stale one is the doc a reader of the store consults to learn which refresh tokens the store can resolve: it says the service holds only provider-issued tokens, when half the sessions in it are named by tokens this service minted itself. The module doc of the same file and the doc of `mint_provider_token` state the implemented behaviour, so the contradiction is inside one file.

## Evidence

The record's doc claims the refresh token is never minted here:

  services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/src/state.rs:1696-1698
    /// The refresh token itself is NOT this service's to mint — a caller holds one
    /// the provider issued — so it is held only as the key's third element, and this
    /// record states nothing further about it.

The same file's module doc states the opposite:

  services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/src/state.rs:70-72
    //! `ExchangeCodeForToken` STARTS such a session: it mints the refresh token the
    //! session is named by ([`mint_provider_token`] mints both) as well as the session's
    //! first access token, ...

  services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/src/state.rs:1785-1789 (doc of `mint_provider_token`)
    /// One mint for both tokens a token operation hands back: the ACCESS token a
    /// refresh and an exchange answer with, and the REFRESH token an exchange issues
    /// to name the session it starts ...

And the handler mints it and uses it as the session key:

  services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/src/ops/exchange_code_for_token.rs:243-245
        let (Some(access_token), Some(refresh_token)) = (
            crate::state::mint_provider_token(credentials),
            crate::state::mint_provider_token(credentials),

  services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/src/ops/exchange_code_for_token.rs:258-262
        let key = (
            input.provider,
            input.request.client_id,
            refresh_token.clone(),
        );

Test coverage: tests/all/provider_tokens.rs:9 pins that an exchange answers "a refresh token — and every value is minted rather than fixed", i.e. the minting the record's doc denies; no test asserts the doc's claim.

## Steps to Reproduce

1. Read services/.../src/state.rs:1696-1698: the session record states the refresh token is NOT this service's to mint.
2. Read services/.../src/ops/exchange_code_for_token.rs:243-245 and 258-262: the handler mints a refresh token via the credential store and stores the session under it.
3. Read services/.../src/state.rs:70-72 and 1785-1789: the same file documents that an exchange mints the refresh token.
4. POST /tokens/figma with a body carrying code/redirectUri: the answer's `refreshToken` is emulator-minted, and the stored session is keyed by it — the opposite of what the record's doc states.
