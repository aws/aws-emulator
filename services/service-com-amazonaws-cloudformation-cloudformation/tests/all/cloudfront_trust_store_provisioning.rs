// SPDX-License-Identifier: CC0-1.0

//! A stack's declared `AWS::CloudFront::TrustStore` is a trust store Amazon CloudFront really
//! holds, addressable by the id the stack publishes for it.
//!
//! The rule under test: **a resource type CloudFormation publishes is one a template may
//! declare — the `Template format error: Unrecognized resource types: [<types>]` refusal is
//! reserved for a type CloudFormation does not know — and a declared type this emulator
//! provisions is created by calling the OWNING service's own create, so the physical resource
//! id the stack publishes is the identifier that service minted, its `Fn::GetAtt` values are the
//! ones that service reports, a declaration that service REFUSES fails the RESOURCE (and the
//! stack rolls back) carrying that refusal, and the stack's delete takes the resource away
//! there.** For a trust store the calls are `CreateTrustStore` (`POST /2020-05-31/trust-store`),
//! `UpdateTrustStore` (`PUT /2020-05-31/trust-store/{Id}`) and `DeleteTrustStore`
//! (`DELETE /2020-05-31/trust-store/{Id}`), the read is `GetTrustStore`
//! (`GET /2020-05-31/trust-store/{Identifier}`), and `EntityNotFound` (HTTP 404) is modelled
//! ONLY for an identifier that names no trust store — so a 404 for an id CloudFormation itself
//! reported is a defect.
//!
//! The rows differ along what the rule generalises over: how many CA certificates the bundle the
//! create reads holds and how many the one its update rotates onto holds — the count only the
//! owning service can report, since it is a property of the object's bytes — whether the
//! declaration asks for the CA certificate's OCSP endpoint to be used at all, whether it states
//! tags, and the form of the name it states.
//!
//! The certificates are real self-signed CA certificates minted here (`rcgen`) and staged in the
//! emulator's own Amazon S3 with `PutObject`, so every count asserted below is the number of
//! certificates a PARSER accepted rather than a number this suite and the owning service both
//! guessed.
//!
//! Everything goes over the wire each service speaks — awsQuery for CloudFormation, restXml for
//! Amazon CloudFront and Amazon S3 — through the one router `harness.rs` builds, which mounts
//! them all on one engine.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{
    assert_sdk_diagnostic_with_attempts, cloudfront, failed_resource_reason, physical_resource_id,
    quoted_and_class, CLOUDFRONT_API_VERSION,
};
use crate::stack_lifecycle::{
    call, create_stack, member, output_value, router, settled_stack, xml_blocks, xml_text,
    ACCOUNT_ID, REGION,
};
use crate::stack_resource_provisioning::s3;
use service_com_amazonaws_cloudformation_cloudformation::state::status;

/// The resource type this suite deploys, and the logical id every row declares it under.
const TRUST_STORE_TYPE: &str = "AWS::CloudFront::TrustStore";
const LOGICAL_ID: &str = "TrustStore";

/// The resource segment a trust store's ARN carries
/// (`arn:aws:cloudfront::<account>:trust-store/<Id>`). CloudFront is a GLOBAL service, so the
/// Region segment of the ARN is empty.
const TRUST_STORE_ARN_TYPE: &str = "trust-store";

/// The namespace label every trust store id CloudFront mints carries.
const TRUST_STORE_ID_LABEL: &str = "ts_";

/// The statuses CloudFront reports a trust store it has accepted in
/// (`smithy_query(query="shape",shape="TrustStoreStatus")`, whose values are `pending`, `active`
/// and `failed`): a create answers the transient one and the clock owes the terminal one, so a
/// read taken at either instant is one of these two and `failed` is neither.
const ACCEPTED_STATUSES: &[&str] = &["pending", "active"];

/// The error code CloudFront answers for an identifier no trust store answers to.
const ENTITY_NOT_FOUND: &str = "EntityNotFound";

/// The `HandlerErrorCode` class a provider reports a refusal of the REQUEST it was handed under
/// — a value of the model's own `HandlerErrorCode` enum.
const INVALID_REQUEST_CLASS: &str = "InvalidRequest";

/// The resource status a refused create leaves the resource in.
const CREATE_FAILED: &str = "CREATE_FAILED";

/// The SDK's own name for the service that owns the type, and the statuses it answers a CA
/// certificates bundle it cannot use with (`InvalidArgument`, HTTP 400) and an address it holds
/// nothing at (`EntityNotFound`, HTTP 404).
const CLOUDFRONT_SDK_NAME: &str = "CloudFront";
const INVALID_ARGUMENT_STATUS: u16 = 400;
const ENTITY_NOT_FOUND_STATUS: u16 = 404;

/// The `HandlerErrorCode` class a provider reports a refusal naming a resource that is not there
/// under — a value of the model's own `HandlerErrorCode` enum.
const NOT_FOUND_CLASS: &str = "NotFound";

/// The three tags CloudFormation records about a resource it provisioned, which a trust store's
/// provider does not apply.
const PROVENANCE_TAG_KEYS: &[&str] = &[
    "aws:cloudformation:stack-name",
    "aws:cloudformation:stack-id",
    "aws:cloudformation:logical-id",
];

/// The bucket every CA certificates bundle below is staged in.
const BUNDLE_BUCKET: &str = "cfn-cloudfront-trust-store-bundles";

/// The key the bundle a row's create reads lives under, and the one its update rotates onto.
const CREATED_BUNDLE: &str = "created/ca-bundle.pem";
const ROTATED_BUNDLE: &str = "rotated/ca-bundle.pem";

/// The key the object that is no CA certificates bundle at all is staged under.
const UNREADABLE_BUNDLE: &str = "broken/not-a-bundle.pem";

// ---------------------------------------------------------------------------
// The declarations under test
// ---------------------------------------------------------------------------

/// One row: the stack it deploys, what its trust store declares, and how many CA certificates
/// the bundles its create and its update read hold.
struct Row {
    case: &'static str,
    stack: &'static str,
    /// `TrustStore$Name`: the name the account knows the trust store by.
    name: &'static str,
    /// `UseClientCertificateOCSPEndpoint`, or `None` for a declaration that states it nowhere.
    ocsp_endpoint: Option<bool>,
    /// The tags the declaration asks for, which may be none at all.
    declared_tags: &'static [(&'static str, &'static str)],
    /// The tags the stack OPERATION supplies, which CloudFormation propagates to the resources
    /// it creates.
    stack_tags: &'static [(&'static str, &'static str)],
    created_certificates: usize,
    rotated_certificates: usize,
}

const ROWS: &[Row] = &[
    Row {
        case: "a one-certificate bundle, the OCSP endpoint asked for, declared and stack tags, \
               rotated onto a three-certificate bundle",
        stack: "cfn-cf-ts-ocsp",
        name: "cfn-cf-ts-ocsp",
        ocsp_endpoint: Some(true),
        declared_tags: &[("plane", "viewer-mtls"), ("owner", "orders-team")],
        stack_tags: &[("cost-centre", "1234")],
        created_certificates: 1,
        rotated_certificates: 3,
    },
    Row {
        case: "a three-certificate bundle, the OCSP endpoint stated nowhere, no tags at all, \
               rotated onto a two-certificate bundle",
        stack: "cfn-cf-ts-plain",
        name: "cfn_cf.ts-plain-2",
        ocsp_endpoint: None,
        declared_tags: &[],
        stack_tags: &[],
        created_certificates: 3,
        rotated_certificates: 2,
    },
    Row {
        case: "a two-certificate bundle, the OCSP endpoint turned off, one declared tag, \
               rotated onto a one-certificate bundle",
        stack: "cfn-cf-ts-no-ocsp",
        name: "CfnCfTsNoOcsp",
        ocsp_endpoint: Some(false),
        declared_tags: &[("plane", "viewer-mtls")],
        stack_tags: &[],
        created_certificates: 2,
        rotated_certificates: 1,
    },
];

/// The declared properties of one row's trust store, reading the bundle at `bundle_key`.
fn properties(row: &Row, bundle_key: &str) -> Value {
    let mut declared = ::serde_json::Map::new();
    declared.insert("Name".to_string(), json!(row.name));
    declared.insert(
        "CaCertificatesBundleSource".to_string(),
        json!({
            "CaCertificatesBundleS3Location": {
                "Bucket": BUNDLE_BUCKET,
                "Key": bundle_key,
                "Region": REGION,
            },
        }),
    );
    if let Some(ocsp) = row.ocsp_endpoint {
        declared.insert("UseClientCertificateOCSPEndpoint".to_string(), json!(ocsp));
    }
    if !row.declared_tags.is_empty() {
        let tags: Vec<Value> = row
            .declared_tags
            .iter()
            .map(|(key, value)| json!({"Key": key, "Value": value}))
            .collect();
        declared.insert("Tags".to_string(), Value::Array(tags));
    }
    Value::Object(declared)
}

/// The whole template one row deploys at `bundle_key`: the trust store, and the outputs a client
/// reads the deployment's identifiers out of.
fn template(row: &Row, bundle_key: &str) -> String {
    json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Resources": {
            LOGICAL_ID: {"Type": TRUST_STORE_TYPE, "Properties": properties(row, bundle_key)},
        },
        "Outputs": {
            "StoreRef": {"Value": {"Ref": LOGICAL_ID}},
            "StoreId": {"Value": {"Fn::GetAtt": [LOGICAL_ID, "Id"]}},
            "StoreArn": {"Value": {"Fn::GetAtt": [LOGICAL_ID, "Arn"]}},
            "StoreStatus": {"Value": {"Fn::GetAtt": [LOGICAL_ID, "Status"]}},
            "CaCertificates": {
                "Value": {"Fn::GetAtt": [LOGICAL_ID, "NumberOfCaCertificates"]},
            },
        },
    })
    .to_string()
}

/// The stack-level `Tags` members one row's stack operation carries.
fn stack_tag_members(row: &Row) -> Vec<(String, String)> {
    row.stack_tags
        .iter()
        .enumerate()
        .flat_map(|(index, (key, value))| {
            let position = index + 1;
            [
                (
                    format!("Tags.member.{position}.Key"),
                    (*key).to_string(),
                ),
                (
                    format!("Tags.member.{position}.Value"),
                    (*value).to_string(),
                ),
            ]
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Staging the bundles a trust store is built from
// ---------------------------------------------------------------------------

/// A bundle of `count` real self-signed CA certificates, PEM encoded one after another — what
/// `openssl` produces for a CA bundle and what a caller stages in Amazon S3.
fn ca_bundle(count: usize) -> String {
    (0..count)
        .map(|index| {
            let key = rcgen::KeyPair::generate().expect("a key pair");
            let mut params = rcgen::CertificateParams::new(vec![format!("ca{index}.example.com")])
                .expect("certificate parameters");
            params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
            params
                .self_signed(&key)
                .expect("a self-signed certificate")
                .pem()
        })
        .collect()
}

/// Create [`BUNDLE_BUCKET`] and stage one object, through Amazon S3's own wire: the object a
/// trust store is built from is one a caller really wrote.
async fn stage(router: &MultiServiceRouter, case: &str, key: &str, body: &[u8]) {
    let created = s3(router, "PUT", &format!("/{BUNDLE_BUCKET}"), b"").await;
    assert!(
        (200..300).contains(&created.status) || created.status == 409,
        "case {case:?}: the bundle bucket must be there first, got {} {}",
        created.status,
        created.body
    );
    let staged = s3(router, "PUT", &format!("/{BUNDLE_BUCKET}/{key}"), body).await;
    assert!(
        (200..300).contains(&staged.status),
        "case {case:?}: the object {key} must be staged first, got {} {}",
        staged.status,
        staged.body
    );
}

// ---------------------------------------------------------------------------
// Reading the trust store back from Amazon CloudFront
// ---------------------------------------------------------------------------

/// What CloudFront reports for one trust store identifier: its name, status and certificate
/// count, or the status and code it refused the read with.
async fn get_trust_store(
    router: &MultiServiceRouter,
    identifier: &str,
) -> Result<(String, String, String), (u16, String)> {
    let (code, body) = cloudfront(
        router,
        "GET",
        &format!("/{CLOUDFRONT_API_VERSION}/{TRUST_STORE_ARN_TYPE}/{identifier}"),
    )
    .await;
    if code != 200 {
        return Err((code, xml_text(&body, "Code").unwrap_or_default()));
    }
    Ok((
        xml_text(&body, "Name").unwrap_or_default(),
        xml_text(&body, "Status").unwrap_or_default(),
        xml_text(&body, "NumberOfCaCertificates").unwrap_or_default(),
    ))
}

/// The tags CloudFront holds for one resource, as `(key, value)` pairs.
async fn tags_for(router: &MultiServiceRouter, case: &str, arn: &str) -> Vec<(String, String)> {
    let (code, body) = cloudfront(
        router,
        "GET",
        &format!("/{CLOUDFRONT_API_VERSION}/tagging?Resource={arn}"),
    )
    .await;
    assert_eq!(
        code, 200,
        "case {case:?}: ListTagsForResource({arn}) must answer 200, got {code} {body}"
    );
    xml_blocks(&body, "Tag")
        .into_iter()
        .map(|tag| {
            (
                xml_text(&tag, "Key").unwrap_or_default(),
                xml_text(&tag, "Value").unwrap_or_default(),
            )
        })
        .collect()
}

/// One stack output, or a panic naming the body that carried none.
fn output(case: &str, described: &str, key: &str) -> String {
    output_value(described, key)
        .unwrap_or_else(|| panic!("case {case:?}: no {key:?} output, got {described}"))
}

// ---------------------------------------------------------------------------
// The rule
// ---------------------------------------------------------------------------

/// Every declared trust store is accepted, EXISTS in Amazon CloudFront under the physical
/// resource id the stack reports for it, carries what the template declared and the certificates
/// of the bundle that service read, takes a rotation of that bundle in place, and goes away with
/// the stack.
#[tokio::test]
async fn declared_trust_stores_are_created_read_updated_and_deleted_in_cloudfront() {
    for row in ROWS {
        let case = row.case;
        let router = router();
        stage(
            &router,
            case,
            CREATED_BUNDLE,
            ca_bundle(row.created_certificates).as_bytes(),
        )
        .await;
        stage(
            &router,
            case,
            ROTATED_BUNDLE,
            ca_bundle(row.rotated_certificates).as_bytes(),
        )
        .await;

        let tags: Vec<(String, String)> = stack_tag_members(row);
        let members: Vec<(&str, &str)> = tags
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str()))
            .collect();
        let sent: Vec<_> = members
            .iter()
            .map(|(name, value)| member(name, value))
            .collect();
        let (code, body) = create_stack(
            &router,
            row.stack,
            &template(row, CREATED_BUNDLE),
            &sent,
        )
        .await;
        // The issue's own observable: a template declaring the type must not be refused as one
        // whose resource types CloudFormation does not recognise.
        assert_eq!(
            code, 200,
            "case {case:?}: CreateStack must be accepted, got {code} {body}"
        );

        let described = settled_stack(&router, case, row.stack).await;
        assert_eq!(
            xml_text(&described, "StackStatus").as_deref(),
            Some(status::CREATE_COMPLETE),
            "case {case:?}: the stack must reach CREATE_COMPLETE, got {:?} ({:?})",
            xml_text(&described, "StackStatus"),
            xml_text(&described, "StackStatusReason"),
        );

        // 1. The identity the stack published is the id CloudFront minted, and the attributes
        //    that carry it agree with it.
        let published = physical_resource_id(&router, case, row.stack, LOGICAL_ID).await;
        let body = published
            .strip_prefix(TRUST_STORE_ID_LABEL)
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: expected the namespace-labelled id CloudFront mints for a \
                     trust store, got {published:?}"
                )
            });
        assert!(
            !body.is_empty() && body.chars().all(|c| c.is_ascii_alphanumeric()),
            "case {case:?}: expected an alphanumeric id body after \
             {TRUST_STORE_ID_LABEL:?}, got {published:?}"
        );
        let expected_arn =
            format!("arn:aws:cloudfront::{ACCOUNT_ID}:{TRUST_STORE_ARN_TYPE}/{published}");
        for (key, expected) in [
            ("StoreRef", published.as_str()),
            ("StoreId", published.as_str()),
            ("StoreArn", expected_arn.as_str()),
        ] {
            let value = output(case, &described, key);
            assert_eq!(
                value, expected,
                "case {case:?}: expected the {key:?} output to be {expected:?}, got {value:?}"
            );
        }

        // 2. The read the issue filed: the id the stack published resolves in the service that
        //    owns the trust store, which reports it under the declared name with the
        //    certificates it counted in the staged bundle.
        let reported = get_trust_store(&router, &published).await;
        let (name, reported_status, counted) = reported.clone().unwrap_or_else(|refused| {
            panic!(
                "case {case:?}: expected CloudFront to hold {published:?}, got {refused:?} — a \
                 CREATE_COMPLETE stack must not publish an id that resolves to nothing"
            )
        });
        assert_eq!(
            (name.as_str(), counted.as_str()),
            (row.name, row.created_certificates.to_string().as_str()),
            "case {case:?}: expected the declared name and the counted certificates, got \
             {reported:?}"
        );
        assert!(
            ACCEPTED_STATUSES.contains(&reported_status.as_str()),
            "case {case:?}: expected one of {ACCEPTED_STATUSES:?}, got {reported_status:?}"
        );
        // …and the attributes the stack publishes are the values that service reported.
        let published_count = output(case, &described, "CaCertificates");
        assert_eq!(
            published_count,
            row.created_certificates.to_string(),
            "case {case:?}: expected the published NumberOfCaCertificates to be the count \
             CloudFront reported, got {published_count:?}"
        );
        let published_status = output(case, &described, "StoreStatus");
        assert!(
            ACCEPTED_STATUSES.contains(&published_status.as_str()),
            "case {case:?}: expected the published Status to be one of {ACCEPTED_STATUSES:?}, \
             got {published_status:?}"
        );

        // 3. The tags the trust store carries are the declaration's own and the stack
        //    operation's, and NEVER the three records CloudFormation keeps about the resource.
        let held = tags_for(&router, case, &expected_arn).await;
        let keys: BTreeSet<&str> = held.iter().map(|(key, _)| key.as_str()).collect();
        let asked: BTreeSet<&str> = row
            .declared_tags
            .iter()
            .chain(row.stack_tags.iter())
            .map(|(key, _)| *key)
            .collect();
        assert_eq!(
            keys, asked,
            "case {case:?}: expected exactly the declared and stack-level tag keys, got {held:?}"
        );
        for (key, value) in row.declared_tags.iter().chain(row.stack_tags.iter()) {
            assert!(
                held.iter()
                    .any(|(held_key, held_value)| held_key == key && held_value == value),
                "case {case:?}: expected the tag {key:?}={value:?}, got {held:?}"
            );
        }
        for withheld in PROVENANCE_TAG_KEYS {
            assert!(
                !keys.contains(withheld),
                "case {case:?}: expected no {withheld:?} tag on the trust store, got {held:?}"
            );
        }

        // 4. An update that re-points the declaration at another bundle is applied through the
        //    owning service's own update: the trust store keeps its identity and reports the
        //    certificates of the bundle it now reads.
        let mut updated = vec![
            member("StackName", row.stack),
            member("TemplateBody", &template(row, ROTATED_BUNDLE)),
        ];
        updated.extend(members.iter().map(|(name, value)| member(name, value)));
        let (code, body) = call(&router, "UpdateStack", &updated).await;
        assert_eq!(
            code, 200,
            "case {case:?}: UpdateStack must be accepted, got {code} {body}"
        );
        let described = settled_stack(&router, case, row.stack).await;
        assert_eq!(
            xml_text(&described, "StackStatus").as_deref(),
            Some(status::UPDATE_COMPLETE),
            "case {case:?}: the rotation must complete, got {:?} ({:?})",
            xml_text(&described, "StackStatus"),
            xml_text(&described, "StackStatusReason"),
        );
        assert_eq!(
            output(case, &described, "StoreRef"),
            published,
            "case {case:?}: expected the rotation to leave the trust store's identity where it \
             was, got {described}"
        );
        let rotated = get_trust_store(&router, &published).await;
        let (name, _, counted) = rotated.clone().unwrap_or_else(|refused| {
            panic!("case {case:?}: expected the rotated trust store to resolve, got {refused:?}")
        });
        assert_eq!(
            (name.as_str(), counted.as_str()),
            (row.name, row.rotated_certificates.to_string().as_str()),
            "case {case:?}: expected the certificates of the bundle the update named, got \
             {rotated:?}"
        );
        let published_count = output(case, &described, "CaCertificates");
        assert_eq!(
            published_count,
            row.rotated_certificates.to_string(),
            "case {case:?}: expected the published count to follow the rotation, got \
             {published_count:?}"
        );

        // 5. The teardown takes the trust store away again: a resource the stack created and no
        //    longer records must not be left behind in the service that holds it.
        let (code, body) = call(&router, "DeleteStack", &[member("StackName", row.stack)]).await;
        assert_eq!(
            code, 200,
            "case {case:?}: DeleteStack must be accepted, got {code} {body}"
        );
        let described = settled_stack(&router, case, row.stack).await;
        assert!(
            described.is_empty()
                || xml_text(&described, "StackStatus").as_deref() == Some(status::DELETE_COMPLETE),
            "case {case:?}: the teardown must complete, got {described}"
        );
        let gone = get_trust_store(&router, &published).await;
        assert_eq!(
            gone,
            Err((404, ENTITY_NOT_FOUND.to_string())),
            "case {case:?}: expected the torn-down trust store to be gone from CloudFront, got \
             {gone:?}"
        );
    }
}

/// One bundle the owning service will not build a trust store from: what is staged at the
/// address the declaration names, the status and `HandlerErrorCode` class that service's refusal
/// falls in, and whether this type's provider re-raises it inside its invalid-request exception.
struct Refused {
    case: &'static str,
    stack: &'static str,
    /// The object staged at [`UNREADABLE_BUNDLE`], or `None` for a row that stages nothing
    /// there at all.
    staged: Option<&'static [u8]>,
    status_answered: u16,
    expected_class: &'static str,
    enveloped: bool,
}

/// A CA certificates bundle CloudFront will not take is refused where the RESOURCE is
/// provisioned — by the service that owns the type — rather than by template parsing:
/// `CreateStack` is accepted, the resource reaches `CREATE_FAILED` carrying that service's own
/// sentence inside the provider's invalid-request exception, the stack rolls back, and nothing is
/// left in CloudFront.
#[tokio::test]
async fn a_ca_bundle_cloudfront_will_not_take_fails_the_resource_and_rolls_the_stack_back() {
    // Rows: an object whose bytes are no PEM at all and one whose PEM block is no certificate —
    // each a bundle CloudFront refuses as an invalid ARGUMENT, which the provider re-raises as
    // its invalid-request exception — and an address the object store holds nothing at, which
    // that service answers as a missing ENTITY: a refusal of another class, which travels as it
    // came.
    let rows: &[Refused] = &[
        Refused {
            case: "an object that is no PEM at all",
            stack: "cfn-cf-ts-not-pem",
            staged: Some(b"this is not a certificate bundle"),
            status_answered: INVALID_ARGUMENT_STATUS,
            expected_class: INVALID_REQUEST_CLASS,
            enveloped: true,
        },
        Refused {
            case: "a PEM block that is no certificate",
            stack: "cfn-cf-ts-not-a-certificate",
            staged: Some(b"-----BEGIN PUBLIC KEY-----\nMEE=\n-----END PUBLIC KEY-----\n"),
            status_answered: INVALID_ARGUMENT_STATUS,
            expected_class: INVALID_REQUEST_CLASS,
            enveloped: true,
        },
        Refused {
            case: "an address the object store holds nothing at",
            stack: "cfn-cf-ts-absent",
            staged: None,
            status_answered: ENTITY_NOT_FOUND_STATUS,
            expected_class: NOT_FOUND_CLASS,
            enveloped: false,
        },
    ];

    for refused in rows {
        let Refused {
            case,
            stack,
            staged,
            status_answered,
            expected_class,
            enveloped,
        } = refused;
        let router = router();
        let row = Row {
            case,
            stack,
            name: stack,
            ocsp_endpoint: None,
            declared_tags: &[],
            stack_tags: &[],
            created_certificates: 0,
            rotated_certificates: 0,
        };
        if let Some(bytes) = staged {
            stage(&router, case, UNREADABLE_BUNDLE, bytes).await;
        } else {
            // The bucket exists and the key does not, which is the store answering that it holds
            // nothing at that address.
            stage(&router, case, "other/object", b"x").await;
        }

        let (code, body) =
            create_stack(&router, stack, &template(&row, UNREADABLE_BUNDLE), &[]).await;
        assert_eq!(
            code, 200,
            "case {case:?}: the template is well formed, so CreateStack must be accepted — the \
             bundle is the resource's to refuse — got {code} {body}"
        );

        let described = settled_stack(&router, case, stack).await;
        assert_eq!(
            xml_text(&described, "StackStatus").as_deref(),
            Some(status::ROLLBACK_COMPLETE),
            "case {case:?}: expected the refused create to be rolled back, got {described}"
        );

        let reason =
            failed_resource_reason(case, &router, REGION, stack, LOGICAL_ID, CREATE_FAILED).await;
        let (quoted, class) = quoted_and_class(case, &reason);
        // A refusal of the REQUEST is re-raised by this type's provider as the resource-provider
        // plugin's invalid-request exception, naming the type it manages; a refusal of any OTHER
        // class travels as the owning service worded it.
        let opens = format!("Invalid request provided: {TRUST_STORE_TYPE}: ");
        let sentence = if *enveloped {
            quoted.strip_prefix(&opens).unwrap_or_else(|| {
                panic!("case {case:?}: expected the reason to open with {opens:?}, got {quoted:?}")
            })
        } else {
            assert!(
                !quoted.starts_with(&opens),
                "case {case:?}: expected a refusal of another class to carry no invalid-request \
                 exception, got {quoted:?}"
            );
            quoted.as_str()
        };
        assert!(
            !sentence.trim().is_empty(),
            "case {case:?}: expected CloudFront's own sentence in the reason, got {quoted:?}"
        );
        assert_sdk_diagnostic_with_attempts(case, &quoted, CLOUDFRONT_SDK_NAME, *status_answered);
        assert_eq!(
            class, *expected_class,
            "case {case:?}: expected the {expected_class} class for what the provider answered, \
             got {class:?} in {reason:?}"
        );

        // Nothing is left behind: the trust store the declaration asked for was never created,
        // so no read of the account's trust stores reports one under the declared name.
        let answered = get_trust_store(&router, row.name).await;
        assert_eq!(
            answered,
            Err((404, ENTITY_NOT_FOUND.to_string())),
            "case {case:?}: expected CloudFront to hold no trust store under the declared name, \
             got {answered:?}"
        );
    }
}

/// A declaration missing the bundle the trust store is built from is refused by the provider,
/// naming the property, and nothing is asked of CloudFront.
#[tokio::test]
async fn a_declaration_without_the_bundle_source_fails_the_resource_naming_the_property() {
    let router = router();
    let stack = "cfn-cf-ts-no-source";
    let template = json!({
        "Resources": {
            LOGICAL_ID: {
                "Type": TRUST_STORE_TYPE,
                "Properties": {"Name": stack},
            },
        },
    })
    .to_string();
    let case = "a declaration that states no CaCertificatesBundleSource";

    let (code, body) = create_stack(&router, stack, &template, &[]).await;
    assert_eq!(
        code, 200,
        "case {case:?}: the template is well formed, so CreateStack must be accepted, got \
         {code} {body}"
    );
    let described = settled_stack(&router, case, stack).await;
    assert_eq!(
        xml_text(&described, "StackStatus").as_deref(),
        Some(status::ROLLBACK_COMPLETE),
        "case {case:?}: expected the refused create to be rolled back, got {described}"
    );
    let reason =
        failed_resource_reason(case, &router, REGION, stack, LOGICAL_ID, CREATE_FAILED).await;
    let (quoted, _) = quoted_and_class(case, &reason);
    assert_eq!(
        quoted,
        format!(
            "Property CaCertificatesBundleSource cannot be empty for resource of type \
             {TRUST_STORE_TYPE}"
        ),
        "case {case:?}: expected the refusal to name the missing property, got {quoted:?}"
    );
    let answered = get_trust_store(&router, stack).await;
    assert_eq!(
        answered,
        Err((404, ENTITY_NOT_FOUND.to_string())),
        "case {case:?}: expected CloudFront to hold no trust store, got {answered:?}"
    );
}

/// The complement, so the refusal the issue reported is still the refusal for a type
/// CloudFormation really does not know: a template declaring one is answered `ValidationError`
/// with `Template format error: Unrecognized resource types: [<types>]` and creates nothing.
#[tokio::test]
async fn a_cloudfront_type_cloudformation_does_not_publish_is_still_unrecognized() {
    let router = router();
    for (case, spelled) in [
        (
            "a misspelling of the published type",
            "AWS::CloudFront::Truststore",
        ),
        (
            "a CloudFront resource with no type",
            "AWS::CloudFront::TrustStoreNothing",
        ),
    ] {
        let template = json!({
            "Resources": {LOGICAL_ID: {"Type": spelled, "Properties": {}}}
        })
        .to_string();
        let (code, body) = create_stack(&router, "cfn-cf-ts-unknown", &template, &[]).await;
        assert_eq!(
            code, 400,
            "case {case:?}: an unrecognized type must be refused, got {code} {body}"
        );
        assert_eq!(
            xml_text(&body, "Code").as_deref(),
            Some("ValidationError"),
            "case {case:?}: the refusal must be a ValidationError, got {body}"
        );
        assert_eq!(
            xml_text(&body, "Message").as_deref(),
            Some(
                format!("Template format error: Unrecognized resource types: [{spelled}]")
                    .as_str()
            ),
            "case {case:?}: the refusal must name the type that is not recognised, got {body}"
        );
    }
}
