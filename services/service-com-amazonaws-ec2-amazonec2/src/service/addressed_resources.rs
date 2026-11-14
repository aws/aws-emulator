// SPDX-License-Identifier: CC0-1.0

//! The resources an EC2 request ADDRESSES, as the ARNs IAM judges its action against —
//! read from the request itself, ahead of any handler.
//!
//! An EC2 action that supports resource-level permissions is authorized on the resources
//! the request names ("Actions, resources, and condition keys for Amazon EC2", IAM Service
//! Authorization Reference, which lists the resource types each action is judged on), and
//! IAM names resources by ARN while an EC2 request names them by ID. This is the one
//! reading that turns the ids a request carries into those ARNs, so the authorization gate
//! ([`super::authorization`]) can decide such an action and a refusal can say which
//! resource it was refused ON.
//!
//! ## How an id becomes an ARN
//!
//! Every resource of this service is addressed as
//! `arn:<partition>:ec2:<region>:<account>:<resource type>/<id>` (the same form every ARN
//! this crate reports is built in, through `engine`'s one builder), and an EC2 id is
//! `<prefix>-<hexadecimal>`: the prefix fixes the resource type and the hexadecimal part
//! carries no type information at all. So the reading is a lookup of the prefix in
//! [`RESOURCE_TYPE_BY_ID_PREFIX`], with the id's last `-`-separated segment required to be
//! one of the two hexadecimal lengths EC2 issues ([`SHORT_ID_HEX_DIGITS`],
//! [`LONG_ID_HEX_DIGITS`]) — which is what keeps a LONGER id form apart from the shorter
//! one it extends: an attachment id (`eni-attach-…`) reads as the prefix `eni-attach`,
//! which is not a resource type, rather than as a network interface.
//!
//! The ids come out of the request's own parameter list, which an ec2Query request carries
//! as an `application/x-www-form-urlencoded` body — read through `form_urlencoded`, the
//! crate that decodes that media type, rather than by splitting the bytes here.
//!
//! A value that is not an id of a kind the table carries addresses nothing here: the gate
//! then decides the request without naming a resource, which withholds no grant a
//! resource-scoped statement gives (see [`super::authorization`]). That is the deliberate
//! direction of every gap in this reading — it can only fail to find a resource, never
//! invent one.

use std::collections::BTreeSet;

use aws_smithy_runtime_api::http::Request as RtRequest;

// The one reading of a request's own parameter list, and the bound every gate of this front
// door reads it under: the parse here and the one authorization decision taken per resource
// found are both capped by it.
use super::request_parameters::{request_parameters, MAX_PARAMETERS_READ};

/// How many hexadecimal digits the SHORTER id form EC2 issues carries ("Resource IDs",
/// Amazon EC2 User Guide: the earlier 8-character form).
const SHORT_ID_HEX_DIGITS: usize = 8;

/// How many hexadecimal digits the LONGER id form EC2 issues carries (the 17-character
/// form every resource type now gets).
const LONG_ID_HEX_DIGITS: usize = 17;

/// The resource type of each id prefix this reading knows, as the ARN of that resource
/// spells it ("Resource types defined by Amazon EC2", IAM Service Authorization Reference).
///
/// Keyed by the id's prefix — everything before the hexadecimal part — so a prefix that
/// EXTENDS another (`tgw-attach` beside `tgw`) is its own entry and never read as the
/// shorter one. A kind whose ARN does not carry this account or is addressed by NAME rather
/// than by id (an AMI, a key pair, a placement group) is deliberately absent: its ARN
/// cannot be built from the id alone.
pub const RESOURCE_TYPE_BY_ID_PREFIX: &[(&str, &str)] = &[
    ("acl", "network-acl"),
    ("cr", "capacity-reservation"),
    ("dopt", "dhcp-options"),
    ("eipalloc", "elastic-ip"),
    ("eni", "network-interface"),
    ("fl", "vpc-flow-log"),
    ("i", "instance"),
    ("igw", "internet-gateway"),
    ("lt", "launch-template"),
    ("nat", "natgateway"),
    ("pcx", "vpc-peering-connection"),
    ("rtb", "route-table"),
    ("sg", "security-group"),
    ("snap", "snapshot"),
    ("subnet", "subnet"),
    ("tgw", "transit-gateway"),
    ("tgw-attach", "transit-gateway-attachment"),
    ("tgw-rtb", "transit-gateway-route-table"),
    ("vol", "volume"),
    ("vpc", "vpc"),
    ("vpce", "vpc-endpoint"),
    ("vpce-svc", "vpc-endpoint-service"),
];

/// The ARNs of the resources `request` addresses, in the order its parameters carry them
/// and with each ARN named once.
///
/// `region` is the Region the request was signed for, which is the Region its resources are
/// in — an EC2 endpoint serves one Region.
#[must_use]
pub fn addressed_by(request: &RtRequest, region: &str) -> Vec<String> {
    let mut arns = Vec::new();
    let mut seen = BTreeSet::new();
    // Only the VALUES: which member a value was sent under differs per operation, while the
    // id's own prefix says what kind of resource it names, so the reading does not depend on
    // an operation's parameter names.
    let values = request_parameters(request, MAX_PARAMETERS_READ).map(|(_, value)| value);
    for value in values {
        let Some(resource_type) = resource_type_of(&value) else {
            continue;
        };
        let arn = ::engine::emulator_arn("ec2", region, &format!("{resource_type}/{value}"));
        if seen.insert(arn.clone()) {
            arns.push(arn);
        }
    }
    arns
}

/// The resource type `value` is an id of, or `None` when it is not an EC2 resource id of a
/// kind [`RESOURCE_TYPE_BY_ID_PREFIX`] carries.
#[must_use]
pub fn resource_type_of(value: &str) -> Option<&'static str> {
    let (prefix, hex) = value.rsplit_once('-')?;
    if hex.len() != SHORT_ID_HEX_DIGITS && hex.len() != LONG_ID_HEX_DIGITS {
        return None;
    }
    if !hex.bytes().all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f')) {
        return None;
    }
    RESOURCE_TYPE_BY_ID_PREFIX
        .iter()
        .find(|(known, _)| *known == prefix)
        .map(|(_, resource_type)| *resource_type)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RULE: the prefix of an EC2 id fixes the resource type its ARN carries, and a value
    /// that is not an id of a kind the table carries names no resource — including a LONGER
    /// id form that extends a listed prefix.
    #[test]
    fn an_id_reads_as_the_resource_type_its_prefix_names() {
        // (the value, the resource type its ARN carries)
        let rows: &[(&str, Option<&str>)] = &[
            ("subnet-0123456789abcdef0", Some("subnet")),
            // The shorter form EC2 issued before the long ids, same kind.
            ("subnet-1a2b3c4d", Some("subnet")),
            ("vpc-0123456789abcdef0", Some("vpc")),
            ("sg-0123456789abcdef0", Some("security-group")),
            ("eni-0123456789abcdef0", Some("network-interface")),
            ("i-0123456789abcdef0", Some("instance")),
            ("vol-0123456789abcdef0", Some("volume")),
            ("snap-0123456789abcdef0", Some("snapshot")),
            ("igw-0123456789abcdef0", Some("internet-gateway")),
            ("nat-0123456789abcdef0", Some("natgateway")),
            ("eipalloc-0123456789abcdef0", Some("elastic-ip")),
            ("tgw-0123456789abcdef0", Some("transit-gateway")),
            // A prefix that EXTENDS a listed one is its own entry…
            (
                "tgw-attach-0123456789abcdef0",
                Some("transit-gateway-attachment"),
            ),
            // …and one the table does not carry names nothing rather than reading as the
            // shorter prefix it extends.
            ("eni-attach-0123456789abcdef0", None),
            ("rtbassoc-0123456789abcdef0", None),
            // Not an id at all.
            ("10.0.0.0/16", None),
            ("subnet", None),
            ("subnet-", None),
            // The right prefix with the wrong body: not hexadecimal, and neither published
            // length.
            ("subnet-0123456789abcdefg", None),
            ("subnet-0123", None),
            ("subnet-0123456789abcdef01", None),
            // A kind addressed by name or whose ARN carries no account.
            ("ami-0123456789abcdef0", None),
        ];
        for (value, expected) in rows {
            let read = resource_type_of(value);
            assert_eq!(
                read, *expected,
                "case {value:?}: expected {expected:?}, got {read:?}"
            );
        }
    }

    /// RULE: the ARNs come from the request's own parameter VALUES, each named once and in
    /// the order the request carries them, whatever member each was sent under — and a value
    /// is read as the text its percent-encoding stands for.
    #[test]
    fn the_arns_are_the_requests_own_ids_in_order() {
        let region = "us-west-2";
        let body = "Action=CreateNetworkInterface&Version=2016-11-15\
                    &SubnetId=subnet-0123456789abcdef0&SecurityGroupId.1=sg-0123456789abcdef0\
                    &SecurityGroupId.2=sg-0123456789abcdef0&Description=an+interface\
                    &PrivateIpAddress=10.0.0.5";
        let request = {
            let mut request = ::aws_smithy_runtime_api::http::Request::new(
                ::aws_smithy_types::body::SdkBody::from(body),
            );
            request
                .set_uri(format!("https://ec2.{region}.amazonaws.com/"))
                .expect("a well-formed URI");
            request
        };
        let addressed = addressed_by(&request, region);
        assert_eq!(
            addressed,
            vec![
                ::engine::emulator_arn("ec2", region, "subnet/subnet-0123456789abcdef0"),
                ::engine::emulator_arn("ec2", region, "security-group/sg-0123456789abcdef0"),
            ],
            "case \"an interface create\": expected the subnet and the group once each, got \
             {addressed:?}"
        );

        // A value a client percent-encoded, and one carrying text that is not ASCII at all:
        // the encoding is resolved before the id is read, and no value is refused or indexed
        // into.
        let body = "Action=CreateTags&ResourceId.1=subnet%2D0123456789abcdef0\
                    &Tag.1.Key=%C3%A9quipe&Tag.1.Value=%C3&ResourceId.2=vol-1a2b3c4d";
        let request = {
            let mut request = ::aws_smithy_runtime_api::http::Request::new(
                ::aws_smithy_types::body::SdkBody::from(body),
            );
            request
                .set_uri(format!("https://ec2.{region}.amazonaws.com/"))
                .expect("a well-formed URI");
            request
        };
        let addressed = addressed_by(&request, region);
        assert_eq!(
            addressed,
            vec![
                ::engine::emulator_arn("ec2", region, "subnet/subnet-0123456789abcdef0"),
                ::engine::emulator_arn("ec2", region, "volume/vol-1a2b3c4d"),
            ],
            "case \"a percent-encoded tag write\": expected both ids, got {addressed:?}"
        );
    }
}
