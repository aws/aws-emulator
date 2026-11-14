// SPDX-License-Identifier: CC0-1.0

//! The refusals WAF answers a request it will not act on at all: one whose ARN names a
//! resource of another kind, and one that leaves out information the call cannot be made
//! without.
//!
//! Both are stated with an error code this service's Smithy model does not declare, so
//! they travel on [`crate::scaffolded::Scaffolded::access_denied`] rather than as a
//! modelled variant; what each one SAYS is here, beside the wording of every other
//! WAFv2 refusal.
//!
//! The two messages differ from every modelled exception's in that they name the REQUEST
//! rather than the resource: the request shape as the model declares it, and — for an
//! omission — its members with the values the request carried. Nothing else from the
//! request enters either sentence.

use super::member_path;

/// The sentence WAF answers an ARN that is well formed but names a resource of a kind
/// this request cannot address: the fixed text `Invalid ARN provided for `, the request
/// shape's name, a space, and the ARN exactly as the request supplied it.
///
/// `request_shape` is the operation's input shape as the model names it
/// (`smithy_query(query="operation",operation="GetWebACL")` → `GetWebACLRequest`), which
/// is how WAF names the request in this class of refusal.
pub fn invalid_arn_for_request(request_shape: &str, arn: &str) -> String {
    format!("Invalid ARN provided for {request_shape} {arn}")
}

/// The sentence WAF answers a request that omits information it cannot act without: the
/// fixed text `Critical information is missing in your request: `, then the request
/// shape rendered as `<Shape>(<member>=<value>, …).`
///
/// Each member is named in lowerCamel — the same casing a validation clause spells a
/// member path in ([`member_path`]), so `ARN` reads `aRN` — in the order the model
/// declares them, and a member the request did not carry is rendered `null`. Only values
/// the request itself supplied appear; nothing is taken from the store.
pub fn critical_information_missing(request_shape: &str, members: &[(&str, Option<&str>)]) -> String {
    let rendered: Vec<String> = members
        .iter()
        .map(|(member, value)| {
            format!(
                "{}={}",
                member_path(member),
                value.unwrap_or(ABSENT_MEMBER_VALUE)
            )
        })
        .collect();
    format!(
        "Critical information is missing in your request: {request_shape}({}).",
        rendered.join(", ")
    )
}

/// How a member the request did not carry is rendered in
/// [`critical_information_missing`].
const ABSENT_MEMBER_VALUE: &str = "null";

#[cfg(test)]
mod tests {
    use super::*;

    /// The rendering is the request's, member by member: the model's order, lowerCamel
    /// names (so an all-capitals member keeps its tail), `null` for what the request did
    /// not carry, and the supplied value for what it did.
    #[test]
    fn an_omission_names_the_request_shape_and_every_member() {
        assert_eq!(
            critical_information_missing(
                "ListWebACLsRequest",
                &[("Scope", None), ("NextMarker", None), ("Limit", None)]
            ),
            "Critical information is missing in your request: \
             ListWebACLsRequest(scope=null, nextMarker=null, limit=null)."
        );
        assert_eq!(
            critical_information_missing(
                "GetWebACLRequest",
                &[
                    ("Name", None),
                    ("Scope", Some("REGIONAL")),
                    ("Id", None),
                    ("ARN", None),
                ]
            ),
            "Critical information is missing in your request: \
             GetWebACLRequest(name=null, scope=REGIONAL, id=null, aRN=null)."
        );
    }

    /// The ARN refusal echoes the value the request supplied, whatever it holds.
    #[test]
    fn an_unusable_arn_is_echoed_after_the_request_shape() {
        assert_eq!(
            invalid_arn_for_request(
                "GetWebACLRequest",
                "arn:aws:wafv2:us-west-2:000000000000:regional/ipset/set/id"
            ),
            "Invalid ARN provided for GetWebACLRequest \
             arn:aws:wafv2:us-west-2:000000000000:regional/ipset/set/id"
        );
    }
}
