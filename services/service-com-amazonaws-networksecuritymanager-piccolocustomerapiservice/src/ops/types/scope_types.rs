// SPDX-License-Identifier: CC0-1.0

//! The shapes the Scope entity operations (`CreateScope`, `GetScope`) reach.
//!
//! HAND-AUTHORED, not generated. The scaffold's op-stub generator declined every scope
//! operation ("shape missing from model: smithy.api#Unit", recorded in
//! `src/ops/SKIPPED_STUBS.md`): the `ScopeConfiguration` closure reaches `AccountFilter`, a
//! union one of whose members targets `smithy.api#Unit`, a prelude shape the model JSON
//! does not carry and the vendored codegen cannot type. Everything that union does not
//! touch is declared with the ordinary derive here; the union itself, and the valueless
//! member it carries, have the one hand-written schema and codec pair below.
//!
//! Member sets, member order, requiredness and targets come from
//! `smithy_query(query="closure", operation="CreateScope", model_path="services/\
//! service-com-amazonaws-networksecuritymanager-piccolocustomerapiservice/smithy-model/\
//! com-amazonaws-networksecuritymanager-piccolocustomerapiservice.json")`, and the derive
//! re-checks each one against that model at compile time.
#![allow(unused_imports)]

use super::*;

smithy_macros::smithy_model!("smithy-model/com-amazonaws-networksecuritymanager-piccolocustomerapiservice.json");

// ---------------------------------------------------------------------------
// The operations' input and output shapes
// ---------------------------------------------------------------------------

#[smithy_macros::smithy(shape = "com.amazonaws.networksecuritymanager#CreateScopeInput")]
#[derive(Debug, Clone, Default)]
pub struct CreateScopeInput {
    pub client_token: Option<String>,
    pub scope_name: String,
    pub scope_description: Option<String>,
    pub scope_configuration: ScopeConfiguration,
    /// `SpecializedScopeType` — "Omit this value to create a standard scope that you can
    /// associate with a deployment." Carried as the enum's string value on the wire.
    pub specialized_scope_type: Option<String>,
    pub is_published: bool,
    pub tags: Option<std::collections::HashMap<String, String>>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.networksecuritymanager#CreateScopeOutput")]
#[derive(Debug, Clone, Default)]
pub struct CreateScopeOutput {
    pub scope_id: String,
    pub scope_arn: String,
    pub scope_name: String,
    pub scope_description: Option<String>,
    pub scope_configuration: Option<ScopeConfiguration>,
    /// `SpecializedScopeType` — "This value is set when the scope is created and can't be
    /// changed."
    pub specialized_scope_type: Option<String>,
    pub status: String,
    pub version: String,
    pub update_token: Option<String>,
    pub is_snapshot: Option<bool>,
    pub has_published_version: Option<bool>,
    pub updated_at: Option<::aws_smithy_types::DateTime>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.networksecuritymanager#GetScopeInput")]
#[derive(Debug, Clone, Default)]
pub struct GetScopeInput {
    pub scope_identifier: String,
}

#[smithy_macros::smithy(shape = "com.amazonaws.networksecuritymanager#GetScopeOutput")]
#[derive(Debug, Clone, Default)]
pub struct GetScopeOutput {
    pub scope_id: String,
    pub scope_arn: String,
    pub scope_name: String,
    pub scope_description: Option<String>,
    pub scope_configuration: Option<ScopeConfiguration>,
    /// `SpecializedScopeType` — see [`CreateScopeOutput::specialized_scope_type`].
    pub specialized_scope_type: Option<String>,
    pub status: String,
    pub version: String,
    pub update_token: Option<String>,
    pub is_snapshot: Option<bool>,
    pub has_published_version: Option<bool>,
    pub updated_at: Option<::aws_smithy_types::DateTime>,
}

// ---------------------------------------------------------------------------
// The scope configuration: which accounts and which resources are in scope
// ---------------------------------------------------------------------------

#[smithy_macros::smithy(shape = "com.amazonaws.networksecuritymanager#ScopeConfiguration")]
#[derive(Debug, Clone, Default)]
pub struct ScopeConfiguration {
    pub account_filter: Option<AccountFilter>,
    /// `ResourceScopeMap` — keyed by `ScopeResourceType`, which the wire carries as the
    /// enum's string value.
    pub resource_scopes: std::collections::HashMap<String, ResourceScope>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.networksecuritymanager#AccountSet")]
#[derive(Debug, Clone, Default)]
pub struct AccountSet {
    pub account_ids: Option<Vec<String>>,
    pub organizational_units: Option<Vec<String>>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.networksecuritymanager#ResourceScope")]
#[derive(Debug, Clone, Default)]
pub struct ResourceScope {
    pub include_all: Option<bool>,
    pub include: Option<ResourceSet>,
    pub exclude: Option<ResourceSet>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.networksecuritymanager#ResourceSet")]
#[derive(Debug, Clone, Default)]
pub struct ResourceSet {
    pub explicit_arns: Option<Vec<String>>,
    pub expression: Option<ResourceLogicalExpression>,
}

/// How deeply one `ResourceLogicalExpression` may nest inside another before a request
/// carrying it is refused.
///
/// The expression is SELF-REFERENTIAL — `and`/`or` hold a list of expressions and `not`
/// holds one — so the nesting of a request body drives the recursion of reading it, and a
/// body nested far enough would exhaust the serving thread's stack, which no handler can
/// catch. [`ResourceLogicalExpressionDepth`] bounds it where the recursion happens.
///
/// The model bounds each `and`/`or` level at 20 entries (`ResourceLogicalExpressionList` is
/// `@length(max: 20)`) but states no nesting ceiling, so this number is a CHOSEN emulator
/// bound, as generous as that per-level one: what matters is that a bound exists and that
/// passing it is a clean refusal rather than a dead process.
const MAX_RESOURCE_EXPRESSION_DEPTH: usize = 20;

thread_local! {
    /// How many `ResourceLogicalExpression`s the walk on THIS thread currently sits
    /// inside. Thread-local because a request is read on one thread and the codec trait
    /// carries no place to thread a budget through.
    static RESOURCE_EXPRESSION_DEPTH: ::std::cell::Cell<usize> =
        const { ::std::cell::Cell::new(0) };
}

/// One level of [`ResourceLogicalExpression`] nesting, released when it drops — so an early
/// return, including the error paths below, cannot leave the count too high.
struct ResourceLogicalExpressionDepth;

impl ResourceLogicalExpressionDepth {
    /// Enter one level, refusing to descend past [`MAX_RESOURCE_EXPRESSION_DEPTH`].
    fn enter() -> ::std::result::Result<Self, ::aws_smithy_schema::serde::SerdeError> {
        RESOURCE_EXPRESSION_DEPTH.with(|depth| {
            let next = depth.get().saturating_add(1);
            if next > MAX_RESOURCE_EXPRESSION_DEPTH {
                return ::std::result::Result::Err(
                    ::aws_smithy_schema::serde::SerdeError::custom(format!(
                        "expression nests deeper than the limit of \
                         {MAX_RESOURCE_EXPRESSION_DEPTH}"
                    )),
                );
            }
            depth.set(next);
            ::std::result::Result::Ok(Self)
        })
    }
}

impl Drop for ResourceLogicalExpressionDepth {
    fn drop(&mut self) {
        RESOURCE_EXPRESSION_DEPTH.with(|depth| depth.set(depth.get().saturating_sub(1)));
    }
}

/// `ResourceLogicalExpression` — the boolean expression a resource set selects by.
///
/// HAND-WRITTEN schema and codec, like [`AccountFilter`]'s but for a different reason: this
/// union is recursive, and reading it has to carry the depth budget above, which the derive
/// has nowhere to put. What follows is otherwise exactly what
/// `#[smithy_macros::smithy(union_for = ..)]` emits for a four-member union — one
/// `M_<MEMBER>` member schema per member in the model's member order, a `SCHEMA` of type
/// `Union` naming them in that order, the `<SHAPE>_SCHEMA` static and the
/// `SerializableStruct` / `DeserializableStruct` pair, arity rule included.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
pub enum ResourceLogicalExpression {
    Criteria(ResourceCriteria),
    And(Vec<ResourceLogicalExpression>),
    Or(Vec<ResourceLogicalExpression>),
    Not(Box<ResourceLogicalExpression>),
}

#[allow(non_upper_case_globals)]
impl ResourceLogicalExpression {
    pub const M_CRITERIA: ::aws_smithy_schema::Schema = ::aws_smithy_schema::Schema::new_member(
        ::aws_smithy_schema::ShapeId::from_static(
            "com.amazonaws.networksecuritymanager#ResourceLogicalExpression$criteria",
            "com.amazonaws.networksecuritymanager",
            "criteria",
        ),
        ::aws_smithy_schema::ShapeType::Structure,
        "criteria",
        0,
    );

    pub const M_AND: ::aws_smithy_schema::Schema = ::aws_smithy_schema::Schema::new_member(
        ::aws_smithy_schema::ShapeId::from_static(
            "com.amazonaws.networksecuritymanager#ResourceLogicalExpression$and",
            "com.amazonaws.networksecuritymanager",
            "and",
        ),
        ::aws_smithy_schema::ShapeType::List,
        "and",
        1,
    );

    pub const M_OR: ::aws_smithy_schema::Schema = ::aws_smithy_schema::Schema::new_member(
        ::aws_smithy_schema::ShapeId::from_static(
            "com.amazonaws.networksecuritymanager#ResourceLogicalExpression$or",
            "com.amazonaws.networksecuritymanager",
            "or",
        ),
        ::aws_smithy_schema::ShapeType::List,
        "or",
        2,
    );

    pub const M_NOT: ::aws_smithy_schema::Schema = ::aws_smithy_schema::Schema::new_member(
        ::aws_smithy_schema::ShapeId::from_static(
            "com.amazonaws.networksecuritymanager#ResourceLogicalExpression$not",
            "com.amazonaws.networksecuritymanager",
            "not",
        ),
        ::aws_smithy_schema::ShapeType::Union,
        "not",
        3,
    );

    pub const SCHEMA: ::aws_smithy_schema::Schema = ::aws_smithy_schema::Schema::new_struct(
        ::aws_smithy_schema::ShapeId::from_static(
            "com.amazonaws.networksecuritymanager#ResourceLogicalExpression",
            "com.amazonaws.networksecuritymanager",
            "ResourceLogicalExpression",
        ),
        ::aws_smithy_schema::ShapeType::Union,
        &[&Self::M_CRITERIA, &Self::M_AND, &Self::M_OR, &Self::M_NOT],
    );
}

pub static RESOURCE_LOGICAL_EXPRESSION_SCHEMA: &::aws_smithy_schema::Schema =
    &ResourceLogicalExpression::SCHEMA;

impl ::aws_smithy_schema::serde::SerializableStruct for ResourceLogicalExpression {
    fn serialize_members(
        &self,
        s: &mut dyn ::aws_smithy_schema::serde::ShapeSerializer,
    ) -> ::std::result::Result<(), ::aws_smithy_schema::serde::SerdeError> {
        match self {
            Self::Criteria(value) => s.write_struct(&Self::M_CRITERIA, value)?,
            Self::And(values) => s.write_list(&Self::M_AND, &|sl| {
                for value in values.iter() {
                    sl.write_struct(&Self::SCHEMA, value)?;
                }
                Ok(())
            })?,
            Self::Or(values) => s.write_list(&Self::M_OR, &|sl| {
                for value in values.iter() {
                    sl.write_struct(&Self::SCHEMA, value)?;
                }
                Ok(())
            })?,
            Self::Not(value) => s.write_struct(&Self::M_NOT, value.as_ref())?,
        }
        Ok(())
    }
}

impl ::aws_smithy_schema_server::DeserializableStruct for ResourceLogicalExpression {
    fn deserialize(
        d: &mut dyn ::aws_smithy_schema::serde::ShapeDeserializer,
    ) -> ::std::result::Result<Self, ::aws_smithy_schema::serde::SerdeError> {
        // One level of the budget, taken BEFORE anything is read and released when this
        // frame leaves, so the nesting of a request body cannot drive the recursion below
        // past a fixed number of frames.
        let _depth = ResourceLogicalExpressionDepth::enter()?;
        let mut out: Option<Self> = None;
        d.read_struct(&Self::SCHEMA, &mut |member, d| {
            // A union carries EXACTLY one member, so a second member on the wire is
            // ambiguous and is refused here rather than silently overwritten — the same
            // arity rule the derive enforces for every other union.
            let read = match member.member_index() {
                Some(0) => Some(Self::Criteria(
                    <ResourceCriteria as ::aws_smithy_schema_server::DeserializableStruct>::deserialize(d)?,
                )),
                Some(1) => Some(Self::And(read_expression_list(&Self::M_AND, d)?)),
                Some(2) => Some(Self::Or(read_expression_list(&Self::M_OR, d)?)),
                Some(3) => Some(Self::Not(Box::new(
                    <Self as ::aws_smithy_schema_server::DeserializableStruct>::deserialize(d)?,
                ))),
                _ => None,
            };
            if let Some(read) = read {
                if out.is_some() {
                    return ::std::result::Result::Err(
                        ::aws_smithy_schema::serde::SerdeError::custom(
                            "union 'ResourceLogicalExpression' had more than one member set"
                                .to_string(),
                        ),
                    );
                }
                out = Some(read);
            }
            Ok(())
        })?;
        out.ok_or_else(|| {
            ::aws_smithy_schema::serde::SerdeError::custom(
                "union 'ResourceLogicalExpression' had no member set".to_string(),
            )
        })
    }
}

/// The `ResourceLogicalExpressionList` one of the union's branches holds, read element by
/// element — each element taking its own level of the depth budget above.
fn read_expression_list(
    member: &::aws_smithy_schema::Schema,
    d: &mut dyn ::aws_smithy_schema::serde::ShapeDeserializer,
) -> ::std::result::Result<Vec<ResourceLogicalExpression>, ::aws_smithy_schema::serde::SerdeError> {
    let mut out: Vec<ResourceLogicalExpression> = Vec::new();
    d.read_list(member, &mut |element| {
        out.push(
            <ResourceLogicalExpression as ::aws_smithy_schema_server::DeserializableStruct>::deserialize(
                element,
            )?,
        );
        Ok(())
    })?;
    Ok(out)
}

#[smithy_macros::smithy(union_for = "com.amazonaws.networksecuritymanager#ResourceCriteria")]
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
pub enum ResourceCriteria {
    Tags(std::collections::HashMap<String, String>),
    albConfig(AlbConfiguration),
}

#[smithy_macros::smithy(shape = "com.amazonaws.networksecuritymanager#AlbConfiguration")]
#[derive(Debug, Clone, Default)]
pub struct AlbConfiguration {
    pub scheme: Option<String>,
    pub ip_address_type: Option<String>,
}

// ---------------------------------------------------------------------------
// `AccountFilter`, the union the derive cannot type
// ---------------------------------------------------------------------------

/// `smithy.api#Unit` — the target of a union member that carries no value, written on the
/// wire as the empty object `{}`.
///
/// Schema and codec are hand-written for the same reason [`AccountFilter`]'s are: the model
/// JSON carries no `smithy.api#Unit` shape, so `#[smithy_macros::smithy(shape = ..)]`
/// refuses the id before it ever reaches its empty-struct path. This is what that path
/// emits for a shape with no members.
#[derive(Debug, Clone, Default)]
pub struct ScopeUnitValue {}

impl ScopeUnitValue {
    pub const SCHEMA: ::aws_smithy_schema::Schema = ::aws_smithy_schema::Schema::new_struct(
        ::aws_smithy_schema::ShapeId::from_static("smithy.api#Unit", "smithy.api", "Unit"),
        ::aws_smithy_schema::ShapeType::Structure,
        &[],
    );
}

impl ::aws_smithy_schema::serde::SerializableStruct for ScopeUnitValue {
    fn serialize_members(
        &self,
        _s: &mut dyn ::aws_smithy_schema::serde::ShapeSerializer,
    ) -> ::std::result::Result<(), ::aws_smithy_schema::serde::SerdeError> {
        Ok(())
    }
}

impl ::aws_smithy_schema_server::DeserializableStruct for ScopeUnitValue {
    fn deserialize(
        d: &mut dyn ::aws_smithy_schema::serde::ShapeDeserializer,
    ) -> ::std::result::Result<Self, ::aws_smithy_schema::serde::SerdeError> {
        d.read_struct(&Self::SCHEMA, &mut |_, _| Ok(()))?;
        Ok(Self {})
    }
}

/// `AccountFilter` — "Determines which accounts are in scope. Exactly one of
/// `includeAll`, `include`, or `exclude` is set."
///
/// HAND-WRITTEN schema and codec, the one place in this module the derive cannot be used:
/// `includeAll` targets `smithy.api#Unit`, which the vendored codegen cannot classify (it
/// is a prelude shape, absent from the model's `shapes` map), so
/// `#[smithy_macros::smithy(union_for = ..)]` fails on this union alone. What follows is
/// exactly what that derive emits for a three-member union — one `M_<MEMBER>` member schema
/// per member in the model's member order, a `SCHEMA` of type `Union` naming them in that
/// order, the `<SHAPE>_SCHEMA` static and the `SerializableStruct` /
/// `DeserializableStruct` pair — with the valueless member written as an empty structure.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
pub enum AccountFilter {
    /// "Includes all accounts. No account filtering is applied."
    includeAll(ScopeUnitValue),
    /// "Includes only the specified accounts and organizational units."
    Include(AccountSet),
    /// "Excludes the specified accounts and organizational units. All others are in
    /// scope."
    Exclude(AccountSet),
}

#[allow(non_upper_case_globals)]
impl AccountFilter {
    pub const M_INCLUDE_ALL: ::aws_smithy_schema::Schema =
        ::aws_smithy_schema::Schema::new_member(
            ::aws_smithy_schema::ShapeId::from_static(
                "com.amazonaws.networksecuritymanager#AccountFilter$includeAll",
                "com.amazonaws.networksecuritymanager",
                "includeAll",
            ),
            ::aws_smithy_schema::ShapeType::Structure,
            "includeAll",
            0,
        );

    pub const M_INCLUDE: ::aws_smithy_schema::Schema = ::aws_smithy_schema::Schema::new_member(
        ::aws_smithy_schema::ShapeId::from_static(
            "com.amazonaws.networksecuritymanager#AccountFilter$include",
            "com.amazonaws.networksecuritymanager",
            "include",
        ),
        ::aws_smithy_schema::ShapeType::Structure,
        "include",
        1,
    );

    pub const M_EXCLUDE: ::aws_smithy_schema::Schema = ::aws_smithy_schema::Schema::new_member(
        ::aws_smithy_schema::ShapeId::from_static(
            "com.amazonaws.networksecuritymanager#AccountFilter$exclude",
            "com.amazonaws.networksecuritymanager",
            "exclude",
        ),
        ::aws_smithy_schema::ShapeType::Structure,
        "exclude",
        2,
    );

    pub const SCHEMA: ::aws_smithy_schema::Schema = ::aws_smithy_schema::Schema::new_struct(
        ::aws_smithy_schema::ShapeId::from_static(
            "com.amazonaws.networksecuritymanager#AccountFilter",
            "com.amazonaws.networksecuritymanager",
            "AccountFilter",
        ),
        ::aws_smithy_schema::ShapeType::Union,
        &[&Self::M_INCLUDE_ALL, &Self::M_INCLUDE, &Self::M_EXCLUDE],
    );
}

pub static ACCOUNT_FILTER_SCHEMA: &::aws_smithy_schema::Schema = &AccountFilter::SCHEMA;

impl ::aws_smithy_schema::serde::SerializableStruct for AccountFilter {
    fn serialize_members(
        &self,
        s: &mut dyn ::aws_smithy_schema::serde::ShapeSerializer,
    ) -> ::std::result::Result<(), ::aws_smithy_schema::serde::SerdeError> {
        match self {
            Self::includeAll(value) => s.write_struct(&Self::M_INCLUDE_ALL, value)?,
            Self::Include(value) => s.write_struct(&Self::M_INCLUDE, value)?,
            Self::Exclude(value) => s.write_struct(&Self::M_EXCLUDE, value)?,
        }
        Ok(())
    }
}

impl ::aws_smithy_schema_server::DeserializableStruct for AccountFilter {
    fn deserialize(
        d: &mut dyn ::aws_smithy_schema::serde::ShapeDeserializer,
    ) -> ::std::result::Result<Self, ::aws_smithy_schema::serde::SerdeError> {
        let mut out: Option<Self> = None;
        d.read_struct(&Self::SCHEMA, &mut |member, d| {
            // A union carries EXACTLY one member, so a second member on the wire is
            // ambiguous and is refused here rather than silently overwritten — the same
            // arity rule the derive enforces for every other union.
            let read = match member.member_index() {
                Some(0) => Some(Self::includeAll(
                    <ScopeUnitValue as ::aws_smithy_schema_server::DeserializableStruct>::deserialize(d)?,
                )),
                Some(1) => Some(Self::Include(
                    <AccountSet as ::aws_smithy_schema_server::DeserializableStruct>::deserialize(d)?,
                )),
                Some(2) => Some(Self::Exclude(
                    <AccountSet as ::aws_smithy_schema_server::DeserializableStruct>::deserialize(d)?,
                )),
                _ => None,
            };
            if let Some(read) = read {
                if out.is_some() {
                    return ::std::result::Result::Err(
                        ::aws_smithy_schema::serde::SerdeError::custom(
                            "union 'AccountFilter' had more than one member set".to_string(),
                        ),
                    );
                }
                out = Some(read);
            }
            Ok(())
        })?;
        out.ok_or_else(|| {
            ::aws_smithy_schema::serde::SerdeError::custom(
                "union 'AccountFilter' had no member set".to_string(),
            )
        })
    }
}
