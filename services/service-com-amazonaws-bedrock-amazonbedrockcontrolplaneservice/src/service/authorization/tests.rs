// SPDX-License-Identifier: CC0-1.0

//! The checks of the authorization gate that need no built service: what a URI space
//! addresses, which action the gate asks about, whose policies a principal ARN names, and
//! how a refusal is worded.
//!
//! Split out of [`super`] so that module reads as the decision it takes rather than as the
//! decision plus its table of cases; `super::tests` is where they live, so every one of them
//! still reaches the private items it is about through `use super::*`.

use super::*;

/// The decision one stored document reaches about an action on a resource, through the
/// shared evaluator this gate consults.
fn decision(document: &str, action: &str, resource: &str) -> Decision {
    iam::identity_decision(
        &[document.to_string()],
        &iam::Request::new(action, resource, &[]),
    )
}

/// RULE: a request that names no ARN of the family it addressed is judged against that
/// family's collection, and against every resource only where the space holds no
/// resource of this service.
#[test]
fn a_uri_space_names_the_resource_family_an_action_is_judged_against() {
    // (case, the request target, what that space addresses)
    let rows: &[(&str, &str, Addressed)] = &[
        ("a create", "/guardrails", Addressed::Family(&["guardrail"])),
        (
            "a listing with a query",
            "/guardrails?maxResults=1",
            Addressed::Family(&["guardrail"]),
        ),
        (
            "a read addressed by a bare id",
            "/guardrails/abcd1234",
            Addressed::Family(&["guardrail"]),
        ),
        (
            "an absolute request target",
            "https://bedrock.us-west-2.amazonaws.com/model-invocation-job",
            Addressed::Family(&["model-invocation-job"]),
        ),
        (
            "the plural space of the same family",
            "/model-invocation-jobs",
            Addressed::Family(&["model-invocation-job"]),
        ),
        (
            "the singular space of the evaluation jobs",
            "/evaluation-job/abcd1234/stop",
            Addressed::Family(&["evaluation-job"]),
        ),
        (
            "a family whose space nests the resource",
            "/model-customization/custom-model-deployments",
            Addressed::Family(&[crate::state::CUSTOM_MODEL_DEPLOYMENT_RESOURCE_TYPE]),
        ),
        (
            "a throughput space, whose resources are named provisioned models",
            "/provisioned-model-throughput",
            Addressed::Family(&["provisioned-model"]),
        ),
        (
            "a space holding two kinds of the same resource",
            "/inference-profiles",
            Addressed::Family(&["application-inference-profile", "inference-profile"]),
        ),
        // The spaces whose target is the ARN the request carries, whatever family it
        // belongs to.
        (
            "a tagging operation",
            "/tagResource",
            Addressed::NamedByTheRequest,
        ),
        (
            "a resource-policy operation",
            "/resource-policy/arn%3Aaws%3Abedrock%3Aus-west-2%3A000000000000%3Aguardrail%2Fabc",
            Addressed::NamedByTheRequest,
        ),
        // And the spaces that address no resource of this service at all.
        (
            "an account-level setting",
            "/data-retention",
            Addressed::NoResource,
        ),
        (
            "the logging configuration",
            "/logging/modelinvocations",
            Addressed::NoResource,
        ),
        // The VERSION-PREFIXED targets: the space a versioned URI addresses is read PAST the
        // prefix, so a project-scoped storage setting addresses the same project family
        // `/projects/{id}` does, while the account-level and organization-level ones address
        // no resource of this service — and the prefix alone names nothing.
        (
            "a project-scoped setting under the version prefix",
            "/v1/projects/proj_abcdefghijklmnopqrst/data_retention/storage/anthropic",
            Addressed::Family(&[PROJECT]),
        ),
        (
            "an account-level setting under the version prefix",
            "/v1/data_retention/storage/anthropic",
            Addressed::NoResource,
        ),
        (
            "an organization-level setting under the version prefix",
            "/v1/data_retention/organization-storage/anthropic",
            Addressed::NoResource,
        ),
        ("the version prefix alone", "/v1", Addressed::NoResource),
        (
            "a target this model does not bind",
            "/nothing-here",
            Addressed::NoResource,
        ),
        ("the root", "/", Addressed::NoResource),
    ];
    for (case, target, expected) in rows {
        let read = addressed(target);
        assert_eq!(
            read, *expected,
            "case {case:?}: expected {expected:?}, got {read:?}"
        );
    }
}

/// RULE: every resource type this table maps a family to is one this service's OWN ARN
/// builder spells for a resource of that family, so the collection an action is judged
/// against is written exactly as the ARNs of the resources it reaches are — and a
/// policy scoped to this account's resources of that type covers it.
///
/// Asserted by minting an ARN through the builder each family's resources are minted by
/// and reading its resource type back ([`crate::state::names_arn_of`]), so a family
/// mapped to the WRONG type fails here.
#[test]
fn every_mapped_family_is_named_by_the_type_its_own_arns_carry() {
    let region = "us-west-2";
    let id = "0123456789ab";
    // One ARN per family, minted by `crate::state`'s own builder for it.
    let minted: &[(&str, String)] = &[
        (
            "advanced-prompt-optimization-jobs",
            crate::state::advanced_prompt_optimization_job_arn(region, id),
        ),
        (
            "automated-reasoning-policies",
            crate::state::automated_reasoning_policy_arn(region, id),
        ),
        (
            "custom-models",
            crate::state::custom_model_arn(region, "amazon.base-v1:0", id),
        ),
        (
            "evaluation-jobs",
            crate::state::evaluation_job_arn(region, id),
        ),
        ("guardrails", crate::state::guardrail_arn(region, id)),
        (
            "imported-models",
            crate::state::imported_model_arn(region, id),
        ),
        (
            "inference-profiles",
            crate::state::application_inference_profile_arn(region, id),
        ),
        (
            "inference-profiles",
            crate::state::system_defined_inference_profile_arn(region, id),
        ),
        (
            "model-customization",
            crate::state::custom_model_deployment_arn(region, id),
        ),
        (
            "model-copy-jobs",
            crate::state::model_copy_job_arn(region, id),
        ),
        (
            "model-customization-jobs",
            crate::state::model_customization_job_arn(region, "amazon.base-v1:0", id),
        ),
        (
            "model-import-jobs",
            crate::state::model_import_job_arn(region, id),
        ),
        (
            "model-invocation-job",
            crate::state::model_invocation_job_arn(region, id),
        ),
        ("prompt-routers", crate::state::prompt_router_arn(region, id)),
        (
            "provisioned-model-throughput",
            crate::state::provisioned_model_arn(region, id),
        ),
        (
            "foundation-models",
            crate::state::foundation_model_arn(region, "amazon.base-v1:0"),
        ),
    ];
    for (space, arn) in minted {
        let Addressed::Family(resource_types) = addressed(&format!("/{space}")) else {
            panic!("case {space:?}: this space must address a resource family");
        };
        assert!(
            resource_types
                .iter()
                .any(|resource_type| crate::state::names_arn_of(arn, resource_type)),
            "case {space:?}: {arn:?} is of none of the mapped types {resource_types:?}"
        );
    }
}

/// RULE: every URI space this service serves is CLASSIFIED — it addresses a family of
/// this service's resources, or the resource the request names, or no resource at all —
/// so a space added by a later model refresh cannot silently fall through to the
/// resource that no scoped policy matches.
#[test]
fn every_uri_space_of_this_service_is_classified() {
    for space in crate::ROUTE_PATH_SEGMENTS {
        let reached = addressed(&format!("/{space}"));
        let resource_free = RESOURCE_FREE_SPACES.contains(space);
        assert_eq!(
            reached == Addressed::NoResource,
            resource_free,
            "case {space:?}: a space reads as addressing no resource exactly when it is \
                 one of the spaces stated to hold none, got {reached:?} against \
                 {resource_free}"
        );
    }
}

/// RULE: the action this gate asks about is `bedrock:<OperationName>`, so a policy
/// scoped to this service's actions covers every operation of it and nothing outside
/// it.
#[test]
fn the_action_asked_about_carries_this_services_prefix() {
    let document = r#"{"Statement":[{"Effect":"Allow","Action":"bedrock:*","Resource":"*"}]}"#;
    for operation in ["CreateCustomModel", "GetCustomModel", "ListCustomModels"] {
        let action = format!("{ACTION_PREFIX}:{operation}");
        let reached = decision(document, &action, ANY_RESOURCE);
        assert_eq!(
            reached,
            Decision::Allowed,
            "case {operation:?}: expected bedrock:* to allow {action}, got {reached:?}"
        );
    }
    let outside = decision(document, "s3:GetObject", ANY_RESOURCE);
    assert_eq!(
        outside,
        Decision::NotStated,
        "case \"another service\": expected bedrock:* to grant nothing outside this \
             service, got {outside:?}"
    );
}

/// RULE: a session ARN names the role whose policies are read, an IAM user ARN the
/// user whose policies are; every other principal form names neither.
#[test]
fn a_principal_arn_names_the_entity_its_policies_are_read_from() {
    // (case, the ARN, the role it names, the user it names)
    let rows: &[(&str, &str, Option<&str>, Option<&str>)] = &[
        (
            "an assumed-role session",
            "arn:aws:sts::000000000000:assumed-role/bedrock-caller/session-1",
            Some("bedrock-caller"),
            None,
        ),
        (
            "a session name carrying a slash",
            "arn:aws:sts::000000000000:assumed-role/bedrock-caller/one/two",
            Some("bedrock-caller"),
            None,
        ),
        (
            "an IAM user",
            "arn:aws:iam::000000000000:user/bedrock-user",
            None,
            Some("bedrock-user"),
        ),
        (
            "an IAM user under a path",
            "arn:aws:iam::000000000000:user/team/bedrock-user",
            None,
            Some("bedrock-user"),
        ),
        (
            "the account's own identity",
            "arn:aws:iam::000000000000:root",
            None,
            None,
        ),
        ("a value that is no ARN", "AKIDTESTONLY", None, None),
    ];
    for (case, arn, role, user) in rows {
        assert_eq!(
            assumed_role_name(arn),
            *role,
            "case {case:?}: expected the role {role:?}, got {:?}",
            assumed_role_name(arn)
        );
        assert_eq!(
            iam_user_name(arn),
            *user,
            "case {case:?}: expected the user {user:?}, got {:?}",
            iam_user_name(arn)
        );
    }
}

/// RULE: the refusal names the principal, the action, the resource the request
/// addressed and which of IAM's rules refused — every part taken from the request and
/// the stored policies — and it names no resource for an action that addresses none.
#[test]
fn the_refusal_names_the_principal_the_action_and_the_resource() {
    let principal = "arn:aws:sts::000000000000:assumed-role/bedrock-caller/session-1";
    let model = "arn:aws:bedrock:us-west-2:000000000000:custom-model/imported/0123456789ab";
    // (case, the operation, the resource it addressed, the clause the sentence carries)
    let rows: &[(&str, &str, &str, String)] = &[
        (
            "a resource-scoped action",
            "GetCustomModel",
            model,
            format!(" on resource: {model}"),
        ),
        (
            "an action that addresses no resource",
            "GetModelInvocationLoggingConfiguration",
            ANY_RESOURCE,
            String::new(),
        ),
    ];
    for (case, operation, resource, clause) in rows {
        let action = format!("{ACTION_PREFIX}:{operation}");
        let reason = format!("because no identity-based policy allows the {action} action");
        let refusal = not_authorized(principal, &action, resource, &reason);
        let message = refusal.message.unwrap_or_default();
        assert_eq!(
            message,
            format!(
                "User: {principal} is not authorized to perform: {action}{clause} {reason}"
            ),
            "case {case:?}: expected IAM's wording, got {message:?}"
        );
    }
}

/// RULE: a request that carries ONE resource's own ARN resource id in its URI is judged
/// against that resource, so the id is read off the segment after the space — and only
/// where it is spelled as THAT family spells a resource id, never from an ARN label, a
/// sub-resource segment, or a label of any other form.
#[test]
fn a_uri_carrying_a_resource_id_addresses_that_one_resource() {
    let encoded_arn = "arn%3Aaws%3Abedrock%3Aus-west-2%3A000000000000%3Aguardrail%2Fabc123";
    /// One row: the case, the request target, and the resource type and id it
    /// addresses.
    type Row<'a> = (&'a str, &'a str, Option<(&'a str, &'a str)>);
    // (case, the request target, the type and id it addresses)
    let rows: &[Row<'_>] = &[
        (
            "a read addressed by a bare guardrail id",
            "/guardrails/abc123",
            Some(("guardrail", "abc123")),
        ),
        (
            "the same id with a query string",
            "/guardrails/abc123?guardrailVersion=DRAFT",
            Some(("guardrail", "abc123")),
        ),
        (
            "an absolute request target",
            "https://bedrock.us-west-2.amazonaws.com/guardrails/abc123",
            Some(("guardrail", "abc123")),
        ),
        (
            "a batch inference job read",
            "/model-invocation-job/0123456789ab",
            Some(("model-invocation-job", "0123456789ab")),
        ),
        (
            "a sub-resource of that job",
            "/model-invocation-job/0123456789ab/stop",
            Some(("model-invocation-job", "0123456789ab")),
        ),
        (
            "an optimization job read",
            "/advanced-prompt-optimization-jobs/0123456789ab",
            Some((
                "advanced-prompt-optimization-job",
                "0123456789ab",
            )),
        ),
        ("a create, which names no member", "/guardrails", None),
        ("a listing, which names none either", "/guardrails?maxResults=1", None),
        (
            "a label carrying the whole ARN, which the request NAMES instead",
            &format!("/guardrails/{encoded_arn}"),
            None,
        ),
        (
            "a label of a form no resource id of the family takes",
            "/guardrails/Not-An-Id",
            None,
        ),
        (
            "a label longer than the family's id",
            &format!("/guardrails/{}", "a".repeat(65)),
            None,
        ),
        (
            "a label shorter than a fixed-length family's id",
            "/model-invocation-job/abc",
            None,
        ),
        (
            "a batch-delete, whose space addresses its targets by the request's body",
            "/advanced-prompt-optimization-job/batch-delete",
            None,
        ),
        (
            "a family whose label may be a resource NAME",
            "/imported-models/my-model",
            None,
        ),
        (
            "a space holding no resource of this service",
            "/logging/modelinvocations",
            None,
        ),
    ];
    for (case, target, expected) in rows {
        let read = id_addressed_member(target);
        assert_eq!(
            read, *expected,
            "case {case:?}: expected {expected:?}, got {read:?}"
        );
    }
}

/// RULE: every space that addresses its resources by their own id is a space of this
/// service's own resource families, and the ARN this gate builds from a label is the
/// very ARN that family's own builder mints for the same id — so a policy scoped to the
/// resource the store holds covers the request that addresses it by id.
#[test]
fn every_id_addressed_family_builds_the_arn_its_own_resources_carry() {
    let region = "us-west-2";
    let id = "0123456789ab";
    // (the space, the ARN that family's own builder in `crate::state` mints for `id`)
    let minted: &[(&str, String)] = &[
        (
            "advanced-prompt-optimization-jobs",
            crate::state::advanced_prompt_optimization_job_arn(region, id),
        ),
        ("guardrails", crate::state::guardrail_arn(region, id)),
        (
            "model-invocation-job",
            crate::state::model_invocation_job_arn(region, id),
        ),
    ];
    assert_eq!(
        minted.len(),
        ID_ADDRESSED_FAMILIES.len(),
        "every id-addressed family needs a row here, got {} rows for {} families",
        minted.len(),
        ID_ADDRESSED_FAMILIES.len()
    );
    for (space, arn) in minted {
        let family = ID_ADDRESSED_FAMILIES
            .iter()
            .find(|family| family.space == *space)
            .expect("the row names an id-addressed family");
        let Addressed::Family(resource_types) = addressed(&format!("/{space}")) else {
            panic!("case {space:?}: this space must address a resource family");
        };
        assert!(
            resource_types.contains(&family.resource_type),
            "case {space:?}: {:?} is none of the family's types {resource_types:?}",
            family.resource_type
        );
        assert!(
            family.admits_id(id),
            "case {space:?}: {id:?} must be a resource id of this family"
        );
        let built = ::engine::emulator_arn(
            crate::state::ARN_NAMESPACE,
            region,
            &format!("{}/{id}", family.resource_type),
        );
        assert_eq!(
            &built, arn,
            "case {space:?}: expected the builder's {arn:?}, got {built:?}"
        );
    }
}
