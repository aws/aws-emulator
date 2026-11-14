// SPDX-License-Identifier: CC0-1.0

//! The account's ROLE ALIASES: the record one holds, and what every reader of one
//! reports.
//!
//! A role alias is a NAME that "points to a role ARN. This allows you to change the
//! role without having to update the device" (`CreateRoleAlias$roleAlias`), together
//! with how long the credentials handed out under it stay valid. The name is the
//! alias's identity — the model binds it as the `@httpLabel` of
//! `/role-aliases/{roleAlias}` on every single-alias operation
//! (`smithy_query(query="operation",operation="CreateRoleAlias")`) — which is why the
//! store is a `BTreeMap` keyed by it: the map IS the registry, and `ListRoleAliases`
//! is `@paginated`, so a page boundary is only meaningful over a stable order.
//!
//! One record carries everything a reader hands back, so `DescribeRoleAlias` after
//! `CreateRoleAlias` reports the created alias member for member: the reader clones
//! what the writer stored and the two cannot drift.

use std::time::SystemTime;

use crate::ops::types::RoleAliasDescription;

/// How many role aliases this emulator keeps for the account.
///
/// The store is one a CALLER grows — one record per `CreateRoleAlias` — so it is
/// bounded, for the same reason [`super::MAX_AUTHORIZERS`] is: past the ceiling nothing
/// is recorded and the create is refused with the modelled `LimitExceededException`
/// rather than growing without bound.
pub const MAX_ROLE_ALIASES: usize = 100;

/// How many tags one role alias may carry: the same documented IoT tagging limit
/// [`super::MAX_POLICY_TAGS`] records, for the same reason — `TagList` has no modelled
/// maximum, so without a ceiling one request would choose how large a single record is.
pub const MAX_ROLE_ALIAS_TAGS: usize = super::MAX_POLICY_TAGS;

/// The `credentialDurationSeconds` a role alias created without one carries.
///
/// `CreateRoleAlias$credentialDurationSeconds` is documented "How long (in seconds) the
/// credentials will be valid. The default value is 3,600 seconds", and it sits inside
/// the member's modelled `range 900..43200`, so a create that names no duration yields
/// an alias whose credentials last an hour.
pub const DEFAULT_CREDENTIAL_DURATION_SECONDS: i32 = 3_600;

/// The ARN of a role alias:
/// `arn:aws:iot:<region>:<account>:rolealias/<roleAlias>`.
///
/// `rolealias/` is the resource type AWS IoT's role-alias ARNs use — it is what
/// `CreateRoleAlias$roleAliasArn` / `RoleAliasDescription$roleAliasArn` report and what
/// an `iot:AssumeRoleWithCertificate` policy statement names an alias by; the namespace
/// and account segments are the ones [`super::thing_arn`] documents.
pub fn role_alias_arn(region: &str, role_alias: &str) -> String {
    ::emulator_server::emulator_arn("iot", region, &format!("rolealias/{role_alias}"))
}

/// The message every operation that addresses ONE role alias refuses a name the account
/// holds no alias under with: the family's fixed text, with the name taken from the
/// request.
pub fn missing_role_alias_message(role_alias: &str) -> String {
    format!("Role alias {role_alias} not found")
}

/// One role alias in the role-alias store, as this service persists it.
#[derive(Debug, Clone)]
pub struct RegisteredRoleAlias {
    /// "The role alias" — its identity, and its key in the store.
    pub role_alias: String,
    /// "The ARN of the role alias" ([`role_alias_arn`]), minted on the create.
    pub role_alias_arn: String,
    /// "The role ARN" the alias points to, which a later update may replace.
    pub role_arn: String,
    /// "The role alias owner": the account that created it.
    pub owner: String,
    /// "The number of seconds for which the credential is valid"
    /// ([`DEFAULT_CREDENTIAL_DURATION_SECONDS`] when the create named none).
    pub credential_duration_seconds: i32,
    /// "The UNIX timestamp of when the role alias was created".
    pub creation_date: SystemTime,
    /// "The UNIX timestamp of when the role alias was last modified" — equal to
    /// [`RegisteredRoleAlias::creation_date`] until a later write moves it.
    pub last_modified_date: SystemTime,
    /// The tags a create supplied, "Metadata which can be used to manage the role
    /// alias". Keyed by tag key, because `Tag$Key` is `@required` and keys a resource's
    /// tag set, so the last value given for a key is the one the alias carries. No
    /// `RoleAliasDescription` member reports them: they manage the alias rather than
    /// describe it.
    pub tags: ::std::collections::BTreeMap<String, Option<String>>,
}

impl RegisteredRoleAlias {
    /// Record `role_alias` as created at `now` in `region`, owned by `owner`, pointing
    /// at `role_arn` for `credential_duration_seconds` (the default when the create
    /// named none).
    pub fn new(
        region: &str,
        role_alias: String,
        role_arn: String,
        owner: String,
        credential_duration_seconds: Option<i32>,
        tags: ::std::collections::BTreeMap<String, Option<String>>,
        now: SystemTime,
    ) -> Self {
        Self {
            role_alias_arn: role_alias_arn(region, &role_alias),
            role_alias,
            role_arn,
            owner,
            credential_duration_seconds: credential_duration_seconds
                .unwrap_or(DEFAULT_CREDENTIAL_DURATION_SECONDS),
            creation_date: now,
            last_modified_date: now,
            tags,
        }
    }

    /// This record as the `RoleAliasDescription` every reader of one role alias
    /// reports: one function, so two readers cannot describe the same record
    /// differently.
    pub fn description(&self) -> RoleAliasDescription {
        RoleAliasDescription {
            role_alias: Some(self.role_alias.clone()),
            role_alias_arn: Some(self.role_alias_arn.clone()),
            role_arn: Some(self.role_arn.clone()),
            owner: Some(self.owner.clone()),
            credential_duration_seconds: Some(self.credential_duration_seconds),
            creation_date: Some(::aws_smithy_types::DateTime::from(self.creation_date)),
            last_modified_date: Some(::aws_smithy_types::DateTime::from(self.last_modified_date)),
        }
    }

    /// Apply an `UpdateRoleAlias` to this record at `now`.
    ///
    /// `UpdateRoleAlias` takes `roleArn` and `credentialDurationSeconds` as OPTIONAL
    /// members beside the `@httpLabel` that names the alias
    /// (`smithy_query(query="operation",operation="UpdateRoleAlias")`), so an update
    /// replaces exactly the members the request carried and leaves every other one as
    /// the create wrote it — the alias's name, its ARN and its owner are its identity
    /// and no update member names them. The moment of the write becomes the record's
    /// "UNIX timestamp of when the role alias was last modified", which is what every
    /// later reader reports.
    pub fn apply_update(
        &mut self,
        role_arn: Option<String>,
        credential_duration_seconds: Option<i32>,
        now: SystemTime,
    ) {
        if let Some(role_arn) = role_arn {
            self.role_arn = role_arn;
        }
        if let Some(credential_duration_seconds) = credential_duration_seconds {
            self.credential_duration_seconds = credential_duration_seconds;
        }
        self.last_modified_date = now;
    }
}
