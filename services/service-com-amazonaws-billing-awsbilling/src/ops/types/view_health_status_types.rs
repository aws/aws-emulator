// SPDX-License-Identifier: CC0-1.0

//! The health status every billing view this surface reports carries, as the
//! endpoint that serves it puts that structure on the wire.
//!
//! A read of a billing view answers `healthStatus` with its status code and the
//! reasons for it under BOTH spellings — `statusCode` beside `StatusCode`, and
//! `statusReasons` beside `StatusReasons` — one value each way, so a client
//! keyed on either reads the same status. That is recorded behaviour of the
//! plane that answers for billing views, the same plane whose answers announce
//! `application/x-amz-json-1.1` while this service's model declares
//! `awsJson1_0` (see `crate::service`): the model spells the two members
//! camelCase, and the endpoint writes the capitalised pair as well.
//!
//! The awsJson codecs take a member's NAME as its JSON key (`@jsonName` is
//! honoured by restJson1 only), so a second spelling is a second member SCHEMA
//! the one stored value is written through — which the
//! `#[smithy_macros::smithy(shape = …)]` expansion derives from the model and
//! cannot be told. So this shape is declared here, hand-written, rather than
//! beside the other shapes reached by more than one operation: everything below
//! is what that expansion emits for a structure — the member statics, the
//! `SCHEMA` an enclosing structure writes it with, the `SCHEMA_*` static, and
//! the two serde impls — with each stored member written under both of its wire
//! names. Nothing about the wire is invented: both spellings carry the value the
//! service recorded for the view (`crate::state::health_status`).
#![allow(unused_imports)]

use super::*;

/// The health of one billing view: "a status code and optional reasons for the
/// status", derived on every read from the state the view depends on — see
/// [`crate::state::health_status`], the one place this structure is built.
///
/// Two Rust fields, four wire members: which spelling a client reads is a
/// property of the endpoint, not of the value, so the value is stored once.
#[derive(Debug, Clone, Default)]
pub struct BillingViewHealthStatus {
    /// A `BillingViewStatus` value — written as `statusCode` and as
    /// `StatusCode`.
    pub status_code: Option<String>,
    /// The `BillingViewStatusReasons` explaining it, empty when the service
    /// recorded none — written as `statusReasons` and as `StatusReasons`.
    pub status_reasons: Option<Vec<String>>,
}

/// `statusCode`, as the model spells it.
static BILLING_VIEW_HEALTH_STATUS_M_STATUS_CODE: ::aws_smithy_schema::Schema =
    ::aws_smithy_schema::Schema::new_member(
        ::aws_smithy_schema::ShapeId::from_static(
            "com.amazonaws.billing#BillingViewHealthStatus$statusCode",
            "com.amazonaws.billing",
            "statusCode",
        ),
        ::aws_smithy_schema::ShapeType::String,
        "statusCode",
        0,
    );

/// The same member under the capitalised spelling the endpoint also writes. The
/// shape id and the member index stay the model's own — only the wire key
/// differs — and `@jsonName` is carried as well, so a codec that honours it
/// (restJson1) spells the member the way the awsJson codecs do here.
static BILLING_VIEW_HEALTH_STATUS_M_STATUS_CODE_CAPITALISED: ::aws_smithy_schema::Schema =
    ::aws_smithy_schema::Schema::new_member(
        ::aws_smithy_schema::ShapeId::from_static(
            "com.amazonaws.billing#BillingViewHealthStatus$statusCode",
            "com.amazonaws.billing",
            "statusCode",
        ),
        ::aws_smithy_schema::ShapeType::String,
        "StatusCode",
        1,
    )
    .with_json_name("StatusCode");

/// `statusReasons`, as the model spells it.
static BILLING_VIEW_HEALTH_STATUS_M_STATUS_REASONS: ::aws_smithy_schema::Schema =
    ::aws_smithy_schema::Schema::new_member(
        ::aws_smithy_schema::ShapeId::from_static(
            "com.amazonaws.billing#BillingViewHealthStatus$statusReasons",
            "com.amazonaws.billing",
            "statusReasons",
        ),
        ::aws_smithy_schema::ShapeType::List,
        "statusReasons",
        2,
    );

/// The same list under the capitalised spelling — see
/// [`BILLING_VIEW_HEALTH_STATUS_M_STATUS_CODE_CAPITALISED`].
static BILLING_VIEW_HEALTH_STATUS_M_STATUS_REASONS_CAPITALISED: ::aws_smithy_schema::Schema =
    ::aws_smithy_schema::Schema::new_member(
        ::aws_smithy_schema::ShapeId::from_static(
            "com.amazonaws.billing#BillingViewHealthStatus$statusReasons",
            "com.amazonaws.billing",
            "statusReasons",
        ),
        ::aws_smithy_schema::ShapeType::List,
        "StatusReasons",
        3,
    )
    .with_json_name("StatusReasons");

/// The members of [`BillingViewHealthStatus`], as the one `'static` slice its
/// `SCHEMA` borrows (an associated `const` cannot borrow a temporary one).
static BILLING_VIEW_HEALTH_STATUS_MEMBERS: &[&::aws_smithy_schema::Schema] = &[
    &BILLING_VIEW_HEALTH_STATUS_M_STATUS_CODE,
    &BILLING_VIEW_HEALTH_STATUS_M_STATUS_CODE_CAPITALISED,
    &BILLING_VIEW_HEALTH_STATUS_M_STATUS_REASONS,
    &BILLING_VIEW_HEALTH_STATUS_M_STATUS_REASONS_CAPITALISED,
];

impl BillingViewHealthStatus {
    /// The schema an enclosing structure — the billing view a read reports —
    /// writes this member with.
    pub const SCHEMA: ::aws_smithy_schema::Schema = ::aws_smithy_schema::Schema::new_struct(
        ::aws_smithy_schema::ShapeId::from_static(
            "com.amazonaws.billing#BillingViewHealthStatus",
            "com.amazonaws.billing",
            "BillingViewHealthStatus",
        ),
        ::aws_smithy_schema::ShapeType::Structure,
        BILLING_VIEW_HEALTH_STATUS_MEMBERS,
    );
}

/// The schema under the name the generated modules give every shape's, so a
/// reference to it resolves wherever the shape is reached from.
pub static SCHEMA_BILLING_VIEW_HEALTH_STATUS: &::aws_smithy_schema::Schema =
    &BillingViewHealthStatus::SCHEMA;

impl ::aws_smithy_schema::serde::SerializableStruct for BillingViewHealthStatus {
    fn serialize_members(
        &self,
        serializer: &mut dyn ::aws_smithy_schema::serde::ShapeSerializer,
    ) -> ::std::result::Result<(), ::aws_smithy_schema::serde::SerdeError> {
        if let Some(status_code) = self.status_code.as_ref() {
            for member in [
                &BILLING_VIEW_HEALTH_STATUS_M_STATUS_CODE,
                &BILLING_VIEW_HEALTH_STATUS_M_STATUS_CODE_CAPITALISED,
            ] {
                serializer.write_string(member, status_code.as_str())?;
            }
        }
        if let Some(status_reasons) = self.status_reasons.as_ref() {
            for member in [
                &BILLING_VIEW_HEALTH_STATUS_M_STATUS_REASONS,
                &BILLING_VIEW_HEALTH_STATUS_M_STATUS_REASONS_CAPITALISED,
            ] {
                serializer.write_list(member, &|elements| {
                    for reason in status_reasons.iter() {
                        elements.write_string(member, reason.as_str())?;
                    }
                    Ok(())
                })?;
            }
        }
        Ok(())
    }
}

impl ::aws_smithy_schema_server::DeserializableStruct for BillingViewHealthStatus {
    fn deserialize(
        deserializer: &mut dyn ::aws_smithy_schema::serde::ShapeDeserializer,
    ) -> ::std::result::Result<Self, ::aws_smithy_schema::serde::SerdeError> {
        let mut status_code: Option<String> = None;
        let mut status_reasons: Option<Vec<String>> = None;
        deserializer.read_struct(&Self::SCHEMA, &mut |member, value| {
            match member.member_index() {
                // Either spelling names the one member, on the way in as on the
                // way out.
                Some(0) | Some(1) => status_code = Some(value.read_string(member)?),
                Some(2) | Some(3) => {
                    let mut reasons: Vec<String> = Vec::new();
                    value.read_list(member, &mut |element| {
                        reasons.push(element.read_string(member)?);
                        Ok(())
                    })?;
                    status_reasons = Some(reasons);
                }
                _ => {}
            }
            Ok(())
        })?;
        Ok(Self {
            status_code,
            status_reasons,
        })
    }
}
