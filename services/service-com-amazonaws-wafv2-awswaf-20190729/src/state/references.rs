// SPDX-License-Identifier: CC0-1.0

//! A wafv2 resource ARN: what each of its segments says about the resource it names, and
//! whether a request can use a reference it carries at all.
//!
//! An ARN names a resource completely — the account that owns it, the region whose
//! endpoint serves it, the namespace its `Scope` names, its kind, its name and its id —
//! and [`crate::state::resource_arn`] is what mints every one of those segments. These
//! readers are the other direction: they take a value a REQUEST carried and say what it
//! names, so an operation handed an ARN can judge it before it resolves anything (see
//! `crate::ops::get_web_acl`).
//!
//! One of those readings is a judgement in its own right: a reference naming a namespace
//! other than the one the request addresses is one the request cannot use
//! ([`reference_of_another_namespace`]).
//!
//! Each reader answers `None` for a value that is no wafv2 resource ARN at all, so a
//! caller that needs to tell "not an ARN" from "an ARN of the wrong X" asks
//! [`is_wafv2_resource_arn`] first.

use super::*;

/// Whether `value` is a reference to a resource of the namespace a request naming
/// `scope`, made at an endpoint in `request_region`, addresses.
///
/// A REGIONAL resource lives in the region of the endpoint that created it, and a
/// CLOUDFRONT-scoped one in the single global namespace reached through one endpoint
/// ([`namespace_region`]) — which is the pair [`resource_arn`] mints an ARN's region and
/// `global`/`regional` segments from. So a reference belongs to the namespace a request
/// addresses exactly when both segments match what a resource created in that namespace
/// would carry.
pub fn addresses_namespace(value: &str, scope: &str, request_region: &str) -> bool {
    arn_resource_region(value) == Some(namespace_region(scope, request_region))
        && arn_scope(value) == Some(scope)
}

/// Whether `value` is a resource reference of a namespace OTHER than the one a request
/// naming `scope` at an endpoint in `request_region` addresses.
///
/// A WAFv2 resource lives in the namespace its own ARN names, and an endpoint cannot act
/// on another one — so such a reference is one the request cannot use: "your request
/// references an ARN that is malformed, or corresponds to a resource with which [it]
/// can't be [used]", `WAFInvalidParameterException`'s own doc
/// ([`invalid_arn_exception`]). Both halves of the comparison come from the REQUEST — the
/// ARN it carried, and the `Scope` it named at the endpoint it reached — so the
/// judgement needs no store, and whether the referenced resource EXISTS is a separate
/// question only the store can answer. `GetWebACL` makes the same judgement about the
/// ARN it addresses a web ACL by (`crate::ops::get_web_acl`).
///
/// `false` for a value that is no wafv2 resource ARN at all: what KIND of value it is,
/// and what kind of resource it names, are separate readings of the same reference
/// ([`is_wafv2_resource_arn`], [`arn_resource_kind`]) that each member judges for
/// itself.
pub fn reference_of_another_namespace(value: &str, scope: &str, request_region: &str) -> bool {
    is_wafv2_resource_arn(value) && !addresses_namespace(value, scope, request_region)
}

/// `true` when `value` has the shape of a wafv2 resource ARN:
/// `arn:<partition>:wafv2:<region>:<account>:<global|regional>/<kind>/<name>/<id>`.
///
/// `GetWebACL` separates the two ARN failures the model names: an ARN that is
/// malformed is `WAFInvalidParameterException` ("Your request references an ARN
/// that is malformed", from that error's own doc), while a well-formed ARN naming
/// nothing stored is `WAFNonexistentItemException`.
pub fn is_wafv2_resource_arn(value: &str) -> bool {
    let mut segments = value.split(':');
    let (
        Some("arn"),
        Some(partition),
        Some("wafv2"),
        Some(_region),
        Some(_account),
        Some(resource),
        None,
    ) = (
        segments.next(),
        segments.next(),
        segments.next(),
        segments.next(),
        segments.next(),
        segments.next(),
        segments.next(),
    )
    else {
        return false;
    };
    let resource_parts: Vec<&str> = resource.split('/').collect();
    !partition.is_empty()
        && resource_parts.len() == 4
        && matches!(resource_parts[0], "global" | "regional")
        && resource_parts[1..].iter().all(|part| !part.is_empty())
}

/// The resource-kind segment of a wafv2 resource ARN (`webacl`, `ipset`,
/// `rulegroup`, …), or `None` when `value` is not one at all.
///
/// The ARN layout is the one [`is_wafv2_resource_arn`] pins:
/// `arn:<partition>:wafv2:<region>:<account>:<global|regional>/<kind>/<name>/<id>`,
/// so the kind is the second slash-separated segment of the resource part.
pub fn arn_resource_kind(value: &str) -> Option<&str> {
    if !is_wafv2_resource_arn(value) {
        return None;
    }
    value.split(':').nth(5)?.split('/').nth(1)
}

/// The REGION segment of a wafv2 resource ARN, or `None` when `value` is not one at all.
///
/// Same layout as [`arn_resource_kind`], so the region is the ARN's fourth
/// colon-separated field. A resource is reached through the endpoint of the region its
/// ARN names — [`resource_arn`] mints that segment from the scope and the endpoint the
/// create arrived at — so this is what a request addressing a resource BY ARN is judged
/// against: an endpoint cannot act on an ARN naming another region's namespace.
pub fn arn_resource_region(value: &str) -> Option<&str> {
    if !is_wafv2_resource_arn(value) {
        return None;
    }
    value.split(':').nth(3)
}

/// The ACCOUNT segment of a wafv2 resource ARN, or `None` when `value` is not one at
/// all.
///
/// Same layout as [`arn_resource_kind`], so the account is the ARN's fifth
/// colon-separated field. A resource belongs to the account that created it, and WAF
/// resolves an ARN for the CALLER's account: an ARN naming another account names an
/// account WAF cannot resolve for this caller (see
/// [`crate::scaffolded::UNRECOGNIZED_CLIENT_CODE`]).
pub fn arn_account(value: &str) -> Option<&str> {
    if !is_wafv2_resource_arn(value) {
        return None;
    }
    value.split(':').nth(4)
}

/// `true` when `value` is a wafv2 resource ARN whose account segment is not the
/// account this emulator serves — the account every call it answers is made for.
pub fn arn_names_another_account(value: &str) -> bool {
    arn_account(value).is_some_and(|account| account != ::engine::emulator_account_id())
}
