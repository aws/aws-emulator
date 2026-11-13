Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 557dff2185bef67f1e026291691c32d64dc3fcba

# A portal request addressed to any ordinary hostname (a docker service name, a proxy, a corporate DNS name) is not claimed at all, so all four routes answer 404 UnknownService instead of their modelled result

_Filed: 2026-11-01T15:32:51Z_
_Operation: Logout | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `557dff2185bef67f1e026291691c32d64dc3fcba`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`EmulatedService::can_handle` claims an unsigned portal request only in two shapes: when the resolved route key IS the signing name (`awsssoportal`), or when the resolved route key came from a host that `is_portal_endpoint` accepts (`portal.sso` or `portal.sso.<something>`). Everything else falls to the `None` arm, which claims by the request's own modelled (method, path) signature.

The crate's comment on that `None` arm enumerates what reaches it: "a host that names no service (the emulator's own endpoint, a bare IP, a proxy)". That enumeration is wrong for anything but loopback, an IP literal and an absent host. The framework's host-style tier (`service_from_host_authority`) returns the FIRST DOT-SEPARATED LABEL of any host, rejecting only a `localhost` label and all-digit labels. So a client pointed at `http://emulator.corp.local:9297`, `http://host.docker.internal:9297`, `http://aws:9297` (a docker-compose service name) or any ALB/proxy DNS name resolves to route key `emulator` / `host` / `aws`, lands in `can_handle`'s `Some(key)` arm, fails `is_portal_endpoint`, and the service does NOT claim the request — even though its method and path are exactly one of the four `@http` bindings and `serves_route` would have claimed it. No other service's `can_handle` matches that key either, so the router answers 404 `UnknownService`.

The consequence is endpoint-dependent service for every operation of this crate: the identical, perfectly modelled request is served on `localhost:9297`, on `127.0.0.1:9297` and with no `Host` header at all, and refused 404 on `emulator.corp.local`. For `Logout` it also means the one state change the operation exists to make — invalidating the server-side sign-in session — never happens, so a later `ListAccounts`/`GetRoleCredentials` with that token keeps being served; `GetRoleCredentials`, `ListAccounts` and `ListAccountRoles` are unreachable the same way, and the 401 credential gate in `override_routing_error` is never consulted because the service never claims the request.

This is NOT the already-filed path-spelling defect (`20261030T212213Z-sso-listaccounts-wrong-behavior`, which is about `serves_route`/`absent_required_members` comparing a slash-padded path with `==` while the REST router trims slashes). Here the path is spelled exactly as the model does and `serves_route` is never reached at all: the defect is the `Some(key)` arm's extra `is_portal_endpoint` conjunct swallowing a request whose host merely has a dotted (or single, non-loopback) label.

## Evidence

The claim arm and its own enumeration of what reaches the `None` arm (`services/service-com-amazonaws-sso-swbportalservice/src/service.rs:310-321`):
```rust
            Some(key) => endpoint_host(parts).is_some_and(|host| {
                Some(key.as_str()) == host.split('.').next() && is_portal_endpoint(&host)
            }),
            // NOTHING in the request named a service at all: no credential scope, no
            // `X-Amz-Target` (`restJson1` has none), and a host that names no service
            // (the emulator's own endpoint, a bare IP, a proxy). That is the normal
            // shape of a portal request from a client pointed at this emulator, so the
            // request is attributed by its own modelled signature instead
            None => serves_route(parts),
```
and what `is_portal_endpoint` admits (`src/service.rs:82-87`):
```rust
fn is_portal_endpoint(host: &str) -> bool {
    host == PORTAL_ENDPOINT_PREFIX
        || host
            .strip_prefix(PORTAL_ENDPOINT_PREFIX)
            .is_some_and(|rest| rest.starts_with('.'))
}
```

The routing key a "proxy" host actually resolves to — the first label of ANY host, with only a `localhost` label and IP literals rejected (`vendor/server/src/lib.rs:1592-1604`):
```rust
fn service_from_host_authority(host: &str) -> Option<String> {
    let host_no_userinfo = host.rsplit('@').next().unwrap_or(host);
    let host_no_port = host_no_userinfo
        .split(':')
        .next()
        .unwrap_or(host_no_userinfo);
    let label = host_no_port.split('.').next()?;
    let key = normalize_service_key(label);
    if key.is_empty() || key == "localhost" || key.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some(key)
}
```
so `Host: emulator.corp.local:9297` yields `Some("emulator")`, which equals `host.split('.').next()` and so passes the first conjunct, and then fails `is_portal_endpoint` — the request is unclaimed.

The rule this crate states for itself (`tests/all/endpoint_agnostic_routing.rs:5-12`):
```
//! RULE: every portal operation is `@optionalAuth` with an empty `@auth` and the
//! protocol is `restJson1`, so a portal request carries no SigV4 credential scope and
//! no `X-Amz-Target`; when the host it is addressed to names no service either, the
//! request's own modelled signature — the (method, path) pair of one of the four
//! `@http` bindings — is what attributes it to this service. So each of the four
//! operations answers its modelled result (its own 200 from state, or 401
//! `UnauthorizedException` for a token that names no usable session) whatever endpoint
//! host the client was pointed at, including none at all.
```

Why the suite does not catch it — every endpoint row is loopback, an IP literal, absent, or the portal host itself, i.e. exactly the hosts for which `service_from_host_authority` returns `None` or the portal key (`tests/all/endpoint_agnostic_routing.rs:46-56`):
```rust
const ENDPOINTS: &[(&str, Option<&str>)] = &[
    ("the emulator's own loopback endpoint", Some("localhost:9297")),
    ("a loopback name with no port", Some("localhost")),
    ("a loopback IP with a port", Some("127.0.0.1:9297")),
    ("a private IP endpoint", Some("10.0.0.5:8080")),
    ("no Host header at all", None),
    (
        "the modelled portal endpoint",
        Some("portal.sso.us-east-1.amazonaws.com"),
    ),
];
```
No row uses a non-loopback, non-IP hostname, so the `Some(key)` arm is never exercised with a host that is not `portal.sso*`.

The Logout state change that is lost when the request is unclaimed (`src/ops/logout.rs:180-191`):
```rust
        // Invalidating the sign-in session is what this operation DOES.
        if !state.lock().invalidate_session(&input.access_token, now) {
```

Test coverage: `tests/all/endpoint_agnostic_routing.rs` sweeps the six endpoints above × the four routes (all served), and `tests/all/portal_session.rs:734-798` (`the_portal_claims_both_unsigned_and_signed_requests`) covers only the portal host and a SigV4-signed request. Nothing pins an ordinary hostname, which is why the `is_portal_endpoint` conjunct can reject a modelled request without failing the suite.

## Steps to Reproduce

1. Build the harness `tests/all/portal_session.rs:85-114` builds: an `engine::InMemoryEngine`, `MultiServiceRouter::register_service(EmulatedService::new(engine))`, then `put_account("111111111111", "Production", "production@example.com", ["AdministratorAccess"])` and `put_session("tok-live", "tok-live@example.com", ["111111111111"], 8h)`.
2. Dispatch `POST http://emulator.corp.local:9297/logout` with `host: emulator.corp.local:9297` and `x-amz-sso_bearer_token: tok-live`. Observed: 404 `UnknownService` — `resolve_route_key` yields `Some("emulator")` (first host label, `vendor/server/src/lib.rs:1598`), `can_handle`'s `Some(key)` arm requires `is_portal_endpoint("emulator.corp.local")`, which is false, so the service never claims the request and `serves_route` is never consulted. Expected per `tests/all/endpoint_agnostic_routing.rs:5-12`: the modelled 200 and the session invalidated.
3. Check the session in state: `portal.state().lock().sessions["tok-live"].active` is still `true` — the one state change `Logout` exists to make did not happen.
4. Dispatch the IDENTICAL request to `http://localhost:9297/logout` with `host: localhost:9297`. Observed: 200 and the session invalidated — the only difference is the endpoint hostname.
5. Repeat step 2 for the three read routes, e.g. `GET http://host.docker.internal:9297/assignment/accounts` and `GET http://aws:9297/assignment/accounts` (a single-label docker service name) with `x-amz-sso_bearer_token: tok-live`: both answer 404 `UnknownService` instead of the seeded `accountList`, while `host: localhost:9297` answers 200.
6. Repeat step 2 with NO bearer header on `emulator.corp.local`: the answer is still 404 `UnknownService`, not the 401 `UnauthorizedException` "Session token not found or invalid" that `src/service.rs:363-372` and `tests/all/bearer_token_gate.rs:6-13` state is answered on every bound route — `override_routing_error` is never reached because the service did not claim the request.
