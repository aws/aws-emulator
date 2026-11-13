// SPDX-License-Identifier: CC0-1.0

//! How an UPDATE of an infrastructure configuration is held.
//!
//! `UpdateInfrastructureConfiguration` "doesn't support selective updates. The request
//! replaces the configuration, so include every setting that you want to keep. Omitted
//! optional properties are cleared" — so an update does not merge into the stored record,
//! it REPLACES it: everything the request describes is written as submitted, every
//! optional property it omits is cleared, and the members an update request cannot carry
//! (the name the ARN was derived from, the resource's own tags, the instant it was
//! created) are what the configuration keeps.
//!
//! Beside the record it carries the two things the operation's own `clientToken` needs,
//! kept apart from the create's: an update is a different operation, so a token spent on
//! one never answers for the other ([`UPDATE_INFRASTRUCTURE_CONFIGURATION_OPERATION`]).

use super::*;

use crate::ops::types::UpdateInfrastructureConfigurationInput;

impl ComAmazonawsImagebuilderImagebuilderState {
    /// Replace the stored configuration with `record` and file the receipt for the update
    /// that wrote it.
    ///
    /// The key is the record's own ARN, which an update never changes, so the replacement
    /// takes the place of what it replaced rather than adding a second entry.
    pub fn replace_infrastructure_configuration(
        &mut self,
        record: InfrastructureConfigurationRecord,
        client_token: &str,
        fingerprint: String,
        request_id: String,
    ) {
        self.file_create_receipt(
            UPDATE_INFRASTRUCTURE_CONFIGURATION_OPERATION,
            &record.region,
            client_token,
            fingerprint,
            request_id,
            record.arn.clone(),
        );
        self.infrastructure_configurations
            .insert(record.arn.clone(), record);
    }

    /// The receipt `client_token` already earned from `UpdateInfrastructureConfiguration`
    /// in `request_region`, if any.
    #[must_use]
    pub fn infrastructure_configuration_update_receipt(
        &self,
        request_region: &str,
        client_token: &str,
    ) -> Option<&CreateReceipt> {
        self.create_receipt(
            UPDATE_INFRASTRUCTURE_CONFIGURATION_OPERATION,
            request_region,
            client_token,
        )
    }
}

/// A deterministic rendering of the settings one `UpdateInfrastructureConfiguration`
/// request carries, so a retry of it can be told from a different request reusing its
/// `clientToken`.
///
/// `clientToken` is left out, for the reason
/// [`infrastructure_configuration_request_fingerprint`] leaves it out: it is the key this
/// rendering is compared under. The ARN is part of it — the same token aimed at a
/// different configuration is a different request.
#[must_use]
pub fn infrastructure_configuration_update_fingerprint(
    input: &UpdateInfrastructureConfigurationInput,
) -> String {
    let mut fields: BTreeMap<&'static str, String> = BTreeMap::new();
    fields.insert(
        "infrastructureConfigurationArn",
        text(Some(input.infrastructure_configuration_arn.as_str())),
    );
    fields.insert("description", text(input.description.as_deref()));
    fields.insert(
        "instanceProfileName",
        text(Some(input.instance_profile_name.as_str())),
    );
    fields.insert("subnetId", text(input.subnet_id.as_deref()));
    fields.insert("keyPair", text(input.key_pair.as_deref()));
    fields.insert("snsTopicArn", text(input.sns_topic_arn.as_deref()));
    fields.insert("instanceTypes", list(input.instance_types.as_deref()));
    fields.insert("securityGroupIds", list(input.security_group_ids.as_deref()));
    fields.insert("resourceTags", tag_map(input.resource_tags.as_ref()));
    fields.insert(
        "terminateInstanceOnFailure",
        flag(input.terminate_instance_on_failure),
    );
    fields.insert(
        "terminateInstanceOnCompletion",
        flag(input.terminate_instance_on_completion),
    );
    fields.insert("logging", logging_fingerprint(input.logging.as_ref()));
    fields.insert(
        "instanceMetadataOptions",
        instance_metadata_options_fingerprint(input.instance_metadata_options.as_ref()),
    );
    fields.insert("placement", placement_fingerprint(input.placement.as_ref()));
    fields.insert(
        "parentInstance",
        parent_instance_fingerprint(input.parent_instance.as_ref()),
    );
    fields
        .iter()
        .map(|(member, rendered)| format!("{member}={rendered};"))
        .collect()
}
