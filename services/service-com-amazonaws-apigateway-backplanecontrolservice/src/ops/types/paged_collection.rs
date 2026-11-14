// SPDX-License-Identifier: CC0-1.0

//! The `position`/`limit` page envelope every paginated collection of this service
//! answers in.
//!
//! HAND-AUTHORED, not scaffolded.
//!
//! # The answer carries a member the on-disk model does not declare
//!
//! Each collection shape of this service declares the page and the cursor and nothing
//! else — `DocumentationParts` is `items` + `position`
//! (`smithy_query(query="shape", shape="DocumentationParts")`), and `ApiKeys`,
//! `BasePathMappings` and the rest are the same two members — while the service's own
//! answer also ECHOES the page size the request asked for, as `limit`, beside the cursor
//! the collection carries on at. A request that asked for no page size is told none (the
//! member states what the caller requested, not the default the collection fell back
//! to), and so is a request whose page ends the collection: the echo travels with the
//! cursor, so an answer that reports no cursor reports no page size either
//! (`crate::state::Page::echoed_limit`).
//!
//! `#[smithy_macros::smithy(shape = …)]` derives a structure's schema and serializer
//! from the model's member list and admits exactly that list, so the echo cannot be a
//! field of the generated collection shapes. It is carried here instead, as a wrapper
//! that serializes the modelled collection it holds and then writes the one unmodelled
//! member — so each collection shape keeps the derive, its members and their order, and
//! a later model refresh that declares `limit` only has to move the field onto that
//! shape and delete this module.
//!
//! Deliberately absent, for the same reason the derive's own output would be inert here:
//! the constraint traits (the validating walk runs on operation INPUTS only, and this
//! shape is output-only) and `DeserializableStruct` (nothing reads an answer back off
//! the wire).

/// One paginated collection answer: the modelled page, and the page size the request
/// asked for.
///
/// `Collection` is the operation's own modelled output shape, serialized exactly as that
/// shape's derive serializes it; the echo is appended after it.
#[derive(Debug, Clone)]
pub struct Paged<Collection> {
    /// The modelled collection shape: the page of elements and the `position` cursor.
    pub collection: Collection,
    /// The page size the request asked for, echoed back; `None` when it asked for none,
    /// and `None` for the page that ends the collection.
    pub limit: Option<i32>,
}

impl<Collection> Paged<Collection> {
    /// The answer `collection` makes to a request that asked for a page of `limit`
    /// elements.
    pub fn new(collection: Collection, limit: Option<i32>) -> Self {
        Self { collection, limit }
    }
}

/// The member schema the echo is written against. Its shape id is deliberately one the
/// model does not declare, so it cannot collide with a modelled shape, and the member
/// name is the one every collection of this service echoes the page size under.
const M_LIMIT: ::aws_smithy_schema::Schema = ::aws_smithy_schema::Schema::new_member(
    ::aws_smithy_schema::ShapeId::from_static(
        "com.amazonaws.apigateway#PagedCollection$limit",
        "com.amazonaws.apigateway",
        "limit",
    ),
    ::aws_smithy_schema::ShapeType::Integer,
    "limit",
    0,
);

impl<Collection: ::aws_smithy_schema::serde::SerializableStruct>
    ::aws_smithy_schema::serde::SerializableStruct for Paged<Collection>
{
    fn serialize_members(
        &self,
        s: &mut dyn ::aws_smithy_schema::serde::ShapeSerializer,
    ) -> ::std::result::Result<(), ::aws_smithy_schema::serde::SerdeError> {
        self.collection.serialize_members(s)?;
        if let ::std::option::Option::Some(limit) = self.limit {
            s.write_integer(&M_LIMIT, limit)?;
        }
        Ok(())
    }
}
