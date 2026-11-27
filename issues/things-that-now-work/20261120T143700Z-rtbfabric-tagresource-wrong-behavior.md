Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 2352e453041dca9d0b463884f1fb4473cbd545be

# The tagging surface resolves a resourceArn against the CALLER's signing Region, not the Region the resource was created in

_Filed: 2026-11-20T14:37:00Z_
_Operation: TagResource | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `2352e453041dca9d0b463884f1fb4473cbd545be`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`tagged_resource` is the single reading of a tagging ARN for `TagResource`, `UntagResource` and `ListTagsForResource`, and it refuses an ARN whose Region field differs from `region` — which every one of the three handlers obtains from the CURRENT request's SigV4 credential scope (`crate::state::request_region(ctx.parts())`). Nothing in the store records which Region a resource belongs to: `RequesterGateway`, `ResponderGateway`, `Link` and `LinkRoutingRule` carry no Region member, and the only Region fact a record holds is baked into a requester gateway's `domainName` at create time from that create's own credential scope.

So the ARN's Region field is never compared against the resource at all — only against the caller's own signing Region. Two consequences, both of which the module's own doc sentence denies:

* a gateway created by a request signed for us-east-1 (its `domainName` is `<id>.rtbfabric.us-east-1.amazonaws.com`) is tagged, untagged and listed happily through an ARN that names us-west-2, as long as that call is signed for us-west-2 — an ARN "naming another Region" that the surface says "names a resource this store never holds" is answered about, and a tag write lands on the record;
* the ARN that names the Region the gateway actually was created in is REFUSED (the 500 `InternalServerErrorException` unresolved answer) whenever the caller signs for a different Region, even though `GetRequesterGateway`/`GetLink`/every other read of this crate resolves that same record regardless of the signing Region.

Secondarily, the Region the comparison uses is passed through `dns_label`, a helper written to build a `DomainName` label (it lowercases and drops every character outside `[a-z0-9-]`). The modelled `RtbTaggableResourceArn` pattern allows `_` in the Region field (`^arn:aws:rtbfabric:[a-zA-Z0-9_-]+:...`), so for a credential scope Region carrying a character `dns_label` strips, `request_region` returns a value no ARN can ever spell, and every tagging call from that caller is refused while every other operation still serves it.

The defect is that the tagging surface claims to scope by the resource's Region while the store keeps no Region per record, so the check only ever restates the caller's own credential scope.

## Evidence

The rule the surface writes down, services/service-com-amazonaws-rtbfabric-rtbfabric/src/state/tagging.rs:107-123:
```
/// The resource `resource_arn` addresses in the account and Region this request was
/// made against, or `None` when it addresses none.
///
/// The ARN is read through the one cross-service ARN parse, so the vendor field says
/// whose resource is named. A tagging call is answered about this emulator's own
/// account ([`::engine::emulator_account_id`]) in the Region the request addressed:
/// an ARN naming another partition, another account or another Region names a
/// resource this store never holds, which is the same answer as one naming a gateway
/// that was never created.
pub fn tagged_resource<'a>(resource_arn: &'a str, region: &str) -> Option<TaggedResource<'a>> {
    let arn = ::service_common::arn::parse_of(resource_arn, ARN_VENDOR)?;
    if arn.partition != ARN_PARTITION
        || arn.region != region
        || arn.account != ::engine::emulator_account_id()
    {
        return None;
    }
```

What `region` is — the CURRENT request's credential scope, src/state.rs:720-726:
```
/// The region the caller addressed, from the request's SigV4 credential scope;
/// [`DEFAULT_REGION`] when the request carries no scope (an anonymous call).
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .and_then(|scope| dns_label(&scope.region))
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}
```
and the three handlers pass exactly that: src/ops/tag_resource.rs:146 `let region = crate::state::request_region(ctx.parts());` (same at src/ops/untag_resource.rs:146 and src/ops/list_tags_for_resource.rs:152), then src/ops/tag_resource.rs:155-158:
```
        let resource = crate::state::tagged_resource(&input.resource_arn, &region)
            .ok_or_else(crate::state::tagging_unresolved)?;
        let tags = crate::state::resource_tags_mut(&mut store, resource)
            .ok_or_else(crate::state::tagging_unresolved)?;
```

That NO record carries a Region, src/state.rs:147-163 (and the same for `ResponderGateway` at 179-203, `Link` at 230-250):
```
/// A requester gateway, as `CreateRequesterGateway` persisted it.
#[derive(Debug, Clone)]
pub struct RequesterGateway {
    pub gateway_id: String,
    pub domain_name: String,
    pub status: String,
...
    pub vpc_id: String,
    pub subnet_ids: Vec<String>,
    pub security_group_ids: Vec<String>,
    pub description: Option<String>,
    pub tags: HashMap<String, String>,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
}
```
The only Region fact the record does hold, written from the CREATE's scope — src/ops/create_requester_gateway.rs:170 and :243:
```
        let region = crate::state::request_region(ctx.parts());
...
            domain_name: crate::state::gateway_domain_name(&gateway_id, &region),
```
with src/state.rs:728-732:
```
pub fn gateway_domain_name(gateway_id: &str, region: &str) -> String {
    format!("{gateway_id}.rtbfabric.{region}.amazonaws.com")
}
```
And the dns-label mangling that the ARN grammar does not share, src/state.rs:707-718:
```
/// One DNS label, from arbitrary text: lowercase, only `[a-z0-9-]`, and never
/// leading or trailing `-`, which is what the modelled `DomainName` pattern
/// accepts for each dot-separated label.
fn dns_label(text: &str) -> Option<String> {
```
against the modelled ARN Region field, `smithy_query(query="operation", operation="TagResource")`: `"pattern": "^arn:aws:rtbfabric:[a-zA-Z0-9_-]+:[0-9]{12}:gateway/..."`.

Test coverage: tests/all/resource_tagging.rs:32-33 signs every request for ONE Region (`const REGION: &str = "us-west-2";` — "The region the test signs for, and so the region every ARN it builds names"), and its only cross-Region case, tests/all/resource_tagging.rs:531-538 ("a gateway of another Region", ARN region `eu-west-1`), is created and read under that same us-west-2 scope. No test creates a resource under one credential scope and tags it under another, so neither symptom above is pinned.

## Steps to Reproduce

1. POST /requester-gateway signed with an authorization header whose credential scope Region is us-east-1 — note the minted gatewayId G and that the answer's domainName is G.rtbfabric.us-east-1.amazonaws.com, i.e. a gateway created in us-east-1.
2. POST /tags/arn%3Aaws%3Artbfabric%3Aus-west-2%3A<emulator-account>%3Agateway%2FG with body {"tags":{"owner":"ads"}}, signed with a credential scope Region of us-west-2: observed HTTP 204 and the tag written onto the us-east-1 gateway; expected the surface's unresolved refusal (HTTP 500 InternalServerErrorException / "Internal server error."), because src/state/tagging.rs:113-115 states that an ARN "naming another Region names a resource this store never holds".
3. POST /tags/arn%3Aaws%3Artbfabric%3Aus-east-1%3A<emulator-account>%3Agateway%2FG — the ARN that names the Region the gateway WAS created in — still signed for us-west-2: observed HTTP 500 InternalServerErrorException, so the resource's own ARN is the one that is refused.
4. GET /requester-gateway/G signed for us-west-2: HTTP 200 — every other read of the crate resolves the record irrespective of the signing Region, proving the store holds no Region for it.
