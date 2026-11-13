// SPDX-License-Identifier: CC0-1.0

//! How this service's `restXml` responses serialize an UNWRAPPED output document.
//!
//! One S3 operation answers a document with no envelope at all: `GetBucketLocation`
//! sends `<LocationConstraint>us-west-2</LocationConstraint>` as the whole body, the
//! single output member promoted to the response root. The model says so twice — the
//! output shape carries `@xmlName("LocationConstraint")` and the operation carries
//! `aws.customizations#s3UnwrappedXmlOutput`
//! (`smithy_query(query="operation",operation="GetBucketLocation",
//! model_path="services/service-com-amazonaws-s3-amazons3/smithy-model/com-amazonaws-s3-amazons3.json")`
//! reports `s3UnwrappedXmlOutput: true`) — and every SDK reads the Region off that
//! root's own text: botocore's `parse_get_bucket_location` takes `root.text` and reports
//! `None` for anything nested.
//!
//! The framework's generic restXml serializer opens the output shape's element and then
//! writes each member inside it, which for this one operation produces the member nested
//! under an identically-named root — a document whose root carries no text, so the Region
//! is lost. [`UnwrappedXmlRestXml`] is the protocol `service.rs` hands the dispatch
//! sequence through `Service::override_protocol`: every operation but the unwrapped ones
//! is the framework's own restXml protocol, delegated to unchanged, and an operation named
//! by [`UNWRAPPED_XML_OUTPUT_OPERATIONS`] has its single member written AS the root.
//!
//! This is the one place that rule lives, so no handler and no shape declaration mentions
//! the envelope at all.
//!
//! ## `@xmlAttribute`: a member that is an ATTRIBUTE, not an element
//!
//! The same protocol carries the second thing the framework's generic XML codec cannot
//! express. An ACL grantee's `Type` is an XML ATTRIBUTE of its own element:
//! `smithy_query(query="shape",shape="Grantee")` reports
//! `Type: {xmlName: "xsi:type", xmlAttribute: true, required: true}`, and the `xsi` prefix
//! stands for the namespace `Grant$Grantee` declares
//! (`smithy_query(query="shape",shape="Grant")` reports
//! `xmlNamespace {uri: "http://www.w3.org/2001/XMLSchema-instance", prefix: "xsi"}`), so
//! every S3 client sends and reads
//! `<Grantee xmlns:xsi="…" xsi:type="Group"><URI>…</URI></Grantee>`.
//!
//! The framework's codec reads a structure's members from its CHILD ELEMENTS and writes
//! them the same way, with no notion of an attribute, so unaided it refuses every real
//! `AccessControlPolicy` for the missing required member and answers an `<xsi:type>`
//! element whose prefix nothing declares — a document a namespace-aware SDK parser cannot
//! read at all. [`XML_ATTRIBUTE_MEMBERS`] is the one place that mapping lives:
//! [`attributes_as_elements`] normalizes an incoming attribute into the element the codec
//! binds, and [`elements_as_attributes`] puts the outgoing element back where the API
//! publishes it. Both are applied ONLY to a document that carries one, so every other
//! request and response goes through the framework's own path untouched.
//!
//! A `Grantee` is not the ACL's alone: `LoggingEnabled$TargetGrants` reaches one too
//! (`smithy_query(query="shape",shape="TargetGrant")`), so both normalizations are keyed on
//! the DOCUMENT — a request whose body is an XML document, a response whose body spells the
//! attribute — and not on which operation is being served. (The one normalization that IS
//! keyed on an operation's payload member is the website document below, which exists
//! BECAUSE one operation owns that document's refusals.)
//!
//! ## `@xmlName` on a UNION member: an element the codec does not match
//!
//! The same protocol carries one more thing the framework's generic XML codec cannot
//! express. A union's set member is read from the child element whose local name is the
//! member's own SMITHY name, so a union member carrying an `@xmlName` arrives under a name
//! that matches nothing and the union deserializes as "no member set" — a request every S3
//! client sends correctly, refused for a member it did set.
//! `UpdateObjectEncryption`'s payload is the one such document in this service:
//! `smithy_query(query="shape",shape="ObjectEncryption")` reports its single member
//! `SSEKMS` with `xmlName: "SSE-KMS"`. [`XML_NAMED_UNION_MEMBERS`] is the one place that
//! mapping lives, and [`published_names_as_members`] normalizes an incoming document onto
//! the member name the codec binds. Nothing is normalized on the way OUT: no operation in
//! this service answers a union.
//!
//! ## A document the OPERATION, not the boundary, refuses
//! One normalization IS keyed on the operation, and on purpose. S3 publishes a refusal of
//! its own for several members of a `WebsiteConfiguration` — one that names the member in
//! `<ArgumentName>` and echoes what it held in `<ArgumentValue>` — and two of those members
//! carry a MODELLED constraint as well (`ErrorDocument$Key`'s minimum length and
//! `Protocol`'s value set), so the request boundary would refuse them before
//! `PutBucketWebsite` ran, in a [`smithy_server_runtime::Refused`] that carries a code and a
//! sentence and has no room for either element. The same goes for a document the published
//! schema rejects: the codec reads an element it does not know by IGNORING it, so what
//! reaches a handler no longer says whether the caller spelled one.
//!
//! So a website document `crate::state::website_document_rejection` reports a rejection for
//! is withheld from the codec in full — the codec is handed
//! [`EMPTY_WEBSITE_CONFIGURATION`], a container with no member in it — and that operation
//! answers the refusal from the bytes the request carried, which [`request_document`] keeps
//! verbatim. It is keyed on the payload MEMBER ([`carries_website_document`]) rather than on
//! anything the document spells, and it changes what the codec reads only for a
//! configuration that is refused, so no accepted request is affected by it.
//!
//! ## The TEXT of an element: what the codec trims, and what a boolean may be spelled
//!
//! Two more things the codec reads differently from the schema S3 publishes, both about an
//! element's TEXT rather than about a member the protocol cannot express.
//!
//! A request document is validated against that schema, where a boolean element is an
//! `xsd:boolean`, whose lexical space is `true`, `false`, `1` and `0` (W3C XML Schema Part 2,
//! §3.2.2) — the codec reads the two words only, so a document spelling one the numeric way is
//! refused where the published schema admits it. [`XML_BOOLEAN_DOCUMENT_ELEMENTS`] is the
//! catalog of the elements a request document binds a boolean from and
//! [`xsd_boolean_spellings_folded`] re-spells those two values onto the words; text in neither
//! spelling is left exactly as it arrived, for the codec to refuse.
//!
//! And the codec reads every element's text TRIMMED, which is right for a value whose own
//! lexical space ignores surrounding whitespace and wrong for one the caller CHOSE: an object
//! key may be composed of whitespace (`DeleteObjects` names such a key in its own
//! documentation) or begin with it, and trimming makes the first the empty string its modelled
//! minimum length then refuses and the second another key altogether.
//! [`XML_OBJECT_KEY_DOCUMENT_ELEMENTS`] is the catalog of the elements a request document
//! carries a key in and [`verbatim_text_folded`] re-spells the whitespace at either end as the
//! character references that state it, which the codec then decodes back to the same text.
//!
//! Both catalogs are DERIVED from the model — the first from the members whose target is a
//! boolean shape, the second from those targeting `ObjectKey` — and
//! `tests/all/zzz_batch_probe.rs` re-derives each so a model update cannot leave one behind.
//! Neither reaches the framework's own codec, which is vendored and read-only here: what they
//! change is the document THIS service hands it.
//!
//! ## A MODE header, whose value S3 reads case-insensitively
//!
//! The last thing this protocol normalizes is in a HEADER rather than in a document. S3 reads
//! the value of `x-amz-checksum-mode` as the NAME OF A MODE, not as a case-sensitive
//! identifier: the AWS CLI's own `s3 cp` download path sends it lowercase and AWS serves the
//! read, while the member's target enum lists the one value `ENABLED`, so matching the value
//! verbatim refuses `InvalidArgument` for a request every such client sends.
//! [`MODE_HEADER_VALUES`] is the one place that rule lives, and [`mode_headers_folded`] folds
//! an incoming value onto the model's spelling — only its CASE, so a mode the model does not
//! list is still refused, and only for an operation whose own input binds that header.
//!
//! ## An output that binds NOTHING to the body: a response with no document
//!
//! The framework's generic serializer always opens the output shape's element, so an
//! operation whose every output member is bound to a response HEADER still answers an empty
//! envelope — `<PutObjectOutput></PutObjectOutput>` for a write whose whole result is its
//! `ETag` and `x-amz-version-id`. S3 sends no body there at all, and a client that reads the
//! response DOCUMENT for the write's result finds a document carrying none of it.
//! [`without_unbound_body`] is the one place that rule lives, keyed on the output SCHEMA so
//! every such operation answers the same way; an output with a payload member or a member
//! written into the envelope keeps the document that IS its response.
//!
//! ## A BODY that is not XML: the media type it is described by
//!
//! One response of this service carries a document in another language. `GetBucketPolicy`'s
//! payload is "the bucket policy as a JSON document" (`GetBucketPolicyOutput$Policy`), and a
//! payload targeting a plain string gets Smithy's default `text/plain`, so a client or a
//! proxy that dispatches on `content-type` will not parse the body it was handed.
//! [`JSON_PAYLOAD_OUTPUT_OPERATIONS`] is the one place that fact lives, and
//! [`with_json_payload_content_type`] is what applies it; nothing about the body changes.
//!
//! ## A HEADER the API reports and the model declares no member for
//!
//! The last thing this module carries is a response header an output shape has nowhere to
//! put. S3 reports the object version every object sub-resource call acted on in
//! `x-amz-version-id`, and this model binds a member for it on the three tagging operations
//! alone — the ACL and Object Lock sub-resources model only `RequestCharged`, so their
//! generated output shapes cannot carry the version. [`ActedOnVersion`] is the one place
//! that binding lives: it wraps a modelled output, writes it unchanged, and reports the
//! version beside it through [`ACTED_ON_VERSION_ID_MEMBER`].
//!
//! ## An INSTANT, at the resolution S3 reports one
//!
//! Every instant S3 writes into a document is in one fixed-width form —
//! `2026-11-02T10:44:23.000Z`, whole seconds and a three-digit fraction — whichever
//! operation reports it, because the instants it records have one-second resolution. The
//! framework's serializer renders a timestamp at whatever resolution the value carries and
//! drops trailing zeros from its fraction, so an instant taken from the clock goes out in a
//! width no S3 response has. [`with_second_resolution_timestamps`] is the one place that
//! rule lives, scoped to the elements the model reports a timestamp in
//! ([`DOCUMENT_TIMESTAMP_ELEMENTS`]).
//!
//! ## A BUCKET NAME S3 holds no bucket under
//!
//! The one refusal this protocol makes that is not about a document: a request addressing a
//! bucket whose name is outside the published naming rules names no bucket in any account,
//! and S3 refuses it for the name rather than reporting the bucket as absent. Every
//! operation bound under the `{Bucket}` label is refused the same way, so the rule lives
//! here, at the point every such request passes through — see
//! [`UnwrappedXmlRestXml::invalid_bucket_name_violation`].

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use aws_smithy_runtime_api::http::{Request, Response, StatusCode};
use aws_smithy_schema::serde::{SerdeError, SerializableStruct, ShapeDeserializer, ShapeSerializer};
use aws_smithy_schema::{Schema, ShapeId, ShapeType};
use aws_smithy_schema_server::constraints::{path, EnumTrait, Violation, ViolationKind};
use aws_smithy_schema_server::protocol::{ProtocolError, ServerProtocol};
use aws_smithy_schema_server::xml::XmlShapeDeserializer;
use aws_smithy_schema_server::{xml_escape, HttpBindingDeserializer, RestXmlServerProtocol};
use aws_smithy_types::body::SdkBody;
use aws_smithy_types::{BigDecimal, BigInteger, Blob, DateTime, Document};
use quick_xml::events::{BytesEnd, BytesStart, BytesText, Event};
use quick_xml::{Reader, Writer};

/// The `xmlns` every S3 response document carries, from the model's own
/// `@xmlNamespace` (`smithy_query(query="service_info")` reports
/// `xmlNamespace.uri`). The framework stamps it on an enveloped response's root; an
/// unwrapped one gets it here, on the root this module writes.
pub const S3_XML_NAMESPACE: &str = "http://s3.amazonaws.com/doc/2006-03-01/";

/// Whether this service's model declares `restXml`'s `noErrorWrapping`
/// (`smithy_query(query="service_info")` reports `protocol_config.restXml.noErrorWrapping:
/// true`), which is what makes an S3 error document a bare `<Error>` root. The generated
/// protocol is built with the same flag; this protocol replaces it, so it must carry it
/// too or every error would go out in an envelope no S3 client reads.
const NO_ERROR_WRAPPING: bool = true;

/// The operations whose output document carries NO envelope: the single output member's
/// element is the response root.
///
/// Source: the operations this service's model marks
/// `aws.customizations#s3UnwrappedXmlOutput`, re-derived from the model by
/// `tests/all/bucket_location.rs` so a model update that marks another operation cannot
/// quietly keep the envelope.
pub const UNWRAPPED_XML_OUTPUT_OPERATIONS: &[&str] = &["GetBucketLocation"];

/// The operations whose response body is a JSON DOCUMENT, and so is described as one.
///
/// An `@httpPayload` member IS the body, and Smithy's default media type for a payload
/// targeting a plain string is `text/plain` — correct for a payload that is text, wrong for
/// one that is a document in another language, which a client or a proxy dispatching on
/// `content-type` will then not parse. S3 has exactly one such response: the bucket policy,
/// "the bucket policy as a JSON document" (`GetBucketPolicyOutput$Policy`, the operation's
/// only output member and its payload), which AWS serves as
/// [`JSON_PAYLOAD_CONTENT_TYPE`]. The model carries no `@mediaType` for it
/// (`smithy_query(query="shape",shape="Policy")` reports a bare string), so the fact lives
/// here, keyed on the operation, beside the other thing the generic serializer cannot know.
pub const JSON_PAYLOAD_OUTPUT_OPERATIONS: &[&str] = &["GetBucketPolicy"];

/// The operations whose response document names its ROOT element something other than the
/// name the model resolves for the output shape, as `(operation, modelled, served)`.
///
/// The root element of a restXml response is the output shape's `xmlName`, else the shape's
/// own name ([`root_element_name`]) — and for two of the four bucket-configuration listings
/// neither is the element the service sends. A client reads the result out of the element the
/// service SENDS, so the served name is the contract and the resolved one is what this table
/// corrects; nothing else about the document changes.
///
/// The two rows, and why the other two listings need none:
///
/// * the analytics listing's output shape declares `ListBucketAnalyticsConfigurationResult`
///   (`smithy_query(query="shape",shape="ListBucketAnalyticsConfigurationsOutput")`) where
///   the service answers the PLURAL `ListBucketAnalyticsConfigurationsResult`;
/// * the S3 Intelligent-Tiering listing's output shape declares NO `xmlName`
///   (`smithy_query(query="shape",shape="ListBucketIntelligentTieringConfigurationsOutput")`),
///   so the resolved name is the shape's own —
///   `ListBucketIntelligentTieringConfigurationsOutput`, which is no element S3 sends — where
///   the service answers `ListIntelligentTieringConfigurationsResult`, the `List…Result` form
///   its sibling listings' output shapes declare for themselves;
/// * the metrics and S3 Inventory listings declare `ListMetricsConfigurationsResult` and
///   `ListInventoryConfigurationsResult`, which ARE the elements the service sends, so their
///   documents go out as the model resolves them.
const SERVED_OUTPUT_ROOT_ELEMENTS: &[(&str, &str, &str)] = &[
    (
        "ListBucketAnalyticsConfigurations",
        "ListBucketAnalyticsConfigurationResult",
        "ListBucketAnalyticsConfigurationsResult",
    ),
    (
        "ListBucketIntelligentTieringConfigurations",
        "ListBucketIntelligentTieringConfigurationsOutput",
        "ListIntelligentTieringConfigurationsResult",
    ),
];

/// The `content-type` a response whose body is a JSON document carries — see
/// [`JSON_PAYLOAD_OUTPUT_OPERATIONS`].
pub const JSON_PAYLOAD_CONTENT_TYPE: &str = "application/json";

/// Whether a name is an address this emulator serves a bucket's objects at — the one
/// question an ADDRESS cannot answer from its own shape (see [`super::addressed_in_host`]).
///
/// A closure rather than an engine, so this file names no engine type and holds no store of
/// its own: `super`'s constructor captures the one shared engine it was handed.
pub type BucketLookup = Arc<dyn Fn(&str) -> bool + Send + Sync>;

/// Which ACCESS POINT an address reaches, when it reaches one — the other thing an address
/// cannot answer from its own shape (see [`crate::state::access_points`]).
///
/// A closure for the same reason as [`BucketLookup`], over the same shared engine.
pub type AccessPointLookup =
    Arc<dyn Fn(&str) -> Option<crate::state::access_points::AccessPointRoute> + Send + Sync>;

/// This service's wire protocol: `restXml` exactly as the model declares it, with an
/// unwrapped output document for the operations the model marks as carrying one.
///
/// Everything but those outputs is the framework's own restXml protocol, delegated to
/// unchanged — request deserialization and the bare `<Error>` envelope included.
pub struct UnwrappedXmlRestXml {
    inner: RestXmlServerProtocol,
    /// Whether a name is a bucket this emulator holds, for the one question an ADDRESS
    /// cannot answer from its own shape: whether a request's first HOST label is the bucket
    /// (virtual-hosted style) or part of the endpoint's name (path style). See
    /// [`super::addressed_in_host`] for the rule and [`Self::with_buckets`] for where the
    /// answer comes from.
    holds_bucket: Option<BucketLookup>,
    /// Which access point an address reaches, for the request-scoped note the decision on
    /// an access point path is taken from. See [`Self::with_access_points`].
    addressed_access_point: Option<AccessPointLookup>,
}

/// Hand-written because the bucket lookup is a closure, which carries no `Debug` of its own;
/// the protocol is otherwise its inner one plus that one function.
impl ::std::fmt::Debug for UnwrappedXmlRestXml {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        f.debug_struct("UnwrappedXmlRestXml")
            .field("inner", &self.inner)
            .field("holds_bucket", &self.holds_bucket.is_some())
            .field(
                "addressed_access_point",
                &self.addressed_access_point.is_some(),
            )
            .finish()
    }
}

impl UnwrappedXmlRestXml {
    /// Build the protocol once, at service construction: `override_protocol` borrows it
    /// from `&self` and must never build one per request.
    ///
    /// The route table is EMPTY by design. A protocol's routes are read by
    /// [`ServerProtocol::routing_key`] alone, and the router resolves a request's
    /// operation through the generated `ServiceDefinition`'s own protocol (whose table is
    /// the model's); an author-overridden protocol drives only the modeled sequence —
    /// `deserialize_input`, `serialize_output`, `serialize_error` — none of which consults
    /// routes.
    pub fn new() -> Self {
        Self {
            inner: RestXmlServerProtocol::new(&[])
                .with_namespace(S3_XML_NAMESPACE)
                .with_no_error_wrapping(NO_ERROR_WRAPPING),
            holds_bucket: None,
            addressed_access_point: None,
        }
    }

    /// The same protocol, able to ask whether a name is a bucket this emulator holds.
    ///
    /// Taken as a [`BucketLookup`] rather than as an engine, so this file names no engine
    /// type and holds no store of its own. A protocol built WITHOUT it reads every request
    /// as path-style, which is what a protocol constructed outside a service (a unit test of
    /// the codec) has.
    pub fn with_buckets(mut self, holds_bucket: BucketLookup) -> Self {
        self.holds_bucket = Some(holds_bucket);
        self
    }

    /// The bucket a virtual-hosted-style `request` addresses in its host, when it is one —
    /// see [`super::addressed_in_host`]. The LABEL as the host spelled it, which is an
    /// access point's address when the request was addressed to one.
    fn addressed_in_host(&self, request: &Request) -> Option<String> {
        let holds_bucket = self.holds_bucket.as_ref()?;
        let host = super::request_host_of(request)?;
        super::addressed_in_host(host, request_path(request.uri()), &**holds_bucket)
            .map(str::to_string)
    }

    /// The same protocol, able to resolve the ACCESS POINT an address reaches.
    ///
    /// Taken as an [`AccessPointLookup`] for the same reason as [`Self::with_buckets`]'s
    /// closure. A protocol built WITHOUT it resolves no access point, which is what a
    /// protocol constructed outside a service (a unit test of the codec) has.
    pub fn with_access_points(mut self, addressed_access_point: AccessPointLookup) -> Self {
        self.addressed_access_point = Some(addressed_access_point);
        self
    }

    /// The access point `request` was addressed THROUGH, when its address reaches one: the
    /// first label of its host for a virtual-hosted-style request — the form every AWS SDK
    /// rewrites an access point ARN into — else the first segment of its path, which is
    /// where an access point's ARN or alias arrives when a client sends one.
    fn addressed_access_point(
        &self,
        request: &Request,
    ) -> Option<crate::state::access_points::AccessPointRoute> {
        let addressed_access_point = self.addressed_access_point.as_ref()?;
        let address = self.addressed_in_host(request).or_else(|| {
            super::first_path_segment(request_path(request.uri()))
                .map(super::sigv4::percent_decoded)
        })?;
        addressed_access_point(&address)
    }

    /// The request's address in the PATH-STYLE form every route of this service is bound in
    /// (`/{Bucket}`, `/{Bucket}/{Key+}`), when that is not the path the request arrived
    /// with — `None` when it already is.
    ///
    /// Two forms are not. A VIRTUAL-HOSTED-STYLE request names the address in its host and
    /// leaves its path as the key alone, so the bucket that address reaches goes back in
    /// front of it ([`super::path_with_bucket`]). And a request whose first path segment is
    /// an ACCESS POINT's address addresses the bucket that access point fronts, so that
    /// bucket replaces the segment ([`super::access_point_addressed_path`], the one reading
    /// of that rewrite, which also decides which requests an access point serves at all).
    fn path_style_path(&self, request: &Request) -> Option<String> {
        let path = request_path(request.uri());
        if let Some(label) = self.addressed_in_host(request) {
            // An access point's endpoint label addresses the bucket it fronts, for a request
            // that access point's data plane serves; a request it does not serve addresses
            // no object of it, so it is not resolved to that bucket at all.
            let bucket = match self.addressed_access_point(request) {
                Some(route) => {
                    if !super::access_point_serves(request.method(), path) {
                        return None;
                    }
                    route.bucket
                }
                None => label,
            };
            return Some(super::path_with_bucket(&bucket, path));
        }
        let addressed_access_point = self.addressed_access_point.as_ref()?;
        super::access_point_addressed_path(request.method(), path, &**addressed_access_point)
    }

    /// The access point this request is SERVED through: the one its address reaches, when
    /// the access point's data plane serves the request at all.
    ///
    /// Read off the REWRITE rather than deciding again: the bucket the labels bind from is
    /// the fronted bucket exactly for a request the access point serves
    /// ([`Self::path_style_path`]), so a request whose address reaches one and whose bound
    /// path still names that address is not one it serves — and the decision taken on such a
    /// request is not taken over the access point's resources either.
    fn served_access_point(
        &self,
        request: &Request,
    ) -> Option<crate::state::access_points::AccessPointRoute> {
        let route = self.addressed_access_point(request)?;
        let path_style = self.path_style_path(request)?;
        super::first_path_segment(&path_style)
            .is_some_and(|segment| segment == route.bucket)
            .then_some(route)
    }
}

impl Default for UnwrappedXmlRestXml {
    fn default() -> Self {
        Self::new()
    }
}

/// The XML element an output shape occupies: its `@xmlName` when the model renames it
/// (`GetBucketLocationOutput` is `LocationConstraint`), else the shape's own name — the
/// same resolution the framework's XML codec performs.
fn root_element_name(schema: &Schema) -> &str {
    match schema.xml_name() {
        Some(name) => name.value(),
        None => schema.shape_id().shape_name(),
    }
}

/// `x-amz-version-id`, as an object sub-resource response reports the version it acted on.
///
/// S3 answers an object sub-resource call — the tag set, the ACL, the Object Lock legal hold
/// or retention of one object version — with the id of the version it read or wrote in this
/// header, and this model binds an output member for it on only three of them
/// (`smithy_query(query="operation",operation="PutObjectAcl",
/// model_path="services/service-com-amazonaws-s3-amazons3/smithy-model/com-amazonaws-s3-amazons3.json")`
/// reports an output of `RequestCharged` alone, and `GetObjectAcl`, `PutObjectLegalHold` and
/// `PutObjectRetention` the same). The operations whose output shape has nowhere to carry it
/// bind it HERE instead, through [`ActedOnVersion`]: one member declaration, carrying the
/// member name and the header every one of them reports the version under.
///
/// A member schema is all the HTTP binder needs — it routes a member carrying `@httpHeader`
/// into the response headers and never into the body — so each of those operations keeps its
/// modelled output schema, and therefore the body and the status it already answered with.
/// The member targets `ObjectVersionId`, the shape every modelled `VersionId` member of this
/// service targets.
pub static ACTED_ON_VERSION_ID_MEMBER: Schema = Schema::new_member(
    ShapeId::from_static(
        "com.amazonaws.s3#ObjectVersionId",
        "com.amazonaws.s3",
        "ObjectVersionId",
    ),
    ShapeType::String,
    "VersionId",
    0,
)
.with_http_header("x-amz-version-id");

/// A modelled output, answered with the id of the object version the call acted on.
///
/// The wrapped `output` writes its own members exactly as it does unwrapped — body document,
/// bound headers and status alike — and the version id goes out in
/// `x-amz-version-id` beside them ([`ACTED_ON_VERSION_ID_MEMBER`]).
///
/// `version_id` is the id the store holds for the version the request addressed, and `None`
/// for a version that has none: a bucket whose versioning was never enabled, and the null
/// version of a suspended one. S3 reports no version id for those, so the header is then
/// absent rather than carrying an id nothing minted.
#[derive(Debug, Clone)]
pub struct ActedOnVersion<O> {
    /// The operation's modelled output, serialized first and unchanged.
    pub output: O,
    /// The id of the object version this call read or wrote, when that version has one.
    pub version_id: Option<String>,
}

impl<O: SerializableStruct> SerializableStruct for ActedOnVersion<O> {
    fn serialize_members(&self, serializer: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
        self.output.serialize_members(serializer)?;
        if let Some(version_id) = self.version_id.as_deref() {
            serializer.write_string(&ACTED_ON_VERSION_ID_MEMBER, version_id)?;
        }
        Ok(())
    }
}

/// The ONE string an unwrapped output's single member carries, or `None` for an output
/// that wrote no string at all (an absent member — `us-east-1`'s null location
/// constraint) or wrote anything else.
///
/// An unwrapped output document is a root element holding text, so the only member shape
/// it can express is one string. Every other write is recorded as "not a shape this
/// document can carry" and makes the caller fall back to the enveloped form rather than
/// emit a document that drops a value.
#[derive(Default)]
struct SingleString {
    value: Option<String>,
    unsupported: bool,
}

impl SingleString {
    /// Note a write this document cannot carry.
    fn refuse(&mut self) -> Result<(), SerdeError> {
        self.unsupported = true;
        Ok(())
    }
}

impl ShapeSerializer for SingleString {
    fn write_string(&mut self, _schema: &Schema, value: &str) -> Result<(), SerdeError> {
        if self.value.is_some() {
            // A second member: more than a root's text, so not an unwrapped document.
            return self.refuse();
        }
        self.value = Some(value.to_string());
        Ok(())
    }

    fn write_struct(
        &mut self,
        _schema: &Schema,
        _value: &dyn SerializableStruct,
    ) -> Result<(), SerdeError> {
        self.refuse()
    }

    fn write_list(
        &mut self,
        _schema: &Schema,
        _write_elements: &dyn Fn(&mut dyn ShapeSerializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        self.refuse()
    }

    fn write_map(
        &mut self,
        _schema: &Schema,
        _write_entries: &dyn Fn(&mut dyn ShapeSerializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        self.refuse()
    }

    fn write_boolean(&mut self, _schema: &Schema, _value: bool) -> Result<(), SerdeError> {
        self.refuse()
    }

    fn write_byte(&mut self, _schema: &Schema, _value: i8) -> Result<(), SerdeError> {
        self.refuse()
    }

    fn write_short(&mut self, _schema: &Schema, _value: i16) -> Result<(), SerdeError> {
        self.refuse()
    }

    fn write_integer(&mut self, _schema: &Schema, _value: i32) -> Result<(), SerdeError> {
        self.refuse()
    }

    fn write_long(&mut self, _schema: &Schema, _value: i64) -> Result<(), SerdeError> {
        self.refuse()
    }

    fn write_float(&mut self, _schema: &Schema, _value: f32) -> Result<(), SerdeError> {
        self.refuse()
    }

    fn write_double(&mut self, _schema: &Schema, _value: f64) -> Result<(), SerdeError> {
        self.refuse()
    }

    fn write_big_integer(
        &mut self,
        _schema: &Schema,
        _value: &BigInteger,
    ) -> Result<(), SerdeError> {
        self.refuse()
    }

    fn write_big_decimal(
        &mut self,
        _schema: &Schema,
        _value: &BigDecimal,
    ) -> Result<(), SerdeError> {
        self.refuse()
    }

    fn write_blob(&mut self, _schema: &Schema, _value: &[u8]) -> Result<(), SerdeError> {
        self.refuse()
    }

    fn write_timestamp(&mut self, _schema: &Schema, _value: &DateTime) -> Result<(), SerdeError> {
        self.refuse()
    }

    fn write_document(&mut self, _schema: &Schema, _value: &Document) -> Result<(), SerdeError> {
        self.refuse()
    }

    fn write_null(&mut self, _schema: &Schema) -> Result<(), SerdeError> {
        // An absent member writes nothing on the wire either way.
        Ok(())
    }
}

/// The unwrapped document for `output`, or `None` when its members are not one string —
/// in which case the caller serializes it in the framework's enveloped form rather than
/// inventing a document for it.
///
/// An absent member (`us-east-1`'s null location constraint) is the empty root, which is
/// exactly what AWS sends and what every SDK reads back as "no constraint".
fn unwrapped_document(
    output: &dyn SerializableStruct,
    output_schema: &Schema,
) -> Result<Option<String>, SerdeError> {
    let mut members = SingleString::default();
    output.serialize_members(&mut members)?;
    if members.unsupported {
        return Ok(None);
    }
    let root = root_element_name(output_schema);
    Ok(Some(match members.value {
        Some(value) => format!(
            "<{root} xmlns=\"{S3_XML_NAMESPACE}\">{}</{root}>",
            xml_escape(&value)
        ),
        None => format!("<{root} xmlns=\"{S3_XML_NAMESPACE}\"/>"),
    }))
}

/// One member this service's model marks `@xmlAttribute`: how the API spells it, which
/// element the framework's codec binds it from, and what its prefix stands for.
///
/// `Grantee$Type` is the member this exists for; see the module docs for the
/// `smithy_query` that reports its three traits. The `element` is the member's own Smithy
/// name, which is what the codec's struct reader matches a child element's LOCAL name
/// against — the `@xmlName` (`xsi:type`) would reach it as the local name `type`, which
/// matches nothing.
#[derive(Debug, Clone, Copy)]
pub struct XmlAttributeMember {
    /// The attribute the API publishes, prefix included (`xsi:type`).
    pub attribute: &'static str,
    /// The element the framework's XML codec binds the member from (`Type`).
    pub element: &'static str,
    /// The namespace the attribute's prefix stands for, declared on the element the
    /// attribute is written back onto so the document stays namespace-well-formed.
    pub namespace: &'static str,
}

/// Every member of this service's model that travels as an XML ATTRIBUTE rather than as a
/// child element.
///
/// One row today — the ACL grantee's `Type` — and the two normalizations below are written
/// over the TABLE rather than over that one row, so a model update that marks another
/// member needs a row here and nothing else.
pub const XML_ATTRIBUTE_MEMBERS: &[XmlAttributeMember] = &[XmlAttributeMember {
    attribute: "xsi:type",
    element: "Type",
    namespace: "http://www.w3.org/2001/XMLSchema-instance",
}];

/// The member spelled by the attribute named `attribute`, if any.
fn member_by_attribute(attribute: &[u8]) -> Option<&'static XmlAttributeMember> {
    XML_ATTRIBUTE_MEMBERS
        .iter()
        .find(|member| member.attribute.as_bytes() == attribute)
}

/// The member the codec writes as the element named `element`, if any.
///
/// The WRITE path names the element by the member's `@xmlName` — the attribute spelling —
/// so that is what an outgoing document carries and what is matched here.
fn member_by_element(element: &[u8]) -> Option<&'static XmlAttributeMember> {
    XML_ATTRIBUTE_MEMBERS
        .iter()
        .find(|member| member.attribute.as_bytes() == element)
}

/// Whether `haystack` holds `needle` anywhere.
///
/// A byte scan rather than a decode: the documents this is asked about are ACL payloads,
/// but the question is asked before anything parses them.
fn holds(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.len() >= needle.len() && haystack.windows(needle.len()).any(|slice| slice == needle)
}

/// Whether the input shape binds a member as the request's XML DOCUMENT: an `@httpPayload`
/// member whose target is a structured shape (a structure, union, list or map).
///
/// What tells a request whose BODY IS A DOCUMENT from one whose body is arbitrary bytes (an
/// object's payload) or has no body at all. Only such a document can carry a member that
/// travels as an XML attribute, and only such a document may be scanned for one: an
/// object's bytes are not XML and must never be rewritten.
fn is_xml_document_payload(member: &Schema) -> bool {
    member.http_payload().is_some()
        && matches!(
            member.shape_type(),
            ShapeType::Structure
                | ShapeType::Union
                | ShapeType::List
                | ShapeType::Map
                | ShapeType::Document
        )
}

/// Whether the input shape's body is an XML document, i.e. whether this request can carry a
/// member that travels as an XML attribute.
///
/// The gate on the request side. A `Grantee` is reached by MORE than the two ACL writes —
/// `BucketLoggingStatus` reaches one through `LoggingEnabled$TargetGrants`
/// (`smithy_query(query="shape",shape="TargetGrant")`) — so the gate is the SHAPE OF THE
/// BODY rather than the name of one payload member: every operation whose body is a
/// document is normalized when that document actually spells an attribute, and every
/// operation whose body is not is left to the framework's own path without even being
/// scanned.
fn carries_xml_document_payload(input_schema: &Schema) -> bool {
    input_schema
        .members()
        .iter()
        .any(|member| is_xml_document_payload(member))
}

/// The violation to refuse `request` with when its body is an XML DOCUMENT carrying a
/// character the XML character set does not allow, or `None` when it is not or does not.
///
/// XML 1.0 defines which characters a document may contain (see
/// [`crate::state::first_forbidden_xml_char`]); a body carrying any other one is not
/// well-formed XML, whatever it otherwise says, so no codec can read it and no handler can
/// act on it. Refusing it as a violation of the member the document is BOUND to is what
/// makes it carry S3's own `MalformedXML` (`crate::service::s3_refusal` chooses the code
/// from the member), rather than reaching the body codec and surfacing as whatever that
/// codec, or the response serializer echoing the character back, happens to produce.
///
/// Scoped to a STRUCTURED `@httpPayload` member, so the bytes of an object's body — which
/// are not a document and may legitimately hold any byte at all — are never scanned.
fn xml_charset_violation(request: &Request, input_schema: &Schema) -> Option<Violation> {
    let payload = input_schema
        .members()
        .iter()
        .find(|member| is_xml_document_payload(member))?;
    let body = std::str::from_utf8(request.body().bytes()?).ok()?;
    let forbidden = crate::state::first_forbidden_xml_char(body)?;
    Some(Violation {
        path: path::member(payload.member_name()?),
        kind: ViolationKind::Pattern(XML_CHARACTER_SET.to_string()),
        value: Some(format!("{forbidden:?}")),
        sensitive: false,
    })
}

/// The XML 1.0 `Char` production, as the refusal above names the rule a body broke (W3C
/// XML 1.0, §2.2 "Characters").
///
/// `pub(crate)`: it is also what tells `crate::service::violation_is_in_request_document`
/// that a violation naming the payload member ALONE is about the DOCUMENT rather than
/// about a scalar binding of a member of the same name.
pub(crate) const XML_CHARACTER_SET: &str =
    "#x9 | #xA | #xD | [#x20-#xD7FF] | [#xE000-#xFFFD] | [#x10000-#x10FFFF]";

/// Whether the input's XML document payload is a `WebsiteConfiguration` — the one document
/// whose own refusals `PutBucketWebsite` is the place that states them, so the rewrite above
/// is scoped to it by the member the codec would bind rather than by anything the
/// document happens to spell.
fn carries_website_document(input_schema: &Schema) -> bool {
    input_schema
        .members()
        .iter()
        .find(|member| is_xml_document_payload(member))
        .and_then(|payload| payload.member_name())
        == Some(WEBSITE_CONFIGURATION_MEMBER)
}

/// The input member `PutBucketWebsite` binds its document as
/// (`smithy_query(query="operation",operation="PutBucketWebsite")` reports
/// `WebsiteConfiguration` as the `@httpPayload`).
const WEBSITE_CONFIGURATION_MEMBER: &str = "WebsiteConfiguration";

/// The website document handed to the codec in place of one the published schema rejects: the
/// container with no member in it, which parses to the configuration a refused request never
/// stores anything from. `PutBucketWebsite` reads the document the request actually carried
/// (`request_document`) and refuses it from that, so nothing is read from this stand-in.
const EMPTY_WEBSITE_CONFIGURATION: &[u8] = b"<WebsiteConfiguration></WebsiteConfiguration>";

/// The violation to refuse `request` with when its body is an XML DOCUMENT in which a member
/// the model declares a UNION does not carry EXACTLY ONE member, or `None` when every union
/// the document spells carries one (and for every request whose body is not a document).
///
/// A union is a choice: "a filter must have exactly one prefix, one tag, or one conjunction
/// (AnalyticsAndOperator)" (`AnalyticsFilter`'s own documentation). A document naming two of
/// such a union's members, or none, is therefore not a document of that shape — it is one the
/// published schema does not validate, which is S3's `MalformedXML`
/// (`crate::service::s3_refusal` chooses the code from the member the violation names).
/// Refused HERE, before the codec reads the body, because the codec reports the same document
/// as a failure it has no S3 code for and a client would then read the framework's own
/// generic one instead.
fn union_arity_violation(request: &Request, input_schema: &Schema) -> Option<Violation> {
    let payload = input_schema
        .members()
        .iter()
        .find(|member| is_xml_document_payload(member))?;
    let payload_member = path::member(payload.member_name()?);
    let unions: Vec<&(&str, &str, &[&str])> = EXACTLY_ONE_MEMBER_UNIONS
        .iter()
        .filter(|(document, _, _)| *document == payload_member)
        .collect();
    if unions.is_empty() {
        return None;
    }
    let document = request.body().bytes()?;
    let index = union_without_exactly_one_member(document, &unions)?;
    let (_, _, members) = unions[index];
    Some(Violation {
        path: ::aws_smithy_schema_server::constraints::path::join(&[
            payload_member,
            path::member(unions[index].1),
        ]),
        kind: ViolationKind::EnumValue(members.iter().map(|name| (*name).to_string()).collect()),
        value: None,
        sensitive: false,
    })
}

/// The UNION members of this service's request documents whose own documentation states that
/// the choice is EXACTLY ONE, as `(payload member, element, the members it chooses between)`.
///
/// `AnalyticsFilter` is one: "a filter must have exactly one prefix, one tag, or one
/// conjunction (AnalyticsAndOperator)", and its members are the ones
/// `smithy_query(query="shape",shape="AnalyticsFilter")` declares. `MetricsFilter` is the
/// other: "a filter must be a prefix, an object tag, an access point ARN, or a conjunction
/// (MetricsAndOperator)" (`smithy_query(query="doc",shape="MetricsFilter")`), over the four
/// members that shape declares. The lifecycle and replication rule filters are deliberately
/// NOT here: S3 documents an EMPTY `<Filter>` on a lifecycle rule as the way to apply the
/// rule to every object in the bucket, so the choice there may be none and a document naming
/// none still validates.
const EXACTLY_ONE_MEMBER_UNIONS: &[(&str, &str, &[&str])] = &[
    ("analyticsConfiguration", "Filter", &["Prefix", "Tag", "And"]),
    (
        "metricsConfiguration",
        "Filter",
        &["Prefix", "Tag", "AccessPointArn", "And"],
    ),
];

/// The index in `unions` of the first union element of `document` that does not carry exactly
/// one child element, or `None` when every one of them carries one.
///
/// A union's element is matched by NAME, which is what the codec matches it by too; its
/// children are counted at the one level below it, so a predicate's own children (a tag's
/// `Key` and `Value`) are not counted as members of the choice.
fn union_without_exactly_one_member(
    document: &[u8],
    unions: &[&(&str, &str, &[&str])],
) -> Option<usize> {
    let mut reader = Reader::from_reader(document);
    let mut depth = 0usize;
    // The union being counted: its index in `unions`, the depth its element opened at, and
    // how many child elements have been seen inside it so far.
    let mut counting: Option<(usize, usize, usize)> = None;
    loop {
        match reader.read_event() {
            Ok(Event::Start(start)) => {
                let local = String::from_utf8_lossy(start.name().as_ref()).into_owned();
                depth += 1;
                match counting.as_mut() {
                    Some((_, opened_at, children)) if depth == *opened_at + 1 => *children += 1,
                    Some(_) => {}
                    None => {
                        if let Some(index) =
                            unions.iter().position(|(_, element, _)| *element == local)
                        {
                            counting = Some((index, depth, 0));
                        }
                    }
                }
            }
            Ok(Event::Empty(empty)) => {
                let local = String::from_utf8_lossy(empty.name().as_ref()).into_owned();
                match counting.as_mut() {
                    Some((_, opened_at, children)) if depth == *opened_at => *children += 1,
                    Some(_) => {}
                    None => {
                        // A self-closing union element carries no member at all.
                        if let Some(index) =
                            unions.iter().position(|(_, element, _)| *element == local)
                        {
                            return Some(index);
                        }
                    }
                }
            }
            Ok(Event::End(_)) => {
                if let Some((index, opened_at, children)) = counting {
                    if depth == opened_at {
                        if children != 1 {
                            return Some(index);
                        }
                        counting = None;
                    }
                }
                depth = depth.saturating_sub(1);
            }
            Ok(Event::Eof) | Err(_) => return None,
            _ => {}
        }
    }
}


/// `document` with every `@xmlAttribute` member moved out of its element's start tag and
/// into a child element of that element, or `None` when it carries none (in which case
/// nothing is rewritten and the original bytes are used).
///
/// The namespace declarations are left exactly where they were: an unused `xmlns:xsi` is
/// legal, and removing it would change a document the client chose.
fn attributes_as_elements(document: &[u8]) -> Option<Vec<u8>> {
    let mut reader = Reader::from_reader(document);
    reader.config_mut().trim_text(false);
    let mut writer = Writer::new(Vec::new());
    let mut moved = false;
    loop {
        match reader.read_event() {
            Ok(Event::Eof) => break,
            Ok(Event::Start(start)) => {
                let (clean, children) = split_attribute_members(&start);
                moved |= !children.is_empty();
                writer.write_event(Event::Start(clean)).ok()?;
                for (element, value) in children {
                    write_text_element(&mut writer, element, &value)?;
                }
            }
            Ok(Event::Empty(empty)) => {
                let (clean, children) = split_attribute_members(&empty);
                if children.is_empty() {
                    writer.write_event(Event::Empty(clean)).ok()?;
                    continue;
                }
                // An element that held its members in attributes alone is self-closing;
                // it has to open and close now that they are children of it.
                moved = true;
                let name = String::from_utf8_lossy(clean.name().as_ref()).into_owned();
                writer.write_event(Event::Start(clean)).ok()?;
                for (element, value) in children {
                    write_text_element(&mut writer, element, &value)?;
                }
                writer.write_event(Event::End(BytesEnd::new(name))).ok()?;
            }
            Ok(other) => {
                writer.write_event(other).ok()?;
            }
            // A document the XML reader cannot walk is left for the framework's own
            // parser to refuse, so the refusal a client sees is the framework's.
            Err(_) => return None,
        }
    }
    moved.then(|| writer.into_inner())
}

/// `start` without the attributes that are really members, beside the elements those
/// members become.
///
/// Both halves carry the attribute's bytes VERBATIM — the kept attributes because
/// `push_attribute` copies an `Attribute`'s raw value unchanged, and a moved one because
/// it is written as pre-escaped character data. XML spells an entity reference the same
/// way in an attribute value and in character data, so the value the codec finally reads
/// is the one the client sent, decoded exactly once.
fn split_attribute_members(
    start: &BytesStart<'_>,
) -> (BytesStart<'static>, Vec<(&'static str, Vec<u8>)>) {
    let name = String::from_utf8_lossy(start.name().as_ref()).into_owned();
    let mut clean = BytesStart::new(name).into_owned();
    let mut members: Vec<(&'static str, Vec<u8>)> = Vec::new();
    for attribute in start.attributes().flatten() {
        match member_by_attribute(attribute.key.as_ref()) {
            Some(member) => members.push((member.element, attribute.value.to_vec())),
            None => clean.push_attribute(attribute),
        }
    }
    (clean, members)
}

/// Write `<name>text</name>`, with `text` taken as already-escaped character data.
fn write_text_element(writer: &mut Writer<Vec<u8>>, name: &str, text: &[u8]) -> Option<()> {
    let escaped = String::from_utf8_lossy(text).into_owned();
    writer.write_event(Event::Start(BytesStart::new(name))).ok()?;
    writer
        .write_event(Event::Text(BytesText::from_escaped(escaped)))
        .ok()?;
    writer.write_event(Event::End(BytesEnd::new(name))).ok()?;
    Some(())
}

/// One UNION member whose element the API publishes under a name the framework's XML codec
/// does not match it by.
///
/// The codec reads a union's set member from the child element whose local name is the
/// member's own SMITHY name; a union member carrying an `@xmlName` therefore arrives under
/// a name that matches nothing, and the union deserializes as "no member set". The
/// published name is what every S3 client sends, so an incoming document is normalized onto
/// the name the codec binds — the same normalization [`XML_ATTRIBUTE_MEMBERS`] does for a
/// member that travels as an attribute, and for the same reason.
#[derive(Debug, Clone, Copy)]
pub struct XmlNamedUnionMember {
    /// The element the API publishes (`ObjectEncryption$SSEKMS`'s `@xmlName`, `SSE-KMS`).
    pub published: &'static str,
    /// The member's own Smithy name, which is the element the codec binds it from
    /// (`SSEKMS`).
    pub member: &'static str,
}

/// Every union member of this service's model whose `@xmlName` differs from its member
/// name.
///
/// One row today — `ObjectEncryption$SSEKMS`
/// (`smithy_query(query="shape",shape="ObjectEncryption")` reports
/// `SSEKMS: {xmlName: "SSE-KMS"}`) — and the normalization below is written over the TABLE
/// rather than over that row. `tests/all/object_encryption_update.rs` re-derives the set
/// from the model, so a model update that names another one cannot quietly go unmatched.
pub const XML_NAMED_UNION_MEMBERS: &[XmlNamedUnionMember] = &[XmlNamedUnionMember {
    published: "SSE-KMS",
    member: "SSEKMS",
}];

/// The union member the API publishes under the element named `element`, if any.
fn union_member_by_published_name(element: &[u8]) -> Option<&'static XmlNamedUnionMember> {
    XML_NAMED_UNION_MEMBERS
        .iter()
        .find(|member| member.published.as_bytes() == element)
}

/// `document` with every published union-member element renamed to the member name the
/// codec binds, or `None` when it carries none.
///
/// Only the element NAME changes: its attributes, its children and their text are written
/// back exactly as they arrived.
fn published_names_as_members(document: &[u8]) -> Option<Vec<u8>> {
    let mut reader = Reader::from_reader(document);
    reader.config_mut().trim_text(false);
    let mut writer = Writer::new(Vec::new());
    let mut renamed = false;
    loop {
        match reader.read_event() {
            Ok(Event::Eof) => break,
            Ok(Event::Start(start)) => match union_member_by_published_name(start.name().as_ref()) {
                Some(member) => {
                    renamed = true;
                    writer
                        .write_event(Event::Start(renamed_start(&start, member.member)))
                        .ok()?;
                }
                None => {
                    writer.write_event(Event::Start(start)).ok()?;
                }
            },
            Ok(Event::Empty(empty)) => match union_member_by_published_name(empty.name().as_ref()) {
                Some(member) => {
                    renamed = true;
                    writer
                        .write_event(Event::Empty(renamed_start(&empty, member.member)))
                        .ok()?;
                }
                None => {
                    writer.write_event(Event::Empty(empty)).ok()?;
                }
            },
            Ok(Event::End(end)) => match union_member_by_published_name(end.name().as_ref()) {
                Some(member) => {
                    renamed = true;
                    writer
                        .write_event(Event::End(BytesEnd::new(member.member)))
                        .ok()?;
                }
                None => {
                    writer.write_event(Event::End(end)).ok()?;
                }
            },
            Ok(other) => {
                writer.write_event(other).ok()?;
            }
            // A document the XML reader cannot walk is left for the framework's own
            // parser to refuse, so the refusal a client sees is the framework's.
            Err(_) => return None,
        }
    }
    renamed.then(|| writer.into_inner())
}

/// Every ELEMENT a request document of this service carries an OBJECT KEY in, under the name
/// restXml writes it with.
///
/// An object key is bytes the CALLER chose and S3 addresses the object by exactly — "an object
/// that's composed entirely of whitespace characters" is a key `DeleteObjects` names in its own
/// documentation — so the whitespace at either end of such an element's text is part of the
/// value rather than layout around it. The framework's codec reads an element's text TRIMMED,
/// which is right for every value whose own lexical space ignores surrounding whitespace (a
/// boolean, a number, a timestamp) and wrong for a key: a key of spaces arrives as the empty
/// string, which the model's `length {min 1}` then refuses, and a key with a leading space
/// arrives as another key altogether. [`verbatim_text_folded`] is what keeps it.
///
/// ONE element name carries the two members the model targets at `ObjectKey`:
/// `ObjectIdentifier$Key`, the key a batch delete names, and `Tag$Key`, which that shape
/// documents as "name of the object key"
/// (`smithy_query(query="shape",shape="ObjectIdentifier,Tag")`). Both are kept, and for the
/// same reason: whitespace is a character a tag key may carry too (the published tag
/// restrictions admit "letters, whitespace, and numbers representable in UTF-8"), so a tag key
/// stated with it is a tag key with it.
///
/// Source: the members of this service's request documents whose target is the `ObjectKey`
/// shape, re-derived from the model by `tests/all/zzz_batch_probe.rs`.
pub const XML_OBJECT_KEY_DOCUMENT_ELEMENTS: &[&str] = &["Key"];

/// The XML character reference for each whitespace character an element's text may begin or end
/// with — the form that states that character where the codec's trim cannot take it away, and
/// which the codec then decodes back to the character itself.
///
/// XML 1.0's own whitespace (§2.3 `S`): space, tab, carriage return and line feed.
const XML_WHITESPACE_REFERENCES: [(char, &str); 4] = [
    (' ', "&#32;"),
    ('\t', "&#9;"),
    ('\r', "&#13;"),
    ('\n', "&#10;"),
];

/// `document` with the leading and trailing whitespace of every object-key element re-spelled
/// as the character references that state it ([`XML_WHITESPACE_REFERENCES`]), or `None` when no
/// such element's text has any (in which case nothing is rewritten and the original bytes are
/// used).
///
/// Only the whitespace AT THE ENDS is re-spelled, and only inside an element of
/// [`XML_OBJECT_KEY_DOCUMENT_ELEMENTS`]: the text between those ends is written back exactly as
/// it arrived — a reference the client itself sent included, because the text is handed on
/// pre-escaped and so is decoded exactly once, the way
/// [`attributes_as_elements`] hands on an attribute's value.
fn verbatim_text_folded(document: &[u8]) -> Option<Vec<u8>> {
    fold_element_text(
        document,
        XML_OBJECT_KEY_DOCUMENT_ELEMENTS,
        &end_whitespace_as_references,
    )
}

/// `raw` with the whitespace it begins and ends with written as character references, or `None`
/// when it begins and ends with none (and so states itself already), and for text the ends of
/// which are not the whole of it and whose middle is not UTF-8 — which no re-spelling can
/// state, and which the framework's own parser is left to refuse.
///
/// Read as BYTES: every character folded is ASCII ([`XML_WHITESPACE_REFERENCES`]), so the ends
/// are found without decoding, and the middle travels through unchanged.
fn end_whitespace_as_references(raw: &[u8]) -> Option<String> {
    let is_xml_whitespace =
        |byte: u8| XML_WHITESPACE_REFERENCES.iter().any(|(ws, _)| *ws as u32 == u32::from(byte));
    let reference = |byte: u8| {
        XML_WHITESPACE_REFERENCES
            .iter()
            .find(|(ws, _)| *ws as u32 == u32::from(byte))
            .map(|(_, reference)| *reference)
            .unwrap_or_default()
    };
    let leading = raw.iter().take_while(|byte| is_xml_whitespace(**byte)).count();
    let trailing = raw[leading..]
        .iter()
        .rev()
        .take_while(|byte| is_xml_whitespace(**byte))
        .count();
    if leading == 0 && trailing == 0 {
        return None;
    }
    // The middle is what the ends leave: for text that is whitespace all the way through there
    // is none, and every character of it is stated once.
    let middle = ::std::str::from_utf8(&raw[leading..raw.len() - trailing]).ok()?;
    let mut stated: String = raw[..leading].iter().map(|byte| reference(*byte)).collect();
    stated.push_str(middle);
    stated.extend(raw[raw.len() - trailing..].iter().map(|byte| reference(*byte)));
    Some(stated)
}

/// `document` with the TEXT of every element named in `catalog` replaced by what `folded`
/// states it as, or `None` when `folded` states none of them differently (in which case nothing
/// is rewritten and the original bytes are used).
///
/// The ONE walk both text folds of this module are written over, so the two cannot drift apart
/// in how they read a document: an element is matched by NAME, which is what the codec matches
/// it by too, its text is handed to `folded` as the BYTES it arrived as, and what comes back is
/// written as pre-escaped character data — so a value is decoded exactly once, the way
/// [`attributes_as_elements`] hands on an attribute's. Everything else — every other element,
/// every attribute, every comment — is written back exactly as it arrived.
fn fold_element_text(
    document: &[u8],
    catalog: &[&str],
    folded: &dyn Fn(&[u8]) -> Option<String>,
) -> Option<Vec<u8>> {
    let mut reader = Reader::from_reader(document);
    reader.config_mut().trim_text(false);
    let mut writer = Writer::new(Vec::new());
    let mut changed = false;
    // Whether the element whose text is being read is one of the catalog's.
    let mut in_catalog = false;
    loop {
        match reader.read_event() {
            Ok(Event::Eof) => break,
            Ok(Event::Start(start)) => {
                in_catalog = catalog
                    .iter()
                    .any(|element| element.as_bytes() == start.name().as_ref());
                writer.write_event(Event::Start(start)).ok()?;
            }
            Ok(Event::Text(text)) if in_catalog => match folded(text.as_ref()) {
                Some(stated) => {
                    changed = true;
                    writer
                        .write_event(Event::Text(BytesText::from_escaped(stated)))
                        .ok()?;
                }
                None => {
                    writer.write_event(Event::Text(text)).ok()?;
                }
            },
            Ok(Event::End(end)) => {
                in_catalog = false;
                writer.write_event(Event::End(end)).ok()?;
            }
            Ok(other) => {
                writer.write_event(other).ok()?;
            }
            // A document the XML reader cannot walk is left for the framework's own
            // parser to refuse, so the refusal a client sees is the framework's.
            Err(_) => return None,
        }
    }
    changed.then(|| writer.into_inner())
}

/// `start` under the element name `name`, carrying its attributes verbatim —
/// `push_attribute` copies an `Attribute`'s raw value unchanged, so nothing is re-escaped.
fn renamed_start(start: &BytesStart<'_>, name: &'static str) -> BytesStart<'static> {
    let mut renamed = BytesStart::new(name).into_owned();
    for attribute in start.attributes().flatten() {
        renamed.push_attribute(attribute);
    }
    renamed
}

/// The two values an XML boolean may be spelled with that the framework's codec does not read,
/// and the value each of them means.
///
/// A request document is validated against the schema S3 publishes for it, where a boolean
/// element is an `xsd:boolean` — whose lexical space is `true`, `false`, `1` and `0` (W3C XML
/// Schema Part 2, §3.2.2), the last two being the numeric spellings of the first two. The
/// codec reads the words alone, so a document spelling a boolean the numeric way is refused
/// where the published schema admits it; [`xsd_boolean_spellings_folded`] re-spells those two
/// into the words before the codec reads the document, which leaves the value the client
/// stated unchanged. Any other text is in neither spelling and is left exactly as it arrived,
/// for the codec to refuse.
const XSD_BOOLEAN_SPELLINGS: [(&str, &str); 2] = [("1", "true"), ("0", "false")];

/// Every ELEMENT a request document of this service binds a BOOLEAN member from, under the
/// name restXml writes it with (the member's `@xmlName`, else its own name).
///
/// The fold below is keyed on the element name, so this is the catalog that says which
/// elements it may touch: an element naming anything else keeps its text exactly as the
/// client sent it, whatever that text is.
///
/// Source: the members of this service's request documents whose target is a `boolean` shape,
/// re-derived from the model by `tests/all/zzz_batch_probe.rs` so a model update that adds one
/// cannot quietly keep being refused.
pub const XML_BOOLEAN_DOCUMENT_ELEMENTS: &[&str] = &[
    "AllowQuotedRecordDelimiter",
    "BlockPublicAcls",
    "BlockPublicPolicy",
    "BucketKeyEnabled",
    "ExpiredObjectDeleteMarker",
    "IgnorePublicAcls",
    "IsEnabled",
    "Quiet",
    "RestrictPublicBuckets",
];

/// `document` with every boolean element spelled `1` or `0` re-spelled as `true` or `false`,
/// or `None` when it spells none that way (in which case nothing is rewritten and the original
/// bytes are used).
///
/// Only the TEXT of an element the model declares a boolean changes
/// ([`XML_BOOLEAN_DOCUMENT_ELEMENTS`]), and only when that text is one of the two numeric
/// spellings: every other element, every attribute and every other value is written back
/// exactly as it arrived.
fn xsd_boolean_spellings_folded(document: &[u8]) -> Option<Vec<u8>> {
    fold_element_text(document, XML_BOOLEAN_DOCUMENT_ELEMENTS, &|raw| {
        let spelled = ::std::str::from_utf8(raw).ok()?;
        XSD_BOOLEAN_SPELLINGS
            .iter()
            .find(|(numeric, _)| *numeric == spelled.trim())
            .map(|(_, word)| (*word).to_string())
    })
}

/// `document` with every `@xmlAttribute` member moved out of its own element and onto its
/// parent's start tag, with the namespace its prefix needs declared there, or `None` when
/// it carries none.
fn elements_as_attributes(document: &[u8]) -> Option<Vec<u8>> {
    let mut reader = Reader::from_reader(document);
    reader.config_mut().trim_text(false);

    // The document's events, with the attribute members' own elements left out, and for
    // each event that opened an element the members that belong on its start tag.
    //
    // Keyed BY THAT EVENT's index rather than held as one flat list, so the write pass
    // below is one lookup per start tag: a document carrying many promoted members (an
    // ACL's grants, a logging status's target grants) costs time linear in its events
    // rather than in events times members.
    let mut events: Vec<Event<'static>> = Vec::new();
    let mut open: Vec<usize> = Vec::new();
    let mut promoted: BTreeMap<usize, Vec<(&'static XmlAttributeMember, String)>> =
        BTreeMap::new();
    let mut reading: Option<(usize, &'static XmlAttributeMember, String)> = None;
    loop {
        match reader.read_event() {
            Ok(Event::Eof) => break,
            Ok(Event::Start(start)) => {
                match (
                    open.last().copied(),
                    member_by_element(start.name().as_ref()),
                ) {
                    (Some(parent), Some(member)) => {
                        reading = Some((parent, member, String::new()));
                    }
                    _ => {
                        open.push(events.len());
                        events.push(Event::Start(start.into_owned()));
                    }
                }
            }
            Ok(Event::Empty(empty)) => {
                match (
                    open.last().copied(),
                    member_by_element(empty.name().as_ref()),
                ) {
                    // An empty element is the empty value of its member.
                    (Some(parent), Some(member)) => promoted
                        .entry(parent)
                        .or_default()
                        .push((member, String::new())),
                    _ => events.push(Event::Empty(empty.into_owned())),
                }
            }
            Ok(Event::Text(text)) => match reading.as_mut() {
                Some((_, _, value)) => {
                    value.push_str(&text.xml10_content().unwrap_or_default());
                }
                None => events.push(Event::Text(text.into_owned())),
            },
            Ok(Event::End(end)) => match reading.take() {
                Some((parent, member, value)) => {
                    promoted.entry(parent).or_default().push((member, value));
                }
                None => {
                    open.pop();
                    events.push(Event::End(end.into_owned()));
                }
            },
            Ok(other) => events.push(other.into_owned()),
            // A document this reader cannot walk is answered as the framework wrote it,
            // rather than being dropped.
            Err(_) => return None,
        }
    }
    if promoted.is_empty() {
        return None;
    }

    let mut writer = Writer::new(Vec::new());
    for (index, event) in events.into_iter().enumerate() {
        match event {
            Event::Start(mut start) => {
                for (member, value) in promoted.get(&index).into_iter().flatten() {
                    if let Some((prefix, _)) = member.attribute.split_once(':') {
                        let declaration = format!("xmlns:{prefix}");
                        start.push_attribute((declaration.as_str(), member.namespace));
                    }
                    start.push_attribute((member.attribute, value.as_str()));
                }
                writer.write_event(Event::Start(start)).ok()?;
            }
            other => writer.write_event(other).ok()?,
        }
    }
    Some(writer.into_inner())
}

/// The path component of a request URI, without the scheme, the authority or the query.
///
/// `pub(super)`, with [`request_query`] below it: [`super`]'s routing seam reads the same
/// two components off a request URI, and one reader for both keeps the request this
/// service ROUTES and the request it DESERIALIZES from disagreeing about which path and
/// which query string arrived.
pub(super) fn request_path(uri: &str) -> &str {
    let after_authority = match uri.find("://") {
        None => uri,
        Some(scheme_end) => match uri[scheme_end + 3..].find('/') {
            None => "/",
            Some(path_start) => &uri[scheme_end + 3 + path_start..],
        },
    };
    after_authority
        .split('?')
        .next()
        .unwrap_or(after_authority)
}

/// The query component of a request URI (everything after the first `?`), or the empty
/// string when it has none.
pub(super) fn request_query(uri: &str) -> &str {
    match uri.find('?') {
        Some(mark) => &uri[mark + 1..],
        None => "",
    }
}

/// The request headers whose VALUE S3 reads as a MODE NAME rather than as a case-sensitive
/// identifier, each with the spelling the model gives that mode.
///
/// `x-amz-checksum-mode` is the one such header this service binds: it asks for the checksum
/// the object was stored with ("to retrieve the checksum, this parameter must be enabled",
/// `ChecksumMode`'s own documentation) and its target enum has exactly ONE permitted value, so
/// a case-insensitive match cannot be ambiguous. S3 serves the header however a client spells
/// it — the AWS CLI's own `s3 cp` download path sends `enabled` and AWS answers 200 with the
/// object's checksum, where refusing it answers `InvalidArgument` for a request AWS accepts.
/// No other value is accepted: a mode the model does not list is still refused, because only
/// the CASE is folded here.
///
/// Source: the permitted spellings are the GENERATED value set the model's own enum produced
/// ([`crate::ops::types::CHECKSUM_MODE_VALUES`], `["ENABLED"]`), so a value the enum gains
/// cannot drift from this fold; the header name is the `@httpHeader` the three operations that
/// bind the member carry (`GetObject`, `HeadObject`, `GetObjectAnnotation`), and the fold below
/// is applied only where the operation's own input binds it.
const MODE_HEADER_VALUES: &[(&str, &[&str])] = &[(
    "x-amz-checksum-mode",
    crate::ops::types::CHECKSUM_MODE_VALUES,
)];

/// `request`'s headers with every [`MODE_HEADER_VALUES`] value the request spells in another
/// case folded to the model's spelling — or `None` when there is nothing to fold, which is
/// every request that spells them as the model does and every operation whose input binds no
/// such header.
///
/// Driven off `input_schema`: the fold applies to a header the operation being served really
/// binds, so a header of that name on an operation that binds no such member is left exactly
/// as it arrived.
fn mode_headers_folded(request: &Request, input_schema: &Schema) -> Option<Vec<(String, String)>> {
    let bound = |name: &str| {
        input_schema.members().iter().any(|member| {
            member
                .http_header()
                .is_some_and(|header| header.value().eq_ignore_ascii_case(name))
        })
    };
    let folded = |name: &str, value: &str| {
        MODE_HEADER_VALUES
            .iter()
            .filter(|(header, _)| header.eq_ignore_ascii_case(name))
            .flat_map(|(_, modelled)| modelled.iter())
            .find(|modelled| **modelled != value && modelled.eq_ignore_ascii_case(value))
            .copied()
    };
    let mut headers = request_headers(request);
    let mut changed = false;
    for (name, value) in &mut headers {
        if !bound(name) {
            continue;
        }
        if let Some(modelled) = folded(name, value) {
            *value = modelled.to_string();
            changed = true;
        }
    }
    changed.then_some(headers)
}

/// The INPUT member `input_schema` binds to the request header `name`, or `None` for a
/// header this operation binds nothing from.
///
/// One lookup for every header rule below, so each is applied to a header the operation
/// being served really binds — a header of that name on an operation that binds no such
/// member is left exactly as it arrived.
fn header_member<'s>(input_schema: &'s Schema, name: &str) -> Option<&'s Schema> {
    input_schema
        .members()
        .iter()
        .find(|member| {
            member
                .http_header()
                .is_some_and(|header| header.value().eq_ignore_ascii_case(name))
        })
        .copied()
}

/// The value set `member` publishes, for a member the model constrains to one.
///
/// Read off the member's own trait map the way the validating walk reads it, so the set this
/// module judges a header by is the very set the boundary would refuse it against — there is
/// no second copy of it here.
fn permitted_values(member: &Schema) -> Option<&'static [&'static str]> {
    let published = member.traits()?.get(&EnumTrait::TRAIT_ID)?;
    Some(published.as_any().downcast_ref::<EnumTrait>()?.values)
}

/// The TIMESTAMP format the framework's binding deserializer reads a value bound to a
/// request header with: the one the member's own `@timestampFormat` names, and `date-time`
/// for a member that names none.
///
/// Mirrors that reader exactly, because the rule below is about the values it would REFUSE:
/// what this returns is the format the value has to be in by the time the codec sees it.
fn bound_timestamp_format(member: &Schema) -> ::aws_smithy_types::date_time::Format {
    match member.timestamp_format().map(|format| format.format()) {
        Some(::aws_smithy_schema::traits::TimestampFormat::EpochSeconds) => {
            ::aws_smithy_types::date_time::Format::EpochSeconds
        }
        Some(::aws_smithy_schema::traits::TimestampFormat::HttpDate) => {
            ::aws_smithy_types::date_time::Format::HttpDate
        }
        Some(::aws_smithy_schema::traits::TimestampFormat::DateTime) | None => {
            ::aws_smithy_types::date_time::Format::DateTime
        }
    }
}

/// `value` re-spelled in `format`, for a request header that carries an HTTP-date — or
/// `None` when it is already a value that format reads, and when it is no HTTP-date either.
///
/// "An HTTP-date value represents time as an instance of Coordinated Universal Time (UTC)"
/// and is the `IMF-fixdate` of RFC 9110, which is the format Smithy's HTTP bindings give a
/// `timestamp` bound to a HEADER: every conditional header an SDK sends
/// (`If-Unmodified-Since` and the `x-amz-copy-source-if-*` dates among them) is spelled that
/// way, and the model's own members carry no `@timestampFormat` to say otherwise. Read
/// through the ISO 8601 `date-time` parser instead, such a header is not a date at all and
/// the whole request is refused for it — so a conditional read can never be evaluated.
///
/// The instant is what is preserved: the HTTP date is parsed and written back in the format
/// the codec reads, which is the same instant to the second the header carries. A value that
/// format already reads is left alone, so a member whose own format IS `http-date`, and a
/// client that sends the format the member declares, are untouched.
fn as_bound_timestamp(value: &str, format: ::aws_smithy_types::date_time::Format) -> Option<String> {
    if DateTime::from_str(value, format).is_ok() {
        return None;
    }
    let instant =
        DateTime::from_str(value, ::aws_smithy_types::date_time::Format::HttpDate).ok()?;
    instant.fmt(format).ok()
}

/// The request headers whose value the OPERATION judges rather than the boundary, each
/// WITHHELD from the codec when it is outside the value set the model publishes for it.
///
/// `x-amz-acl` and `x-amz-object-ownership` are the two. A value outside the published set
/// is refused by S3 with its own `InvalidArgument` envelope — the `<ArgumentName>` /
/// `<ArgumentValue>` pair naming the header and the value as sent, beside a sentence that is
/// empty for the one ([`crate::scaffolded::Scaffolded::invalid_canned_acl`]) and names the
/// header for the other ([`crate::state::invalid_object_ownership_message`]) — and an
/// envelope with members of its own is something only a handler can answer: the boundary's
/// refusal carries the constraint clause the validating walk wrote, which names the Smithy
/// member path and the model's value list instead.
///
/// So the value is kept from the codec, exactly as a website document the owning operation
/// is the one to refuse is ([`crate::state::website_document_rejection`]), and every
/// operation that binds the header answers the refusal itself from the request's own
/// headers ([`rejected_canned_acl`], [`rejected_object_ownership`]). The set is still
/// enforced, in that one place: nothing outside it is ever stored.
const HEADERS_JUDGED_BY_THE_HANDLER: [&str; 2] = [
    crate::state::CANNED_ACL_ARGUMENT,
    crate::state::OBJECT_OWNERSHIP_ARGUMENT,
];

/// The `x-amz-acl` value `parts` carried that is outside the set `input_schema`'s own `ACL`
/// member publishes, or `None` for a request carrying a published value or no such header at
/// all.
///
/// The judgement the boundary would have made, made where the refusal can carry S3's own
/// envelope — see [`HEADERS_JUDGED_BY_THE_HANDLER`], which is what withholds the value from
/// the codec, and note that the set comes from the operation's OWN member, so a bucket's
/// canned ACLs and an object's are each judged against their own published values.
pub(crate) fn rejected_canned_acl<'p>(
    parts: &'p ::http::request::Parts,
    input_schema: &Schema,
) -> Option<&'p str> {
    let value = parts
        .headers
        .get(crate::state::CANNED_ACL_ARGUMENT)
        .and_then(|value| value.to_str().ok())?;
    let permitted = permitted_values(header_member(
        input_schema,
        crate::state::CANNED_ACL_ARGUMENT,
    )?)?;
    (!permitted.contains(&value)).then_some(value)
}

/// The `x-amz-object-ownership` value `parts` carried that is outside the set
/// `input_schema`'s own `ObjectOwnership` member publishes, or `None` for a request carrying
/// a published value or no such header at all.
///
/// The judgement the boundary would have made, made where the refusal can carry S3's own
/// envelope — see [`HEADERS_JUDGED_BY_THE_HANDLER`], which is what withholds the value from
/// the codec, and [`rejected_canned_acl`], the same reading for the other header judged that
/// way. The set comes from the operation's OWN member, so nothing outside it is ever stored.
pub(crate) fn rejected_object_ownership<'p>(
    parts: &'p ::http::request::Parts,
    input_schema: &Schema,
) -> Option<&'p str> {
    let value = parts
        .headers
        .get(crate::state::OBJECT_OWNERSHIP_ARGUMENT)
        .and_then(|value| value.to_str().ok())?;
    let permitted = permitted_values(header_member(
        input_schema,
        crate::state::OBJECT_OWNERSHIP_ARGUMENT,
    )?)?;
    (!permitted.contains(&value)).then_some(value)
}

/// The `max-buckets` value `parts` carried that bounds no page of buckets, with the fault it
/// carries — or `None` for a request bounding one, and for an operation that binds no such
/// parameter.
///
/// The judgement the boundary would have made, made where the refusal can carry S3's own
/// envelope: the `<ArgumentName>` / `<ArgumentValue>` pair naming the parameter and the value
/// as sent, which only a handler's envelope can carry. A value that is no INTEGER the
/// boundary cannot word at all — its codec reports a parse failure, in a sentence about Rust
/// types — so such a value is withheld from the codec
/// ([`without_withheld_max_buckets`]) and the operation answers both faults, through the one
/// reading of them ([`crate::state::max_buckets_fault`]).
pub(crate) fn rejected_max_buckets(
    parts: &::http::request::Parts,
    input_schema: &Schema,
) -> Option<(String, crate::state::MaxBucketsFault)> {
    rejected_max_buckets_in(parts.uri.query().unwrap_or_default(), input_schema)
}

/// [`rejected_max_buckets`] over a request's QUERY STRING, which is what the protocol reads
/// it from on the way in (where a request is a runtime-API one rather than its parts).
fn rejected_max_buckets_in(
    query: &str,
    input_schema: &Schema,
) -> Option<(String, crate::state::MaxBucketsFault)> {
    query_member(input_schema, crate::state::MAX_BUCKETS_ARGUMENT)?;
    let value = super::sigv4::query_parameter(query, crate::state::MAX_BUCKETS_ARGUMENT)?;
    crate::state::max_buckets_fault(&value).map(|fault| (value, fault))
}

/// The INPUT member `input_schema` binds to the request query parameter `name`, or `None` for
/// a parameter this operation binds nothing from — the query half of [`header_member`].
fn query_member<'s>(input_schema: &'s Schema, name: &str) -> Option<&'s Schema> {
    input_schema
        .members()
        .iter()
        .find(|member| {
            member
                .http_query()
                .is_some_and(|query| query.value() == name)
        })
        .copied()
}

/// `query` with the `max-buckets` parameter removed when its value is one the codec must not
/// be handed ([`crate::state::withholds_max_buckets`]) — a value bounding no page, which the
/// operation refuses itself ([`rejected_max_buckets`]), and a parameter stated with no value,
/// which states no bound at all. `None` when the query carries nothing to withhold, which
/// leaves it exactly as it arrived.
fn without_withheld_max_buckets(query: &str, input_schema: &Schema) -> Option<String> {
    query_member(input_schema, crate::state::MAX_BUCKETS_ARGUMENT)?;
    let value = super::sigv4::query_parameter(query, crate::state::MAX_BUCKETS_ARGUMENT)?;
    if !crate::state::withholds_max_buckets(&value) {
        return None;
    }
    let kept: Vec<&str> = query
        .split('&')
        .filter(|parameter| {
            parameter.split('=').next().unwrap_or(parameter) != crate::state::MAX_BUCKETS_ARGUMENT
        })
        .collect();
    Some(kept.join("&"))
}

/// The member a create's configuration names the Region the bucket is to be created in, as
/// the model declares it and as restXml spells its element
/// (`smithy_query(query="shape",shape="CreateBucketConfiguration")` reports
/// `LocationConstraint`).
const LOCATION_CONSTRAINT_MEMBER: &str = "LocationConstraint";

/// The `<LocationConstraint>` the request's own document spelled that is outside the set the
/// model publishes for it, or `None` for a request that spelled a published value, none at
/// all, or no document.
///
/// The judgement the boundary would have made, made where the refusal can carry S3's own
/// envelope — the same seam the canned ACL is judged through
/// ([`HEADERS_JUDGED_BY_THE_HANDLER`] and [`rejected_canned_acl`]): the refusal of a value
/// that names no location names the VALUE in a member of its own, which only a handler's
/// envelope can carry, and the value is withheld from the codec
/// ([`without_rejected_location_constraint`]) so the boundary does not refuse the document
/// first. The set is the GENERATED one the model's own enum produced
/// ([`crate::ops::types::BUCKET_LOCATION_CONSTRAINT_VALUES`]), the same set the validating
/// walk judges the member by, so the two cannot drift and nothing outside it is ever stored.
///
/// `input_schema` is the operation's own, so the judgement is made only for an operation
/// whose document declares that member — every other operation's request is untouched.
pub(crate) fn rejected_location_constraint(input_schema: &Schema) -> Option<String> {
    if !carries_xml_document_payload(input_schema)
        || !input_schema
            .members()
            .iter()
            .any(|member| member.member_name() == Some(CREATE_BUCKET_CONFIGURATION_MEMBER))
    {
        return None;
    }
    let spelled = spelled_in_request(LOCATION_CONSTRAINT_MEMBER)?;
    (!crate::ops::types::BUCKET_LOCATION_CONSTRAINT_VALUES.contains(&spelled.as_str()))
        .then_some(spelled)
}

/// The input member `CreateBucket` binds its configuration document as
/// (`smithy_query(query="operation",operation="CreateBucket")` reports
/// `CreateBucketConfiguration` as the `@httpPayload`) — the one document that carries a
/// location constraint.
const CREATE_BUCKET_CONFIGURATION_MEMBER: &str = "CreateBucketConfiguration";

/// The document the codec is handed in place of a create configuration naming a location S3
/// cannot create a bucket in: the container with no member in it, exactly as a website
/// document the owning operation refuses is replaced by [`EMPTY_WEBSITE_CONFIGURATION`].
///
/// Nothing is read from it: `CreateBucket` refuses such a request from the text the document
/// actually carried ([`spelled_in_request`]) before it reads the configuration at all, so the
/// stand-in only has to be a document the codec can build — and replacing the whole container
/// rather than cutting one element out of it keeps the one reading of what the document SPELLS
/// (the module's XML reader, through [`rejected_location_constraint`]) the only reading there
/// is.
const EMPTY_CREATE_BUCKET_CONFIGURATION: &[u8] =
    b"<CreateBucketConfiguration></CreateBucketConfiguration>";

/// The request headers a CONDITION arrives in as a date, and whose value a recipient is to
/// IGNORE when it is not one.
///
/// "A recipient MUST ignore the If-Unmodified-Since header field if the received field value
/// is not a valid HTTP-date" (RFC 9110, *If-Unmodified-Since*, and the same sentence for
/// *If-Modified-Since*): a conditional date a server cannot read states no condition, so the
/// request is served as if it carried none — refusing the whole request for it would make a
/// header a client may send unparseable text in fatal, which HTTP says it is not.
///
/// The four are the conditional DATE members of this service's object reads and copies, as
/// the model binds them (`smithy_query(query="operation",operation="GetObject,CopyObject")`
/// reports `IfModifiedSince: {httpHeader: "If-Modified-Since"}`,
/// `IfUnmodifiedSince: {httpHeader: "If-Unmodified-Since"}` and the two
/// `x-amz-copy-source-if-*-since` headers, which are S3's prefixed spellings of the same two
/// HTTP conditions). A timestamp header that is NOT a condition — the Object Lock
/// retain-until date — is not here: it is a value the write stores rather than a condition it
/// is judged by, so a value that is no date is refused rather than dropped.
const CONDITIONAL_DATE_HEADERS: [&str; 4] = [
    "If-Modified-Since",
    "If-Unmodified-Since",
    "x-amz-copy-source-if-modified-since",
    "x-amz-copy-source-if-unmodified-since",
];

/// Whether `name` is one of [`CONDITIONAL_DATE_HEADERS`].
fn states_a_date_condition(name: &str) -> bool {
    CONDITIONAL_DATE_HEADERS
        .iter()
        .any(|header| header.eq_ignore_ascii_case(name))
}

/// `request`'s headers as the codec behind this protocol is given them, or `None` when
/// nothing about them changes — which is every request that needs none of the rules below.
///
/// Each rule is driven off `input_schema`, so it applies only to a header the operation being
/// served really binds, and each is about a value the framework's own codec would otherwise
/// read as something the request did not say:
///
/// * a MODE value spelled in another case is folded to the model's spelling
///   ([`mode_headers_folded`]);
/// * an `x-amz-copy-source` is put into the form the member's pattern is applied to, its
///   percent escapes decoded ([`crate::state::canonical_copy_source`]);
/// * a timestamp bound to a header is re-spelled from the HTTP-date a client sends into the
///   format the codec reads ([`as_bound_timestamp`]);
/// * a CONDITION whose date is no date at all is dropped, the way HTTP says a recipient
///   treats one ([`CONDITIONAL_DATE_HEADERS`]);
/// * a value the OPERATION judges is withheld ([`HEADERS_JUDGED_BY_THE_HANDLER`]).
///
/// The rules are applied over ONE pass of the headers, in place, in that order, so a request
/// that needs several is served and one that needs none allocates nothing beyond the pairs
/// [`mode_headers_folded`] already reads.
fn normalized_headers(request: &Request, input_schema: &Schema) -> Option<Vec<(String, String)>> {
    let folded = mode_headers_folded(request, input_schema);
    let mut changed = folded.is_some();
    let mut headers = folded.unwrap_or_else(|| request_headers(request));
    headers.retain_mut(|(name, value)| {
        let Some(member) = header_member(input_schema, name) else {
            return true;
        };
        if name.eq_ignore_ascii_case(crate::state::COPY_SOURCE_ARGUMENT) {
            if let Some(canonical) = crate::state::canonical_copy_source(value) {
                if canonical != *value {
                    *value = canonical;
                    changed = true;
                }
            }
        } else if matches!(member.shape_type(), ShapeType::Timestamp) {
            let format = bound_timestamp_format(member);
            match as_bound_timestamp(value, format) {
                Some(respelled) => {
                    *value = respelled;
                    changed = true;
                }
                // Neither the format the codec reads nor an HTTP-date: a condition stated in
                // a value that is no date states no condition at all, so the header is
                // dropped.
                None if DateTime::from_str(value, format).is_err()
                    && states_a_date_condition(name) =>
                {
                    changed = true;
                    return false;
                }
                None => {}
            }
        } else if HEADERS_JUDGED_BY_THE_HANDLER
            .iter()
            .any(|judged| judged.eq_ignore_ascii_case(name))
            && permitted_values(member).is_some_and(|permitted| !permitted.contains(&value.as_str()))
        {
            changed = true;
            return false;
        }
        true
    });
    changed.then_some(headers)
}


/// A request's headers as `(name, value)` pairs, original case preserved and every value
/// of a repeated header kept, which is what the binding deserializer needs to bind a
/// list-typed or prefixed header.
fn request_headers(request: &Request) -> Vec<(String, String)> {
    let headers = request.headers();
    let names: BTreeSet<&str> = headers.iter().map(|(name, _)| name).collect();
    let mut pairs: Vec<(String, String)> = Vec::new();
    for name in names {
        for value in headers.get_all(name) {
            pairs.push((name.to_string(), value.to_string()));
        }
    }
    pairs
}

/// A body deserializer that OWNS its XML document, so a NORMALIZED body can be read
/// without borrowing from the request the original arrived in.
///
/// The framework's own body deserializer borrows the request's bytes, which a rewritten
/// document is not; this holds the rewritten document and builds the framework's reader
/// over it per read. Each read starts at the document's root, which is exactly how the
/// binding deserializer uses a body: once, for the payload member.
struct OwnedXmlBody {
    document: Vec<u8>,
}

/// Read one scalar from the owned document through the framework's own XML reader.
macro_rules! owned_xml_scalar {
    ($name:ident -> $value:ty) => {
        fn $name(&mut self, schema: &Schema) -> Result<$value, SerdeError> {
            XmlShapeDeserializer::new(&self.document).$name(schema)
        }
    };
}

impl ShapeDeserializer for OwnedXmlBody {
    fn read_struct(
        &mut self,
        schema: &Schema,
        consumer: &mut dyn FnMut(&Schema, &mut dyn ShapeDeserializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        XmlShapeDeserializer::new(&self.document).read_struct(schema, consumer)
    }

    fn read_list(
        &mut self,
        schema: &Schema,
        consumer: &mut dyn FnMut(&mut dyn ShapeDeserializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        XmlShapeDeserializer::new(&self.document).read_list(schema, consumer)
    }

    fn read_map(
        &mut self,
        schema: &Schema,
        consumer: &mut dyn FnMut(String, &mut dyn ShapeDeserializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        XmlShapeDeserializer::new(&self.document).read_map(schema, consumer)
    }

    owned_xml_scalar!(read_boolean -> bool);
    owned_xml_scalar!(read_byte -> i8);
    owned_xml_scalar!(read_short -> i16);
    owned_xml_scalar!(read_integer -> i32);
    owned_xml_scalar!(read_long -> i64);
    owned_xml_scalar!(read_float -> f32);
    owned_xml_scalar!(read_double -> f64);
    owned_xml_scalar!(read_big_integer -> BigInteger);
    owned_xml_scalar!(read_big_decimal -> BigDecimal);
    owned_xml_scalar!(read_string -> String);
    owned_xml_scalar!(read_blob -> Blob);
    owned_xml_scalar!(read_timestamp -> DateTime);
    owned_xml_scalar!(read_document -> Document);

    fn is_null(&self) -> bool {
        XmlShapeDeserializer::new(&self.document).is_null()
    }

    fn container_size(&self) -> Option<usize> {
        XmlShapeDeserializer::new(&self.document).container_size()
    }
}

/// `response` with the status HTTP gives a PARTIAL answer, when it is one.
///
/// "The 206 (Partial Content) status code indicates that the server is successfully
/// fulfilling a range request for the target resource by transferring one or more parts of
/// the selected representation" and such a response "generates a `Content-Range` header
/// field describing what range of the selected representation is enclosed" (RFC 9110, HTTP
/// Semantics, §15.3.7). So the `Content-Range` a handler bound IS the signal: a successful
/// response that reports one carries part of the object rather than the whole of it, and
/// 200 would tell a client it had received everything.
///
/// The rule lives here, once, because an operation's success status is a single modelled
/// constant (`Operation::SUCCESS_STATUS`) and this service's model binds the member rather
/// than the status: two operations answer a `Content-Range` (`GetObject` and `HeadObject`),
/// and neither handler can state a second status of its own. Only a 200 is promoted — an
/// operation whose modelled success status is anything else is answering something this rule
/// is not about.
fn with_partial_content_status(response: Response) -> Response {
    if response.status().as_u16() != 200 || response.headers().get("content-range").is_none() {
        return response;
    }
    with_status(response, PARTIAL_CONTENT_STATUS)
}

/// The status a response carrying part of the selected representation is sent with — see
/// [`with_partial_content_status`].
const PARTIAL_CONTENT_STATUS: u16 = 206;

/// The operation whose success status depends on what the call DID, and the status it
/// answers for a retrieval it newly initiated.
///
/// `RestoreObject`'s own documentation states both: "a successful action returns either the
/// `200 OK` or `202 Accepted` status code … if the object is not previously restored, then
/// Amazon S3 returns `202 Accepted` in the response … if the object is previously restored,
/// Amazon S3 returns `200 OK` in the response." The model declares the one success status an
/// operation may have (`http {code: 200}`,
/// `smithy_query(query="operation",operation="RestoreObject")`), so the handler cannot state
/// the other one itself: it records what its call did ([`note_restore_initiated`]) and the
/// promotion happens here, the way a served byte range is promoted above.
const RESTORE_OBJECT_OPERATION: &str = "RestoreObject";
/// See [`RESTORE_OBJECT_OPERATION`].
const ACCEPTED_STATUS: u16 = 202;

thread_local! {
    /// Whether the restore the operation serving this thread's request took was one the
    /// addressed version had no restored copy for — the `202 Accepted` case.
    ///
    /// A request-scoped note, held for the same reason and under the same discipline as the
    /// request-correlation pair ([`crate::state::request_ids`]): the handler records it as
    /// the last thing it does and the request walk serializes the output in the same poll,
    /// the serializer TAKES it before anything that can fail, and every request this service
    /// deserializes clears whatever this thread may still hold. So the note a response reads
    /// is its own request's or none at all.
    static RESTORE_INITIATED: ::std::cell::Cell<bool> = const { ::std::cell::Cell::new(false) };
}

/// Record that the restore this request took had no restored copy before it, so the response
/// is the `202 Accepted` `RestoreObject` answers for a retrieval it initiated.
pub fn note_restore_initiated() {
    RESTORE_INITIATED.with(|slot| slot.set(true));
}

/// The note for the response now being serialized, TAKEN rather than read, so a note one
/// response left behind can never promote a later one.
fn restore_initiated() -> bool {
    RESTORE_INITIATED.with(|slot| slot.replace(false))
}

/// Drop whatever note this thread may still hold, called at the START of every request this
/// service deserializes — the same discipline
/// [`crate::state::request_ids::forget_any_unanswered`] holds the correlation pair under.
fn forget_any_unanswered_restore() {
    let _ = restore_initiated();
}

/// `response` with the status a retrieval this call INITIATED is answered with, when it is
/// one — see [`RESTORE_OBJECT_OPERATION`]. Only a 200 is promoted, and only for the one
/// operation whose documentation states the pair of statuses.
fn with_accepted_status(response: Response, operation_name: &str, initiated: bool) -> Response {
    if !initiated || operation_name != RESTORE_OBJECT_OPERATION {
        return response;
    }
    with_status(response, ACCEPTED_STATUS)
}

/// The header S3's answer about a key whose CURRENT version is a delete marker carries:
/// "`x-amz-delete-marker`: indicates whether the specified object is (true) or is not (false)
/// a delete marker" (`GetObjectOutput$DeleteMarker`, bound to that header), beside the
/// `x-amz-version-id` of the marker itself (`$VersionId`).
///
/// A read or a copy of such a key answers 404 — "if the current version is a delete marker,
/// Amazon S3 behaves as if the object was deleted" — and a read that names the marker's own
/// version answers 405; either way the headers are what say WHY the object is not there,
/// WHICH version to delete to bring it back and WHEN that version was written ("it has a key
/// name and version ID like any other object", Amazon S3 User Guide, *Working with delete
/// markers*, and when the marker was laid is what a listing reports as its `LastModified`),
/// since the error document carries no member for any of them. An operation that resolved the
/// address to a marker records it ([`note_addressed_delete_marker`]) and the headers are
/// written here, the way the status of an initiated restore is promoted here.
const DELETE_MARKER_HEADER: &str = "x-amz-delete-marker";
/// See [`DELETE_MARKER_HEADER`] — the value that header carries for such an answer.
const DELETE_MARKER_HEADER_VALUE: &str = "true";

::std::thread_local! {
    /// The version id and the modification instant of the delete marker the operation
    /// serving this thread's request resolved the addressed key to, or `None`.
    ///
    /// A request-scoped note, held under the same discipline as the restore note above: the
    /// handler records it as the last thing it does before refusing, the serializer TAKES it
    /// before anything that can fail, and every request this service deserializes clears
    /// whatever this thread may still hold.
    static ADDRESSED_DELETE_MARKER: ::std::cell::RefCell<Option<(String, String)>> =
        const { ::std::cell::RefCell::new(None) };
}

/// Record that the address this request names resolves to a DELETE MARKER, whose version is
/// `version_id` (`None` for the `null` version a bucket that was not versioning-enabled
/// stored) and which was written at `last_modified` — see [`DELETE_MARKER_HEADER`].
pub fn note_addressed_delete_marker(
    version_id: Option<&str>,
    last_modified: ::std::time::SystemTime,
) {
    let recorded = version_id
        .unwrap_or(crate::state::NULL_VERSION_ID)
        .to_string();
    let written = DateTime::from(last_modified)
        .fmt(aws_smithy_types::date_time::Format::HttpDate)
        .unwrap_or_default();
    ADDRESSED_DELETE_MARKER.with(|slot| slot.replace(Some((recorded, written))));
}

/// The note for the response now being serialized, TAKEN rather than read, so a note one
/// response left behind can never name a later one.
fn addressed_delete_marker() -> Option<(String, String)> {
    ADDRESSED_DELETE_MARKER.with(|slot| slot.take())
}

/// Drop whatever note this thread may still hold, called at the START of every request this
/// service deserializes.
fn forget_any_unanswered_delete_marker() {
    let _ = addressed_delete_marker();
}

/// `response` with the two headers S3's answer about a key whose current version is a delete
/// marker carries — see [`DELETE_MARKER_HEADER`] — for a response whose request resolved to
/// one, and unchanged for every other.
fn with_delete_marker_headers(
    mut response: Response,
    marker: Option<(String, String)>,
) -> Response {
    let Some((version_id, last_modified)) = marker else {
        return response;
    };
    // A value a header map refuses is dropped rather than turning a served response into a
    // transport failure; a version id and an HTTP date never are.
    let _ = response
        .headers_mut()
        .try_insert(DELETE_MARKER_HEADER, DELETE_MARKER_HEADER_VALUE);
    let _ = response
        .headers_mut()
        .try_insert(VERSION_ID_RESPONSE_HEADER, version_id);
    let _ = response
        .headers_mut()
        .try_insert(LAST_MODIFIED_HEADER, last_modified);
    response
}

/// The header a response naming the version it acted on in: `x-amz-version-id`, which every
/// object operation's output binds its `VersionId` to
/// (`smithy_query(query="operation",operation="GetObject")`).
const VERSION_ID_RESPONSE_HEADER: &str = "x-amz-version-id";

/// The status a read answers when the caller's cached copy is still current: 304, which
/// `GetObjectRequest$IfNoneMatch` and `$IfModifiedSince` both name ("otherwise, return a 304
/// Not Modified error").
///
/// Such a response carries NO document at all — "the server generating a 304 response MUST
/// NOT generate content" (RFC 9110 §15.4.5) — so this status is what tells [`serialize_error`]
/// to answer the bare status instead of an `<Error>` body, the way the errorless 400 is
/// answered. The two validator headers RFC 9110 requires with it ("MUST generate … ETag,
/// Last-Modified") come off the version the read resolved, which only the handler sees, so it
/// records them ([`note_not_modified`]) and they are written here — together with the
/// `VersionId` that version is addressed by, which the read's output binds to
/// [`VERSION_ID_RESPONSE_HEADER`] and which therefore names the representation this answer is
/// about just as it does on the 200.
pub const NOT_MODIFIED_STATUS: u16 = 304;

/// The header a 304 reports the entity tag of the current representation in.
const ETAG_HEADER: &str = "etag";
/// The header it reports that representation's modification date in, in HTTP's own date
/// format (`GetObjectOutput$LastModified` is bound to it).
const LAST_MODIFIED_HEADER: &str = "last-modified";

/// What a [`NOT_MODIFIED_STATUS`] answer reports about the representation the caller already
/// holds: the two validators of it, and the version it is held as.
#[derive(Debug, Clone)]
struct NotModifiedRepresentation {
    /// Its entity tag, as the object reports it.
    etag: String,
    /// When it was last modified, in HTTP's own date format.
    last_modified: String,
    /// The id of the version the read resolved, or `None` for an object that carries none —
    /// which is every object of a bucket that mints no version ids.
    version_id: Option<String>,
}

::std::thread_local! {
    /// What the version a read of this thread's request answered NOT MODIFIED for reports,
    /// or `None`.
    ///
    /// A request-scoped note, held under the same discipline as the delete-marker note: the
    /// handler records it as the last thing it does before refusing, the serializer TAKES it
    /// before anything that can fail, and every request this service deserializes clears
    /// whatever this thread may still hold.
    static NOT_MODIFIED_REPRESENTATION: ::std::cell::RefCell<Option<NotModifiedRepresentation>> =
        const { ::std::cell::RefCell::new(None) };
}

/// Record what the representation a read answers [`NOT_MODIFIED_STATUS`] for reports: its
/// entity tag as the object reports it, its modification instant, and the id of the version
/// the read resolved (`None` for an object carrying none).
pub fn note_not_modified(
    etag: &str,
    last_modified: ::std::time::SystemTime,
    version_id: Option<&str>,
) {
    let rendered = DateTime::from(last_modified)
        .fmt(aws_smithy_types::date_time::Format::HttpDate)
        .unwrap_or_default();
    let representation = NotModifiedRepresentation {
        etag: etag.to_string(),
        last_modified: rendered,
        version_id: version_id.map(str::to_string),
    };
    NOT_MODIFIED_REPRESENTATION.with(|slot| slot.replace(Some(representation)));
}

/// The note for the response now being serialized, TAKEN rather than read.
fn not_modified_representation() -> Option<NotModifiedRepresentation> {
    NOT_MODIFIED_REPRESENTATION.with(|slot| slot.take())
}

/// Drop whatever note this thread may still hold, called at the START of every request this
/// service deserializes.
fn forget_any_unanswered_representation() {
    let _ = not_modified_representation();
}

/// The header a 405 names the methods the addressed resource DOES allow in: "the origin
/// server MUST generate an `Allow` header field in a 405 response containing a list of the
/// target resource's currently supported methods" (RFC 9110 §15.5.6).
///
/// It is the only thing in such a response that says what a client may do instead — the
/// `MethodNotAllowed` document states the method that was refused and the kind of resource it
/// was refused against, not what would work. The set belongs to the RESOURCE, so the refusal
/// that names the resource type is what records it
/// ([`crate::scaffolded::Scaffolded::method_not_allowed`]) and it is written here, the way the
/// delete-marker headers are.
const ALLOW_HEADER: &str = "allow";

::std::thread_local! {
    /// The methods the resource this thread's request addressed allows, as the `Allow`
    /// header's comma-separated list, or `None`.
    static ALLOWED_METHODS: ::std::cell::RefCell<Option<String>> =
        const { ::std::cell::RefCell::new(None) };
}

/// Record the methods the addressed resource allows — see [`ALLOW_HEADER`].
pub fn note_allowed_methods(methods: &str) {
    ALLOWED_METHODS.with(|slot| slot.replace(Some(methods.to_string())));
}

/// The note for the response now being serialized, TAKEN rather than read.
fn allowed_methods() -> Option<String> {
    ALLOWED_METHODS.with(|slot| slot.take())
}

/// Drop whatever note this thread may still hold, called at the START of every request this
/// service deserializes.
fn forget_any_unanswered_allowed_methods() {
    let _ = allowed_methods();
}

/// `response` with the `Allow` header naming what the addressed resource permits, for a
/// refusal whose handler recorded a set, and unchanged for every other.
fn with_allow_header(mut response: Response, methods: Option<String>) -> Response {
    let Some(methods) = methods else {
        return response;
    };
    // A value a header map refuses is dropped rather than turning a served response into a
    // transport failure; a method list never is.
    let _ = response.headers_mut().try_insert(ALLOW_HEADER, methods);
    response
}

/// `response` re-sent with `status`, keeping every header the serializer wrote, or unchanged
/// for a status no HTTP response can carry (neither of this module's two ever is) or a
/// response whose status is not the 200 the promotion is from.
fn with_status(response: Response, status: u16) -> Response {
    if response.status().as_u16() != 200 {
        return response;
    }
    let Ok(promoted_status) = StatusCode::try_from(status) else {
        return response;
    };
    let headers: Vec<(String, String)> = response
        .headers()
        .iter()
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .collect();
    let mut promoted = Response::new(promoted_status, response.into_body());
    for (name, value) in headers {
        // `append`, not `insert`: a header name the serializer wrote twice keeps both of its
        // values, and a value the HTTP layer refuses is dropped rather than turning a served
        // response into a transport failure.
        let _ = promoted.headers_mut().try_append(name, value);
    }
    promoted
}

/// The response headers this model binds to a TIMESTAMP whose `timestampFormat` is
/// `date-time`, which is the format each of them has to go out in.
///
/// `smithy_query(query="shape",shape="ObjectLockRetainUntilDate",
/// model_path="services/service-com-amazonaws-s3-amazons3/smithy-model/com-amazonaws-s3-amazons3.json")`
/// reports `timestampFormat: date-time` on the shape `GetObject`'s and `HeadObject`'s
/// `x-amz-object-lock-retain-until-date` member targets, and S3 answers that header as an
/// ISO8601 date-time accordingly. Every other timestamp header of this service's responses
/// (`Last-Modified`, `x-amz-abort-date`) targets a shape carrying no such trait, and so goes
/// out in HTTP's own date format.
const DATE_TIME_TIMESTAMP_HEADERS: [&str; 1] = ["x-amz-object-lock-retain-until-date"];

/// `response` with every header of [`DATE_TIME_TIMESTAMP_HEADERS`] re-rendered in the
/// `date-time` format the model binds it with.
///
/// The rule lives here, once, for the same reason [`with_partial_content_status`] does: the
/// framework's header serializer renders a bound timestamp in HTTP's date format unless the
/// MEMBER itself carries the format trait, and in this model the trait is on the member's
/// TARGET shape — so no handler and no shape declaration can state it, and a header left in
/// the other format is one no client reads as the date the API publishes. Only a header the
/// response actually carries is touched, and a value that does not read back as a date is
/// left exactly as it is rather than dropped.
fn with_date_time_timestamp_headers(mut response: Response) -> Response {
    for name in DATE_TIME_TIMESTAMP_HEADERS {
        let Some(rendered) = response.headers().get(name).map(str::to_string) else {
            continue;
        };
        let Ok(instant) =
            DateTime::from_str(&rendered, aws_smithy_types::date_time::Format::HttpDate)
        else {
            continue;
        };
        let Ok(published) = instant.fmt(aws_smithy_types::date_time::Format::DateTime) else {
            continue;
        };
        let _ = response.headers_mut().try_insert(name, published);
    }
    response
}

/// The header naming the request a response answers: "`x-amz-request-id`: A value created
/// by Amazon S3 that uniquely identifies the request" (Amazon S3 API Reference, *Common
/// Response Headers*), which every S3 response carries — it is what a client's
/// `ResponseMetadata.RequestId` reports and what an event notification names the call it
/// came of by.
const REQUEST_ID_HEADER: &str = "x-amz-request-id";

/// The second correlation header of that same list: "`x-amz-id-2`: A special token that
/// helps Amazon Web Services troubleshoot problems", reported as
/// `ResponseMetadata.HostId`.
const EXTENDED_REQUEST_ID_HEADER: &str = "x-amz-id-2";
/// The pair a SUCCESS response carries: the one the operation that served the request
/// recorded ([`crate::state::request_ids`]), so an event notification and the response of
/// the call that produced it name the SAME request, or a freshly minted one when nothing
/// recorded any (every operation that publishes no event).
///
/// TAKEN before anything that can fail, so a serializer that errors on its way to a
/// response cannot leave a pair behind for a LATER response on this thread to pick up. An
/// ERROR response's pair is the envelope's own — see `serialize_error`.
fn answering_request() -> crate::state::request_ids::RequestCorrelation {
    crate::state::request_ids::answering()
        .unwrap_or_else(crate::state::request_ids::RequestCorrelation::mint)
}

/// `response` with the two request-correlation headers every S3 response carries.
fn with_request_correlation(
    mut response: Response,
    correlation: crate::state::request_ids::RequestCorrelation,
) -> Response {
    // A value a header map refuses is dropped rather than turning a served response into a
    // transport failure; both are hex, so neither ever is.
    let _ = response
        .headers_mut()
        .try_insert(REQUEST_ID_HEADER, correlation.request_id);
    let _ = response
        .headers_mut()
        .try_insert(EXTENDED_REQUEST_ID_HEADER, correlation.extended_request_id);
    response
}

/// `response` — an ERROR envelope — named by a freshly minted correlation pair
/// ([`crate::state::request_ids`]) in both its `<RequestId>` / `<HostId>` elements and its
/// two headers.
///
/// A refusal's envelope carries the same pair of ids a served response does: an
/// `x-amz-request-id` is "a value created by Amazon S3 that uniquely identifies the
/// request" and an `x-amz-id-2` "a special token that helps Amazon Web Services
/// troubleshoot problems" (Amazon S3 API Reference, *Common Response Headers*), and the
/// REST-XML error document mirrors them in `<RequestId>` and `<HostId>`. What a client, a
/// log correlator or a support request reads is the id's SHAPE — the one every S3 response
/// of this emulator is named by, minted in one draw so the two ids name the same request —
/// and not whatever a generic serializer happened to generate, so the ids the inner
/// serializer wrote are replaced wherever it wrote them. The `content-length` it set is
/// corrected rather than left stale, the way [`with_served_root_element`] corrects it.
fn with_envelope_request_correlation(mut response: Response) -> Response {
    let minted = crate::state::request_ids::RequestCorrelation::mint();
    // The ids the inner serializer chose, read back off the headers it wrote them to: they
    // are opaque tokens, so finding them in the document is what locates the two elements
    // carrying them without re-parsing the document.
    let written: Vec<(String, String)> = [
        (REQUEST_ID_HEADER, &minted.request_id),
        (EXTENDED_REQUEST_ID_HEADER, &minted.extended_request_id),
    ]
    .into_iter()
    .filter_map(|(header, replacement)| {
        let written = response.headers().get(header)?;
        // An empty pattern would be found between every pair of characters in the body;
        // the serializer writes neither id empty, and this is what keeps that true.
        (!written.is_empty()).then(|| (written.to_string(), replacement.to_string()))
    })
    .collect();
    if let Some(body) = response.body().bytes() {
        let mut document = String::from_utf8_lossy(body).into_owned();
        for (chosen, minted) in &written {
            document = document.replace(chosen.as_str(), minted.as_str());
        }
        let length = document.len();
        *response.body_mut() = SdkBody::from(document);
        if response.headers().get("content-length").is_some() {
            response
                .headers_mut()
                .insert("content-length", length.to_string());
        }
    }
    with_request_correlation(response, minted)
}

/// `response` with its document's ROOT element named the way the service names it, or
/// `response` unchanged for an operation whose root the model already names correctly (see
/// [`SERVED_OUTPUT_ROOT_ELEMENTS`]).
///
/// The root name appears exactly twice in a document — its start tag and its end tag — and
/// no member of these outputs shares the name, so renaming both occurrences renames the root
/// and nothing else. The `content-length` the serializer set is corrected rather than left
/// stale, the way [`with_attribute_members`] corrects it: a header that disagrees with the
/// body is worse than none.
fn with_served_root_element(mut response: Response, operation_name: &str) -> Response {
    let Some((_, modelled, served)) = SERVED_OUTPUT_ROOT_ELEMENTS
        .iter()
        .find(|(operation, _, _)| *operation == operation_name)
    else {
        return response;
    };
    let Some(body) = response.body().bytes() else {
        return response;
    };
    let document = String::from_utf8_lossy(body)
        .replace(&format!("<{modelled}"), &format!("<{served}"))
        .replace(&format!("</{modelled}>"), &format!("</{served}>"));
    let length = document.len();
    *response.body_mut() = SdkBody::from(document);
    if response.headers().get("content-length").is_some() {
        response
            .headers_mut()
            .insert("content-length", length.to_string());
    }
    response
}

/// `response` with every `@xmlAttribute` member written where the API publishes it, or
/// `response` unchanged when its body carries none.
///
/// A `content-length` the serializer set is corrected rather than left stale: the
/// normalized document is a different length (an element becomes an attribute and a
/// namespace declaration appears), and a header that disagrees with the body is worse
/// than none.
fn with_attribute_members(mut response: Response) -> Response {
    let Some(body) = response.body().bytes() else {
        return response;
    };
    if !XML_ATTRIBUTE_MEMBERS
        .iter()
        .any(|member| holds(body, format!("<{}", member.attribute).as_bytes()))
    {
        return response;
    }
    let Some(document) = elements_as_attributes(body) else {
        return response;
    };
    let length = document.len();
    *response.body_mut() = SdkBody::from(document);
    if response.headers().get("content-length").is_some() {
        response
            .headers_mut()
            .insert("content-length", length.to_string());
    }
    response
}

::std::thread_local! {
    /// The XML DOCUMENT the request this thread is serving carried, for the length of that
    /// one request — see [`spelled_in_request`].
    static REQUEST_DOCUMENT: ::std::cell::RefCell<Option<Vec<u8>>> =
        const { ::std::cell::RefCell::new(None) };
    /// Whether that request carried an `x-amz-object-attributes` header, and with what — see
    /// [`object_attributes_header_as_sent`].
    static OBJECT_ATTRIBUTES_HEADER: ::std::cell::RefCell<Option<String>> =
        const { ::std::cell::RefCell::new(None) };
}

/// Record the `x-amz-object-attributes` header the request this thread is serving carried,
/// `None` for a request that carried none.
///
/// Called for EVERY request this service reads, under the same discipline as
/// [`remember_request_document`]: a `None` overwrites whatever the previous request on this
/// thread recorded, so no refusal can read another request's header.
fn remember_object_attributes_header(header: Option<&str>) {
    let recorded = header.map(str::to_string);
    OBJECT_ATTRIBUTES_HEADER.with(|note| {
        *note.borrow_mut() = recorded;
    });
}

/// The `x-amz-object-attributes` header the request this thread is serving SENT, or `None`
/// when it sent none at all.
///
/// What tells the two requests that state no attribute apart, which is what decides their
/// refusal (`crate::service::published_refusal`): a request that sent NO such header asked for
/// no attributes, and one that sent it EMPTY named an attribute that is not one. The parsed
/// input cannot tell them apart — both bind an empty list — so the request's own headers are
/// what answers it.
///
/// Read under the same discipline as [`spelled_in_request`].
pub fn object_attributes_header_as_sent() -> Option<String> {
    OBJECT_ATTRIBUTES_HEADER.with(|note| note.borrow().clone())
}

/// Record `document` as the XML document the request this thread is serving carried, `None`
/// for a request whose body is not one.
///
/// Called for EVERY request this service reads, under the same discipline as
/// [`crate::state::addressed_bucket`]: a `None` overwrites whatever the previous request on
/// this thread recorded, so no handler can read another request's document.
fn remember_request_document(document: Option<&[u8]>) {
    let recorded = document.map(<[u8]>::to_vec);
    REQUEST_DOCUMENT.with(|note| {
        *note.borrow_mut() = recorded;
    });
}

/// The TEXT the request's own XML document spelled `element` in — the first element of that
/// local name — or `None` for a request that carried no document or no such element.
///
/// What an `<ArgumentValue>` echoes for a value refused inside a request document: S3's
/// `InvalidArgument` names the field it rejected and the value that field CARRIED, and the
/// parsed value has lost its spelling (a timestamp parses to an instant, which can be
/// rendered several ways and is no longer the text the caller sent). A client matching the
/// envelope against what it sent reads the text, so the text is what goes out.
///
/// Read rather than taken: the handler that refuses the value runs in the same stretch of
/// the one dispatch that deserialized the request (`vendor/smithy-server-runtime`'s request
/// walk), and the note is replaced by the next request this thread reads.
pub fn spelled_in_request(element: &str) -> Option<String> {
    REQUEST_DOCUMENT.with(|note| {
        let document = note.borrow();
        element_text(document.as_deref()?, element)
    })
}

/// The WHOLE XML document the request this thread is serving carried, as it arrived, or
/// `None` for a request whose body is not one.
///
/// What a handler judging the DOCUMENT rather than one value in it reads: whether a payload
/// validates against the schema S3 publishes for it is a question about the elements the
/// caller spelled, and the parsed input no longer carries them — a codec reads an element it
/// does not know by ignoring it, so an undeclared one is indistinguishable, after parsing,
/// from a member that was never sent.
///
/// Read under the same discipline as [`spelled_in_request`]: the handler runs in the same
/// stretch of the one dispatch that deserialized the request, and the note is replaced by
/// the next request this thread reads.
pub fn request_document() -> Option<Vec<u8>> {
    REQUEST_DOCUMENT.with(|note| note.borrow().clone())
}

/// The text of the first element of `document` whose name is `element`, read through the
/// same XML reader this module normalizes documents with.
fn element_text(document: &[u8], element: &str) -> Option<String> {
    let mut reader = Reader::from_reader(document);
    let mut inside = false;
    loop {
        match reader.read_event() {
            Ok(Event::Start(start)) => {
                inside = start.name().as_ref() == element.as_bytes();
            }
            Ok(Event::Text(text)) if inside => {
                return text.xml10_content().ok().map(|text| text.into_owned())
            }
            Ok(Event::End(_)) => inside = false,
            Ok(Event::Eof) | Err(_) => return None,
            _ => {}
        }
    }
}

/// `path` with every TRAILING slash of the key it addresses written as an escape, or `None`
/// for a path whose key does not end in one — which is every path that needs nothing here.
///
/// An object key is "a sequence of Unicode characters whose UTF-8 encoding is at most 1,024
/// bytes long" (`smithy_query(query="doc",shape="ObjectKey")`), and `/` is one of them: a key
/// may carry slashes anywhere, the trailing position included, which is exactly how the
/// "folder" objects every console and CLI creates are named (`sourc/`). The key is the
/// `{Key+}` greedy label of the request path, so what the client sent after the bucket
/// segment IS the key, decoded once and otherwise verbatim.
///
/// The codec that binds a greedy label matches the path by its SEGMENTS, having trimmed the
/// separators off BOTH ends first, which drops an empty trailing one — so
/// `PUT /<bucket>/sourc/` would bind `sourc` and the write would land on another key, one the
/// client never named. That is a gap in the shared codec, filed as
/// `issues/.../gap-smithy-schema-server-greedy-label-drops-a-trailing-separator.md`; this
/// adapter is what stands in for it until the codec keeps the segment, and should be deleted
/// with it. Escaping the trailing slashes puts them inside the last segment, where the
/// label's own percent-decode turns them back into the slashes the request carried: the key
/// the handler stores is then the key the client addressed, and a read, a listing, a delete
/// and a copy source all name that one key.
///
/// Nothing else about the path changes, and a path whose key carries no trailing slash is
/// left exactly as it arrived.
fn key_trailing_slashes_escaped(path: &str) -> Option<String> {
    let (bucket, key) = path.strip_prefix('/')?.split_once('/')?;
    if !key.ends_with('/') {
        return None;
    }
    let kept = key.trim_end_matches('/');
    let escaped = ESCAPED_PATH_SEPARATOR.repeat(key.len() - kept.len());
    Some(format!("/{bucket}/{kept}{escaped}"))
}

/// The separator [`key_trailing_slashes_escaped`] writes as an escape, in the one spelling
/// the codec's percent-decode reads back as a slash: left literal it would be read as the end
/// of the path rather than as part of the key.
const ESCAPED_PATH_SEPARATOR: &str = "%2F";

/// The OBJECT a request addresses, read off its PATH-STYLE path and its query string: the
/// `{Key+}` label below the bucket segment, decoded the way the codec decodes a path label,
/// and the `versionId` parameter when the request carries one. `None` for a path that
/// addresses a bucket or the service root, which names no object at all.
///
/// What the `<Key>` / `<VersionId>` of an object-scoped error document is written from (see
/// [`crate::scaffolded`]), so one request's address is read once, here, rather than per
/// handler. The decode and the query read are the ones [`super::sigv4`] already owns, so a
/// request's address is read one way wherever it is read.
///
/// `pub(super)`: the request-metric fold reads the object a counted request addressed through
/// this same one ([`super::request_metrics`]), so the key a filter is judged against is the
/// key the request's own answer was named by.
pub(super) fn addressed_object(
    path: &str,
    query: &str,
) -> Option<crate::state::addressed_bucket::AddressedObject> {
    let (_, key) = path.trim_start_matches('/').split_once('/')?;
    if key.is_empty() {
        return None;
    }
    Some(crate::state::addressed_bucket::AddressedObject {
        key: super::sigv4::percent_decoded(key),
        version_id: super::sigv4::query_parameter(query, VERSION_ID_PARAMETER),
    })
}

/// The query parameter a request names one stored version of an object by — the
/// `@httpQuery` every operation that takes a version binds it from
/// (`smithy_query(query="operation",operation="GetObject")` reports
/// `VersionId: {httpQuery: "versionId"}`).
const VERSION_ID_PARAMETER: &str = "versionId";

/// The query parameter a request names the multipart upload it addresses by — the
/// `@httpQuery` every operation that takes one binds it from
/// (`smithy_query(query="operation",operation="CompleteMultipartUpload")` reports
/// `UploadId: {httpQuery: "uploadId"}`).
const UPLOAD_ID_PARAMETER: &str = "uploadId";

/// `response` describing its body as the JSON document it is — see
/// [`JSON_PAYLOAD_OUTPUT_OPERATIONS`]. Only the header changes; the body the handler
/// answered with is untouched.
fn with_json_payload_content_type(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert("content-type", JSON_PAYLOAD_CONTENT_TYPE);
    response
}

/// `response` with no body at all when the operation's output binds NOTHING to one.///
/// An output every member of which goes out in a response HEADER (or as the response
/// status) leaves no document to send, and S3 sends none: `PutObject` answers 200 with an
/// empty payload and its result — the stored object's `ETag`, the `x-amz-version-id` of
/// the version it created — in those headers (Amazon S3 API Reference, *PutObject* sample
/// response, whose body is `Content-Length: 0`). The framework's generic restXml
/// serializer always opens the output shape's element, so unaided such an operation
/// answers an empty envelope (`<PutObjectOutput></PutObjectOutput>`): a document the API
/// does not send, and one in which a client that reads the response DOCUMENT for the
/// write's result finds none of it.
///
/// The response's own HEADERS are left exactly as the serializer wrote them, this
/// service's `content-type` included: a response that carries no body still describes what
/// a `GET` of the same resource would have answered with, which is what makes a HEAD's
/// headers worth reading at all.
///
/// Keyed on the output SCHEMA rather than on the operation, so every operation whose
/// result is entirely header-bound answers the same way and no new one has to be added
/// here. An output with a payload member or any member written into the envelope is
/// untouched — its body IS the response.
fn without_unbound_body(mut response: Response, output_schema: &Schema) -> Response {
    let binds_nothing = output_schema.members().iter().all(|member| {
        member.http_header().is_some()
            || member.http_prefix_headers().is_some()
            || member.http_response_code().is_some()
    });
    if !binds_nothing {
        return response;
    }
    *response.body_mut() = SdkBody::empty();
    response
}

/// The fraction every instant S3 writes into a document carries: three zero digits, because
/// the instants it records have one-second resolution (see
/// [`as_second_resolution_date_time`]). Fixed text, so the form is fixed-width.
const SECOND_RESOLUTION_FRACTION: &str = ".000";

/// The elements a response DOCUMENT of this service reports an instant S3 ITSELF RECORDED in.
///
/// The generated schemas carry an operation's own members and not the shapes they reach, so
/// which elements a document reports a timestamp in cannot be walked at serialization time;
/// the catalog lives here instead, and `tests/all/bucket_create_list_and_tagging_contracts.rs`
/// re-derives it from the model — every `timestamp` member of an operation OUTPUT and of the
/// shapes those outputs reach, under the element name restXml writes it with — so a model
/// update that reports another instant cannot quietly keep the framework's rendering of it.
///
/// A timestamp a REQUEST document also carries is deliberately NOT here: a lifecycle rule's
/// `Date` and an Object Lock `RetainUntilDate` are values the CALLER sent and the service
/// stored, and a stored configuration reads back as the caller wrote it. The form below is the
/// one S3 renders the instants IT records in, which is why the catalog is the output-only
/// half.
pub const DOCUMENT_TIMESTAMP_ELEMENTS: &[&str] = &[
    "CreationDate",
    "Expiration",
    "Initiated",
    "LastModified",
    "RestoreExpiryDate",
];

/// Whether `output_schema`'s response body is one of this service's XML DOCUMENTS.
///
/// An output whose `@httpPayload` member is a SCALAR (a blob or a string) IS its body: an
/// object's own bytes, or a bucket policy's JSON, neither of which is a document of this
/// service's and neither of which may be rewritten, whatever text it happens to carry.
fn answers_xml_document(output_schema: &Schema) -> bool {
    !output_schema
        .members()
        .iter()
        .any(|member| member.http_payload().is_some() && !is_structured_shape(member))
}

/// Whether `schema`'s target is a shape a document is written FOR, rather than a scalar
/// bound to the raw body.
fn is_structured_shape(schema: &Schema) -> bool {
    matches!(
        schema.shape_type(),
        ShapeType::Structure | ShapeType::Union | ShapeType::List | ShapeType::Map
    )
}

/// `response` with every instant its document reports rendered the way Amazon S3 renders
/// one: the fixed-width ISO-8601 form `YYYY-MM-DDTHH:MM:SS.000Z` — whole seconds, and the
/// three-digit fraction S3's own values carry.
///
/// S3 records an instant at one-second resolution and reports it at that one width wherever
/// it appears — a bucket's `CreationDate`, an object's `LastModified`, an upload's
/// `Initiated` — so the form is a property of this service's wire rather than of one
/// operation, and a client that reads S3's fixed-width timestamp (or compares one by shape)
/// rejects any other width. The framework's serializer renders a timestamp at whatever
/// resolution the VALUE carries and drops trailing zeros from its fraction, which for an
/// instant taken from the clock is a microsecond form no S3 response has.
///
/// Scoped twice over, so nothing but an instant is ever touched: only a response whose body
/// is one of this service's documents is read at all ([`answers_xml_document`]), only the
/// elements the model reports a timestamp in are looked for
/// ([`DOCUMENT_TIMESTAMP_ELEMENTS`]), and only text that reads back as an instant is
/// rewritten. A `content-length` the serializer set is corrected rather than left
/// stale, the way [`with_served_root_element`] corrects it.
fn with_second_resolution_timestamps(mut response: Response, output_schema: &Schema) -> Response {
    if !answers_xml_document(output_schema) {
        return response;
    }
    let Some(body) = response.body().bytes() else {
        return response;
    };
    let mut document = String::from_utf8_lossy(body).into_owned();
    let mut rewritten = false;
    for element in DOCUMENT_TIMESTAMP_ELEMENTS {
        if let Some(rendered) = with_second_resolution_text(&document, element) {
            document = rendered;
            rewritten = true;
        }
    }
    if !rewritten {
        return response;
    }
    let length = document.len();
    *response.body_mut() = SdkBody::from(document);
    if response.headers().get("content-length").is_some() {
        response
            .headers_mut()
            .insert("content-length", length.to_string());
    }
    response
}

/// `document` with the text of every `element` that reads back as an instant re-rendered by
/// [`as_second_resolution_date_time`], or `None` when nothing changed.
///
/// The text of an element is the run between its start and end tags, and an instant carries
/// no markup, so the run is read without a parser; text that does NOT read back as an
/// instant is left exactly as it is.
fn with_second_resolution_text(document: &str, element: &str) -> Option<String> {
    let open = format!("<{element}>");
    // A document that does not carry the element at all is left alone without copying it:
    // the catalog is walked for every response, and most responses report no instant.
    if !document.contains(&open) {
        return None;
    }
    let close = format!("</{element}>");
    let mut rendered = String::with_capacity(document.len());
    let mut rest = document;
    let mut changed = false;
    while let Some(start) = rest.find(&open) {
        let after_open = start + open.len();
        let Some(text_length) = rest.get(after_open..).and_then(|tail| tail.find(&close)) else {
            break;
        };
        let text = rest.get(after_open..after_open + text_length)?;
        rendered.push_str(rest.get(..after_open)?);
        match as_second_resolution_date_time(text) {
            Some(instant) => {
                changed |= instant != text;
                rendered.push_str(&instant);
            }
            None => rendered.push_str(text),
        }
        rest = rest.get(after_open + text_length..)?;
    }
    rendered.push_str(rest);
    changed.then_some(rendered)
}

/// The instant `text` names, rendered the way S3 writes one into a document, or `None` for
/// text that does not read back as an ISO-8601 instant at all.
///
/// The whole seconds are rendered by the library (the calendar is its own) and the fraction
/// is written as the three zero digits S3's own values carry: S3 records an instant at
/// ONE-SECOND resolution — a bucket's creation date, an object's last-modified date — and
/// renders it with a three-digit fraction, so every value it sends ends in `.000Z`. Anything
/// finer in the value is resolution the service does not keep, so it is dropped rather than
/// reported.
fn as_second_resolution_date_time(text: &str) -> Option<String> {
    let instant = DateTime::from_str(text, aws_smithy_types::date_time::Format::DateTime).ok()?;
    let whole_seconds = DateTime::from_secs(instant.secs())
        .fmt(aws_smithy_types::date_time::Format::DateTime)
        .ok()?;
    Some(format!(
        "{}{SECOND_RESOLUTION_FRACTION}Z",
        whole_seconds.strip_suffix('Z')?
    ))
}

impl ServerProtocol for UnwrappedXmlRestXml {
    type Request = Request;
    type Response = Response;

    fn protocol_id(&self) -> &ShapeId {
        self.inner.protocol_id()
    }

    fn routing_key<'a>(&self, request: &'a Self::Request) -> Result<&'a str, ProtocolError> {
        // Routing is the generated entry's protocol's job (see `new`); this answers
        // through the same code path, which with no routes reports the request as naming
        // no operation rather than guessing one.
        self.inner.routing_key(request)
    }

    fn deserialize_input<'a>(
        &self,
        request: &'a Self::Request,
        input_schema: &Schema,
    ) -> Result<Box<dyn ShapeDeserializer + 'a>, ProtocolError> {
        // Whatever this worker thread may still hold from an earlier response is dropped
        // before this request's handler can record its own pair, so no response can ever
        // be named by another request's id (see [`crate::state::request_ids`]).
        crate::state::request_ids::forget_any_unanswered();
        // And whatever restore note it may hold, for the same reason: the note a response
        // reads has to be its own request's (see [`with_accepted_status`]).
        forget_any_unanswered_restore();
        // And the delete-marker note, under the same discipline (see
        // [`DELETE_MARKER_HEADER`]).
        forget_any_unanswered_delete_marker();
        // And the two notes a refusal of its own carries: what a not-modified answer
        // reports about the representation the caller holds (see [`NOT_MODIFIED_STATUS`])
        // and the methods a 405 advertises (see [`ALLOW_HEADER`]).
        forget_any_unanswered_representation();
        forget_any_unanswered_allowed_methods();
        // The ACCESS POINT this request was addressed THROUGH, when its address reaches one
        // ([`Self::addressed_access_point`]), recorded for the decision taken on the path it
        // names: an access point has a policy of its own, and that policy governs the
        // requests made through the access point and no others
        // ([`crate::state::access_points`], and
        // [`crate::service::request_authorization::denial`] for the decision). Recorded for
        // EVERY request this service reads, so a request addressed to a bucket directly
        // records `None` rather than leaving the previous request's note behind.
        let through_access_point = self.served_access_point(request);
        crate::state::access_points::remember(through_access_point.clone());
        // The request's address in the PATH-STYLE form every route of this service is bound
        // in ([`Self::path_style_path`]), which is the path the three notes below and the
        // `@httpLabel` members are read from: a virtual-hosted-style request's bucket comes
        // back in front of its key, and an access point's address becomes the bucket it
        // fronts.
        let path_style = self
            .path_style_path(request)
            .unwrap_or_else(|| request_path(request.uri()).to_string());
        // The BUCKET this request addresses, recorded for the one thing that needs it once
        // the handler has returned: the `BucketName` a bucket-scoped error document names
        // the bucket by when the refusal did not name one itself
        // ([`crate::state::addressed_bucket`]). Recorded for EVERY request this service
        // reads, before anything here can refuse one, so the slot a response reads is
        // always its own request's — a request that addresses no bucket (`GET /`) records
        // nothing rather than leaving the previous one's note behind.
        //
        // It is the bucket the request's address REACHES, in either form: the first label of
        // the host for a virtual-hosted-style request ([`super::addressed_in_host`]), else
        // the first segment of the path, which is the `{Bucket}` label every bucket-scoped
        // route binds. A name S3 can address no bucket by ([`super::names_a_bucket`]) names
        // no bucket at all and is not recorded: the element is a bucket's name, not an echo
        // of arbitrary request text — and such a request is refused for the NAME anyway
        // (`crate::service::invalid_bucket_name`).
        let addressed_bucket = super::first_path_segment(&path_style)
            .map(str::to_string)
            .filter(|name| super::names_a_bucket(name));
        crate::state::addressed_bucket::remember(addressed_bucket.as_deref());
        // And the OBJECT it addresses, for the `<Key>` and `<VersionId>` an object-scoped
        // error document names it by — recorded under the same discipline, from the request's
        // address in PATH-STYLE form, so a virtual-hosted-style request (whose path is the
        // key alone) records the same key a path-style one does. A request that addresses a
        // bucket or the service root records nothing.
        crate::state::addressed_bucket::remember_object(addressed_object(
            &path_style,
            request_query(request.uri()),
        ));
        // And the multipart UPLOAD it addresses, for the `<UploadId>` an upload-scoped error
        // document names the upload that was not there by — the `uploadId` query parameter
        // every operation that addresses an upload binds that member from
        // (`smithy_query(query="operation",operation="CompleteMultipartUpload")` reports
        // `UploadId: {httpQuery: "uploadId"}`, as do `UploadPart`, `UploadPartCopy`,
        // `ListParts` and `AbortMultipartUpload`). Recorded under the same discipline: a
        // request that addresses no upload records nothing.
        crate::state::addressed_bucket::remember_upload(
            super::sigv4::query_parameter(request_query(request.uri()), UPLOAD_ID_PARAMETER)
                .as_deref(),
        );
        // And the DOCUMENT it carried, for a refusal that has to echo a value AS THE REQUEST
        // SPELLED IT once the codec has parsed it ([`spelled_in_request`]). Only a request
        // whose body IS an XML document carries one; every other request records nothing,
        // which is what clears whatever the previous request on this thread recorded.
        let document = match carries_xml_document_payload(input_schema) {
            true => request.body().bytes(),
            false => None,
        };
        remember_request_document(document);
        // And whether it carried an `x-amz-object-attributes` header, which is what tells a
        // request that sent none from one that sent it empty once the codec has bound both to
        // an empty list (see [`object_attributes_header_as_sent`]). Recorded for EVERY
        // request, so the note a refusal reads is always its own request's.
        remember_object_attributes_header(
            request.headers().get(crate::state::OBJECT_ATTRIBUTES_HEADER),
        );
        // A request whose body IS an XML document is refused here when that document
        // carries a character XML cannot hold, before any codec reads it. See
        // [`xml_charset_violation`].
        if let Some(violation) = xml_charset_violation(request, input_schema) {
            return Err(ProtocolError::Invalid(vec![violation]));
        }
        // And when a UNION member of that document does not carry exactly one member, which
        // is not a document of the shape it claims to be. See [`union_arity_violation`].
        if let Some(violation) = union_arity_violation(request, input_schema) {
            return Err(ProtocolError::Invalid(vec![violation]));
        }
        // The PATH the `@httpLabel` members are bound from, when it is not the path the
        // request arrived with — the PATH-STYLE address resolved above
        // ([`Self::path_style_path`]): a virtual-hosted-style request names its bucket in
        // the host and leaves its path as the key alone, so the bucket is put back in front
        // of it — the same normalization the routing seam resolved the operation from
        // (`super::Service::override_operation`), so the operation that was matched binds the
        // bucket that was addressed — and a request addressed to an ACCESS POINT binds the
        // bucket that access point fronts. And a key that ENDS in a slash keeps it, as an
        // escape the label's own decode undoes ([`key_trailing_slashes_escaped`]), in either
        // address form. `None` for a request that needs neither, whose path already IS the
        // path the labels bind from.
        let addressed_path = self.path_style_path(request);
        let bound_path = key_trailing_slashes_escaped(
            addressed_path
                .as_deref()
                .unwrap_or_else(|| request_path(request.uri())),
        )
        .or(addressed_path);
        // The HEADERS the bound members are read from, normalized where a value the request
        // carried is not one the framework's codec reads as the request meant it — a MODE
        // spelled in another case, a percent-encoded copy source, an HTTP-date, and the one
        // value an OPERATION judges ([`normalized_headers`]). `None` leaves the headers
        // exactly as they arrived, which is every request that needs no normalization.
        let folded_headers = normalized_headers(request, input_schema);
        // The QUERY the bound members are read from, with the one PARAMETER an OPERATION
        // judges withheld: a `max-buckets` that bounds no page is kept from the codec, whose
        // own report of a value that is no integer is a sentence about Rust types, so
        // `ListBuckets` answers the refusal S3 publishes for it instead
        // ([`rejected_max_buckets`]), and so is one stated with no value at all, which states
        // no bound. `None` leaves the query exactly as it arrived.
        let folded_query = without_withheld_max_buckets(request_query(request.uri()), input_schema);
        // Only a request whose BODY IS AN XML DOCUMENT can carry a member the framework's
        // codec cannot bind as the document spells it, and only a body that actually
        // spells one needs normalizing; everything else — every request whose body is not
        // a document, and every document whose members are already spelled the way the
        // codec binds them — is the framework's own path, byte for byte.
        if carries_xml_document_payload(input_schema) {
            let body = request
                .body()
                .bytes()
                .ok_or_else(|| ProtocolError::BodyRead("body is not available as bytes".into()))?;
            // Four normalizations, each applied only to a document that calls for it, and
            // each over the result of the one before it so a document needing several is
            // served.
            let mut normalized: Option<Vec<u8>> = None;
            if XML_ATTRIBUTE_MEMBERS
                .iter()
                .any(|member| holds(body, format!("{}=", member.attribute).as_bytes()))
            {
                normalized = attributes_as_elements(body);
            }
            let current: &[u8] = normalized.as_deref().unwrap_or(body);
            if XML_NAMED_UNION_MEMBERS
                .iter()
                .any(|member| holds(current, format!("<{}", member.published).as_bytes()))
            {
                if let Some(document) = published_names_as_members(current) {
                    normalized = Some(document);
                }
            }
            // And a THIRD, for a BOOLEAN the document spells the numeric way: the published
            // schema admits `1` and `0` for one, and the codec reads only the words, so those
            // two spellings are folded onto the words the codec binds
            // ([`xsd_boolean_spellings_folded`]). The byte scan comes first, so only a
            // document that carries a boolean element at all is walked.
            let current: &[u8] = normalized.as_deref().unwrap_or(body);
            if XML_BOOLEAN_DOCUMENT_ELEMENTS
                .iter()
                .any(|element| holds(current, format!("<{element}>").as_bytes()))
            {
                if let Some(document) = xsd_boolean_spellings_folded(current) {
                    normalized = Some(document);
                }
            }
            // And a FOURTH, for the whitespace an OBJECT KEY's element begins or ends with:
            // the codec reads an element's text trimmed, and a key's whitespace is part of the
            // key, so it is re-spelled as the character references that state it
            // ([`verbatim_text_folded`]). The byte scan comes first, so only a document that
            // carries such an element at all is walked.
            let current: &[u8] = normalized.as_deref().unwrap_or(body);
            if XML_OBJECT_KEY_DOCUMENT_ELEMENTS
                .iter()
                .any(|element| holds(current, format!("<{element}>").as_bytes()))
            {
                if let Some(document) = verbatim_text_folded(current) {
                    normalized = Some(document);
                }
            }
            // And a FIFTH, for the one document whose refusals `PutBucketWebsite` is the
            // one place that can state them: a website document that operation refuses on
            // the DOCUMENT — because the published schema rejects it, or because a member
            // carries a value only the document still holds — is withheld from the codec in
            // full, so the operation answers the refusal S3 publishes, naming the member,
            // rather than the boundary refusing it in a document with no room for the
            // member's name. Judged on the body AS IT ARRIVED, which is the same bytes the
            // handler reads it from ([`request_document`]), so the two cannot disagree
            // about which documents that operation is the one to refuse. See
            // `crate::state::website_document_rejection`.
            if carries_website_document(input_schema)
                && crate::state::website_document_rejection(body).is_some()
            {
                normalized = Some(EMPTY_WEBSITE_CONFIGURATION.to_vec());
            }
            // And a SIXTH, for the one VALUE a create's configuration carries that the
            // operation rather than the boundary refuses: a location constraint naming no
            // Region the model publishes is withheld from the codec — the whole container is
            // replaced by [`EMPTY_CREATE_BUCKET_CONFIGURATION`] — so `CreateBucket` answers
            // the refusal S3 publishes for it, which names the value in a member of its own,
            // rather than the boundary reporting the document as one that does not validate.
            // See [`rejected_location_constraint`].
            if rejected_location_constraint(input_schema).is_some() {
                normalized = Some(EMPTY_CREATE_BUCKET_CONFIGURATION.to_vec());
            }
            if let Some(document) = normalized {
                let uri = request.uri();
                let binding = HttpBindingDeserializer::new(
                    input_schema.http().map(|http| http.uri()),
                    bound_path.as_deref().unwrap_or(request_path(uri)),
                    folded_query.as_deref().unwrap_or_else(|| request_query(uri)),
                    folded_headers.unwrap_or_else(|| request_headers(request)),
                    OwnedXmlBody {
                        document: document.clone(),
                    },
                )
                .with_payload_bytes(document);
                return Ok(Box::new(binding));
            }
        }
        if bound_path.is_some() || folded_headers.is_some() || folded_query.is_some() {
            // The framework's own deserializer reads the path, the query and the headers off
            // the request, which for a virtual-hosted-style request carries no bucket in its
            // path, for a key that ends in a separator loses it, for a request spelling a mode
            // in another case carries a value the model's enum does not list, and for a page
            // bound the operation judges carries a value the codec cannot read — so this one
            // is built over the normalized path, query and headers instead. Everything else
            // is the framework's: the same binding deserializer, over the same body.
            let path = match bound_path {
                Some(path) => path,
                None => request_path(request.uri()).to_string(),
            };
            let body = request
                .body()
                .bytes()
                .ok_or_else(|| ProtocolError::BodyRead("body is not available as bytes".into()))?;
            // The body has to be IN MEMORY to be read here, which is the same condition the
            // framework's own restXml deserializer puts on EVERY request it reads, so this
            // branch asks no more of a request than the path it stands in for: a body not
            // available as bytes is refused there too, whatever made this branch the one taken.
            let uri = request.uri();
            let binding = HttpBindingDeserializer::new(
                input_schema.http().map(|http| http.uri()),
                &path,
                folded_query.as_deref().unwrap_or_else(|| request_query(uri)),
                folded_headers.unwrap_or_else(|| request_headers(request)),
                OwnedXmlBody {
                    document: body.to_vec(),
                },
            );
            // An `@httpPayload` member IS the body, so the raw bytes are wired for it (the
            // binding deserializer picks them for a scalar payload and the codec above for
            // a structured one). An input with no payload member never reads them.
            let binding = match input_schema
                .members()
                .iter()
                .any(|member| member.http_payload().is_some())
            {
                true => binding.with_payload_bytes(body.to_vec()),
                false => binding,
            };
            return Ok(Box::new(binding));
        }
        self.inner.deserialize_input(request, input_schema)
    }

    fn serialize_output(
        &self,
        output: &dyn SerializableStruct,
        output_schema: &Schema,
        operation_name: &str,
        success_status: u16,
    ) -> Result<Self::Response, ProtocolError> {
        // Taken FIRST, before anything that can fail: see [`answering_request`].
        let correlation = answering_request();
        // The same, for the note that decides which of the two statuses one operation's
        // success is answered with (see [`with_accepted_status`]).
        let accepted = restore_initiated();
        // And for the delete-marker note, which a SUCCESS never carries: taking it here is
        // what keeps a note an earlier refusal left from naming this response.
        let _ = addressed_delete_marker();
        // The same for the two notes only a refusal carries.
        let _ = not_modified_representation();
        let _ = allowed_methods();
        if UNWRAPPED_XML_OUTPUT_OPERATIONS.contains(&operation_name) {
            if let Some(body) =
                unwrapped_document(output, output_schema).map_err(ProtocolError::Serde)?
            {
                let status = StatusCode::try_from(success_status).map_err(|_| {
                    ProtocolError::Malformed(format!(
                        "{operation_name} declares the unusable success status {success_status}"
                    ))
                })?;
                let mut response = Response::new(status, SdkBody::from(body));
                response
                    .headers_mut()
                    .insert("content-type", "application/xml");
                return Ok(with_request_correlation(response, correlation));
            }
        }
        let response =
            self.inner
                .serialize_output(output, output_schema, operation_name, success_status)?;
        // A response whose body is a JSON document says so, whatever media type the generic
        // serializer derived for its payload member.
        let response = match JSON_PAYLOAD_OUTPUT_OPERATIONS.contains(&operation_name) {
            true => with_json_payload_content_type(response),
            false => response,
        };
        // A member the model marks `@xmlAttribute` has been written as an element; it
        // belongs on its parent's start tag, where the API publishes it and where a
        // namespace-aware parser can read it. Every instant the document reports is
        // rendered in the fixed-width form S3 writes one in
        // ([`with_second_resolution_timestamps`]). And a response that reports a
        // `Content-Range` carries PART of the object, which HTTP answers with 206, while a
        // retrieval one operation INITIATED is the 202 its documentation states. A
        // timestamp header the model binds in the `date-time` format is re-rendered in it.
        // An output that binds nothing to the body answers with no body at all.
        Ok(with_request_correlation(
            without_unbound_body(
                with_date_time_timestamp_headers(with_accepted_status(
                    with_partial_content_status(with_second_resolution_timestamps(
                        with_served_root_element(
                            with_attribute_members(response),
                            operation_name,
                        ),
                        output_schema,
                    )),
                    operation_name,
                    accepted,
                )),
                output_schema,
            ),
            correlation,
        ))
    }

    fn serialize_error(
        &self,
        error: &dyn SerializableStruct,
        error_schema: &Schema,
        error_code: &str,
        http_status: u16,
    ) -> Result<Self::Response, ProtocolError> {
        // TAKEN before anything that can fail, the way the correlation pair is: the two
        // headers an answer about a key whose current version is a delete marker carries
        // (see [`DELETE_MARKER_HEADER`]).
        let marker = addressed_delete_marker();
        // The same for the two notes a refusal of its own carries.
        let representation = not_modified_representation();
        let allowed = allowed_methods();
        // A read whose caller already holds the current representation is answered with the
        // bare status, the validators of that representation and the version it is held as,
        // and NO document: see [`NOT_MODIFIED_STATUS`].
        if http_status == NOT_MODIFIED_STATUS {
            let status = StatusCode::try_from(http_status).map_err(|_| {
                ProtocolError::Malformed(format!("the unusable refusal status {http_status}"))
            })?;
            let mut response = Response::new(status, SdkBody::empty());
            if let Some(representation) = representation {
                let _ = response
                    .headers_mut()
                    .try_insert(ETAG_HEADER, representation.etag);
                let _ = response
                    .headers_mut()
                    .try_insert(LAST_MODIFIED_HEADER, representation.last_modified);
                // The version the read evaluated, named the way every other answer about a
                // version names it, so a caller revalidating a cached copy learns WHICH
                // version is unchanged and can pin a later read to it.
                if let Some(version_id) = representation.version_id {
                    let _ = response
                        .headers_mut()
                        .try_insert(VERSION_ID_RESPONSE_HEADER, version_id);
                }
            }
            return Ok(with_request_correlation(
                response,
                crate::state::request_ids::RequestCorrelation::mint(),
            ));
        }
        // One refusal of this service carries NO error document: a request S3 rejects before
        // its API layer answers the bare status and an empty body, which is what an SDK
        // reports the status itself as the code it saw. The handler names it by
        // [`crate::service::object_write::ERRORLESS_REFUSAL_CODE`], and writing an `<Error>`
        // for it would hand a client a code AWS never sent.
        if error_code == crate::service::object_write::ERRORLESS_REFUSAL_CODE {
            let status = StatusCode::try_from(http_status).map_err(|_| {
                ProtocolError::Malformed(format!("the unusable refusal status {http_status}"))
            })?;
            return Ok(with_delete_marker_headers(
                Response::new(status, SdkBody::empty()),
                marker,
            ));
        }
        // The pair an error response names its request by. This model sets
        // `noErrorWrapping` ([`NO_ERROR_WRAPPING`]), so the inner serializer writes one
        // pair of its own into both the `<RequestId>` / `<HostId>` elements of the body and
        // the matching response headers — in a shape of its own choosing, which is not the
        // shape S3's ids have. A refusal publishes no event notification and so records no
        // pair, so the pair is minted here and written over the one the envelope carries,
        // keeping the body and the headers in agreement.
        Ok(with_allow_header(
            with_delete_marker_headers(
                with_envelope_request_correlation(self.inner.serialize_error(
                    error,
                    error_schema,
                    error_code,
                    http_status,
                )?),
                marker,
            ),
            allowed,
        ))
    }

    fn serialize_event_stream_message(
        &self,
        event_type: &str,
        is_exception: bool,
        event: &dyn SerializableStruct,
        event_schema: &Schema,
    ) -> Result<Self::Response, ProtocolError> {
        self.inner
            .serialize_event_stream_message(event_type, is_exception, event, event_schema)
    }
}
