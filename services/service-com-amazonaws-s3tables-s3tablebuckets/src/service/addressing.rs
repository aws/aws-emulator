// SPDX-License-Identifier: CC0-1.0

//! What a request ADDRESSES: the members of its input that name the resource it
//! acts on, read ONCE per request.
//!
//! Two decisions are taken in front of every handler of this service — whether
//! the ARNs the request names a resource by belong to the Region this endpoint
//! serves ([`super::addressed_region`]), and whether the policies in force let
//! the caller perform the action ([`super::authorization`]) — and both are about
//! the same thing: which resource the request addresses. That is read off the
//! request's own BUILT input through the tree's one rendering of a shape as a
//! document ([`::service_common::management_events::recorded_shape`]), so
//! neither decision keeps a table of URI templates and neither needs a list of
//! which operation takes which member.
//!
//! The rendering lives here so it happens once for both, and so the two member
//! lists below have one place to be stated: a member added to the model is added
//! here and both decisions see it.

use ::service_common::management_events::{recorded_member_name, recorded_shape, WithheldFromRecord};

/// The members no addressing decision reads a resource out of, and which are
/// left out of the document: the values a write CARRIES — a policy document, a
/// table's Iceberg metadata, a tag map, a configuration value. None of them
/// names the resource the request acts on, and a caller controls their size.
const PAYLOAD_MEMBERS: &[&str] = &[
    "resourcePolicy",
    "metadata",
    "tags",
    "tagKeys",
    "value",
    "configuration",
    "encryptionConfiguration",
    "storageClassConfiguration",
];

/// The input members of this model that address a resource by ARN
/// (`smithy_query(query="operation")`), in the order a request narrows what it
/// addresses:
///
/// * `resourceArn` — the tagging operations, which address a table bucket or a
///   table by ARN ("The tagged resource can be a table bucket or a table");
/// * `tableArn` — a table's own ARN, which the replication and record-expiration
///   operations address one by;
/// * `tableARN` — the same thing under the other spelling this model uses for
///   it, which the migration operations address a destination table by;
/// * `tableBucketARN` — the table bucket every other operation of this service
///   addresses, or addresses a table inside, by.
pub(super) const ARN_MEMBERS: &[&str] =
    &["resourceArn", "tableArn", "tableARN", "tableBucketARN"];

/// The addressing members of ONE request's input.
pub(super) struct Addressing {
    /// The input as a document, with the payload members above withheld.
    document: ::serde_json::Value,
}

impl Addressing {
    /// Read `input`, an operation's built input of shape `schema`.
    pub(super) fn of(
        input: &dyn ::aws_smithy_schema::serde::SerializableStruct,
        schema: &::aws_smithy_schema::Schema,
    ) -> Self {
        Self {
            document: recorded_shape(
                input,
                schema,
                WithheldFromRecord {
                    omitted: PAYLOAD_MEMBERS,
                    ..WithheldFromRecord::default()
                },
            ),
        }
    }

    /// The text `member` carries, and `None` when the input carries no such
    /// member or carries it empty — neither of which addresses anything.
    pub(super) fn text(&self, member: &str) -> Option<&str> {
        self.document
            .get(recorded_member_name(member))
            .and_then(::serde_json::Value::as_str)
            .filter(|value| !value.is_empty())
    }

    /// Every ARN this request addresses a resource by, in [`ARN_MEMBERS`] order.
    pub(super) fn arns(&self) -> impl Iterator<Item = &str> + '_ {
        ARN_MEMBERS.iter().filter_map(|member| self.text(member))
    }
}
