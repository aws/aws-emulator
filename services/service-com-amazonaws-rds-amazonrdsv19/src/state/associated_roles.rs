// SPDX-License-Identifier: CC0-1.0

//! The IAM roles associated with a DB instance or a DB cluster: the rules
//! `AddRoleToDBInstance`, `AddRoleToDBCluster`, `RemoveRoleFromDBInstance` and
//! `RemoveRoleFromDBCluster` share.
//!
//! An association is what lets a database engine act under a caller-supplied role for ONE
//! feature of that engine — loading from and unloading to Amazon S3, invoking a Lambda
//! function, calling an Aurora machine-learning service — so the four operations all answer
//! the same questions before they write: is the value an IAM role ARN at all
//! ([`is_iam_role_arn`]), does it name a role this account's IAM holds that
//! `rds.amazonaws.com` may assume, and is the `FeatureName` one the addressed resource's
//! ENGINE VERSION publishes. Each question has one answer per request whichever resource
//! the request addressed, so they are stated once here rather than twice in the instance
//! and cluster handlers.
//!
//! What an association already in place is refused for is the one thing that differs by
//! resource, because Amazon RDS words and codes the two refusals differently: a DB
//! INSTANCE's engine takes one role ARN per feature name, and a DB CLUSTER takes one
//! association per role. The unpublished-`FeatureName` refusal is NOT one of those: it is one
//! sentence for both resources ([`unsupported_feature_name_message`]), because the engine is
//! what decides whether a feature name exists and the resource does not change that answer.
//!
//! The feature names are not a list of this module's own: they are the
//! `DBEngineVersion$SupportedFeatureNames` this service already publishes for that engine
//! version (`FeatureName` documents them as "the feature for the DB instance that the IAM
//! role is to be associated with. For information about supported feature names, see
//! DBEngineVersion"), read from the one catalogue
//! [`crate::ops::describe_db_engine_versions`] reports them from, so a client that read the
//! list and a request that names one of its entries cannot disagree.

use super::*;
use crate::ops::DBClusterAssociatedRole;

/// The service principal a role must trust for Amazon RDS to assume it on a DB instance's
/// or DB cluster's behalf: the principal `rds.amazonaws.com`, which is what the trust
/// policy of every role in "Setting up IAM roles to access Amazon S3" (Amazon RDS User
/// Guide) grants `sts:AssumeRole` to.
pub const RDS_SERVICE_PRINCIPAL: &str = "rds.amazonaws.com";

/// The service principal a role must trust for an Amazon RDS snapshot EXPORT to assume it:
/// "export.rds.amazonaws.com", the principal the trust policy in "Exporting DB snapshot
/// data to Amazon S3" (Amazon RDS User Guide) grants `sts:AssumeRole` to.
pub const EXPORT_SERVICE_PRINCIPAL: &str = "export.rds.amazonaws.com";

/// How many IAM roles one DB instance or DB cluster may have associated with it at once:
/// the "IAM roles per DB instance" / "IAM roles per DB cluster" quota Amazon RDS publishes
/// (Quotas and constraints for Amazon RDS, Amazon RDS User Guide), which is what the
/// modelled `DBInstanceRoleQuotaExceededFault` / `DBClusterRoleQuotaExceededFault` reports
/// having reached.
pub const MAX_ASSOCIATED_ROLES: usize = 5;

/// The `SupportedFeatureNames` of the engine version a resource runs — the `FeatureName`
/// values an association on it may name.
///
/// Resolved through the published catalogue, exactly as `DescribeDBEngineVersions` resolves
/// it: a request that names no version reads the engine's default version's list, which is
/// the version such a resource was created with. An engine the catalogue publishes nothing
/// for has no published list at all, which is reported as an empty one — there is then no
/// feature name this service can say is supported.
#[must_use]
pub fn supported_feature_names(engine: &str, engine_version: Option<&str>) -> &'static [&'static str] {
    crate::ops::describe_db_engine_versions::resolve_version(engine, engine_version)
        .ok()
        .flatten()
        .map_or(&[], |version| version.profile.feature_names)
}

/// Whether `feature_name` is one the engine version this resource runs publishes.
///
/// Compared exactly: a feature name is a published identifier (`s3Import`, `S3_INTEGRATION`,
/// `SQLSERVER_BACKUP_RESTORE`), and the catalogue states each one in the case AWS publishes
/// it in, so a value in another case names no published feature.
#[must_use]
pub fn feature_name_is_supported(
    engine: &str,
    engine_version: Option<&str>,
    feature_name: &str,
) -> bool {
    supported_feature_names(engine, engine_version).contains(&feature_name)
}

/// What a request naming a `FeatureName` the addressed resource's engine version does not
/// publish is refused with: "The feature name &lt;FeatureName&gt; is not valid for the engine
/// &lt;engine&gt;."
///
/// ONE sentence for the whole class, whichever resource the request addressed — a DB instance
/// ([`crate::ops::add_role_to_db_instance`]) or a DB cluster
/// ([`check_cluster_role_feature_name`]): the defect is the same one, a value outside the
/// engine version's published `SupportedFeatureNames`, and Amazon RDS words it the same way
/// for both.
///
/// The FIXED text is, in this order, "The feature name ", " is not valid for the engine " and
/// "." — the clause naming the engine comes LAST, introduced by "for the engine". The VARIABLE
/// parts are the value the request carried, copied verbatim, and the ENGINE it was rejected
/// for, under the DESCRIPTION that engine is published with rather than its product name
/// ([`engine_published_description`]: `aurora-postgresql` is "Aurora (PostgreSQL)" there and
/// "Aurora PostgreSQL" as a product) — because the engine is what decides whether a feature
/// name exists at all ([`feature_name_is_supported`]), and that description is the name of it
/// a client reading the published catalogue has already seen. A caller that read the sentence
/// knows both which value was wrong and which engine's list to read instead.
///
/// A role Amazon RDS cannot take on is a defect of the ROLE rather than of the feature and
/// carries that integration's own sentence ([`ROLE_NOT_TRUSTED_MESSAGE`]).
#[must_use]
pub fn unsupported_feature_name_message(engine: &str, feature_name: &str) -> String {
    format!(
        "The feature name {feature_name} is not valid for the engine {}.",
        engine_published_description(engine)
    )
}

/// Hold ONE `FeatureName` a DB cluster's role association names to the `SupportedFeatureNames`
/// of the engine version that cluster runs: `Ok` for a feature the engine publishes and for an
/// association that names NONE — which associates the role with the cluster itself, the member
/// being optional and no catalogue entry gating it — and otherwise the sentence the request is
/// refused with ([`unsupported_feature_name_message`]), which is the Query API's
/// `InvalidParameterValue`.
///
/// The one reading of that question for every call that records an association on a cluster:
/// `AddRoleToDBCluster`, which makes one ([`crate::ops::add_role_to_db_cluster`]), and the
/// `AssociatedRoles` list a create or restore carries, which is a list of them
/// ([`check_cluster_role_feature_names`]).
pub fn check_cluster_role_feature_name(
    engine: &str,
    engine_version: Option<&str>,
    feature_name: Option<&str>,
) -> Result<(), String> {
    let Some(feature_name) = feature_name else {
        return Ok(());
    };
    if feature_name_is_supported(engine, engine_version, feature_name) {
        return Ok(());
    }
    Err(unsupported_feature_name_message(engine, feature_name))
}

/// Hold every `AssociatedRoles` entry a DB cluster is being created or restored WITH to the
/// `SupportedFeatureNames` of the engine version that cluster will run, the way
/// `AddRoleToDBCluster` holds the one association it makes
/// ([`crate::ops::add_role_to_db_cluster`]).
///
/// An entry is a role association exactly as that call makes one — "a list of Amazon Web
/// Services Identity and Access Management (IAM) roles to associate with the DB cluster", each
/// carrying the `RoleArn` and the optional `FeatureName` that call takes — so the feature name
/// it names has to be one the engine publishes whichever call records it: a create that took a
/// value AWS refuses would leave a cluster acting for a feature of an engine that has no such
/// feature. Each entry is judged by [`check_cluster_role_feature_name`], the one reading of
/// that question, and `Err` carries the sentence the FIRST entry it refuses is refused with.
pub fn check_cluster_role_feature_names(
    engine: &str,
    engine_version: Option<&str>,
    roles: &[DBClusterAssociatedRole],
) -> Result<(), String> {
    for role in roles {
        check_cluster_role_feature_name(engine, engine_version, role.feature_name.as_deref())?;
    }
    Ok(())
}

/// The engine's `DBEngineDescription`, as the published engine catalogue reports it
/// (`aurora-postgresql` → `Aurora (PostgreSQL)`), for the sentences that name the engine
/// that way rather than by its product name ([`engine_display_name`], which is
/// `Aurora PostgreSQL` for that same engine): the two differ, and which one a sentence
/// carries is that sentence's own — the unpublished-feature refusal of either resource
/// ([`unsupported_feature_name_message`]) names the engine by this description.
///
/// An engine the catalogue publishes nothing for is named by the `Engine` value itself —
/// there is no other name for it this service knows.
#[must_use]
pub fn engine_published_description(engine: &str) -> &str {
    crate::ops::describe_db_engine_versions::engine_facts(engine)
        .map_or(engine, |facts| facts.description)
}

/// The ARN form a member that names an IAM ROLE must carry:
/// `arn:<aws partition>:iam::<account digits>:role/<name>`.
///
/// Source: this service's OWN model states the form, as the `IAMRoleArn` pattern
/// `^arn:aws[a-z-]*:iam::[0-9]*:role/.*$` that `AddRoleToDBCluster$RoleArn` and
/// `RemoveRoleFromDBCluster$RoleArn` target — so the partition is an `aws` one, an IAM ARN
/// carries no Region, the account field is digits (the model lets it be empty) and the
/// resource is a `role/…`. The sibling members of the same surface
/// (`AddRoleToDBInstance$RoleArn`, `RemoveRoleFromDBInstance$RoleArn`) are modelled as a
/// plain `String`, so for them the check is the handler's and this is where it lives: one
/// reading of "the ARN of an IAM role" for every member of this service that takes one.
///
/// The vendor and the resource type come from `service_common::arn`'s shared requirement,
/// so this crate and every other agree on what an IAM role ARN is; the three field rules
/// beyond it are what this service's own pattern adds.
#[must_use]
pub fn is_iam_role_arn(value: &str) -> bool {
    let role = ::service_common::arn::IAM_ROLE;
    // One parse, filtered to the vendor the shared requirement names.
    let Some(arn) = ::service_common::arn::parse_of(value, role.vendor) else {
        return false;
    };
    let partition_of_aws = arn
        .partition
        .strip_prefix("aws")
        .is_some_and(|rest| rest.bytes().all(|byte| byte.is_ascii_lowercase() || byte == b'-'));
    // `role/` and not a bare `role`: the pattern's `.*` admits an empty role NAME, but not
    // a resource field that names no role at all.
    let names_a_role = role
        .resource_type
        .and_then(|kind| arn.resource.strip_prefix(kind))
        .is_some_and(|rest| rest.starts_with('/'));
    partition_of_aws
        && arn.region.is_empty()
        && arn.account.bytes().all(|byte| byte.is_ascii_digit())
        && names_a_role
}

/// What a request whose `RoleArn` is not an IAM role ARN at all is refused with.
///
/// The sentence names the value that was rejected — the caller has to see which of its
/// ARNs was unreadable — and nothing else: whether the role EXISTS is a different
/// question, asked only of a value that is an ARN of a role to begin with.
#[must_use]
pub fn invalid_role_arn_message(role_arn: &str) -> String {
    format!("Invalid Role ARN: {role_arn}")
}

/// What a request naming a role Amazon RDS cannot assume is refused with: the value has to
/// name a role this account's IAM holds, and that role's trust policy has to grant
/// `sts:AssumeRole` to the service principal that will act under it. Both conditions leave
/// the engine with no role to act under, which is what the one sentence reports.
///
/// It is answered BEFORE the `FeatureName` is held to the engine's published feature names:
/// a role the service cannot take on is associable for no feature at all, so a request whose
/// role AND feature name are both unusable is refused for the role.
///
/// On a DB CLUSTER it is the sentence a role Amazon RDS cannot take on is refused with,
/// whichever feature the request named: the role and the feature are the two halves of one
/// integration there — the role has to carry what the named feature needs — and this half is
/// reported as the integration being unperformable rather than as a defect of one value. A
/// `FeatureName` the cluster's engine does not publish is the OTHER half and carries its own
/// sentence, which names the value and the engine
/// ([`unsupported_feature_name_message`]).
///
/// ONE fixed sentence, with no variable part: this refusal names neither the role ARN the
/// request carried, nor the principal the trust policy is missing, nor the feature that was
/// named — it reports only that the value does not carry what the role integration needs — so
/// it is a constant rather than a `format!` over the request, and the cardinality of the text
/// stays at one for a client matching on it.
pub const ROLE_NOT_TRUSTED_MESSAGE: &str = "IAM role ARN value is invalid or does not \
                                            include the required permissions for: \
                                            AWS_ROLE_INTEGRATION";

/// Whether `roles` already carries an association of `role_arn` for `feature_name`.
///
/// The pair is the identity of the association a DISASSOCIATION names: "the specified
/// RoleArn value doesn't match the specified feature", so a role associated for another
/// feature is not the association the request asked to remove. A `FeatureName` the request
/// left out compares as the association that names none.
///
/// What an ASSOCIATION is refused for is narrower on each resource, and is not this pair:
/// a DB instance's feature takes one role ([`holds_feature`]) and a DB cluster's role is
/// associated once ([`holds_role`]).
#[must_use]
pub fn holds_association(
    roles: &[(String, Option<String>)],
    role_arn: &str,
    feature_name: Option<&str>,
) -> bool {
    roles.iter().any(|(held_arn, held_feature)| {
        held_arn == role_arn && held_feature.as_deref() == feature_name
    })
}

/// Whether `roles` already carries an association of ANY role for `feature_name`.
///
/// A DB INSTANCE's engine admits one role ARN per feature name
/// ([`one_arn_per_feature_message`]), so this — not the role-and-feature pair — is what an
/// association on an instance is refused for: the feature is taken, whichever role took it.
#[must_use]
pub fn holds_feature(roles: &[(String, Option<String>)], feature_name: &str) -> bool {
    roles
        .iter()
        .any(|(_, held_feature)| held_feature.as_deref() == Some(feature_name))
}

/// Whether `roles` already carries an association of `role_arn`, for any feature or none.
///
/// The associations of BOTH resources are keyed by the ROLE as well as by the feature: the
/// already-exists fault of each reports the role ARN the request carried
/// ([`cluster_role_already_associated_message`],
/// [`instance_role_already_associated_message`]), which is what the modelled
/// `DBInstanceRoleAlreadyExistsFault` / `DBClusterRoleAlreadyExistsFault` is for — "the
/// specified RoleArn or FeatureName value is already associated with the DB instance" — so
/// one role is associated with one resource once, whichever feature the second request names.
#[must_use]
pub fn holds_role(roles: &[(String, Option<String>)], role_arn: &str) -> bool {
    roles.iter().any(|(held_arn, _)| held_arn == role_arn)
}

/// The engine's own name, as Amazon RDS writes it in a sentence about the engine: the
/// product name the published engine catalogue reports for that `Engine` value
/// (`postgres` → `PostgreSQL`), which `DescribeDBEngineVersions` already answers with.
///
/// An engine the catalogue publishes nothing for is named by the `Engine` value itself —
/// there is no other name for it this service knows.
#[must_use]
pub fn engine_display_name(engine: &str) -> &str {
    crate::ops::describe_db_engine_versions::engine_facts(engine)
        .map_or(engine, |facts| facts.product)
}

/// What an association on a DB INSTANCE whose `FeatureName` already has a role is refused
/// with: a feature of a DB instance's engine acts under ONE role, so the second ARN for it
/// is a value this parameter will not take rather than a resource conflict.
///
/// The sentence names the engine (by the product name it is published under) and the
/// feature name the request carried, because those are the two halves of the rule the
/// caller has to work with.
#[must_use]
pub fn one_arn_per_feature_message(engine: &str, feature_name: &str) -> String {
    format!(
        "The engine {} supports only one ARN associated with feature name {feature_name}.",
        engine_display_name(engine)
    )
}

/// How Amazon RDS words the already-exists fault of a DB CLUSTER's association: it names
/// the role ARN the request carried and the cluster it is already associated with, and tells
/// the caller which value to look at.
///
/// `identifier` is the value the REQUEST named rather than the key the store folded it to,
/// so a caller matching the sentence against what it sent finds it there — the same reason
/// the not-found wordings name the request's spelling.
#[must_use]
pub fn cluster_role_already_associated_message(role_arn: &str, identifier: &str) -> String {
    format!(
        "Role ARN {role_arn} is already associated with DB Cluster: {identifier}. Verify your \
         role ARN and try again."
    )
}

/// How Amazon RDS words the already-exists fault of a DB INSTANCE's association, which the
/// instance side words differently: the fault is "the specified RoleArn OR FeatureName value
/// is already associated with the DB instance", and the sentence says exactly that — it names
/// the role ARN, the feature name and the instance of the request, in that order, and ends in
/// the instruction to check them.
///
/// The same family as the disassociation's not-found sentence
/// ([`role_not_associated_message`]), which names the same three values of the request in the
/// same order.
#[must_use]
pub fn instance_role_already_associated_message(
    role_arn: &str,
    feature_name: &str,
    identifier: &str,
) -> String {
    format!(
        "The {role_arn} IAM role or {feature_name} feature is already associated with the \
         {identifier} DB instance. Check the name and try again."
    )
}

/// How Amazon RDS words the role-not-found fault of a DB INSTANCE's disassociation: the
/// (role, feature) pair the request named is not one the instance carries, so there is nothing
/// to remove.
///
/// The sentence names the three values the request carried — the role ARN, the feature name and
/// the instance identifier, in that order — and ends in the instruction to check them: "The
/// &lt;role-arn&gt; IAM role doesn't match with the &lt;FeatureName&gt; feature for the
/// &lt;identifier&gt; DB instance. Check and try again." The pair is the identity of the
/// association ([`holds_association`]), so a role associated under ANOTHER feature does not
/// match the one the request named, which is what that wording says.
#[must_use]
pub fn instance_role_not_matched_message(
    identifier: &str,
    role_arn: &str,
    feature_name: &str,
) -> String {
    format!(
        "The {role_arn} IAM role doesn't match with the {feature_name} feature for the \
         {identifier} DB instance. Check and try again."
    )
}

/// How a DB CLUSTER's disassociation words the same fault: the role the request named is not
/// associated with that cluster, for the feature it named or for any.
///
/// A cluster's `FeatureName` is optional on that member, so the sentence reports the feature
/// only where the request carried one.
#[must_use]
pub fn role_not_associated_message(
    resource: &str,
    identifier: &str,
    role_arn: &str,
    feature_name: Option<&str>,
) -> String {
    match feature_name {
        Some(feature) => format!(
            "The IAM role {role_arn} is not associated with the {resource} {identifier} for the \
             feature {feature}."
        ),
        None => {
            format!("The IAM role {role_arn} is not associated with the {resource} {identifier}.")
        }
    }
}

/// How Amazon RDS words the quota fault of an association: the resource already carries
/// every role it may.
#[must_use]
pub fn role_quota_message(resource: &str, identifier: &str) -> String {
    format!(
        "The {resource} {identifier} already has {MAX_ASSOCIATED_ROLES} IAM roles associated \
         with it, which is the maximum."
    )
}

/// The `(RoleArn, FeatureName)` pairs of a stored DB cluster's associations, which is the
/// form [`holds_association`] reads them in.
#[must_use]
pub fn cluster_role_pairs(roles: &[DBClusterRole]) -> Vec<(String, Option<String>)> {
    roles
        .iter()
        .map(|role| {
            (
                role.role_arn.clone().unwrap_or_default(),
                role.feature_name.clone(),
            )
        })
        .collect()
}

/// The same pairs for a stored DB instance's associations.
#[must_use]
pub fn instance_role_pairs(roles: &[DBInstanceRole]) -> Vec<(String, Option<String>)> {
    roles
        .iter()
        .map(|role| {
            (
                role.role_arn.clone().unwrap_or_default(),
                role.feature_name.clone(),
            )
        })
        .collect()
}
