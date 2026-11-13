// SPDX-License-Identifier: CC0-1.0

//! `DeleteSAMLProvider`, end to end over the awsQuery wire, cross-read through
//! `GetSAMLProvider`, `ListSAMLProviderTags` and `ListSAMLProviders`.
//!
//! Every assertion drives a `MultiServiceRouter` holding this service over one shared
//! `InMemoryEngine`, exactly as the production router does, so what is under test is the
//! HTTP behaviour (status, awsQuery error code, XML members) of providers that were
//! PERSISTED by `CreateSAMLProvider`, never a handler called directly. The awsQuery
//! harness is the one `tests/all/user_lifecycle.rs` owns and the other suites in this
//! crate share, and the metadata documents are the ones `tests/all/saml_providers.rs`
//! builds.
//!
//! The rule under test is that the delete removes the provider the ARN names from the
//! account and nothing else: afterwards no read reports it, the listing no longer carries
//! it, its name is free for a create that mints a NEW record, every other provider the
//! account holds reads back unchanged, and a role whose trust policy names the deleted
//! provider's ARN keeps that document — "deleting the provider resource from IAM does not
//! update any roles that reference the SAML provider resource's ARN as a principal in
//! their trust policies". An ARN naming no held provider is refused as absent and a value
//! that is not an ARN as invalid, with nothing removed either way.
//!
//! Each suite is a TABLE whose rows differ along what the rule generalises over: the
//! provider name's alphabet and its length at the modelled bounds, how the metadata
//! document is spelled, whether it states an expiration and in which form, the encryption
//! mode, whether a private key was uploaded and the tag count — so no single request is
//! the specification.

use std::collections::BTreeMap;

use smithy_server_runtime::MultiServiceRouter;

use crate::role_lifecycle::{borrowed, optional, param, reported_tags};
use crate::saml_providers::{MetadataForm, expected_arn, metadata_document};
use crate::user_lifecycle::{call, element, elements, error_code, router};

/// How many characters a provider name may carry: `SAMLProviderNameType` is len 1..128
/// (`smithy_query(query="operation", operation="CreateSAMLProvider")`), so the table holds
/// a name at each published bound.
const MAX_PROVIDER_NAME_CHARACTERS: usize = 128;

/// One provider the account is made to hold, with the members the row exercises.
struct DeletionCase {
    /// `Name` — pattern `^[\w._-]+$`, len 1..128 (`CreateSAMLProvider$Name`).
    name: String,
    /// How the row's metadata document is spelled.
    form: MetadataForm,
    /// The issuer the document names in `entityID`.
    entity_id: &'static str,
    /// The document's `validUntil`, when it states one.
    valid_until: Option<&'static str>,
    /// `AssertionEncryptionMode` — one value of `assertionEncryptionModeType`, or none.
    encryption_mode: Option<&'static str>,
    /// Whether the create uploads a private key in `AddPrivateKey`.
    private_key: bool,
    /// The tags the create carries, in request order.
    tags: Vec<(&'static str, &'static str)>,
}

impl DeletionCase {
    fn document(&self) -> String {
        metadata_document(self.form, self.entity_id, self.valid_until)
    }

    fn params(&self) -> Vec<(String, String)> {
        let mut params = vec![
            param("Name", &self.name),
            param("SAMLMetadataDocument", &self.document()),
        ];
        if let Some(mode) = self.encryption_mode {
            params.push(param("AssertionEncryptionMode", mode));
        }
        if self.private_key {
            // "The private key must be a .pem file" (`CreateSAMLProvider$AddPrivateKey`),
            // whose pattern `^[\u0009\u000A\u000D\u0020-\u00FF]+$` admits this ASCII body.
            params.push(param(
                "AddPrivateKey",
                &format!(
                    "-----BEGIN PRIVATE KEY----- {} -----END PRIVATE KEY-----",
                    "QUJDREVGR0hJSktMTU5PUFFSU1RVVldYWVph".repeat(8)
                ),
            ));
        }
        for (index, (key, value)) in self.tags.iter().enumerate() {
            let position = index + 1;
            params.push(param(&format!("Tags.member.{position}.Key"), key));
            params.push(param(&format!("Tags.member.{position}.Value"), value));
        }
        params
    }

    fn expected_tags(&self) -> BTreeMap<String, String> {
        self.tags
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect()
    }
}

/// The providers this suite's rows create, in the order they are created.
fn deletion_cases() -> Vec<DeletionCase> {
    vec![
        DeletionCase {
            name: "DeletedIdP".to_string(),
            form: MetadataForm::Entity,
            entity_id: "https://idp.example.com/saml",
            valid_until: Some("2032-05-30T09:01:31Z"),
            encryption_mode: None,
            private_key: false,
            tags: Vec::new(),
        },
        DeletionCase {
            name: "corp_idp-2.eu".to_string(),
            form: MetadataForm::PrefixedEntity,
            entity_id: "urn:example:idp:corp",
            valid_until: None,
            encryption_mode: Some("Allowed"),
            private_key: false,
            tags: vec![("env", "dev")],
        },
        DeletionCase {
            name: "Federation.Partner_7".to_string(),
            form: MetadataForm::EntityGroup,
            entity_id: "https://partner.example.org/metadata",
            valid_until: Some("2030-01-02T03:04:05.678Z"),
            encryption_mode: Some("Required"),
            private_key: true,
            tags: vec![("team", "platform"), ("env", "prod")],
        },
        DeletionCase {
            // The shortest name the model admits.
            name: "a".to_string(),
            form: MetadataForm::Entity,
            entity_id: "https://single.example.net/idp",
            // Two hours EAST of UTC, so the instant is two hours EARLIER in the `Z` form.
            valid_until: Some("2035-12-31T23:59:59+02:00"),
            encryption_mode: None,
            private_key: true,
            tags: vec![("zeta", "last"), ("alpha", "first"), ("mu", "middle")],
        },
        DeletionCase {
            // The longest name the model admits.
            name: "Z".repeat(MAX_PROVIDER_NAME_CHARACTERS),
            form: MetadataForm::EntityGroup,
            entity_id: "urn:example:federation:group",
            valid_until: None,
            encryption_mode: Some("Required"),
            private_key: false,
            tags: vec![("only", "one")],
        },
        DeletionCase {
            name: "ZoneLessExpiry".to_string(),
            form: MetadataForm::PrefixedEntity,
            entity_id: "https://zoneless.example.com/saml",
            // An `xs:dateTime` stating no zone names an instant all the same.
            valid_until: Some("2031-07-04T12:00:00"),
            encryption_mode: Some("Allowed"),
            private_key: true,
            tags: vec![("b", "second"), ("a", "first")],
        },
    ]
}

/// Create one fixture provider, answering the ARN the create reports.
async fn create_provider(router: &MultiServiceRouter, case: &DeletionCase) -> String {
    let params = case.params();
    let (status, body) = call(router, "CreateSAMLProvider", &borrowed(&params)).await;
    assert_eq!(
        status, 200,
        "case {:?}: the fixture CreateSAMLProvider must succeed, got {status}: {body}",
        case.name
    );
    element(&body, "SAMLProviderArn")
}

/// One `DeleteSAMLProvider` call.
async fn delete_provider(router: &MultiServiceRouter, arn: &str) -> (u16, String) {
    let params = vec![param("SAMLProviderArn", arn)];
    call(router, "DeleteSAMLProvider", &borrowed(&params)).await
}

/// One `GetSAMLProvider` call.
async fn read_provider(router: &MultiServiceRouter, arn: &str) -> (u16, String) {
    let params = vec![param("SAMLProviderArn", arn)];
    call(router, "GetSAMLProvider", &borrowed(&params)).await
}

/// The ARNs `ListSAMLProviders` reports, sorted, so the set is independent of the order
/// the creates and deletes arrived in.
async fn held_arns(router: &MultiServiceRouter, case: &str) -> Vec<String> {
    let (status, body) = call(router, "ListSAMLProviders", &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from ListSAMLProviders, got {status} with body {body}"
    );
    let mut arns = elements(&body, "Arn");
    arns.sort();
    arns
}

/// Assert the provider the ARN names reads back as the record `case` created.
async fn assert_reads_back(router: &MultiServiceRouter, case: &DeletionCase, arn: &str) {
    let (status, read) = read_provider(router, arn).await;
    assert_eq!(
        status, 200,
        "case {:?}: the held provider must still read back, got {status}: {read}",
        case.name
    );
    assert_eq!(
        element(&read, "SAMLMetadataDocument"),
        case.document(),
        "case {:?}: deleting another provider must leave the held document alone, got {read}",
        case.name
    );
    assert_eq!(
        optional(&read, "AssertionEncryptionMode").as_deref(),
        case.encryption_mode,
        "case {:?}: deleting another provider must leave the held encryption mode alone, got \
         {read}",
        case.name
    );
    assert_eq!(
        elements(&read, "KeyId").len(),
        usize::from(case.private_key),
        "case {:?}: deleting another provider must leave the held private key metadata alone, \
         got {read}",
        case.name
    );
    assert_eq!(
        reported_tags(&read),
        case.expected_tags(),
        "case {:?}: deleting another provider must leave the held tags alone, got {read}",
        case.name
    );
}

// ---------------------------------------------------------------------------
// The named provider, and nothing else
// ---------------------------------------------------------------------------

/// A delete removes the provider the ARN names: afterwards the read reports it absent
/// (`NoSuchEntity`, HTTP 404), its tags are gone with it, the listing no longer carries
/// it — and every other provider the account holds reads back exactly as its create wrote
/// it. The providers are deleted in an order that is NOT the order they were created in,
/// so the rule cannot depend on removing the newest or the oldest entry.
#[tokio::test]
async fn deleting_a_provider_removes_that_provider_and_leaves_every_other_one() {
    let router = router();
    let cases = deletion_cases();
    let mut arns: Vec<String> = Vec::new();
    for case in &cases {
        arns.push(create_provider(&router, case).await);
    }

    let mut expected = arns.clone();
    expected.sort();
    assert_eq!(
        held_arns(&router, "every provider created").await,
        expected,
        "the account must hold every created provider before any delete"
    );

    // An order that is neither the creation order nor its reverse.
    for index in [2usize, 0, 5, 1, 4, 3] {
        let case = &cases[index];
        let arn = &arns[index];

        let (status, body) = delete_provider(&router, arn).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200 from DeleteSAMLProvider, got {status} with body {body}",
            case.name
        );

        let (status, read) = read_provider(&router, arn).await;
        assert_eq!(
            status, 404,
            "case {:?}: a deleted provider must not read back, got {status}: {read}",
            case.name
        );
        assert_eq!(
            error_code(&read),
            "NoSuchEntity",
            "case {:?}: expected NoSuchEntity, got {read}",
            case.name
        );

        // The tags went with the record: the provider they hung off is gone, so the tag
        // read is the same "entity absent" answer.
        let tag_params = vec![param("SAMLProviderArn", arn)];
        let (status, tags) = call(
            &router,
            "ListSAMLProviderTags",
            &borrowed(&tag_params),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {:?}: a deleted provider's tags must not read back, got {status}: {tags}",
            case.name
        );

        expected.retain(|held| held != arn);
        assert_eq!(
            held_arns(&router, &case.name).await,
            expected,
            "case {:?}: the listing must report exactly the providers still held",
            case.name
        );

        // Every provider not yet deleted still reports the record its create wrote.
        for (other_index, other) in cases.iter().enumerate() {
            if expected.contains(&arns[other_index]) {
                assert_reads_back(&router, other, &arns[other_index]).await;
            }
        }
    }

    assert!(
        held_arns(&router, "every provider deleted").await.is_empty(),
        "an account whose every provider was deleted must hold none"
    );
}

/// A deleted name is FREE: a later create for it is served and mints a new record, which
/// reports the document, expiration, encryption mode and tags THAT create carried — never
/// anything the deleted provider held — under its own provider uuid.
#[tokio::test]
async fn a_deleted_name_can_be_created_again_as_a_new_record() {
    let router = router();
    let mut uuids: Vec<String> = Vec::new();

    for case in deletion_cases() {
        let arn = create_provider(&router, &case).await;
        let (status, first) = read_provider(&router, &arn).await;
        assert_eq!(
            status, 200,
            "case {:?}: the fixture provider must read back, got {status}: {first}",
            case.name
        );
        let first_uuid = element(&first, "SAMLProviderUUID");

        let (status, body) = delete_provider(&router, &arn).await;
        assert_eq!(
            status, 200,
            "case {:?}: the delete must be served, got {status} with body {body}",
            case.name
        );

        // The second create carries a DIFFERENT document, expiration, encryption mode and
        // tags from the deleted provider's, so a record that survived the delete would be
        // visible in the read below.
        let recreated = DeletionCase {
            name: case.name.clone(),
            form: MetadataForm::EntityGroup,
            entity_id: "https://recreated.example.com/saml",
            valid_until: Some("2033-03-03T03:03:03Z"),
            encryption_mode: Some("Required"),
            private_key: true,
            tags: vec![("recreated", "yes")],
        };
        let new_arn = create_provider(&router, &recreated).await;
        assert_eq!(
            new_arn,
            expected_arn(&case.name),
            "case {:?}: the ARN is derived from the name, so the recreated provider keeps it, \
             got {new_arn:?}",
            case.name
        );
        assert_reads_back(&router, &recreated, &new_arn).await;

        let (_, second) = read_provider(&router, &new_arn).await;
        let second_uuid = element(&second, "SAMLProviderUUID");
        assert_ne!(
            second_uuid, first_uuid,
            "case {:?}: a recreated provider is a new record, so its uuid must be its own",
            case.name
        );
        assert!(
            !uuids.contains(&second_uuid),
            "case {:?}: every provider's uuid must be its own, got the held {second_uuid:?}",
            case.name
        );
        uuids.push(first_uuid);
        uuids.push(second_uuid);

        // Leave the account empty for the next row.
        let (status, body) = delete_provider(&router, &new_arn).await;
        assert_eq!(
            status, 200,
            "case {:?}: the recreated provider must delete too, got {status}: {body}",
            case.name
        );
    }
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// RULE: an ARN naming no provider the account holds is refused as absent
/// (`NoSuchEntity`, HTTP 404) — which a second delete of the same ARN is a case of —
/// while a value that is not an ARN is refused as invalid (`InvalidInput`, HTTP 400). A
/// refused delete removes nothing.
#[tokio::test]
async fn an_absent_provider_is_not_found_and_a_non_arn_is_invalid() {
    let router = router();
    let cases = deletion_cases();
    let held = create_provider(&router, &cases[1]).await;
    let doomed = create_provider(&router, &cases[2]).await;

    let (status, body) = delete_provider(&router, &doomed).await;
    assert_eq!(
        status, 200,
        "the first delete must be served, got {status} with body {body}"
    );

    // A second delete of the same ARN, and an ARN no create ever minted, are the same
    // answer: the account holds no such provider.
    for (what, arn) in [
        ("the same provider twice", doomed.clone()),
        ("a name no create used", expected_arn("NeverCreated")),
        ("another unused name", expected_arn("another.provider-1")),
    ] {
        let (status, body) = delete_provider(&router, &arn).await;
        assert_eq!(
            status, 404,
            "case {what:?}: expected HTTP 404, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "NoSuchEntity",
            "case {what:?}: expected NoSuchEntity, got {body}"
        );
        // The refusal names the ARN the request asked for: what IAM holds for a provider
        // is the metadata manifest its create uploaded, so an ARN it holds none for is
        // reported as that manifest not being found for THAT ARN.
        assert_eq!(
            element(&body, "Message"),
            format!("Manifest not found for arn {arn}"),
            "case {what:?}: the refusal must name the ARN, got {body}"
        );
    }

    // Values past `arnType`'s 20-character minimum, so the HANDLER is what refuses them
    // rather than the boundary validator.
    for value in [
        "DeletedIdPButNotAnArnAtAll",
        "iam::saml-provider/corp_idp-2.eu",
        "arn-but-not-really-one/corp_idp-2.eu",
    ] {
        let (status, body) = delete_provider(&router, value).await;
        assert_eq!(
            status, 400,
            "case {value:?}: a value that is not an ARN must be refused with HTTP 400, got \
             {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidInput",
            "case {value:?}: expected InvalidInput, got {body}"
        );
    }

    // A value SHORTER than `arnType`'s published 20-character minimum is refused by the
    // modelled constraint, before the operation runs at all.
    let (status, body) = delete_provider(&router, "corp_idp-2.eu").await;
    assert_eq!(
        status, 400,
        "a value shorter than arnType's minimum must be refused with HTTP 400, got {status} \
         with body {body}"
    );

    // Nothing was removed by any refused call: the provider the account still holds reads
    // back as its create wrote it, and it is the only one listed.
    assert_reads_back(&router, &cases[1], &held).await;
    assert_eq!(
        held_arns(&router, "after every refusal").await,
        vec![held],
        "a refused delete must remove nothing"
    );
}

// ---------------------------------------------------------------------------
// Roles that trust the provider
// ---------------------------------------------------------------------------

/// "Deleting the provider resource from IAM does not update any roles that reference the
/// SAML provider resource's ARN as a principal in their trust policies": the role keeps
/// the trust policy it was created with, naming the ARN of a provider the account no
/// longer holds.
#[tokio::test]
async fn deleting_a_provider_leaves_a_role_that_trusts_it_alone() {
    let router = router();

    for (index, case) in deletion_cases().into_iter().enumerate() {
        let arn = create_provider(&router, &case).await;
        let trust_policy = format!(
            "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"Allow\",\"Principal\":\
             {{\"Federated\":\"{arn}\"}},\"Action\":\"sts:AssumeRoleWithSAML\",\"Condition\":\
             {{\"StringEquals\":{{\"SAML:aud\":\"https://signin.aws.amazon.com/saml\"}}}}}}]}}"
        );
        let role_name = format!("FederatedRole{index}");
        let (status, body) = call(
            &router,
            "CreateRole",
            &[
                ("RoleName", role_name.as_str()),
                ("AssumeRolePolicyDocument", trust_policy.as_str()),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {role_name:?}: the fixture CreateRole must succeed, got {status}: {body}"
        );

        let (status, body) = delete_provider(&router, &arn).await;
        assert_eq!(
            status, 200,
            "case {role_name:?}: the delete must be served, got {status} with body {body}"
        );

        let (status, role) = call(&router, "GetRole", &[("RoleName", role_name.as_str())]).await;
        assert_eq!(
            status, 200,
            "case {role_name:?}: the role must still read back, got {status}: {role}"
        );
        assert_eq!(
            crate::user_lifecycle::document_element(&role, "AssumeRolePolicyDocument"),
            trust_policy,
            "case {role_name:?}: the role must keep the trust policy naming the deleted \
             provider, got {role}"
        );
    }
}
