// SPDX-License-Identifier: CC0-1.0

//! Performing a restore: re-creating the protected resource a recovery point
//! was taken of, in the SERVICE that owns it.
//!
//! # The rule this module implements
//!
//! A restore is not a record of intent: it produces a resource, and the
//! resource belongs to another service. So the work here is a call to that
//! service — made the way every emulated service reaches another, with the AWS
//! SDK driven over the emulator's own in-process transport
//! ([`smithy_server_runtime::EmbeddedEmulator`]) — and what the restore reports
//! as its `CreatedResourceArn` is what THAT service answered with, never a
//! value composed here. A resource the owning service does not hold is
//! therefore never reported as created.
//!
//! One arm per resource type whose restore this emulator can perform; the first
//! is Amazon DynamoDB, which the restore of a table recovery point creates
//! through `CreateTable`. The restored table's key schema is the SOURCE table's,
//! read back from DynamoDB itself, so the table a restore creates has the shape
//! the protected resource has. The ITEMS it held are not restored: a backup in
//! this emulator copies no bytes ([`super::RecoveryPoint::backup_size_in_bytes`]
//! stays 0), so there is nothing captured to put back, and capturing a protected
//! resource's contents is the subject of the backup side's own issue.
//!
//! A type with no arm here, and a call the owning service refuses, both end the
//! restore in the modelled failure status carrying the reason
//! ([`RestoreOutcome::Failed`]): nothing is reported created for a restore that
//! created nothing.

use std::collections::BTreeMap;

use smithy_server_runtime::EmbeddedEmulator;

/// The `ResourceType` of an Amazon DynamoDB table, as this service spells it
/// ([`super::SUPPORTED_RESOURCE_TYPES`]).
pub const RESOURCE_TYPE_DYNAMODB: &str = "DynamoDB";

/// The `Metadata` key that names the table a DynamoDB restore creates.
///
/// "You might need to provide a new resource name if the original already
/// exists" (model doc for `StartRestoreJob$Metadata`); for a DynamoDB table
/// that name is carried under this key. Matched case-insensitively, because the
/// metadata of a recovery point is reported in the spelling the protected
/// resource's own service uses and a client passes it back as it received it.
pub const METADATA_TARGET_TABLE_NAME: &str = "targetTableName";

/// The `BillingMode` of a table that is billed per request, as DynamoDB spells
/// it: the mode a restored table is created in when the source table is in it.
const BILLING_MODE_PAY_PER_REQUEST: &str = "PAY_PER_REQUEST";

/// The provisioned capacity a restored table is created with when the source
/// table is provisioned but reports no capacity of its own — DynamoDB requires
/// a positive value for each, and this is the smallest pair that satisfies it.
const DEFAULT_PROVISIONED_CAPACITY: i64 = 1;

/// What performing a restore came to.
#[derive(Debug, Clone)]
pub enum RestoreOutcome {
    /// The resource was created, and this is the ARN the service that owns it
    /// answered with — so a read of that service reports the resource this
    /// restore says it created.
    Created(String),
    /// Nothing was created, and this says why. It becomes the restore job's
    /// `StatusMessage`, beside the modelled failure status.
    Failed(String),
}

/// Re-create the protected resource `source_resource_arn` names, as the restore
/// metadata asks for, and answer what that came to.
///
/// Called with NO state lock held: the hop below is an await, and this
/// service's store is not held across one.
pub async fn perform_restore(
    callback: &EmbeddedEmulator,
    region: &str,
    resource_type: Option<&str>,
    source_resource_arn: &str,
    metadata: &BTreeMap<String, String>,
    source_tags: &BTreeMap<String, String>,
    copy_source_tags: bool,
) -> RestoreOutcome {
    match resource_type {
        Some(RESOURCE_TYPE_DYNAMODB) => {
            restore_dynamodb_table(
                callback,
                region,
                source_resource_arn,
                metadata,
                source_tags,
                copy_source_tags,
            )
            .await
        }
        // A type this emulator cannot re-create has no restore to perform, so
        // the job says so rather than reporting a resource nothing created.
        Some(other) => RestoreOutcome::Failed(format!(
            "this emulator performs no restore of a {other} recovery point, so no resource was \
             created"
        )),
        None => RestoreOutcome::Failed(
            "the restore names no resource type, so there is no resource to create".to_string(),
        ),
    }
}

/// The DynamoDB arm: create the table the metadata names, shaped like the table
/// the recovery point was taken of.
async fn restore_dynamodb_table(
    callback: &EmbeddedEmulator,
    region: &str,
    source_resource_arn: &str,
    metadata: &BTreeMap<String, String>,
    source_tags: &BTreeMap<String, String>,
    copy_source_tags: bool,
) -> RestoreOutcome {
    let Some(target_table_name) = metadata_value(metadata, METADATA_TARGET_TABLE_NAME) else {
        // A start whose metadata names no target table is refused when it is made
        // ([`super::rejected_restore_metadata`]), so no stored job reaches here
        // without one; a restore that has no target still creates nothing rather
        // than choosing a name of its own.
        return RestoreOutcome::Failed(format!(
            "the restore metadata names no {METADATA_TARGET_TABLE_NAME}, so there is no table to \
             create"
        ));
    };
    let Some(source_table_name) = table_name_of(source_resource_arn) else {
        return RestoreOutcome::Failed(
            "the recovery point's resource is not a DynamoDB table, so there is no table to \
             create"
                .to_string(),
        );
    };

    let client = dynamodb_client(callback, region);
    // The shape of the table to create is the SOURCE table's, read back from
    // the service that holds it rather than guessed here.
    let source = match client
        .describe_table()
        .table_name(source_table_name)
        .send()
        .await
    {
        Ok(answer) => answer,
        Err(error) => {
            return RestoreOutcome::Failed(format!(
                "the table the recovery point was taken of could not be read: {}",
                ::aws_smithy_types::error::display::DisplayErrorContext(&error)
            ))
        }
    };
    let Some(source) = source.table else {
        return RestoreOutcome::Failed(
            "the table the recovery point was taken of reports no description".to_string(),
        );
    };

    let mut create = client
        .create_table()
        .table_name(target_table_name)
        .set_key_schema(source.key_schema.clone())
        .set_attribute_definitions(source.attribute_definitions.clone());
    // One of the two is required by `CreateTable`, and which one is the source
    // table's own billing mode.
    let pay_per_request = source
        .billing_mode_summary
        .as_ref()
        .and_then(|summary| summary.billing_mode.as_ref())
        .is_some_and(|mode| mode.as_str() == BILLING_MODE_PAY_PER_REQUEST);
    if pay_per_request {
        create = create.billing_mode(::aws_sdk_dynamodb::types::BillingMode::PayPerRequest);
    } else {
        let read = source
            .provisioned_throughput
            .as_ref()
            .and_then(|throughput| throughput.read_capacity_units)
            .filter(|units| *units > 0)
            .unwrap_or(DEFAULT_PROVISIONED_CAPACITY);
        let write = source
            .provisioned_throughput
            .as_ref()
            .and_then(|throughput| throughput.write_capacity_units)
            .filter(|units| *units > 0)
            .unwrap_or(DEFAULT_PROVISIONED_CAPACITY);
        create = create.provisioned_throughput(
            ::aws_sdk_dynamodb::types::ProvisionedThroughput::builder()
                .read_capacity_units(read)
                .write_capacity_units(write)
                .build()
                .expect("a provisioned throughput with both capacities set"),
        );
    }
    // "If this equals True, tags included in the backup will be copied to the
    // restored resource" (model doc for
    // `StartRestoreJob$CopySourceTagsToRestoredResource`) — the tags included in
    // the backup are the ones the recovery point carries.
    if copy_source_tags {
        for (key, value) in source_tags {
            create = create.tags(
                ::aws_sdk_dynamodb::types::Tag::builder()
                    .key(key)
                    .value(value)
                    .build()
                    .expect("a tag with both members set"),
            );
        }
    }

    match create.send().await {
        // The ARN reported is the one DynamoDB answered with, so the resource
        // this restore reports created is one that service holds.
        Ok(created) => match created
            .table_description
            .and_then(|description| description.table_arn)
        {
            Some(table_arn) => RestoreOutcome::Created(table_arn),
            None => RestoreOutcome::Failed(
                "the restored table was created but its own service reported no ARN for it"
                    .to_string(),
            ),
        },
        Err(error) => RestoreOutcome::Failed(format!(
            "the restored table could not be created: {}",
            ::aws_smithy_types::error::display::DisplayErrorContext(&error)
        )),
    }
}

/// The value the restore metadata carries under `name`, matched
/// case-insensitively — see [`METADATA_TARGET_TABLE_NAME`].
///
/// `pub(crate)` because the SYNCHRONOUS refusal of metadata that names no such key reads it
/// through the same step the restore does ([`super::rejected_restore_metadata`]), so the
/// refusal and the restore can never disagree about which keys a request carries.
pub(crate) fn metadata_value<'m>(
    metadata: &'m BTreeMap<String, String>,
    name: &str,
) -> Option<&'m str> {
    metadata
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str())
        .filter(|value| !value.is_empty())
}

/// The table name a DynamoDB table ARN names: the resource field is
/// `table/<name>`.
///
/// `pub(crate)` because asking DynamoDB whether it HOLDS that table reads the ARN through
/// the same step ([`super::protected_resource_answer`]).
pub(crate) fn table_name_of(resource_arn: &str) -> Option<&str> {
    let resource = resource_arn.splitn(6, ':').nth(5)?;
    resource.strip_prefix("table/").filter(|name| !name.is_empty())
}

/// The Amazon DynamoDB client a restore is performed through, and that the lookup of a
/// protected table asks ([`super::protected_resource_answer`]) — ONE construction of the
/// hop for both directions of the same question.
///
/// Configured as every other in-process client in this tree is
/// (`::service_common::eventbridge_client`): the callback routes IN PROCESS to
/// this emulator's own inbound listener, so the endpoint URL is addressing for
/// that transport rather than a host anything connects to; `region` is the
/// Region the restore runs in; and the credentials are the callback's, which
/// sign this hop and never appear in a response. There is nothing to retry,
/// time out or watch for a stalled stream over an in-process transport, and each
/// of those would need a sleep implementation this client is not given.
///
/// Built here rather than in `services/common` on purpose: that crate is a
/// dependency of EVERY service crate, so the DynamoDB SDK declared there would
/// be compiled into every one of them for the sake of this one constructor.
/// The sibling copy in the Redshift crate's zero-ETL source lookup is the other
/// service that asks DynamoDB something, and it is configured identically.
pub(crate) fn dynamodb_client(
    callback: &EmbeddedEmulator,
    region: &str,
) -> ::aws_sdk_dynamodb::Client {
    let config = ::aws_sdk_dynamodb::Config::builder()
        .behavior_version(::aws_sdk_dynamodb::config::BehaviorVersion::latest())
        .region(::aws_sdk_dynamodb::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://dynamodb.{region}.amazonaws.com"))
        .credentials_provider(callback.aws_credentials_provider())
        .http_client(callback.http_client())
        .retry_config(::aws_sdk_dynamodb::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_dynamodb::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_dynamodb::config::IdentityCache::no_cache())
        .stalled_stream_protection(
            ::aws_sdk_dynamodb::config::StalledStreamProtectionConfig::disabled(),
        )
        .build();
    ::aws_sdk_dynamodb::Client::from_conf(config)
}
