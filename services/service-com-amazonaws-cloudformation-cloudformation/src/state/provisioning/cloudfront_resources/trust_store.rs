// SPDX-License-Identifier: CC0-1.0

//! `AWS::CloudFront::TrustStore`: the trust store a stack declares so a distribution can
//! validate the client certificates viewers present, created in Amazon CloudFront itself.
//!
//! ## Why it is really created
//!
//! A trust store's identity is the `ts_…` id CloudFront MINTS for it: `GetTrustStore`,
//! `UpdateTrustStore`, `DeleteTrustStore`, `ListDistributionsByTrustStore` and a
//! distribution's own `TrustStoreConfig$TrustStoreId` all address one by that id, and the ARN
//! built around it is what names the trust store elsewhere. So a stack that reported a trust
//! store created while nothing created it publishes an id and an ARN that resolve to nothing —
//! the first `GetTrustStore` on a stack output answers `EntityNotFound`, blaming the client for
//! an id the deployment itself produced, and the distribution that was meant to authenticate
//! viewers against the store can never be wired to it. Worse, the one value the resource
//! exists to report — `NumberOfCaCertificates`, "the trust store's number of CA certificates" —
//! is a property of the BUNDLE's bytes, which only the service that read them can count.
//! Calling that service's own create is what makes the identity the stack publishes one
//! CloudFront really holds, and the count one something really counted.
//!
//! ## The declaration
//!
//! The type publishes exactly the members `CreateTrustStore` takes ([`PROPERTIES`]), each
//! spelled as that operation spells it, so the declaration is rendered as the operation's own
//! request document through the rendering the catalogue in [`super`] uses — the CA certificates
//! bundle source as the nested structure the API takes, and the tag list in the container the
//! model wraps one in ([`super::WRAPPED_LISTS`]).
//!
//! The two members the API declares `@required` are required by the type as well, and neither
//! has a documented value a handler could supply for a declaration that states it nowhere, so
//! such a declaration is refused naming the member rather than sent without it. Every bound
//! BELOW them is the owning service's to enforce: the create is made, and the refusal it
//! answers is what fails the resource — a bundle that service's Amazon S3 holds nothing at, a
//! bundle whose bytes hold no CA certificate, a name another trust store of the account already
//! holds — so a client sees a resource-handler outcome rather than a template-syntax error.
//!
//! ## Changing one
//!
//! `UpdateTrustStore` takes the trust store's id with the bundle source to rebuild it from and
//! whether to check revocation against the CA certificate's OCSP endpoint, so those two are
//! applied to the trust store that exists and it keeps its identity. A changed `Name` never
//! reaches here: no call of that service renames a trust store, so the name is the property the
//! resource takes its identity from and a new one recreates the resource
//! ([`crate::state::template::name_is_create_only`]). A changed `Tags` is carried by no call
//! made here, as a key value store's is not ([`super::connection_and_store_resources`]).
//!
//! ## Deleting one
//!
//! `DeleteTrustStore` takes the id and the version CloudFront reports the trust store under, so
//! the delete reads that version first and removes exactly the trust store the stack created. A
//! trust store CloudFront no longer holds is already in the state the delete asks for, judged
//! on the CODE and never on a message; a trust store a distribution still validates client
//! certificates against is `CannotDeleteEntityWhileInUse`, which is a delete that cannot be
//! made rather than one that is still being made, so it travels as that service's own refusal.

use std::collections::BTreeMap;

use serde_json::{Map, Value};

use super::{Created, Refusal, Removal, StoredResource};

/// The resource type this module provisions.
pub(crate) const CLOUDFRONT_TRUST_STORE: &str = crate::state::template::CLOUDFRONT_TRUST_STORE;

/// Every property the type publishes, in the order `CreateTrustStoreRequest` declares its
/// members (`smithy_query(query="shape",shape="CreateTrustStoreRequest")`).
///
/// This is both the ORDER the request document writes them in and the set of members a
/// declaration may state; a declared property this list does not carry is one no call here
/// applies.
const PROPERTIES: &[&str] = &[
    NAME_PROPERTY,
    BUNDLE_SOURCE_PROPERTY,
    OCSP_ENDPOINT_PROPERTY,
    TAGS_PROPERTY,
];

/// The name the trust store is created under: `@required` by the API and by the type, and the
/// property the resource takes its identity from.
const NAME_PROPERTY: &str = "Name";

/// The CA certificates bundle the trust store is built from: `@required` by the API and by the
/// type, and the one member an update can give the trust store a new value for.
const BUNDLE_SOURCE_PROPERTY: &str = "CaCertificatesBundleSource";

/// Whether the trust store checks certificate revocation against the CA certificate's own OCSP
/// responder. Optional on the create, and bound to a HEADER on the update
/// (`UpdateTrustStore$UseClientCertificateOCSPEndpoint`).
const OCSP_ENDPOINT_PROPERTY: &str = "UseClientCertificateOCSPEndpoint";

/// The tags the create carries.
const TAGS_PROPERTY: &str = "Tags";

/// The members of the request this module cannot build without: the ones the API declares
/// `@required` for which neither the declaration nor a documented value supplies anything.
const REQUIRED_PROPERTIES: &[&str] = &[NAME_PROPERTY, BUNDLE_SOURCE_PROPERTY];

/// The attributes the type publishes, which are the members of that service's own trust store
/// a declaration does not state (`smithy_query(query="shape",shape="TrustStore")`): the ARN
/// and the id CloudFront minted, the status it reports the trust store in, the instant it last
/// changed it and the number of CA certificates the bundle holds.
const PUBLISHED_ATTRIBUTES: &[&str] = &[
    "Arn",
    "Id",
    "LastModifiedTime",
    "NumberOfCaCertificates",
    "Status",
];

/// The element CloudFront reports a trust store under, in every answer that carries one
/// (`CreateTrustStoreResult$TrustStore`, the operation's `@httpPayload`).
const REPORTED_TRUST_STORE: &str = "TrustStore";

/// The code CloudFront answers for a trust store this account does not hold, which is what
/// says a delete has nothing left to remove: "The entity was not found."
const ENTITY_NOT_FOUND: &str = "EntityNotFound";

/// The URI `CreateTrustStore` is bound to, under this service's API-version prefix.
const TRUST_STORE_COLLECTION: &str = "/trust-store";

/// The root element of the create's request document, as the operation's own request shape is:
/// no member of it is bound to the URI or to a header, so every member travels in the body.
const CREATE_REQUEST: &str = "CreateTrustStoreRequest";

/// Create the trust store an `AWS::CloudFront::TrustStore` declares.
///
/// The tags the declaration asks for and the ones CloudFormation gives the resource travel on
/// the create itself: `CreateTrustStore` takes the trust store and the tag list as one request,
/// which is the one call that makes a tagged trust store in a single step.
///
/// `Err` is CloudFront's own refusal of the create: a bundle address its Amazon S3 holds
/// nothing at, a bundle it may not read, a bundle whose bytes hold no CA certificate, a name
/// another trust store of the account already holds.
pub(super) async fn create(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    let body = request_document(&resource.properties, resource, propagated)?;
    let answered = super::call_cloudfront(
        ctx,
        region,
        "POST",
        TRUST_STORE_COLLECTION,
        Some(&body),
        None,
    )
    .await
    .map_err(provider_exception)?;
    let attributes = attributes(&answered);
    let id = attributes
        .get("Id")
        .cloned()
        .ok_or_else(|| unidentified("CreateTrustStore"))?;
    Ok(Created {
        physical_resource_id: Some(id),
        attributes,
    })
}

/// Apply a CHANGED declaration to the trust store the stack already has, through
/// `UpdateTrustStore` — the call that takes the trust store's id, so it keeps its identity and
/// nothing is replaced.
///
/// The call takes the bundle source in its body and the OCSP setting in a header, both
/// conditional on the version CloudFront reports the trust store under, so the version is read
/// first and travels as the `If-Match`.
pub(super) async fn update(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    declared: &Value,
) -> Result<Option<Created>, Refusal> {
    super::refuse_unapplied_properties(declared, &resource.resource_type, PROPERTIES)?;
    super::refuse_unrenderable_members(declared, &resource.resource_type, 0)?;
    let empty = Map::new();
    let members = declared.as_object().unwrap_or(&empty);
    let source = members
        .get(BUNDLE_SOURCE_PROPERTY)
        .filter(|value| !value.is_null())
        .ok_or_else(|| missing(BUNDLE_SOURCE_PROPERTY, resource))?;
    // The payload IS the bundle source structure (`UpdateTrustStore`'s `@httpPayload`), so the
    // document is that one member rendered under its own element and nothing around it.
    let body = super::rendered_member(
        BUNDLE_SOURCE_PROPERTY,
        source,
        members,
        &resource.resource_type,
    );
    let path = trust_store_path(&resource.physical_resource_id);
    let (_, version) = super::read_version(ctx, region, &path)
        .await
        .map_err(provider_exception)?;
    let (answered, _) = super::call_cloudfront_headed(
        ctx,
        region,
        "PUT",
        &path,
        Some(&body),
        version.as_deref(),
        &ocsp_endpoint_header(members),
    )
    .await
    .map_err(provider_exception)?;
    Ok(Some(Created::attributes(attributes(&answered))))
}

/// Delete the trust store a stack created, addressed by the id CloudFront minted for it.
pub(super) async fn delete(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<(), Refusal> {
    remove(ctx, region, &resource.physical_resource_id)
        .await
        .map(|_| ())
}

/// Ask CloudFront to take the trust store away, and report whether it still holds it.
///
/// The delete is conditional on the version the trust store is reported under, so that version
/// is read first. A trust store CloudFront no longer holds is already in the state the delete
/// asks for, judged on the CODE and never on a message. Every other refusal is that service's
/// own and travels as it is — a trust store a distribution still validates client certificates
/// against is `CannotDeleteEntityWhileInUse`, which is a delete that cannot be made rather than
/// one that is still being made.
pub(super) async fn remove(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    id: &str,
) -> Result<Removal, Refusal> {
    let path = trust_store_path(id);
    let version = match super::read_version(ctx, region, &path).await {
        Ok((_, version)) => version,
        Err(refusal) if refusal.code == ENTITY_NOT_FOUND => return Ok(Removal::Gone),
        Err(refusal) => return Err(provider_exception(refusal)),
    };
    match super::call_cloudfront(ctx, region, "DELETE", &path, None, version.as_deref()).await {
        Ok(_) => Ok(Removal::Gone),
        Err(refusal) if refusal.code == ENTITY_NOT_FOUND => Ok(Removal::Gone),
        Err(refusal) => Err(provider_exception(refusal)),
    }
}

/// One refusal CloudFront answered, as the EXCEPTION this type's provider raises for it — which
/// is what CloudFormation quotes on the failed resource's event
/// ([`super::super::handler_failure_reason`]).
///
/// This provider is written against the CloudFormation resource-provider plugin, so a refusal
/// of the REQUEST is re-raised as that plugin's invalid-request exception
/// ([`super::super::invalid_request_exception`]) with the TYPE the handler manages as its
/// label: the reason reads `Invalid request provided: AWS::CloudFront::TrustStore: <that
/// service's sentence and its SDK's diagnostic of the call>`, as the recorded
/// `ResourceStatusReason` of a trust store whose CA certificates bundle CloudFront would not
/// take carries it (in the behavioral corpus `simparity-behavioral-aws-v1`, check
/// `cloudformation__cloudfront__s3.deploy-cloudfront-truststore-invalid-ca-bundle-rejected`). A
/// refusal of any OTHER class is passed on as that service worded it.
fn provider_exception(refusal: Refusal) -> Refusal {
    super::super::invalid_request_exception(PROVIDER_LABEL, refusal)
}

/// The label this provider states inside that exception: the type it manages, with the
/// separator the recorded reason carries after it.
const PROVIDER_LABEL: &str = "AWS::CloudFront::TrustStore: ";

/// The `CreateTrustStoreRequest` document a create sends: the declared members in the order the
/// request shape declares them, with the tag list CloudFormation propagates to the resource
/// folded into its own.
fn request_document(
    properties: &Value,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<String, Refusal> {
    super::refuse_unapplied_properties(properties, &resource.resource_type, PROPERTIES)?;
    super::refuse_unrenderable_members(properties, &resource.resource_type, 0)?;
    let empty = Map::new();
    let declared = properties.as_object().unwrap_or(&empty);
    for property in REQUIRED_PROPERTIES {
        if declared
            .get(*property)
            .filter(|value| !value.is_null())
            .is_none()
        {
            return Err(missing(property, resource));
        }
    }
    let mut rendered = String::new();
    for property in PROPERTIES {
        // The tag list is built from the declaration AND the tags CloudFormation gives the
        // resource, so it is rendered from that union rather than from the declared list.
        if *property == TAGS_PROPERTY {
            rendered.push_str(&tags_element(properties, propagated));
            continue;
        }
        if let Some(value) = declared.get(*property).filter(|value| !value.is_null()) {
            rendered.push_str(&super::rendered_member(
                property,
                value,
                declared,
                &resource.resource_type,
            ));
        }
    }
    let namespace = super::CLOUDFRONT_XML_NAMESPACE;
    Ok(format!(
        "<{CREATE_REQUEST} xmlns=\"{namespace}\">{rendered}</{CREATE_REQUEST}>"
    ))
}

/// The `Tags` element a create carries: the tags the declaration asks for and the STACK-LEVEL
/// tags CloudFormation propagates to the resource, in the container the model wraps a tag list
/// in.
///
/// The three records CloudFormation keeps about a resource it provisioned
/// ([`super::super::is_stack_provenance_tag`]) are NOT among them: a provider written against
/// the resource-provider plugin is handed those separately from the tag set it is to apply, and
/// this one applies the set — so a trust store carries the tags the template and the stack
/// operation stated and nothing else, which is what a read of its tags reports (in the
/// behavioral corpus `simparity-behavioral-aws-v1`, check
/// `cloudformation__cloudfront__s3.deploy-cloudfront-truststore-round-trip-and-delete`, whose
/// read of the trust store's tags reports exactly the declared and stack-level ones).
fn tags_element(properties: &Value, propagated: &[(String, String)]) -> String {
    let applied: Vec<(String, String)> = propagated
        .iter()
        .filter(|(key, _)| !super::super::is_stack_provenance_tag(key))
        .cloned()
        .collect();
    let tags = super::tags_of(properties, &applied);
    if tags.is_empty() {
        return String::new();
    }
    let items: String = tags
        .iter()
        .map(|(key, value)| {
            format!(
                "<Tag>{}{}</Tag>",
                super::xml_leaf("Key", key),
                super::xml_leaf("Value", value)
            )
        })
        .collect();
    format!("<{TAGS_PROPERTY}><Items>{items}</Items></{TAGS_PROPERTY}>")
}

/// The OCSP header an update carries, or no header at all where the declaration states the
/// property nowhere: the member is optional, so a declaration that names none asks for no new
/// value for it.
fn ocsp_endpoint_header(declared: &Map<String, Value>) -> Vec<(&'static str, String)> {
    declared
        .get(OCSP_ENDPOINT_PROPERTY)
        .filter(|value| !value.is_null())
        .and_then(crate::state::scalar_text)
        .map(|value| vec![(OCSP_ENDPOINT_HEADER, value)])
        .unwrap_or_default()
}

/// The header `UpdateTrustStore` binds its OCSP member to, as that model names it
/// (`smithy_query(query="operation",operation="UpdateTrustStore")`).
const OCSP_ENDPOINT_HEADER: &str = "useclientcertificateocspendpoint";

/// The attributes a `TrustStore` answer reports for the resource: the ones the type publishes
/// ([`PUBLISHED_ATTRIBUTES`]), each read off that service's own report of the trust store.
fn attributes(answered: &str) -> BTreeMap<String, String> {
    let reported = super::xml_scalar_children(answered, REPORTED_TRUST_STORE);
    let mut attributes = BTreeMap::new();
    for attribute in PUBLISHED_ATTRIBUTES {
        if let Some(value) = reported
            .get(*attribute)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
        {
            attributes.insert(
                (*attribute).to_string(),
                super::published_attribute(attribute, value),
            );
        }
    }
    attributes
}

/// The URI the per-trust-store operations are bound to
/// (`/2020-05-31/trust-store/{Id}`), with the id written as ONE path segment so an id outside
/// the modelled form is refused by that operation's validation instead of addressing a
/// different trust store.
fn trust_store_path(id: &str) -> String {
    format!("{TRUST_STORE_COLLECTION}/{}", super::percent_encoded(id))
}

/// The refusal for an answer that identifies nothing: a resource whose identity nothing
/// reported is not one a stack can publish.
fn unidentified(operation: &str) -> Refusal {
    Refusal {
        code: "UnidentifiedFailure".to_string(),
        message: format!("{operation} answered no {REPORTED_TRUST_STORE}.Id"),
    }
}

/// The refusal for a declaration that states none of a property no call here can be built
/// without, naming which one it was.
fn missing(property: &str, resource: &StoredResource) -> Refusal {
    Refusal {
        code: super::MISSING_PROPERTY.to_string(),
        message: format!(
            "Property {property} cannot be empty for resource of type {}",
            resource.resource_type
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The record a declaration is rendered against: only its type, its properties and its
    /// physical id are read off here.
    fn resource(properties: Value) -> StoredResource {
        StoredResource {
            logical_id: "TheTrustStore".to_string(),
            resource_type: CLOUDFRONT_TRUST_STORE.to_string(),
            physical_resource_id: "ts_EXAMPLE1234567".to_string(),
            attributes: BTreeMap::new(),
            metadata: None,
            properties,
            materialised: false,
            deletion_policy: None,
            update_replace_policy: None,
        }
    }

    /// One well-formed declaration, as a template writes it: the trust store's name, the CA
    /// certificates bundle in Amazon S3 it is built from, and the OCSP setting.
    fn declared() -> Value {
        ::serde_json::json!({
            "Name": "viewer-cas",
            "CaCertificatesBundleSource": {
                "CaCertificatesBundleS3Location": {
                    "Bucket": "a-bundle-bucket",
                    "Key": "cas/bundle.pem",
                    "Region": "us-east-1",
                },
            },
            "UseClientCertificateOCSPEndpoint": true,
        })
    }

    /// The document a create sends is the operation's own request: the declared members in the
    /// order the request shape declares them, the bundle source as the nested structure the API
    /// takes, and the stack-level tags in the container the model wraps a tag list in.
    #[test]
    fn the_create_renders_the_operations_own_request_document() {
        let properties = declared();
        let propagated = [("owner".to_string(), "orders-team".to_string())];
        let rendered = request_document(&properties, &resource(properties.clone()), &propagated)
            .expect("case {a declaration of every member}: expected a request document");
        let namespace = super::super::CLOUDFRONT_XML_NAMESPACE;
        assert!(
            rendered.starts_with(&format!("<{CREATE_REQUEST} xmlns=\"{namespace}\">"))
                && rendered.ends_with(&format!("</{CREATE_REQUEST}>")),
            "case {{a declaration of every member}}: expected the operation's own request \
             element, got {rendered}"
        );
        for fragment in [
            "<Name>viewer-cas</Name>",
            "<CaCertificatesBundleSource><CaCertificatesBundleS3Location>",
            "<Bucket>a-bundle-bucket</Bucket>",
            "<Key>cas/bundle.pem</Key>",
            "<Region>us-east-1</Region>",
            "<UseClientCertificateOCSPEndpoint>true</UseClientCertificateOCSPEndpoint>",
            "<Tags><Items><Tag>",
            "<Key>owner</Key>",
        ] {
            assert!(
                rendered.contains(fragment),
                "case {fragment:?}: expected the request document to carry it, got {rendered}"
            );
        }
        // The members travel in the order the request shape declares them.
        let positions: Vec<usize> = [NAME_PROPERTY, BUNDLE_SOURCE_PROPERTY, OCSP_ENDPOINT_PROPERTY]
            .iter()
            .map(|member| {
                rendered
                    .find(&format!("<{member}>"))
                    .unwrap_or_else(|| panic!("case {member:?}: expected it in {rendered}"))
            })
            .collect();
        assert!(
            positions.windows(2).all(|pair| pair[0] < pair[1]),
            "case {{the member order}}: expected the request shape's order, got {rendered}"
        );
    }

    /// The tags a create applies are the declaration's own and the stack-level ones, and never
    /// the three records CloudFormation keeps about the resource it provisioned.
    #[test]
    fn the_three_stack_provenance_tags_are_not_applied_to_a_trust_store() {
        let properties = ::serde_json::json!({
            "Tags": [{ "Key": "sim2real-ts", "Value": "viewer-cas" }],
        });
        let propagated = [
            ("sim2real".to_string(), "a-stack".to_string()),
            (
                crate::state::provisioning::STACK_NAME_TAG.to_string(),
                "a-stack".to_string(),
            ),
            (
                crate::state::provisioning::STACK_ID_TAG.to_string(),
                "a-stack-id".to_string(),
            ),
            (
                crate::state::provisioning::LOGICAL_ID_TAG.to_string(),
                "TheTrustStore".to_string(),
            ),
        ];
        let rendered = tags_element(&properties, &propagated);
        for applied in ["<Key>sim2real-ts</Key>", "<Key>sim2real</Key>"] {
            assert!(
                rendered.contains(applied),
                "case {applied:?}: expected the tag element to carry it, got {rendered}"
            );
        }
        for withheld in [
            crate::state::provisioning::STACK_NAME_TAG,
            crate::state::provisioning::STACK_ID_TAG,
            crate::state::provisioning::LOGICAL_ID_TAG,
        ] {
            assert!(
                !rendered.contains(withheld),
                "case {withheld:?}: expected the tag element not to carry it, got {rendered}"
            );
        }
    }

    /// A declaration that states none of a `@required` member is refused naming that member,
    /// rather than sent to CloudFront without it.
    #[test]
    fn a_declaration_missing_a_required_member_is_refused_naming_it() {
        for property in REQUIRED_PROPERTIES {
            let mut properties = declared();
            properties
                .as_object_mut()
                .expect("the declaration above is an object")
                .remove(*property);
            let refused =
                request_document(&properties, &resource(properties.clone()), &[]).expect_err(
                    "expected the refusal of a declaration missing a required member",
                );
            assert_eq!(
                refused.message,
                format!(
                    "Property {property} cannot be empty for resource of type \
                     {CLOUDFRONT_TRUST_STORE}"
                ),
                "case {property:?}: expected the refusal to name the missing property, got \
                 {refused:?}"
            );
        }
    }

    /// A declared property the type does not publish is refused rather than dropped: a trust
    /// store reported created with less than its declaration asked for is the shallow success
    /// this module exists to avoid.
    #[test]
    fn a_property_the_type_does_not_publish_is_refused() {
        let mut properties = declared();
        properties
            .as_object_mut()
            .expect("the declaration above is an object")
            .insert("Comment".to_string(), ::serde_json::json!("a comment"));
        let refused = request_document(&properties, &resource(properties.clone()), &[])
            .expect_err("expected the refusal of a property the type does not publish");
        assert!(
            refused.message.contains("Comment"),
            "case {{a property the type does not publish}}: expected the refusal to name it, \
             got {refused:?}"
        );
    }

    /// The attributes are read off the answer's own `TrustStore`, and an answer that reports
    /// none of a published attribute carries none rather than an empty one.
    #[test]
    fn the_attributes_are_the_ones_the_answer_reports() {
        let answered = "<TrustStore><Id>ts_EXAMPLE1234567</Id>\
             <Arn>arn:aws:cloudfront::000000000000:trust-store/ts_EXAMPLE1234567</Arn>\
             <Name>viewer-cas</Name><Status>pending</Status>\
             <NumberOfCaCertificates>2</NumberOfCaCertificates></TrustStore>";
        let read = attributes(answered);
        for (attribute, expected) in [
            ("Id", "ts_EXAMPLE1234567"),
            (
                "Arn",
                "arn:aws:cloudfront::000000000000:trust-store/ts_EXAMPLE1234567",
            ),
            ("Status", "pending"),
            ("NumberOfCaCertificates", "2"),
        ] {
            assert_eq!(
                read.get(attribute).map(String::as_str),
                Some(expected),
                "case {attribute:?}: expected {expected:?}, got {read:?}"
            );
        }
        assert!(
            !read.contains_key("LastModifiedTime"),
            "case {{an answer reporting no instant}}: expected no LastModifiedTime, got {read:?}"
        );
        // The trust store's NAME is the declaration's, not an attribute the type publishes.
        assert!(
            !read.contains_key("Name"),
            "case {{the published set}}: expected only {PUBLISHED_ATTRIBUTES:?}, got {read:?}"
        );
    }

    /// The OCSP setting travels as the header the update binds it to, and a declaration that
    /// states it nowhere sends no header at all.
    #[test]
    fn the_ocsp_setting_travels_as_the_header_the_update_binds_it_to() {
        let rows: &[(&str, Value, Option<&str>)] = &[
            ("asked for", ::serde_json::json!(true), Some("true")),
            ("turned off", ::serde_json::json!(false), Some("false")),
            (
                "stated as the text a YAML template writes",
                ::serde_json::json!("true"),
                Some("true"),
            ),
            ("stated nowhere", Value::Null, None),
        ];
        for (case, value, expected) in rows {
            let mut members = Map::new();
            if !value.is_null() {
                members.insert(OCSP_ENDPOINT_PROPERTY.to_string(), value.clone());
            }
            let sent = ocsp_endpoint_header(&members);
            let found = sent.first().map(|(_, value)| value.as_str());
            assert_eq!(
                found, *expected,
                "case {case:?}: expected {expected:?}, got {found:?}"
            );
            assert!(
                sent.iter().all(|(name, _)| *name == OCSP_ENDPOINT_HEADER),
                "case {case:?}: expected the modelled header name, got {sent:?}"
            );
        }
    }
}
