Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 0920ef23503655d832629572c20a90ca0ad72cd2

# ListLinks omits the gateway's outbound external links, contradicting the crate's own definition of "the gateway's links" (and hardcoding publicEndpoint to null)

_Filed: 2026-11-21T01:37:41Z_
_Operation: ListLinks | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `0920ef23503655d832629572c20a90ca0ad72cd2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListLinks` enumerates only `store.links` — the links between two stored gateways — and never looks at `store.outbound_external_links`, although the crate holds those records keyed by the same `(gatewayId, linkId)` pair and the crate's own state module defines them as links OF that gateway. The result is that one and the same gateway answers two contradicting things about the very same record: `GetRequesterGateway` reports `totalLinksCount: 1` after a `CreateOutboundExternalLink` (because `link_counts` scans `outbound_external_links`), while `ListLinks` on that gateway answers an empty page. The listing's own entry shape carries `publicEndpoint` precisely for such a link, and the handler hardcodes it to `None` behind a comment that states something false about the store: "no link this store holds has one" — `crate::state::OutboundExternalLink` holds exactly that field (`public_endpoint: String`), written by `CreateOutboundExternalLink`, which is implemented. So the member can never be anything but null, and an entire class of persisted links is invisible to the only listing the service has.

## Evidence

Smithy, `smithy_query(query="doc", shape="ListLinks")` — verbatim: "Lists links associated with gateways." / "Returns a list of all links for the specified gateways, including their status and configuration details." (the same sentence is reproduced in the handler header, services/service-com-amazonaws-rtbfabric-rtbfabric/src/ops/list_links.rs:52-53). The entry shape's member, `smithy_query(query="shape", shape="ListLinksResponseStructure")`: `"publicEndpoint": {"target": "URL", ... "doc": "<p>The public endpoint of the outbound link.</p>"}`.

The handler scans only one map — services/service-com-amazonaws-rtbfabric-rtbfabric/src/ops/list_links.rs:209-214:
```
        let mut mine: Vec<&crate::state::Link> = store
            .links
            .values()
            .filter(|link| link.has_end(&input.gateway_id))
            .collect();
```
and asserts the store holds no link with a public endpoint — src/ops/list_links.rs:267-270:
```
        // A public endpoint belongs to an outbound external link, which only
        // `CreateOutboundExternalLink` mints; no link this store holds has one, so
        // nothing is reported for it rather than a synthesised address.
        public_endpoint: None,
```
The crate's own state says the opposite, twice. src/state/external_links.rs:37-47 — the record IS held, with its endpoint:
```
pub struct OutboundExternalLink {
    pub link_id: String,
    pub gateway_id: String,
    pub status: String,
...
    /// The address outside the fabric this link carries traffic to, as the caller
    /// stated it.
    pub public_endpoint: String,
```
and src/state.rs:593-599 — it is one of the gateway's links:
```
    /// Count the stored links `gateway_id` is an end of, by status.
    ///
    /// A link to an endpoint OUTSIDE the fabric is one of that gateway's links
    /// too — `RequesterGatewayStatus`'s reader documents `totalLinksCount` as
    /// "The total count of links for the requester gateway" — so it counts here
    /// exactly as a link between two gateways does.
```
The create that persists it is implemented and inserts into that map — src/ops/create_outbound_external_link.rs:311-313:
```
        store
            .outbound_external_links
            .insert((input.gateway_id, link_id.clone()), link);
```
Test coverage: the existing suite pins the counting half but never lists an external link. tests/all/gateway_link_lifecycle.rs:2191-2205:
```
        // The gateway's own reader counts it: an external link is one of that
        // gateway's links, and `totalLinksCount` is derived by scanning them.
...
        assert_eq!(
            gateway["totalLinksCount"],
            json!(1),
            "case {case:?}: expected the gateway to count its one outbound external link, got {:?}",
```
No test calls `ListLinks` after a `CreateOutboundExternalLink` (every `fabric.list_links(...)` call sits at tests/all/gateway_link_lifecycle.rs:1701-1859, above the outbound-external-link section that starts at line 1885), so nothing pins the omission with a stated reason.

## Steps to Reproduce

1. POST /requester-gateway with a clientToken, vpcId, subnetIds, securityGroupIds — note the returned gatewayId R.
2. POST /requester-gateway/R/outbound-external-link with a clientToken, a publicEndpoint such as "https://bidder.example.com" and logSettings — note the returned linkId L and status PENDING_CREATION.
3. GET /requester-gateway/R — observe `totalLinksCount: 1`: the service counts L as one of R's links.
4. GET /requester-gateway/R/outbound-external-link/L — observe 200 with the stored `publicEndpoint`, confirming the record is persisted.
5. GET /gateway/R/links/ — observed: `links: []` (no entry for L), so the listing documented to return "all links for the specified gateways" reports none of them, and `publicEndpoint` is unreachable in any ListLinks entry.
6. Expected: L appears in the ListLinks page with its stored status and its `publicEndpoint`, consistent with the `totalLinksCount` the same gateway reports.
