// SPDX-License-Identifier: CC0-1.0

//! The fields GraphQL's INTROSPECTION SYSTEM adds to every schema, answered on a GraphQL
//! API's own `GRAPHQL` endpoint ([`super::execution`]).
//!
//! A GraphQL schema carries three fields nothing declares and no resolver is attached to:
//! `__typename`, which names the type the selection is on and is available on every object,
//! and `__schema` and `__type(name:)`, which the QUERY root carries and which report the
//! type system itself. They are what a client — a console, a code generator, a GraphQL IDE
//! or a test that waits for a deployed schema to be served — asks for first, and no resolver
//! could answer one: `CreateResolver` attaches a resolver to a field of a type the schema
//! declares, and these three belong to no such type.
//!
//! What they answer is the API's OWN type system, rendered exactly as this crate already
//! renders it for `GetIntrospectionSchema` (`crate::state::introspection_schema`), so a
//! client reading a schema through the control plane and one introspecting it through the
//! data plane cannot be told two different things about one API.
//!
//! The types the introspection system defines are not types the API declares, so the walk
//! that projects a selection onto a resolved value cannot read their fields' types out of
//! the API's schema. [`field_type`] is that half of the introspection schema: the type each
//! field of `__Schema`, `__Type`, `__Field`, `__InputValue`, `__Directive` and `__EnumValue`
//! resolves to, as GraphQL's own introspection schema declares it.
//!
//! Whether the API serves the two ROOT meta-fields at all is its own configuration
//! ([`serves_introspection`]): an API that disables introspection serves a schema those
//! meta-types are no part of, so a document selecting a field of one selects a field nothing
//! declares, and the request's own DOCUMENT VALIDATION is what reports it.

use ::serde_json::Value as Json;

use crate::state::GraphqlApiRecord;

/// The field that names the type the selection is on, available on every object of every
/// schema.
pub const TYPENAME_FIELD: &str = "__typename";

/// The two fields the QUERY root carries, which report the type system itself.
pub const SCHEMA_FIELD: &str = "__schema";
pub const TYPE_FIELD: &str = "__type";

/// The argument `__type` names the type it reports by: `__type(name: String!)`.
pub const TYPE_NAME_ARGUMENT: &str = "name";

/// The introspection types the two root fields resolve to.
pub const SCHEMA_TYPE: &str = "__Schema";
pub const TYPE_TYPE: &str = "__Type";

/// The other four the introspection system defines.
const FIELD_TYPE: &str = "__Field";
const INPUT_VALUE_TYPE: &str = "__InputValue";
const DIRECTIVE_TYPE: &str = "__Directive";
const ENUM_VALUE_TYPE: &str = "__EnumValue";

/// Whether `type_name` is one of the types the introspection system itself defines, whose
/// fields the API's own schema declares nothing about.
pub fn is_introspection_type(type_name: &str) -> bool {
    matches!(
        type_name,
        SCHEMA_TYPE | TYPE_TYPE | FIELD_TYPE | INPUT_VALUE_TYPE | DIRECTIVE_TYPE | ENUM_VALUE_TYPE
    )
}

/// Whether `field_name` is a field the INTROSPECTION SYSTEM adds rather than one a schema
/// declares: GraphQL reserves every name beginning with [`RESERVED_PREFIX`] for that system,
/// so no schema declares one and a document selecting one is selecting no field of the
/// API's own type system.
pub fn is_introspection_field(field_name: &str) -> bool {
    field_name.starts_with(RESERVED_PREFIX)
}

/// The prefix GraphQL reserves for the names its type and introspection systems define.
const RESERVED_PREFIX: &str = "__";

/// The introspection type one of the QUERY ROOT's own two meta-fields resolves to, or `None`
/// for a field that is neither of them.
///
/// `__schema` reports the type system as a [`SCHEMA_TYPE`] and `__type(name:)` reports one
/// type of it as a [`TYPE_TYPE`]; both belong to the query root alone. Read where the fields
/// those two carry have to be named without the API's own schema — the walk that projects a
/// selection onto one of them ([`field_type`]), and the one that reports them undefined on an
/// API serving no introspection at all.
pub fn root_field_type(field_name: &str) -> Option<&'static str> {
    match field_name {
        SCHEMA_FIELD => Some(SCHEMA_TYPE),
        TYPE_FIELD => Some(TYPE_TYPE),
        _ => None,
    }
}

/// Whether `api` serves GraphQL's INTROSPECTION SYSTEM on its data plane at all.
///
/// `GraphqlApi$introspectionConfig` "sets the value of the GraphQL API to enable (`ENABLED`)
/// or disable (`DISABLED`) introspection", and "will produce an error if the operation
/// attempts to use the introspection feature while this field is disabled" — so an API
/// configured with [`INTROSPECTION_DISABLED`] carries no introspection meta-type in the
/// schema its data plane serves, and a document selecting a field of one is selecting a field
/// nothing declares. Only the two meta-fields of the query root are taken away with them:
/// `__typename` is the TYPE system's own field rather than the introspection system's, and
/// what the API's schema declares is untouched either way.
pub fn serves_introspection(api: &GraphqlApiRecord) -> bool {
    api.introspection_config != INTROSPECTION_DISABLED
}

/// The `GraphQLApiIntrospectionConfig` value that takes introspection off an API. Source: the
/// enum's own values (`smithy_query(query="shape",shape="GraphQLApiIntrospectionConfig")` —
/// `ENABLED | DISABLED`), the other of which is the default
/// (`crate::state::DEFAULT_INTROSPECTION_CONFIG`).
const INTROSPECTION_DISABLED: &str = "DISABLED";

/// The type one field of an introspection type resolves to, as GraphQL's introspection
/// schema declares it, or `None` for a field that resolves to a scalar (or for a type this
/// is not asked about) — which carries no sub-selection and so needs no type to project it
/// against.
pub fn field_type(type_name: &str, field_name: &str) -> Option<&'static str> {
    let named = match (type_name, field_name) {
        (SCHEMA_TYPE, "types" | "queryType" | "mutationType" | "subscriptionType") => TYPE_TYPE,
        (SCHEMA_TYPE, "directives") => DIRECTIVE_TYPE,
        (TYPE_TYPE, "fields") => FIELD_TYPE,
        (TYPE_TYPE, "inputFields") => INPUT_VALUE_TYPE,
        (TYPE_TYPE, "interfaces" | "possibleTypes" | "ofType") => TYPE_TYPE,
        (TYPE_TYPE, "enumValues") => ENUM_VALUE_TYPE,
        (FIELD_TYPE | DIRECTIVE_TYPE, "args") => INPUT_VALUE_TYPE,
        (FIELD_TYPE | INPUT_VALUE_TYPE, "type") => TYPE_TYPE,
        _ => return None,
    };
    Some(named)
}

/// What `__schema` answers for `api`: its whole type system as introspection's `__Schema`,
/// and `null` for an API that holds no type system at all — there is nothing to introspect
/// then, which is the same nothing `GetIntrospectionSchema` refuses such an API with.
///
/// The directives the schema defines are part of the answer here, unlike the control-plane
/// call's, because the client's own selection is what asks for them.
pub fn schema(api: &GraphqlApiRecord) -> Json {
    api.introspection_schema_document(true).unwrap_or(Json::Null)
}

/// What `__type(name:)` answers, read out of the `__Schema` `document` the request already
/// rendered: that type as introspection's `__Type`, and `null` for a name the API's type
/// system does not hold, which is what GraphQL reports for a type a schema does not declare.
///
/// Read out of the one document rather than rendered again, so the two fields of one request
/// cannot report one type two ways — and so a document selecting many of them renders the
/// type system once.
pub fn named_type(document: &Json, name: &str) -> Json {
    document
        .get(TYPES_MEMBER)
        .and_then(Json::as_array)
        .into_iter()
        .flatten()
        .find(|reported| reported.get(NAME_MEMBER).and_then(Json::as_str) == Some(name))
        .cloned()
        .unwrap_or(Json::Null)
}

/// What `__typename` answers for a selection on `type_name`: that type's name.
///
/// It is the TYPE SYSTEM that declares the type, so a selection on a type the API's schema
/// does not declare has no type name to report (`Json::Null`) — an API that holds no type
/// system names nothing at all. The introspection system's own types are not declared by the
/// API's schema, so they are named as they stand.
pub fn type_name(document: &Json, type_name: &str) -> Json {
    if is_introspection_type(type_name) {
        return Json::String(type_name.to_string());
    }
    match named_type(document, type_name).is_null() {
        true => Json::Null,
        false => Json::String(type_name.to_string()),
    }
}

/// The members of a `__Schema` and a `__Type` this module reads: the list of every type the
/// schema holds, and a type's own name.
const TYPES_MEMBER: &str = "types";

/// A `__Type`'s own name, which is what says WHICH type of the schema a reported `__Type`
/// is — read by the walk that answers a selection on one ([`super::execution`]).
pub const NAME_MEMBER: &str = "name";
