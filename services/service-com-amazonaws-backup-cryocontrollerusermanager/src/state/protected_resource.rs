// SPDX-License-Identifier: CC0-1.0

//! Whether the PROTECTED RESOURCE a job was started for exists at all, asked of the service
//! that owns it.
//!
//! # The rule this module implements
//!
//! A backup captures a resource that belongs to ANOTHER service, and the `ResourceArn` naming
//! it is not resolved when the call is made: `StartBackupJob` is accepted on what it can be
//! judged on synchronously — the vault, the parameters — and the backup then runs against
//! whatever that ARN names. So a resource the owning service does not hold is a backup that
//! can capture nothing: the job produces no recovery point and its TERMINAL state is the
//! failure one, carrying why ([`super::BackupFailure`]).
//!
//! The question is therefore put to the service that OWNS the resource, never answered from
//! this service's own records: the owning service is the only component that knows whether it
//! holds the resource, and a resource it reports absent is one no backup of this account could
//! read. It is asked over the emulator's own in-process transport
//! ([`smithy_server_runtime::EmbeddedEmulator`]), the same hop a restore creates the resource
//! back through ([`super::perform_restore`]), with no store guard held.
//!
//! One arm per Backup resource type ([`super::resource_type_of`]) whose owning service this
//! emulator can ask; the first is Amazon DynamoDB, whose tables `DescribeTable` resolves.
//!
//! Nothing is decided on an answer this emulator could not get — a type with no arm here, an
//! ARN that names no resource of that type, an emulator with the owning service not mounted,
//! and any other failure of the hop all leave the job's outcome alone, the way the role
//! question treats the silences it meets ([`super::withheld_backup`]). Only the owning
//! service ANSWERING that it holds no such resource grounds a failure, so a job is never
//! failed for a lookup this emulator merely could not make.

use smithy_server_runtime::EmbeddedEmulator;

/// What the service that owns a protected resource answered about it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtectedResource {
    /// The owning service reports that it holds the resource.
    Held,
    /// The owning service ANSWERED that it holds no such resource.
    Absent,
    /// Nothing was learned: see the module docs.
    Unknown,
}

/// What the service that owns `resource_arn` answers about it, for a resource of
/// `resource_type` ([`super::resource_type_of`]).
///
/// Asked with NO store guard held: it makes one in-process call and opens no socket.
pub async fn protected_resource_answer(
    callback: &EmbeddedEmulator,
    region: &str,
    resource_type: Option<&str>,
    resource_arn: &str,
) -> ProtectedResource {
    match resource_type {
        Some(super::RESOURCE_TYPE_DYNAMODB) => {
            dynamodb_table_answer(callback, region, resource_arn).await
        }
        // A type whose owning service this emulator cannot ask decides nothing.
        _ => ProtectedResource::Unknown,
    }
}

/// The Amazon DynamoDB arm: the table the ARN names, as DynamoDB itself reports it.
///
/// The client and the table-name reading are the ones the restore of a table recovery point
/// uses ([`super::perform_restore`]), so the two directions of the same question reach
/// DynamoDB through one hop and read one ARN form.
async fn dynamodb_table_answer(
    callback: &EmbeddedEmulator,
    region: &str,
    resource_arn: &str,
) -> ProtectedResource {
    let Some(table_name) = super::table_name_of(resource_arn) else {
        return ProtectedResource::Unknown;
    };
    let answer = super::dynamodb_client(callback, region)
        .describe_table()
        .table_name(table_name)
        .send()
        .await;
    match answer {
        // An answer carrying no table description is this emulator failing to READ an
        // answer rather than DynamoDB reporting the table absent.
        Ok(described) => {
            if described.table.is_some() {
                ProtectedResource::Held
            } else {
                ProtectedResource::Unknown
            }
        }
        Err(error) => {
            if error.as_service_error().is_some_and(
                ::aws_sdk_dynamodb::operation::describe_table::DescribeTableError::is_resource_not_found_exception,
            ) {
                ProtectedResource::Absent
            } else {
                ProtectedResource::Unknown
            }
        }
    }
}
