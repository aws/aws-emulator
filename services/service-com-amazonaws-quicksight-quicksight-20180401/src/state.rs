// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-quicksight-quicksight-20180401 service.
//!
//! Besides the state struct this module holds the few helpers every QuickSight
//! handler needs — ARN construction, request-id minting, the `NextToken` codec and
//! the model's enum catalogs — so the per-operation files in `src/ops/` share one
//! implementation instead of each growing its own.
//!
//! The wording of the conflict and not-found refusals those handlers raise is one
//! concern of its own, so it lives in [`refusals`] and is re-exported from here.

mod authorization;
/// The impl block in this module adds the account's identity-store reading to the state
/// struct, so it exports no name of its own and needs no re-export.
mod identity_store;
mod refusals;
mod users;

pub use authorization::*;
pub use refusals::*;
pub use users::*;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime};

use ::aws_smithy_types::DateTime;

use crate::ops::types::{
    AccessDeniedException, AccountInfo, ColumnGroup, ColumnLevelPermissionRule,
    CustomInstructions,
    DataPrepConfiguration, DataSet, DataSetSummary, DataSetUsageConfiguration, DataSource,
    User,
    DataSourceErrorInfo, DataSourceParameters, DatasetParameter, FieldFolder, Folder, Group,
    IAMPolicyAssignment, InputColumn, LogicalTable, NamespaceInfoV2, OutputColumn,
    PerformanceConfiguration, PhysicalTable, RefreshFrequency, RefreshSchedule,
    ResourcePermission,
    RowLevelPermissionDataSet, RowLevelPermissionTagConfiguration, SemanticModelConfiguration,
    SslProperties, Tag, Theme, ThemeAlias, ThemeConfiguration, ThemeVersion, TopicDetails,
    VpcConnectionProperties,
};

/// Per-service state for com-amazonaws-quicksight-quicksight-20180401. A handler reaches it through [`service_state`].
///
/// Every map is keyed by the request's `AwsAccountId` first, so a resource created
/// under one account id is invisible under another — the account id is part of the
/// identity of every QuickSight resource, not a filter applied afterwards.
#[derive(Debug, Default)]
pub struct ComAmazonawsQuicksightQuicksight20180401State {
    /// Data sources by `(AwsAccountId, DataSourceId)`.
    pub data_sources: HashMap<(String, String), DataSourceRecord>,
    /// Data sets by `(AwsAccountId, DataSetId)`.
    pub data_sets: HashMap<(String, String), DataSetRecord>,
    /// Ingestions by `(AwsAccountId, DataSetId, IngestionId)`.
    pub ingestions: HashMap<(String, String, String), IngestionRecord>,
    /// Refresh schedules by `(AwsAccountId, DataSetId, ScheduleId)`.
    ///
    /// A refresh schedule is a child of one data set, and its `ScheduleId` is only
    /// unique inside that data set, so the data set it refreshes is part of its key.
    pub refresh_schedules: HashMap<(String, String, String), RefreshScheduleRecord>,
    /// Q topics by `(AwsAccountId, TopicId)`.
    pub topics: HashMap<(String, String), TopicRecord>,
    /// Themes by `(AwsAccountId, ThemeId)`.
    pub themes: HashMap<(String, String), ThemeRecord>,
    /// Folders by `(AwsAccountId, FolderId)`.
    ///
    /// Nesting is held on the record as the ancestors' ARNs rather than as a tree of
    /// its own, so a folder is reached by its id in one lookup however deep it sits.
    pub folders: HashMap<(String, String), FolderRecord>,
    /// IAM policy assignments by `(AwsAccountId, AssignmentName)`.
    ///
    /// The name alone identifies the assignment inside its account: the model states
    /// that assignment names are unique per Amazon Web Services account, so the
    /// namespace the assignment was created in is held on the record — it is the
    /// address a later call must present, not part of the name's identity.
    pub iam_policy_assignments: HashMap<(String, String), IamPolicyAssignmentRecord>,
    /// Namespaces by `(AwsAccountId, Namespace)`.
    ///
    /// The name alone identifies a namespace inside its account: the model documents
    /// that a namespace must be unique within the Amazon Web Services account.
    pub namespaces: HashMap<(String, String), NamespaceRecord>,
    /// Groups by `(AwsAccountId, Namespace, GroupName)`.
    ///
    /// A group belongs to one namespace and its name identifies it inside that
    /// namespace, so the namespace is part of its key: two namespaces of the same
    /// account can each hold a group of the same name, and neither is reached from
    /// the other.
    pub groups: HashMap<(String, String, String), GroupRecord>,
    /// Users by `(AwsAccountId, Namespace, UserName)`.
    ///
    /// A user belongs to one namespace and its name identifies it inside that
    /// namespace, so the namespace is part of its key — the same identity a group has:
    /// two namespaces of the same account can each hold a user of the same name, and
    /// neither is reached from the other.
    pub users: HashMap<(String, String, String), UserRecord>,
    /// The sign-up details `CreateAccountSubscription` stored, by `AwsAccountId`.
    ///
    /// An account with no entry here is still a QuickSight account — see
    /// [`ComAmazonawsQuicksightQuicksight20180401State::account_info`] — it just has
    /// not had its subscription customised by a sign-up call.
    pub account_subscriptions: HashMap<String, AccountSubscriptionRecord>,
}

impl ComAmazonawsQuicksightQuicksight20180401State {
    /// One account's data sources, ordered by id — the order `ListDataSources` pages
    /// through.
    pub fn data_sources_in(&self, aws_account_id: &str) -> Vec<&DataSourceRecord> {
        let mut records: Vec<&DataSourceRecord> = self
            .data_sources
            .iter()
            .filter(|((account, _), _)| account == aws_account_id)
            .map(|(_, record)| record)
            .collect();
        records.sort_by(|left, right| left.data_source_id.cmp(&right.data_source_id));
        records
    }

    /// One account's data sets, ordered by id — the order `ListDataSets` pages through.
    pub fn data_sets_in(&self, aws_account_id: &str) -> Vec<&DataSetRecord> {
        let mut records: Vec<&DataSetRecord> = self
            .data_sets
            .iter()
            .filter(|((account, _), _)| account == aws_account_id)
            .map(|(_, record)| record)
            .collect();
        records.sort_by(|left, right| left.data_set_id.cmp(&right.data_set_id));
        records
    }

    /// How many data sources `aws_account_id` holds — what
    /// [`MAX_DATA_SOURCES_PER_ACCOUNT`] is checked against.
    pub fn data_source_count(&self, aws_account_id: &str) -> usize {
        self.data_sources
            .keys()
            .filter(|(account, _)| account == aws_account_id)
            .count()
    }

    /// How many data sets `aws_account_id` holds — what [`MAX_DATA_SETS_PER_ACCOUNT`]
    /// is checked against.
    pub fn data_set_count(&self, aws_account_id: &str) -> usize {
        self.data_sets
            .keys()
            .filter(|(account, _)| account == aws_account_id)
            .count()
    }

    /// One account's topics, ordered by id — the order a topic read iterates in.
    pub fn topics_in(&self, aws_account_id: &str) -> Vec<&TopicRecord> {
        let mut records: Vec<&TopicRecord> = self
            .topics
            .iter()
            .filter(|((account, _), _)| account == aws_account_id)
            .map(|(_, record)| record)
            .collect();
        records.sort_by(|left, right| left.topic_id.cmp(&right.topic_id));
        records
    }

    /// How many topics `aws_account_id` holds — what [`MAX_TOPICS_PER_ACCOUNT`] is
    /// checked against.
    pub fn topic_count(&self, aws_account_id: &str) -> usize {
        self.topics
            .keys()
            .filter(|(account, _)| account == aws_account_id)
            .count()
    }

    /// One data set's refresh schedules, ordered by `ScheduleId` — the order a
    /// refresh-schedule read iterates in.
    pub fn refresh_schedules_in(
        &self,
        aws_account_id: &str,
        data_set_id: &str,
    ) -> Vec<&RefreshScheduleRecord> {
        let mut records: Vec<&RefreshScheduleRecord> = self
            .refresh_schedules
            .iter()
            .filter(|((account, data_set, _), _)| {
                account == aws_account_id && data_set == data_set_id
            })
            .map(|(_, record)| record)
            .collect();
        records.sort_by(|left, right| left.schedule_id.cmp(&right.schedule_id));
        records
    }

    /// The refresh schedule `data_set_id` holds under `schedule_id`, if any.
    pub fn refresh_schedule(
        &self,
        aws_account_id: &str,
        data_set_id: &str,
        schedule_id: &str,
    ) -> Option<&RefreshScheduleRecord> {
        self.refresh_schedules.get(&(
            aws_account_id.to_string(),
            data_set_id.to_string(),
            schedule_id.to_string(),
        ))
    }

    /// How many themes `aws_account_id` holds — what [`MAX_THEMES_PER_ACCOUNT`] is
    /// checked against.
    pub fn theme_count(&self, aws_account_id: &str) -> usize {
        self.themes
            .keys()
            .filter(|(account, _)| account == aws_account_id)
            .count()
    }

    /// The folder a call addressing `(aws_account_id, folder_id)` reaches, if the
    /// account holds one under that id.
    pub fn folder(&self, aws_account_id: &str, folder_id: &str) -> Option<&FolderRecord> {
        self.folders
            .get(&(aws_account_id.to_string(), folder_id.to_string()))
    }

    /// The folder in `aws_account_id` whose ARN is `arn`, if any — what resolving a
    /// submitted `ParentFolderArn` means.
    ///
    /// Resolved the same way as [`Self::data_source_by_arn`]: a QuickSight folder ARN
    /// ends in `folder/<FolderId>`, so the id is read off the ARN and the candidate's
    /// own ARN is then compared in full — an ARN that differs anywhere else (another
    /// account, another region, another service) matches nothing.
    pub fn folder_by_arn(&self, aws_account_id: &str, arn: &str) -> Option<&FolderRecord> {
        let folder_id = arn.rsplit_once("folder/").map(|(_, id)| id)?;
        self.folders
            .get(&(aws_account_id.to_string(), folder_id.to_string()))
            .filter(|record| record.arn == arn)
    }

    /// How many folders `aws_account_id` holds — what [`MAX_FOLDERS_PER_ACCOUNT`] is
    /// checked against.
    pub fn folder_count(&self, aws_account_id: &str) -> usize {
        self.folders
            .keys()
            .filter(|(account, _)| account == aws_account_id)
            .count()
    }

    /// The data set in `aws_account_id` whose ARN is `arn`, if any. A topic's
    /// `DatasetMetadata` names the data set it is built on by ARN, so resolving a
    /// submitted association means resolving that ARN inside the same account.
    ///
    /// Resolved the same way as [`Self::data_source_by_arn`]: a QuickSight data set ARN
    /// ends in `dataset/<DataSetId>`, so the id is read off the ARN and the candidate's
    /// own ARN is then compared in full — an ARN that differs anywhere else (another
    /// account, another region, another service) matches nothing.
    pub fn data_set_by_arn(&self, aws_account_id: &str, arn: &str) -> Option<&DataSetRecord> {
        let data_set_id = arn.rsplit_once("dataset/").map(|(_, id)| id)?;
        self.data_sets
            .get(&(aws_account_id.to_string(), data_set_id.to_string()))
            .filter(|record| record.arn == arn)
    }

    /// The data source in `aws_account_id` whose ARN is `arn`, if any. A
    /// `PhysicalTable` names its data source by ARN, so resolving a submitted table
    /// means resolving that ARN inside the same account.
    ///
    /// Resolved through the key rather than by scanning: a QuickSight data source ARN
    /// ends in `datasource/<DataSourceId>`, so the id is read off the ARN and the
    /// candidate's own ARN is then compared in full — an ARN that differs anywhere
    /// else (another account, another region, another service) matches nothing.
    pub fn data_source_by_arn(&self, aws_account_id: &str, arn: &str) -> Option<&DataSourceRecord> {
        let data_source_id = arn.rsplit_once("datasource/").map(|(_, id)| id)?;
        self.data_sources
            .get(&(aws_account_id.to_string(), data_source_id.to_string()))
            .filter(|record| record.arn == arn)
    }

    /// The IAM policy assignment `aws_account_id` holds under `assignment_name`,
    /// whichever namespace it was created in — what a create checks the account-wide
    /// uniqueness of the name against.
    pub fn iam_policy_assignment_named(
        &self,
        aws_account_id: &str,
        assignment_name: &str,
    ) -> Option<&IamPolicyAssignmentRecord> {
        self.iam_policy_assignments
            .get(&(aws_account_id.to_string(), assignment_name.to_string()))
    }

    /// The assignment a call addressing `(aws_account_id, namespace, assignment_name)`
    /// reaches: the account's assignment of that name, and only while it is the
    /// namespace that assignment was created in — another namespace holds no
    /// assignment of that name to read, update or delete.
    pub fn iam_policy_assignment(
        &self,
        aws_account_id: &str,
        namespace: &str,
        assignment_name: &str,
    ) -> Option<&IamPolicyAssignmentRecord> {
        self.iam_policy_assignment_named(aws_account_id, assignment_name)
            .filter(|record| record.namespace == namespace)
    }

    /// [`Self::iam_policy_assignment`], for a call that updates what it addresses.
    pub fn iam_policy_assignment_mut(
        &mut self,
        aws_account_id: &str,
        namespace: &str,
        assignment_name: &str,
    ) -> Option<&mut IamPolicyAssignmentRecord> {
        self.iam_policy_assignments
            .get_mut(&(aws_account_id.to_string(), assignment_name.to_string()))
            .filter(|record| record.namespace == namespace)
    }

    /// Removes the assignment a call addressing `(aws_account_id, namespace,
    /// assignment_name)` reaches, answering `None` — and removing nothing — when that
    /// namespace holds no assignment of that name.
    pub fn remove_iam_policy_assignment(
        &mut self,
        aws_account_id: &str,
        namespace: &str,
        assignment_name: &str,
    ) -> Option<IamPolicyAssignmentRecord> {
        self.iam_policy_assignment(aws_account_id, namespace, assignment_name)?;
        self.iam_policy_assignments
            .remove(&(aws_account_id.to_string(), assignment_name.to_string()))
    }

    /// How many IAM policy assignments `aws_account_id` holds, across its namespaces —
    /// what [`MAX_IAM_POLICY_ASSIGNMENTS_PER_ACCOUNT`] is checked against.
    pub fn iam_policy_assignment_count(&self, aws_account_id: &str) -> usize {
        self.iam_policy_assignments
            .keys()
            .filter(|(account, _)| account == aws_account_id)
            .count()
    }
}

impl ComAmazonawsQuicksightQuicksight20180401State {
    /// The subscription QuickSight reports for `aws_account_id`.
    ///
    /// QuickSight is enabled in every account this emulator serves — no operation in
    /// this crate requires a prior sign-up, and a data source or data set can be
    /// created under any `AwsAccountId` — so every account has a subscription in the
    /// terminal [`ACCOUNT_SUBSCRIPTION_STATUS_CREATED`] status. The details reported
    /// are the ones `CreateAccountSubscription` stored for that account when it signed
    /// up, and the service defaults for an account that never did.
    pub fn account_info(&self, aws_account_id: &str) -> AccountInfo {
        match self.account_subscriptions.get(aws_account_id) {
            Some(record) => record.to_account_info(),
            None => default_account_info(aws_account_id),
        }
    }
}

/// A stored account subscription: what one account's `CreateAccountSubscription`
/// sign-up chose.
///
/// The author contact details the sign-up may carry (`FirstName`, `LastName`,
/// `EmailAddress`, `ContactNumber`) are deliberately absent, for the same reason
/// `DataSourceRecord` drops `Credentials`: QuickSight returns them on no read, so
/// keeping them would only hold personal data nothing can reach.
#[derive(Debug, Clone)]
pub struct AccountSubscriptionRecord {
    pub account_name: String,
    pub edition: String,
    pub notification_email: String,
    /// The `AuthenticationMethod` the sign-up chose, which is what `AccountInfo`
    /// reports as its `AuthenticationType`.
    pub authentication_method: String,
    pub iam_identity_center_instance_arn: Option<String>,
    pub active_directory_name: Option<String>,
    pub realm: Option<String>,
    pub directory_id: Option<String>,
    pub admin_groups: Vec<String>,
    pub author_groups: Vec<String>,
    pub reader_groups: Vec<String>,
    pub created_time: DateTime,
}

impl AccountSubscriptionRecord {
    /// The modelled `AccountInfo` this record describes.
    pub fn to_account_info(&self) -> AccountInfo {
        AccountInfo {
            account_name: Some(self.account_name.clone()),
            edition: Some(self.edition.clone()),
            notification_email: Some(self.notification_email.clone()),
            authentication_type: Some(self.authentication_method.clone()),
            account_subscription_status: Some(
                ACCOUNT_SUBSCRIPTION_STATUS_CREATED.to_string(),
            ),
            iam_identity_center_instance_arn: self.iam_identity_center_instance_arn.clone(),
        }
    }
}

/// The `AccountInfo` of an account that never called `CreateAccountSubscription`.
///
/// `NotificationEmail` and `IAMIdentityCenterInstanceArn` are left unset rather than
/// invented: only a sign-up (or an account-settings update) supplies an address, and
/// this emulator has no IAM Identity Center instance.
fn default_account_info(aws_account_id: &str) -> AccountInfo {
    AccountInfo {
        account_name: Some(default_account_name(aws_account_id)),
        edition: Some(DEFAULT_EDITION.to_string()),
        notification_email: None,
        authentication_type: Some(DEFAULT_AUTHENTICATION_METHOD.to_string()),
        account_subscription_status: Some(ACCOUNT_SUBSCRIPTION_STATUS_CREATED.to_string()),
        iam_identity_center_instance_arn: None,
    }
}

/// The QuickSight account name of an account that never named one.
///
/// A QuickSight `AccountName` is unique across all of Amazon Web Services, so the
/// default is derived from the account id, which is what makes it unique. It matches
/// the model's `AccountName` constraints (len 1..62, pattern
/// `^(?!D-|d-)([\da-zA-Z]+)([-]*[\da-zA-Z])*$`), so a name this emulator reports is
/// one `CreateAccountSubscription` would have accepted.
pub fn default_account_name(aws_account_id: &str) -> String {
    format!("quicksight-{aws_account_id}")
}

/// The `AccountSubscriptionStatus` of a QuickSight account that exists and is usable —
/// the status AWS reports for a created account. The member is a free-form string in
/// the model (`com.amazonaws.quicksight#AccountInfo$AccountSubscriptionStatus`), which
/// declares no enum.
pub const ACCOUNT_SUBSCRIPTION_STATUS_CREATED: &str = "ACCOUNT_CREATED";

/// The edition reported for an account that never chose one, and stored for a sign-up
/// that names no `Edition`.
///
/// `ENTERPRISE` (model enum `com.amazonaws.quicksight#Edition`) is the edition whose
/// feature set this emulator serves: namespaces, groups, folders and themes are all
/// Enterprise-edition features, and operations over them are part of this service.
pub const DEFAULT_EDITION: &str = "ENTERPRISE";

/// The authentication type reported for an account that never chose one.
///
/// A QuickSight account subscribed with no identity store of its own — its callers are
/// the Amazon Web Services identities this emulator authenticates, and nothing else — is
/// what an account that never signed up through this service reports, so the identities
/// it manages are those alone until a sign-up chooses otherwise (model enum
/// `com.amazonaws.quicksight#AuthenticationMethodOption`).
pub const DEFAULT_AUTHENTICATION_METHOD: &str = AUTHENTICATION_METHOD_IAM_ONLY;

/// The `AuthenticationMethodOption` that authenticates callers through Amazon Web
/// Services identities alone, so the account holds no QuickSight identity store.
pub const AUTHENTICATION_METHOD_IAM_ONLY: &str = "IAM_ONLY";

/// The `AuthenticationMethodOption` whose account manages QuickSight identities of its
/// own beside the Amazon Web Services ones.
pub const AUTHENTICATION_METHOD_IAM_AND_QUICKSIGHT: &str = "IAM_AND_QUICKSIGHT";

/// The `AuthenticationMethodOption` values whose accounts are authenticated through
/// IAM, which is what `SignupResponse$IAMUser` reports.
pub const IAM_AUTHENTICATION_METHODS: &[&str] = &["IAM_AND_QUICKSIGHT", "IAM_ONLY"];

/// The `AuthenticationMethodOption` that authenticates against an Active Directory,
/// and so the one whose sign-up must name that directory.
pub const AUTHENTICATION_METHOD_ACTIVE_DIRECTORY: &str = "ACTIVE_DIRECTORY";

/// The `AuthenticationMethodOption` that authenticates against IAM Identity Center.
pub const AUTHENTICATION_METHOD_IAM_IDENTITY_CENTER: &str = "IAM_IDENTITY_CENTER";

/// The `Edition` whose sign-up also subscribes to QuickSight Q, and so the one whose
/// `CreateAccountSubscription` requires the author's contact details.
pub const EDITION_ENTERPRISE_AND_Q: &str = "ENTERPRISE_AND_Q";

/// The TOTAL number of account subscriptions this emulator stores. The key is the
/// request's `AwsAccountId`, so — as for data sources and data sets — the bound that
/// holds for the process is a total one.
pub const MAX_ACCOUNT_SUBSCRIPTIONS: usize = 10_000;

impl ComAmazonawsQuicksightQuicksight20180401State {
    /// The namespace `aws_account_id` holds under `name`, or `None` when that account
    /// never created one — the stored record a read answers from.
    ///
    /// The record is returned whatever stage it has reached, a namespace whose
    /// deletion already fell due included; a caller serving a request wants
    /// [`Self::namespace_at`], which is that record only while the namespace is still
    /// there.
    pub fn namespace(&self, aws_account_id: &str, name: &str) -> Option<&NamespaceRecord> {
        self.namespaces
            .get(&(aws_account_id.to_string(), name.to_string()))
    }

    /// The namespace a call addressing `(aws_account_id, name)` at `now` reaches: the
    /// stored record, and only until the deletion the clock owes it falls due — from
    /// that instant the namespace is gone whether or not anything has purged the map.
    pub fn namespace_at(
        &self,
        aws_account_id: &str,
        name: &str,
        now: SystemTime,
    ) -> Option<&NamespaceRecord> {
        self.namespace(aws_account_id, name)
            .filter(|record| !record.is_deleted_at(now))
    }

    /// Materialises the namespace deletions the clock has already made due: each one
    /// was owed at the same instant whether or not anybody called, so a handler that
    /// takes the store may drop the records they left behind — and, with each namespace,
    /// whatever groups and users it still held.
    pub fn settle_namespace_deletions(&mut self, now: SystemTime) {
        let gone: Vec<(String, String)> = self
            .namespaces
            .iter()
            .filter(|(_, record)| record.is_deleted_at(now))
            .map(|(key, _)| key.clone())
            .collect();
        for (aws_account_id, namespace) in gone {
            self.namespaces
                .remove(&(aws_account_id.clone(), namespace.clone()));
            self.remove_groups_in(&aws_account_id, &namespace);
            self.remove_users_in(&aws_account_id, &namespace);
        }
    }

    /// The namespace a read of `(aws_account_id, name)` reports.
    ///
    /// Owned rather than borrowed because the namespace every account has — the one
    /// the model's `Namespace` members call `default` — was minted by no create, so it
    /// is described from the account and the request's region rather than from the map.
    pub fn namespace_info(
        &self,
        aws_account_id: &str,
        name: &str,
        region: &str,
        now: SystemTime,
    ) -> Option<NamespaceInfoV2> {
        match self.namespace_at(aws_account_id, name, now) {
            Some(record) => Some(record.to_namespace_info(now)),
            None if name == DEFAULT_NAMESPACE => {
                Some(default_namespace(region, aws_account_id).to_namespace_info(now))
            }
            None => None,
        }
    }

    /// How many namespaces `aws_account_id` holds as of `now` — what
    /// [`MAX_NAMESPACES_PER_ACCOUNT`] is checked against. A namespace whose deletion
    /// fell due is gone, so it holds none of the account's quota.
    pub fn namespace_count(&self, aws_account_id: &str, now: SystemTime) -> usize {
        self.namespaces
            .iter()
            .filter(|((account, _), record)| {
                account == aws_account_id && !record.is_deleted_at(now)
            })
            .count()
    }

    /// Whether `(aws_account_id, name)` addresses a namespace that exists as of `now` —
    /// the stored one a create put there, or the namespace every account already has.
    pub fn namespace_exists(&self, aws_account_id: &str, name: &str, now: SystemTime) -> bool {
        self.namespace_at(aws_account_id, name, now).is_some() || name == DEFAULT_NAMESPACE
    }

    /// The `NamespaceStatus` `(aws_account_id, name)` reports as of `now`, or `None`
    /// when the account holds no such namespace.
    ///
    /// The namespace every account already has was minted by no create, so it is always
    /// in the terminal status; a stored one reports whichever stage the clock has
    /// brought it to.
    pub fn namespace_status(
        &self,
        aws_account_id: &str,
        name: &str,
        now: SystemTime,
    ) -> Option<&'static str> {
        match self.namespace_at(aws_account_id, name, now) {
            Some(record) => Some(record.status_at(now)),
            None if name == DEFAULT_NAMESPACE => Some(NAMESPACE_STATUS_CREATED),
            None => None,
        }
    }

    /// Removes the groups `(aws_account_id, namespace)` holds, answering how many went:
    /// deleting a namespace deletes the users and groups associated with it, so they go
    /// with the namespace rather than outliving it.
    pub fn remove_groups_in(&mut self, aws_account_id: &str, namespace: &str) -> usize {
        let before = self.groups.len();
        self.groups.retain(|(account, group_namespace, _), _| {
            account != aws_account_id || group_namespace != namespace
        });
        before - self.groups.len()
    }

    /// The group `(aws_account_id, namespace)` holds under `group_name`, or `None`
    /// when no create stored one — the record every group read answers from.
    pub fn group(
        &self,
        aws_account_id: &str,
        namespace: &str,
        group_name: &str,
    ) -> Option<&GroupRecord> {
        self.groups.get(&(
            aws_account_id.to_string(),
            namespace.to_string(),
            group_name.to_string(),
        ))
    }

    /// How many groups `(aws_account_id, namespace)` holds — what
    /// [`MAX_GROUPS_PER_NAMESPACE`] is checked against.
    pub fn group_count(&self, aws_account_id: &str, namespace: &str) -> usize {
        self.groups
            .keys()
            .filter(|(account, group_namespace, _)| {
                account == aws_account_id && group_namespace == namespace
            })
            .count()
    }
}

/// A stored group: what `CreateGroup` persisted, which is what every later group read
/// reports.
///
/// The record carries no timestamps: the modelled `Group` structure declares none, so
/// nothing is stored that no read can report.
#[derive(Debug, Clone)]
pub struct GroupRecord {
    pub arn: String,
    pub group_name: String,
    /// The description the create submitted, if any. `GroupDescription` is optional,
    /// so a group created without one reports none rather than an empty string.
    pub description: Option<String>,
    /// The service's own handle on this group, minted when it was created and stable
    /// for as long as the group exists.
    pub principal_id: String,
}

impl GroupRecord {
    /// This group as the modelled `Group` structure every group read reports.
    pub fn to_group(&self) -> Group {
        Group {
            arn: Some(self.arn.clone()),
            group_name: Some(self.group_name.clone()),
            description: self.description.clone(),
            principal_id: Some(self.principal_id.clone()),
        }
    }
}

/// How many groups one namespace may hold before `CreateGroup` refuses with the
/// modelled `LimitExceededException`: the ceiling `CreateGroup`'s own documentation
/// states ("You can create up to 10,000 groups in a namespace").
pub const MAX_GROUPS_PER_NAMESPACE: usize = 10_000;

/// The TOTAL number of groups this emulator holds, across every account and
/// namespace.
///
/// This is the bound that holds for the process, for the same reason as
/// [`MAX_DATA_SOURCES`]: every part of the store key comes from the request — the
/// account id and the namespace are both `@httpLabel`s the caller chooses — so the
/// per-namespace ceiling alone would be reset by varying either.
pub const MAX_GROUPS: usize = 100_000;

/// The `ExceptionResourceType` a group's refusals report (model enum
/// `com.amazonaws.quicksight#ExceptionResourceType`).
pub const EXCEPTION_RESOURCE_TYPE_GROUP: &str = "GROUP";

/// A stored namespace: what `CreateNamespace` persisted, which is what every later
/// namespace read reports.
///
/// The record carries no timestamps: the modelled `NamespaceInfoV2` structure declares
/// none, so nothing is stored that no read can report.
#[derive(Debug, Clone)]
pub struct NamespaceRecord {
    pub arn: String,
    pub name: String,
    /// The Region whose free SPICE capacity the namespace uses: the Region the
    /// `CreateNamespace` call ran in.
    pub capacity_region: String,
    pub identity_store: String,
    /// When the clock owes this namespace its move out of `CREATING`. Stored rather
    /// than waited on: a reader materialises the transition the clock has already made
    /// due.
    pub creation_settles_at: SystemTime,
    /// When the clock owes this namespace its removal, once a delete has asked for it;
    /// `None` for a namespace no delete has addressed. Deleting a namespace is an
    /// asynchronous process, so the record stays — reporting the non-terminal
    /// `DELETING` status — until that instant, and is gone from then on.
    pub deletion_settles_at: Option<SystemTime>,
    pub tags: Vec<Tag>,
}

impl NamespaceRecord {
    /// This namespace's `NamespaceStatus` as of `now`: the non-terminal creation status
    /// until the transition the clock owes falls due, `CREATED` from then on, and the
    /// non-terminal deletion status for as long as a delete it has been asked for is
    /// still in flight.
    pub fn status_at(&self, now: SystemTime) -> &'static str {
        if self.deletion_settles_at.is_some() {
            NAMESPACE_STATUS_DELETING
        } else if now >= self.creation_settles_at {
            NAMESPACE_STATUS_CREATED
        } else {
            NAMESPACE_STATUS_CREATING
        }
    }

    /// Whether the deletion this namespace was asked for has fallen due as of `now` —
    /// the instant from which the namespace is gone, so nothing reaches it any more.
    pub fn is_deleted_at(&self, now: SystemTime) -> bool {
        self.deletion_settles_at.is_some_and(|due| now >= due)
    }

    /// The modelled `NamespaceInfoV2` structure this record describes, as of `now`.
    pub fn to_namespace_info(&self, now: SystemTime) -> NamespaceInfoV2 {
        NamespaceInfoV2 {
            name: Some(self.name.clone()),
            arn: Some(self.arn.clone()),
            capacity_region: Some(self.capacity_region.clone()),
            creation_status: Some(self.status_at(now).to_string()),
            identity_store: Some(self.identity_store.clone()),
            // A namespace whose creation succeeded carries no error, and nothing in
            // this emulator fails a namespace creation.
            namespace_error: None,
            // This emulator has no IAM Identity Center instance, so a namespace is
            // backed by no Identity Center application either.
            iam_identity_center_application_arn: None,
            iam_identity_center_instance_arn: None,
        }
    }
}

/// The namespace every QuickSight account already has, which no create minted.
///
/// It is ready from the instant the account exists — this emulator's accounts are all
/// subscribed, as [`ComAmazonawsQuicksightQuicksight20180401State::account_info`]
/// states — so its creation transition is already due and it carries no tags.
fn default_namespace(region: &str, aws_account_id: &str) -> NamespaceRecord {
    NamespaceRecord {
        arn: namespace_arn(region, aws_account_id, DEFAULT_NAMESPACE),
        name: DEFAULT_NAMESPACE.to_string(),
        capacity_region: region.to_string(),
        identity_store: IDENTITY_STORE_QUICKSIGHT.to_string(),
        creation_settles_at: SystemTime::UNIX_EPOCH,
        // The namespace every account has cannot be deleted, so no delete is ever in
        // flight for it.
        deletion_settles_at: None,
        tags: Vec::new(),
    }
}

/// The name of the namespace every account has: the one the model's `Namespace`
/// members document a caller that uses no custom namespace to address.
pub const DEFAULT_NAMESPACE: &str = "default";

/// `NamespaceStatus` of a namespace that is ready to use (model enum
/// `com.amazonaws.quicksight#NamespaceStatus`).
pub const NAMESPACE_STATUS_CREATED: &str = "CREATED";

/// `NamespaceStatus` of a namespace whose creation is still in flight (same model
/// enum). A namespace is created in this non-terminal status — the model calls the
/// creation an asynchronous process — and leaves it once [`NAMESPACE_CREATION_SPAN`]
/// has elapsed.
pub const NAMESPACE_STATUS_CREATING: &str = "CREATING";

/// `NamespaceStatus` of a namespace whose deletion is still in flight (same model
/// enum). `DeleteNamespace` is documented as an asynchronous process, so a namespace it
/// accepted reports this non-terminal status until [`NAMESPACE_DELETION_SPAN`] has
/// elapsed, and is gone from then on.
pub const NAMESPACE_STATUS_DELETING: &str = "DELETING";

/// The only `IdentityStore` the model admits, and so the store every namespace uses.
pub const IDENTITY_STORE_QUICKSIGHT: &str = "QUICKSIGHT";

/// How long a new namespace stays in `CREATING` before the clock owes it its terminal
/// status. Chosen (QuickSight publishes no duration): long enough that a caller
/// reading the namespace straight after the create still observes the non-terminal
/// status the create reported, short enough to be over by the time anything polls.
pub const NAMESPACE_CREATION_SPAN: Duration = Duration::from_secs(3);

/// How long a namespace stays in `DELETING` before the clock owes its removal. Chosen
/// on the same ground as [`NAMESPACE_CREATION_SPAN`] (QuickSight publishes no
/// duration): long enough that a caller reading the namespace straight after the delete
/// still observes the non-terminal status, short enough to be over by the time anything
/// polls.
pub const NAMESPACE_DELETION_SPAN: Duration = Duration::from_secs(3);

/// How many namespaces one account may hold before `CreateNamespace` refuses with the
/// modelled `LimitExceededException`. The model documents the limit: by default there
/// is a limit of 100 namespaces per Amazon Web Services account.
pub const MAX_NAMESPACES_PER_ACCOUNT: usize = 100;

/// The TOTAL number of namespaces this emulator holds, across every account — the
/// process bound, for the same reason as [`MAX_DATA_SOURCES`].
pub const MAX_NAMESPACES: usize = 10_000;

/// `ExceptionResourceType` of a namespace — what a refusal about one reports (model
/// enum `com.amazonaws.quicksight#ExceptionResourceType`).
pub const EXCEPTION_RESOURCE_TYPE_NAMESPACE: &str = "NAMESPACE";

/// A stored data source.
///
/// `Credentials` are deliberately absent: the member is `@sensitive`, QuickSight
/// never returns it on any read, and nothing this emulator does needs it — so it is
/// dropped when the request is served rather than kept in memory.
#[derive(Debug, Clone)]
pub struct DataSourceRecord {
    pub arn: String,
    pub data_source_id: String,
    pub name: String,
    pub r#type: String,
    /// The `ResourceStatus` this data source carries until the transition the clock
    /// owes it falls due: the non-terminal status the create or the update reported.
    pub status: String,
    /// The `ResourceStatus` it reaches then — the terminal outcome of the create or
    /// the update that last addressed it.
    pub settled_status: String,
    /// When the clock owes this data source its move from [`Self::status`] to
    /// [`Self::settled_status`]. Stored rather than waited on: a reader materialises
    /// the transition the clock has already made due.
    pub status_settles_at: SystemTime,
    pub created_time: DateTime,
    pub last_updated_time: DateTime,
    pub data_source_parameters: Option<DataSourceParameters>,
    pub vpc_connection_properties: Option<VpcConnectionProperties>,
    pub ssl_properties: Option<SslProperties>,
    pub permissions: Vec<ResourcePermission>,
    pub tags: Vec<Tag>,
    pub error_info: Option<DataSourceErrorInfo>,
}

impl DataSourceRecord {
    /// This data source's `ResourceStatus` as of `now`: the non-terminal status the
    /// create or update reported until the transition the clock owes it falls due, the
    /// terminal one from then on.
    pub fn status_at(&self, now: SystemTime) -> &str {
        if now >= self.status_settles_at {
            &self.settled_status
        } else {
            &self.status
        }
    }

    /// The modelled `DataSource` structure this record describes, as of `now` — the
    /// instant that decides which side of the lifecycle transition its status is on.
    pub fn to_data_source(&self, now: SystemTime) -> DataSource {
        DataSource {
            arn: Some(self.arn.clone()),
            data_source_id: Some(self.data_source_id.clone()),
            name: Some(self.name.clone()),
            r#type: Some(self.r#type.clone()),
            status: Some(self.status_at(now).to_string()),
            created_time: Some(self.created_time),
            last_updated_time: Some(self.last_updated_time),
            data_source_parameters: self.data_source_parameters.clone(),
            // The allow list of alternate parameter sets a shared credential may be
            // used with. It reaches the service only inside the `@sensitive`
            // `Credentials` structure, which neither the create nor the update keeps,
            // so no stored data source carries one — and the model documents a null
            // list as allowing the parameters the credentials were used with.
            alternate_data_source_parameters: None,
            vpc_connection_properties: self.vpc_connection_properties.clone(),
            ssl_properties: self.ssl_properties.clone(),
            error_info: self.error_info.clone(),
            // Secret-based credentials are a separate feature of CreateDataSource's
            // `Credentials` member (`SecretArn`); nothing stores one yet.
            secret_arn: None,
            credential_status: None,
            last_credential_verified_at: None,
        }
    }
}

/// A stored data set.
#[derive(Debug, Clone)]
pub struct DataSetRecord {
    pub arn: String,
    pub data_set_id: String,
    pub name: String,
    pub created_time: DateTime,
    pub last_updated_time: DateTime,
    pub import_mode: String,
    pub physical_table_map: HashMap<String, PhysicalTable>,
    pub logical_table_map: Option<HashMap<String, LogicalTable>>,
    pub output_columns: Vec<OutputColumn>,
    pub column_groups: Option<Vec<ColumnGroup>>,
    pub field_folders: Option<HashMap<String, FieldFolder>>,
    pub row_level_permission_data_set: Option<RowLevelPermissionDataSet>,
    pub row_level_permission_tag_configuration: Option<RowLevelPermissionTagConfiguration>,
    pub column_level_permission_rules: Option<Vec<ColumnLevelPermissionRule>>,
    pub data_set_usage_configuration: Option<DataSetUsageConfiguration>,
    pub dataset_parameters: Option<Vec<DatasetParameter>>,
    pub performance_configuration: Option<PerformanceConfiguration>,
    pub use_as: Option<String>,
    pub data_prep_configuration: Option<DataPrepConfiguration>,
    pub semantic_model_configuration: Option<SemanticModelConfiguration>,
    pub permissions: Vec<ResourcePermission>,
    pub tags: Vec<Tag>,
    /// SPICE bytes this data set holds. Nothing has been ingested into an emulated
    /// data set, so it stays 0 until an ingestion loads rows.
    pub consumed_spice_capacity_in_bytes: i64,
}

impl DataSetRecord {
    /// The modelled `DataSet` structure this record describes.
    pub fn to_data_set(&self) -> DataSet {
        DataSet {
            arn: Some(self.arn.clone()),
            data_set_id: Some(self.data_set_id.clone()),
            name: Some(self.name.clone()),
            created_time: Some(self.created_time),
            last_updated_time: Some(self.last_updated_time),
            physical_table_map: Some(self.physical_table_map.clone()),
            logical_table_map: self.logical_table_map.clone(),
            output_columns: Some(self.output_columns.clone()),
            import_mode: Some(self.import_mode.clone()),
            consumed_spice_capacity_in_bytes: self.consumed_spice_capacity_in_bytes,
            column_groups: self.column_groups.clone(),
            field_folders: self.field_folders.clone(),
            row_level_permission_data_set: self.row_level_permission_data_set.clone(),
            row_level_permission_tag_configuration: self
                .row_level_permission_tag_configuration
                .clone(),
            column_level_permission_rules: self.column_level_permission_rules.clone(),
            data_set_usage_configuration: self.data_set_usage_configuration.clone(),
            dataset_parameters: self.dataset_parameters.clone(),
            performance_configuration: self.performance_configuration.clone(),
            use_as: self.use_as.clone(),
            data_prep_configuration: self.data_prep_configuration.clone(),
            semantic_model_configuration: self.semantic_model_configuration.clone(),
        }
    }

    /// The modelled `DataSetSummary` for this record, as `ListDataSets` reports it.
    pub fn to_summary(&self) -> DataSetSummary {
        DataSetSummary {
            arn: Some(self.arn.clone()),
            data_set_id: Some(self.data_set_id.clone()),
            name: Some(self.name.clone()),
            created_time: Some(self.created_time),
            last_updated_time: Some(self.last_updated_time),
            import_mode: Some(self.import_mode.clone()),
            row_level_permission_data_set: self.row_level_permission_data_set.clone(),
            row_level_permission_data_set_map: None,
            row_level_permission_tag_configuration_applied: self
                .row_level_permission_tag_configuration
                .is_some(),
            column_level_permission_rules_applied: self
                .column_level_permission_rules
                .as_ref()
                .is_some_and(|rules| !rules.is_empty()),
            use_as: self.use_as.clone(),
        }
    }
}

/// A stored ingestion: the SPICE refresh `CreateDataSet` starts for an
/// `ImportMode=SPICE` data set, so the `IngestionId` it returns names a real object.
#[derive(Debug, Clone)]
pub struct IngestionRecord {
    pub arn: String,
    pub ingestion_id: String,
    /// The status the ingestion was created in. QuickSight starts a SPICE refresh in
    /// a non-terminal status, so [`IngestionRecord::status_at`] is what a reader
    /// reports.
    pub ingestion_status: String,
    pub created_time: DateTime,
    pub request_source: String,
    /// When the clock owes this ingestion its move to `COMPLETED`. Stored rather than
    /// waited on: a reader materialises the transition the clock has already made due.
    pub completes_at: SystemTime,
}

/// How long an emulated SPICE ingestion takes to complete. Chosen (QuickSight
/// publishes no duration): long enough that a caller acting straight after
/// `CreateDataSet` still observes the non-terminal status, short enough to be over by
/// the time anything polls.
pub const INGESTION_DURATION: Duration = Duration::from_millis(500);

impl IngestionRecord {
    /// This ingestion's status as of `now`: the stored non-terminal status until the
    /// completion the clock owes falls due, `COMPLETED` from then on.
    pub fn status_at(&self, now: SystemTime) -> &str {
        if now >= self.completes_at {
            INGESTION_STATUS_COMPLETED
        } else {
            &self.ingestion_status
        }
    }
}

/// `IngestionStatus` a freshly created SPICE ingestion carries (model enum
/// `com.amazonaws.quicksight#IngestionStatus`).
pub const INGESTION_STATUS_INITIALIZED: &str = "INITIALIZED";
/// The terminal `IngestionStatus` of an ingestion that loaded its rows.
pub const INGESTION_STATUS_COMPLETED: &str = "COMPLETED";
/// `IngestionRequestSource` for an ingestion a caller's own request started, as
/// opposed to one a refresh schedule started.
pub const INGESTION_REQUEST_SOURCE_MANUAL: &str = "MANUAL";

/// A stored refresh schedule: the `RefreshSchedule` a create persisted under one data
/// set, which is what every later refresh-schedule read reports.
///
/// The submitted frequency is held whole rather than split into its members: a read
/// returns that same structure, so storing it is what makes the read report the
/// schedule the create was given.
#[derive(Debug, Clone)]
pub struct RefreshScheduleRecord {
    pub arn: String,
    pub schedule_id: String,
    pub schedule_frequency: RefreshFrequency,
    pub start_after_date_time: Option<DateTime>,
    pub refresh_type: String,
}

impl RefreshScheduleRecord {
    /// The modelled `RefreshSchedule` structure this record describes. The `Arn` is
    /// the one the service minted for the schedule, not whatever the create's input
    /// carried there.
    pub fn to_refresh_schedule(&self) -> RefreshSchedule {
        RefreshSchedule {
            schedule_id: self.schedule_id.clone(),
            schedule_frequency: self.schedule_frequency.clone(),
            start_after_date_time: self.start_after_date_time,
            refresh_type: self.refresh_type.clone(),
            arn: Some(self.arn.clone()),
        }
    }
}

/// How many refresh schedules one data set may hold: the model documents that up to 5
/// different schedules can be created for a single dataset.
pub const MAX_REFRESH_SCHEDULES_PER_DATA_SET: usize = 5;

/// The TOTAL number of refresh schedules this emulator holds, across every data set —
/// the process bound, for the same reason as [`MAX_DATA_SOURCES`].
pub const MAX_REFRESH_SCHEDULES: usize = 10_000;

/// The `RefreshInterval` members a data set may only hold ONE schedule of: the model
/// documents, for each of these intervals, that it can only be used for one schedule
/// per dataset. The longer intervals carry no such restriction.
pub const SINGLE_SCHEDULE_REFRESH_INTERVALS: &[&str] = &["MINUTE15", "MINUTE30", "HOURLY"];

/// The `RefreshInterval` members only an incremental refresh may use: the model
/// documents, for each of these intervals, that it is only supported for incremental
/// refreshes.
pub const INCREMENTAL_ONLY_REFRESH_INTERVALS: &[&str] = &["MINUTE15", "MINUTE30"];

/// The `RefreshInterval` members a schedule must name a `RefreshOnDay` for: the model
/// states that value is required for the weekly and monthly refresh intervals, which are
/// the two that refresh on ONE day of their period.
pub const REFRESH_ON_DAY_REQUIRED_INTERVALS: &[&str] = &["WEEKLY", "MONTHLY"];

/// The `IngestionType` member naming a partial refresh of a data set's rows (model
/// enum `com.amazonaws.quicksight#IngestionType`).
pub const INGESTION_TYPE_INCREMENTAL_REFRESH: &str = "INCREMENTAL_REFRESH";

/// A stored Q topic: what `CreateTopic` persisted, which is what every later topic
/// read reports.
///
/// The submitted [`TopicDetails`] is held whole rather than split into its members:
/// `DescribeTopic` returns that same structure, so storing it is what makes the read
/// report the definition the create was given.
#[derive(Debug, Clone)]
pub struct TopicRecord {
    pub arn: String,
    pub topic_id: String,
    pub topic: TopicDetails,
    pub custom_instructions: Option<CustomInstructions>,
    /// The folder the topic was asked to reside in, if any — stored so a folder read
    /// reports the membership once folders are implemented.
    pub folder_arns: Vec<String>,
    pub tags: Vec<Tag>,
    pub created_time: DateTime,
    /// The refresh that indexes the topic's associated data sets, minted by the create
    /// that associated them; `None` for a topic associated with no data set, which has
    /// nothing to index.
    pub refresh: Option<TopicRefreshRecord>,
}

/// A stored topic refresh: the one `CreateTopic` starts, so the `RefreshArn` it
/// reports names a real object.
#[derive(Debug, Clone)]
pub struct TopicRefreshRecord {
    pub arn: String,
    pub refresh_id: String,
    pub created_time: DateTime,
}

/// How many topics one account may hold before `CreateTopic` refuses with the
/// modelled `LimitExceededException`. Same standing as
/// [`MAX_DATA_SETS_PER_ACCOUNT`]: a fidelity limit, not the process bound.
pub const MAX_TOPICS_PER_ACCOUNT: usize = 1_000;

/// The TOTAL number of topics this emulator holds, across every account — the
/// process bound, for the same reason as [`MAX_DATA_SOURCES`].
pub const MAX_TOPICS: usize = 10_000;

/// `ResourceStatus` of a resource whose creation succeeded (model enum
/// `com.amazonaws.quicksight#ResourceStatus`).
pub const RESOURCE_STATUS_CREATION_SUCCESSFUL: &str = "CREATION_SUCCESSFUL";

/// `ResourceStatus` of a resource whose creation is still in flight (same model
/// enum). A theme version is created in this non-terminal status and leaves it once
/// [`THEME_CREATION_SPAN`] has elapsed.
pub const RESOURCE_STATUS_CREATION_IN_PROGRESS: &str = "CREATION_IN_PROGRESS";

/// `ResourceStatus` of a resource whose last update succeeded (same model enum). A
/// data source reaches it once the asynchronous update the service accepted falls due.
pub const RESOURCE_STATUS_UPDATE_SUCCESSFUL: &str = "UPDATE_SUCCESSFUL";

/// `ResourceStatus` of a resource whose last update is still in flight (same model
/// enum) — what `UpdateDataSource` reports for the update it accepted.
pub const RESOURCE_STATUS_UPDATE_IN_PROGRESS: &str = "UPDATE_IN_PROGRESS";

/// How long a data source stays in the non-terminal status its create or its update
/// reported before the clock owes it the terminal one. Chosen (QuickSight publishes no
/// duration): long enough that a caller reading the data source straight after the
/// write still observes the non-terminal status that write reported, short enough to be
/// over by the time anything polls.
pub const DATA_SOURCE_LIFECYCLE_SPAN: Duration = Duration::from_secs(3);

/// `ThemeType` of a theme a caller created, as opposed to one of the starting themes
/// the service itself defines (`QUICKSIGHT`). Model enum
/// `com.amazonaws.quicksight#ThemeType`.
pub const THEME_TYPE_CUSTOM: &str = "CUSTOM";

/// The `VersionNumber` of the version a create mints: a theme's versions are numbered
/// from 1, and every `UpdateTheme` adds the next one.
pub const THEME_INITIAL_VERSION_NUMBER: i64 = 1;

/// The `AliasName` keyword that names a resource's latest version, whichever version
/// that is (model shape `com.amazonaws.quicksight#AliasName` admits it beside the
/// aliases a caller mints).
pub const ALIAS_LATEST: &str = "$LATEST";

/// The `AliasName` keyword that names a resource's published version. The model
/// documents it as not applying to themes.
pub const ALIAS_PUBLISHED: &str = "$PUBLISHED";

/// How long a new theme version stays in `CREATION_IN_PROGRESS` before the clock owes
/// it its terminal status. Chosen (QuickSight publishes no duration): long enough that
/// a caller reading the theme straight after the create still observes the non-terminal
/// status the create reported, short enough to be over by the time anything polls.
pub const THEME_CREATION_SPAN: Duration = Duration::from_secs(3);

/// How many themes one account may hold before `CreateTheme` refuses with the modelled
/// `LimitExceededException`. Same standing as [`MAX_DATA_SOURCES_PER_ACCOUNT`]: a
/// fidelity limit, not the process bound.
pub const MAX_THEMES_PER_ACCOUNT: usize = 1_000;

/// The TOTAL number of themes this emulator holds, across every account — the process
/// bound, for the same reason as [`MAX_DATA_SOURCES`].
pub const MAX_THEMES: usize = 10_000;

/// A stored theme: what `CreateTheme` persisted, which is what every later theme read
/// reports.
///
/// The versions are held in a list because a theme IS its version history — the
/// numbering starts at 1 and each later update appends — so a read for a particular
/// `VersionNumber`, and the latest-version read, both answer from this one record.
#[derive(Debug, Clone)]
pub struct ThemeRecord {
    pub arn: String,
    pub theme_id: String,
    pub name: String,
    pub created_time: DateTime,
    pub last_updated_time: DateTime,
    /// The theme's versions, in version-number order (so the last is the latest).
    pub versions: Vec<ThemeVersionRecord>,
    /// The aliases a caller minted for this theme, keyed by `AliasName`.
    ///
    /// An alias belongs to one theme and its name identifies it inside that theme, so
    /// it is held on the theme's own record: two themes of the same account can each
    /// carry an alias of the same name, pointing at versions of their own. Ordered by
    /// name so a read that pages through them has one stable order.
    pub aliases: BTreeMap<String, ThemeAliasRecord>,
    pub permissions: Vec<ResourcePermission>,
    pub tags: Vec<Tag>,
}

impl ThemeRecord {
    /// The theme's latest version — the one a read that names no version or alias
    /// reports. A theme always has at least the version its create minted.
    pub fn latest_version(&self) -> Option<&ThemeVersionRecord> {
        self.versions.last()
    }

    /// The theme's alias named `alias_name`, if a caller minted one.
    pub fn alias(&self, alias_name: &str) -> Option<&ThemeAliasRecord> {
        self.aliases.get(alias_name)
    }

    /// The theme's version numbered `version_number`, if it has one.
    pub fn version(&self, version_number: i64) -> Option<&ThemeVersionRecord> {
        self.versions
            .iter()
            .find(|version| version.version_number == version_number)
    }

    /// The modelled `Theme` structure this record describes, reporting `version` as
    /// the version member and that version's status as of `now`.
    pub fn to_theme(&self, version: &ThemeVersionRecord, now: SystemTime) -> Theme {
        Theme {
            arn: Some(self.arn.clone()),
            name: Some(self.name.clone()),
            theme_id: Some(self.theme_id.clone()),
            version: Some(version.to_theme_version(now)),
            created_time: Some(self.created_time),
            last_updated_time: Some(self.last_updated_time),
            r#type: Some(THEME_TYPE_CUSTOM.to_string()),
        }
    }
}

/// One version of a stored theme: the configuration the create or update was given,
/// under the version number that create or update minted.
#[derive(Debug, Clone)]
pub struct ThemeVersionRecord {
    pub version_number: i64,
    pub arn: String,
    pub description: Option<String>,
    pub base_theme_id: String,
    pub created_time: DateTime,
    pub configuration: ThemeConfiguration,
    /// When the clock owes this version its move out of `CREATION_IN_PROGRESS`.
    /// Stored rather than waited on: a reader materialises the transition the clock
    /// has already made due.
    pub creation_settles_at: SystemTime,
}

impl ThemeVersionRecord {
    /// This version's `ResourceStatus` as of `now`: the non-terminal creation status
    /// until the transition the clock owes falls due, `CREATION_SUCCESSFUL` from then
    /// on.
    pub fn status_at(&self, now: SystemTime) -> &'static str {
        if now >= self.creation_settles_at {
            RESOURCE_STATUS_CREATION_SUCCESSFUL
        } else {
            RESOURCE_STATUS_CREATION_IN_PROGRESS
        }
    }

    /// The modelled `ThemeVersion` structure this record describes, as of `now`.
    pub fn to_theme_version(&self, now: SystemTime) -> ThemeVersion {
        ThemeVersion {
            version_number: Some(self.version_number),
            arn: Some(self.arn.clone()),
            description: self.description.clone(),
            base_theme_id: Some(self.base_theme_id.clone()),
            created_time: Some(self.created_time),
            configuration: Some(self.configuration.clone()),
            // A version whose creation succeeded carries no error; nothing in this
            // emulator fails a theme creation.
            errors: None,
            status: Some(self.status_at(now).to_string()),
        }
    }
}

/// The prefix the model reserves on an `AliasName`: `CreateThemeAlias` documents that
/// an alias name can't begin with a `$`, because names that do are the service's own
/// keywords ([`ALIAS_LATEST`], [`ALIAS_PUBLISHED`]).
pub const ALIAS_RESERVED_PREFIX: char = '$';

/// How many aliases one theme may hold before a create refuses with the modelled
/// `LimitExceededException`. Same standing as [`MAX_THEMES_PER_ACCOUNT`]: a fidelity
/// limit on a map whose keys the caller chooses, not a published quota.
pub const MAX_THEME_ALIASES_PER_THEME: usize = 1_000;

/// A stored theme alias: the name a caller minted and the theme version it points at.
///
/// It holds no version of its own — an alias IS a name for one of the theme's
/// versions, so a read resolves it to that version rather than to a copy of it.
#[derive(Debug, Clone)]
pub struct ThemeAliasRecord {
    pub arn: String,
    pub alias_name: String,
    pub theme_version_number: i64,
}

impl ThemeAliasRecord {
    /// The modelled `ThemeAlias` structure this record describes.
    pub fn to_theme_alias(&self) -> ThemeAlias {
        ThemeAlias {
            arn: Some(self.arn.clone()),
            alias_name: Some(self.alias_name.clone()),
            theme_version_number: Some(self.theme_version_number),
        }
    }
}

/// The `FolderType` a folder has when the create names none. The model documents the
/// default: by default, folderType is SHARED.
pub const FOLDER_TYPE_SHARED: &str = "SHARED";

/// The `SharingModel` a folder has when the create names none. The model documents the
/// default: the default value for this parameter is ACCOUNT.
pub const SHARING_MODEL_ACCOUNT: &str = "ACCOUNT";

/// How many ancestors a folder may have. The modelled `Path` list — the shape
/// `Folder`'s `FolderPath` targets — admits at most 10 ARNs, so a folder nested deeper
/// than that could not be reported and is refused instead.
/// Source: smithy_query(query="shape", shape="Path").
pub const MAX_FOLDER_ANCESTORS: usize = 10;

/// How many folders one account may hold before `CreateFolder` refuses with the
/// modelled `LimitExceededException`. Same standing as
/// [`MAX_DATA_SOURCES_PER_ACCOUNT`]: a fidelity limit, not the process bound.
pub const MAX_FOLDERS_PER_ACCOUNT: usize = 1_000;

/// The TOTAL number of folders this emulator holds, across every account — the process
/// bound, for the same reason as [`MAX_DATA_SOURCES`].
pub const MAX_FOLDERS: usize = 10_000;

/// A stored folder: what `CreateFolder` persisted, which is what every later folder
/// read reports.
///
/// The folder carries no status: the modelled `Folder` structure declares none —
/// QuickSight hands a folder back ready to use — so there is no transition for the
/// clock to owe it.
#[derive(Debug, Clone)]
pub struct FolderRecord {
    pub arn: String,
    pub folder_id: String,
    /// The display name, absent when the create submitted none: `Name` is optional on
    /// `CreateFolder`, and a folder created without one has nothing to report.
    pub name: Option<String>,
    pub folder_type: String,
    pub sharing_model: String,
    /// The ARNs of this folder's ancestors, outermost first — the root-level folder of
    /// the chain at index 0 and the immediate parent last. Empty for a folder created
    /// with no `ParentFolderArn`, which is a root-level folder.
    pub ancestor_arns: Vec<String>,
    pub created_time: DateTime,
    pub last_updated_time: DateTime,
    pub permissions: Vec<ResourcePermission>,
    pub tags: Vec<Tag>,
}

impl FolderRecord {
    /// The modelled `Folder` structure this record describes.
    pub fn to_folder(&self) -> Folder {
        Folder {
            folder_id: Some(self.folder_id.clone()),
            arn: Some(self.arn.clone()),
            name: self.name.clone(),
            folder_type: Some(self.folder_type.clone()),
            // `Path` is a list of at least one ARN, so a root-level folder — which has
            // no ancestor to name — reports no path at all rather than an empty list
            // the model does not admit.
            folder_path: if self.ancestor_arns.is_empty() {
                None
            } else {
                Some(self.ancestor_arns.clone())
            },
            created_time: Some(self.created_time),
            last_updated_time: Some(self.last_updated_time),
            sharing_model: Some(self.sharing_model.clone()),
        }
    }
}

/// A stored IAM policy assignment: the rule `CreateIAMPolicyAssignment` persisted,
/// which is what every later read of that assignment reports.
///
/// The namespace is a field rather than part of the store key because the model makes
/// the name unique per account: the namespace is the address a later call must present
/// to reach this assignment.
///
/// The assignment carries no ARN and no timestamps: the modelled
/// `IAMPolicyAssignment` structure declares neither, so nothing is stored that no
/// read can report.
#[derive(Debug, Clone)]
pub struct IamPolicyAssignmentRecord {
    pub aws_account_id: String,
    pub namespace: String,
    /// The server-minted id of the assignment, stable for its lifetime.
    pub assignment_id: String,
    pub assignment_name: String,
    pub assignment_status: String,
    pub policy_arn: Option<String>,
    /// The users and groups the policy is assigned to, keyed the way the request
    /// keyed them; `None` for an assignment created against no identity.
    pub identities: Option<HashMap<String, Vec<String>>>,
}

impl IamPolicyAssignmentRecord {
    /// The modelled `IAMPolicyAssignment` structure this record describes.
    pub fn to_iam_policy_assignment(&self) -> IAMPolicyAssignment {
        IAMPolicyAssignment {
            aws_account_id: Some(self.aws_account_id.clone()),
            assignment_id: Some(self.assignment_id.clone()),
            assignment_name: Some(self.assignment_name.clone()),
            policy_arn: self.policy_arn.clone(),
            identities: self.identities.clone(),
            assignment_status: Some(self.assignment_status.clone()),
        }
    }
}

/// The `ExceptionResourceType` an IAM policy assignment's refusals report (model enum
/// `com.amazonaws.quicksight#ExceptionResourceType`).
pub const EXCEPTION_RESOURCE_TYPE_IAM_POLICY_ASSIGNMENT: &str = "IAMPOLICY_ASSIGNMENT";

/// How many IAM policy assignments one account may hold before a create is refused.
/// Same standing as [`MAX_DATA_SOURCES_PER_ACCOUNT`]: a fidelity limit, not the
/// process bound.
pub const MAX_IAM_POLICY_ASSIGNMENTS_PER_ACCOUNT: usize = 1_000;

/// The TOTAL number of IAM policy assignments this emulator holds, across every
/// account and namespace — the process bound, for the same reason as
/// [`MAX_DATA_SOURCES`]: every part of the key comes from the request.
pub const MAX_IAM_POLICY_ASSIGNMENTS: usize = 10_000;

/// The `AssignmentId` of a newly created IAM policy assignment. Server-minted: the
/// request names only the assignment's name, and the id is the service's own handle
/// on the rule.
pub fn next_assignment_id() -> String {
    uuid_shaped(next_minted_id())
}

/// The import mode whose data set QuickSight loads into SPICE, and so the one whose
/// creation starts an ingestion.
pub const IMPORT_MODE_SPICE: &str = "SPICE";

/// The page size a list operation uses when the request names no `MaxResults`: the
/// model's own ceiling for the member (`range 1..100`), so an unpaginated caller sees
/// everything a single page can hold.
pub const DEFAULT_MAX_RESULTS: usize = 100;

/// How many data sources one account may hold before `CreateDataSource` refuses with
/// the modelled `LimitExceededException`.
///
/// This is the emulator's own ceiling, of the same order as QuickSight's published
/// per-account data source quota. A caller that frees a slot with `DeleteDataSource`
/// can create again. It is a FIDELITY limit only: the account id is request-supplied,
/// so what bounds this process is [`MAX_DATA_SOURCES`].
pub const MAX_DATA_SOURCES_PER_ACCOUNT: usize = 1_000;

/// How many data sets one account may hold before `CreateDataSet` refuses with the
/// modelled `LimitExceededException`. Same standing as
/// [`MAX_DATA_SOURCES_PER_ACCOUNT`]: a fidelity limit, not the process bound.
pub const MAX_DATA_SETS_PER_ACCOUNT: usize = 1_000;

/// The TOTAL number of data sources this emulator holds, across every account.
///
/// This is the bound that holds for the process: both halves of the store key come
/// from the request — the account id is an `@httpLabel` the caller chooses — so a
/// per-account ceiling alone would be reset by varying the account id. A create loop
/// stops here however it varies its keys.
pub const MAX_DATA_SOURCES: usize = 10_000;

/// The TOTAL number of data sets this emulator holds, across every account — the
/// process bound, for the same reason as [`MAX_DATA_SOURCES`].
pub const MAX_DATA_SETS: usize = 10_000;

/// The TOTAL number of ingestions this emulator holds, across every account.
///
/// `CreateDataSet` is the only operation that inserts one, and it inserts at most one
/// per data set, so this ceiling is reached only if the data-set ceiling is; it is
/// checked explicitly all the same, so the bound on this map does not rest on
/// reasoning about another one.
pub const MAX_INGESTIONS: usize = 10_000;

/// The region of the resources a request addresses: the region of its SigV4
/// credential scope, which is the region the client signed for.
///
/// Falls back to `us-east-1` — the emulator's default region — for an unsigned or
/// unparseable scope, so an anonymous caller still gets a well-formed ARN.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| "us-east-1".to_string())
}

/// A QuickSight ARN: `arn:aws:quicksight:<region>:<account>:<resource>`.
///
/// The account segment is the `AwsAccountId` the request names — every QuickSight
/// operation takes it as a required, pattern-validated `@httpLabel`, and it is the
/// account that owns the resource, so two accounts' resources never share an ARN. A
/// client signed with this emulator's own credentials presents
/// [`::engine::emulator_account_id`] there.
pub fn quicksight_arn(region: &str, aws_account_id: &str, resource: &str) -> String {
    format!("arn:aws:quicksight:{region}:{aws_account_id}:{resource}")
}

/// The ARN of a data source: `.../datasource/<DataSourceId>`.
pub fn data_source_arn(region: &str, aws_account_id: &str, data_source_id: &str) -> String {
    quicksight_arn(region, aws_account_id, &format!("datasource/{data_source_id}"))
}

/// The ARN of a data set: `.../dataset/<DataSetId>`.
pub fn data_set_arn(region: &str, aws_account_id: &str, data_set_id: &str) -> String {
    quicksight_arn(region, aws_account_id, &format!("dataset/{data_set_id}"))
}

/// The ARN of one data set's ingestion: `.../dataset/<DataSetId>/ingestion/<IngestionId>`.
pub fn ingestion_arn(
    region: &str,
    aws_account_id: &str,
    data_set_id: &str,
    ingestion_id: &str,
) -> String {
    quicksight_arn(
        region,
        aws_account_id,
        &format!("dataset/{data_set_id}/ingestion/{ingestion_id}"),
    )
}

/// The ARN of one data set's refresh schedule:
/// `.../dataset/<DataSetId>/refresh-schedule/<ScheduleId>` — a refresh schedule is a
/// child of the data set it refreshes, so its ARN is scoped by that data set.
pub fn refresh_schedule_arn(
    region: &str,
    aws_account_id: &str,
    data_set_id: &str,
    schedule_id: &str,
) -> String {
    quicksight_arn(
        region,
        aws_account_id,
        &format!("dataset/{data_set_id}/refresh-schedule/{schedule_id}"),
    )
}

/// The ARN of a Q topic: `.../topic/<TopicId>`.
pub fn topic_arn(region: &str, aws_account_id: &str, topic_id: &str) -> String {
    quicksight_arn(region, aws_account_id, &format!("topic/{topic_id}"))
}

/// The ARN of a namespace: `.../namespace/<Namespace>`.
pub fn namespace_arn(region: &str, aws_account_id: &str, namespace: &str) -> String {
    quicksight_arn(region, aws_account_id, &format!("namespace/{namespace}"))
}

/// The ARN of a group: `.../group/<Namespace>/<GroupName>` — the resource
/// `CreateGroup`'s documentation names (`arn:aws:quicksight:::group/default/`).
pub fn group_arn(
    region: &str,
    aws_account_id: &str,
    namespace: &str,
    group_name: &str,
) -> String {
    quicksight_arn(
        region,
        aws_account_id,
        &format!("group/{namespace}/{group_name}"),
    )
}

/// The ARN of an IAM policy assignment: `arn:aws:quicksight::<account>:assignment/<AssignmentName>`.
///
/// The region segment is EMPTY — unlike every other QuickSight resource, an assignment
/// is addressed by its account and name alone, which is the form the service reports —
/// so this one is built with no region rather than with the request's.
pub fn iam_policy_assignment_arn(aws_account_id: &str, assignment_name: &str) -> String {
    quicksight_arn(
        "",
        aws_account_id,
        &format!("assignment/{assignment_name}"),
    )
}

/// The ARN of a folder: `.../folder/<FolderId>`.
pub fn folder_arn(region: &str, aws_account_id: &str, folder_id: &str) -> String {
    quicksight_arn(region, aws_account_id, &format!("folder/{folder_id}"))
}

/// The ARN of a theme: `.../theme/<ThemeId>`.
pub fn theme_arn(region: &str, aws_account_id: &str, theme_id: &str) -> String {
    quicksight_arn(region, aws_account_id, &format!("theme/{theme_id}"))
}

/// The ARN of one version of a theme: `.../theme/<ThemeId>/version/<VersionNumber>`.
pub fn theme_version_arn(
    region: &str,
    aws_account_id: &str,
    theme_id: &str,
    version_number: i64,
) -> String {
    quicksight_arn(
        region,
        aws_account_id,
        &format!("theme/{theme_id}/version/{version_number}"),
    )
}

/// The ARN of one alias of a theme: `.../theme/<ThemeId>/alias/<AliasName>` — an alias
/// is a child of the theme it names a version of, so its ARN is scoped by that theme.
pub fn theme_alias_arn(
    region: &str,
    aws_account_id: &str,
    theme_id: &str,
    alias_name: &str,
) -> String {
    quicksight_arn(
        region,
        aws_account_id,
        &format!("theme/{theme_id}/alias/{alias_name}"),
    )
}

/// The ARN of one topic's refresh: `.../topic/<TopicId>/refresh/<RefreshId>` — the
/// resource the `DescribeTopicRefresh` route addresses
/// (`/accounts/{AwsAccountId}/topics/{TopicId}/refresh/{RefreshId}`).
pub fn topic_refresh_arn(
    region: &str,
    aws_account_id: &str,
    topic_id: &str,
    refresh_id: &str,
) -> String {
    quicksight_arn(
        region,
        aws_account_id,
        &format!("topic/{topic_id}/refresh/{refresh_id}"),
    )
}

/// The source of every server-minted id in this service.
///
/// Process-wide rather than a field of the state struct on purpose: a reporting
/// operation also puts a `RequestId` in its response, and a read must not write to
/// the store it reports on.
static MINTED_IDS: AtomicU64 = AtomicU64::new(1);

/// A fresh id, unique for the life of the process.
fn next_minted_id() -> u64 {
    MINTED_IDS.fetch_add(1, Ordering::Relaxed)
}

/// `n` rendered as a well-formed RFC 4122 UUID string (version 4, variant 1) — the
/// shape AWS request and ingestion ids have. The value is the monotonic counter, not
/// randomness, so it is unique without being derived from the request.
fn uuid_shaped(n: u64) -> String {
    format!("00000000-0000-4000-8000-{n:012x}")
}

/// The `RequestId` to report for one response.
pub fn next_request_id() -> String {
    uuid_shaped(next_minted_id())
}

/// The `PrincipalId` of a newly created group: the service's own handle on the group,
/// which the request does not name, so the create mints it.
pub fn next_group_principal_id() -> String {
    uuid_shaped(next_minted_id())
}

/// The `IngestionId` for a newly started ingestion. Matches the model's
/// `IngestionId` pattern (`^[a-zA-Z0-9-_]+$`, 1..128).
pub fn next_ingestion_id() -> String {
    uuid_shaped(next_minted_id())
}

/// The `RefreshId` of a newly started topic refresh. Matches the model's
/// `ResourceId`, the shape the `DescribeTopicRefresh` label targets.
pub fn next_topic_refresh_id() -> String {
    uuid_shaped(next_minted_id())
}

/// `SystemTime::now` as the modelled timestamp type.
pub fn now_timestamp(now: SystemTime) -> DateTime {
    let millis = now
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    DateTime::from_millis(i64::try_from(millis).unwrap_or(i64::MAX))
}

/// Prefix of this service's `NextToken`s, so a token from somewhere else is rejected
/// rather than silently read as a position.
const PAGE_TOKEN_PREFIX: &str = "qs1-";

/// A `NextToken` resuming after the id `after`.
///
/// The token carries the last id of the page just returned, so the next page is the
/// ids ordered after it: inserting or deleting another resource between pages cannot
/// make a caller skip or repeat an item the way an index-based token would.
pub fn encode_page_token(after: &str) -> String {
    format!("{PAGE_TOKEN_PREFIX}{}", ::hex::encode(after))
}

/// The id a `NextToken` resumes after, or `None` when the token is not one this
/// service issued.
pub fn decode_page_token(token: &str) -> Option<String> {
    let body = token.strip_prefix(PAGE_TOKEN_PREFIX)?;
    String::from_utf8(::hex::decode(body).ok()?).ok()
}

/// One page of `items` (ordered by the id `id_of` reads), and the `NextToken` to read
/// the page after it.
///
/// `max_results` is the page size the request asked for; the model constrains it to
/// 1..100 before a handler sees it, and an absent one means [`DEFAULT_MAX_RESULTS`].
pub fn page_of<'a, T>(
    items: &[&'a T],
    id_of: impl Fn(&T) -> &str,
    after: Option<&str>,
    max_results: Option<i32>,
) -> (Vec<&'a T>, Option<String>) {
    let start = match after {
        // Resume *after* the recorded id. `partition_point` also does the right thing
        // when that id has since been deleted: the page continues from the next id in
        // order rather than restarting.
        Some(after) => items.partition_point(|item| id_of(item) <= after),
        None => 0,
    };
    let page_size = max_results
        .and_then(|requested| usize::try_from(requested).ok())
        .filter(|requested| *requested > 0)
        .unwrap_or(DEFAULT_MAX_RESULTS);
    let end = start.saturating_add(page_size).min(items.len());
    let page: Vec<&'a T> = items.get(start..end).unwrap_or_default().to_vec();
    let next_token = match page.last() {
        Some(last) if end < items.len() => Some(encode_page_token(id_of(last))),
        _ => None,
    };
    (page, next_token)
}

/// How many grants one resource's permission list may hold: the model's own ceiling
/// for `ResourcePermissionList` (`length 1..64`), the shape every `Permissions`
/// member targets — so an update that would leave a resource holding more grants than
/// its list can carry is refused rather than stored.
pub const MAX_RESOURCE_PERMISSIONS: usize = 64;

/// One resource's permission list after granting `grant` and revoking `revoke`.
///
/// A permission list holds one entry per principal, so an update is applied per
/// principal rather than by appending entries: a grant adds its actions to that
/// principal's entry (creating the entry when the principal holds nothing yet), and a
/// revoke takes its actions away, dropping the entry once the principal is left
/// holding none. Revokes are applied before grants, so a call that names one principal
/// in both leaves it holding what the call granted. Entry order is the order
/// principals were first granted in and each entry's actions keep the order they were
/// granted in, so repeated reads report the same list; granting an action a principal
/// already holds, or revoking one it does not, changes nothing.
pub fn apply_permission_updates(
    current: &[ResourcePermission],
    grant: &[ResourcePermission],
    revoke: &[ResourcePermission],
) -> Vec<ResourcePermission> {
    let mut updated: Vec<ResourcePermission> = current.to_vec();

    for revoked in revoke {
        let Some(entry) = updated
            .iter_mut()
            .find(|entry| entry.principal == revoked.principal)
        else {
            continue;
        };
        entry
            .actions
            .retain(|action| !revoked.actions.contains(action));
    }

    for granted in grant {
        let at = match updated
            .iter()
            .position(|entry| entry.principal == granted.principal)
        {
            Some(at) => at,
            None => {
                updated.push(ResourcePermission {
                    principal: granted.principal.clone(),
                    actions: Vec::new(),
                });
                updated.len() - 1
            }
        };
        for action in &granted.actions {
            let entry = &mut updated[at];
            if !entry.actions.contains(action) {
                entry.actions.push(action.clone());
            }
        }
    }

    // A principal left with no action holds no grant on the resource at all, so its
    // entry goes rather than staying as an empty one — the modelled `ActionList`
    // carries at least one action.
    updated.retain(|entry| !entry.actions.is_empty());
    updated
}

/// The ARN of the data source a `PhysicalTable` reads, whichever kind of table it is.
pub fn physical_table_data_source_arn(table: &PhysicalTable) -> &str {
    match table {
        PhysicalTable::RelationalTable(table) => &table.data_source_arn,
        PhysicalTable::CustomSql(table) => &table.data_source_arn,
        PhysicalTable::S3Source(table) => &table.data_source_arn,
        PhysicalTable::SaaSTable(table) => &table.data_source_arn,
        PhysicalTable::FileSource(table) => &table.data_source_arn,
    }
}

/// The columns a `PhysicalTable` presents, whichever kind of table it is.
pub fn physical_table_input_columns(table: &PhysicalTable) -> &[InputColumn] {
    match table {
        PhysicalTable::RelationalTable(table) => &table.input_columns,
        PhysicalTable::CustomSql(table) => table.columns.as_deref().unwrap_or(&[]),
        PhysicalTable::S3Source(table) => &table.input_columns,
        PhysicalTable::SaaSTable(table) => &table.input_columns,
        PhysicalTable::FileSource(table) => &table.input_columns,
    }
}

/// The columns a data set projects, derived from the tables it is built on.
///
/// A data set's output columns are the columns of its physical tables: taken in
/// physical-table-key order, each table's columns in the order the table declares
/// them, and de-duplicated by name (one column per name, as a projection has). The
/// transforms a `LogicalTableMap` can apply on top are stored and returned verbatim
/// but are not folded in here; that is `UpdateDataSet`'s and the transform
/// operations' own gap.
pub fn derive_output_columns(
    physical_table_map: &HashMap<String, PhysicalTable>,
) -> Vec<OutputColumn> {
    let mut table_keys: Vec<&String> = physical_table_map.keys().collect();
    table_keys.sort();

    let mut columns: Vec<OutputColumn> = Vec::new();
    // De-duplication is a hash lookup per column, so the work is linear in the total
    // number of columns the request carries rather than quadratic in it.
    let mut seen: HashSet<&str> = HashSet::new();
    for key in table_keys {
        let Some(table) = physical_table_map.get(key) else {
            continue;
        };
        for column in physical_table_input_columns(table) {
            if !seen.insert(column.name.as_str()) {
                continue;
            }
            columns.push(OutputColumn {
                name: Some(column.name.clone()),
                id: column.id.clone(),
                // `InputColumn` carries no description; QuickSight only has one once
                // a transform or UpdateDataSet sets it.
                description: None,
                r#type: Some(column.r#type.clone()),
                sub_type: column.sub_type.clone(),
            });
        }
    }
    columns
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsQuicksightQuicksight20180401State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsQuicksightQuicksight20180401State>)
}
