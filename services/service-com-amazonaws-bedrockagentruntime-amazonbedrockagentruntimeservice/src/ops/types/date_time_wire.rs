// SPDX-License-Identifier: CC0-1.0

//! The schema and the codec a structure needs when a member of it carries
//! `@timestampFormat("date-time")`.
//!
//! WHY THIS MODULE EXISTS. Every `DateTimestamp` member of this service's session plane
//! carries `@timestampFormat("date-time")` —
//! `smithy_query(query="operation",operation="PutInvocationStep")` reports
//! `"timestampFormat": "date-time"` on `invocationStepTime`, and
//! `smithy_query(query="shape",shape="GetSessionResponse,InvocationStep,InvocationStepSummary")`
//! the same on `createdAt`, `lastUpdatedAt` and `invocationStepTime`. On the restJson1
//! wire that makes the RFC 3339 / ISO-8601 STRING form part of the contract, in BOTH
//! directions. The JSON codec decides the rendering per member, from the member schema's
//! own format (`vendor/smithy-schema-server/src/json.rs::read_timestamp` /
//! `write_timestamp` -> `Schema::timestamp_format()`), and falls back to the protocol
//! default — epoch seconds, a JSON NUMBER — when the member schema reports no format.
//!
//! `vendor/smithy-macros` captures `@length`/`@range`/`@pattern`/`@enum`/`@uniqueItems`,
//! `@xmlName`, `@jsonName` and the HTTP bindings onto each member schema it emits, but it
//! never emits `.with_timestamp_format(..)` (grep the tree: every call site is
//! hand-written service code). So a macro-built `date-time` member schema reports NO
//! format: on the REQUEST path an ISO-8601 `invocationStepTime` — the only form an SDK
//! sends for such a member — is REFUSED with a 400 `ValidationException`
//! ("invalid timestamp: failed to parse int"), and on the RESPONSE path the instant goes
//! out as a bare epoch number, a type the modelled `date-time` member does not carry.
//!
//! `vendor/` is read-only from inside the emulator, so the fix lives here: the macros
//! below emit, for one structure, exactly what `#[smithy_macros::smithy(shape = ...)]`
//! emits — one `M_<MEMBER>` member schema per member (index = that member's position in
//! the model's member list), the `SCHEMA` naming them in that order, the
//! `<SHAPE>_SCHEMA` static, and the `SerializableStruct`/`DeserializableStruct` pair —
//! plus the one chain call the codegen omits, which [`session_member_schema!`] attaches
//! for the `date_time` member kind.
//!
//! WHAT IS DELIBERATELY ABSENT.
//!
//! * Constraint traits (`@length`, `@pattern`, …). The validating walk that enforces them
//!   runs on operation INPUTS only (`aws_smithy_schema_server::validate_to_message`, which
//!   `smithy_service` calls once on the deserialized input before the handler runs). Every
//!   structure built with these macros is output-only, so the macro's captured constraints
//!   would be inert here too — the ONE input shape that carries a `date-time` member
//!   (`PutInvocationStepRequest`) therefore does NOT use these macros: its schema is
//!   written out in `put_invocation_step_types.rs` WITH its `@pattern` trait maps and its
//!   HTTP bindings.
//! * Map/list sub-member schemas (`.with_list_member(..)` / key and value schemas). Only
//!   the XML codecs read them, to name an element; this service's sole protocol is
//!   restJson1.
//! * `@xmlName`/`@jsonName`. No member of the shapes below declares either: each one's
//!   wire name IS its Smithy member name.
//!
//! REMOVE THIS MODULE when the vendored codegen learns `smithy.api#timestampFormat`: the
//! fix belongs there, because every `@timestampFormat` member of every service in this
//! workspace has the same defect, and this is not the first service to have to compensate
//! for it by hand — `issues/things-that-do-not-work/20261022T000000Z-smithy-macros-drops-
//! timestampformat-from-member-schemas.md` carries the generator gap, and
//! `services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/src/ops/types/
//! date_time_wire.rs` carries a near-identical macro for the same reason. Once
//! `vendor/smithy-macros` emits `with_timestamp_format`, delete these macros and the
//! `session_schema_and_codec!` blocks and put `#[smithy_macros::smithy(shape = ...)]` back
//! on the structures that carry one. `tests/all/date_time_wire_form.rs` pins the wire
//! form, so it fails if the format does not reach the wire.
//!
//! The structures that need them are spread across this service's per-operation `types/`
//! modules, where each shape is declared exactly once, so `types/mod.rs` pulls this module
//! in with `#[macro_use]` — ahead of those modules, which is what puts these macros in
//! their textual scope. They are NOT `#[macro_export]`ed: `src/ops/mod.rs` reaches
//! `types` through an `include!`, so a `macro_rules!` declared under it counts as
//! macro-expanded and cannot be named by an absolute `$crate::` path (rust-lang #52234).

/// The `ShapeType` one member kind names. Mirrors the vendored codegen's
/// `codegen::shape_type_ident`.
macro_rules! session_shape_type {
    (string) => {
        ::aws_smithy_schema::ShapeType::String
    };
    (date_time) => {
        ::aws_smithy_schema::ShapeType::Timestamp
    };
    (map_of_string) => {
        ::aws_smithy_schema::ShapeType::Map
    };
    (union_of) => {
        ::aws_smithy_schema::ShapeType::Union
    };
    (list_of) => {
        ::aws_smithy_schema::ShapeType::List
    };
}

/// One member's schema, as the vendored codegen would emit it, plus the
/// `@timestampFormat("date-time")` chain call it cannot.
///
/// `$kind` is the member's shape kind — `string` (a `string` or an `enum` shape, both
/// string-encoded), `date_time`, `map_of_string`, `union_of(T)` or `list_of(T)` (a list of
/// structures, written through the item shape's own schema).
macro_rules! session_member_schema {
    ($ns:literal, $local:literal, $wire:literal, $idx:literal, date_time) => {
        ::aws_smithy_schema::Schema::new_member(
            ::aws_smithy_schema::ShapeId::from_static(
                concat!($ns, "#", $local, "$", $wire),
                $ns,
                $wire,
            ),
            ::aws_smithy_schema::ShapeType::Timestamp,
            $wire,
            $idx,
        )
        // The one call the vendored codegen omits; see the module note.
        .with_timestamp_format(::aws_smithy_schema::traits::TimestampFormat::DateTime)
    };
    ($ns:literal, $local:literal, $wire:literal, $idx:literal, $kind:tt $(($inner:ty))?) => {
        ::aws_smithy_schema::Schema::new_member(
            ::aws_smithy_schema::ShapeId::from_static(
                concat!($ns, "#", $local, "$", $wire),
                $ns,
                $wire,
            ),
            session_shape_type!($kind),
            $wire,
            $idx,
        )
    };
}

/// Write one member, as the vendored codegen's `codegen::emit_write` does for that kind: a
/// string map writes each key and then its value against the member's own schema, and a
/// union writes itself as a structure.
macro_rules! session_write_member {
    ($s:ident, $m:expr, $v:expr, string) => { $s.write_string($m, $v.as_str())?; };
    ($s:ident, $m:expr, $v:expr, date_time) => { $s.write_timestamp($m, $v)?; };
    ($s:ident, $m:expr, $v:expr, map_of_string) => {
        $s.write_map($m, &|__sm| {
            for (__k, __mv) in $v.iter() {
                __sm.write_string($m, __k.as_str())?;
                __sm.write_string($m, __mv.as_str())?;
            }
            Ok(())
        })?;
    };
    ($s:ident, $m:expr, $v:expr, union_of($inner:ty)) => { $s.write_struct($m, $v)?; };
    ($s:ident, $m:expr, $v:expr, list_of($inner:ty)) => {
        $s.write_list($m, &|__sl| {
            for __it in $v.iter() {
                // The item's OWN schema, as the vendored codegen writes a list of
                // structures (`codegen::emit_write`), so the item is written as its shape
                // rather than as the list member.
                __sl.write_struct(&<$inner>::SCHEMA, __it)?;
            }
            Ok(())
        })?;
    };
}

/// Read one member, as the vendored codegen's `codegen::emit_read` does for that kind.
///
/// Entry counts are bounded by the request body the server already reads with a cap, and
/// nesting DEPTH by the codec itself: the vendored JSON deserializer takes a depth guard at
/// the entry of `read_struct`, `read_list` and `read_map` and refuses a body nesting deeper
/// than its `MAX_DEPTH`, which the protocol boundary renders as a client refusal.
macro_rules! session_read_member {
    ($d:ident, $m:expr, string) => { $d.read_string($m)? };
    ($d:ident, $m:expr, date_time) => { $d.read_timestamp($m)? };
    ($d:ident, $m:expr, map_of_string) => {{
        let mut __out: ::std::collections::HashMap<::std::string::String, ::std::string::String> =
            ::std::collections::HashMap::new();
        $d.read_map($m, &mut |__k, __dm| {
            let __value = __dm.read_string($m)?;
            __out.insert(__k, __value);
            Ok(())
        })?;
        __out
    }};
    ($d:ident, $m:expr, union_of($inner:ty)) => {
        <$inner as ::aws_smithy_schema_server::DeserializableStruct>::deserialize($d)?
    };
    ($d:ident, $m:expr, list_of($inner:ty)) => {{
        let mut __out: ::std::vec::Vec<$inner> = ::std::vec::Vec::new();
        $d.read_list($m, &mut |__dl| {
            __out.push(
                <$inner as ::aws_smithy_schema_server::DeserializableStruct>::deserialize(__dl)?,
            );
            Ok(())
        })?;
        __out
    }};
}

/// Write a member only when the value is there: a `@required` member always, an `Option`
/// one when it is `Some` — the vendored codegen's own `is_option` split.
macro_rules! session_write_present_member {
    ($s:ident, $this:ident, $m:path, $field:ident, req, $($kind:tt)+) => {
        session_write_member!($s, &$m, (&$this.$field), $($kind)+);
    };
    ($s:ident, $this:ident, $m:path, $field:ident, opt, $($kind:tt)+) => {
        if let ::std::option::Option::Some(__v) = $this.$field.as_ref() {
            session_write_member!($s, &$m, __v, $($kind)+);
        }
    };
}

/// The value a read slot contributes to the struct: a `@required` member absent from the
/// wire is refused by name, an optional one stays `None`.
macro_rules! session_member_value {
    ($slot:ident, $wire:literal, req) => {
        $slot.ok_or_else(|| ::aws_smithy_schema_server::missing_member($wire))?
    };
    ($slot:ident, $wire:literal, opt) => {
        $slot
    };
}

/// The schema, the serializer and the deserializer for one structure, from its member
/// list: everything `#[smithy_macros::smithy(shape = ...)]` emits for an output shape,
/// with `@timestampFormat` honoured.
///
/// Each member entry is `(CONST, field, "wireName", index, req|opt, kind)`; `req` is a
/// `@required` member (a bare Rust field, refused when absent on the wire) and `opt` an
/// `Option` one. The entries must be given in the model's member order, which is the order
/// `smithy_query(query="shape", shape=...)` lists them in and the order each struct
/// declares its fields in.
macro_rules! session_schema_and_codec {
    (
        $ty:ident, $schema_static:ident, $shape:literal, $ns:literal, $local:literal,
        $( ( $cname:ident, $field:ident, $wire:literal, $idx:literal, $presence:tt, $($kind:tt)+ ) ),+ $(,)?
    ) => {
        #[allow(non_upper_case_globals)]
        impl $ty {
            $(
                pub const $cname: ::aws_smithy_schema::Schema =
                    session_member_schema!($ns, $local, $wire, $idx, $($kind)+);
            )+

            pub const SCHEMA: ::aws_smithy_schema::Schema =
                ::aws_smithy_schema::Schema::new_struct(
                    ::aws_smithy_schema::ShapeId::from_static($shape, $ns, $local),
                    ::aws_smithy_schema::ShapeType::Structure,
                    &[$(&Self::$cname,)+],
                );
        }

        pub static $schema_static: &::aws_smithy_schema::Schema = &$ty::SCHEMA;

        impl ::aws_smithy_schema::serde::SerializableStruct for $ty {
            fn serialize_members(
                &self,
                s: &mut dyn ::aws_smithy_schema::serde::ShapeSerializer,
            ) -> ::std::result::Result<(), ::aws_smithy_schema::serde::SerdeError> {
                $(
                    session_write_present_member!(
                        s, self, Self::$cname, $field, $presence, $($kind)+
                    );
                )+
                Ok(())
            }
        }

        impl ::aws_smithy_schema_server::DeserializableStruct for $ty {
            fn deserialize(
                d: &mut dyn ::aws_smithy_schema::serde::ShapeDeserializer,
            ) -> ::std::result::Result<Self, ::aws_smithy_schema::serde::SerdeError> {
                $( let mut $field = ::std::option::Option::None; )+
                d.read_struct(&Self::SCHEMA, &mut |member, d| {
                    match member.member_index() {
                        $(
                            ::std::option::Option::Some($idx) => {
                                $field = ::std::option::Option::Some(
                                    session_read_member!(d, &Self::$cname, $($kind)+),
                                );
                            }
                        )+
                        _ => {}
                    }
                    Ok(())
                })?;
                Ok(Self {
                    $( $field: session_member_value!($field, $wire, $presence) ),+
                })
            }
        }
    };
}
