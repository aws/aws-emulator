Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: f60279ee1b26e62c04ba7bfaecc753f1127e63e1

# ListResourceSetResources reports no AccountId for a member whose ARN leaves the account field empty, although the discovery inventory reports that same resource's owner

_Filed: 2026-10-29T09:03:49Z_
_Operation: ListResourceSetResources | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `f60279ee1b26e62c04ba7bfaecc753f1127e63e1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`Resource$AccountId` is "The Amazon Web Services account ID that the associated resource belongs to". The handler derives it exclusively from the account segment of the member's own ARN and reports nothing when that segment is empty.

The discovery inventory in the same crate derives the owner of exactly those ARNs differently: for an ARN whose owning service leaves the account field empty (an API Gateway stage, `arn:aws:apigateway:<region>::/restapis/<id>/stages/<name>`, or a global ARN such as CloudFront's or S3's) it reports the emulator's account as the owner, with a comment saying so, and `ListDiscoveredResources` serves it as `DiscoveredResource$AccountId`. The membership classifier reads the same empty field the same way — "it carries no account to weigh and the resource belongs to whichever account of the organization holds it" — which is why such an item is associable at all.

So the two surfaces of ONE resource disagree: a client that calls `ListDiscoveredResources` (AccountId = 123456789012), associates the resource with `BatchAssociateResource` (accepted), then calls `ListResourceSetResources` sees the owner disappear. The crate states the inventory and the membership operations make "the same readings", and this is a reading where they do not.

## Evidence

The membership reader's derivation — services/service-com-amazonaws-fms-awsfms-20180101/src/ops/list_resource_set_resources.rs:274-287:
```
/// The account a member resource belongs to (`Resource$AccountId`: "the Amazon
/// Web Services account ID that the associated resource belongs to"), read off
/// the account field of the resource's own ARN.
///
/// Derived rather than stored, so it cannot disagree with the URI it describes.
/// `None` when that field carries no `AWSAccountId` — a global service's ARN
/// leaves it empty — because the modelled member is optional and its pattern
/// (`^[0-9]+$`) admits nothing else.
fn resource_account_id(uri: &str) -> Option<String> {
    crate::state::parse_arn(uri)
        .map(|arn| arn.account)
        .filter(|account| crate::state::is_account_id(account))
        .map(str::to_string)
}
```

The sibling derivation for the SAME ARNs — src/state.rs:3607-3637 (`api_gateway_stages`):
```
/// An API Gateway ARN carries an EMPTY account field
/// (`arn:aws:apigateway:<region>::/restapis/<id>/stages/<name>`, built by that
/// service's own `state::service_arn`), so the owning account is this
/// emulator's rather than the URI's.
...
                name: stage.clone(),
                account: emulator_owner(),
```
which `ListDiscoveredResources` reports — src/state.rs:3711-3719:
```
            DiscoveredResource {
                uri: Some(held.uri),
                account_id: Some(held.account),
```

The crate's claim that the two sides read an ARN the same way — src/state.rs:2839-2848 (`resource_set_item_failure_reason`):
```
/// An ARN field a service LEAVES EMPTY names no value to disagree with, so it is
/// no reason to refuse the item: ... and an account-less service's ARN carries no
/// account segment to weigh against the organization's accounts.
...
/// Those are the same readings the discoverable inventory makes
/// ([`in_region`], [`discovered_resources`]), which is what keeps a resource
/// reported as "available to be associated with a resource set" one these
/// operations actually take.
```
and src/state.rs:2894-2901:
```
/// Whether `arn` names a resource of one of `accounts`: ... or it carries no account at all —
/// an account-less service's ARN (API Gateway's, for one) leaves that field empty, so it carries
/// no account to weigh and the resource belongs to whichever account of the organization holds it.
```

No test pins the omission: tests/all/associable_discovered_resources.rs:169-254 associates exactly these account-less ARNs and reads the membership back, but asserts only `URI` (line 249: `.map(|item| item["URI"].as_str()...)`), and tests/all/resource_set_resources.rs:87-99 builds every expected pair from URIs that all carry an account, so its `.filter(|account| !account.is_empty())` branch is never exercised.

## Steps to Reproduce

1. AssociateAdminAccount in the emulator.
2. Create an API Gateway REST API with a `prod` stage (an account-less ARN) through the API Gateway service.
3. Call ListDiscoveredResources with `MemberAccountIds: ["<emulator account>"]`, `ResourceType: "AWS::ApiGateway::Stage"` and note the item: `URI` = `arn:aws:apigateway:<region>::/restapis/<id>/stages/prod`, `AccountId` = the emulator account.
4. PutResourceSet with `ResourceTypeList: ["AWS::ApiGateway::Stage"]`, then BatchAssociateResource that URI — it is accepted with empty `FailedItems`.
5. Call ListResourceSetResources for the set: the item carries the same `URI` but no `AccountId`, so the owner the inventory reported for the same resource is gone.
