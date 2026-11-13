// SPDX-License-Identifier: CC0-1.0

//! The IAM condition context a request to this service is judged under: the keys the
//! request's own members and the addressed secret populate, collected here so every
//! operation spells one key the same way.
//!
//! AWS evaluates a statement's `Condition` against the context the service builds for the
//! call, so an `Allow` qualified by a condition permits exactly the requests that satisfy
//! it and a `Deny` so qualified refuses exactly those. The keys are the ones the IAM
//! Service Authorization Reference lists for this service ("Condition keys for AWS Secrets
//! Manager" and the global keys a tagging request carries), each populated from the member
//! of the request it names, or from the stored secret the request addresses:
//!
//! * the tags a request SENDS populate [`REQUEST_TAG_PREFIX`]`<key>` and name their keys in
//!   [`TAG_KEYS_KEY`];
//! * the tags the ADDRESSED secret carries populate [`RESOURCE_TAG_PREFIX`]`<key>` and this
//!   service's own spelling of that family, [`SERVICE_RESOURCE_TAG_PREFIX`]`<key>`;
//! * every other key is one request member's value, under the name this service publishes
//!   it as.
//!
//! A key this context MODELS is declared to the shared evaluator whether or not the request
//! carries a value for it ([`Conditions::published`]), which is what makes a clause over a
//! key the request DID NOT carry decidable: it is a key the request really has no value
//! for, so a positive comparison over it does not hold — and an unsatisfied condition stops
//! its statement from applying, whichever effect that statement states. A key this context
//! does not model is left out of both lists, where the shared evaluator's fail-closed
//! reading of an undecided clause takes over.

use crate::ops::types::{ReplicaRegionType, Tag};

use super::StoredTag;

/// The global condition-key family carrying one tag a request SENDS: "aws:RequestTag/tag-key
/// — works with the string operators. Checks the tag key-value pair that was passed in the
/// request" (IAM's global condition keys).
pub const REQUEST_TAG_PREFIX: &str = "aws:RequestTag/";

/// The global condition key naming the tag KEYS a request sends: "aws:TagKeys — checks the
/// tag keys that were passed in the request". It carries one value per key, so a statement
/// decides it under one of IAM's set qualifiers.
pub const TAG_KEYS_KEY: &str = "aws:TagKeys";

/// The global condition-key family carrying one tag the ADDRESSED resource carries:
/// "aws:ResourceTag/tag-key — checks the tag key-value pair attached to the resource".
pub const RESOURCE_TAG_PREFIX: &str = "aws:ResourceTag/";

/// This service's own spelling of that family, which its Service Authorization Reference
/// lists beside the global one ("secretsmanager:ResourceTag/tag-key").
pub const SERVICE_RESOURCE_TAG_PREFIX: &str = "secretsmanager:ResourceTag/";

/// The `SecretId` the request addressed the secret by, as it carried it.
pub const SECRET_ID_KEY: &str = "secretsmanager:SecretId";

/// The global condition key naming the principal that made the request: "aws:PrincipalArn
/// — the ARN of the principal that made the request" (IAM's global condition keys).
///
/// "For IAM roles, the request context returns the ARN of the role, not the ARN of the user
/// that assumed the role", so a request signed by an assumed-role session answers it with
/// the ROLE's ARN — the identity whose permissions are in force — rather than with the
/// session ARN STS reports for the credential.
pub const PRINCIPAL_ARN_KEY: &str = "aws:PrincipalArn";

/// The global condition key naming that principal's account: "aws:PrincipalAccount — the
/// account the principal belongs to" (IAM's global condition keys).
pub const PRINCIPAL_ACCOUNT_KEY: &str = "aws:PrincipalAccount";

/// The `Name` a create states for the secret.
pub const NAME_KEY: &str = "secretsmanager:Name";

/// The `Description` a create or an update states.
pub const DESCRIPTION_KEY: &str = "secretsmanager:Description";

/// The `KmsKeyId` a create or an update states.
pub const KMS_KEY_ID_KEY: &str = "secretsmanager:KmsKeyId";

/// The `VersionId` a read of one version names.
pub const VERSION_ID_KEY: &str = "secretsmanager:VersionId";

/// The staging label the request names: `VersionStage` on a read or a stage move, and one
/// value per entry of `VersionStages` on a value write.
pub const VERSION_STAGE_KEY: &str = "secretsmanager:VersionStage";

/// The `RotationLambdaARN` a rotation configures.
pub const ROTATION_LAMBDA_ARN_KEY: &str = "secretsmanager:RotationLambdaARN";

/// The `BlockPublicPolicy` a resource-policy write states.
pub const BLOCK_PUBLIC_POLICY_KEY: &str = "secretsmanager:BlockPublicPolicy";

/// The `ForceDeleteWithoutRecovery` a delete states.
pub const FORCE_DELETE_KEY: &str = "secretsmanager:ForceDeleteWithoutRecovery";

/// The `RecoveryWindowInDays` a delete states, compared by the numeric operators.
pub const RECOVERY_WINDOW_KEY: &str = "secretsmanager:RecoveryWindowInDays";

/// The `ForceOverwriteReplicaSecret` a create or a replication states.
pub const FORCE_OVERWRITE_REPLICA_KEY: &str = "secretsmanager:ForceOverwriteReplicaSecret";

/// The Regions a create or a replication replicates to: one value per entry of
/// `AddReplicaRegions`, so a statement decides it under a set qualifier.
pub const ADD_REPLICA_REGIONS_KEY: &str = "secretsmanager:AddReplicaRegions";

/// The condition keys one request publishes, with the value it carries for each.
///
/// Built by the operation that owns the members it reads, and handed to the authorization
/// gate, which adds the keys the ADDRESSED secret populates before it decides. An operation
/// that publishes no keys of its own passes [`Conditions::none`].
#[derive(Debug, Clone, Default)]
pub struct Conditions {
    /// The `(key, value)` pairs the request carries, one pair per value of a key that
    /// carries several.
    values: Vec<(String, String)>,
    /// The keys and key FAMILIES this request models, whether or not it carries a value
    /// for each. Every name is one of this module's constants, which is why they are
    /// `'static`.
    published: Vec<&'static str>,
}

impl Conditions {
    /// The context of a request that publishes no key of its own.
    #[must_use]
    pub fn none() -> Self {
        Self::default()
    }

    /// Publish `key` and carry `value` for it where the request states one.
    #[must_use]
    pub fn text(mut self, key: &'static str, value: Option<&str>) -> Self {
        self.publish(key);
        if let Some(value) = value {
            self.values.push((key.to_string(), value.to_string()));
        }
        self
    }

    /// The same, for a boolean member: the value is the literal IAM's `Bool` operator
    /// compares against.
    #[must_use]
    pub fn flag(self, key: &'static str, value: Option<bool>) -> Self {
        let rendered = value.map(|flag| flag.to_string());
        self.text(key, rendered.as_deref())
    }

    /// The same, for a numeric member: the value is the figure IAM's numeric operators
    /// compare against.
    #[must_use]
    pub fn count(self, key: &'static str, value: Option<i64>) -> Self {
        let rendered = value.map(|count| count.to_string());
        self.text(key, rendered.as_deref())
    }

    /// The same, for a member that carries SEVERAL values: `key` is published once and
    /// carries one pair per value, which is the form IAM's set qualifiers decide.
    #[must_use]
    pub fn each<Value: AsRef<str>>(mut self, key: &'static str, values: &[Value]) -> Self {
        self.publish(key);
        for value in values {
            self.values
                .push((key.to_string(), value.as_ref().to_string()));
        }
        self
    }

    /// Publish the tags this request SENDS: one pair of the request-tag family per tag, and
    /// one value of [`TAG_KEYS_KEY`] per key.
    #[must_use]
    pub fn request_tags(mut self, tags: Option<&[Tag]>) -> Self {
        self.publish(REQUEST_TAG_PREFIX);
        self.publish(TAG_KEYS_KEY);
        for tag in tags.unwrap_or_default() {
            let Some(key) = tag.key.as_deref() else {
                continue;
            };
            self.values.push((TAG_KEYS_KEY.to_string(), key.to_string()));
            if let Some(value) = tag.value.as_deref() {
                self.values
                    .push((format!("{REQUEST_TAG_PREFIX}{key}"), value.to_string()));
            }
        }
        self
    }

    /// Publish the tag KEYS a request names without values — the keys an untagging removes,
    /// which is the member AWS populates [`TAG_KEYS_KEY`] from for such a request.
    #[must_use]
    pub fn removed_tag_keys(self, keys: &[String]) -> Self {
        self.each(TAG_KEYS_KEY, keys)
    }

    /// Publish the Regions a request replicates the secret to: one value of
    /// [`ADD_REPLICA_REGIONS_KEY`] per entry of `AddReplicaRegions` that names a Region.
    #[must_use]
    pub fn replica_regions(self, regions: Option<&[ReplicaRegionType]>) -> Self {
        let named: Vec<&str> = regions
            .unwrap_or_default()
            .iter()
            .filter_map(|region| region.region.as_deref())
            .collect();
        self.each(ADD_REPLICA_REGIONS_KEY, &named)
    }

    /// Add the keys the ADDRESSED secret populates: one pair per tag it carries, under both
    /// spellings of the resource-tag family.
    ///
    /// Called by the authorization gate once it has read the secret the request addresses,
    /// so no operation has to read it again — and a secret this account holds none of
    /// carries no tags, which is a context this still MODELS: the families are published
    /// either way.
    pub fn add_resource_tags(&mut self, tags: &[StoredTag]) {
        self.publish(RESOURCE_TAG_PREFIX);
        self.publish(SERVICE_RESOURCE_TAG_PREFIX);
        for tag in tags {
            let Some(key) = tag.key.as_deref() else {
                continue;
            };
            let Some(value) = tag.value.as_deref() else {
                continue;
            };
            for family in [RESOURCE_TAG_PREFIX, SERVICE_RESOURCE_TAG_PREFIX] {
                self.values
                    .push((format!("{family}{key}"), value.to_string()));
            }
        }
    }

    /// Add the keys the CALLER populates: the ARN of the principal that made the request
    /// and the account that principal belongs to.
    ///
    /// Called by the authorization gate once it has resolved the caller, so a statement
    /// scoped to one principal or one account — the documented way a resource-based
    /// policy admits a whole account without admitting everybody — is decided rather than
    /// left undecided. A request whose credential speaks for no principal populates
    /// neither, so a clause over one of them stays undecided for it.
    ///
    /// `principal_arn` is the ARN of the IDENTITY whose permissions are in force, which for
    /// a request signed by an assumed-role session is the ROLE's: "for IAM roles, the
    /// request context returns the ARN of the role, not the ARN of the user that assumed the
    /// role" ([`PRINCIPAL_ARN_KEY`], IAM's global condition keys). The gate reads it off the
    /// caller it resolved (`crate::state::authorization`), so a policy that locks a secret
    /// to one role is satisfied by every session of that role.
    pub fn add_caller(&mut self, principal_arn: &str) {
        self.publish(PRINCIPAL_ARN_KEY);
        self.values
            .push((PRINCIPAL_ARN_KEY.to_string(), principal_arn.to_string()));
        let account = ::service_common::arn::parse(principal_arn)
            .map(|arn| arn.account)
            .filter(|account| !account.is_empty());
        if let Some(account) = account {
            self.publish(PRINCIPAL_ACCOUNT_KEY);
            self.values
                .push((PRINCIPAL_ACCOUNT_KEY.to_string(), account.to_string()));
        }
    }

    /// The `(key, value)` pairs the decision reads.
    #[must_use]
    pub fn values(&self) -> &[(String, String)] {
        &self.values
    }

    /// The keys and families this request models. See the module docs.
    #[must_use]
    pub fn published(&self) -> &[&'static str] {
        &self.published
    }

    /// Record `key` as modelled, once: a family published twice would make a request
    /// declare the same thing twice, and the evaluator reads the list by membership.
    fn publish(&mut self, key: &'static str) {
        if !self.published.contains(&key) {
            self.published.push(key);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The pairs a context carries for `key`, in the order it collected them.
    fn carried(conditions: &Conditions, key: &str) -> Vec<String> {
        conditions
            .values()
            .iter()
            .filter(|(name, _)| name == key)
            .map(|(_, value)| value.clone())
            .collect()
    }

    /// RULE: a key this context models is PUBLISHED whether or not the request carries a
    /// value for it, so a clause over a member the request left out is decidable.
    #[test]
    fn a_modelled_key_is_published_even_with_no_value() {
        let absent = Conditions::none()
            .flag(FORCE_DELETE_KEY, None)
            .count(RECOVERY_WINDOW_KEY, None)
            .text(VERSION_STAGE_KEY, None);
        for key in [FORCE_DELETE_KEY, RECOVERY_WINDOW_KEY, VERSION_STAGE_KEY] {
            assert!(
                absent.published().contains(&key),
                "case {key:?}: expected the key to be published, got {:?}",
                absent.published()
            );
            assert!(
                carried(&absent, key).is_empty(),
                "case {key:?}: expected no value, got {:?}",
                carried(&absent, key)
            );
        }
    }

    /// RULE: a scalar member's value is the text IAM compares against — the boolean
    /// literals for a flag, the figure for a count.
    #[test]
    fn a_scalar_member_carries_the_text_iam_compares() {
        // (case, the context, the key, the values it must carry)
        let rows: Vec<(&str, Conditions, &str, Vec<&str>)> = vec![
            (
                "a flag set",
                Conditions::none().flag(FORCE_DELETE_KEY, Some(true)),
                FORCE_DELETE_KEY,
                vec!["true"],
            ),
            (
                "a flag cleared",
                Conditions::none().flag(BLOCK_PUBLIC_POLICY_KEY, Some(false)),
                BLOCK_PUBLIC_POLICY_KEY,
                vec!["false"],
            ),
            (
                "a count",
                Conditions::none().count(RECOVERY_WINDOW_KEY, Some(30)),
                RECOVERY_WINDOW_KEY,
                vec!["30"],
            ),
            (
                "a string",
                Conditions::none().text(VERSION_STAGE_KEY, Some("AWSCURRENT")),
                VERSION_STAGE_KEY,
                vec!["AWSCURRENT"],
            ),
            (
                "a member carrying several values",
                Conditions::none().each(
                    ADD_REPLICA_REGIONS_KEY,
                    &["eu-west-1".to_string(), "us-east-2".to_string()],
                ),
                ADD_REPLICA_REGIONS_KEY,
                vec!["eu-west-1", "us-east-2"],
            ),
        ];
        for (case, conditions, key, expected) in rows {
            let reached = carried(&conditions, key);
            assert_eq!(
                reached, expected,
                "case {case:?}: expected {expected:?} for {key}, got {reached:?}"
            );
        }
    }

    /// RULE: the tags a request sends populate the request-tag family and name their keys,
    /// and the tags the addressed secret carries populate both spellings of the
    /// resource-tag family.
    #[test]
    fn tags_populate_the_families_their_side_of_the_request_names() {
        let sent = vec![Tag {
            key: Some("owner".to_string()),
            value: Some("platform".to_string()),
        }];
        let conditions = Conditions::none().request_tags(Some(&sent));
        assert_eq!(
            carried(&conditions, "aws:RequestTag/owner"),
            vec!["platform".to_string()],
            "case \"a sent tag\": expected the request-tag family to carry it, got {:?}",
            conditions.values()
        );
        assert_eq!(
            carried(&conditions, TAG_KEYS_KEY),
            vec!["owner".to_string()],
            "case \"a sent tag\": expected its key named, got {:?}",
            conditions.values()
        );

        let mut addressed = Conditions::none();
        addressed.add_resource_tags(&[StoredTag {
            key: Some("team".to_string()),
            value: Some("payments".to_string()),
        }]);
        for family in [RESOURCE_TAG_PREFIX, SERVICE_RESOURCE_TAG_PREFIX] {
            let key = format!("{family}team");
            assert_eq!(
                carried(&addressed, &key),
                vec!["payments".to_string()],
                "case {key:?}: expected the stored tag's value, got {:?}",
                addressed.values()
            );
        }
    }
}
