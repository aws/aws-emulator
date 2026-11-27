// SPDX-License-Identifier: CC0-1.0

//! The ONE omission of a STRING MEMBER whose stored value is unset, for every service whose
//! model declares such a member `@required` on the read side — so no response of that class
//! reports a value the member's own shape forbids.
//!
//! # The rule
//!
//! A response carries NO KEY for a member that holds no value. A string member whose own
//! `@length` minimum is 1 or more cannot hold the empty string at all, so for such a member
//! "unset" can only be reported by leaving the member out: a client can then tell "unset"
//! from "set to something", and what it reads back is a value it can send straight into the
//! write that sets the member — which the boundary walk refuses the empty string for,
//! because the same shape bounds that input member.
//!
//! The rule keys on the member's own VALUE SPACE, not on a list of member names. Whether a
//! member that CAN hold the empty string reports it or omits it is each service's own
//! recorded fact — some AWS services really do answer `""` for a value nobody set — so
//! only a member whose shape admits no empty value at all is omitted here, and a service
//! takes the rule explicitly (see below) rather than every service getting it.
//!
//! # Why a service needs this
//!
//! The generated shapes cannot express it. `#[smithy_macros::smithy(shape = ..)]` derives a
//! member's Rust optionality from the model and requires a `@required` member to be a
//! NON-`Option` field, and the serializer it emits writes every non-`Option` field
//! unconditionally. So a handler can only hand such a struct a string, and the empty string
//! is the one that stands for "unset" — which for a member whose `@length` forbids it is a
//! value no client of the modelled API may send back. Dropping it on the way out is
//! therefore done once, here, rather than by hand-writing the `Schema` and the serializer of
//! every shape that carries such a member, which is a copy of generated code per shape.
//!
//! Only a STRUCTURE MEMBER is subject to the rule. A list element and a map value are
//! written against the COLLECTION member's schema, whose `@length` bounds the element count
//! rather than the element, so an empty string inside a collection is written exactly as the
//! handler produced it and no element is ever dropped. Nothing else about the walk changes: a
//! member's wire name, its order and its schema stay the ones the model gave it, and the
//! request side is untouched.
//!
//! # Where a service takes it
//!
//! At its own wire protocol, which `Service::override_protocol` hands back: every output,
//! error and event the protocol serializes is wrapped in [`OmittingUnsetStrings`], which is
//! what carries the rule into every nesting level of the shape. The IoT SiteWise crate's
//! `src/service/unset_string_members.rs` is the one in the tree, and the shape of it is:
//!
//! ```ignore
//! fn serialize_output(
//!     &self,
//!     output: &dyn SerializableStruct,
//!     output_schema: &Schema,
//!     operation_name: &str,
//!     success_status: u16,
//! ) -> Result<Self::Response, ProtocolError> {
//!     self.inner.serialize_output(
//!         &OmittingUnsetStrings(output),
//!         output_schema,
//!         operation_name,
//!         success_status,
//!     )
//! }
//! ```
//!
//! The wrapper costs one stack frame per nesting level and nothing else — no buffer, no copy
//! of the body and no caller-sized allocation. The nesting it descends is the SHAPE's, which
//! the model fixes, so no request can choose how deep it goes.

use aws_smithy_schema::serde::{SerdeError, SerializableStruct, ShapeSerializer};
use aws_smithy_schema::Schema;
use aws_smithy_schema_server::LengthTrait;
use aws_smithy_types::{BigDecimal, BigInteger, DateTime, Document};

/// Whether `schema`'s own `@length` forbids the empty string: a minimum of 1 or more. A
/// member carrying no `@length` at all, or one whose minimum is 0, admits the empty string
/// and keeps it.
#[must_use]
pub fn forbids_the_empty_string(schema: &Schema) -> bool {
    schema
        .traits()
        .and_then(|traits| traits.get(&LengthTrait::TRAIT_ID))
        .and_then(|length| length.as_any().downcast_ref::<LengthTrait>())
        .and_then(|length| length.min)
        .is_some_and(|min| min >= 1)
}

/// One shape, serialized with every unset string member left out — the form a protocol hands
/// its codec in place of the shape itself.
pub struct OmittingUnsetStrings<'a>(pub &'a dyn SerializableStruct);

impl SerializableStruct for OmittingUnsetStrings<'_> {
    fn serialize_members(&self, s: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
        self.0.serialize_members(&mut MemberWriter {
            inner: s,
            of_a_structure: true,
        })
    }
}

/// The serializer a shape's members are written through: every call is the codec's own,
/// except a structure member holding a string its `@length` forbids, which is not written at
/// all.
///
/// A nested structure re-enters the wrapper, so the rule reaches such a member at any depth.
/// A list or a map re-enters it with `of_a_structure` cleared, because the values written
/// inside one are elements, not members.
struct MemberWriter<'a> {
    inner: &'a mut dyn ShapeSerializer,
    /// Whether what is being written is a MEMBER of a structure — and so whether the schema
    /// each call carries is that member's own, constraints and all.
    of_a_structure: bool,
}

impl ShapeSerializer for MemberWriter<'_> {
    fn write_struct(
        &mut self,
        schema: &Schema,
        value: &dyn SerializableStruct,
    ) -> Result<(), SerdeError> {
        self.inner
            .write_struct(schema, &OmittingUnsetStrings(value))
    }

    fn write_list(
        &mut self,
        schema: &Schema,
        write_elements: &dyn Fn(&mut dyn ShapeSerializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        self.inner.write_list(schema, &|inner| {
            write_elements(&mut MemberWriter {
                inner,
                of_a_structure: false,
            })
        })
    }

    fn write_map(
        &mut self,
        schema: &Schema,
        write_entries: &dyn Fn(&mut dyn ShapeSerializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        self.inner.write_map(schema, &|inner| {
            write_entries(&mut MemberWriter {
                inner,
                of_a_structure: false,
            })
        })
    }

    fn write_boolean(&mut self, schema: &Schema, value: bool) -> Result<(), SerdeError> {
        self.inner.write_boolean(schema, value)
    }

    fn write_byte(&mut self, schema: &Schema, value: i8) -> Result<(), SerdeError> {
        self.inner.write_byte(schema, value)
    }

    fn write_short(&mut self, schema: &Schema, value: i16) -> Result<(), SerdeError> {
        self.inner.write_short(schema, value)
    }

    fn write_integer(&mut self, schema: &Schema, value: i32) -> Result<(), SerdeError> {
        self.inner.write_integer(schema, value)
    }

    fn write_long(&mut self, schema: &Schema, value: i64) -> Result<(), SerdeError> {
        self.inner.write_long(schema, value)
    }

    fn write_float(&mut self, schema: &Schema, value: f32) -> Result<(), SerdeError> {
        self.inner.write_float(schema, value)
    }

    fn write_double(&mut self, schema: &Schema, value: f64) -> Result<(), SerdeError> {
        self.inner.write_double(schema, value)
    }

    fn write_big_integer(&mut self, schema: &Schema, value: &BigInteger) -> Result<(), SerdeError> {
        self.inner.write_big_integer(schema, value)
    }

    fn write_big_decimal(&mut self, schema: &Schema, value: &BigDecimal) -> Result<(), SerdeError> {
        self.inner.write_big_decimal(schema, value)
    }

    /// The one rewrite on the way out: a structure member whose value is the empty string its
    /// own `@length` forbids is skipped, so the response carries no member rather than a
    /// value the shape rejects. Every other string — a value the member admits, or one
    /// written as a collection element — is the codec's own.
    fn write_string(&mut self, schema: &Schema, value: &str) -> Result<(), SerdeError> {
        if self.of_a_structure && value.is_empty() && forbids_the_empty_string(schema) {
            return Ok(());
        }
        self.inner.write_string(schema, value)
    }

    fn write_blob(&mut self, schema: &Schema, value: &[u8]) -> Result<(), SerdeError> {
        self.inner.write_blob(schema, value)
    }

    fn write_timestamp(&mut self, schema: &Schema, value: &DateTime) -> Result<(), SerdeError> {
        self.inner.write_timestamp(schema, value)
    }

    fn write_document(&mut self, schema: &Schema, value: &Document) -> Result<(), SerdeError> {
        self.inner.write_document(schema, value)
    }

    fn write_null(&mut self, schema: &Schema) -> Result<(), SerdeError> {
        self.inner.write_null(schema)
    }
}

#[cfg(test)]
mod tests {
    use super::forbids_the_empty_string;
    use aws_smithy_schema::{Schema, ShapeId, ShapeType, TraitMap};
    use aws_smithy_schema_server::LengthTrait;
    use std::sync::LazyLock;

    /// A member's captured traits carrying one `@length`, as the codegen captures the
    /// constraints of the shape a member targets.
    fn length(min: Option<i64>, max: Option<i64>) -> TraitMap {
        let mut traits = TraitMap::new();
        traits.insert(Box::new(LengthTrait::new(min, max)));
        traits
    }

    static MINIMUM_OF_ZERO: LazyLock<TraitMap> = LazyLock::new(|| length(Some(0), Some(2048)));
    static MINIMUM_OF_ONE: LazyLock<TraitMap> = LazyLock::new(|| length(Some(1), Some(2048)));
    static MINIMUM_ONLY: LazyLock<TraitMap> = LazyLock::new(|| length(Some(1), None));
    static FIXED_LENGTH: LazyLock<TraitMap> = LazyLock::new(|| length(Some(36), Some(36)));
    static MAXIMUM_ONLY: LazyLock<TraitMap> = LazyLock::new(|| length(None, Some(8192)));

    /// A string member schema as the codegen builds one, before its captured traits are
    /// chained on.
    const BARE: Schema = Schema::new_member(
        ShapeId::from_static("ns.example#Owner$text", "ns.example", "text"),
        ShapeType::String,
        "text",
        0,
    );

    /// RULE: a member forbids the empty string exactly when its own `@length` minimum is 1 or
    /// more. The rows span what the predicate generalises over — no `@length` at all, a
    /// minimum of 0 (and so a member the empty string IS a value of), a minimum of 1 with and
    /// without a maximum, a higher minimum, and a maximum on its own.
    #[test]
    fn only_a_minimum_of_one_or_more_forbids_the_empty_string() {
        let rows: [(&str, Option<&LazyLock<TraitMap>>, bool); 6] = [
            ("no length at all", None, false),
            ("a minimum of 0", Some(&MINIMUM_OF_ZERO), false),
            ("a minimum of 1", Some(&MINIMUM_OF_ONE), true),
            ("a minimum of 1 with no maximum", Some(&MINIMUM_ONLY), true),
            ("a fixed length of 36", Some(&FIXED_LENGTH), true),
            ("a maximum on its own", Some(&MAXIMUM_ONLY), false),
        ];
        for (case, traits, expected) in rows {
            let schema = match traits {
                Some(traits) => BARE.with_traits(traits),
                None => BARE,
            };
            let forbids = forbids_the_empty_string(&schema);
            assert_eq!(
                forbids, expected,
                "case {case:?}: expected forbids_the_empty_string {expected}, got {forbids}"
            );
        }
    }
}
