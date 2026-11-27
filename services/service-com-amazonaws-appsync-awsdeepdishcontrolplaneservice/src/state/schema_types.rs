// SPDX-License-Identifier: CC0-1.0

//! The GraphQL types a GraphQL API holds: how one caller-supplied type definition is
//! parsed, kept and reported in either format the model names.
//!
//! A type arrives as "The type definition, in GraphQL Schema Definition Language (SDL)
//! format" (`CreateTypeRequest$definition`), so a stored type keeps the SDL text the
//! caller wrote together with the definition that text parses to. The
//! `TypeDefinitionFormat` a call names then chooses the REPRESENTATION the reported
//! `Type$definition` carries — the type's SDL, or its introspection JSON — and both are
//! computed from the definitions the API holds, never from the request: a READ answers
//! the service's own SDL rendering of the parsed type (`super::type_sdl`), while the call
//! that STATED a definition answers that definition as it was written.
//!
//! The parse is a real GraphQL parse (`apollo-parser`), so what the service accepts is
//! what GraphQL's SDL grammar accepts, and a definition it cannot parse is refused
//! rather than stored as text nothing understands.

use std::collections::BTreeMap;

use apollo_parser::cst::{self, CstNode};
use apollo_parser::Parser;
use serde_json::{json, Value};

use crate::ops::types::Type;

/// The `TypeDefinitionFormat` value naming a definition written in GraphQL SDL.
pub const SDL_FORMAT: &str = "SDL";

/// The `TypeDefinitionFormat` value naming a definition reported as the type's
/// introspection JSON.
pub const JSON_FORMAT: &str = "JSON";

/// The deepest nesting the GraphQL parser descends into one caller-supplied type
/// definition, and the most tokens it reads from one. `CreateTypeRequest$definition`
/// and `UpdateTypeRequest$definition` carry no modelled `@length`, so without these a
/// single request chooses how much stack and CPU one parse costs; past either bound the
/// parse fails and the request is refused.
pub const DEFINITION_RECURSION_LIMIT: usize = 64;

/// See [`DEFINITION_RECURSION_LIMIT`].
pub const DEFINITION_TOKEN_LIMIT: usize = 100_000;

/// The `__TypeKind` an SDL `type` definition has, as GraphQL's introspection names it.
pub const OBJECT_KIND: &str = "OBJECT";

/// The `__TypeKind` an SDL `interface` definition has.
pub const INTERFACE_KIND: &str = "INTERFACE";

/// The `__TypeKind` an SDL `input` definition has.
pub const INPUT_OBJECT_KIND: &str = "INPUT_OBJECT";

/// The `__TypeKind` an SDL `enum` definition has.
pub const ENUM_KIND: &str = "ENUM";

/// The `__TypeKind` an SDL `union` definition has.
pub const UNION_KIND: &str = "UNION";

/// The `__TypeKind` an SDL `scalar` definition has.
pub const SCALAR_KIND: &str = "SCALAR";

/// The `__TypeKind` a list type REFERENCE has (`[Post]`).
pub const LIST_KIND: &str = "LIST";

/// The `__TypeKind` a non-null type REFERENCE has (`Post!`).
pub const NON_NULL_KIND: &str = "NON_NULL";

/// The scalar types every AppSync schema holds without defining them: GraphQL's five
/// built-in scalars (`Int`, `Float`, `String`, `Boolean`, `ID`) and the nine AppSync
/// adds (`AWSDate`, `AWSTime`, `AWSDateTime`, `AWSTimestamp`, `AWSEmail`, `AWSJSON`,
/// `AWSURL`, `AWSPhone`, `AWSIPAddress`).
///
/// A reference to one of these names resolves to a [`SCALAR_KIND`] even though no
/// `CreateType` call defined it, which is what lets a field's type reference carry its
/// kind without the schema holding a definition for it.
pub const BUILT_IN_SCALARS: &[&str] = &[
    "Int",
    "Float",
    "String",
    "Boolean",
    "ID",
    "AWSDate",
    "AWSTime",
    "AWSDateTime",
    "AWSTimestamp",
    "AWSEmail",
    "AWSJSON",
    "AWSURL",
    "AWSPhone",
    "AWSIPAddress",
];

/// The directive a field or enum value is marked deprecated with.
pub(crate) const DEPRECATED_DIRECTIVE: &str = "deprecated";

/// The argument [`DEPRECATED_DIRECTIVE`] states its reason in.
pub(crate) const DEPRECATED_REASON_ARGUMENT: &str = "reason";

/// The reason introspection reports for a deprecation whose directive names none:
/// `@deprecated`'s `reason` argument has exactly this default in GraphQL's own
/// definition of the directive.
pub(crate) const DEFAULT_DEPRECATION_REASON: &str = "No longer supported";

/// One stored GraphQL type of a `GraphqlApiRecord`.
///
/// Keeps the definition as the caller wrote it AND the definition it parses to: the
/// first is what the call that stated it answers with, the second is what every REPORT of
/// the type is rendered from, so no two representations of one type can disagree.
#[derive(Debug, Clone)]
pub struct TypeRecord {
    /// Creation order, used to keep a listing (and its `nextToken` cursor) stable
    /// across calls.
    pub seq: u64,
    /// The type's ARN, named under its API's own path.
    pub arn: String,
    /// The SDL text the create (or the last update) carried, kept verbatim.
    pub definition: String,
    /// What [`Self::definition`] parses to.
    pub parsed: TypeDefinition,
}

impl TypeRecord {
    /// This record as the modelled `Type` shape a READ of the type answers with, with its
    /// definition in `format`.
    ///
    /// The single projection every operation that REPORTS a stored type answers with, so
    /// a type read on its own and the same type read in a listing carry the same body.
    /// Both representations are rendered from the parsed definition — the type's
    /// introspection JSON, or the service's own SDL printing of it
    /// ([`TypeDefinition::render_sdl`]) — so one type is reported with one document
    /// however it got into the API, and never with the text a request supplied.
    /// `api_types` is the set of types the type's own API holds, which the JSON rendering
    /// resolves type references against.
    pub fn to_type(&self, format: &str, api_types: &BTreeMap<String, TypeRecord>) -> Type {
        self.projected(format, self.rendered_definition(format, api_types))
    }

    /// This record as the modelled `Type` shape the call that STATED its definition —
    /// the create, or the update that carried one — answers with.
    ///
    /// Recorded behaviour: such a call reports the definition back as the request wrote
    /// it, while every later read reports the service's own rendering of the stored type
    /// ([`Self::to_type`]).
    pub fn to_stated_type(&self, format: &str, api_types: &BTreeMap<String, TypeRecord>) -> Type {
        self.projected(format, self.stated_definition(format, api_types))
    }

    /// This record as the modelled `Type` shape an UPDATE of its definition answers with.
    ///
    /// Recorded behaviour: the `Type` an update reports carries NO `name` — the member is
    /// absent from the body, unlike the one a read answers with — while its definition is
    /// the one the request stated ([`Self::to_stated_type`]). `Type$name` is an optional
    /// member, so the projection a call answers with is the operation's own.
    pub fn to_updated_type(&self, format: &str, api_types: &BTreeMap<String, TypeRecord>) -> Type {
        Type {
            name: None,
            ..self.to_stated_type(format, api_types)
        }
    }

    /// This record as the modelled `Type` shape a report that reaches the type THROUGH
    /// another API answers with: the definition and every other member exactly as
    /// [`Self::to_type`] reports them, named under the API `api_id` in `region`.
    ///
    /// A type is named under the API it is addressed through
    /// ([`type_resource`]), so a report reached through another API — an association
    /// listing, which addresses a Merged API and reports the types its source API's schema
    /// contributes — names each type under THAT API's own path, which is also the ARN the
    /// merged copy of the type carries (`super::schema_merge`).
    pub fn to_type_under_api(
        &self,
        format: &str,
        api_types: &BTreeMap<String, TypeRecord>,
        api_id: &str,
        region: &str,
    ) -> Type {
        Type {
            arn: Some(::engine::emulator_arn(
                super::ARN_SERVICE,
                region,
                &type_resource(api_id, &self.parsed.name),
            )),
            ..self.to_type(format, api_types)
        }
    }

    /// This record as the modelled `Type` shape carrying `definition`: every other member
    /// comes from the stored record whichever call is answering.
    fn projected(&self, format: &str, definition: String) -> Type {
        Type {
            name: Some(self.parsed.name.clone()),
            description: self.parsed.description.clone(),
            arn: Some(self.arn.clone()),
            definition: Some(definition),
            format: Some(format.to_string()),
        }
    }

    /// This type's definition in `format` as a READ reports it: the type's introspection
    /// JSON, or the SDL rendering of the parsed definition.
    fn rendered_definition(
        &self,
        format: &str,
        api_types: &BTreeMap<String, TypeRecord>,
    ) -> String {
        if format == JSON_FORMAT {
            self.introspection_json(api_types)
        } else {
            self.parsed.render_sdl()
        }
    }

    /// This type's definition in `format` as the call that stated it reports it: the
    /// type's introspection JSON, or the SDL text that request carried.
    fn stated_definition(&self, format: &str, api_types: &BTreeMap<String, TypeRecord>) -> String {
        if format == JSON_FORMAT {
            self.introspection_json(api_types)
        } else {
            self.definition.clone()
        }
    }

    /// This type's introspection JSON, as the text a `JSON` report carries.
    fn introspection_json(&self, api_types: &BTreeMap<String, TypeRecord>) -> String {
        self.parsed.render_json(api_types)
    }
}

impl TypeDefinition {
    /// This definition as a GraphQL introspection `__Type`: the JSON representation of a
    /// GraphQL type, as the introspection system defines it.
    ///
    /// One reader for every type a schema has, so a type an API holds, the same type
    /// reported as part of its schema (`super::introspection_schema`) and a type the
    /// introspection system itself defines cannot be reported differently.
    ///
    /// Every member is taken from the definition, and every type REFERENCE is resolved
    /// against the types the API holds plus the types every schema holds without defining
    /// them — its built-in scalars ([`BUILT_IN_SCALARS`]) and the introspection system's
    /// own ([`super::introspection_meta_schema`]) — so an interface reports the object
    /// types that implement it, and a reference to a type nothing has defined carries its
    /// name with no kind rather than a kind the schema does not support.
    pub(crate) fn introspection(&self, api_types: &BTreeMap<String, TypeRecord>) -> Value {
        let has_fields = self.kind == OBJECT_KIND || self.kind == INTERFACE_KIND;
        json!({
            "kind": self.kind,
            "name": self.name,
            "description": self.description,
            "fields": has_fields.then(|| {
                self
                    .fields
                    .iter()
                    .map(|field| field_json(field, api_types))
                    .collect::<Vec<Value>>()
            }),
            "inputFields": (self.kind == INPUT_OBJECT_KIND).then(|| {
                self
                    .input_fields
                    .iter()
                    .map(|input| input_value_json(input, api_types))
                    .collect::<Vec<Value>>()
            }),
            "interfaces": has_fields.then(|| {
                self
                    .interfaces
                    .iter()
                    .map(|name| named_type_json(name, api_types))
                    .collect::<Vec<Value>>()
            }),
            "enumValues": (self.kind == ENUM_KIND).then(|| {
                self
                    .enum_values
                    .iter()
                    .map(enum_value_json)
                    .collect::<Vec<Value>>()
            }),
            "possibleTypes": self.possible_types(api_types),
        })
    }

    /// The types this one can resolve to at runtime: a union's members, and the object
    /// types that implement an interface. Every other kind has none, which
    /// introspection reports as null rather than as an empty list.
    fn possible_types(&self, api_types: &BTreeMap<String, TypeRecord>) -> Value {
        match self.kind {
            UNION_KIND => Value::Array(
                self.members
                    .iter()
                    .map(|name| named_type_json(name, api_types))
                    .collect(),
            ),
            INTERFACE_KIND => Value::Array(
                api_types
                    .values()
                    .filter(|record| {
                        record.parsed.kind == OBJECT_KIND
                            && record.parsed.interfaces.contains(&self.name)
                    })
                    .map(|record| named_type_json(&record.parsed.name, api_types))
                    .collect(),
            ),
            _ => Value::Null,
        }
    }
}

/// One parsed GraphQL type definition: the type's kind, its name and the members that
/// kind carries.
///
/// Only the members the kind can hold are filled, so a projection reads them by kind
/// rather than guessing from what is empty.
///
/// Compared member by member where two source APIs declare one type name: two
/// declarations of a member that are EQUAL say the same thing and compose to one
/// (`super::combine_type_definitions`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TypeDefinition {
    /// The `__TypeKind` the SDL keyword declares ([`OBJECT_KIND`] and the rest).
    pub kind: &'static str,
    pub name: String,
    /// The definition's description, i.e. the SDL description string written in front
    /// of it, which is what a GraphQL type's description is.
    pub description: Option<String>,
    /// The interfaces an object or interface type implements.
    pub interfaces: Vec<String>,
    /// The fields an object or interface type declares.
    pub fields: Vec<FieldDefinition>,
    /// The input fields an input object type declares.
    pub input_fields: Vec<InputValueDefinition>,
    /// The values an enum type declares.
    pub enum_values: Vec<EnumValueDefinition>,
    /// The member types a union type names.
    pub members: Vec<String>,
    /// The directives written on the definition itself, in the order it writes them.
    pub directives: Vec<DirectiveUse>,
}

/// One field of a parsed object or interface type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldDefinition {
    pub name: String,
    pub description: Option<String>,
    pub arguments: Vec<InputValueDefinition>,
    /// The type the field resolves to, or `None` for a field whose definition names
    /// none.
    pub value_type: Option<TypeRef>,
    pub deprecation: Deprecation,
    /// The directives written on the field, in the order the definition writes them.
    pub directives: Vec<DirectiveUse>,
}

/// One argument of a field, or one field of an input object type: the two are the same
/// shape in GraphQL (`__InputValue`) and in SDL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputValueDefinition {
    pub name: String,
    pub description: Option<String>,
    pub value_type: Option<TypeRef>,
    /// The default value as GraphQL writes it, which is the form introspection reports
    /// it in ("a GraphQL-formatted string representing the default value").
    pub default_value: Option<String>,
    /// The directives written on the argument or input field, in the order the
    /// definition writes them.
    pub directives: Vec<DirectiveUse>,
}

/// One value of a parsed enum type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnumValueDefinition {
    pub name: String,
    pub description: Option<String>,
    pub deprecation: Deprecation,
    /// The directives written on the value, in the order the definition writes them.
    pub directives: Vec<DirectiveUse>,
}

/// One directive written on a type definition or on a member of one: the directive's
/// name and the arguments it states.
///
/// A directive is part of the definition it is written on — GraphQL's `@deprecated` and
/// AppSync's own authorization directives are what a schema states ABOUT a type — so a
/// stored definition keeps every one of them and every report of the type writes them
/// back, rather than reporting a definition the caller never stated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectiveUse {
    /// The directive's name, without the leading `@`.
    pub name: String,
    /// The arguments the directive states, in the order it writes them.
    pub arguments: Vec<DirectiveArgument>,
}

/// One argument of a [`DirectiveUse`]: its name, its value as GraphQL writes one — so a
/// string value carries its quotation and a list or object value its brackets — and the
/// STRING values that value carries, read by the parser.
///
/// Both forms are kept because they answer different questions. The written text is what a
/// report of the type writes back, verbatim, so a directive reads as the definition declared
/// it. The strings are what a RULE about the directive is judged on — the groups an
/// `@aws_auth` guard names, the mutations an `@aws_subscribe` link names — and they are the
/// parser's own `StringValue`s, so GraphQL's escaping is applied by `apollo-parser` rather
/// than by a second reading of its string syntax.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectiveArgument {
    pub name: String,
    pub value: String,
    /// The string values the argument's value carries: one for a string, one per string
    /// element for a list of them, and none for a value that carries no string at all.
    pub strings: Vec<String>,
}

/// Whether a field or enum value is deprecated, and why.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Deprecation {
    pub is_deprecated: bool,
    pub reason: Option<String>,
}

/// A reference to a type from a field or an argument: a name, a list of them, or a
/// non-null one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypeRef {
    Named(String),
    List(Box<TypeRef>),
    NonNull(Box<TypeRef>),
}

/// One wrapper a type reference nests another reference in, as GraphQL's type system
/// defines the two: a list (`[T]`) and a non-null (`T!`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypeWrapper {
    List,
    NonNull,
}

/// Why a caller-supplied type definition is not one type definition this service can
/// store.
///
/// The ground is kept apart from the TEXT the refusal carries: the service answers one
/// fixed sentence for every ground ([`DEFINITION_NOT_READABLE_MESSAGE`]), so these
/// variants say what was wrong for this crate's own readers, not for the caller.
#[derive(Debug, Clone)]
pub enum TypeDefinitionError {
    /// The definition is not well-formed GraphQL.
    Unparseable,
    /// The definition parses but declares no type (an empty document, or one that only
    /// declares something a type cannot be, such as an operation or a directive).
    NoTypeDeclared,
    /// The definition declares more than one type, and `CreateType` "Creates a Type
    /// object" — one — so there is no single type for the call to answer with.
    MoreThanOneTypeDeclared,
}

/// The text a `BadRequestException` (HTTP 400) carries for a definition that cannot be
/// read as one type definition in the declared `TypeDefinitionFormat`.
///
/// Recorded behaviour: the sentence is entirely FIXED — it reports neither which token
/// nor which position failed, nor which of the grounds applies — so the parser's own
/// diagnostic stays internal and every ground answers the same text.
pub const DEFINITION_NOT_READABLE_MESSAGE: &str = "Unable to parse provided type definition.";

impl TypeDefinitionError {
    /// The text a `BadRequestException` (HTTP 400) carries for this refusal.
    ///
    /// One sentence for every ground ([`DEFINITION_NOT_READABLE_MESSAGE`]), shared by
    /// every operation that reads a caller-supplied definition, so `CreateType` and
    /// `UpdateType` cannot disagree about how an unreadable definition is reported.
    pub fn message(&self) -> String {
        DEFINITION_NOT_READABLE_MESSAGE.to_string()
    }
}

/// Parse one caller-supplied type definition, written in GraphQL SDL.
///
/// A type definition declares exactly one type — `CreateType` "Creates a Type object"
/// and addresses it by that type's name afterwards — so a document that declares none,
/// or more than one, is refused rather than stored.
///
/// The name a definition declares is a GraphQL name (`[_A-Za-z][_0-9A-Za-z]*`), which
/// is the alphabet the modelled `ResourceName` admits, so a parsed name can always be
/// addressed back by `GetType`, `UpdateType` and `DeleteType`.
///
/// The parse is bounded by [`DEFINITION_RECURSION_LIMIT`] and
/// [`DEFINITION_TOKEN_LIMIT`], so neither the depth nor the amount of work one
/// caller-supplied definition costs comes from the request.
pub fn parse_type_definition(definition: &str) -> Result<TypeDefinition, TypeDefinitionError> {
    let tree = Parser::new(definition)
        .recursion_limit(DEFINITION_RECURSION_LIMIT)
        .token_limit(DEFINITION_TOKEN_LIMIT)
        .parse();
    // The parser's account of what is wrong with the definition is NOT reported: the
    // refusal's sentence is fixed, so only the fact that the parse failed is kept.
    if tree.errors().next().is_some() {
        return Err(TypeDefinitionError::Unparseable);
    }

    let mut declared: Option<TypeDefinition> = None;
    for node in tree.document().definitions() {
        // Everything else a GraphQL document can declare — an operation, a fragment, a
        // directive, a schema, a type EXTENSION — declares no type, so it leaves nothing
        // for this call to create.
        let Some(parsed) = declared_type(&node) else {
            return Err(TypeDefinitionError::NoTypeDeclared);
        };
        if declared.replace(parsed?).is_some() {
            return Err(TypeDefinitionError::MoreThanOneTypeDeclared);
        }
    }

    declared.ok_or(TypeDefinitionError::NoTypeDeclared)
}

/// The type `node` declares, or `None` for a definition that declares no type at all.
///
/// The one reading of what each SDL type keyword declares, so a type parsed out of a
/// single definition and one parsed out of a whole schema document cannot differ.
fn declared_type(
    node: &cst::Definition,
) -> Option<Result<TypeDefinition, TypeDefinitionError>> {
    let parsed = match node {
        cst::Definition::ObjectTypeDefinition(object) => (|| {
            Ok(TypeDefinition {
                kind: OBJECT_KIND,
                name: name_text(object.name())?,
                description: description_text(object.description()),
                interfaces: implemented_interfaces(object.implements_interfaces()),
                fields: field_definitions(object.fields_definition()),
                directives: directive_uses(object.directives()),
                ..TypeDefinition::default()
            })
        })(),
        cst::Definition::InterfaceTypeDefinition(interface) => (|| {
            Ok(TypeDefinition {
                kind: INTERFACE_KIND,
                name: name_text(interface.name())?,
                description: description_text(interface.description()),
                interfaces: implemented_interfaces(interface.implements_interfaces()),
                fields: field_definitions(interface.fields_definition()),
                directives: directive_uses(interface.directives()),
                ..TypeDefinition::default()
            })
        })(),
        cst::Definition::InputObjectTypeDefinition(input) => (|| {
            Ok(TypeDefinition {
                kind: INPUT_OBJECT_KIND,
                name: name_text(input.name())?,
                description: description_text(input.description()),
                input_fields: input_field_definitions(input.input_fields_definition()),
                directives: directive_uses(input.directives()),
                ..TypeDefinition::default()
            })
        })(),
        cst::Definition::EnumTypeDefinition(enumeration) => (|| {
            Ok(TypeDefinition {
                kind: ENUM_KIND,
                name: name_text(enumeration.name())?,
                description: description_text(enumeration.description()),
                enum_values: enum_value_definitions(enumeration.enum_values_definition()),
                directives: directive_uses(enumeration.directives()),
                ..TypeDefinition::default()
            })
        })(),
        cst::Definition::UnionTypeDefinition(union) => (|| {
            Ok(TypeDefinition {
                kind: UNION_KIND,
                name: name_text(union.name())?,
                description: description_text(union.description()),
                members: union_members(union.union_member_types()),
                directives: directive_uses(union.directives()),
                ..TypeDefinition::default()
            })
        })(),
        cst::Definition::ScalarTypeDefinition(scalar) => (|| {
            Ok(TypeDefinition {
                kind: SCALAR_KIND,
                name: name_text(scalar.name())?,
                description: description_text(scalar.description()),
                directives: directive_uses(scalar.directives()),
                ..TypeDefinition::default()
            })
        })(),
        _ => return None,
    };
    Some(parsed)
}

/// The type `node` EXTENDS, as the members the extension contributes to it, or `None`
/// for a definition that extends no type.
///
/// An extension carries no description of its own — GraphQL's SDL grammar gives a
/// description only to a definition — so what it contributes is members alone.
fn extended_type(node: &cst::Definition) -> Option<Result<TypeDefinition, TypeDefinitionError>> {
    let parsed = match node {
        cst::Definition::ObjectTypeExtension(object) => (|| {
            Ok(TypeDefinition {
                kind: OBJECT_KIND,
                name: name_text(object.name())?,
                interfaces: implemented_interfaces(object.implements_interfaces()),
                fields: field_definitions(object.fields_definition()),
                directives: directive_uses(object.directives()),
                ..TypeDefinition::default()
            })
        })(),
        cst::Definition::InterfaceTypeExtension(interface) => (|| {
            Ok(TypeDefinition {
                kind: INTERFACE_KIND,
                name: name_text(interface.name())?,
                interfaces: implemented_interfaces(interface.implements_interfaces()),
                fields: field_definitions(interface.fields_definition()),
                directives: directive_uses(interface.directives()),
                ..TypeDefinition::default()
            })
        })(),
        cst::Definition::InputObjectTypeExtension(input) => (|| {
            Ok(TypeDefinition {
                kind: INPUT_OBJECT_KIND,
                name: name_text(input.name())?,
                input_fields: input_field_definitions(input.input_fields_definition()),
                directives: directive_uses(input.directives()),
                ..TypeDefinition::default()
            })
        })(),
        cst::Definition::EnumTypeExtension(enumeration) => (|| {
            Ok(TypeDefinition {
                kind: ENUM_KIND,
                name: name_text(enumeration.name())?,
                enum_values: enum_value_definitions(enumeration.enum_values_definition()),
                directives: directive_uses(enumeration.directives()),
                ..TypeDefinition::default()
            })
        })(),
        cst::Definition::UnionTypeExtension(union) => (|| {
            Ok(TypeDefinition {
                kind: UNION_KIND,
                name: name_text(union.name())?,
                members: union_members(union.union_member_types()),
                directives: directive_uses(union.directives()),
                ..TypeDefinition::default()
            })
        })(),
        cst::Definition::ScalarTypeExtension(scalar) => (|| {
            Ok(TypeDefinition {
                kind: SCALAR_KIND,
                name: name_text(scalar.name())?,
                directives: directive_uses(scalar.directives()),
                ..TypeDefinition::default()
            })
        })(),
        _ => return None,
    };
    Some(parsed)
}

/// The input fields a `{ … }` input field set declares.
fn input_field_definitions(
    fields: Option<cst::InputFieldsDefinition>,
) -> Vec<InputValueDefinition> {
    fields
        .map(|fields| {
            fields
                .input_value_definitions()
                .map(input_value_definition)
                .collect()
        })
        .unwrap_or_default()
}

/// The values an `{ … }` enum value set declares.
fn enum_value_definitions(
    values: Option<cst::EnumValuesDefinition>,
) -> Vec<EnumValueDefinition> {
    values
        .map(|values| {
            values
                .enum_value_definitions()
                .map(enum_value_definition)
                .collect()
        })
        .unwrap_or_default()
}

/// The member types a `= A | B` union member list names.
fn union_members(members: Option<cst::UnionMemberTypes>) -> Vec<String> {
    members
        .map(|members| named_type_names(members.named_types()))
        .unwrap_or_default()
}

/// The text of a declared name, or [`TypeDefinitionError::NoTypeDeclared`] for a
/// definition that names nothing.
fn name_text(name: Option<cst::Name>) -> Result<String, TypeDefinitionError> {
    name.map(|name| name.text().to_string())
        .filter(|name| !name.is_empty())
        .ok_or(TypeDefinitionError::NoTypeDeclared)
}

/// The text of an SDL description, which is a string value written in front of the
/// definition it describes.
fn description_text(description: Option<cst::Description>) -> Option<String> {
    description
        .and_then(|description| description.string_value())
        .map(String::from)
}

/// The names an `implements` clause lists.
fn implemented_interfaces(interfaces: Option<cst::ImplementsInterfaces>) -> Vec<String> {
    interfaces
        .map(|interfaces| named_type_names(interfaces.named_types()))
        .unwrap_or_default()
}

/// The names of a list of named type references.
fn named_type_names(types: impl Iterator<Item = cst::NamedType>) -> Vec<String> {
    types
        .filter_map(|named| named.name().map(|name| name.text().to_string()))
        .collect()
}

/// The fields a `{ … }` field set declares, in the order the definition writes them —
/// the order introspection reports them in.
fn field_definitions(fields: Option<cst::FieldsDefinition>) -> Vec<FieldDefinition> {
    fields
        .map(|fields| {
            fields
                .field_definitions()
                .map(|field| FieldDefinition {
                    name: field
                        .name()
                        .map(|name| name.text().to_string())
                        .unwrap_or_default(),
                    description: description_text(field.description()),
                    arguments: field
                        .arguments_definition()
                        .map(|arguments| {
                            arguments
                                .input_value_definitions()
                                .map(input_value_definition)
                                .collect()
                        })
                        .unwrap_or_default(),
                    value_type: type_reference(field.ty()),
                    deprecation: deprecation(field.directives()),
                    directives: directive_uses(field.directives()),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// One argument or input field as its parsed form.
pub(crate) fn input_value_definition(input: cst::InputValueDefinition) -> InputValueDefinition {
    InputValueDefinition {
        name: input
            .name()
            .map(|name| name.text().to_string())
            .unwrap_or_default(),
        description: description_text(input.description()),
        value_type: type_reference(input.ty()),
        default_value: input
            .default_value()
            .and_then(|default| default.value())
            .map(|value| value.syntax().text().to_string().trim().to_string()),
        directives: directive_uses(input.directives()),
    }
}

/// One enum value as its parsed form.
fn enum_value_definition(value: cst::EnumValueDefinition) -> EnumValueDefinition {
    EnumValueDefinition {
        name: value
            .enum_value()
            .and_then(|value| value.name())
            .map(|name| name.text().to_string())
            .unwrap_or_default(),
        description: description_text(value.description()),
        deprecation: deprecation(value.directives()),
        directives: directive_uses(value.directives()),
    }
}

/// The directives a definition or one of its members carries, each with the arguments it
/// states, in the order the definition writes them.
///
/// What is kept is what the definition WROTE: an argument's value is kept as GraphQL
/// writes one, so a report of the type can write the directive back as the type declares
/// it instead of dropping it.
fn directive_uses(directives: Option<cst::Directives>) -> Vec<DirectiveUse> {
    let Some(directives) = directives else {
        return Vec::new();
    };
    directives
        .directives()
        .filter_map(|directive| {
            let name = directive.name()?.text().to_string();
            let arguments = directive
                .arguments()
                .map(|arguments| {
                    arguments
                        .arguments()
                        .filter_map(|argument| {
                            Some(DirectiveArgument {
                                name: argument.name()?.text().to_string(),
                                strings: argument_strings(argument.value().as_ref()),
                                value: argument
                                    .value()?
                                    .syntax()
                                    .text()
                                    .to_string()
                                    .trim()
                                    .to_string(),
                            })
                        })
                        .collect()
                })
                .unwrap_or_default();
            Some(DirectiveUse { name, arguments })
        })
        .collect()
}

/// The STRING values one directive argument's value carries, as the parser read them: the
/// value itself for a string, each string element for a list of them, and none for anything
/// else.
///
/// Read from `apollo-parser`'s own `StringValue`, which applies GraphQL's escaping, so a rule
/// judged on these names (an `@aws_auth` group, an `@aws_subscribe` mutation) is judged on
/// the value the schema wrote rather than on a re-reading of its quotation.
fn argument_strings(value: Option<&cst::Value>) -> Vec<String> {
    match value {
        Some(cst::Value::StringValue(text)) => vec![String::from(text.clone())],
        Some(cst::Value::ListValue(list)) => list
            .values()
            .filter_map(|element| match element {
                cst::Value::StringValue(text) => Some(String::from(text)),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// The type a field or argument names, as a reference tree.
///
/// Public because a type reference is read wherever a GraphQL type is named: beside the type
/// definitions this module parses, the data plane coerces a request's variable values against
/// the references the operation's variable definitions name.
pub fn type_reference(value_type: Option<cst::Type>) -> Option<TypeRef> {
    match value_type? {
        cst::Type::NamedType(named) => Some(TypeRef::Named(
            named.name().map(|name| name.text().to_string())?,
        )),
        cst::Type::ListType(list) => Some(TypeRef::List(Box::new(type_reference(list.ty())?))),
        cst::Type::NonNullType(non_null) => {
            let inner = match (non_null.named_type(), non_null.list_type()) {
                (Some(named), _) => {
                    TypeRef::Named(named.name().map(|name| name.text().to_string())?)
                }
                (_, Some(list)) => TypeRef::List(Box::new(type_reference(list.ty())?)),
                (None, None) => return None,
            };
            Some(TypeRef::NonNull(Box::new(inner)))
        }
    }
}

/// Whether the directives a field or enum value carries deprecate it, and the reason
/// they state.
fn deprecation(directives: Option<cst::Directives>) -> Deprecation {
    let Some(directives) = directives else {
        return Deprecation::default();
    };
    for directive in directives.directives() {
        if directive
            .name()
            .map(|name| name.text().to_string())
            .is_none_or(|name| name != DEPRECATED_DIRECTIVE)
        {
            continue;
        }
        let reason = directive
            .arguments()
            .and_then(|arguments| {
                arguments
                    .arguments()
                    .filter(|argument| {
                        argument
                            .name()
                            .is_some_and(|name| name.text() == DEPRECATED_REASON_ARGUMENT)
                    })
                    .find_map(|argument| match argument.value() {
                        Some(cst::Value::StringValue(reason)) => Some(String::from(reason)),
                        _ => None,
                    })
            })
            .unwrap_or_else(|| DEFAULT_DEPRECATION_REASON.to_string());
        return Deprecation {
            is_deprecated: true,
            reason: Some(reason),
        };
    }
    Deprecation::default()
}

/// One field as introspection's `__Field`.
fn field_json(field: &FieldDefinition, api_types: &BTreeMap<String, TypeRecord>) -> Value {
    json!({
        "name": field.name,
        "description": field.description,
        "args": field
            .arguments
            .iter()
            .map(|argument| input_value_json(argument, api_types))
            .collect::<Vec<Value>>(),
        "type": type_reference_json(field.value_type.as_ref(), api_types),
        "isDeprecated": field.deprecation.is_deprecated,
        "deprecationReason": field.deprecation.reason,
    })
}

/// One argument or input field as introspection's `__InputValue`.
pub(crate) fn input_value_json(
    input: &InputValueDefinition,
    api_types: &BTreeMap<String, TypeRecord>,
) -> Value {
    json!({
        "name": input.name,
        "description": input.description,
        "type": type_reference_json(input.value_type.as_ref(), api_types),
        "defaultValue": input.default_value,
    })
}

/// One enum value as introspection's `__EnumValue`.
fn enum_value_json(value: &EnumValueDefinition) -> Value {
    json!({
        "name": value.name,
        "description": value.description,
        "isDeprecated": value.deprecation.is_deprecated,
        "deprecationReason": value.deprecation.reason,
    })
}

/// A type reference as introspection's wrapping `__Type`: a list and a non-null
/// reference carry their inner reference in `ofType` and no name of their own.
fn type_reference_json(
    value_type: Option<&TypeRef>,
    api_types: &BTreeMap<String, TypeRecord>,
) -> Value {
    match value_type {
        None => Value::Null,
        Some(TypeRef::Named(name)) => named_type_json(name, api_types),
        Some(TypeRef::List(inner)) => json!({
            "kind": LIST_KIND,
            "name": Value::Null,
            "ofType": type_reference_json(Some(inner.as_ref()), api_types),
        }),
        Some(TypeRef::NonNull(inner)) => json!({
            "kind": NON_NULL_KIND,
            "name": Value::Null,
            "ofType": type_reference_json(Some(inner.as_ref()), api_types),
        }),
    }
}

/// A named type reference as introspection's `__Type`, with the kind the API's stored
/// types give that name.
fn named_type_json(name: &str, api_types: &BTreeMap<String, TypeRecord>) -> Value {
    json!({
        "kind": kind_of(name, api_types),
        "name": name,
        "ofType": Value::Null,
    })
}

/// The kind the type `name` has in this API: the kind of its stored definition, the kind
/// the introspection system's own definition of it declares
/// ([`super::introspection_meta_schema`]), or [`SCALAR_KIND`] for a scalar every schema
/// holds ([`BUILT_IN_SCALARS`]). `None` when nothing defines it, which is reported as a
/// reference with no kind rather than as a guess.
///
/// The introspection system's types are types of EVERY schema, so a reference to one
/// resolves whether or not the API defines anything — and GraphQL reserves their `__`
/// prefix, so no type an API holds can carry one of those names.
fn kind_of(name: &str, api_types: &BTreeMap<String, TypeRecord>) -> Option<&'static str> {
    if BUILT_IN_SCALARS.contains(&name) {
        return Some(SCALAR_KIND);
    }
    api_types
        .get(name)
        .map(|record| record.parsed.kind)
        .or_else(|| super::introspection_meta_schema::meta_type_kind(name))
}

/// The AppSync service quota for types on ONE GraphQL API.
///
/// It bounds the per-API collection however a type got into it — created one at a time
/// by `CreateType` or installed by a whole schema document — so neither path can grow
/// the store without limit.
pub const TYPES_PER_API: usize = 2_500;

/// The resource portion of a type's ARN: a type is a child of its API, so it is named
/// under the API's own `apis/{apiId}` path, as `GetType`'s URI
/// (`/v1/apis/{apiId}/types/{typeName}`) addresses it.
pub fn type_resource(api_id: &str, type_name: &str) -> String {
    format!("apis/{api_id}/types/{type_name}")
}

/// The text a `NotFoundException` (HTTP 404) carries for a type the named API does not
/// hold.
///
/// The service words this one as a FIXED sentence with no variable part — it names the
/// kind of resource that was not found and nothing of the request — so there is nothing
/// to interpolate and this is a constant rather than a template.
///
/// Shared by every operation that addresses a type by name, so they cannot disagree
/// about how a missing type is reported. The wording is the one a recorded `DeleteType`
/// refusal carries; should a read ever be recorded answering a different sentence, the
/// readers would need their own text rather than this one.
pub const TYPE_NOT_FOUND_MESSAGE: &str = "Type not found.";

/// The text a `BadRequestException` (HTTP 400) carries for a create whose type name is
/// already taken within the API: the name, which is the member the caller has to
/// change.
///
/// Worded after the service's other name-taken sentence
/// (`crate::state::data_source_exists_message`).
pub fn type_exists_message(type_name: &str) -> String {
    format!("Type with name {type_name} already exists.")
}

/// The text a `BadRequestException` (HTTP 400) carries for an update whose new definition
/// declares a type other than the one the request addresses: both names, which are the two
/// members the caller has to bring together.
///
/// A type is addressed by name within its API and its definition is the definition OF that
/// type, so a definition declaring another name defines no type the request can update.
/// This is a definition the service CAN read — so it is not the unreadable-definition
/// refusal ([`DEFINITION_NOT_READABLE_MESSAGE`]) — and the sentence names the two type
/// names the caller has to reconcile, taken from the request itself.
pub fn type_name_mismatch_message(addressed: &str, declared: &str) -> String {
    format!("Provided type definition must declare the type {addressed}, not {declared}.")
}

/// One type a whole schema document declares: the SDL text the document writes it in
/// and the definition that text parses to.
///
/// The same pair a single `CreateType` stores ([`TypeRecord`]), so a type that arrived
/// inside a schema document and one created on its own are stored and reported
/// identically.
#[derive(Debug, Clone)]
pub struct DeclaredType {
    /// The document's own text for this type, the definition and every extension of it
    /// the document writes, in document order.
    pub definition: String,
    /// What [`Self::definition`] parses to.
    pub parsed: TypeDefinition,
}

/// Why a caller-supplied schema document is not a GraphQL schema this service can
/// create.
///
/// Each ground is a rule of GraphQL's own type system, checked against the parsed
/// document rather than against the request's text.
#[derive(Debug, Clone)]
pub enum SchemaDocumentError {
    /// The document is not well-formed GraphQL: reading it as GraphQL stops at this
    /// line and column — the line counted from one and the column from zero, the way
    /// the service's own diagnostic states a position.
    Unparseable {
        line: usize,
        column: usize,
    },
    /// The document declares no type at all, so it defines no schema.
    NoTypeDeclared,
    /// The document declares a type that carries no name.
    TypeWithoutName,
    /// The document declares one type name twice: a GraphQL type system defines each
    /// name once, so the second declaration has no type of its own to be.
    TypeDeclaredTwice(String),
    /// The document extends a type it never declares, which leaves the extension's
    /// members on no type.
    ExtendedTypeNotDeclared(String),
    /// Types of the document reference names nothing declares and that are no scalar
    /// every schema holds, so those references resolve to no type — every one of them,
    /// in the order the document writes them, because the schema has as many problems as
    /// it has unresolved references.
    UndefinedTypesReferenced(Vec<UndefinedTypeReference>),
    /// The document declares more types than an API can hold ([`TYPES_PER_API`]), so
    /// the type system it defines does not fit on one API.
    MoreTypesThanTheQuota(usize),
}

/// One reference of a schema document that resolves to no type.
#[derive(Debug, Clone)]
pub struct UndefinedTypeReference {
    /// The type whose definition carries the reference.
    pub type_name: String,
    /// The name it references.
    pub referenced: String,
}

/// The text a document that is not well-formed GraphQL is reported with: the parser's
/// own error list, naming the position in the document where reading it stops.
///
/// Recorded behaviour supplies this sentence — an API whose schema was never created is
/// answered with the parse of its EMPTY document — and everything but the line and the
/// column is fixed text. The line is counted from one and the column from zero, which is
/// where that recorded diagnostic puts an empty document's origin ([`DOCUMENT_ORIGIN`]).
pub fn invalid_syntax_detail(line: usize, column: usize) -> String {
    format!(
        "errors=[InvalidSyntaxError{{ message=Invalid Syntax ,locations=\
         [SourceLocation{{line={line}, column={column}}}]}}]"
    )
}

/// The text a document whose TYPE SYSTEM does not hold together is reported with: how
/// many problems the schema has, and each problem on its own line.
///
/// Recorded behaviour supplies the envelope and the problem sentence for a field whose
/// type the schema does not define ([`SchemaDocumentError::detail`]).
pub fn schema_problems_detail(problems: &[String]) -> String {
    format!(
        "Found {} problem(s) with the schema:\n{}",
        problems.len(),
        problems.join("\n")
    )
}

/// The line and column `index` falls at in `document`: the line counted from one and the
/// column from zero, the way the service's own diagnostic states a position
/// ([`DOCUMENT_ORIGIN`]).
fn source_location(document: &str, index: usize) -> (usize, usize) {
    let Some(read) = document.get(..index) else {
        return DOCUMENT_ORIGIN;
    };
    let line = read.matches('\n').count() + 1;
    let column = read
        .rsplit_once('\n')
        .map_or(read, |(_, last)| last)
        .chars()
        .count();
    (line, column)
}

/// A GraphQL document's own origin, as the position the service's diagnostic states:
/// line one, column zero. Reading an EMPTY document — the document an API that holds no
/// schema has — stops there, which is the position recorded behaviour reports for it.
pub const DOCUMENT_ORIGIN: (usize, usize) = (1, 0);

impl SchemaDocumentError {
    /// The detail a failed schema creation reports: what about the document has to be
    /// fixed.
    ///
    /// This is the `details` of the schema status, "Detailed information about the
    /// status of the schema creation operation", and it is what the schema itself is
    /// reported as invalid with when a later call asks for it
    /// (`GraphqlApiRecord::introspection_schema`), so the two cannot disagree about one
    /// document. A document that does not PARSE is reported as the parser's error list
    /// ([`invalid_syntax_detail`]); one that parses but defines no type system that
    /// holds together is reported as that system's problems
    /// ([`schema_problems_detail`]).
    pub fn detail(&self) -> String {
        match self {
            Self::Unparseable { line, column } => invalid_syntax_detail(*line, *column),
            _ => schema_problems_detail(&self.problems()),
        }
    }

    /// The problems the type system this document defines has, one sentence each.
    ///
    /// The sentence for a reference the schema does not define is the service's own
    /// (recorded behaviour); the remaining grounds — none of which a recording states —
    /// name the document and what about it has to be fixed.
    fn problems(&self) -> Vec<String> {
        match self {
            // Reading the document failed before any type system was read out of it, so
            // it has no type-system problem to report; the parse is reported instead
            // (see `detail`).
            Self::Unparseable { line, column } => vec![invalid_syntax_detail(*line, *column)],
            Self::NoTypeDeclared => {
                vec!["The schema document declares no GraphQL type.".to_string()]
            }
            Self::TypeWithoutName => {
                vec!["The schema document declares a type without a name.".to_string()]
            }
            Self::TypeDeclaredTwice(name) => vec![format!(
                "The schema document declares the type {name} more than once."
            )],
            Self::ExtendedTypeNotDeclared(name) => vec![format!(
                "The schema document extends the type {name}, which it does not declare."
            )],
            Self::UndefinedTypesReferenced(references) => references
                .iter()
                .map(|reference| {
                    format!(
                        "The field type '{}' is not present when resolving type '{}'.",
                        reference.referenced, reference.type_name
                    )
                })
                .collect(),
            Self::MoreTypesThanTheQuota(declared) => vec![format!(
                "The schema document declares {declared} types, more than the \
                 {TYPES_PER_API} an API can hold."
            )],
        }
    }
}

/// Parse one caller-supplied schema document, written in GraphQL schema language, into
/// the types it declares — in document order — beside how many definitions it states
/// ([`SchemaDocument`]).
///
/// A schema document declares a whole type system rather than the one type
/// [`parse_type_definition`] reads, so every type definition it writes is kept, a type
/// extension contributes its members to the type of that name, and the definitions a
/// type system carries that are no type at all (a `schema` definition naming the root
/// operation types, a directive definition) declare no type and are left to the
/// document.
///
/// The type system the document defines has to hold together, so the parse refuses a
/// document that declares nothing, declares one name twice, extends a type it does not
/// declare, or references a type neither it nor GraphQL defines
/// ([`BUILT_IN_SCALARS`]) — each the rule GraphQL's own type system states, and each
/// reported as the detail of a FAILED creation rather than stored as a schema nothing
/// resolves.
///
/// The parse is bounded by [`DEFINITION_RECURSION_LIMIT`] and
/// [`DEFINITION_TOKEN_LIMIT`], the same bounds a single definition is read under, so
/// neither the depth nor the amount of work one caller-supplied document costs comes
/// from the request.
pub fn parse_schema_document(definition: &str) -> Result<SchemaDocument, SchemaDocumentError> {
    let tree = Parser::new(definition)
        .recursion_limit(DEFINITION_RECURSION_LIMIT)
        .token_limit(DEFINITION_TOKEN_LIMIT)
        .parse();
    if let Some(error) = tree.errors().next() {
        let (line, column) = source_location(definition, error.index());
        return Err(SchemaDocumentError::Unparseable { line, column });
    }

    // How much the document STATES, which is not how many types it declares: a `schema`
    // definition and a directive definition declare no type (see this function's own
    // account of them), and what a successful creation details is counted over all of them
    // (`super::schema_success_detail`).
    let definitions = tree.document().definitions().count();

    // Declaration order is the document's, which is the order a listing reports a
    // schema's types in, so it is kept beside the by-name index the checks below need.
    let mut order: Vec<String> = Vec::new();
    let mut declared: BTreeMap<String, DeclaredType> = BTreeMap::new();
    let mut extensions: Vec<(TypeDefinition, String)> = Vec::new();

    for node in tree.document().definitions() {
        let text = node.syntax().text().to_string().trim().to_string();
        if let Some(parsed) = declared_type(&node) {
            let parsed = parsed.map_err(|_| SchemaDocumentError::TypeWithoutName)?;
            let name = parsed.name.clone();
            if declared.contains_key(&name) {
                return Err(SchemaDocumentError::TypeDeclaredTwice(name));
            }
            order.push(name.clone());
            declared.insert(
                name,
                DeclaredType {
                    definition: text,
                    parsed,
                },
            );
            continue;
        }
        if let Some(parsed) = extended_type(&node) {
            let parsed = parsed.map_err(|_| SchemaDocumentError::TypeWithoutName)?;
            extensions.push((parsed, text));
        }
    }

    // An extension contributes to the type of its name, so it is applied after the
    // whole document has been read: GraphQL does not require a type to be declared
    // before the extension that extends it.
    for (extension, text) in extensions {
        let Some(target) = declared.get_mut(&extension.name) else {
            return Err(SchemaDocumentError::ExtendedTypeNotDeclared(extension.name));
        };
        target.parsed.extend_with(extension);
        target.definition.push_str("\n\n");
        target.definition.push_str(&text);
    }

    if order.is_empty() {
        return Err(SchemaDocumentError::NoTypeDeclared);
    }

    // The type system a document installs is bounded by the same per-API quota a type
    // created on its own counts against, so one document cannot grow the store past it.
    if order.len() > TYPES_PER_API {
        return Err(SchemaDocumentError::MoreTypesThanTheQuota(order.len()));
    }

    // Every type reference of the finished type system has to resolve, so a document
    // that names a type it does not define is refused rather than stored as a schema
    // whose fields resolve to nothing. EVERY unresolved reference is collected: the
    // schema has as many problems as it has of them, and the detail states each.
    let mut undefined: Vec<UndefinedTypeReference> = Vec::new();
    for name in &order {
        let Some(entry) = declared.get(name) else {
            continue;
        };
        for referenced in entry.parsed.referenced_types() {
            if BUILT_IN_SCALARS.contains(&referenced.as_str()) || declared.contains_key(&referenced)
            {
                continue;
            }
            undefined.push(UndefinedTypeReference {
                type_name: name.clone(),
                referenced,
            });
        }
    }
    if !undefined.is_empty() {
        return Err(SchemaDocumentError::UndefinedTypesReferenced(undefined));
    }

    Ok(SchemaDocument {
        types: order
            .iter()
            .filter_map(|name| declared.remove(name))
            .collect(),
        definitions,
    })
}

/// What one caller-supplied schema document holds: the types it declares, in document order,
/// beside HOW MANY top-level definitions it states.
///
/// The two counts differ, and both are read off the one document: a `schema` definition and a
/// directive definition are definitions that declare no type, so a document that binds its
/// root operation types states one more definition than it declares types.
#[derive(Debug, Clone)]
pub struct SchemaDocument {
    /// The types the document declares, in the order it declares them.
    pub types: Vec<DeclaredType>,
    /// How many top-level definitions the document states, type definitions and the
    /// definitions that declare no type alike.
    pub definitions: usize,
}

impl TypeDefinition {
    /// Contribute everything the extension `extension` declares to this type.
    ///
    /// A type extension "can be used to represent a type which has been extended from
    /// some original type": the members it writes are ADDED to the ones the definition
    /// already declares, in the order the document writes them, and nothing of the
    /// definition is replaced — an extension carries no description and no kind of its
    /// own to replace them with.
    fn extend_with(&mut self, extension: Self) {
        self.interfaces.extend(extension.interfaces);
        self.fields.extend(extension.fields);
        self.input_fields.extend(extension.input_fields);
        self.enum_values.extend(extension.enum_values);
        self.members.extend(extension.members);
        self.directives.extend(extension.directives);
    }

    /// Every type name this definition references, in the order the definition REACHES
    /// them: the type each field resolves to and then the types that field's arguments
    /// take, field by field; the interfaces it implements; the types a union holds; and
    /// the type of each input field.
    ///
    /// The ORDER is part of the answer, because it is the order an introspection document
    /// reports a schema's types in (`super::introspection_schema`). A caller that only
    /// asks WHICH types are referenced — the check that every reference of a created
    /// document resolves — reads the same list.
    ///
    /// A list or non-null reference is a wrapper around a named one (`[Post!]!`), so
    /// what is reported is the NAME at its root — the only part that has to resolve to a
    /// declared type.
    pub(crate) fn referenced_types(&self) -> Vec<String> {
        let mut referenced: Vec<String> = Vec::new();
        for field in &self.fields {
            referenced.extend(field.value_type.as_ref().map(TypeRef::root_name));
            for argument in &field.arguments {
                referenced.extend(argument.value_type.as_ref().map(TypeRef::root_name));
            }
        }
        referenced.extend(self.interfaces.iter().cloned());
        referenced.extend(self.members.iter().cloned());
        for input_field in &self.input_fields {
            referenced.extend(input_field.value_type.as_ref().map(TypeRef::root_name));
        }
        referenced
    }
}

impl TypeRef {
    /// This reference flattened: the wrappers it nests, outermost first, and the NAME at
    /// its root.
    /// The chain is walked ITERATIVELY, so reading a reference costs one step per wrapper
    /// and no stack frame per wrapper however deeply the definition it was parsed from
    /// nested it — and every reader of a reference goes through here rather than
    /// recursing itself. How deep one can get is bounded on the way IN, by the parse
    /// ([`DEFINITION_RECURSION_LIMIT`]), which refuses a definition that nests past it
    /// instead of storing one.
    pub(crate) fn flattened(&self) -> (Vec<TypeWrapper>, &str) {
        let mut wrappers: Vec<TypeWrapper> = Vec::new();
        let mut reference = self;
        loop {
            match reference {
                Self::Named(name) => return (wrappers, name.as_str()),
                Self::List(inner) => {
                    wrappers.push(TypeWrapper::List);
                    reference = inner;
                }
                Self::NonNull(inner) => {
                    wrappers.push(TypeWrapper::NonNull);
                    reference = inner;
                }
            }
        }
    }

    /// The name at the root of this reference, unwrapping every list and non-null
    /// around it.
    fn root_name(&self) -> String {
        self.flattened().1.to_string()
    }
}
