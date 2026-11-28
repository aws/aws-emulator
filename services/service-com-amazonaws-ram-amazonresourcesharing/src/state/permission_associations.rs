// SPDX-License-Identifier: CC0-1.0

//! The same relation as [`super::permission_attachments`], read from the PERMISSION's
//! side: which resource shares one managed permission governs.
//!
//! A managed permission and a resource share are associated exactly while that share is
//! governed by that permission, which is one relation with two reads: a read of a
//! share's permissions reports the permissions of one share
//! ([`ComAmazonawsRamAmazonresourcesharingState::share_permissions`]), and a read of a
//! permission's associations reports the shares of one permission, "which resource
//! shares use which versions of the specified managed permission"
//! (`ListPermissionAssociations`' own documentation). Both are derived from the same
//! attachments, so the two can never disagree about whether a share uses a permission.
//!
//! An association carries the version of the permission the share is governed by, so a
//! share left on an older version is reported at that version with
//! `AssociatedPermission$defaultVersion` false, and the stamps and statuses it reports
//! are the ones the attachment and the permission already hold.

use super::{
    decode_cursor, is_permission_arn, is_share_arn, to_date_time,
    ComAmazonawsRamAmazonresourcesharingState, FIELD_SEPARATOR,
    SHARE_PERMISSION_STATUS_ASSOCIATED,
};
use crate::ops::types::AssociatedPermission;

/// One association between one managed permission and one resource share.
#[derive(Debug, Clone)]
pub struct PermissionAssociationRecord {
    /// The ARN of the associated managed permission.
    pub permission_arn: String,
    /// The ARN of the resource share it governs.
    pub resource_share_arn: String,
    /// The version of the permission that share is governed by.
    pub permission_version: i32,
    /// Whether that is the permission's current default version
    /// (`AssociatedPermission$defaultVersion`: "indicates whether the associated resource
    /// share is using the default version of the permission").
    pub default_version: bool,
    /// The resource type the permission applies to.
    pub resource_type: String,
    /// "The current status of the association between the permission and the resource
    /// share" (`AssociatedPermission$status`).
    ///
    /// What is reported here is the status of the ASSOCIATION rather than of either side
    /// of it: the request filters on it with a member that takes a
    /// `ResourceShareAssociationStatus`
    /// (`ListPermissionAssociationsRequest$associationStatus`, "list only those
    /// associations with resource shares that match this status"), and a filter matches
    /// the values of the field it filters. So an association this store holds — one a
    /// share is currently governed by — reports that it is associated
    /// ([`SHARE_PERMISSION_STATUS_ASSOCIATED`]), exactly as a read of the same
    /// attachment from the share's side does.
    pub status: String,
    /// The `featureSet` of the associated resource share, which is what that member of
    /// an association reports: "indicates what features are available for this resource
    /// share" (`AssociatedPermission$featureSet`).
    pub feature_set: String,
    /// When the association was last updated, in epoch milliseconds.
    pub last_updated_time: i64,
}

impl PermissionAssociationRecord {
    /// The pair this association is identified by, which is also the order a paginated
    /// read pages over: the permission, then the share it governs.
    pub fn key(&self) -> (&str, &str) {
        (&self.permission_arn, &self.resource_share_arn)
    }

    /// This association as the modelled `AssociatedPermission` wire shape.
    ///
    /// `permissionVersion` is modelled as a STRING
    /// (`smithy_query(query="shape",shape="AssociatedPermission")`), so the version
    /// number is rendered as one.
    pub fn view(&self) -> AssociatedPermission {
        AssociatedPermission {
            arn: Some(self.permission_arn.clone()),
            permission_version: Some(self.permission_version.to_string()),
            default_version: Some(self.default_version),
            resource_type: Some(self.resource_type.clone()),
            status: Some(self.status.clone()),
            feature_set: Some(self.feature_set.clone()),
            last_updated_time: Some(to_date_time(self.last_updated_time)),
            resource_share_arn: Some(self.resource_share_arn.clone()),
            // The model declares template parameters on the association; a permission
            // this slice holds declares none, so there is nothing to report.
            permission_template_parameters: None,
        }
    }
}

impl ComAmazonawsRamAmazonresourcesharingState {
    /// Every association between a managed permission and a live resource share, in
    /// (permission ARN, share ARN) order — the order a paginated read pages in — and
    /// each pair once.
    ///
    /// The associations of one share are exactly the permissions that govern it
    /// ([`Self::share_permissions`]): the ones it holds by name, each at the version
    /// that attachment carries, plus the default managed permission of every resource
    /// type it shares that none of those applies to. A share `DeleteResourceShare` has
    /// removed is governed by nothing, so it is associated with no permission either.
    pub fn permission_associations(&self) -> Vec<PermissionAssociationRecord> {
        let mut associations: Vec<PermissionAssociationRecord> = self
            .resource_shares
            .values()
            .filter(|share| !share.is_deleted())
            .flat_map(|share| {
                self.share_permissions(&share.arn)
                    .into_iter()
                    .map(move |holder| PermissionAssociationRecord {
                        permission_arn: holder.arn().to_string(),
                        resource_share_arn: share.arn.clone(),
                        permission_version: holder.version.version,
                        default_version: holder.version.version
                            == holder.permission.default_version,
                        resource_type: holder.permission.resource_type.clone(),
                        status: SHARE_PERMISSION_STATUS_ASSOCIATED.to_string(),
                        feature_set: share.feature_set.clone(),
                        last_updated_time: holder.last_updated_time,
                    })
            })
            .collect();
        associations.sort_by(|left, right| left.key().cmp(&right.key()));
        associations
    }
}

/// A `nextToken` naming the association the next page starts at.
///
/// An association is identified by the PAIR it relates, so the value a token carries is
/// both ARNs, hex-encoded the way every other cursor of this service is — opaque to a
/// caller, not secret.
pub fn encode_permission_association_cursor(
    permission_arn: &str,
    resource_share_arn: &str,
) -> String {
    hex::encode(format!(
        "{permission_arn}{FIELD_SEPARATOR}{resource_share_arn}"
    ))
}

/// The (permission ARN, share ARN) pair a `nextToken` names, or `None` when the token is
/// not one this service issued — the `InvalidNextTokenException` case.
///
/// A token this service issued names a permission and a share, whether or not either
/// still exists, so the test is on the FORM of both values, as every other decoder's is.
pub fn decode_permission_association_cursor(token: &str) -> Option<(String, String)> {
    let decoded = decode_cursor(token)?;
    let (permission_arn, resource_share_arn) = decoded.split_once(FIELD_SEPARATOR)?;
    (is_permission_arn(permission_arn) && is_share_arn(resource_share_arn))
        .then(|| (permission_arn.to_string(), resource_share_arn.to_string()))
}
