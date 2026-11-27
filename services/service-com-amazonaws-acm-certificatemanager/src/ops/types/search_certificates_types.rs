// SPDX-License-Identifier: CC0-1.0

//! Type definitions reached only by the `search_certificates` operation.
//!
//! HAND-WRITTEN at a generated path, and therefore NOT re-rendered by a model update:
//! the four filter unions below carry readers this service wrote itself (see
//! [`CertificateFilterStatement`]), which a re-render would delete. Everything else in
//! this file is what the scaffold renders for this operation, member for member, and
//! has to be carried forward BY HAND when the model changes — a new or renamed member of
//! any shape declared here, and the union member lists and indices below with it.
//!
//! Each structure/union is declared exactly once across this service's `types/`
//! modules, and `types/mod.rs` re-exports them all.
#![allow(unused_imports)]

use super::*;

smithy_macros::smithy_model!("smithy-model/com-amazonaws-acm-certificatemanager.json");

/// The namespace every shape in this file belongs to, as the hand-written member
/// schemas below name it.
const NAMESPACE: &str = "com.amazonaws.acm";

#[smithy_macros::smithy(shape = "com.amazonaws.acm#SearchCertificatesRequest")]
#[derive(Debug, Clone, Default)]
pub struct SearchCertificatesInput {
    pub filter_statement: Option<CertificateFilterStatement>,
    pub max_results: i32,
    pub next_token: Option<String>,
    pub sort_by: String,
    pub sort_order: String,
}

/// One member schema of a filter union, named and indexed as the model orders it.
///
/// Mirrors what `#[smithy_macros::smithy(union_for = ...)]` emits for a union member,
/// argument for argument: the member's shape id, the `ShapeType` of its target, its
/// wire name and its index. Constraint traits are absent here exactly as they are
/// absent from the macro's union members, so no rule this service enforces is dropped —
/// the enum value sets sitting directly on these members are refused by the operation
/// itself (`crate::ops::search_certificates`), which is where they already were.
macro_rules! filter_union_member {
    ($union:literal, $name:literal, $ty:ident, $index:literal) => {
        ::aws_smithy_schema::Schema::new_member(
            ::aws_smithy_schema::ShapeId::from_static(
                concat!("com.amazonaws.acm#", $union, "$", $name),
                NAMESPACE,
                $name,
            ),
            ::aws_smithy_schema::ShapeType::$ty,
            $name,
            $index,
        )
    };
}

/// Why a union of this operation's input that carries other than exactly one member is
/// refused: the union's name, the exactly-one-member constraint, and the members the
/// REQUEST set, in the order it set them.
///
/// The members are collected as the reader meets them, so the sentence is built from the
/// request rather than from the shape: a caller debugging a generated filter is told
/// WHICH members collided. A union a request set no member of carries the empty list —
/// the same constraint, reported the same way.
pub fn exactly_one_member_message(union_name: &str, members: &[&str]) -> String {
    format!(
        "Each {union_name} must contain exactly one member. A {union_name} in your \
         request contains the members [{}].",
        members.join(", ")
    )
}

/// The one member of `union_name` a request set, or the exactly-one-member refusal.
fn one_member<T>(
    union_name: &str,
    members: &[&str],
    set: Option<T>,
) -> ::std::result::Result<T, ::aws_smithy_schema::serde::SerdeError> {
    match set {
        Some(value) if members.len() == 1 => ::std::result::Result::Ok(value),
        _ => ::std::result::Result::Err(::aws_smithy_schema::serde::SerdeError::custom(
            exactly_one_member_message(union_name, members),
        )),
    }
}

/// The budget one request's filter statement is READ under: how deep the reader may
/// descend, and how many statement nodes it may build.
///
/// Both bounds are the service's own
/// (`crate::ops::search_certificates::MAX_STATEMENT_DEPTH` and `MAX_STATEMENT_NODES`),
/// and both are enforced HERE because the reader is the first walk a request's
/// statement reaches: it recurses on the native stack and allocates one node per
/// statement as it goes, so a tree deeper or larger than the service admits is refused
/// before it becomes a value at all rather than after the whole body has been turned
/// into one. The handler's own walk re-checks the same two bounds, because a statement
/// can also be built in-process.
///
/// One request is read on one thread, so the counters are thread-local, and the claim
/// each statement takes releases itself on drop — so the `?` path and an unwind both
/// leave the counters where they were found. The node budget is refilled by the
/// OUTERMOST statement of a read, which is what makes it a per-request budget rather
/// than a per-process one.
mod statement_budget {
    use crate::ops::search_certificates::{
        statement_depth_message, statement_nodes_message, MAX_STATEMENT_DEPTH, MAX_STATEMENT_NODES,
    };

    thread_local! {
        /// How many statements the reader is currently inside.
        static DEPTH: ::std::cell::Cell<usize> = const { ::std::cell::Cell::new(0) };
        /// How many statement nodes the statement being read may still build.
        static NODES: ::std::cell::Cell<usize> = const { ::std::cell::Cell::new(0) };
    }

    /// One statement's claim on the budget, held for as long as that statement is being
    /// read and released when it is dropped.
    pub(super) struct Claim;

    impl Claim {
        /// Claim one node at one level deeper, or refuse with the bound that was
        /// reached.
        ///
        /// The claim is constructed BEFORE either bound is checked, so a refusal
        /// releases the level it took exactly as a successful read does.
        pub(super) fn enter(
        ) -> ::std::result::Result<Self, ::aws_smithy_schema::serde::SerdeError> {
            let depth = DEPTH.with(|depth| {
                let entered = depth.get() + 1;
                depth.set(entered);
                entered
            });
            let claim = Self;
            if depth == 1 {
                // The outermost statement of this read: the budget is this request's.
                NODES.with(|nodes| nodes.set(MAX_STATEMENT_NODES));
            }
            if depth > MAX_STATEMENT_DEPTH {
                return ::std::result::Result::Err(
                    ::aws_smithy_schema::serde::SerdeError::custom(statement_depth_message()),
                );
            }
            let claimed = NODES.with(|nodes| match nodes.get().checked_sub(1) {
                ::std::option::Option::Some(left) => {
                    nodes.set(left);
                    true
                }
                ::std::option::Option::None => false,
            });
            if !claimed {
                return ::std::result::Result::Err(
                    ::aws_smithy_schema::serde::SerdeError::custom(statement_nodes_message()),
                );
            }
            ::std::result::Result::Ok(claim)
        }
    }

    impl ::std::ops::Drop for Claim {
        fn drop(&mut self) {
            DEPTH.with(|depth| depth.set(depth.get().saturating_sub(1)));
        }
    }
}

/// A logical combination of filters, or one leaf filter
/// (`com.amazonaws.acm#CertificateFilterStatement`).
///
/// Written out rather than carrying `#[smithy_macros::smithy(union_for = ...)]` for two
/// reasons, both about what a caller is told:
///
/// * The generated reader refuses a body that sets two members with `union '<Shape>' had
///   more than one member set` — an internal Smithy phrasing no refusal of this service
///   states, naming neither member. ACM names the union, states the exactly-one-member
///   constraint and lists the members the request set
///   ([`exactly_one_member_message`]).
/// * A statement's depth and size are bounded by this service, and the reader is the
///   first walk a request's statement reaches, so it is where those bounds are stated
///   and where a statement past them stops being built ([`statement_budget`]).
///
/// Everything else below — the member schemas, their indices, and the read and write
/// arms — mirrors what that macro emits for this union, member for member.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
pub enum CertificateFilterStatement {
    And(Vec<CertificateFilterStatement>),
    Or(Vec<CertificateFilterStatement>),
    Not(Box<CertificateFilterStatement>),
    Filter(CertificateFilter),
}

impl CertificateFilterStatement {
    const M_AND: ::aws_smithy_schema::Schema =
        filter_union_member!("CertificateFilterStatement", "And", List, 0);
    const M_OR: ::aws_smithy_schema::Schema =
        filter_union_member!("CertificateFilterStatement", "Or", List, 1);
    const M_NOT: ::aws_smithy_schema::Schema =
        filter_union_member!("CertificateFilterStatement", "Not", Union, 2);
    const M_FILTER: ::aws_smithy_schema::Schema =
        filter_union_member!("CertificateFilterStatement", "Filter", Union, 3);

    pub const SCHEMA: ::aws_smithy_schema::Schema = ::aws_smithy_schema::Schema::new_struct(
        ::aws_smithy_schema::ShapeId::from_static(
            "com.amazonaws.acm#CertificateFilterStatement",
            NAMESPACE,
            "CertificateFilterStatement",
        ),
        ::aws_smithy_schema::ShapeType::Union,
        &[&Self::M_AND, &Self::M_OR, &Self::M_NOT, &Self::M_FILTER],
    );

    /// Read the members of one statement, at a nesting level already admitted.
    fn read_members(
        d: &mut dyn ::aws_smithy_schema::serde::ShapeDeserializer,
    ) -> ::std::result::Result<Self, ::aws_smithy_schema::serde::SerdeError> {
        let mut members: Vec<&'static str> = Vec::new();
        let mut set: Option<Self> = None;
        d.read_struct(&Self::SCHEMA, &mut |member, d| {
            match member.member_index() {
                Some(0) => {
                    members.push("And");
                    set = Some(Self::And(read_statements(d, &Self::M_AND)?));
                }
                Some(1) => {
                    members.push("Or");
                    set = Some(Self::Or(read_statements(d, &Self::M_OR)?));
                }
                Some(2) => {
                    members.push("Not");
                    set = Some(Self::Not(::std::boxed::Box::new(
                        <Self as ::aws_smithy_schema_server::DeserializableStruct>::deserialize(d)?,
                    )));
                }
                Some(3) => {
                    members.push("Filter");
                    set = Some(Self::Filter(
                        <CertificateFilter as ::aws_smithy_schema_server::DeserializableStruct>
                            ::deserialize(d)?,
                    ));
                }
                _ => {}
            }
            Ok(())
        })?;
        one_member("CertificateFilterStatement", &members, set)
    }
}

pub static CERTIFICATE_FILTER_STATEMENT_SCHEMA: &::aws_smithy_schema::Schema =
    &CertificateFilterStatement::SCHEMA;

/// One `CertificateFilterStatementList`, read element by element; a failure inside an
/// element is reported against that element's position, as the generated readers report
/// it.
fn read_statements(
    d: &mut dyn ::aws_smithy_schema::serde::ShapeDeserializer,
    member: &::aws_smithy_schema::Schema,
) -> ::std::result::Result<Vec<CertificateFilterStatement>, ::aws_smithy_schema::serde::SerdeError>
{
    let mut items: Vec<CertificateFilterStatement> = Vec::new();
    let mut position = 0usize;
    d.read_list(member, &mut |dl| {
        position += 1;
        items.push(
            <CertificateFilterStatement as ::aws_smithy_schema_server::DeserializableStruct>
                ::deserialize(dl)?,
        );
        Ok(())
    })
    .map_err(|e| ::aws_smithy_schema_server::within_list_element(e, position))?;
    Ok(items)
}

impl ::aws_smithy_schema::serde::SerializableStruct for CertificateFilterStatement {
    fn serialize_members(
        &self,
        s: &mut dyn ::aws_smithy_schema::serde::ShapeSerializer,
    ) -> ::std::result::Result<(), ::aws_smithy_schema::serde::SerdeError> {
        match self {
            Self::And(v) => s.write_list(&Self::M_AND, &|sl| {
                for item in v.iter() {
                    sl.write_struct(&Self::SCHEMA, item)?;
                }
                Ok(())
            })?,
            Self::Or(v) => s.write_list(&Self::M_OR, &|sl| {
                for item in v.iter() {
                    sl.write_struct(&Self::SCHEMA, item)?;
                }
                Ok(())
            })?,
            Self::Not(v) => s.write_struct(&Self::M_NOT, v)?,
            Self::Filter(v) => s.write_struct(&Self::M_FILTER, v)?,
        }
        Ok(())
    }
}

impl ::aws_smithy_schema_server::DeserializableStruct for CertificateFilterStatement {
    fn deserialize(
        d: &mut dyn ::aws_smithy_schema::serde::ShapeDeserializer,
    ) -> ::std::result::Result<Self, ::aws_smithy_schema::serde::SerdeError> {
        // One node, one level deeper than the statement that contains this one, out of
        // the budget this read is allowed; released when this frame leaves, whichever
        // way it leaves.
        let _claim = statement_budget::Claim::enter()?;
        Self::read_members(d)
    }
}

/// One leaf filter (`com.amazonaws.acm#CertificateFilter`), written out for the same
/// refusal wording as [`CertificateFilterStatement`].
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
pub enum CertificateFilter {
    CertificateArn(String),
    X509AttributeFilter(X509AttributeFilter),
    AcmCertificateMetadataFilter(AcmCertificateMetadataFilter),
}

impl CertificateFilter {
    const M_CERTIFICATE_ARN: ::aws_smithy_schema::Schema =
        filter_union_member!("CertificateFilter", "CertificateArn", String, 0);
    const M_X509_ATTRIBUTE_FILTER: ::aws_smithy_schema::Schema =
        filter_union_member!("CertificateFilter", "X509AttributeFilter", Union, 1);
    const M_ACM_CERTIFICATE_METADATA_FILTER: ::aws_smithy_schema::Schema =
        filter_union_member!("CertificateFilter", "AcmCertificateMetadataFilter", Union, 2);

    pub const SCHEMA: ::aws_smithy_schema::Schema = ::aws_smithy_schema::Schema::new_struct(
        ::aws_smithy_schema::ShapeId::from_static(
            "com.amazonaws.acm#CertificateFilter",
            NAMESPACE,
            "CertificateFilter",
        ),
        ::aws_smithy_schema::ShapeType::Union,
        &[
            &Self::M_CERTIFICATE_ARN,
            &Self::M_X509_ATTRIBUTE_FILTER,
            &Self::M_ACM_CERTIFICATE_METADATA_FILTER,
        ],
    );
}

pub static CERTIFICATE_FILTER_SCHEMA: &::aws_smithy_schema::Schema = &CertificateFilter::SCHEMA;

impl ::aws_smithy_schema::serde::SerializableStruct for CertificateFilter {
    fn serialize_members(
        &self,
        s: &mut dyn ::aws_smithy_schema::serde::ShapeSerializer,
    ) -> ::std::result::Result<(), ::aws_smithy_schema::serde::SerdeError> {
        match self {
            Self::CertificateArn(v) => s.write_string(&Self::M_CERTIFICATE_ARN, v.as_str())?,
            Self::X509AttributeFilter(v) => s.write_struct(&Self::M_X509_ATTRIBUTE_FILTER, v)?,
            Self::AcmCertificateMetadataFilter(v) => {
                s.write_struct(&Self::M_ACM_CERTIFICATE_METADATA_FILTER, v)?
            }
        }
        Ok(())
    }
}

impl ::aws_smithy_schema_server::DeserializableStruct for CertificateFilter {
    fn deserialize(
        d: &mut dyn ::aws_smithy_schema::serde::ShapeDeserializer,
    ) -> ::std::result::Result<Self, ::aws_smithy_schema::serde::SerdeError> {
        let mut members: Vec<&'static str> = Vec::new();
        let mut set: Option<Self> = None;
        d.read_struct(&Self::SCHEMA, &mut |member, d| {
            match member.member_index() {
                Some(0) => {
                    members.push("CertificateArn");
                    set = Some(Self::CertificateArn(
                        d.read_string(&Self::M_CERTIFICATE_ARN)?,
                    ));
                }
                Some(1) => {
                    members.push("X509AttributeFilter");
                    set = Some(Self::X509AttributeFilter(
                        <X509AttributeFilter as ::aws_smithy_schema_server::DeserializableStruct>
                            ::deserialize(d)?,
                    ));
                }
                Some(2) => {
                    members.push("AcmCertificateMetadataFilter");
                    set = Some(Self::AcmCertificateMetadataFilter(
                        <AcmCertificateMetadataFilter as
                            ::aws_smithy_schema_server::DeserializableStruct>::deserialize(d)?,
                    ));
                }
                _ => {}
            }
            Ok(())
        })?;
        one_member("CertificateFilter", &members, set)
    }
}

/// One X.509 attribute filter (`com.amazonaws.acm#X509AttributeFilter`), written out for
/// the same refusal wording as [`CertificateFilterStatement`].
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
pub enum X509AttributeFilter {
    Subject(SubjectFilter),
    SubjectAlternativeName(SubjectAlternativeNameFilter),
    ExtendedKeyUsage(String),
    KeyUsage(String),
    KeyAlgorithm(String),
    SerialNumber(String),
    NotAfter(TimestampRange),
    NotBefore(TimestampRange),
}

impl X509AttributeFilter {
    const M_SUBJECT: ::aws_smithy_schema::Schema =
        filter_union_member!("X509AttributeFilter", "Subject", Union, 0);
    const M_SUBJECT_ALTERNATIVE_NAME: ::aws_smithy_schema::Schema =
        filter_union_member!("X509AttributeFilter", "SubjectAlternativeName", Union, 1);
    const M_EXTENDED_KEY_USAGE: ::aws_smithy_schema::Schema =
        filter_union_member!("X509AttributeFilter", "ExtendedKeyUsage", String, 2);
    const M_KEY_USAGE: ::aws_smithy_schema::Schema =
        filter_union_member!("X509AttributeFilter", "KeyUsage", String, 3);
    const M_KEY_ALGORITHM: ::aws_smithy_schema::Schema =
        filter_union_member!("X509AttributeFilter", "KeyAlgorithm", String, 4);
    const M_SERIAL_NUMBER: ::aws_smithy_schema::Schema =
        filter_union_member!("X509AttributeFilter", "SerialNumber", String, 5);
    const M_NOT_AFTER: ::aws_smithy_schema::Schema =
        filter_union_member!("X509AttributeFilter", "NotAfter", Structure, 6);
    const M_NOT_BEFORE: ::aws_smithy_schema::Schema =
        filter_union_member!("X509AttributeFilter", "NotBefore", Structure, 7);

    pub const SCHEMA: ::aws_smithy_schema::Schema = ::aws_smithy_schema::Schema::new_struct(
        ::aws_smithy_schema::ShapeId::from_static(
            "com.amazonaws.acm#X509AttributeFilter",
            NAMESPACE,
            "X509AttributeFilter",
        ),
        ::aws_smithy_schema::ShapeType::Union,
        &[
            &Self::M_SUBJECT,
            &Self::M_SUBJECT_ALTERNATIVE_NAME,
            &Self::M_EXTENDED_KEY_USAGE,
            &Self::M_KEY_USAGE,
            &Self::M_KEY_ALGORITHM,
            &Self::M_SERIAL_NUMBER,
            &Self::M_NOT_AFTER,
            &Self::M_NOT_BEFORE,
        ],
    );
}

pub static X509_ATTRIBUTE_FILTER_SCHEMA: &::aws_smithy_schema::Schema =
    &X509AttributeFilter::SCHEMA;

impl ::aws_smithy_schema::serde::SerializableStruct for X509AttributeFilter {
    fn serialize_members(
        &self,
        s: &mut dyn ::aws_smithy_schema::serde::ShapeSerializer,
    ) -> ::std::result::Result<(), ::aws_smithy_schema::serde::SerdeError> {
        match self {
            Self::Subject(v) => s.write_struct(&Self::M_SUBJECT, v)?,
            Self::SubjectAlternativeName(v) => {
                s.write_struct(&Self::M_SUBJECT_ALTERNATIVE_NAME, v)?
            }
            Self::ExtendedKeyUsage(v) => s.write_string(&Self::M_EXTENDED_KEY_USAGE, v.as_str())?,
            Self::KeyUsage(v) => s.write_string(&Self::M_KEY_USAGE, v.as_str())?,
            Self::KeyAlgorithm(v) => s.write_string(&Self::M_KEY_ALGORITHM, v.as_str())?,
            Self::SerialNumber(v) => s.write_string(&Self::M_SERIAL_NUMBER, v.as_str())?,
            Self::NotAfter(v) => s.write_struct(&Self::M_NOT_AFTER, v)?,
            Self::NotBefore(v) => s.write_struct(&Self::M_NOT_BEFORE, v)?,
        }
        Ok(())
    }
}

impl ::aws_smithy_schema_server::DeserializableStruct for X509AttributeFilter {
    fn deserialize(
        d: &mut dyn ::aws_smithy_schema::serde::ShapeDeserializer,
    ) -> ::std::result::Result<Self, ::aws_smithy_schema::serde::SerdeError> {
        let mut members: Vec<&'static str> = Vec::new();
        let mut set: Option<Self> = None;
        d.read_struct(&Self::SCHEMA, &mut |member, d| {
            match member.member_index() {
                Some(0) => {
                    members.push("Subject");
                    set = Some(Self::Subject(
                        <SubjectFilter as ::aws_smithy_schema_server::DeserializableStruct>
                            ::deserialize(d)?,
                    ));
                }
                Some(1) => {
                    members.push("SubjectAlternativeName");
                    set = Some(Self::SubjectAlternativeName(
                        <SubjectAlternativeNameFilter as
                            ::aws_smithy_schema_server::DeserializableStruct>::deserialize(d)?,
                    ));
                }
                Some(2) => {
                    members.push("ExtendedKeyUsage");
                    set = Some(Self::ExtendedKeyUsage(
                        d.read_string(&Self::M_EXTENDED_KEY_USAGE)?,
                    ));
                }
                Some(3) => {
                    members.push("KeyUsage");
                    set = Some(Self::KeyUsage(d.read_string(&Self::M_KEY_USAGE)?));
                }
                Some(4) => {
                    members.push("KeyAlgorithm");
                    set = Some(Self::KeyAlgorithm(d.read_string(&Self::M_KEY_ALGORITHM)?));
                }
                Some(5) => {
                    members.push("SerialNumber");
                    set = Some(Self::SerialNumber(d.read_string(&Self::M_SERIAL_NUMBER)?));
                }
                Some(6) => {
                    members.push("NotAfter");
                    set = Some(Self::NotAfter(
                        <TimestampRange as ::aws_smithy_schema_server::DeserializableStruct>
                            ::deserialize(d)?,
                    ));
                }
                Some(7) => {
                    members.push("NotBefore");
                    set = Some(Self::NotBefore(
                        <TimestampRange as ::aws_smithy_schema_server::DeserializableStruct>
                            ::deserialize(d)?,
                    ));
                }
                _ => {}
            }
            Ok(())
        })?;
        one_member("X509AttributeFilter", &members, set)
    }
}

#[smithy_macros::smithy(union_for = "com.amazonaws.acm#SubjectFilter")]
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
pub enum SubjectFilter {
    CommonName(CommonNameFilter),
}

#[smithy_macros::smithy(shape = "com.amazonaws.acm#CommonNameFilter")]
#[derive(Debug, Clone, Default)]
pub struct CommonNameFilter {
    pub value: String,
    pub comparison_operator: String,
}

#[smithy_macros::smithy(union_for = "com.amazonaws.acm#SubjectAlternativeNameFilter")]
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
pub enum SubjectAlternativeNameFilter {
    DnsName(DnsNameFilter),
}

#[smithy_macros::smithy(shape = "com.amazonaws.acm#DnsNameFilter")]
#[derive(Debug, Clone, Default)]
pub struct DnsNameFilter {
    pub value: String,
    pub comparison_operator: String,
}

#[smithy_macros::smithy(shape = "com.amazonaws.acm#TimestampRange")]
#[derive(Debug, Clone, Default)]
pub struct TimestampRange {
    pub start: Option<::aws_smithy_types::DateTime>,
    pub end: Option<::aws_smithy_types::DateTime>,
}

/// One ACM metadata filter (`com.amazonaws.acm#AcmCertificateMetadataFilter`), written
/// out for the same refusal wording as [`CertificateFilterStatement`].
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
pub enum AcmCertificateMetadataFilter {
    Status(String),
    RenewalStatus(String),
    Type(String),
    InUse(bool),
    Exported(bool),
    ExportOption(String),
    ManagedBy(String),
    ValidationMethod(String),
    CertificateKeyPairOrigin(String),
    AcmeEndpointArn(String),
    AcmeAccountId(String),
    RootHierarchyType(String),
}

impl AcmCertificateMetadataFilter {
    const M_STATUS: ::aws_smithy_schema::Schema =
        filter_union_member!("AcmCertificateMetadataFilter", "Status", String, 0);
    const M_RENEWAL_STATUS: ::aws_smithy_schema::Schema =
        filter_union_member!("AcmCertificateMetadataFilter", "RenewalStatus", String, 1);
    const M_TYPE: ::aws_smithy_schema::Schema =
        filter_union_member!("AcmCertificateMetadataFilter", "Type", String, 2);
    const M_IN_USE: ::aws_smithy_schema::Schema =
        filter_union_member!("AcmCertificateMetadataFilter", "InUse", Boolean, 3);
    const M_EXPORTED: ::aws_smithy_schema::Schema =
        filter_union_member!("AcmCertificateMetadataFilter", "Exported", Boolean, 4);
    const M_EXPORT_OPTION: ::aws_smithy_schema::Schema =
        filter_union_member!("AcmCertificateMetadataFilter", "ExportOption", String, 5);
    const M_MANAGED_BY: ::aws_smithy_schema::Schema =
        filter_union_member!("AcmCertificateMetadataFilter", "ManagedBy", String, 6);
    const M_VALIDATION_METHOD: ::aws_smithy_schema::Schema =
        filter_union_member!("AcmCertificateMetadataFilter", "ValidationMethod", String, 7);
    const M_CERTIFICATE_KEY_PAIR_ORIGIN: ::aws_smithy_schema::Schema = filter_union_member!(
        "AcmCertificateMetadataFilter",
        "CertificateKeyPairOrigin",
        String,
        8
    );
    const M_ACME_ENDPOINT_ARN: ::aws_smithy_schema::Schema =
        filter_union_member!("AcmCertificateMetadataFilter", "AcmeEndpointArn", String, 9);
    const M_ACME_ACCOUNT_ID: ::aws_smithy_schema::Schema =
        filter_union_member!("AcmCertificateMetadataFilter", "AcmeAccountId", String, 10);
    const M_ROOT_HIERARCHY_TYPE: ::aws_smithy_schema::Schema = filter_union_member!(
        "AcmCertificateMetadataFilter",
        "RootHierarchyType",
        String,
        11
    );

    pub const SCHEMA: ::aws_smithy_schema::Schema = ::aws_smithy_schema::Schema::new_struct(
        ::aws_smithy_schema::ShapeId::from_static(
            "com.amazonaws.acm#AcmCertificateMetadataFilter",
            NAMESPACE,
            "AcmCertificateMetadataFilter",
        ),
        ::aws_smithy_schema::ShapeType::Union,
        &[
            &Self::M_STATUS,
            &Self::M_RENEWAL_STATUS,
            &Self::M_TYPE,
            &Self::M_IN_USE,
            &Self::M_EXPORTED,
            &Self::M_EXPORT_OPTION,
            &Self::M_MANAGED_BY,
            &Self::M_VALIDATION_METHOD,
            &Self::M_CERTIFICATE_KEY_PAIR_ORIGIN,
            &Self::M_ACME_ENDPOINT_ARN,
            &Self::M_ACME_ACCOUNT_ID,
            &Self::M_ROOT_HIERARCHY_TYPE,
        ],
    );
}

pub static ACM_CERTIFICATE_METADATA_FILTER_SCHEMA: &::aws_smithy_schema::Schema =
    &AcmCertificateMetadataFilter::SCHEMA;

impl ::aws_smithy_schema::serde::SerializableStruct for AcmCertificateMetadataFilter {
    fn serialize_members(
        &self,
        s: &mut dyn ::aws_smithy_schema::serde::ShapeSerializer,
    ) -> ::std::result::Result<(), ::aws_smithy_schema::serde::SerdeError> {
        match self {
            Self::Status(v) => s.write_string(&Self::M_STATUS, v.as_str())?,
            Self::RenewalStatus(v) => s.write_string(&Self::M_RENEWAL_STATUS, v.as_str())?,
            Self::Type(v) => s.write_string(&Self::M_TYPE, v.as_str())?,
            Self::InUse(v) => s.write_boolean(&Self::M_IN_USE, *v)?,
            Self::Exported(v) => s.write_boolean(&Self::M_EXPORTED, *v)?,
            Self::ExportOption(v) => s.write_string(&Self::M_EXPORT_OPTION, v.as_str())?,
            Self::ManagedBy(v) => s.write_string(&Self::M_MANAGED_BY, v.as_str())?,
            Self::ValidationMethod(v) => s.write_string(&Self::M_VALIDATION_METHOD, v.as_str())?,
            Self::CertificateKeyPairOrigin(v) => {
                s.write_string(&Self::M_CERTIFICATE_KEY_PAIR_ORIGIN, v.as_str())?
            }
            Self::AcmeEndpointArn(v) => s.write_string(&Self::M_ACME_ENDPOINT_ARN, v.as_str())?,
            Self::AcmeAccountId(v) => s.write_string(&Self::M_ACME_ACCOUNT_ID, v.as_str())?,
            Self::RootHierarchyType(v) => {
                s.write_string(&Self::M_ROOT_HIERARCHY_TYPE, v.as_str())?
            }
        }
        Ok(())
    }
}

impl ::aws_smithy_schema_server::DeserializableStruct for AcmCertificateMetadataFilter {
    fn deserialize(
        d: &mut dyn ::aws_smithy_schema::serde::ShapeDeserializer,
    ) -> ::std::result::Result<Self, ::aws_smithy_schema::serde::SerdeError> {
        let mut members: Vec<&'static str> = Vec::new();
        let mut set: Option<Self> = None;
        d.read_struct(&Self::SCHEMA, &mut |member, d| {
            match member.member_index() {
                Some(0) => {
                    members.push("Status");
                    set = Some(Self::Status(d.read_string(&Self::M_STATUS)?));
                }
                Some(1) => {
                    members.push("RenewalStatus");
                    set = Some(Self::RenewalStatus(
                        d.read_string(&Self::M_RENEWAL_STATUS)?,
                    ));
                }
                Some(2) => {
                    members.push("Type");
                    set = Some(Self::Type(d.read_string(&Self::M_TYPE)?));
                }
                Some(3) => {
                    members.push("InUse");
                    set = Some(Self::InUse(d.read_boolean(&Self::M_IN_USE)?));
                }
                Some(4) => {
                    members.push("Exported");
                    set = Some(Self::Exported(d.read_boolean(&Self::M_EXPORTED)?));
                }
                Some(5) => {
                    members.push("ExportOption");
                    set = Some(Self::ExportOption(d.read_string(&Self::M_EXPORT_OPTION)?));
                }
                Some(6) => {
                    members.push("ManagedBy");
                    set = Some(Self::ManagedBy(d.read_string(&Self::M_MANAGED_BY)?));
                }
                Some(7) => {
                    members.push("ValidationMethod");
                    set = Some(Self::ValidationMethod(
                        d.read_string(&Self::M_VALIDATION_METHOD)?,
                    ));
                }
                Some(8) => {
                    members.push("CertificateKeyPairOrigin");
                    set = Some(Self::CertificateKeyPairOrigin(
                        d.read_string(&Self::M_CERTIFICATE_KEY_PAIR_ORIGIN)?,
                    ));
                }
                Some(9) => {
                    members.push("AcmeEndpointArn");
                    set = Some(Self::AcmeEndpointArn(
                        d.read_string(&Self::M_ACME_ENDPOINT_ARN)?,
                    ));
                }
                Some(10) => {
                    members.push("AcmeAccountId");
                    set = Some(Self::AcmeAccountId(
                        d.read_string(&Self::M_ACME_ACCOUNT_ID)?,
                    ));
                }
                Some(11) => {
                    members.push("RootHierarchyType");
                    set = Some(Self::RootHierarchyType(
                        d.read_string(&Self::M_ROOT_HIERARCHY_TYPE)?,
                    ));
                }
                _ => {}
            }
            Ok(())
        })?;
        one_member("AcmCertificateMetadataFilter", &members, set)
    }
}

#[smithy_macros::smithy(shape = "com.amazonaws.acm#SearchCertificatesResponse")]
#[derive(Debug, Clone, Default)]
pub struct SearchCertificatesOutput {
    pub results: Option<Vec<CertificateSearchResult>>,
    pub next_token: Option<String>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.acm#CertificateSearchResult")]
#[derive(Debug, Clone, Default)]
pub struct CertificateSearchResult {
    pub certificate_arn: Option<String>,
    pub x509_attributes: Option<X509Attributes>,
    pub certificate_metadata: Option<CertificateMetadata>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.acm#X509Attributes")]
#[derive(Debug, Clone, Default)]
pub struct X509Attributes {
    pub issuer: Option<DistinguishedName>,
    pub subject: Option<DistinguishedName>,
    pub subject_alternative_names: Option<Vec<GeneralName>>,
    pub extended_key_usages: Option<Vec<String>>,
    pub key_algorithm: Option<String>,
    pub key_usages: Option<Vec<String>>,
    pub serial_number: Option<String>,
    pub not_after: Option<::aws_smithy_types::DateTime>,
    pub not_before: Option<::aws_smithy_types::DateTime>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.acm#DistinguishedName")]
#[derive(Debug, Clone, Default)]
pub struct DistinguishedName {
    pub common_name: Option<String>,
    pub domain_components: Option<Vec<String>>,
    pub country: Option<String>,
    pub custom_attributes: Option<Vec<CustomAttribute>>,
    pub distinguished_name_qualifier: Option<String>,
    pub generation_qualifier: Option<String>,
    pub given_name: Option<String>,
    pub initials: Option<String>,
    pub locality: Option<String>,
    pub organization: Option<String>,
    pub organizational_unit: Option<String>,
    pub pseudonym: Option<String>,
    pub serial_number: Option<String>,
    pub state: Option<String>,
    pub surname: Option<String>,
    pub title: Option<String>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.acm#CustomAttribute")]
#[derive(Debug, Clone, Default)]
pub struct CustomAttribute {
    pub object_identifier: Option<String>,
    pub value: Option<String>,
}

#[smithy_macros::smithy(union_for = "com.amazonaws.acm#GeneralName")]
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
pub enum GeneralName {
    DirectoryName(DistinguishedName),
    DnsName(String),
    IpAddress(String),
    OtherName(OtherName),
    RegisteredId(String),
    Rfc822Name(String),
    UniformResourceIdentifier(String),
}

#[smithy_macros::smithy(shape = "com.amazonaws.acm#OtherName")]
#[derive(Debug, Clone, Default)]
pub struct OtherName {
    pub object_identifier: Option<String>,
    pub value: Option<String>,
}

#[smithy_macros::smithy(union_for = "com.amazonaws.acm#CertificateMetadata")]
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
pub enum CertificateMetadata {
    AcmCertificateMetadata(AcmCertificateMetadata),
}

#[smithy_macros::smithy(shape = "com.amazonaws.acm#AcmCertificateMetadata")]
#[derive(Debug, Clone, Default)]
pub struct AcmCertificateMetadata {
    pub created_at: Option<::aws_smithy_types::DateTime>,
    pub exported: Option<bool>,
    pub imported_at: Option<::aws_smithy_types::DateTime>,
    pub in_use: Option<bool>,
    pub issued_at: Option<::aws_smithy_types::DateTime>,
    pub renewal_eligibility: Option<String>,
    pub revoked_at: Option<::aws_smithy_types::DateTime>,
    pub status: Option<String>,
    pub renewal_status: Option<String>,
    pub r#type: Option<String>,
    pub export_option: Option<String>,
    pub managed_by: Option<String>,
    pub validation_method: Option<String>,
    pub certificate_key_pair_origin: Option<String>,
    pub acme_endpoint_arn: Option<String>,
    pub acme_account_id: Option<String>,
    pub certificate_usage: Option<String>,
    pub root_hierarchy_type: Option<String>,
}

