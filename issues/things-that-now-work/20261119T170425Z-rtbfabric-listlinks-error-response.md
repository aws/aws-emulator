Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 561579de3e0b6bd91c56d47afed233bd7828579f

# ListLinks answers an expired nextToken with ValidationException where its own nextToken documentation states an HTTP 400 InvalidToken error

_Filed: 2026-11-19T17:04:25Z_
_Operation: ListLinks | Class: error-response_

> _Note: this issue was identified against the code base at commit `561579de3e0b6bd91c56d47afed233bd7828579f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListLinksRequest$nextToken`'s own modelled documentation names the error an expired pagination token is answered with: an HTTP 400 `InvalidToken` error. The ListLinks handler decodes the token through the shared codec, which already distinguishes expiry (`TokenRefusal::Expired`) from a token it never issued (`TokenRefusal::Unrecognised`), and then collapses BOTH refusals onto the modelled `ValidationException`. So the wire `__type` a client matches on is `ValidationException` and never `InvalidToken`, for every refused token including the expired one.

The crate already honours the other half of that same documentation sentence: `TOKEN_LIFETIME` is 24 hours precisely because of it, and `page_tokens.rs` cites the sentence as the reason. It also already owns the mechanism for answering a code outside an operation's modelled error set (`Scaffolded::unmodeled`, used by the tagging surface through `tagging_unresolved`), so the distinction is reachable without new machinery.

Note on scope: an existing open issue (20261119T001456Z-rtbfabric-listrequestergateways-error-response) files the same defect for ListRequesterGateways and names ListResponderGateways as the second identical copy. ListLinks is a THIRD copy that issue does not mention, and the mapping lives in this handler's own body (list_links.rs:191-195), so fixing the two gateway listings as that issue describes leaves ListLinks answering ValidationException.

## Evidence

Smithy, `smithy_query(query="doc", shape="ListLinksRequest", member="nextToken")` — verbatim: "If nextToken is returned, there are more results available. The value of nextToken is a unique pagination token for each page. Make the call again using the returned token to retrieve the next page. Keep all other arguments unchanged. Each pagination token expires after 24 hours. Using an expired pagination token will return an HTTP 400 InvalidToken error."

The handler that maps both refusals to one modelled error, services/service-com-amazonaws-rtbfabric-rtbfabric/src/ops/list_links.rs:189-198:
```
        let resume_after = match input.next_token.as_deref() {
            Some(token) => Some(
                crate::state::decode_next_token(Self::NAME, token, now).map_err(|refusal| {
                    ListLinksErrorModeled::ValidationException(ValidationException {
                        message: refusal.message(Self::NAME),
                    })
                })?,
            ),
            None => None,
        };
```

The codec already separates the expired case, src/state/page_tokens.rs:30-52:
```
/// Why a `nextToken` could not be resumed from.
#[derive(Debug, PartialEq, Eq)]
pub enum TokenRefusal {
    /// Not a token this operation handed out.
    Unrecognised,
    /// This operation's own token, but issued more than [`TOKEN_LIFETIME`] ago.
    Expired,
}
```
and src/state/page_tokens.rs:12-15 cites the very same documentation sentence as its authority:
```
/// How long a `nextToken` a listing issued stays usable. The `nextToken` member's own
/// documentation states that each pagination token expires after 24 hours, so this span
/// is the service's rather than the emulator's choice.
pub const TOKEN_LIFETIME: ::std::time::Duration = ::std::time::Duration::from_secs(24 * 60 * 60);
```

The crate's existing way to answer a code outside an operation's modelled set, src/state/tagging.rs:66-72:
```
pub fn tagging_unresolved<E>() -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::unmodeled(
        TAGGING_UNRESOLVED_CODE,
        TAGGING_UNRESOLVED_STATUS,
        TAGGING_UNRESOLVED_MESSAGE,
    )
}
```

Test coverage: tests/all/gateway_link_lifecycle.rs:1816-1840 drives six bad ListLinks tokens ("not hex at all", "an odd number of hex digits", "hex carrying no cursor", "hex of a bare link id", "far longer than any issued token", "another listing's token") and asserts only `assert_eq!(status, 400, ...)` — it never asserts the error code, and no test exercises an EXPIRED ListLinks token. The codec's own unit test pins only the refusal variant (src/state/page_tokens.rs:157-161: "a token at the end of its span is expired, not malformed"), not the wire code. So nothing pins `ValidationException` on this path; the status stays 400 either way.

## Steps to Reproduce

1. Create a requester gateway (POST /requester-gateway) and a responder gateway (POST /responder-gateway), then POST /gateway/{requesterId}/create-link with the responder as peerGatewayId so the gateway has at least one link to list.
2. Build a token this operation would have issued at the epoch, using the encoding of `encode_next_token` (operation, unit separator, cursor, unit separator, issuing instant in whole seconds): hex-encode the string "ListLinks\u{1f}link-0000000000000001\u{1f}0".
3. GET /gateway/{requesterId}/links/?nextToken=<that hex>.
4. Observed: HTTP 400 whose body carries `__type` `ValidationException` and the message "nextToken has expired: a ListLinks pagination token expires 24 hours after it is issued".
5. Expected per the documented trigger quoted above: HTTP 400 with the `InvalidToken` error code, which the handler can render through the crate's existing `Scaffolded::unmodeled` seam while keeping the 400 status.
