// SPDX-License-Identifier: CC0-1.0

//! The syntax every name this service admits has to satisfy: cluster, parameter
//! group, subnet group, snapshot, grant and subscription identifiers, database
//! and user names, and the credential and token-scope values a session request
//! carries.

use super::*;

/// The syntax rule Redshift documents for one identifier member.
#[derive(Debug, Clone, Copy)]
pub struct IdentifierRule {
    /// The input member the rule belongs to, named in the refusal.
    pub member: &'static str,
    /// The documented maximum length, in characters.
    pub max_len: usize,
    /// Whether the documented rule forbids uppercase letters.
    pub lowercase_only: bool,
}

/// The rule for a cluster identifier (`ClusterIdentifier`, `NewClusterIdentifier`).
///
/// Source: `smithy_query(query="doc", shape="CreateClusterMessage",
/// member="ClusterIdentifier")` — "Must contain from 1 to 63 alphanumeric
/// characters or hyphens. Alphabetic characters must be lowercase. First
/// character must be a letter. Cannot end with a hyphen or contain two
/// consecutive hyphens."
pub const CLUSTER_IDENTIFIER_RULE: IdentifierRule = IdentifierRule {
    member: "ClusterIdentifier",
    max_len: 63,
    lowercase_only: true,
};

/// The rule for a cluster parameter group name (`ParameterGroupName`).
///
/// Source: `smithy_query(query="doc", shape="CreateClusterParameterGroupMessage",
/// member="ParameterGroupName")` — "Must be 1 to 255 alphanumeric characters or
/// hyphens First character must be a letter. Cannot end with a hyphen or contain
/// two consecutive hyphens." Mixed case is ACCEPTED and folded by
/// [`normalized_parameter_group_name`] rather than refused, because the same doc
/// says the value "is stored as a lower-case string".
pub const PARAMETER_GROUP_NAME_RULE: IdentifierRule = IdentifierRule {
    member: "ParameterGroupName",
    max_len: 255,
    lowercase_only: false,
};

/// The form a cluster parameter group name is STORED and reported in.
///
/// Source: the same doc sentence as [`PARAMETER_GROUP_NAME_RULE`] — "This value
/// is stored as a lower-case string." The rule admits only ASCII alphanumerics
/// and hyphens, so ASCII folding is the whole of it.
pub fn normalized_parameter_group_name(requested: &str) -> String {
    requested.to_ascii_lowercase()
}

/// The rule for a cluster subnet group name (`ClusterSubnetGroupName`).
///
/// Source: `smithy_query(query="doc", shape="CreateClusterSubnetGroupMessage",
/// member="ClusterSubnetGroupName")` — "Must contain no more than 255
/// alphanumeric characters or hyphens. Must not be "Default". Must be unique for
/// all subnet groups that are created by your Amazon Web Services account."
/// Mixed case is ACCEPTED and folded by [`normalized_subnet_group_name`] rather
/// than refused, because the same doc says "Amazon Redshift stores the value as a
/// lowercase string".
pub const SUBNET_GROUP_NAME_RULE: IdentifierRule = IdentifierRule {
    member: "ClusterSubnetGroupName",
    max_len: 255,
    lowercase_only: false,
};

/// The one name a cluster subnet group may not take, compared in the stored
/// (folded) form.
///
/// Source: the same doc sentence as [`SUBNET_GROUP_NAME_RULE`] — "Must not be
/// "Default"".
pub const RESERVED_SUBNET_GROUP_NAME: &str = "default";

/// The form a cluster subnet group name is STORED and reported in.
///
/// Source: `smithy_query(query="doc", shape="CreateClusterSubnetGroupMessage",
/// member="ClusterSubnetGroupName")` — "Amazon Redshift stores the value as a
/// lowercase string." The rule admits only ASCII alphanumerics and hyphens, so
/// ASCII folding is the whole of it.
pub fn normalized_subnet_group_name(requested: &str) -> String {
    requested.to_ascii_lowercase()
}

/// The rule for a snapshot identifier (`SnapshotIdentifier`,
/// `FinalClusterSnapshotIdentifier`).
///
/// Source: `smithy_query(query="doc", shape="CreateClusterSnapshotMessage",
/// member="SnapshotIdentifier")` — "Must contain from 1 to 255 alphanumeric
/// characters or hyphens. First character must be a letter. Cannot end with a
/// hyphen or contain two consecutive hyphens." (No lowercase restriction.)
pub const SNAPSHOT_IDENTIFIER_RULE: IdentifierRule = IdentifierRule {
    member: "SnapshotIdentifier",
    max_len: 255,
    lowercase_only: false,
};

/// The rule for a snapshot copy grant name (`SnapshotCopyGrantName`).
///
/// Source: `smithy_query(query="doc", shape="CreateSnapshotCopyGrantMessage",
/// member="SnapshotCopyGrantName")` — "Must contain from 1 to 63 alphanumeric
/// characters or hyphens. Alphabetic characters must be lowercase. First
/// character must be a letter. Cannot end with a hyphen or contain two
/// consecutive hyphens."
pub const SNAPSHOT_COPY_GRANT_NAME_RULE: IdentifierRule = IdentifierRule {
    member: "SnapshotCopyGrantName",
    max_len: 63,
    lowercase_only: true,
};

/// The rule for an event notification subscription name (`SubscriptionName`).
///
/// Source: `smithy_query(query="doc", shape="CreateEventSubscriptionMessage",
/// member="SubscriptionName")` — "Cannot be null, empty, or blank. Must contain
/// from 1 to 255 alphanumeric characters or hyphens. First character must be a
/// letter. Cannot end with a hyphen or contain two consecutive hyphens." (No
/// lowercase restriction.)
pub const EVENT_SUBSCRIPTION_NAME_RULE: IdentifierRule = IdentifierRule {
    member: "SubscriptionName",
    max_len: 255,
    lowercase_only: false,
};

/// Check `value` against `rule`, naming the member and the constraint it broke.
///
/// `member` overrides the rule's own member name, so `NewClusterIdentifier` and
/// `FinalClusterSnapshotIdentifier` are refused under the same rule as the
/// member it was documented on, while still naming themselves.
pub fn check_identifier(rule: IdentifierRule, member: &str, value: &str) -> Result<(), String> {
    let refuse = |why: &str| {
        Err(format!(
            "Invalid value for {member}: '{value}'. {why} (allowed: 1 to {} {}alphanumeric characters or hyphens, first character a letter, no trailing hyphen and no two consecutive hyphens)",
            rule.max_len,
            if rule.lowercase_only { "lowercase " } else { "" },
        ))
    };
    if value.is_empty() {
        return refuse("The identifier cannot be null, empty, or blank.");
    }
    if value.chars().count() > rule.max_len {
        return refuse("The identifier is longer than allowed.");
    }
    if !value
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-')
    {
        return refuse("The identifier contains a character that is neither alphanumeric nor a hyphen.");
    }
    if rule.lowercase_only && value.chars().any(|c| c.is_ascii_uppercase()) {
        return refuse("Alphabetic characters must be lowercase.");
    }
    if !value
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic())
    {
        return refuse("The first character must be a letter.");
    }
    if value.ends_with('-') {
        return refuse("The identifier cannot end with a hyphen.");
    }
    if value.contains("--") {
        return refuse("The identifier cannot contain two consecutive hyphens.");
    }
    Ok(())
}

/// The syntax rule Redshift documents for a DATABASE object name — a database
/// user, a database name or a database group name.
///
/// A different alphabet from [`IdentifierRule`]'s, which is why it is its own
/// rule: a control-plane identifier admits alphanumerics and hyphens, while a
/// database object name also admits underscore, plus sign, period, at symbol and
/// hyphen, and the three members differ only in whether an uppercase letter is
/// admitted.
#[derive(Debug, Clone, Copy)]
pub struct DatabaseNameRule {
    /// The input member the rule belongs to, named in the refusal.
    pub member: &'static str,
    /// Whether the documented rule forbids uppercase letters.
    pub lowercase_only: bool,
    /// Whether the documented rule forbids the value `PUBLIC`, the group every
    /// database user is in ([`PUBLIC_DATABASE_GROUP`]).
    pub forbids_public: bool,
}

/// The documented maximum length, in characters, of every database object name
/// below: "Must be 1 to 64 alphanumeric characters or hyphens".
pub const DATABASE_NAME_MAX_LEN: usize = 64;

/// The rule for the database user a credential is issued for (`DbUser`).
///
/// Source: `smithy_query(query="doc", shape="GetClusterCredentialsMessage",
/// member="DbUser")` — "Must be 1 to 64 alphanumeric characters or hyphens. The
/// user name can't be PUBLIC. Must contain uppercase or lowercase letters,
/// numbers, underscore, plus sign, period (dot), at symbol (@), or hyphen. First
/// character must be a letter. Must not contain a colon ( : ) or slash ( / )."
pub const DB_USER_RULE: DatabaseNameRule = DatabaseNameRule {
    member: "DbUser",
    lowercase_only: false,
    forbids_public: true,
};

/// The rule for the database a credential authorises logging on to (`DbName`).
///
/// Source: `smithy_query(query="doc", shape="GetClusterCredentialsMessage",
/// member="DbName")` — the same sentences as [`DB_USER_RULE`]'s, without the
/// `PUBLIC` exclusion: "Must be 1 to 64 alphanumeric characters or hyphens Must
/// contain uppercase or lowercase letters, numbers, underscore, plus sign,
/// period (dot), at symbol (@), or hyphen. First character must be a letter.
/// Must not contain a colon ( : ) or slash ( / )."
pub const DB_NAME_RULE: DatabaseNameRule = DatabaseNameRule {
    member: "DbName",
    lowercase_only: false,
    forbids_public: false,
};

/// The rule for one database group a session joins (`DbGroups`).
///
/// Source: `smithy_query(query="doc", shape="GetClusterCredentialsMessage",
/// member="DbGroups")` — "Must be 1 to 64 alphanumeric characters or hyphens
/// Must contain ONLY LOWERCASE letters, numbers, underscore, plus sign, period
/// (dot), at symbol (@), or hyphen. First character must be a letter. Must not
/// contain a colon ( : ) or slash ( / )."
pub const DB_GROUP_RULE: DatabaseNameRule = DatabaseNameRule {
    member: "DbGroups",
    lowercase_only: true,
    forbids_public: false,
};

/// Check one database object name against `rule`, naming the member and the
/// constraint it broke.
///
/// The documented alphabet is the whole of what is admitted, so the colon and
/// the slash the same documentation calls out separately cannot pass it either.
/// The documentation's remaining constraint, "cannot be a reserved word", is a
/// list published in the Amazon Redshift Database Developer Guide rather than
/// anything the API model carries, so it is not checked here.
pub fn check_database_name(rule: DatabaseNameRule, value: &str) -> Result<(), String> {
    let member = rule.member;
    let refuse = |why: &str| {
        Err(format!(
            "Invalid value for {member}: '{value}'. {why} (allowed: 1 to {DATABASE_NAME_MAX_LEN} {}letters, numbers, underscore, plus sign, period, at symbol or hyphen, first character a letter)",
            if rule.lowercase_only { "lowercase " } else { "" },
        ))
    };
    if value.is_empty() {
        return refuse("The name cannot be null, empty, or blank.");
    }
    if value.chars().count() > DATABASE_NAME_MAX_LEN {
        return refuse("The name is longer than allowed.");
    }
    if !value
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '+' | '.' | '@' | '-'))
    {
        return refuse("The name contains a character that is not allowed in it.");
    }
    if rule.lowercase_only && value.chars().any(|c| c.is_ascii_uppercase()) {
        return refuse("Alphabetic characters must be lowercase.");
    }
    if !value.chars().next().is_some_and(|c| c.is_ascii_alphabetic()) {
        return refuse("The first character must be a letter.");
    }
    // Redshift folds an unquoted database identifier to lower case, so the
    // excluded name is matched without regard to the case it arrived in.
    if rule.forbids_public && value.eq_ignore_ascii_case(PUBLIC_DATABASE_GROUP) {
        return refuse("The name cannot be PUBLIC.");
    }
    Ok(())
}

/// The window `GetClusterCredentials` documents for the lifetime of the
/// temporary database credentials it issues: the lifetime a request that names
/// none gets, and the inclusive bounds a request may ask for.
///
/// Source: `smithy_query(query="doc", shape="GetClusterCredentialsMessage",
/// member="DurationSeconds")` — "The number of seconds until the returned
/// temporary password expires. Constraint: minimum 900, maximum 3600. Default:
/// 900". A documented duration, so these are Amazon Redshift's numbers rather
/// than this emulator's choice.
pub const CREDENTIAL_DURATION_DEFAULT_SECONDS: i32 = 900;
/// The smallest `DurationSeconds` a request may name. See
/// [`CREDENTIAL_DURATION_DEFAULT_SECONDS`].
pub const CREDENTIAL_DURATION_MIN_SECONDS: i32 = 900;
/// The largest `DurationSeconds` a request may name. See
/// [`CREDENTIAL_DURATION_DEFAULT_SECONDS`].
pub const CREDENTIAL_DURATION_MAX_SECONDS: i32 = 3600;

/// How long the credentials a request asks for last, or why the request's
/// `DurationSeconds` is outside the documented window.
///
/// The refusal is Amazon Redshift's own recorded wording for this error class —
/// `Token duration must be between <min> and <max> seconds` — whose only
/// variable parts are the two documented bounds, both constants: the refusal
/// does not echo the value the request asked for, so a caller matching on the
/// sentence sees the same one whatever it sent.
pub fn credential_duration(requested: Option<i32>) -> Result<Duration, String> {
    let seconds = requested.unwrap_or(CREDENTIAL_DURATION_DEFAULT_SECONDS);
    if !(CREDENTIAL_DURATION_MIN_SECONDS..=CREDENTIAL_DURATION_MAX_SECONDS).contains(&seconds) {
        return Err(credential_duration_out_of_range_message());
    }
    Ok(Duration::from_secs(seconds.unsigned_abs().into()))
}

/// Why a `DurationSeconds` outside the documented window is refused. See
/// [`credential_duration`].
pub fn credential_duration_out_of_range_message() -> String {
    format!(
        "Token duration must be between {CREDENTIAL_DURATION_MIN_SECONDS} and \
         {CREDENTIAL_DURATION_MAX_SECONDS} seconds"
    )
}

/// The `DbUser` a `GetClusterCredentials` response reports for the user named in
/// its request.
///
/// Source: `smithy_query(query="doc", operation="GetClusterCredentials")` — "The
/// action returns the database user name prefixed with `IAM:` if `AutoCreate` is
/// `False` or `IAMA:` if `AutoCreate` is `True`" — so the prefix is decided by
/// the request's own `AutoCreate` and by nothing else.
pub fn credential_db_user(db_user: &str, auto_create: bool) -> String {
    let prefix = if auto_create { "IAMA:" } else { "IAM:" };
    format!("{prefix}{db_user}")
}

// The clusters a token request may scope its token to are documented on the
// `ClusterIds` member itself — "`ClusterIds` must contain at least 1 cluster
// identifier. `ClusterIds` can hold a maximum of 20 cluster identifiers",
// "cluster identifiers must be 1 to 63 characters in length", "must start with
// a letter" and "can't end with a hyphen or contain two consecutive hyphens"
// (`smithy_query(query="doc", shape="GetIdentityCenterAuthTokenRequest",
// member="ClusterIds")`) — and NO rule of this store enforces them,
// deliberately: the operation they belong to gates on the CALLER's credential
// type first and unconditionally, so no request this emulator serves is ever
// judged on its scope (see `crate::ops::get_identity_center_auth_token`). The
// minimum is in any case the member's own `@required` trait, which the boundary
// validator refuses a request without before any handler runs.
