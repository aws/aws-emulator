// SPDX-License-Identifier: CC0-1.0

//! The INTROSPECTION SYSTEM every GraphQL schema holds without declaring it: the types a
//! schema's introspection document reports beside the ones the schema defines, the
//! descriptions the built-in scalars carry, and the directives every schema of this service
//! supports.
//!
//! GraphQL's introspection system is part of the schema rather than part of any one
//! document: "the schema introspection system is accessible from the meta-fields `__schema`
//! and `__type`", and the types those fields resolve to — `__Schema`, `__Type`, `__Field`,
//! `__InputValue`, `__EnumValue`, `__Directive` and the two enums they name — are types OF
//! every schema, so an introspection document reports them as such. They are written here as
//! the SDL that declares them, parsed by the same GraphQL parser a caller's own document is
//! read with ([`parse_schema_document`]), so one reader renders a type of the introspection
//! system and a type the caller defined, and neither can drift from the other.
//!
//! Nothing of them comes from a request, and nothing of a request changes them: the whole
//! system is a constant of the service. What that constant SAYS is RECORDED behaviour — the
//! introspection system as this service reports it, which is GraphQL's own at the revision
//! the service implements. Two places where that revision shows: `__Directive` carries the
//! legacy `onOperation`/`onFragment`/`onField` fields and a nullable `name`, and
//! `__DirectiveLocation` names the locations listed below and no other. A schema may still
//! state a location the enum does not name; what a schema's own definitions say is read from
//! them, never from here.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use super::schema_types::{parse_schema_document, TypeDefinition};

/// The type an introspection document is reached from: the `__Schema` the `__schema`
/// meta-field resolves to, which names every other type the system defines.
pub(crate) const SCHEMA_META_TYPE: &str = "__Schema";

/// The introspection system's own type definitions, in GraphQL SDL.
///
/// RECORDED behaviour, read against GraphQL's schema-introspection section: each type, the
/// fields it declares, the order it declares them in and the description the system carries
/// for it.
const INTROSPECTION_SYSTEM_SDL: &str = r#"
"""A GraphQL Introspection defines the capabilities of a GraphQL server. It exposes all available types and directives on the server, the entry points for query, mutation, and subscription operations."""
type __Schema {
  "A list of all types supported by this server."
  types: [__Type!]!
  "The type that query operations will be rooted at."
  queryType: __Type!
  "If this server supports mutation, the type that mutation operations will be rooted at."
  mutationType: __Type
  "'A list of all directives supported by this server."
  directives: [__Directive!]!
  "'If this server support subscription, the type that subscription operations will be rooted at."
  subscriptionType: __Type
}

type __Type {
  kind: __TypeKind!
  name: String
  description: String
  fields(includeDeprecated: Boolean = false): [__Field!]
  interfaces: [__Type!]
  possibleTypes: [__Type!]
  enumValues(includeDeprecated: Boolean = false): [__EnumValue!]
  inputFields: [__InputValue!]
  ofType: __Type
}

"""An enum describing what kind of type a given __Type is"""
enum __TypeKind {
  "Indicates this type is a scalar."
  SCALAR
  "Indicates this type is an object. `fields` and `interfaces` are valid fields."
  OBJECT
  "Indicates this type is an interface. `fields` and `possibleTypes` are valid fields."
  INTERFACE
  "Indicates this type is a union. `possibleTypes` is a valid field."
  UNION
  "Indicates this type is an enum. `enumValues` is a valid field."
  ENUM
  "Indicates this type is an input object. `inputFields` is a valid field."
  INPUT_OBJECT
  "Indicates this type is a list. `ofType` is a valid field."
  LIST
  "Indicates this type is a non-null. `ofType` is a valid field."
  NON_NULL
}

type __Field {
  name: String!
  description: String
  args: [__InputValue!]!
  type: __Type!
  isDeprecated: Boolean!
  deprecationReason: String
}

type __InputValue {
  name: String!
  description: String
  type: __Type!
  defaultValue: String
}

type __EnumValue {
  name: String!
  description: String
  isDeprecated: Boolean!
  deprecationReason: String
}

type __Directive {
  name: String
  description: String
  locations: [__DirectiveLocation!]
  args: [__InputValue!]!
  onOperation: Boolean @deprecated(reason: "Use `locations`.")
  onFragment: Boolean @deprecated(reason: "Use `locations`.")
  onField: Boolean @deprecated(reason: "Use `locations`.")
}

"""An enum describing valid locations where a directive can be placed"""
enum __DirectiveLocation {
  "Indicates the directive is valid on queries."
  QUERY
  "Indicates the directive is valid on mutations."
  MUTATION
  "Indicates the directive is valid on fields."
  FIELD
  "Indicates the directive is valid on fragment definitions."
  FRAGMENT_DEFINITION
  "Indicates the directive is valid on fragment spreads."
  FRAGMENT_SPREAD
  "Indicates the directive is valid on inline fragments."
  INLINE_FRAGMENT
  "Indicates the directive is valid on a schema SDL definition."
  SCHEMA
  "Indicates the directive is valid on a scalar SDL definition."
  SCALAR
  "Indicates the directive is valid on an object SDL definition."
  OBJECT
  "Indicates the directive is valid on a field SDL definition."
  FIELD_DEFINITION
  "Indicates the directive is valid on a field argument SDL definition."
  ARGUMENT_DEFINITION
  "Indicates the directive is valid on an interface SDL definition."
  INTERFACE
  "Indicates the directive is valid on an union SDL definition."
  UNION
  "Indicates the directive is valid on an enum SDL definition."
  ENUM
  "Indicates the directive is valid on an enum value SDL definition."
  ENUM_VALUE
  "Indicates the directive is valid on an input object SDL definition."
  INPUT_OBJECT
  "Indicates the directive is valid on an input object field SDL definition."
  INPUT_FIELD_DEFINITION
}
"#;

/// The directives EVERY schema of this service holds without defining them, in GraphQL SDL.
///
/// Two sources, and nothing else. GraphQL's own specified directives, which a conforming
/// server "must provide": `@skip` and `@include`, which a client writes on a selection, and
/// `@deprecated`, which a schema writes on a field or an enum value — each with the
/// description and the described argument it carries, because a client reading a schema's
/// directives reads them as `__Directive`s like any other. And the directives this service's
/// own schemas are written with: the authorization-mode directives it reads a field's
/// permitted modes from (`crate::service::graphql_api::field_auth::MODE_DIRECTIVES`), the
/// Cognito group guard it reads a field's permitted groups from
/// (`crate::service::graphql_api::user_pool`) and the link from a subscription field to the
/// mutations that publish to it (`crate::state::introspection_schema`). A schema applies
/// those without declaring them, so the schema itself is what holds them.
///
/// The locations each one may be applied at are written in the order
/// [`INTROSPECTION_SYSTEM_SDL`] names them, which is the order a schema reports them in.
pub(crate) const SCHEMA_DIRECTIVES_SDL: &str = r#"
"Directs the executor to skip this field or fragment when the `if`'argument is true."
directive @skip(
  "Skipped when true."
  if: Boolean!
) on FIELD | FRAGMENT_SPREAD | INLINE_FRAGMENT

"Directs the executor to include this field or fragment only when the `if` argument is true"
directive @include(
  "Included when true."
  if: Boolean!
) on FIELD | FRAGMENT_SPREAD | INLINE_FRAGMENT

"Marks the field or enum value as deprecated"
directive @deprecated(
  "The reason for the deprecation"
  reason: String = "No longer supported"
) on FIELD_DEFINITION | ENUM_VALUE

directive @aws_api_key on OBJECT | FIELD_DEFINITION

directive @aws_auth(cognito_groups: [String!]!) on FIELD_DEFINITION

directive @aws_cognito_user_pools(cognito_groups: [String!]) on OBJECT | FIELD_DEFINITION

directive @aws_iam on OBJECT | FIELD_DEFINITION

directive @aws_lambda on OBJECT | FIELD_DEFINITION

directive @aws_oidc on OBJECT | FIELD_DEFINITION

directive @aws_subscribe(mutations: [String!]!) on FIELD_DEFINITION
"#;

/// The introspection system's types, by name.
///
/// Parsed once and held, because the definitions are a constant of the service rather than
/// anything a request carries. A catalog that could not be parsed would leave the system
/// empty rather than failing a call, and this module's own test holds it to its contents.
fn meta_types() -> &'static BTreeMap<String, TypeDefinition> {
    static TYPES: OnceLock<BTreeMap<String, TypeDefinition>> = OnceLock::new();
    TYPES.get_or_init(|| {
        parse_schema_document(INTROSPECTION_SYSTEM_SDL)
            .map(|document| {
                document
                    .types
                    .into_iter()
                    .map(|entry| (entry.parsed.name.clone(), entry.parsed))
                    .collect()
            })
            .unwrap_or_default()
    })
}

/// The introspection system's definition of the type named `name`, or `None` for a name the
/// system does not define.
pub(crate) fn meta_type(name: &str) -> Option<&'static TypeDefinition> {
    meta_types().get(name)
}

/// The `__TypeKind` the introspection system's own type `name` has, or `None` for a name it
/// does not define.
///
/// GraphQL reserves every name beginning `__` for this system, so no schema can declare one
/// of these names and this lookup can never shadow a type an API holds.
pub(crate) fn meta_type_kind(name: &str) -> Option<&'static str> {
    meta_type(name).map(|definition| definition.kind)
}

/// The description one of GraphQL's five built-in scalars carries, or `None` for a scalar
/// that carries none.
///
/// Recorded behaviour: `Int`, `Float`, `String`, `Boolean` and `ID` — the scalars GraphQL
/// itself defines — are described as built in, while the scalars AppSync adds carry no
/// description.
pub(crate) fn built_in_scalar_description(name: &str) -> Option<&'static str> {
    BUILT_IN_SCALAR_DESCRIPTIONS
        .iter()
        .find(|(scalar, _)| *scalar == name)
        .map(|(_, description)| *description)
}

/// See [`built_in_scalar_description`].
const BUILT_IN_SCALAR_DESCRIPTIONS: &[(&str, &str)] = &[
    ("Int", "Built-in Int"),
    ("Float", "Built-in Float"),
    ("String", "Built-in String"),
    ("Boolean", "Built-in Boolean"),
    ("ID", "Built-in ID"),
];

#[cfg(test)]
mod tests {
    use super::*;

    /// RULE: the introspection system's SDL is a schema document this service's own parser
    /// reads, so every type it declares is available to render — the catalog cannot silently
    /// fall back to nothing.
    #[test]
    fn the_introspection_system_is_parsed_from_its_own_sdl() {
        let types = meta_types();
        let mut declared: Vec<&str> = types.keys().map(String::as_str).collect();
        declared.sort_unstable();
        assert_eq!(
            declared,
            vec![
                "__Directive",
                "__DirectiveLocation",
                "__EnumValue",
                "__Field",
                "__InputValue",
                "__Schema",
                "__Type",
                "__TypeKind",
            ],
            "case \"the introspection system\": expected every type the system defines, got \
             {declared:?}"
        );
    }

    /// RULE: each of those types carries the members this introspection system declares for
    /// it, the field named after the SDL keyword `type` included — `__Directive` with the
    /// legacy fields the revision this service implements carries, and `__DirectiveLocation`
    /// with the locations it names.
    #[test]
    fn the_introspection_types_carry_the_members_graphql_declares() {
        let field = meta_type("__Field").expect("the system defines __Field");
        let members: Vec<&str> = field
            .fields
            .iter()
            .map(|member| member.name.as_str())
            .collect();
        assert_eq!(
            members,
            vec![
                "name",
                "description",
                "args",
                "type",
                "isDeprecated",
                "deprecationReason",
            ],
            "case \"__Field\": expected the fields GraphQL declares, got {members:?}"
        );
        let directive = meta_type("__Directive").expect("the system defines __Directive");
        let members: Vec<&str> = directive
            .fields
            .iter()
            .map(|member| member.name.as_str())
            .collect();
        assert_eq!(
            members,
            vec![
                "name",
                "description",
                "locations",
                "args",
                "onOperation",
                "onFragment",
                "onField",
            ],
            "case \"__Directive\": expected the fields this system declares, got {members:?}"
        );
        let locations = meta_type("__DirectiveLocation").expect("the system defines it");
        let named: Vec<&str> = locations
            .enum_values
            .iter()
            .map(|value| value.name.as_str())
            .collect();
        assert_eq!(
            named,
            vec![
                "QUERY",
                "MUTATION",
                "FIELD",
                "FRAGMENT_DEFINITION",
                "FRAGMENT_SPREAD",
                "INLINE_FRAGMENT",
                "SCHEMA",
                "SCALAR",
                "OBJECT",
                "FIELD_DEFINITION",
                "ARGUMENT_DEFINITION",
                "INTERFACE",
                "UNION",
                "ENUM",
                "ENUM_VALUE",
                "INPUT_OBJECT",
                "INPUT_FIELD_DEFINITION",
            ],
            "case \"__DirectiveLocation\": expected the locations this system names, got \
             {named:?}"
        );
        let kinds = meta_type("__TypeKind").expect("the system defines __TypeKind");
        let values: Vec<&str> = kinds
            .enum_values
            .iter()
            .map(|value| value.name.as_str())
            .collect();
        assert_eq!(
            values,
            vec![
                "SCALAR",
                "OBJECT",
                "INTERFACE",
                "UNION",
                "ENUM",
                "INPUT_OBJECT",
                "LIST",
                "NON_NULL",
            ],
            "case \"__TypeKind\": expected the kinds GraphQL names, got {values:?}"
        );
        assert_eq!(
            kinds.description.as_deref(),
            Some("An enum describing what kind of type a given __Type is"),
            "case \"__TypeKind\": expected the system's own description, got {:?}",
            kinds.description
        );
    }

    /// RULE: the five scalars GraphQL defines are described as built in; a scalar AppSync
    /// adds carries no description.
    #[test]
    fn the_built_in_scalars_carry_their_descriptions() {
        for (scalar, described) in [
            ("ID", Some("Built-in ID")),
            ("String", Some("Built-in String")),
            ("Boolean", Some("Built-in Boolean")),
            ("Int", Some("Built-in Int")),
            ("Float", Some("Built-in Float")),
            ("AWSDateTime", None),
        ] {
            assert_eq!(
                built_in_scalar_description(scalar),
                described,
                "case {scalar:?}: expected the description {described:?}, got {:?}",
                built_in_scalar_description(scalar)
            );
        }
    }
}
