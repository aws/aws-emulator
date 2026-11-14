// SPDX-License-Identifier: CC0-1.0

//! The TAGS this service's resources carry, and the one resolution step the
//! tagging calls share.
//!
//! # The rule this module implements
//!
//! AWS Backup's tagging calls address a resource by ARN rather than by the name
//! or id the resource's own operations use, so a tag written through
//! `TagResource` has to land on the SAME stored record the resource's creator
//! wrote and its reads report — a vault's tags are the vault's, a recovery
//! point's are the point's. That resolution is stated once here
//! ([`ComAmazonawsBackupCryocontrollerusermanagerState::tagged_resource_tags`]
//! and its `_mut` twin) over every record this service stores that carries both
//! an ARN and tags, so a tag a create stored and a tag a tagging call stored are
//! one set, and an ARN that names no such resource is one not-found condition
//! ([`super::tagged_resource_not_found`]) however it fails to name one.
//!
//! A deleted backup plan is deliberately not one of them: the account no longer
//! holds it, so its tags are not there to read or write
//! ([`super::BackupPlanRecord::deletion_date`]).

use std::collections::BTreeMap;

use super::ComAmazonawsBackupCryocontrollerusermanagerState;
use crate::ops::types::ResourceNotFoundException;

/// The most tags this emulator keeps on ONE resource.
///
/// An EMULATOR-CHOSEN ceiling, for the reason given on [`super::MAX_VAULTS`]:
/// the keys are the caller's, so without a bound one resource's tag set would
/// grow with the request count. AWS caps the tags a resource may carry too, and
/// answers the same modelled `LimitExceededException` past it.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

impl ComAmazonawsBackupCryocontrollerusermanagerState {
    /// The tags of the resource `resource_arn` names, or the modelled refusal
    /// for an ARN that names none of this account's taggable resources.
    pub fn tagged_resource_tags(
        &self,
        resource_arn: &str,
    ) -> Result<&BTreeMap<String, String>, ResourceNotFoundException> {
        if let Some(vault) = self
            .vaults
            .values()
            .find(|vault| vault.arn == resource_arn)
        {
            return Ok(&vault.tags);
        }
        if let Some(point) = self
            .recovery_points
            .values()
            .find(|point| point.arn == resource_arn)
        {
            return Ok(&point.tags);
        }
        if let Some(plan) = self
            .backup_plans
            .values()
            .find(|plan| plan.arn == resource_arn && plan.deletion_date.is_none())
        {
            return Ok(&plan.tags);
        }
        if let Some(framework) = self
            .frameworks
            .values()
            .find(|framework| framework.arn == resource_arn)
        {
            return Ok(&framework.tags);
        }
        if let Some(hold) = self
            .legal_holds
            .values()
            .find(|hold| hold.arn == resource_arn)
        {
            return Ok(&hold.tags);
        }
        if let Some(configuration) = self
            .tiering_configurations
            .values()
            .find(|configuration| configuration.arn == resource_arn)
        {
            return Ok(&configuration.tags);
        }
        if let Some(plan) = self
            .restore_testing_plans
            .values()
            .find(|plan| plan.arn == resource_arn)
        {
            return Ok(&plan.tags);
        }
        Err(super::tagged_resource_not_found())
    }

    /// The SAME resolution for a call that WRITES the tags it resolves, so the
    /// write and the read can never disagree about which record an ARN names.
    pub fn tagged_resource_tags_mut(
        &mut self,
        resource_arn: &str,
    ) -> Result<&mut BTreeMap<String, String>, ResourceNotFoundException> {
        if let Some(vault) = self
            .vaults
            .values_mut()
            .find(|vault| vault.arn == resource_arn)
        {
            return Ok(&mut vault.tags);
        }
        if let Some(point) = self
            .recovery_points
            .values_mut()
            .find(|point| point.arn == resource_arn)
        {
            return Ok(&mut point.tags);
        }
        if let Some(plan) = self
            .backup_plans
            .values_mut()
            .find(|plan| plan.arn == resource_arn && plan.deletion_date.is_none())
        {
            return Ok(&mut plan.tags);
        }
        if let Some(framework) = self
            .frameworks
            .values_mut()
            .find(|framework| framework.arn == resource_arn)
        {
            return Ok(&mut framework.tags);
        }
        if let Some(hold) = self
            .legal_holds
            .values_mut()
            .find(|hold| hold.arn == resource_arn)
        {
            return Ok(&mut hold.tags);
        }
        if let Some(configuration) = self
            .tiering_configurations
            .values_mut()
            .find(|configuration| configuration.arn == resource_arn)
        {
            return Ok(&mut configuration.tags);
        }
        if let Some(plan) = self
            .restore_testing_plans
            .values_mut()
            .find(|plan| plan.arn == resource_arn)
        {
            return Ok(&mut plan.tags);
        }
        Err(super::tagged_resource_not_found())
    }
}
