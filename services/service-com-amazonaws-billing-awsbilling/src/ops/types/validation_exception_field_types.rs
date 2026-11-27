// SPDX-License-Identifier: CC0-1.0

//! The `fieldList` entry a refusal of this service carries, as the endpoint that
//! serves it puts that structure on the wire.
//!
//! An element of a `ValidationException`'s `fieldList` carries a `__type`
//! DISCRIMINATOR no Smithy member declares, stamped in the namespace of the
//! backend that answers for this service rather than in this model's own — the
//! same recorded detail that makes this surface announce
//! `application/x-amz-json-1.1` while the model declares `awsJson1_0` (see
//! `crate::service`).
//!
//! On the awsJson protocols the discriminator is how a client decides how to
//! decode a JSON error document. The framework writes the TOP-LEVEL one itself,
//! reading the error code from the error channel; a NESTED structure is
//! serialized through its own `SerializableStruct`, so the discriminator has to
//! be written there — which the `#[smithy_macros::smithy(shape = …)]` expansion
//! derives from the model and cannot be told. So this shape is declared here,
//! hand-written, rather than beside the other shapes reached by more than one
//! operation: everything below is what that expansion emits for a structure —
//! the member statics, the `SCHEMA` the enclosing list writes each element
//! with, the `SCHEMA_*` static, and the two serde impls — plus the one member
//! the model does not declare. The modelled members carry exactly what the
//! refusal states about the member that failed (`crate::service`).
#![allow(unused_imports)]

use super::*;

/// One violating request member: "the name of the field" and "the message
/// describing why the field failed validation".
#[derive(Debug, Clone, Default)]
pub struct ValidationExceptionField {
    pub name: String,
    pub message: String,
}

/// The namespace the error bodies of this service stamp their `__type`
/// discriminators in: the billing-view surface is served by the insights
/// backend, and a shape id in an error body names the shape as THAT backend
/// knows it rather than as this service's own model spells it. One constant, so
/// every nested error structure of this service stamps the one namespace.
const ERROR_SHAPE_NAMESPACE: &str = "com.amazonaws.awsinsightsindexservice.v0";

/// The `__type` discriminator a `fieldList` element carries: the shape id of the
/// field structure, in [`ERROR_SHAPE_NAMESPACE`].
static VALIDATION_EXCEPTION_FIELD_TYPE: ::std::sync::LazyLock<String> =
    ::std::sync::LazyLock::new(|| {
        format!(
            "{ERROR_SHAPE_NAMESPACE}#{}",
            ValidationExceptionField::SHAPE_NAME
        )
    });

/// The discriminator member: not in the model, so its schema is spelled here the
/// way the framework spells the top-level one — the member NAME is the wire key
/// the awsJson codecs read, and `@jsonName` is carried as well so a codec that
/// honours it (restJson1) spells the member the same way.
static VALIDATION_EXCEPTION_FIELD_M_TYPE: ::aws_smithy_schema::Schema =
    ::aws_smithy_schema::Schema::new_member(
        ::aws_smithy_schema::ShapeId::from_static("smithy.api#__type", "smithy.api", "__type"),
        ::aws_smithy_schema::ShapeType::String,
        "__type",
        0,
    )
    .with_json_name("__type");

/// `name`: the member that failed.
static VALIDATION_EXCEPTION_FIELD_M_NAME: ::aws_smithy_schema::Schema =
    ::aws_smithy_schema::Schema::new_member(
        ::aws_smithy_schema::ShapeId::from_static(
            "com.amazonaws.billing#ValidationExceptionField$name",
            "com.amazonaws.billing",
            "name",
        ),
        ::aws_smithy_schema::ShapeType::String,
        "name",
        0,
    );

/// `message`: why it failed.
static VALIDATION_EXCEPTION_FIELD_M_MESSAGE: ::aws_smithy_schema::Schema =
    ::aws_smithy_schema::Schema::new_member(
        ::aws_smithy_schema::ShapeId::from_static(
            "com.amazonaws.billing#ValidationExceptionField$message",
            "com.amazonaws.billing",
            "message",
        ),
        ::aws_smithy_schema::ShapeType::String,
        "message",
        1,
    )
    .with_xml_name("Message");

/// The modelled members of [`ValidationExceptionField`], as the one `'static`
/// slice its `SCHEMA` borrows (an associated `const` cannot borrow a temporary
/// one).
static VALIDATION_EXCEPTION_FIELD_MEMBERS: &[&::aws_smithy_schema::Schema] = &[
    &VALIDATION_EXCEPTION_FIELD_M_NAME,
    &VALIDATION_EXCEPTION_FIELD_M_MESSAGE,
];

impl ValidationExceptionField {
    /// The model's own name for this shape, which the discriminator names.
    pub const SHAPE_NAME: &'static str = "ValidationExceptionField";

    /// The schema the enclosing `fieldList` writes each element with. The
    /// modelled members only: the discriminator is not a member of the shape,
    /// exactly as the top-level `__type` is not a member of the error shape it
    /// is written into.
    pub const SCHEMA: ::aws_smithy_schema::Schema = ::aws_smithy_schema::Schema::new_struct(
        ::aws_smithy_schema::ShapeId::from_static(
            "com.amazonaws.billing#ValidationExceptionField",
            "com.amazonaws.billing",
            "ValidationExceptionField",
        ),
        ::aws_smithy_schema::ShapeType::Structure,
        VALIDATION_EXCEPTION_FIELD_MEMBERS,
    );
}

/// The schema under the name the generated modules give every shape's, so a
/// reference to it resolves wherever the shape is reached from.
pub static SCHEMA_VALIDATION_EXCEPTION_FIELD: &::aws_smithy_schema::Schema =
    &ValidationExceptionField::SCHEMA;

impl ::aws_smithy_schema::serde::SerializableStruct for ValidationExceptionField {
    fn serialize_members(
        &self,
        serializer: &mut dyn ::aws_smithy_schema::serde::ShapeSerializer,
    ) -> ::std::result::Result<(), ::aws_smithy_schema::serde::SerdeError> {
        // The discriminator first, as the framework writes the top-level one.
        serializer.write_string(
            &VALIDATION_EXCEPTION_FIELD_M_TYPE,
            VALIDATION_EXCEPTION_FIELD_TYPE.as_str(),
        )?;
        serializer.write_string(&VALIDATION_EXCEPTION_FIELD_M_NAME, self.name.as_str())?;
        serializer.write_string(&VALIDATION_EXCEPTION_FIELD_M_MESSAGE, self.message.as_str())?;
        Ok(())
    }
}

impl ::aws_smithy_schema_server::DeserializableStruct for ValidationExceptionField {
    fn deserialize(
        deserializer: &mut dyn ::aws_smithy_schema::serde::ShapeDeserializer,
    ) -> ::std::result::Result<Self, ::aws_smithy_schema::serde::SerdeError> {
        let mut name = String::new();
        let mut message = String::new();
        deserializer.read_struct(&Self::SCHEMA, &mut |member, value| {
            match member.member_index() {
                Some(0) => name = value.read_string(member)?,
                Some(1) => message = value.read_string(member)?,
                _ => {}
            }
            Ok(())
        })?;
        Ok(Self { name, message })
    }
}
