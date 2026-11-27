Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: a07844ce34e0c9873e72adc5edaddfb14e92856b

# The @idempotencyToken parameter fingerprint renders a tag map ambiguously, so a retry with genuinely different tags is answered with the first call's gateway instead of being refused

_Filed: 2026-11-20T09:23:27Z_
_Operation: CreateRequesterGateway | Class: validation_

> _Note: this issue was identified against the code base at commit `a07844ce34e0c9873e72adc5edaddfb14e92856b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The three creates that honour `clientToken` (CreateRequesterGateway, CreateResponderGateway, CreateLinkRoutingRule) decide "same token, same parameters?" by comparing a SHA-256 of a rendering of the request, and the tags half of that rendering is built by `crate::state::canonical_tags`, which joins the sorted pairs as `key=value` separated by `,` with no escaping and no length prefix.

The modelled `TagsMap` value (`TagValue`) carries NO `@pattern` — only a length bound of 0..1600 — so a tag value may legally contain both `,` and `=`. Two DIFFERENT, both-legal tag maps therefore render to the same string and fingerprint identically:

* `{"a": "b,c=d"}` -> `a=b,c=d`
* `{"a": "b", "c": "d"}` -> `a=b,c=d`

So a retry that reuses a `clientToken` with one of these maps where the first call used the other is NOT taken as the parameter mismatch it is: the handler's `prior.fingerprint != fingerprint` branch is skipped, the call answers 200 with the FIRST call's `gatewayId`/`domainName`/`status`, and the second call's tags are silently dropped — no stored resource ever carries them. The handler refuses every other differing member on this path (a changed `vpcId`, `subnetIds`, `description`) with `ValidationException`; only a collision in the tag rendering escapes it.

This is also the exact hazard the crate's own hashing helper guards against one function above: `digest` length-prefixes every part "so two different splits of the same bytes cannot collide", while the tag rendering it is fed re-introduces the ambiguity it was built to avoid.

## Evidence

Model, `smithy_query(query="shape", shape="TagsMap")` — the value has a length bound and NO pattern, so `,` and `=` are legal characters in a tag value (the key's pattern excludes `,`):
```
"TagsMap": {"type": "map",
  "key": {"target": "TagKey", "length": {"min": 1, "max": 128}, "pattern": "^(resourceArn|internalId|[a-zA-Z0-9+\\-=._:/@]+)$"},
  "value": {"target": "TagValue", "length": {"min": 0, "max": 1600}}}
```

The documented contract, reproduced in the handler header, services/service-com-amazonaws-rtbfabric-rtbfabric/src/ops/create_requester_gateway.rs:51-58: "Passing the same value to a later call to an operation requires that you also pass the same value for all other parameters. ... If you retry the operation with the same clientToken, but with different parameters, the retry fails with an IdempotentParameterMismatch error."

The crate's own statement of the same rule, src/state.rs:350-354:
```
    /// A digest of that call's other parameters: the documented contract is that
    /// a retry of the same token with DIFFERENT parameters fails rather than
    /// silently answering the first call's resource.
    pub fingerprint: [u8; 32],
```

The ambiguous rendering, src/state.rs:684-705:
```
/// A tag map rendered canonically (sorted by key), so the same tags in a
/// different iteration order fingerprint identically.
...
pub fn canonical_tags(tags: Option<&HashMap<String, String>>) -> String {
    let mut pairs: Vec<(&str, &str)> = ...
    pairs.sort_unstable();
    pairs
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>()
        .join(",")
}
```

The principle the sibling helper applies to the very same digest, src/state.rs:671-673:
```
/// SHA-256 over `parts`, each length-prefixed so two different splits of the
/// same bytes cannot collide.
fn digest(parts: &[&[u8]]) -> [u8; 32] {
```

Where the collision takes effect, src/ops/create_requester_gateway.rs:184-213 — the tag rendering is interpolated raw and last, and a fingerprint match returns the first call's resource:
```
        let fingerprint = crate::state::parameter_fingerprint(&format!(
            "vpcId={:?}|subnetIds={:?}|securityGroupIds={:?}|description={:?}|tags={}",
            ... crate::state::canonical_tags(input.tags.as_ref()),
        ));
        let token = crate::state::idempotency_key(Self::NAME, &input.client_token);
        if let Some(prior) = store.idempotency.get(&token).cloned() {
            if prior.fingerprint != fingerprint {
                return Err(... ValidationException { message: format!(
                            "clientToken {} was already used for a {} request with different parameters", ...
            if let Some(gateway) = store.requester_gateways.get(&prior.resource_id) {
                return Ok(CreateRequesterGatewayOutput { ... });
```
The same rendering feeds the fingerprint of the other two token-bearing creates: src/ops/create_responder_gateway.rs:254-271 (`|tags={}` ... `crate::state::canonical_tags(input.tags.as_ref())`) and src/ops/create_link_routing_rule.rs:214-221.

Test coverage: the mismatch path is exercised only by varying `vpcId` — tests/all/gateway_link_lifecycle.rs:836-847 ("Same token, different parameters: the documented contract is a failure" / `changed["vpcId"] = json!("vpc-ffffffffffffffff0")`) and tests/all/link_routing_rules.rs:527. No test anywhere uses a tag value containing `,` or `=`, so nothing pins the colliding renderings.

## Steps to Reproduce

1. POST /requester-gateway with {"clientToken":"t1","vpcId":"vpc-0123456789abcdef0","subnetIds":["subnet-0123456789abcdef0"],"securityGroupIds":["sg-0123456789abcdef0"],"tags":{"a":"b","c":"d"}} — answers 200 with a gatewayId G1; the stored gateway carries the two tags.
2. POST /requester-gateway again with the SAME clientToken "t1", the same vpcId/subnetIds/securityGroupIds, but "tags":{"a":"b,c=d"} — one tag whose value contains ',' and '=', which the modelled TagValue admits (no pattern).
3. Observed: HTTP 200 answering G1 (the first call's gatewayId/domainName/status). The second call's different tag map is neither stored nor reported, and GET /requester-gateway/G1 still shows {"a":"b","c":"d"} — the retry was treated as a replay because `canonical_tags` rendered both maps as the string `a=b,c=d`.
4. Expected (the handler's own rule for every other differing member, and the clientToken contract quoted in its header): the retry's parameters differ, so it is refused — HTTP 400 ValidationException "clientToken t1 was already used for a CreateRequesterGateway request with different parameters".
5. The same two bodies against POST /responder-gateway (create_responder_gateway.rs:254-271) and POST /responder-gateway/{gatewayId}/link/{linkId}/routing-rule (create_link_routing_rule.rs:214-221) collide identically.
