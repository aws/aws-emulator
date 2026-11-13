// SPDX-License-Identifier: CC0-1.0

//! The introspection schema of a GraphQL API: the type system it holds, reported in
//! either representation the model names, and the refusal for an API that holds none.
//!
//! `GetIntrospectionSchema` "Retrieves the introspection schema for a GraphQL API", so
//! what it answers is the type system the API holds AT THE INSTANT of the call: the
//! types stored on the API, whatever installed them — a schema creation or a single
//! `CreateType` — together with what the created document declares that no type holds,
//! the root operation types a `schema` definition binds and the directives the schema
//! defines. Nothing of the request takes part in the answer beyond the representation
//! and the directive flag it asks for.
//!
//! The representation the `OutputType` the call names chooses is the same distinction
//! one type is reported under (`super::schema_types`): the SDL the schema's types are
//! written in, or the introspection JSON they render to. Both are computed from the
//! stored definitions, so the two cannot disagree about one schema, and neither can
//! disagree with what `GetType` answers for a type of it.
//!
//! An API whose types are empty holds no type system, so there is no schema to
//! introspect: what such a call reads as the API's schema is an EMPTY document, and the
//! refusal carries the parse of that document — which stops at its origin — whatever
//! left the API without a type system.

use std::collections::BTreeMap;

use apollo_parser::cst::{self, CstNode};
use apollo_parser::Parser;
use serde_json::{json, Value};

use super::schema_types::{input_value_definition, input_value_json};
use super::{
    invalid_syntax_detail, GraphqlApiRecord, InputValueDefinition, TypeRecord, BUILT_IN_SCALARS,
    DEFINITION_RECURSION_LIMIT, DEFINITION_TOKEN_LIMIT, DOCUMENT_ORIGIN, JSON_FORMAT, SCALAR_KIND,
};

/// The name the query root operation type carries in a schema that binds none.
///
/// GraphQL's own default: "the query root operation type must be provided and must be an
/// Object type", and a schema that writes no `schema` definition takes the type named
/// `Query` as it. The same rule gives [`DEFAULT_MUTATION_TYPE`] and
/// [`DEFAULT_SUBSCRIPTION_TYPE`] their names.
pub const DEFAULT_QUERY_TYPE: &str = "Query";

/// See [`DEFAULT_QUERY_TYPE`].
pub const DEFAULT_MUTATION_TYPE: &str = "Mutation";

/// See [`DEFAULT_QUERY_TYPE`].
pub const DEFAULT_SUBSCRIPTION_TYPE: &str = "Subscription";

/// What a schema document declares that its types do not: the root operation types its
/// `schema` definition binds, that definition's own text, and the directives it defines.
///
/// Read from the stored document rather than kept beside it, so the declarations reported
/// are the ones the document that was CREATED writes and cannot drift from it.
#[derive(Debug, Default)]
struct SchemaDeclarations {
    /// The type the document binds as its query root, or `None` where it binds none.
    query: Option<String>,
    /// The type the document binds as its mutation root.
    mutation: Option<String>,
    /// The type the document binds as its subscription root.
    subscription: Option<String>,
    /// The `schema` definition's own SDL text, kept verbatim, when the document writes
    /// one.
    schema_definition: Option<String>,
    /// The directives the document defines, in document order.
    directives: Vec<DirectiveDeclaration>,
}

/// One directive a schema document defines: the SDL text defining it and what that text
/// declares.
#[derive(Debug)]
struct DirectiveDeclaration {
    /// The document's own text for this definition, kept verbatim.
    definition: String,
    name: String,
    description: Option<String>,
    /// Whether the definition marks the directive `repeatable`.
    is_repeatable: bool,
    /// The locations the definition names a directive may be applied at, as GraphQL's
    /// `__DirectiveLocation` spells them.
    locations: Vec<String>,
    /// The arguments the definition declares, which are `__InputValue`s like a field's.
    arguments: Vec<InputValueDefinition>,
}

/// Read `document` for everything a schema declares beside its types.
///
/// The parse is bounded by the same two limits one definition is read under, so neither
/// the depth nor the amount of work reading one document costs comes from the request.
fn schema_declarations(document: &str) -> SchemaDeclarations {
    let tree = Parser::new(document)
        .recursion_limit(DEFINITION_RECURSION_LIMIT)
        .token_limit(DEFINITION_TOKEN_LIMIT)
        .parse();
    let mut declarations = SchemaDeclarations::default();
    for node in tree.document().definitions() {
        match node {
            cst::Definition::SchemaDefinition(schema) => {
                declarations.schema_definition = Some(node_text(schema.syntax()));
                bind_root_types(&mut declarations, schema.root_operation_type_definitions());
            }
            // An extension of the schema binds root operation types the same way the
            // definition does, so a document that extends its own schema is read for
            // them too.
            cst::Definition::SchemaExtension(extension) => {
                bind_root_types(&mut declarations, extension.root_operation_type_definitions());
            }
            cst::Definition::DirectiveDefinition(directive) => {
                let Some(name) = directive.name().map(|name| name.text().to_string()) else {
                    continue;
                };
                declarations.directives.push(DirectiveDeclaration {
                    definition: node_text(directive.syntax()),
                    name,
                    description: directive
                        .description()
                        .and_then(|description| description.string_value())
                        .map(String::from),
                    is_repeatable: directive.repeatable_token().is_some(),
                    locations: directive
                        .directive_locations()
                        .map(|locations| {
                            locations
                                .directive_locations()
                                .map(|location| node_text(location.syntax()))
                                .collect()
                        })
                        .unwrap_or_default(),
                    arguments: directive
                        .arguments_definition()
                        .map(|arguments| {
                            arguments
                                .input_value_definitions()
                                .map(input_value_definition)
                                .collect()
                        })
                        .unwrap_or_default(),
                });
            }
            _ => {}
        }
    }
    declarations
}

/// Bind every root operation type `roots` names onto `declarations`.
fn bind_root_types(
    declarations: &mut SchemaDeclarations,
    roots: impl Iterator<Item = cst::RootOperationTypeDefinition>,
) {
    for root in roots {
        let Some(named) = root
            .named_type()
            .and_then(|named| named.name())
            .map(|name| name.text().to_string())
        else {
            continue;
        };
        let Some(operation) = root.operation_type() else {
            continue;
        };
        if operation.query_token().is_some() {
            declarations.query = Some(named);
        } else if operation.mutation_token().is_some() {
            declarations.mutation = Some(named);
        } else if operation.subscription_token().is_some() {
            declarations.subscription = Some(named);
        }
    }
}

/// The source text of one parsed node, as the document writes it.
fn node_text(node: &apollo_parser::SyntaxNode) -> String {
    node.text().to_string().trim().to_string()
}

impl GraphqlApiRecord {
    /// The type this API's schema binds as the root of its QUERY operations.
    ///
    /// Which type holds them is the schema's own binding: the type its `schema` definition
    /// binds, and the type named [`DEFAULT_QUERY_TYPE`] for a schema that binds none, which
    /// is GraphQL's own default. The same rule gives [`Self::mutation_root_type`] and
    /// [`Self::subscription_root_type`] theirs, so every reader of a root operation type
    /// reads it one way.
    pub fn query_root_type(&self) -> String {
        self.bound_root_type(
            |declarations| declarations.query.clone(),
            DEFAULT_QUERY_TYPE,
        )
    }

    /// The type this API's schema binds as the root of its MUTATION operations; see
    /// [`Self::query_root_type`].
    pub fn mutation_root_type(&self) -> String {
        self.bound_root_type(
            |declarations| declarations.mutation.clone(),
            DEFAULT_MUTATION_TYPE,
        )
    }

    /// The type this API's schema binds as the root of its SUBSCRIPTION operations; see
    /// [`Self::query_root_type`].
    pub fn subscription_root_type(&self) -> String {
        self.bound_root_type(
            |declarations| declarations.subscription.clone(),
            DEFAULT_SUBSCRIPTION_TYPE,
        )
    }

    /// The type the stored schema document binds for one root operation, or GraphQL's
    /// default name for it where the document binds none.
    fn bound_root_type(
        &self,
        bound: impl Fn(&SchemaDeclarations) -> Option<String>,
        default_name: &str,
    ) -> String {
        self.schema_document
            .as_deref()
            .and_then(|document| bound(&schema_declarations(document)))
            .unwrap_or_else(|| default_name.to_string())
    }

    /// The SUBSCRIPTION fields this API's schema declares, in the order the schema
    /// declares them — the fields a real-time client may start a subscription on.
    ///
    /// Which type holds them is the schema's own binding: the type its `schema` definition
    /// binds as the subscription root, and the type named [`DEFAULT_SUBSCRIPTION_TYPE`] for
    /// a schema that binds none, which is GraphQL's own default. An API whose schema holds
    /// no such type declares no subscription field at all, which is what a client
    /// subscribing against it finds.
    ///
    /// Read from the stored document and the stored types, so the fields reported are the
    /// ones the schema that was CREATED declares and cannot drift from it.
    pub fn subscription_fields(&self) -> Vec<String> {
        let named = self.subscription_root_type();
        self.types
            .get(&named)
            .map(|record| {
                record
                    .parsed
                    .fields
                    .iter()
                    .map(|field| field.name.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// This API's introspection schema in `format`, or the text a
    /// `GraphQLSchemaException` carries for an API that holds no schema to introspect.
    ///
    /// What the call reads as the schema of an API that holds no type system is an EMPTY
    /// document, so the refusal is the parse of that document — the same text whatever
    /// left the API without one, because the API holds the same nothing either way.
    ///
    /// `include_directives` is the call's own flag for "whether the schema introspection
    /// should contain directives": the directives the schema defines are reported only
    /// when it asks for them.
    pub fn introspection_schema(
        &self,
        format: &str,
        include_directives: bool,
    ) -> Result<Vec<u8>, String> {
        // A type system with no type in it defines no schema — whatever left it empty: an
        // API no creation has run for, one whose creation FAILED, and one whose creation
        // is still in progress all hold no schema yet.
        if self.types.is_empty() {
            return Err(invalid_syntax_detail(DOCUMENT_ORIGIN.0, DOCUMENT_ORIGIN.1));
        }
        let declarations = schema_declarations(self.schema_document.as_deref().unwrap_or(""));
        let schema = if format == JSON_FORMAT {
            self.introspection_json(&declarations, include_directives)
        } else {
            self.introspection_sdl(&declarations, include_directives)
        };
        Ok(schema.into_bytes())
    }

    /// This API's schema as GraphQL SDL: the `schema` definition the created document
    /// binds its root operation types with (where it writes one — GraphQL lets a schema
    /// whose roots carry the default names omit it), the directives it defines, and every
    /// type the API holds, each in the SDL it was defined with.
    ///
    /// The types are written in the order they were created, which is the order the
    /// document declares them in and the order a listing reports them in, so one API's
    /// schema reads the same on every call.
    fn introspection_sdl(
        &self,
        declarations: &SchemaDeclarations,
        include_directives: bool,
    ) -> String {
        let mut parts: Vec<String> = Vec::new();
        parts.extend(declarations.schema_definition.clone());
        if include_directives {
            parts.extend(
                declarations
                    .directives
                    .iter()
                    .map(|directive| directive.definition.clone()),
            );
        }
        parts.extend(
            self.types_in_creation_order()
                .map(|record| record.definition.trim().to_string()),
        );
        let mut sdl = parts.join("\n\n");
        sdl.push('\n');
        sdl
    }

    /// This API's schema as GraphQL introspection JSON: the result of the introspection
    /// query over it, so the `__Schema` the introspection system defines sits under the
    /// `data` every GraphQL result carries its data under — its root operation types,
    /// every type it holds and, when the call asks for them, the directives it defines.
    ///
    /// Each type is rendered as GraphQL's introspection system defines a `__Type` —
    /// which is not the document a read of ONE type answers with (`super::type_json`) —
    /// and the built-in scalars the schema references are reported beside them, because
    /// an introspection document holds every type its schema's fields resolve to.
    fn introspection_json(
        &self,
        declarations: &SchemaDeclarations,
        include_directives: bool,
    ) -> String {
        let mut types: Vec<Value> = self
            .types_in_creation_order()
            .map(|record| record.introspection(&self.types))
            .collect();
        types.extend(
            self.referenced_built_in_scalars()
                .into_iter()
                .map(|name| built_in_scalar_json(&name)),
        );
        let mut schema = json!({
            "queryType": self.root_type_json(declarations.query.as_deref(), DEFAULT_QUERY_TYPE),
            "mutationType": self
                .root_type_json(declarations.mutation.as_deref(), DEFAULT_MUTATION_TYPE),
            "subscriptionType": self
                .root_type_json(declarations.subscription.as_deref(), DEFAULT_SUBSCRIPTION_TYPE),
            "types": types,
        });
        if include_directives {
            let directives: Vec<Value> = declarations
                .directives
                .iter()
                .map(|directive| self.directive_json(directive))
                .collect();
            if let Some(members) = schema.as_object_mut() {
                members.insert("directives".to_string(), Value::Array(directives));
            }
        }
        serde_json::to_string_pretty(&json!({"data": {"__schema": schema}})).unwrap_or_default()
    }

    /// The types this API holds, in the order they were created.
    fn types_in_creation_order(&self) -> impl Iterator<Item = &TypeRecord> {
        let mut records: Vec<&TypeRecord> = self.types.values().collect();
        records.sort_by_key(|record| record.seq);
        records.into_iter()
    }

    /// One root operation type as introspection reports it: the type the schema binds for
    /// it, or the type of GraphQL's default name for it where the schema binds none —
    /// and null where the API holds no type of that name, because a schema with no
    /// mutations has no mutation root.
    fn root_type_json(&self, bound: Option<&str>, default_name: &str) -> Value {
        let name = bound.unwrap_or(default_name);
        match self.types.contains_key(name) {
            true => json!({"name": name}),
            false => Value::Null,
        }
    }

    /// One directive the schema defines as introspection's `__Directive`.
    fn directive_json(&self, directive: &DirectiveDeclaration) -> Value {
        json!({
            "name": directive.name,
            "description": directive.description,
            "isRepeatable": directive.is_repeatable,
            "locations": directive.locations,
            "args": directive
                .arguments
                .iter()
                .map(|argument| input_value_json(argument, &self.types))
                .collect::<Vec<Value>>(),
        })
    }

    /// The scalars every schema holds without defining them that this API's types
    /// reference, in name order and each reported once.
    ///
    /// A field whose type is one of them resolves to a type of the introspection
    /// document, so the document names them beside the types the API defines.
    fn referenced_built_in_scalars(&self) -> Vec<String> {
        let mut referenced: BTreeMap<String, ()> = BTreeMap::new();
        for record in self.types.values() {
            for name in record.parsed.referenced_types() {
                if BUILT_IN_SCALARS.contains(&name.as_str()) {
                    referenced.insert(name, ());
                }
            }
        }
        referenced.into_keys().collect()
    }
}

/// One built-in scalar as introspection's `__Type`: a scalar carries no member a type
/// system defines, so every member but its kind and name is null — the same rendering a
/// DEFINED scalar of the API gets.
fn built_in_scalar_json(name: &str) -> Value {
    json!({
        "kind": SCALAR_KIND,
        "name": name,
        "description": Value::Null,
        "fields": Value::Null,
        "inputFields": Value::Null,
        "interfaces": Value::Null,
        "enumValues": Value::Null,
        "possibleTypes": Value::Null,
    })
}
