Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 026158241d903674989907608971730578a493f7

# The minted ARN interpolates the credential-scope region raw, so an ARN the service itself published is refused ResourceNotFoundException by the tagging operations

_Filed: 2026-11-26T13:47:57Z_
_Operation: TagResource | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `026158241d903674989907608971730578a493f7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every PCS ARN is built by interpolating `request_region(ctx)` — the region field of the caller's SigV4 credential scope, i.e. caller-supplied text — directly into `arn:aws:pcs:<region>:<account>:<resource>`. The tagging operations then resolve that same ARN back to a record by POSITION: `arn_cluster_id` takes the 6th colon-separated field and requires its first `/`-segment to be `cluster`.

Those two rules contradict each other as soon as the signed region contains a colon (the scope parser only forbids `/` and emptiness). A CreateCluster signed with scope region `us-east-1:x` answers 200 and publishes `cluster.arn = arn:aws:pcs:us-east-1:x:000000000000:cluster/pcs_<id>`; that string still satisfies `resourceArn`'s modelled `@pattern ^arn:aws.*:pcs:.*:[0-9]{12}:.*/[a-z0-9_\-]+$`, so it reaches the handler, where `splitn(6, ':').nth(5)` yields `000000000000:cluster/pcs_<id>`, the first segment is `000000000000:cluster` rather than `cluster`, and `arn_cluster_id` answers `None`. TagResource, UntagResource and ListTagsForResource therefore all refuse the cluster's own published ARN with `ResourceNotFoundException` ("Resource cannot be found.") while `GetCluster` still resolves the cluster — stored tags become unreachable by the only key the tagging surface accepts, and the quota refusal built from `arn_resource` degrades the resource kind to `resource`.

The crate already states the rule this path skips and already owns the helper for it: `state/ec2_subnets.rs` reads the very same signed region through `service_common::arn::region_label` BEFORE it goes anywhere that is later parsed, "instead of being interpolated as given", precisely because "a value carrying a host or path delimiter" changes the meaning of the string it lands in. The ARN builders — `cluster_arn`, `queue_arn`, `compute_node_group_arn` — interpolate it as given into a value this crate parses by colon position.

## Evidence

CRATE SIDE A — the signed region is interpolated into the ARN as received (src/state.rs:1198-1211):
```
/// The region the request was signed for. PCS resources are regional, so every ARN the
/// service mints carries it; a request that carries no SigV4 credential scope gets
/// [`DEFAULT_REGION`].
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .filter(|region| !region.is_empty())
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// `arn:aws:pcs:<region>:<account>:cluster/<cluster id>`.
pub fn cluster_arn(region: &str, cluster_id: &str) -> String {
    ::engine::emulator_arn("pcs", region, &format!("cluster/{cluster_id}"))
}
```
used by the create path verbatim (src/ops/create_cluster.rs:286 and :381):
```
        let region = crate::state::request_region(ctx);
...
            arn: crate::state::cluster_arn(&region, &id),
```
and `emulator_arn` does no normalisation (vendor/server/src/lib.rs:170-172):
```
pub fn emulator_arn(service: &str, region: &str, resource: &str) -> String {
    format!("arn:aws:{service}:{region}:{EMULATOR_ACCOUNT_ID}:{resource}")
}
```

CRATE SIDE B — the same crate states that this region is caller-supplied text and must NOT be interpolated as given, and routes it through an existing helper (src/state/ec2_subnets.rs:68-73, :83):
```
/// The Region a request is signed for is CALLER-SUPPLIED text, and here it ends up in an
/// endpoint URL's authority rather than in a response, so it is read as a Region label
/// (`service_common::arn::region_label`, the tree's one reading of that grammar) instead of
/// being interpolated as given: a value carrying a host or path delimiter would name an
/// authority the caller chose rather than the peer this emulator asks.
...
    let label = ::service_common::arn::region_label(region)?;
```

CRATE SIDE C — the tagging lookup parses that ARN by colon position, so a colon in the region breaks it (src/state.rs:1917-1924):
```
pub fn arn_cluster_id(arn: &str) -> Option<&str> {
    let resource_part = arn.splitn(6, ':').nth(5)?;
    let mut segments = resource_part.split('/');
    if segments.next()? != CLUSTER.resource_type {
        return None;
    }
    segments.next()
}
```
and that `None` is the whole answer of the tag write path (src/state.rs:852-854):
```
pub fn tags_by_arn_mut(&mut self, arn: &str) -> Option<&mut HashMap<String, String>> {
    let (kind, resource_id) = arn_resource(arn);
    let cluster = self.clusters.get_mut(arn_cluster_id(arn)?)?;
```
which the handler turns into the not-found refusal (src/ops/tag_resource.rs:170-172):
```
        let Some(stored) = state.tags_by_arn_mut(&input.resource_arn) else {
            return Err(not_found().into());
        };
```

REACHABILITY — the scope region is any non-empty field with no `/`; a colon is accepted (vendor/server/src/lib.rs:1974-1987):
```
    let mut fields = credential.split('/');
    let (Some(access_key_id), Some(date), Some(region), Some(service), Some("aws4_request"), None) = (
...
    if access_key_id.is_empty() || date.is_empty() || region.is_empty() || service.is_empty() {
        return None;
    }
```

TEST COVERAGE — every tagging test signs with one well-formed region, so nothing pins this (tests/all/resource_tags.rs:35-40):
```
const SIGNED_FOR_PCS: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/pcs/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=deadbeef";

/// The region [`SIGNED_FOR_PCS`] signs for, which every ARN in these tests carries.
const REGION: &str = "us-east-1";
```
`tests/all/resource_tags.rs:584` (`the_tagging_operations_refuse_an_arn_no_resource_carries`) covers an ARN of ANOTHER region that names nothing — not an ARN the service itself minted and then cannot resolve.

## Steps to Reproduce

1. POST / with header `x-amz-target: AWSParallelComputingService.CreateCluster` and an Authorization header whose credential scope region carries a colon: `Credential=AKIDEXAMPLE/20260101/us-east-1:x/pcs/aws4_request`.
2. Body: {"clusterName":"MyCluster","scheduler":{"type":"SLURM","version":"24.11"},"size":"SMALL","networking":{"subnetIds":["subnet-0123456789abcdef0"]}} — answered 200; note `cluster.arn` = `arn:aws:pcs:us-east-1:x:000000000000:cluster/pcs_<id>`.
3. Call TagResource with that exact resourceArn and {"k":"v"} (the ARN satisfies the modelled @pattern, so it reaches the handler).
4. Observed: the client-error status with ResourceNotFoundException "Resource cannot be found." — `arn_cluster_id` (src/state.rs:1917-1924) read `000000000000:cluster` as the first resource segment and answered None.
5. Call ListTagsForResource and UntagResource with the same ARN: both refused the same way, while GetCluster(clusterIdentifier="MyCluster") still answers 200 with that very ARN.
6. Expected, per the crate's own rule for this caller-supplied region (src/state/ec2_subnets.rs:68-73): the region is read as a region label before it is interpolated, so every ARN the service publishes is one its own ARN-keyed lookups resolve.
